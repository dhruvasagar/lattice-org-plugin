//! The export itself, on the events seam: read the buffer, render it, write
//! the file.
//!
//! The grammar seam only checks the target and emits [`EV_REQUESTED`]; this
//! side does the work and posts its result, so a whole-file render never runs
//! on the keystroke. The buffer is read with `buffer-text`, so an export sees
//! the buffer as it is, unsaved edits included, as emacs's does.
//!
//! The renderers here are placeholders: the source in a verbatim block,
//! inside each format's document shell. The parser and the real transcoders
//! replace them; the request, the seam crossing and the sink do not change.

use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::types::{
    EchoLevel, EchoPayload, Effect, FileAnchor, WriteToFilePayload,
};

/// The plugin event carrying a [`Request`] from the command to the events
/// seam.
pub(crate) const EV_REQUESTED: &str = "org/export-requested";

/// An output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Backend {
    Html,
    Markdown,
    Latex,
}

impl Backend {
    /// The output file's extension.
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Backend::Html => "html",
            Backend::Markdown => "md",
            Backend::Latex => "tex",
        }
    }
}

/// One export, as the command hands it across.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Request {
    /// The org buffer, read when the export runs.
    pub(crate) buffer: u32,
    /// Its file, which names the output.
    pub(crate) source: String,
    /// Where the output goes, already checked with `can-write-file`.
    pub(crate) target: String,
    pub(crate) backend: Backend,
    /// Export the subtree around [`line`](Self::line) only.
    pub(crate) subtree: bool,
    /// Leave out the preamble or `<head>`.
    pub(crate) body_only: bool,
    /// The cursor's line when the command ran.
    pub(crate) line: u32,
}

/// The output's path: the source's, with the backend's extension in place of
/// `.org` (or after a name that has none).
pub(crate) fn target_path(source: &str, backend: Backend) -> String {
    let stem = source
        .rsplit_once('/')
        .map_or(source, |(_, name)| name)
        .rsplit_once('.')
        .map(|(stem, _)| stem.len());
    let base = match stem {
        Some(len) => {
            let name_start = source.rfind('/').map_or(0, |i| i + 1);
            &source[..name_start + len]
        }
        None => source,
    };
    format!("{base}.{}", backend.extension())
}

/// The last component of `path`, for the echo area.
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// Hand `request` to the events seam. Returns at once; `Err` is why it was
/// not sent, for the command to echo.
pub(crate) fn request(request: &Request) -> Result<(), String> {
    let bytes = rmp_serde::to_vec(request)
        .map_err(|e| format!("org: the export request did not encode: {e}"))?;
    host_services::emit_event(EV_REQUESTED, &bytes);
    Ok(())
}

/// `on-event` for [`EV_REQUESTED`]: render and write.
pub(crate) fn on_requested(payload: &[u8]) {
    let Ok(request) = rmp_serde::from_slice::<Request>(payload) else {
        return;
    };
    post(run(&request, host_services::buffer_text(request.buffer)));
}

/// The effects one export ends in, given the buffer's text (`None` when it
/// closed before the export read it).
fn run(request: &Request, text: Option<String>) -> Vec<Effect> {
    let Some(text) = text else {
        return vec![echo(
            EchoLevel::Warn,
            format!(
                "org: {} closed before it was exported",
                file_name(&request.source)
            ),
        )];
    };
    let lines: Vec<&str> = text.lines().collect();
    let body = if request.subtree {
        match crate::babel::block::subtree_at(&lines, request.line) {
            Some((first, end)) => lines[first as usize..end as usize].join("\n"),
            None => {
                return vec![echo(
                    EchoLevel::Warn,
                    "org: a subtree export needs the cursor under a headline",
                )]
            }
        }
    } else {
        text.clone()
    };
    let title = title_of(&lines, &request.source);
    let output = render(request.backend, &title, &body, request.body_only);
    vec![
        Effect::WriteToFile(WriteToFilePayload {
            path: request.target.clone(),
            anchor: FileAnchor::Whole,
            text: output,
            cut: None,
            // The output goes beside its source, whose directory exists.
            create_parents: false,
            // On disk is the point: a browser, a forge or a compiler reads it.
            save: true,
        }),
        echo(
            EchoLevel::Info,
            format!("Exported {}", file_name(&request.target)),
        ),
    ]
}

/// The document's title: `#+TITLE:`, else the file's name without `.org`.
fn title_of(lines: &[&str], source: &str) -> String {
    lines
        .iter()
        .find_map(|l| {
            let rest = l.trim_start();
            let (key, value) = rest.split_once(':')?;
            key.eq_ignore_ascii_case("#+title")
                .then(|| value.trim().to_string())
        })
        .unwrap_or_else(|| {
            let name = file_name(source);
            name.strip_suffix(".org").unwrap_or(name).to_string()
        })
}

/// The placeholder renderers: `body` verbatim, inside the format's shell.
fn render(backend: Backend, title: &str, body: &str, body_only: bool) -> String {
    match backend {
        Backend::Html => {
            let block = format!("<pre class=\"example\">\n{}\n</pre>\n", html_escape(body));
            if body_only {
                return block;
            }
            format!(
                "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
                 <title>{}</title>\n</head>\n<body>\n<div id=\"content\">\n\
                 <h1 class=\"title\">{}</h1>\n{block}</div>\n</body>\n</html>\n",
                html_escape(title),
                html_escape(title)
            )
        }
        Backend::Markdown => {
            let block = format!("```org\n{body}\n```\n");
            if body_only {
                return block;
            }
            format!("# {title}\n\n{block}")
        }
        Backend::Latex => {
            let block = format!("\\begin{{verbatim}}\n{body}\n\\end{{verbatim}}\n");
            if body_only {
                return block;
            }
            format!(
                "\\documentclass{{article}}\n\\title{{{title}}}\n\\begin{{document}}\n\
                 \\maketitle\n{block}\\end{{document}}\n"
            )
        }
    }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

fn echo(level: EchoLevel, text: impl Into<String>) -> Effect {
    Effect::Echo(EchoPayload {
        level,
        text: text.into(),
    })
}

fn post(effects: Vec<Effect>) {
    // `err` only when the editor wired nowhere to post to; then there is no
    // one to tell either.
    let _ = host_services::post_effects(&effects);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(backend: Backend) -> Request {
        Request {
            buffer: 1,
            source: "/notes/plan.org".to_string(),
            target: target_path("/notes/plan.org", backend),
            backend,
            subtree: false,
            body_only: false,
            line: 0,
        }
    }

    fn written(effects: &[Effect]) -> (&str, &str) {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::WriteToFile(w) => Some((w.path.as_str(), w.text.as_str())),
                _ => None,
            })
            .expect("a write")
    }

    #[test]
    fn the_output_goes_beside_the_source_with_the_backends_extension() {
        assert_eq!(target_path("/a/plan.org", Backend::Html), "/a/plan.html");
        assert_eq!(target_path("/a/plan.org", Backend::Markdown), "/a/plan.md");
        assert_eq!(
            target_path("/a/v1.2/README", Backend::Latex),
            "/a/v1.2/README.tex"
        );
        assert_eq!(target_path("/a/x.y.org", Backend::Html), "/a/x.y.html");
    }

    /// The write replaces the target whole and saves it: the file is for
    /// another reader.
    #[test]
    fn an_export_writes_the_whole_file_and_saves_it() {
        let effects = run(&request(Backend::Html), Some("* One <&>\n".to_string()));
        let Some(Effect::WriteToFile(w)) = effects.first() else {
            panic!("the write comes first: {effects:?}");
        };
        assert!(matches!(w.anchor, FileAnchor::Whole) && w.save);
        assert_eq!(w.path, "/notes/plan.html");
        assert!(w.text.contains("* One &lt;&amp;&gt;"), "{}", w.text);
        assert!(w.text.contains("<title>plan</title>"), "{}", w.text);
    }

    #[test]
    fn the_title_keyword_names_the_document() {
        let effects = run(
            &request(Backend::Markdown),
            Some("#+title: The Plan\n* One\n".to_string()),
        );
        assert!(written(&effects).1.starts_with("# The Plan\n"));
    }

    #[test]
    fn body_only_leaves_out_the_shell() {
        let mut r = request(Backend::Latex);
        r.body_only = true;
        let effects = run(&r, Some("* One\n".to_string()));
        let text = written(&effects).1;
        assert!(text.starts_with("\\begin{verbatim}"), "{text}");
        assert!(!text.contains("documentclass"));
    }

    #[test]
    fn a_subtree_export_takes_the_headline_around_the_cursor() {
        let mut r = request(Backend::Markdown);
        r.subtree = true;
        r.body_only = true;
        r.line = 2;
        let effects = run(
            &r,
            Some("* One\nx\n* Two\ny\n** Deep\n* Three\n".to_string()),
        );
        assert_eq!(written(&effects).1, "```org\n* Two\ny\n** Deep\n```\n");
        r.line = 0;
        let outside = run(&r, Some("text\n* One\n".to_string()));
        assert!(matches!(&outside[..], [Effect::Echo(_)]), "{outside:?}");
    }

    #[test]
    fn a_buffer_closed_before_the_export_says_so() {
        let effects = run(&request(Backend::Html), None);
        assert!(matches!(&effects[..], [Effect::Echo(e)] if e.text.contains("plan.org")));
    }

    #[test]
    fn a_request_survives_the_crossing() {
        let r = request(Backend::Latex);
        let bytes = rmp_serde::to_vec(&r).unwrap();
        assert_eq!(rmp_serde::from_slice::<Request>(&bytes).unwrap(), r);
    }
}

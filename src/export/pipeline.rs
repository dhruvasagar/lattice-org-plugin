//! The export itself, on the events seam: read the buffer, parse it, render
//! it, write the file.
//!
//! The grammar seam only checks the target and emits [`EV_REQUESTED`]; this
//! side does the work and posts its result, so no export runs on the
//! keystroke. The buffer is read once, with `buffer-text`, when the request
//! arrives: an export sees the buffer as it was then, unsaved edits
//! included, as emacs's does, and edits made while it runs do not reach it.
//!
//! **Stepped.** One events-seam call has a fuel budget, and parsing a large
//! file does not fit in it, nor does rendering one (lattice's
//! `org-export.md` §7). So a [`Run`] does about [`STEP_BYTES`] of work per
//! call and emits [`EV_STEP`] to itself for the next, the way the roam scan
//! walks a corpus: first the parse, a chunk of source per step, then the
//! render, a run of headlines per step.
//! Chunks are cut at headlines, where no org element can continue, so the
//! chunked parse is the whole-file parse. Runs live in guest memory between steps: a new export of the same
//! target replaces the one in flight, and an export of another runs beside
//! it.
//!
//! The renderers are placeholders: the parse tree, in each format's document
//! shell. The transcoders replace them; the request, the crossing, the steps
//! and the sink do not change.

use std::cell::{Cell, RefCell};
use std::ops::Range;

use super::filter::{self, Selection, View};
use super::include::Includes;
use super::options::Options;
use super::parse::{self, Context, Doc, ElementKind, FlatSection, Headline};
use super::targets::Targets;
use super::Host;
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::types::{
    EchoLevel, EchoPayload, Effect, FileAnchor, WriteToFilePayload,
};
use crate::time_format::When;
use std::rc::Rc;

/// The plugin event carrying a [`Request`] from the command to the events
/// seam.
pub(crate) const EV_REQUESTED: &str = "org/export-requested";

/// The plugin event a [`Run`] emits to itself to take its next step; the
/// payload is the run's id.
pub(crate) const EV_STEP: &str = "org/export-step";

/// How much one step does: the source a parse step reads, and the output a
/// render step writes. A parse chunk ends at the first headline past this,
/// and a render step at the first headline past it, so one section larger
/// than it is one step: within a section there is no line an element cannot
/// cross.
///
/// The spike measured about 7,000 fuel per line of real org for parse,
/// objects and render together; 64 KiB is a little over a tenth of the event
/// budget at that rate, leaving room for denser text.
const STEP_BYTES: usize = 64 * 1024;

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

thread_local! {
    /// The exports in flight, oldest first.
    static RUNS: RefCell<Vec<Run>> = const { RefCell::new(Vec::new()) };
    static NEXT_RUN: Cell<u64> = const { Cell::new(1) };
}

/// `on-event` for [`EV_REQUESTED`]: read the buffer and start a run.
pub(crate) fn on_requested(payload: &[u8]) {
    let Ok(request) = rmp_serde::from_slice::<Request>(payload) else {
        return;
    };
    let text = host_services::buffer_text(request.buffer);
    let id = NEXT_RUN.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    let now = When {
        local_secs: crate::local_now_secs(),
    };
    match Run::new(
        id,
        request,
        text,
        crate::todo_keywords(),
        now,
        Rc::new(super::Guest),
    ) {
        Ok(run) => {
            RUNS.with(|runs| {
                let mut runs = runs.borrow_mut();
                // A second export to the same file wins: the first would only
                // write what the second overwrites.
                runs.retain(|r| r.request.target != run.request.target);
                runs.push(run);
            });
            host_services::emit_event(EV_STEP, id.to_string().as_bytes());
        }
        Err(effects) => post(effects),
    }
}

/// `on-event` for [`EV_STEP`]: one step of the run it names.
pub(crate) fn on_step(payload: &[u8]) {
    let Some(id) = std::str::from_utf8(payload)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return;
    };
    let step = RUNS.with(|runs| {
        let mut runs = runs.borrow_mut();
        // Absent when a newer export of its target replaced it.
        let at = runs.iter().position(|r| r.id == id)?;
        match runs[at].step() {
            Step::More => Some(Step::More),
            Step::Done(effects) => {
                runs.remove(at);
                Some(Step::Done(effects))
            }
        }
    });
    match step {
        Some(Step::More) => host_services::emit_event(EV_STEP, payload),
        Some(Step::Done(effects)) => post(effects),
        None => {}
    }
}

/// What a step leaves to do.
#[derive(Debug)]
pub(crate) enum Step {
    /// Another step is needed.
    More,
    /// The run is over; post these.
    Done(Vec<Effect>),
}

/// One export in flight.
pub(crate) struct Run {
    id: u64,
    request: Request,
    /// The buffer's text, as it was when the export was asked for.
    text: String,
    ctx: Context,
    /// The file's `#+INCLUDE:`s while they are being read; `text` is the
    /// expanded text once this is `None`.
    includes: Option<Includes>,
    /// The cursor's line in `text`: the request's, moved past what the
    /// includes above it became.
    line: u32,
    /// What each parse step reads: a byte range of `text`, starting at a
    /// headline or at the top of the file, and its first line. Planned once
    /// the includes are in.
    chunks: Vec<(Range<usize>, u32)>,
    /// The next chunk to parse.
    next: usize,
    sections: Vec<FlatSection>,
    /// The file's keywords, in order, gathered as the chunks are parsed.
    keywords: Vec<(String, String)>,
    /// When the export was asked for: `{{{time}}}` reads it.
    now: When,
    /// The link targets, gathered as the chunks are parsed.
    targets: Targets,
    host: Rc<dyn Host>,
    /// Set once the parse is done.
    render: Option<Render>,
}

/// The render phase: the tree, its options, where the walk is, and the
/// output so far.
struct Render {
    doc: Doc,
    view: View,
    selection: Selection,
    /// The next headline to visit, as indices from `doc.headlines` down
    /// through `children`; empty once the walk is over.
    next: Vec<usize>,
    out: String,
}

impl Run {
    /// A run over `text` (`None` when the buffer closed before the export
    /// read it), or the effects that say why there is none.
    ///
    /// The whole file is parsed even for a subtree export: the file's
    /// keywords (`#+OPTIONS:`, `#+TITLE:`, `#+EXCLUDE_TAGS:`…) apply to a
    /// subtree too, and only the parse tells a keyword from a line inside an
    /// example block that looks like one.
    pub(crate) fn new(
        id: u64,
        request: Request,
        text: Option<String>,
        configured_keywords: Vec<String>,
        now: When,
        host: Rc<dyn Host>,
    ) -> Result<Self, Vec<Effect>> {
        let Some(text) = text else {
            return Err(vec![echo(
                EchoLevel::Warn,
                format!(
                    "org: {} closed before it was exported",
                    file_name(&request.source)
                ),
            )]);
        };
        // The file's own `#+TODO:` lines decide its keywords, as in emacs,
        // where the buffer's settings are read before anything is included.
        let ctx = Context::for_file(&text, configured_keywords);
        let includes = Includes::new(&text, &request.source, ctx.todo_keywords.clone());
        let (includes, chunks) = if includes.done() {
            (None, chunks(&text, STEP_BYTES))
        } else {
            (Some(includes), Vec::new())
        };
        let line = request.line;
        Ok(Run {
            id,
            request,
            ctx,
            includes,
            line,
            chunks,
            next: 0,
            sections: Vec::new(),
            keywords: Vec::new(),
            now,
            targets: Targets::default(),
            host,
            render: None,
            text,
        })
    }

    /// One step: parse the next chunk, or prepare the render, or render the
    /// next headlines, or finish the file and write it.
    pub(crate) fn step(&mut self) -> Step {
        if let Some(includes) = self.includes.as_mut() {
            if !includes.done() {
                let host = Rc::clone(&self.host);
                if let Err(why) = includes.step(&|path| host.read_file(path)) {
                    return Step::Done(vec![echo(EchoLevel::Error, why)]);
                }
                return Step::More;
            }
            if let Some(includes) = self.includes.take() {
                let expanded = includes.finish();
                self.line = expanded.line(self.request.line);
                self.text = expanded.text;
                self.chunks = chunks(&self.text, STEP_BYTES);
            }
            return Step::More;
        }
        if let Some((bytes, first_line)) = self.chunks.get(self.next).cloned() {
            let lines: Vec<&str> = self.text[bytes].lines().collect();
            let parsed = parse::parse_chunk(&lines, first_line, &self.ctx);
            for section in &parsed {
                self.targets.collect(section);
                self.keywords
                    .extend(section.elements.iter().filter_map(|e| match &e.kind {
                        ElementKind::Keyword { key, value } => Some((key.clone(), value.clone())),
                        _ => None,
                    }));
            }
            self.sections.extend(parsed);
            self.next += 1;
            return Step::More;
        }
        let backend = self.request.backend;
        let Some(render) = self.render.as_mut() else {
            return match self.prepare() {
                Ok(render) => {
                    self.render = Some(render);
                    Step::More
                }
                Err(effects) => Step::Done(effects),
            };
        };
        if !render.next.is_empty() {
            let mut body = String::new();
            while !render.next.is_empty() && body.len() < STEP_BYTES {
                let h = at(&render.doc, &render.next);
                let visit = filter::visits(h, &render.selection, &render.view.options);
                if visit {
                    let depth = render.next.len() - 1;
                    let own = filter::own(h, &mut render.view);
                    anchor_line(&mut body, &render.view, h.line, depth);
                    parse::dump::headline(&mut body, &own, depth);
                }
                advance(&render.doc, &mut render.next, visit);
            }
            if let Some(why) = render.view.error() {
                return Step::Done(vec![echo(EchoLevel::Error, why.to_string())]);
            }
            escape_into(backend, &body, &mut render.out);
            if !render.next.is_empty() {
                return Step::More;
            }
        }
        let mut output = std::mem::take(&mut render.out);
        output.push_str(shell_tail(backend, self.request.body_only));
        let target = file_name(&self.request.target);
        let ignored = &render.view.options.ignored;
        let done = if ignored.is_empty() {
            echo(EchoLevel::Info, format!("Exported {target}"))
        } else {
            echo(
                EchoLevel::Warn,
                format!(
                    "Exported {target}; not supported, so left out: {}",
                    ignored.join(", ")
                ),
            )
        };
        Step::Done(vec![
            Effect::WriteToFile(WriteToFilePayload {
                path: self.request.target.clone(),
                anchor: FileAnchor::Whole,
                text: output,
                cut: None,
                // The output goes beside its source, whose directory exists.
                create_parents: false,
                // On disk is the point: a browser, a forge or a compiler reads
                // it.
                save: true,
            }),
            done,
        ])
    }

    /// The step between the parse and the render: build the tree, take the
    /// subtree when that is what was asked for, resolve the options, and
    /// start the output. `Err` when a subtree was asked for and the cursor is
    /// under no headline.
    fn prepare(&mut self) -> Result<Render, Vec<Effect>> {
        let mut doc = parse::assemble(std::mem::take(&mut self.sections));
        let name = file_name(&self.request.source);
        let options = if self.request.subtree {
            let Some(headline) = take_subtree(&mut doc, self.line) else {
                return Err(vec![echo(
                    EchoLevel::Warn,
                    "org: a subtree export needs the cursor under a headline",
                )]);
            };
            let options = Options::resolve(
                &self.keywords,
                Some((&headline.raw_title, &headline.properties)),
                name,
            );
            doc = Doc {
                preamble: Vec::new(),
                headlines: vec![headline],
            };
            options
        } else {
            Options::resolve(&self.keywords, None, name)
        };
        let selection = filter::selection(&doc, &options);
        let mut targets = std::mem::take(&mut self.targets);
        targets.set_live(
            filter::live(&doc, &selection, &options),
            !self.request.subtree,
        );
        let mut view = View::new(
            options,
            self.keywords.clone(),
            self.now,
            targets,
            Rc::clone(&self.host),
        );
        let backend = self.request.backend;
        let title = view.options.title.clone();
        let title = filter::plain(&filter::objects(&title, &mut view));
        let mut out = shell_head(backend, &title, self.request.body_only);
        let mut body = parse::dump::options(&view.options);
        let preamble = filter::elements(&doc.preamble, &mut view);
        parse::dump::preamble(&mut body, &preamble);
        if let Some(why) = view.error() {
            return Err(vec![echo(EchoLevel::Error, why.to_string())]);
        }
        escape_into(backend, &body, &mut out);
        let next = if doc.headlines.is_empty() {
            Vec::new()
        } else {
            vec![0]
        };
        Ok(Render {
            doc,
            view,
            selection,
            next,
            out,
        })
    }
}

/// The placeholder's line naming the anchor a headline's links go to.
fn anchor_line(out: &mut String, view: &View, line: u32, depth: usize) {
    out.push_str(&"  ".repeat(depth));
    out.push_str("anchor ");
    out.push_str(&view.targets().headline_anchor(line));
    out.push('\n');
}

/// Take out of `doc` the innermost headline whose subtree holds `line`.
fn take_subtree(doc: &mut Doc, line: u32) -> Option<Headline> {
    // The last of a run of siblings that starts at or before `line` is the
    // one whose subtree holds it.
    let pick = |list: &[Headline]| list.iter().rposition(|h| h.line <= line);
    let mut path = vec![pick(&doc.headlines)?];
    while let Some(i) = pick(&at(doc, &path).children) {
        path.push(i);
    }
    let last = path.pop()?;
    let siblings = if path.is_empty() {
        &mut doc.headlines
    } else {
        let mut h = &mut doc.headlines[path[0]];
        for &i in &path[1..] {
            h = &mut h.children[i];
        }
        &mut h.children
    };
    Some(siblings.remove(last))
}

/// The headline at `path`, which [`advance`] keeps valid.
fn at<'d>(doc: &'d Doc, path: &[usize]) -> &'d Headline {
    let mut h = &doc.headlines[path[0]];
    for &i in &path[1..] {
        h = &h.children[i];
    }
    h
}

/// Move `path` to the next headline in document order: the first child when
/// `descend` and there is one, else the next sibling of the nearest ancestor
/// that has one. Empty at the end.
fn advance(doc: &Doc, path: &mut Vec<usize>, descend: bool) {
    if descend && !at(doc, path).children.is_empty() {
        path.push(0);
        return;
    }
    while let Some(i) = path.pop() {
        let siblings = if path.is_empty() {
            doc.headlines.len()
        } else {
            at(doc, path).children.len()
        };
        if i + 1 < siblings {
            path.push(i + 1);
            return;
        }
    }
}

/// Cut `text` into chunks of about `budget` bytes, each starting at a
/// headline (or at the top of the file), as `(bytes, first line)`. One pass;
/// an empty text is one empty chunk, so a run always takes a step.
fn chunks(text: &str, budget: usize) -> Vec<(Range<usize>, u32)> {
    let mut out = Vec::new();
    let mut from = 0;
    let mut first = 0;
    let mut at = 0;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        if at - from >= budget && crate::headline::headline_level(line).is_some() {
            out.push((from..at, first));
            from = at;
            first = i as u32;
        }
        at += line.len();
    }
    out.push((from..at, first));
    out
}

/// The placeholder renderers: the parse tree, verbatim inside the format's
/// shell, written in three parts so each step escapes only what it adds.
fn shell_head(backend: Backend, title: &str, body_only: bool) -> String {
    match (backend, body_only) {
        (Backend::Html, true) => "<pre class=\"example\">\n".to_string(),
        (Backend::Html, false) => {
            let mut escaped = String::new();
            html_escape_into(title, &mut escaped);
            let title = escaped;
            format!(
                "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
                 <title>{title}</title>\n</head>\n<body>\n<div id=\"content\">\n\
                 <h1 class=\"title\">{title}</h1>\n<pre class=\"example\">\n"
            )
        }
        (Backend::Markdown, true) => "```\n".to_string(),
        (Backend::Markdown, false) => format!("# {title}\n\n```\n"),
        (Backend::Latex, true) => "\\begin{verbatim}\n".to_string(),
        (Backend::Latex, false) => format!(
            "\\documentclass{{article}}\n\\title{{{title}}}\n\\begin{{document}}\n\
             \\maketitle\n\\begin{{verbatim}}\n"
        ),
    }
}

/// `body` as the shell's verbatim block holds it.
fn escape_into(backend: Backend, body: &str, out: &mut String) {
    match backend {
        Backend::Html => html_escape_into(body, out),
        Backend::Markdown | Backend::Latex => out.push_str(body),
    }
}

fn shell_tail(backend: Backend, body_only: bool) -> &'static str {
    match (backend, body_only) {
        (Backend::Html, true) => "</pre>\n",
        (Backend::Html, false) => "</pre>\n</div>\n</body>\n</html>\n",
        (Backend::Markdown, _) => "```\n",
        (Backend::Latex, true) => "\\end{verbatim}\n",
        (Backend::Latex, false) => "\\end{verbatim}\n\\end{document}\n",
    }
}

fn html_escape_into(s: &str, out: &mut String) {
    out.reserve(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
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

    /// `text` without the placeholder's anchor lines, for tests about which
    /// headlines are exported rather than what they are called.
    fn without_anchors(text: &str) -> String {
        text.split_inclusive('\n')
            .filter(|l| !l.trim_start().starts_with("anchor "))
            .collect()
    }

    /// A host over an in-memory file system, with no roam index.
    struct Files(std::collections::HashMap<String, String>);

    impl Host for Files {
        fn read_file(&self, path: &str) -> Result<String, String> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| "missing".to_string())
        }
        fn id_exists(&self, _: &str) -> Option<bool> {
            None
        }
    }

    /// Run `request` over `text` to the end, and the steps it took.
    fn run_counting(request: &Request, text: Option<String>) -> (Vec<Effect>, usize) {
        run_with_files(request, text, &[])
    }

    /// [`run_counting`] where an include may read `files`.
    fn run_with_files(
        request: &Request,
        text: Option<String>,
        files: &[(&str, &str)],
    ) -> (Vec<Effect>, usize) {
        let host = Rc::new(Files(
            files
                .iter()
                .map(|(p, t)| (p.to_string(), t.to_string()))
                .collect(),
        ));
        let keywords = vec!["TODO".to_string(), "DONE".to_string()];
        let mut run = match Run::new(
            1,
            request.clone(),
            text,
            keywords,
            When { local_secs: 0 },
            host,
        ) {
            Ok(run) => run,
            Err(effects) => return (effects, 0),
        };
        let mut steps = 0;
        loop {
            steps += 1;
            if let Step::Done(effects) = run.step() {
                return (effects, steps);
            }
        }
    }

    fn run(request: &Request, text: Option<String>) -> Vec<Effect> {
        run_counting(request, text).0
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
        assert!(
            w.text
                .contains("headline 1 @0 [&quot;One &lt;&amp;&gt;&quot;]"),
            "{}",
            w.text
        );
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
        assert!(text.starts_with("\\begin{verbatim}\noptions\n"), "{text}");
        assert!(text.contains("\nheadline 1 @0"), "{text}");
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
        let text = without_anchors(written(&effects).1);
        assert!(
            text.contains("\n  title [\"Two\"]\n"),
            "the headline titles it: {text}"
        );
        assert!(
            text.ends_with(
                "\nheadline 1 @2 [\"Two\"]\n  paragraph @3 \"y\"\n  headline 2 @4 [\"Deep\"]\n```\n"
            ),
            "{text}"
        );
        // In the section of a nested headline, the innermost one.
        r.line = 5;
        let effects = run(&r, Some("* One\nx\n* Two\ny\n** Deep\nz\n".to_string()));
        let deep = without_anchors(written(&effects).1);
        assert!(
            deep.ends_with("\nheadline 2 @4 [\"Deep\"]\n  paragraph @5 \"z\"\n```\n"),
            "{deep}"
        );
        r.line = 0;
        let outside = run(&r, Some("text\n* One\n".to_string()));
        assert!(matches!(&outside[..], [Effect::Echo(_)]), "{outside:?}");
    }

    #[test]
    fn chunks_start_at_headlines_past_the_budget() {
        let text = "pre\n* a\nxxxxxxxx\n* b\n* c\nyy\n";
        // Line starts: pre 0, * a 4, xxxxxxxx 8, * b 17, * c 21, yy 25, end 28.
        assert_eq!(
            chunks(text, 10),
            vec![(0..17, 0), (17..28, 3)],
            "a chunk runs to the first headline at least the budget in"
        );
        assert_eq!(
            chunks(text, 1),
            vec![(0..4, 0), (4..17, 1), (17..21, 3), (21..28, 4)]
        );
        assert_eq!(chunks(text, 1000), vec![(0..28, 0)]);
        assert_eq!(chunks("", 10), vec![(0..0, 0)]);
        assert_eq!(chunks("a\r\n* b\r\n", 1), vec![(0..3, 0), (3..8, 1)]);
    }

    /// What the stepped walk should produce, computed in one go: the options
    /// header, then the whole tree's filtered dump, by plain recursion.
    fn filtered_dump(text: &str, ctx: &Context) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let sections = parse::parse_chunk(&lines, 0, ctx);
        let mut targets = Targets::default();
        for section in &sections {
            targets.collect(section);
        }
        let doc = parse::assemble(sections);
        // Keywords count wherever they are, as in ox.
        let mut keywords = Vec::new();
        let mut sections = vec![&doc.preamble];
        let mut stack: Vec<&Headline> = doc.headlines.iter().rev().collect();
        while let Some(h) = stack.pop() {
            sections.push(&h.section);
            stack.extend(h.children.iter().rev());
        }
        for e in sections.into_iter().flatten() {
            if let ElementKind::Keyword { key, value } = &e.kind {
                keywords.push((key.clone(), value.clone()));
            }
        }
        let o = Options::resolve(&keywords, None, "plan.org");
        let sel = filter::selection(&doc, &o);
        let mut out = parse::dump::options(&o);
        targets.set_live(filter::live(&doc, &sel, &o), true);
        let mut v = View::new(
            o,
            keywords,
            When { local_secs: 0 },
            targets,
            Rc::new(Files(Default::default())),
        );
        let preamble = filter::elements(&doc.preamble, &mut v);
        parse::dump::preamble(&mut out, &preamble);
        fn walk(out: &mut String, hs: &[Headline], depth: usize, sel: &Selection, v: &mut View) {
            for h in hs {
                if filter::visits(h, sel, &v.options) {
                    let own = filter::own(h, v);
                    anchor_line(out, v, h.line, depth);
                    parse::dump::headline(out, &own, depth);
                    walk(out, &h.children, depth + 1, sel, v);
                }
            }
        }
        walk(&mut out, &doc.headlines, 0, &sel, &mut v);
        out
    }

    /// A file larger than a step is parsed over several, and the tree is the
    /// one parsing it whole gives.
    #[test]
    fn a_large_file_is_parsed_in_steps_into_the_whole_files_tree() {
        let section = format!("* TODO Heading\n{}\n", "- item\n  text\n".repeat(200));
        let text = section.repeat(1 + 3 * STEP_BYTES / section.len());
        let mut r = request(Backend::Markdown);
        r.body_only = true;
        let (effects, steps) = run_counting(&r, Some(text.clone()));
        assert!(steps > 3, "{steps} steps");
        let ctx = Context {
            todo_keywords: vec!["TODO".to_string(), "DONE".to_string()],
        };
        let whole = filtered_dump(&text, &ctx);
        assert_eq!(written(&effects).1, format!("```\n{whole}```\n"));
    }

    /// The render walks the outline in document order: nested, sibling and
    /// level-skipping headlines come out as the whole tree's dump has them.
    #[test]
    fn the_render_walks_every_headline_in_order() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/export/parse/testdata");
        let mut r = request(Backend::Markdown);
        r.body_only = true;
        for name in ["headlines", "lists", "blocks", "misc", "objects"] {
            let text = std::fs::read_to_string(dir.join(format!("{name}.org"))).expect("corpus");
            let ctx = Context::for_file(&text, vec!["TODO".to_string(), "DONE".to_string()]);
            let whole = filtered_dump(&text, &ctx);
            assert_eq!(
                written(&run(&r, Some(text))).1,
                format!("```\n{whole}```\n"),
                "{name}"
            );
        }
    }

    /// A macro that is not defined fails the export, in the title or in the
    /// body, and nothing is written.
    #[test]
    fn an_undefined_macro_fails_the_export() {
        for text in ["#+TITLE: {{{nope}}}\n* A\n", "* A\n** B\n{{{nope}}}\n"] {
            let effects = run(&request(Backend::Html), Some(text.to_string()));
            assert!(
                matches!(&effects[..], [Effect::Echo(e)] if e.text == "org: {{{nope}}} is not defined"),
                "{text}: {effects:?}"
            );
        }
        let effects = run(
            &request(Backend::Markdown),
            Some("#+MACRO: who Ada\n#+TITLE: By {{{who}}}\n".to_string()),
        );
        assert!(written(&effects).1.starts_with("# By Ada\n"));
    }

    /// Includes are read a step each before the parse, their keywords
    /// apply, and a subtree export under them still finds the cursor's
    /// headline in the expanded text.
    #[test]
    fn includes_are_read_before_the_parse_and_the_cursor_follows() {
        let text = "#+INCLUDE: \"setup.org\"\n* One\n#+INCLUDE: \"part.org\"\n* Two\ny\n";
        let files = [
            (
                "/notes/setup.org",
                "#+MACRO: who Ada\n#+TITLE: By {{{who}}}\n",
            ),
            ("/notes/part.org", "* A\n* B\n"),
        ];
        let mut r = request(Backend::Markdown);
        let (effects, steps) = run_with_files(&r, Some(text.to_string()), &files);
        let out = without_anchors(written(&effects).1);
        assert!(out.starts_with("# By Ada\n"), "{out}");
        assert!(
            out.contains("headline 1 @2 [\"One\"]\n  headline 2 @3 [\"A\"]\n  headline 2 @4 [\"B\"]\nheadline 1 @5"),
            "{out}"
        );
        assert!(steps >= 4, "an include a step: {steps}");

        r.subtree = true;
        r.body_only = true;
        r.line = 4; // `y`, under Two, past both includes.
        let (effects, _) = run_with_files(&r, Some(text.to_string()), &files);
        let out = without_anchors(written(&effects).1);
        assert!(out.contains("\n  title [\"Two\"]\n"), "{out}");
        assert!(
            out.ends_with("\nheadline 1 @5 [\"Two\"]\n  paragraph @6 \"y\"\n```\n"),
            "{out}"
        );

        let (effects, _) = run_with_files(&request(Backend::Html), Some(text.to_string()), &[]);
        assert!(
            matches!(&effects[..], [Effect::Echo(e)] if e.text == "org: cannot include /notes/setup.org: missing"),
            "{effects:?}"
        );
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

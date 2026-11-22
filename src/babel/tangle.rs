//! `:tangle` — write blocks out to the files they name (OB.10, design §7).
//!
//! Pure planning, then effects: every block with a `:tangle` other than `no`
//! goes to its target, the blocks for one target are joined in document
//! order, and each target is written WHOLE (`write-to-file`'s `whole`
//! anchor, saved), which makes it one undoable edit in a buffer the user has
//! open. Every target is checked with `can-write-file` before anything is
//! written: one refusal writes nothing.
//!
//! A `:shebang` target is made executable after its save lands. WASI has no
//! `chmod` and the file may not exist until the write does, so the target is
//! recorded in the store and the events seam calls `set-executable` when it
//! sees that path's `document-saved` — vim's `BufWritePost`, which is the
//! moment it is true.

use super::block::{effective_header_args, src_blocks, SrcBlock};
use super::header_args::HeaderArgs;
use super::Language;
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::types::{
    EchoLevel, EchoPayload, Effect, FileAnchor, WriteToFilePayload,
};

/// Store prefix for targets waiting on their save to be made executable.
pub(crate) const CHMOD_PREFIX: &str = "babel/chmod/";

/// One file a tangle writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) path: String,
    pub(crate) text: String,
    pub(crate) blocks: usize,
    /// It has a `:shebang`, so it is made executable.
    pub(crate) executable: bool,
}

/// A header argument as one string, quotes stripped: `:shebang "#!/bin/sh"`.
fn word(header: &HeaderArgs, key: &str) -> Option<String> {
    let w = header.get(key)?.join(" ");
    let w = w.trim();
    let w = w
        .strip_prefix('"')
        .and_then(|w| w.strip_suffix('"'))
        .unwrap_or(w);
    (!w.is_empty()).then(|| w.to_string())
}

/// The extension `:tangle yes` gives a language: its `org.babel.languages`
/// entry's `ext`, else babel's own name for it, else the language itself.
fn ext_for(lang: &str, languages: &[Language]) -> String {
    if let Some(ext) = languages
        .iter()
        .find(|l| l.name == lang)
        .and_then(|l| l.ext.clone())
    {
        return ext;
    }
    // `org-babel-tangle-lang-exts`, for the languages people tangle most.
    match lang {
        "python" => "py",
        "ruby" => "rb",
        "js" | "javascript" | "node" => "js",
        "shell" | "bash" | "sh" | "zsh" => "sh",
        "emacs-lisp" | "elisp" => "el",
        "rust" => "rs",
        "haskell" => "hs",
        "perl" => "pl",
        "clojure" => "clj",
        "lua" => "lua",
        "C" | "c" => "c",
        "C++" | "cpp" => "cpp",
        "go" => "go",
        other => other,
    }
    .to_string()
}

/// `path` with `.` and `..` folded away, lexically — the guest cannot
/// resolve links, and the host names a buffer's path the same way, so this
/// is what a saved buffer's `document-saved` path is compared against.
pub(crate) fn normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            p => parts.push(p),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Where `:tangle <value>` writes for a block in the file at `file`.
fn target_path(
    value: &str,
    block: &SrcBlock,
    file: Option<&str>,
    languages: &[Language],
) -> Result<String, String> {
    let dir = file.and_then(|f| f.rsplit_once('/')).map(|(d, _)| d);
    let named = if value.eq_ignore_ascii_case("yes") {
        let Some(file) = file else {
            return Err(
                "org: `:tangle yes` names the file after this one, and this buffer has no file"
                    .to_string(),
            );
        };
        let name = file.rsplit('/').next().unwrap_or(file);
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        format!("{stem}.{}", ext_for(&block.lang, languages))
    } else {
        crate::roam_scan::expand_tilde(value)
    };
    if named.starts_with('/') {
        return Ok(normalize(&named));
    }
    match dir {
        Some(d) => Ok(normalize(&format!("{d}/{named}"))),
        None => Err(format!(
            "org: `:tangle {value}` is relative to this file, and this buffer has no file"
        )),
    }
}

/// The files the blocks in `lines` tangle to, in the order they first
/// appear, or why the tangle cannot be planned.
///
/// Babel's layout: a target's blocks in document order, a blank line between
/// two blocks unless the later one says `:padline no`, every block ending
/// in a newline, and the first `:shebang` among its blocks as line one.
pub(crate) fn plan(
    lines: &[&str],
    file: Option<&str>,
    languages: &[Language],
) -> Result<Vec<Target>, String> {
    struct Building {
        path: String,
        shebang: Option<String>,
        parts: Vec<(bool, String)>,
    }
    let mut targets: Vec<Building> = Vec::new();
    for block in src_blocks(lines) {
        let header = effective_header_args(lines, &block);
        let Some(value) = word(&header, "tangle") else {
            continue;
        };
        if value.eq_ignore_ascii_case("no") {
            continue;
        }
        let path = target_path(&value, &block, file, languages)?;
        let padline = !word(&header, "padline").is_some_and(|p| p.eq_ignore_ascii_case("no"));
        let shebang = word(&header, "shebang");
        let i = match targets.iter().position(|t| t.path == path) {
            Some(i) => i,
            None => {
                targets.push(Building {
                    path,
                    shebang: None,
                    parts: Vec::new(),
                });
                targets.len() - 1
            }
        };
        let t = &mut targets[i];
        if t.shebang.is_none() {
            t.shebang = shebang;
        }
        t.parts.push((padline, block.body.clone()));
    }
    Ok(targets
        .into_iter()
        .map(|t| {
            let mut text = String::new();
            if let Some(s) = &t.shebang {
                text.push_str(s);
                text.push('\n');
            }
            for (i, (padline, body)) in t.parts.iter().enumerate() {
                if i > 0 && *padline {
                    text.push('\n');
                }
                text.push_str(body);
                if !body.ends_with('\n') {
                    text.push('\n');
                }
            }
            Target {
                path: t.path,
                text,
                blocks: t.parts.len(),
                executable: t.shebang.is_some(),
            }
        })
        .collect())
}

/// `:org-babel-tangle` / `C-c C-v t`: the effects that write every target, or
/// the one echo saying why nothing was written.
pub(crate) fn tangle(lines: &[&str], file: Option<&str>, languages: &[Language]) -> Vec<Effect> {
    let warn = |text: String| {
        vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text,
        })]
    };
    let targets = match plan(lines, file, languages) {
        Ok(t) => t,
        Err(e) => return warn(e),
    };
    if targets.is_empty() {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Info,
            text: "org: no source block here has a `:tangle`".to_string(),
        })];
    }
    // All or nothing: one refusal and no file is touched, so a tangle never
    // leaves half its outputs from this run and half from the last.
    for t in &targets {
        if let Err(reason) = host_services::can_write_file(&t.path) {
            return warn(format!(
                "org: nothing tangled — {}: {reason}. A directory outside org.directory \
                 goes in org.babel.tangle-directories",
                t.path
            ));
        }
    }
    let mut effects = Vec::new();
    let mut notes = Vec::new();
    for t in &targets {
        if t.executable {
            if let Err(e) = host_services::store_put(&chmod_key(&t.path), t.path.as_bytes()) {
                notes.push(format!("org: {} will not be made executable: {e}", t.path));
            }
        }
        effects.push(Effect::WriteToFile(WriteToFilePayload {
            path: t.path.clone(),
            anchor: FileAnchor::Whole,
            text: t.text.clone(),
            cut: None,
            // Babel's `:mkdirp` is off by default, and a missing directory is
            // more often a typo than a wish.
            create_parents: false,
            // On disk is the point of a tangle: the file is read by whatever
            // runs it, not by the editor.
            save: true,
        }));
    }
    let blocks: usize = targets.iter().map(|t| t.blocks).sum();
    let plural = |n: usize, one: &str| {
        if n == 1 {
            one.to_string()
        } else {
            format!("{one}s")
        }
    };
    effects.push(Effect::Echo(EchoPayload {
        level: EchoLevel::Info,
        text: format!(
            "org: tangled {blocks} {} to {} {}",
            plural(blocks, "block"),
            targets.len(),
            plural(targets.len(), "file")
        ),
    }));
    effects.extend(notes.into_iter().map(|text| {
        Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text,
        })
    }));
    effects
}

fn chmod_key(path: &str) -> String {
    format!("{CHMOD_PREFIX}{path}")
}

/// `document-saved`: a tangle target waiting on its save is made executable
/// now that the file is there. Anything else saved is not ours.
pub(crate) fn on_saved(path: &str) {
    let key = chmod_key(&normalize(path));
    if host_services::store_get(&key).is_none() {
        return;
    }
    let _ = host_services::store_delete(&key);
    if let Err(e) = host_services::set_executable(path) {
        // `post-effects`, not a log: the guest must not import `logging`
        // (it would not instantiate on the grammar seam's linker), and the
        // user asked for an executable they did not get.
        let _ = host_services::post_effects(&[Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: format!("org: tangled {path} but could not make it executable: {e}"),
        })]);
    }
}

/// Forget targets a previous load was waiting on.
pub(crate) fn sweep() {
    for key in host_services::store_keys(CHMOD_PREFIX) {
        let _ = host_services::store_delete(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(t: &str) -> Vec<&str> {
        t.lines().collect()
    }

    #[test]
    fn yes_is_the_files_stem_and_the_languages_ext() {
        let t = "#+begin_src python :tangle yes\nprint(1)\n#+end_src\n";
        let p = plan(&lines(t), Some("/notes/setup.org"), &[]).unwrap();
        assert_eq!(p[0].path, "/notes/setup.py");
        assert!(plan(&lines(t), None, &[]).is_err(), "no file, no stem");
    }

    #[test]
    fn a_targets_blocks_join_in_order_with_padlines() {
        let t = "#+begin_src sh :tangle out.sh :shebang \"#!/bin/sh\"\none\n#+end_src\n\
                 #+begin_src sh :tangle other.sh\nx\n#+end_src\n\
                 #+begin_src sh :tangle out.sh\ntwo\n#+end_src\n\
                 #+begin_src sh :tangle out.sh :padline no\nthree\n#+end_src\n\
                 #+begin_src sh\nnot tangled\n#+end_src\n";
        let p = plan(&lines(t), Some("/n/a.org"), &[]).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].path, "/n/out.sh");
        assert_eq!(p[0].text, "#!/bin/sh\none\n\ntwo\nthree\n");
        assert_eq!(p[0].blocks, 3);
        assert!(p[0].executable);
        assert_eq!(p[1].path, "/n/other.sh");
        assert!(!p[1].executable);
    }

    #[test]
    fn paths_fold_dots_lexically() {
        assert_eq!(normalize("/a/./b/../c"), "/a/c");
        assert_eq!(normalize("/../a"), "/a");
        assert_eq!(normalize("a/../../b"), "../b");
        let t = "#+begin_src sh :tangle ../bin/x.sh\nx\n#+end_src\n";
        assert_eq!(
            plan(&lines(t), Some("/n/org/a.org"), &[]).unwrap()[0].path,
            "/n/bin/x.sh"
        );
    }

    #[test]
    fn a_file_wide_property_tangles_every_block() {
        let t = "#+PROPERTY: header-args :tangle all.sh\n\
                 #+begin_src sh\na\n#+end_src\n#+begin_src sh :tangle no\nb\n#+end_src\n";
        let p = plan(&lines(t), Some("/n/a.org"), &[]).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].text, "a\n");
    }
}

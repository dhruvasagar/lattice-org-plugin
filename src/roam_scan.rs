//! OR.4 — the pass that turns a directory of files into an index.
//!
//! [`crate::roam`] says what a node is; [`crate::roam_index`] says where the
//! records go. This is the thing that walks, reads, parses and drives both, and
//! it is the only code that touches configuration.
//!
//! ## Where this runs
//!
//! On the **async event seam**, which is why the expensive parts are allowed to
//! be expensive: a cold pass reads 706 files and parses the ones whose bytes
//! moved, and none of that is anywhere near the keystroke path. Nothing here is
//! reachable from the grammar seam, and that is structural rather than
//! disciplined — the walk is driven by `register-events` and by the watcher's
//! event, both of which only the event actor delivers.
//!
//! ## Boot does a full walk
//!
//! Not because the store is untrusted, but because lattice was not running
//! while the corpus changed, and a watcher cannot report what it did not
//! observe. The walk is cheap where it can be: reading a file is ~10–50 µs
//! warm, and only files whose content hash moved are parsed — the parse is
//! 1–2 ms and is what the hash exists to protect.

use crate::lattice::plugin_host::config::get_option;
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::tree_sitter;
use crate::{roam, roam_index, roam_tree, todo, DEFAULT_TODO_KEYWORDS};

/// The corpus root, or `None` when roam is not configured.
///
/// **Unset means inert**, and that is a promise rather than a default: with no
/// directory there is no walk, no watcher and no store write, so an org user
/// who keeps no zettelkasten pays nothing for this feature existing.
pub fn roam_directory() -> Option<String> {
    let raw = get_option("roam-directory")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(expand_tilde(trimmed))
}

/// `~` expansion.
///
/// This was a no-op, and its comment explained why: the guest had no
/// environment, so `$HOME` was unreadable and a `~` could only be passed
/// through to a host that would not resolve it either. The cost was a config
/// that could not be portable — `org.roam-directory` had to name
/// `/Users/dhruva/…`, and the same file on another machine found nothing. It
/// failed silently, as an empty corpus rather than a bad path.
///
/// The host now supplies `HOME` in the guest's WASI environment for exactly
/// this, and nothing else: no `PATH`, no `USER`, no shell. Knowing the path
/// grants no access — every read still goes through the capability preopens, so
/// a plugin that learns the home directory and asks to read it is refused
/// exactly as before.
///
/// **`~user` is left verbatim**, deliberately: expanding it against OUR home
/// would produce a plausible path to the wrong place, which is worse than one
/// that still visibly has a `~` in it. Same rule as the host's
/// `lattice_core::home::expand_tilde`, which this mirrors.
fn expand_tilde(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    if !rest.is_empty() && !rest.starts_with('/') {
        return path.to_string();
    }
    let Ok(home) = std::env::var("HOME") else {
        return path.to_string();
    };
    let home = home.trim_end_matches('/');
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        home.to_string()
    } else {
        format!("{home}/{rest}")
    }
}

/// The configured TODO keywords, so a headline's title loses its keyword.
fn todo_keywords() -> Vec<String> {
    let spec = get_option("todo-keywords").unwrap_or_else(|| DEFAULT_TODO_KEYWORDS.to_string());
    todo::parse_todo_keywords(&spec).names()
}

/// Index one file, given its path. Returns whether anything changed.
///
/// Every failure is a skip with a `debug` log and never an error: one
/// unreadable or unparseable file must not fail a pass over 706 of them. That
/// is `error-parser`'s rule and the reason a corpus with one bad file still
/// gets an index.
pub fn index_one(path: &str, keywords: &[String]) -> bool {
    let text = match host_services::read_file(path) {
        Ok(text) => text,
        Err(_) => {
            // Gone, or never readable. Either way the index must stop offering
            // what it holds for it — a destination that does not exist is the
            // failure `f/<path>` exists to prevent.
            return roam_index::forget_file(path);
        }
    };
    // The hash check BEFORE the parse: that ordering is the whole optimisation.
    // A warm pass over an unchanged corpus costs one `get` and one comparison
    // per file and never reaches the 1–2 ms parse below.
    if roam_index::file_row(path).is_some_and(|row| row.hash == roam_index::content_hash(&text)) {
        return false;
    }
    let Some(tree) = tree_sitter::parse_file(path) else {
        // No grammar, no tree, or a denied grant. A file that cannot be parsed
        // contributes nothing — and must also stop contributing whatever it
        // used to, or a note that became unparseable would linger.
        return roam_index::forget_file(path);
    };
    let lines: Vec<&str> = text.lines().collect();
    let outline = roam_tree::outline_from_tree(&tree, &lines);
    let extracted = roam::extract(path, &text, &outline, keywords);
    roam_index::index_file(path, &text, &extracted)
}

/// Walk the corpus and index everything that changed.
///
/// The `nodes` blob is rebuilt **once**, at the end, and only if something
/// moved — rebuilding per file would make a cold pass quadratic in host calls.
pub fn sync_all() -> usize {
    let Some(root) = roam_directory() else {
        return 0;
    };
    // Arm the watch here rather than only at registration, and idempotently.
    //
    // Two reasons, both about ordering. `register-events` runs when the plugin
    // loads, which may be BEFORE the user's `init.rs` sets
    // `org.roam-directory` (the documented CI.1 `plugin-loaded` pattern is
    // exactly that shape) — so a watch armed only there would never arm for
    // the user who configures roam the documented way. And a `watch` on a path
    // already watched is `ok` and arms nothing new, so calling it on every
    // sync costs one host call and closes the hole.
    //
    // The watch goes up BEFORE the walk: a file written between the walk
    // finishing and the watch arming would be observed by neither, and the
    // symptom — a note you know you wrote missing from the picker — reads as
    // data loss.
    let _ = host_services::watch(&root);
    // A denied or unwalkable root is a silent zero, deliberately.
    //
    // **The guest must not log.** Calling `logging::log` makes the component
    // IMPORT `logging`, and org's multi-seam linker does not wire that import
    // on the grammar seam — the WHOLE component then fails to instantiate.
    // That has happened before (OC.2), so the honest degradation here is to
    // index nothing and let `:org-roam-sync` be the user's escape hatch, not
    // to explain the problem at the cost of the plugin.
    let Ok(files) = host_services::walk(&root) else {
        return 0;
    };
    let keywords = todo_keywords();
    let mut changed = 0usize;
    for path in files.iter().filter(|p| is_org(p)) {
        if index_one(path, &keywords) {
            changed += 1;
        }
    }
    if changed > 0 {
        roam_index::rebuild_nodes_blob();
    }
    changed
}

/// Re-index exactly the paths a watcher reported.
///
/// The watcher's common case, and the reason the batch shape matters: a `git
/// pull` arrives as one event carrying many paths, so this rebuilds the blob
/// once for the whole batch rather than once per file.
pub fn reindex(paths: &[String]) -> usize {
    if roam_directory().is_none() {
        return 0;
    }
    let keywords = todo_keywords();
    let mut changed = 0usize;
    for path in paths.iter().filter(|p| is_org(p)) {
        if index_one(path, &keywords) {
            changed += 1;
        }
    }
    if changed > 0 {
        roam_index::rebuild_nodes_blob();
    }
    changed
}

/// Whether a path is a file roam indexes.
///
/// `.org` only. `.org_archive` is deliberately excluded: an archived subtree is
/// out of the corpus by definition, and indexing it would put finished notes in
/// front of the user every time they searched.
pub fn is_org(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("org"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_org_files_are_indexed() {
        assert!(is_org("/roam/note.org"));
        assert!(is_org("/roam/NOTE.ORG"), "extension case does not matter");
        assert!(
            !is_org("/roam/note.org_archive"),
            "an archived subtree is out of the corpus by definition"
        );
        assert!(!is_org("/roam/README.md"));
        assert!(!is_org("/roam/notorg"));
    }

    /// The guest's half of tilde expansion. Mirrors the host's
    /// `lattice_core::home::expand_tilde` rule for rule, because a config that
    /// expands one way in a capability grant and another way in an option is
    /// worse than one that does not expand at all.
    #[test]
    fn a_leading_tilde_expands_against_home() {
        let Ok(home) = std::env::var("HOME") else {
            eprintln!("SKIP: no HOME in this test environment");
            return;
        };
        let home = home.trim_end_matches('/');
        assert_eq!(expand_tilde("~/notes"), format!("{home}/notes"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/"), home);
    }

    #[test]
    fn a_path_without_a_leading_tilde_is_untouched() {
        assert_eq!(expand_tilde("/abs/roam"), "/abs/roam");
        assert_eq!(expand_tilde("relative/roam"), "relative/roam");
        assert_eq!(expand_tilde(""), "");
        assert_eq!(expand_tilde("a~b"), "a~b", "a tilde must LEAD to count");
    }

    /// `~user` stays verbatim. Expanding it against OUR home would point at a
    /// plausible wrong directory, and a roam corpus silently read from the
    /// wrong place is worse than one that is not found.
    #[test]
    fn another_users_home_is_left_alone() {
        assert_eq!(expand_tilde("~alice/roam"), "~alice/roam");
    }
}

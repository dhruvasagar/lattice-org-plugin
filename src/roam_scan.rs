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
use crate::{roam, roam_index, roam_tree, todo};

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
    todo::parse_todo_keywords(&crate::todo_keyword_lines().join("\n")).names()
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

/// OR.4b — a cold scan, carried across many guest calls.
///
/// ## Why this is not one loop
///
/// It was, and on a real corpus it took the plugin down. The async seam's
/// budget is `epoch_deadline: 1_000` — about **one second per call** — and a
/// cold pass over the reference corpus (706 files, each a host `read-file`, a
/// hash, a tree-sitter parse and a store write) takes roughly twenty-seven. So
/// the guest ran past the deadline, **trapped**, and the host quarantined the
/// plugin for the rest of the session.
///
/// Every symptom followed from that and none of them named it: the picker
/// showed `0/0` because the `nodes` blob is only written at the END of a scan;
/// `:org-roam-sync` echoed "re-scanning" and never finished; and after the
/// first trap nothing org did worked at all, because a quarantined plugin
/// stays quarantined.
///
/// No test caught it because every roam test uses a four-file corpus, which
/// finishes in milliseconds. The bug is a function of corpus SIZE, and the
/// per-file estimates in the design fragment were right — it is their sum that
/// was never checked against a real zettelkasten.
///
/// ## The shape
///
/// [`begin`] walks once and parks the file list; each [`step`] indexes at most
/// [`BATCH`] of them and rings the doorbell again if any remain. The queue
/// lives in a `thread_local` because the events store is one `Store` whose
/// memory persists across calls (the clock's `SESSION` is the precedent), so
/// carrying it costs no store round-trip per batch.
///
/// ## What stops a scan from being cancelled
///
/// **A bounded batch cannot trap.** That is the whole guarantee, and it is why
/// `BATCH` is set by the worst case rather than the average.
///
/// **A stale chain stops itself.** Each step carries the generation it was
/// started with, and a step whose generation is not the current one returns
/// without doing anything or re-arming. So pointing `org.roam-directory` at a
/// new corpus mid-scan does not leave two chains interleaving writes from two
/// roots — the old one simply stops on its next hop.
///
/// **One bad file cannot break the chain.** [`index_one`] already swallows a
/// read or parse failure per file (`error-parser`'s rule), so a scan steps past
/// it rather than dying on it.
mod scan {
    /// How long one guest call may spend indexing before it yields.
    ///
    /// **A time budget, not a file count**, and the difference is the whole
    /// point. What traps is wall time against the seam's ~1s epoch deadline
    /// (`epoch_deadline: 1_000` ticks at ~1 ms each). A fixed file count only
    /// approximates that, and approximates it worst exactly where it matters:
    /// a corpus of large or deeply-nested notes costs several times a corpus of
    /// small ones per file, so the count that is safe on one is a trap on the
    /// other. Measuring the thing that actually runs out removes the guess.
    ///
    /// 250 ms leaves 4× headroom under the deadline — enough to absorb one
    /// pathologically slow file discovered mid-batch, since the check happens
    /// BETWEEN files and a single file's parse still has to fit.
    pub const BUDGET_MS: u64 = 250;

    /// Always index at least this many per call, however slow.
    ///
    /// A machine slow enough that one file exceeds the budget would otherwise
    /// make no progress at all and re-arm forever — a livelock that looks
    /// exactly like the hang this whole change exists to remove. Better to risk
    /// one long call than to spin.
    pub const MIN_PER_CALL: usize = 1;

    pub struct Scan {
        pub files: Vec<String>,
        pub at: usize,
        pub changed: usize,
        pub keywords: Vec<String>,
    }

    thread_local! {
        pub static SCAN: std::cell::RefCell<Option<Scan>> =
            const { std::cell::RefCell::new(None) };
        /// Bumped on every `begin`. A step carrying an older value is stale.
        pub static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
}

/// Start a cold scan of the corpus. Returns the number of files queued.
///
/// Replaces any scan in flight — a new root makes the old queue wrong, and the
/// generation bump is what stops the old chain rather than letting both run.
pub fn begin() -> usize {
    let Some(root) = roam_directory() else {
        // Not configured. Clear any progress a previous root left on screen.
        crate::lattice::plugin_host::ui::clear_segment(crate::ROAM_SEGMENT);
        return 0;
    };
    // Arm the watch here rather than only at registration, and idempotently.
    //
    // `register-events` runs when the plugin loads, which may be BEFORE the
    // user's `init.rs` sets `org.roam-directory` (the documented CI.1
    // `plugin-loaded` pattern is exactly that shape). A watch armed only there
    // would never arm for the user who configures roam the documented way, and
    // a `watch` on an already-watched path is `ok` and arms nothing new.
    //
    // BEFORE the walk: a file written between the walk finishing and the watch
    // arming would be observed by neither, and the symptom — a note you know
    // you wrote missing from the picker — reads as data loss.
    let _ = host_services::watch(&root);
    // A denied or unwalkable root is a silent zero, deliberately. **The guest
    // must not log**: calling `logging::log` makes the component IMPORT
    // `logging`, which org's multi-seam linker does not wire on the grammar
    // seam, and the WHOLE component then fails to instantiate (OC.2).
    let Ok(all) = host_services::walk(&root) else {
        return 0;
    };
    let files: Vec<String> = all.into_iter().filter(|p| is_org(p)).collect();
    let total = files.len();
    // Bump BEFORE the queue is replaced: any chain still draining the old
    // root reads this on its next hop and stops.
    scan::GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    let keywords = todo_keywords();
    scan::SCAN.with(|s| {
        *s.borrow_mut() = Some(scan::Scan {
            files,
            at: 0,
            changed: 0,
            keywords,
        });
    });
    if total == 0 {
        crate::lattice::plugin_host::ui::clear_segment(crate::ROAM_SEGMENT);
        return 0;
    }
    publish_progress(0, total);
    total
}

/// The current scan's generation, for the step that is about to be armed.
pub fn generation() -> u64 {
    scan::GENERATION.with(|g| g.get())
}

/// Index one batch. Returns `true` while more remain — the caller re-arms.
///
/// A `generation` that is not the current one is a stale chain: it returns
/// `false` and does nothing, which is what stops two scans interleaving.
pub fn step(generation: u64) -> bool {
    if generation != scan::GENERATION.with(|g| g.get()) {
        return false;
    }
    scan::SCAN.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(sc) = borrow.as_mut() else {
            return false;
        };
        // Index until the time budget is spent, checking BETWEEN files so a
        // file is never abandoned half-indexed.
        let started = std::time::SystemTime::now();
        let mut done_now = 0usize;
        while sc.at < sc.files.len() {
            if index_one(&sc.files[sc.at], &sc.keywords) {
                sc.changed += 1;
            }
            sc.at += 1;
            done_now += 1;
            if done_now >= scan::MIN_PER_CALL {
                let spent = started
                    .elapsed()
                    .map(|d| d.as_millis() as u64)
                    // An unreadable clock yields immediately rather than
                    // running unbounded: one file per call is slow, and a trap
                    // takes the whole plugin down.
                    .unwrap_or(u64::MAX);
                if spent >= scan::BUDGET_MS {
                    break;
                }
            }
        }
        let total = sc.files.len();
        let done = sc.at >= total;
        if done {
            // The blob is rebuilt ONCE, at the end — rebuilding per batch would
            // make a cold scan quadratic in host calls, which is the cost the
            // whole batching exists to avoid paying twice.
            if sc.changed > 0 {
                roam_index::rebuild_nodes_blob();
            }
            *borrow = None;
            crate::lattice::plugin_host::ui::clear_segment(crate::ROAM_SEGMENT);
        } else {
            publish_progress(sc.at, total);
        }
        !done
    })
}

/// Show the scan's progress in the modeline.
///
/// The modeline rather than an echo, because a scan outlives the message line:
/// an echo is replaced by the next thing that echoes, and the whole complaint
/// this fixes was a user who could not tell a long scan from a dead one.
fn publish_progress(at: usize, total: usize) {
    crate::lattice::plugin_host::ui::emit_segment(
        crate::ROAM_SEGMENT,
        &format!("\u{27f3} roam {at}/{total}"),
    );
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

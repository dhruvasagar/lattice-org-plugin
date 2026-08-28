//! Refile targets: where a subtree can go (OM.11).
//!
//! `org-refile` moves the subtree at point somewhere else — usually under a
//! headline in another file. That "under a headline" is the whole feature; a
//! refile that could only append to a file is archive with a file chooser.
//!
//! ## Why the picker source reads the files
//!
//! Targets are headlines in OTHER files, so something has to read them. The
//! agenda's answer is the host reading and handing text to `scan`, and the
//! picker seam has no equivalent — `host-services` walks but does not read.
//!
//! It does not need one. A plugin's `fs:read` / `fs:write` grants become WASI
//! preopens **at the same path the host uses**, so `std::fs::read_to_string`
//! on a granted org file works from inside the guest and is refused outside
//! the grant by WASI itself rather than by discipline. And it happens in
//! `init`, which is off the keystroke path.
//!
//! ## Why the insertion LINE is computed here
//!
//! The guest that read the file is the only one that knows where a headline's
//! subtree ends, and `file-anchor::line` wants exactly that. Computing it in
//! the picker source and routing it through means the refile *action* — which
//! has never seen the target file — forwards a number instead of guessing one.

use crate::headline;

/// One place a subtree can be filed.
pub struct Target {
    /// Absolute path of the target file.
    pub file: String,
    /// The 0-based line to insert BEFORE, or `None` to append at the end.
    ///
    /// Past-the-end is fine: `file-anchor::line` clamps to `end` rather than
    /// failing, which is what makes "after the last headline in the file"
    /// expressible without a special case.
    pub before_line: Option<u32>,
    /// What the user reads in the picker: `notes.org  Work / Q3`.
    pub label: String,
}

/// Every headline in `text` down to `max_level`, plus the file itself.
///
/// The file-level entry comes first because "put this in `inbox.org`" is the
/// answer when none of the headlines is right, and a picker whose first row is
/// a deep heading buries it.
///
/// `max_level` bounds the read the way org's `org-refile-targets` `:maxlevel`
/// does: without it every leaf in a large tree becomes a row, and the list
/// stops being scannable long before it stops being correct.
/// `#[cfg(test)]`, for the reason `capture_target::resolve` gives: production
/// builds the outline at the call site, where it knows whether `parse-file`
/// answered.
#[cfg(test)]
pub fn targets_in(file: &str, display_name: &str, text: &str, max_level: usize) -> Vec<Target> {
    let lines: Vec<&str> = text.lines().collect();
    let outline = headline::outline_text(&lines);
    targets_from(file, display_name, &lines, &outline, max_level)
}

/// [`targets_in`] over an outline someone else produced (OT.8).
///
/// The picker calls this with `tree-sitter.parse-file`'s walk, so a headline
/// written inside `#+BEGIN_SRC` in a project file is not offered as a refile
/// destination — and, worse than merely being offered, is not offered with an
/// insertion line pointing into the middle of a code block.
///
/// One body for both structure sources on purpose. `org-capture.md` §4 says
/// refile's insertion point and capture's "cannot drift apart"; they share
/// `headline::Entry` now rather than sharing only `subtree_end`, which is the
/// same argument one level up.
pub fn targets_from(
    file: &str,
    display_name: &str,
    lines: &[&str],
    outline: &[headline::Entry],
    max_level: usize,
) -> Vec<Target> {
    let mut out = vec![Target {
        file: file.to_string(),
        before_line: None,
        label: display_name.to_string(),
    }];

    // The ancestor titles at each level, so a headline can be shown by its
    // path — two headings called `Notes` under different parents are a real
    // and common case, and indistinguishable by title alone.
    let mut stack: Vec<String> = Vec::new();
    for entry in outline {
        let Some(text) = lines.get(entry.line as usize) else {
            continue;
        };
        stack.truncate(entry.level - 1);
        stack.push(title_of(text, entry.level));
        if entry.level > max_level {
            continue;
        }
        out.push(Target {
            file: file.to_string(),
            before_line: Some(entry.end_line + 1),
            label: format!("{display_name}  {}", stack.join(" / ")),
        });
    }
    out
}

/// The headline's text without its stars. Deliberately NOT stripped of a TODO
/// keyword or tags — the picker matches on what is shown, and `TODO` is a
/// useful thing to type when looking for the place unfinished work lives.
fn title_of(line: &str, level: usize) -> String {
    line[level..].trim().to_string()
}

/// Encode a target as the opaque token the picker hands back at `accept`.
///
/// `path\tline`, with an empty line meaning "append". A tab because a path may
/// contain almost anything else, and this round-trips through `args` as one
/// string rather than needing a typed payload the seam does not have.
pub fn encode(target: &Target) -> String {
    match target.before_line {
        Some(line) => format!("{}\t{line}", target.file),
        None => format!("{}\t", target.file),
    }
}

/// The inverse of [`encode`]. `None` when the token is malformed — which is
/// untrusted input as far as the action is concerned, since the token crosses
/// the boundary twice.
pub fn decode(token: &str) -> Option<(String, Option<u32>)> {
    let (path, line) = token.split_once('\t')?;
    if path.is_empty() {
        return None;
    }
    if line.is_empty() {
        return Some((path.to_string(), None));
    }
    Some((path.to_string(), Some(line.parse().ok()?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "* Work\n** Q3\nbody\n** Q4\n* Personal\n";

    #[test]
    fn the_file_itself_is_the_first_target() {
        let t = targets_in("/org/notes.org", "notes.org", SAMPLE, 3);
        assert_eq!(t[0].label, "notes.org");
        assert!(t[0].before_line.is_none(), "the file target appends");
    }

    #[test]
    fn headlines_are_shown_by_their_path() {
        let t = targets_in("/org/notes.org", "notes.org", SAMPLE, 3);
        let labels: Vec<&str> = t.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "notes.org",
                "notes.org  Work",
                "notes.org  Work / Q3",
                "notes.org  Work / Q4",
                "notes.org  Personal",
            ]
        );
    }

    /// The insertion line is AFTER the chosen headline's whole subtree, so a
    /// refile under `Work` lands below `Q4` rather than in front of `Q3` — the
    /// same reparenting trap `<leader><CR>` refuses (OM.6).
    #[test]
    fn a_target_inserts_after_the_whole_subtree() {
        let t = targets_in("/org/notes.org", "notes.org", SAMPLE, 3);
        let work = t.iter().find(|t| t.label.ends_with("Work")).unwrap();
        assert_eq!(work.before_line, Some(4), "after Q4, before Personal");
        let q3 = t.iter().find(|t| t.label.ends_with("Q3")).unwrap();
        assert_eq!(q3.before_line, Some(3), "after its body, before Q4");
    }

    /// The last headline's target is past the end of the file. That is not an
    /// error: `file-anchor::line` clamps past-the-end to `end`, which is what
    /// "after the last headline" means anyway.
    #[test]
    fn the_last_headline_targets_past_the_end() {
        let t = targets_in("/org/notes.org", "notes.org", SAMPLE, 3);
        let last = t.last().unwrap();
        assert_eq!(last.before_line, Some(5));
    }

    #[test]
    fn max_level_bounds_the_list_without_losing_the_path() {
        let t = targets_in("/org/notes.org", "notes.org", SAMPLE, 1);
        let labels: Vec<&str> = t.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["notes.org", "notes.org  Work", "notes.org  Personal"]
        );
    }

    #[test]
    fn a_token_round_trips_through_the_picker() {
        let t = &targets_in("/org/notes.org", "notes.org", SAMPLE, 3)[1];
        assert_eq!(
            decode(&encode(t)),
            Some(("/org/notes.org".to_string(), Some(4)))
        );
    }

    #[test]
    fn the_file_target_round_trips_as_an_append() {
        let t = &targets_in("/org/notes.org", "notes.org", SAMPLE, 3)[0];
        assert_eq!(
            decode(&encode(t)),
            Some(("/org/notes.org".to_string(), None))
        );
    }

    /// The token crosses the boundary twice, so the action treats it as
    /// untrusted rather than assuming the picker it opened is the one that
    /// answered.
    #[test]
    fn a_malformed_token_is_refused_rather_than_guessed_at() {
        assert!(decode("no-tab-here").is_none());
        assert!(decode("\t3").is_none(), "an empty path is not a file");
        assert!(decode("/org/notes.org\tnotanumber").is_none());
    }
}

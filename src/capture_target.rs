//! OC.5a — resolving a capture target to the line the note is inserted at.
//!
//! `Target::FileHeadline` has parsed since OC.2 and been ignored ever since:
//! both submit paths called `Target::file()` and handed the host
//! `FileAnchor::End`, so a template that said "under Vocabulary" appended at the
//! bottom of the file instead. It looked configured and behaved as if it were
//! not, which is the worst of both — the menu row even printed the target.
//!
//! ## The insertion point is the subtree's end, not the headline's line
//!
//! Inserting immediately *after* the headline would put the new entry in front
//! of everything already filed under it, so a capture list reads
//! newest-to-oldest inside the subtree while the file around it reads
//! oldest-to-newest. Emacs files at the end of the subtree and so does refile
//! (`refile::targets_in` → `subtree_end(...) + 1`); this is the same
//! computation against a file read from disk rather than the open buffer, and
//! deliberately shares [`crate::headline::subtree_end`] so the two cannot drift.
//!
//! ## Matching a headline by name
//!
//! `refile::title_of` keeps the TODO keyword and the tags on purpose — its
//! consumer is a picker, and typing `TODO` there is useful. A *template* names
//! its target in a config file, so the opposite is true: a user writing
//! `headline = "Vocabulary"` must not have their capture silently start
//! appending the day someone adds `:drill:` to that headline. So the comparison
//! here strips both and compares what is left.
//!
//! ## An absent headline appends and says so
//!
//! Not "creates it", and not "refuses". Creating invents structure the user did
//! not ask for, in a file they may not have looked at in months. Refusing loses
//! a note they have already typed — the one outcome capture must never produce.
//! Appending keeps the note and moves it somewhere findable; the echo is what
//! tells them the target moved.

/// Where a capture should be written, resolved against the file's current text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insertion {
    /// Append at the end of the file — a plain `file` target, a file that does
    /// not exist yet, or a headline that could not be found.
    Append,
    /// Insert before this 0-based line: the line after the target subtree's
    /// last. Past-the-end is fine and the host clamps it (`file-anchor.line`).
    AtLine(u32),
}

/// Resolve `headline` against `text`, or [`Insertion::Append`] when it is absent.
///
/// Pure over the file's text so the interesting behaviour — which line, and what
/// happens when the headline is missing — is testable without a filesystem, a
/// capability grant or a running editor.
/// `#[cfg(test)]`: production resolves the outline at the call site, because
/// only there is it known whether `parse-file` answered. This keeps the text
/// path's own behaviour — which line, and what happens when the headline is
/// missing — testable without a filesystem, a capability grant or an editor.
#[cfg(test)]
pub fn resolve(text: &str, headline: &str) -> Insertion {
    let lines: Vec<&str> = text.lines().collect();
    let outline = crate::headline::outline_text(&lines);
    resolve_in(&lines, &outline, headline)
}

/// [`resolve`] over an outline someone else produced (OT.8).
///
/// The capture action calls this with `tree-sitter.parse-file`'s walk, so a
/// template naming `headline = "Vocabulary"` cannot match a `* Vocabulary` line
/// written as an example inside a `#+BEGIN_SRC` block — which would file the
/// note into the middle of a code block and report success.
///
/// Shares [`crate::headline::Entry`] with `refile::targets_from`, one level up
/// from the `subtree_end` sharing this module's header describes, and for the
/// same reason: the two insertion points must not drift.
pub fn resolve_in(lines: &[&str], outline: &[crate::headline::Entry], headline: &str) -> Insertion {
    let wanted = normalise(headline);
    if wanted.is_empty() {
        return Insertion::Append;
    }
    // First match wins. A file with two headlines of the same name is already
    // ambiguous to a human reading it; picking the first is the answer that
    // matches how the user's eye finds it, and it is stable across edits below.
    let found = outline.iter().find(|entry| {
        lines
            .get(entry.line as usize)
            .and_then(|l| l.get(entry.level..))
            .is_some_and(|rest| normalise(&strip_tags(rest)) == wanted)
    });

    match found {
        Some(entry) => Insertion::AtLine(entry.end_line + 1),
        None => Insertion::Append,
    }
}

/// A headline's text with its TODO keyword and tags removed, lowercased and
/// whitespace-collapsed.
///
/// Case- and spacing-insensitive because this compares a hand-written config
/// string against a hand-written file, and `"vocabulary"` vs `"Vocabulary"` is
/// not a distinction a user means to make when they disagree with themselves
/// across two files.
fn normalise(s: &str) -> String {
    strip_todo(s.trim())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Drop a leading TODO-ish keyword — a leading all-caps word.
///
/// Deliberately shape-based rather than read from `org.todo-keywords`: the
/// keyword set is per-file (`#+TODO:`) and per-user, and a capture target that
/// resolved differently depending on an unrelated option would be a very
/// confusing thing to debug. An all-caps first word is what every org TODO
/// keyword looks like, and a headline whose first word is genuinely an
/// all-caps ordinary word can be matched by writing it out in full.
fn strip_todo(s: &str) -> &str {
    let Some((first, rest)) = s.split_once(char::is_whitespace) else {
        return s;
    };
    let keywordish = !first.is_empty()
        && first
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '-' || c == '_');
    if keywordish {
        rest.trim_start()
    } else {
        s
    }
}

/// Drop a trailing `:tag:tag:` cluster.
fn strip_tags(s: &str) -> String {
    let trimmed = s.trim_end();
    let Some(open) = trimmed.rfind(" :") else {
        return trimmed.to_string();
    };
    let tail = &trimmed[open + 1..];
    let is_tags = tail.len() >= 2
        && tail.starts_with(':')
        && tail.ends_with(':')
        && tail[1..tail.len() - 1].split(':').all(|t| {
            !t.is_empty()
                && t.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '@')
        });
    if is_tags {
        trimmed[..open].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OT.8's twin: what the TEXT outline answers for a headline that exists
    /// only as an example inside a `#+BEGIN_SRC` block.
    ///
    /// Not "it files into the block" — it does not. `subtree_end` stops at the
    /// next real headline, so the note lands just after `#+END_SRC`, attributed
    /// to a heading that is not there. Wrong in a quieter way than filing into
    /// the block would be, and quieter is worse: the note is somewhere the user
    /// has no reason to look, and capture reported success.
    ///
    /// The tree answers `Append` for the same input, which is OC.5a's
    /// missing-target contract and comes with the warning that says so.
    /// `a_capture_target_inside_a_block_is_not_a_target` in
    /// `tests/org_structure.rs` is the other half.
    #[test]
    fn the_text_outline_matches_a_headline_written_inside_a_block() {
        let text =
            "* Inbox\n#+BEGIN_SRC org\n* Vocabulary\nexample content\n#+END_SRC\n* Later\ntail\n";
        assert_eq!(
            resolve(text, "Vocabulary"),
            Insertion::AtLine(5),
            "the text scan takes the block's example as a real headline and \
             files just past the block's end"
        );
    }

    const FILE: &str = "\
#+TITLE: Notes
* Inbox
captured stuff
* Vocabulary
** French
un mot
** German
ein Wort
* Archive
old things
";

    /// The headline the template names decides the line, and the note lands
    /// AFTER the whole subtree — not in front of its children.
    #[test]
    fn a_headline_target_inserts_after_its_whole_subtree() {
        // `* Vocabulary` is line 3; its subtree swallows both `**` children and
        // runs through `ein Wort` (line 7), so the insertion point is line 8 —
        // the `* Archive` line, which the new entry is inserted in front of.
        assert_eq!(resolve(FILE, "Vocabulary"), Insertion::AtLine(8));
    }

    /// A nested headline is targetable, and its subtree ends at its SIBLING,
    /// not at the next top-level headline.
    #[test]
    fn a_nested_headline_stops_at_its_sibling() {
        // `** French` is line 4, its subtree is `un mot` (5), so 6 — `** German`.
        assert_eq!(resolve(FILE, "French"), Insertion::AtLine(6));
    }

    /// The last headline in the file targets past the end; the host clamps it.
    #[test]
    fn the_last_headline_targets_past_the_end() {
        assert_eq!(resolve(FILE, "Archive"), Insertion::AtLine(10));
    }

    /// **The failure mode this slice exists to make honest.** A headline that
    /// is not there appends — the note survives — rather than being refused or
    /// causing the headline to be invented.
    #[test]
    fn a_missing_headline_appends_rather_than_losing_the_note() {
        assert_eq!(resolve(FILE, "Nowhere"), Insertion::Append);
    }

    /// A file that does not exist yet reads as empty; capture into it appends
    /// and thereby creates it.
    #[test]
    fn an_empty_file_appends() {
        assert_eq!(resolve("", "Vocabulary"), Insertion::Append);
    }

    /// Tags must not break a target. A user writes `headline = "Vocabulary"`
    /// once; adding `:drill:` to that headline months later must not silently
    /// send every future capture to the bottom of the file.
    #[test]
    fn tags_on_the_headline_do_not_break_the_match() {
        let text = "* Vocabulary :drill:fc:\nun mot\n* Next\n";
        assert_eq!(resolve(text, "Vocabulary"), Insertion::AtLine(2));
    }

    /// Same for a TODO keyword appearing on the target headline.
    #[test]
    fn a_todo_keyword_on_the_headline_does_not_break_the_match() {
        let text = "* TODO Vocabulary\nun mot\n* Next\n";
        assert_eq!(resolve(text, "Vocabulary"), Insertion::AtLine(2));
    }

    /// Case and inner spacing are not distinctions a user means to make when
    /// their config and their file disagree.
    #[test]
    fn matching_ignores_case_and_extra_spacing() {
        let text = "* My   Notes\nbody\n* Next\n";
        assert_eq!(resolve(text, "my notes"), Insertion::AtLine(2));
    }

    /// Body text that merely mentions the name is not a headline.
    #[test]
    fn a_line_that_is_not_a_headline_never_matches() {
        let text = "* Real\nVocabulary is mentioned here\n";
        assert_eq!(resolve(text, "Vocabulary"), Insertion::Append);
    }

    /// A blank headline in the template is the plain-file target. The parser
    /// already degrades this shape, so this is belt-and-braces at the second
    /// place that could get it wrong.
    #[test]
    fn a_blank_headline_appends() {
        assert_eq!(resolve(FILE, "   "), Insertion::Append);
    }

    /// Two headlines with the same name: the first wins, and it is the one the
    /// user's eye finds first too.
    #[test]
    fn a_duplicate_headline_resolves_to_the_first() {
        let text = "* Dup\na\n* Other\nb\n* Dup\nc\n";
        assert_eq!(resolve(text, "Dup"), Insertion::AtLine(2));
    }

    /// A trailing `:` that is not a tag cluster is part of the title.
    #[test]
    fn a_colon_that_is_not_a_tag_cluster_stays_in_the_title() {
        let text = "* See also: refs\nbody\n* Next\n";
        assert_eq!(resolve(text, "See also: refs"), Insertion::AtLine(2));
    }
}

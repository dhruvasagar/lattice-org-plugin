//! Taking a subtree OUT of this file (OM.6b, and OM.11's refile after it).
//!
//! Archive and refile differ only in which file the text lands in. Both have
//! to answer the same two questions about the source, and both get them wrong
//! in the same way if they answer by hand:
//!
//! 1. **What text moves** — the subtree's lines, joined.
//! 2. **What span disappears** — and that is NOT simply the subtree's lines.
//!    Deleting `[start,0)..(end, len(end))` leaves the newline that terminated
//!    the line above plus an empty line where the subtree was. The correct
//!    span swallows one line break, and WHICH break depends on whether the
//!    subtree runs to the end of the file: a subtree in the middle takes the
//!    break that FOLLOWS it, one at the end takes the break that PRECEDES it.
//!    A subtree that is the entire file takes neither.
//!
//! Getting (2) wrong is invisible in a test that only checks the archive file
//! and shows up in the source as a growing run of blank lines, one per archive.
//! So it is computed once, here, over a line accessor — the same shape
//! `headline` uses and for the same reason (`headline.rs`, "Why these take a
//! line accessor").

use crate::headline;

/// Where org files a subtree by default: `org-archive-location`'s `"%s_archive"`.
///
/// Derived from the source file rather than configured, which is what makes
/// `<leader>o$` mean the same thing in every org file without the user
/// declaring anything. (Emacs's location also carries a `::headline` part
/// naming where in the target to file it; this plugin appends, so there is
/// nothing yet for that half to select.)
pub fn archive_path(source: &str) -> String {
    format!("{source}_archive")
}

/// The text to move and the span to remove, for the subtree enclosing
/// `cursor_line`.
///
/// `None` when the cursor is not under any headline — a file's preamble has no
/// subtree to take, and guessing the first one would archive something the
/// user was not looking at.
///
/// The returned span is `(start_line, start_byte, end_line, end_byte)`, ready
/// to become a `range`. The text ends in a newline: the target file is a list
/// of lines, and a headline spliced onto whatever was there before is not one.
pub fn extract_subtree(
    line: impl Fn(u32) -> Option<String>,
    cursor_line: u32,
    line_count: u32,
) -> Option<(String, (u32, u32, u32, u32))> {
    let (start, _) = headline::enclosing_headline(&line, cursor_line)?;
    let end = headline::subtree_end(&line, start, line_count);

    let lines: Vec<String> = (start..=end).map(&line).collect::<Option<_>>()?;
    let text = format!("{}\n", lines.join("\n"));

    let last_len = lines.last().map_or(0, |l| l.len()) as u32;

    // The line break that goes with the subtree.
    //
    // The question is NOT "is this the last headline" but "is there anything
    // after it at all" — and `line(end + 1)` is the only thing that answers it
    // honestly. A file ending in a newline has an empty remainder there even
    // when `end` is its last CONTENT line, and taking the break above in that
    // case leaves the newline behind: archiving the whole of `"* Two\n"` would
    // empty the file down to a single blank line rather than to nothing.
    let span = if line(end + 1).is_some() {
        (start, 0, end + 1, 0)
    } else if start > 0 {
        // Genuinely the end of the file, with no trailing newline. Take the
        // break that PRECEDES the subtree, or the line above keeps a dangling
        // one and the file grows a blank line per archive.
        let above = line(start - 1)?.len() as u32;
        (start - 1, above, end, last_len)
    } else {
        // The subtree is the entire file and the file has no trailing newline:
        // there is no break at either end to take.
        (start, 0, end, last_len)
    };

    Some((text, span))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the `document` handle that reproduces the host's line
    /// contract exactly, INCLUDING the part that bit: `line_count` is content
    /// lines, so `"a\n"` is one — but `line(1)` still answers `Some("")`,
    /// because the trailing newline opens a remainder the count does not
    /// admit to. An accessor built from `str::lines` hides that, and hiding it
    /// is what let the whole-file archive leave a blank line behind.
    fn accessor(text: &'static str) -> (impl Fn(u32) -> Option<String>, u32) {
        // `split` leaves the remainder after a trailing newline as an empty
        // piece — addressable, and deliberately not counted.
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let count = if text.ends_with('\n') {
            lines.len().saturating_sub(1)
        } else {
            lines.len()
        } as u32;
        (move |n: u32| lines.get(n as usize).cloned(), count)
    }

    #[test]
    fn the_archive_file_sits_beside_the_source() {
        assert_eq!(archive_path("/org/today.org"), "/org/today.org_archive");
    }

    #[test]
    fn a_subtree_takes_its_children_with_it() {
        let (line, count) = accessor("* One\nbody\n** Child\n* Two\n");
        // From the body line, which belongs to `* One` — the enclosing
        // headline is what moves, children included.
        let (text, _) = extract_subtree(line, 1, count).unwrap();
        assert_eq!(text, "* One\nbody\n** Child\n");
    }

    /// From a CHILD headline it is the child that moves, not its parent.
    /// Org's archive acts on the subtree at point, and a key that quietly
    /// took the parent too would remove siblings the user was not looking at.
    #[test]
    fn from_a_child_headline_only_the_child_moves() {
        let (line, count) = accessor("* One\n** Child\nbody\n* Two\n");
        let (text, _) = extract_subtree(line, 2, count).unwrap();
        assert_eq!(text, "** Child\nbody\n");
    }

    /// A subtree with a neighbour below takes the break that FOLLOWS it, so
    /// the next headline moves up to where it was.
    #[test]
    fn a_middle_subtree_leaves_no_blank_line() {
        let (line, count) = accessor("* One\nbody\n* Two\n");
        let (_, span) = extract_subtree(line, 0, count).unwrap();
        assert_eq!(span, (0, 0, 2, 0));
    }

    /// The last subtree of a newline-terminated file takes the break that
    /// TERMINATES it — the remainder after the final newline is "something
    /// after", even though `line_count` does not count it.
    #[test]
    fn the_last_subtree_takes_its_own_terminator() {
        let (line, count) = accessor("* One\n* Two\nbody\n");
        let (_, span) = extract_subtree(line, 1, count).unwrap();
        assert_eq!(span, (1, 0, 3, 0));
    }

    /// Archiving everything empties the file. It left `"\n"` before the rule
    /// keyed off `line(end + 1)` instead of `end + 1 < line_count`, and a
    /// blank line accumulating per archive is exactly the failure this module
    /// exists to prevent.
    #[test]
    fn archiving_the_whole_file_leaves_nothing() {
        let (line, count) = accessor("* Only\nbody\n");
        let (text, span) = extract_subtree(line, 0, count).unwrap();
        assert_eq!(text, "* Only\nbody\n");
        assert_eq!(span, (0, 0, 2, 0));
    }

    /// With no trailing newline there IS no break after the last subtree, so
    /// it takes the one above.
    #[test]
    fn an_unterminated_last_subtree_takes_the_break_above_it() {
        let (line, count) = accessor("* One\n* Two\nbody");
        let (_, span) = extract_subtree(line, 1, count).unwrap();
        // From the end of `* One` to the end of `body`.
        assert_eq!(span, (0, 5, 2, 4));
    }

    /// And a one-subtree file with no trailing newline has neither.
    #[test]
    fn an_unterminated_whole_file_subtree_takes_neither_break() {
        let (line, count) = accessor("* Only\nbody");
        let (_, span) = extract_subtree(line, 0, count).unwrap();
        assert_eq!(span, (0, 0, 1, 4));
    }

    #[test]
    fn the_preamble_has_no_subtree_to_archive() {
        let (line, count) = accessor("intro text\n* One\n");
        assert!(extract_subtree(line, 0, count).is_none());
    }
}

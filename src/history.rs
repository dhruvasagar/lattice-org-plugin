//! HB.3 — the completion history a habit's graph is drawn from.
//!
//! Design: `docs/dev/architecture/org-habits.md` §1 and §3 (lattice repo).
//!
//! [`complete`](crate::complete) writes a state-change line on every
//! completion; this reads them back. The two are inverses and must stay so,
//! but the reader is deliberately the more permissive of the pair, because
//! **emacs wrote most of the lines this will ever see.** A user's org files
//! have years of history in them already, which is what lets the graph show
//! something real the day it lands — and none of that history came from here.
//!
//! So the parse tolerates what org emits rather than only what we emit:
//!
//! - `org-log-note-headings` pads the state names (`State %-12s from %-12s`),
//!   so the gaps between tokens are arbitrary runs of spaces.
//! - A completion with an attached note ends the line with ` \\` and continues
//!   on the next one.
//! - `org-log-into-drawer` may be off, in which case the line sits loose under
//!   the planning line instead of inside `:LOGBOOK:`. Both are read; requiring
//!   the drawer would make the graph blank for exactly the users who turned
//!   the option off.
//!
//! ## What counts as a completion
//!
//! A state change **into a done keyword**. `TODO` → `NEXT` is a state change
//! and is not a completion, so the new state is checked against the sequence's
//! done set rather than against the literal word `DONE` — a user whose
//! sequence ends in `CANCELLED | DONE` gets both, and one whose done state is
//! `FINI` gets theirs.
//!
//! Clock lines, notes, and anything else in the drawer are ignored rather than
//! guessed at.
//!
//! ## `:LAST_REPEAT:` is added, not fallen back to
//!
//! The design calls it a fallback for habits whose log was trimmed, and the
//! implementation is simply to include it always and deduplicate. A
//! conditional fallback has to decide *when* the log counts as trimmed, and
//! every rule for that is wrong somewhere: a log holding only older entries is
//! neither empty nor complete. Adding it and deduping is correct in both
//! cases and cannot lose a completion.

use crate::org_date::Date;
use crate::{headline, repeat, timestamp};

/// Every day the task under `lines[0]` was completed, ascending and unique.
///
/// `lines` is a subtree, headline first — the same shape
/// [`complete_repeating`](crate::complete::complete_repeating) takes.
///
/// One day is one completion: a habit finished twice on a Tuesday fills one
/// cell in the graph, not two, so the same date arriving from a log line and
/// from `:LAST_REPEAT:` collapses to one entry.
pub fn completions(lines: &[String], done_keywords: &[String]) -> Vec<Date> {
    let mut out: Vec<Date> = Vec::new();
    for line in &lines[1..own_lines(lines)] {
        if let Some(date) = state_change_into_done(line, done_keywords) {
            out.push(date);
        }
        if let Some(date) = last_repeat(line) {
            out.push(date);
        }
    }
    out.sort_by_key(|d| repeat::to_days(*d));
    out.dedup();
    out
}

/// How far into `lines` this headline's own log extends: up to the first
/// nested headline.
///
/// A subtree contains its children, and a child's `:LOGBOOK:` records the
/// child's completions. Reading the whole subtree would draw a parent's graph
/// from its children's history — plausible-looking and wrong, and wrong in the
/// direction that makes a habit look better kept than it is.
fn own_lines(lines: &[String]) -> usize {
    lines[1..]
        .iter()
        .position(|l| headline::headline_level(l).is_some())
        .map(|i| i + 1)
        .unwrap_or(lines.len())
}

/// `- State "DONE" from "NEXT" [2026-09-03 Wed 09:14]` → the date, when the
/// new state is a done keyword.
fn state_change_into_done(line: &str, done_keywords: &[String]) -> Option<Date> {
    let rest = line.trim_start().strip_prefix("- ")?.trim_start();
    let rest = rest.strip_prefix("State")?;
    // The new state is the first quoted field; `from "OLD"` follows and is not
    // read. Which state it came FROM does not change that the task was
    // finished, and org omits the clause entirely for a task that had no
    // previous keyword.
    let new_state = first_quoted(rest)?;
    done_keywords
        .iter()
        .any(|d| *d == new_state)
        .then_some(())?;
    stamp_date(line)
}

/// `:LAST_REPEAT: [2026-09-03 Wed 09:14]` → the date.
fn last_repeat(line: &str) -> Option<Date> {
    let rest = line.trim().strip_prefix(':')?;
    let (key, value) = rest.split_once(':')?;
    key.eq_ignore_ascii_case("LAST_REPEAT").then_some(())?;
    stamp_date(value)
}

/// The line's first INACTIVE timestamp, as a date.
///
/// Inactive because that is what org logs with, and the distinction carries
/// meaning: an active `<...>` stamp in a note is something the user scheduled,
/// not something they recorded finishing. Reading it as a completion would
/// paint a future day as done.
fn stamp_date(line: &str) -> Option<Date> {
    let stamp = timestamp::first_stamp(line)?;
    (!stamp.active).then_some(())?;
    Some(Date {
        year: stamp.year,
        month: stamp.month,
        day: stamp.day,
    })
}

/// The contents of the first `"..."` in `s`.
fn first_quoted(s: &str) -> Option<String> {
    let open = s.find('"')?;
    let rest = &s[open + 1..];
    let close = rest.find('"')?;
    Some(rest[..close].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn done() -> Vec<String> {
        vec!["DONE".to_string(), "CANCELLED".to_string()]
    }

    fn ymd(dates: &[Date]) -> Vec<(i32, u32, u32)> {
        dates.iter().map(|d| (d.year, d.month, d.day)).collect()
    }

    /// The round trip that matters most: what `complete` writes, read back.
    #[test]
    fn a_line_this_plugin_wrote_is_read_back() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 SCHEDULED: <2026-09-05 Sat .+2d/4d>\n\
             \x20 :LOGBOOK:\n\
             \x20 - State \"DONE\" from \"NEXT\" [2026-09-03 Thu 09:14]\n\
             \x20 :END:\n",
        );
        assert_eq!(ymd(&completions(&subtree, &done())), [(2026, 9, 3)]);
    }

    /// And the one that matters most in practice: what EMACS wrote. Org pads
    /// the state fields to 12 columns, so a parser that split on single spaces
    /// would read every file in the wild as having no history at all.
    #[test]
    fn emacs_padding_between_the_fields_is_tolerated() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 :LOGBOOK:\n\
             \x20 - State \"DONE\"       from \"NEXT\"       [2026-08-30 Sun 21:02]\n\
             \x20 :END:\n",
        );
        assert_eq!(ymd(&completions(&subtree, &done())), [(2026, 8, 30)]);
    }

    /// A completion with a note attached: org ends the line with ` \\` and
    /// continues below. The stamp is still on the first line.
    #[test]
    fn a_completion_carrying_a_note_still_counts() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 :LOGBOOK:\n\
             \x20 - State \"DONE\" from \"NEXT\" [2026-08-30 Sun 21:02] \\\\\n\
             \x20   only the front ones\n\
             \x20 :END:\n",
        );
        assert_eq!(ymd(&completions(&subtree, &done())), [(2026, 8, 30)]);
    }

    /// `org-log-into-drawer` off. The whole point of honouring the option in
    /// `complete` is lost if the reader only looks in the drawer.
    #[test]
    fn a_loose_log_line_outside_any_drawer_counts() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 SCHEDULED: <2026-09-05 Sat .+2d/4d>\n\
             \x20 - State \"DONE\" from \"NEXT\" [2026-09-01 Tue 08:00]\n",
        );
        assert_eq!(ymd(&completions(&subtree, &done())), [(2026, 9, 1)]);
    }

    /// The gate this module turns on. A state change is not a completion.
    #[test]
    fn a_change_into_a_live_state_is_not_a_completion() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 :LOGBOOK:\n\
             \x20 - State \"NEXT\" from \"TODO\" [2026-09-01 Tue 08:00]\n\
             \x20 :END:\n",
        );
        assert!(completions(&subtree, &done()).is_empty());
    }

    /// The done set is the user's, not the word `DONE`. A sequence ending in
    /// `CANCELLED` records a cancellation as a completion because org does —
    /// the habit's day was resolved either way.
    #[test]
    fn every_done_keyword_of_the_sequence_counts() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 :LOGBOOK:\n\
             \x20 - State \"CANCELLED\" from \"NEXT\" [2026-09-02 Wed 08:00]\n\
             \x20 :END:\n",
        );
        assert_eq!(ymd(&completions(&subtree, &done())), [(2026, 9, 2)]);
        // And a done set that does not contain it reads the same line as
        // nothing — the keyword sequence is what decides, not this module.
        assert!(completions(&subtree, &["DONE".to_string()]).is_empty());
    }

    /// Everything else in a drawer is left alone rather than guessed at.
    #[test]
    fn clock_lines_and_notes_are_not_completions() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 :LOGBOOK:\n\
             \x20 CLOCK: [2026-09-01 Tue 08:00]--[2026-09-01 Tue 08:30] =>  0:30\n\
             \x20 - Note taken on [2026-09-01 Tue 09:00] \\\\\n\
             \x20   thinking about it is not doing it\n\
             \x20 :END:\n",
        );
        assert!(completions(&subtree, &done()).is_empty());
    }

    /// The trimmed-log case `:LAST_REPEAT:` exists for.
    #[test]
    fn last_repeat_alone_is_a_completion() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 :PROPERTIES:\n\
             \x20 :STYLE: habit\n\
             \x20 :LAST_REPEAT: [2026-09-03 Thu 09:14]\n\
             \x20 :END:\n",
        );
        assert_eq!(ymd(&completions(&subtree, &done())), [(2026, 9, 3)]);
    }

    /// And the ordinary case, where it duplicates the newest log line. One
    /// day, one cell.
    #[test]
    fn last_repeat_agreeing_with_the_log_yields_one_day() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 :PROPERTIES:\n\
             \x20 :LAST_REPEAT: [2026-09-03 Thu 09:14]\n\
             \x20 :END:\n\
             \x20 :LOGBOOK:\n\
             \x20 - State \"DONE\" from \"NEXT\" [2026-09-03 Thu 09:14]\n\
             \x20 - State \"DONE\" from \"NEXT\" [2026-09-01 Tue 08:00]\n\
             \x20 :END:\n",
        );
        assert_eq!(
            ymd(&completions(&subtree, &done())),
            [(2026, 9, 1), (2026, 9, 3)],
            "ascending, and the duplicated day appears once"
        );
    }

    /// Two completions on one day is one day. Org allows it (complete, undo,
    /// complete) and the graph has one cell per day.
    #[test]
    fn two_completions_on_one_day_are_one_day() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 :LOGBOOK:\n\
             \x20 - State \"DONE\" from \"NEXT\" [2026-09-03 Thu 21:00]\n\
             \x20 - State \"DONE\" from \"NEXT\" [2026-09-03 Thu 09:14]\n\
             \x20 :END:\n",
        );
        assert_eq!(ymd(&completions(&subtree, &done())), [(2026, 9, 3)]);
    }

    /// A child's history belongs to the child. Reading the whole subtree would
    /// make a parent's graph look better kept than the parent is.
    #[test]
    fn a_nested_headlines_log_is_not_the_parents() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 :LOGBOOK:\n\
             \x20 - State \"DONE\" from \"NEXT\" [2026-09-03 Thu 09:14]\n\
             \x20 :END:\n\
             ** NEXT The back ones\n\
             \x20 :LOGBOOK:\n\
             \x20 - State \"DONE\" from \"NEXT\" [2026-09-04 Fri 09:14]\n\
             \x20 :END:\n",
        );
        assert_eq!(ymd(&completions(&subtree, &done())), [(2026, 9, 3)]);
    }

    /// An ACTIVE stamp is something scheduled, not something recorded. Reading
    /// one as a completion would paint a future day as done.
    #[test]
    fn an_active_stamp_is_not_a_completion_record() {
        let subtree = lines(
            "* NEXT Water the plants\n\
             \x20 - State \"DONE\" from \"NEXT\" <2027-01-01 Fri 09:14>\n",
        );
        assert!(completions(&subtree, &done()).is_empty());
    }

    /// A headline with nothing under it. The graph's empty case has to be an
    /// empty list rather than a panic — most tasks are not habits.
    #[test]
    fn a_bare_headline_has_no_history() {
        assert!(completions(&lines("* TODO Ship it\n"), &done()).is_empty());
    }
}

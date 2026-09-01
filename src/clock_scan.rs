//! OA.14b — the time a file logged, for the agenda's clock report.
//!
//! One pass over a file's lines producing one [`Span`] per (headline, day).
//!
//! ## Why this is not a view of the agenda's rows
//!
//! Emacs's clocktable totals every clocked headline in the agenda files. Agenda
//! rows are a FILTERED subset — a headline clocked yesterday with no TODO and
//! no date is not a row at all — so hanging clock data off a row would report
//! only the time that happened to land on one and silently drop the rest. A
//! report that under-reports is worse than none: nothing distinguishes a quiet
//! week from a lossy scan.
//!
//! So this walks headlines independently of whatever the sections admitted, and
//! rides the same `scan` call so the host still makes one guest call per file.
//!
//! ## What it does not do, deliberately
//!
//! - **A running clock contributes nothing.** `CLOCK: [start]` with no `--end`
//!   has no duration yet; inventing one from "now" would make the report
//!   disagree with the file and change under you between two refreshes.
//! - **A span that crosses midnight is filed whole on the day it began**,
//!   rather than split across days. Emacs splits it. Matching that is a
//!   refinement the record's shape already allows, and it is a rounding
//!   difference rather than lost time.
//! - **The `=> H:MM` summary org writes is ignored.** The duration is computed
//!   from the two stamps, so a hand-edited or stale summary cannot make the
//!   report disagree with the timestamps it is derived from.

use crate::clock::{Now, CLOCK};
use crate::timestamp;

/// Time logged on one headline on one day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// 0-based line of the HEADLINE, not of the `CLOCK:` line.
    pub line: u32,
    /// Outline path, outermost first, this headline last.
    pub outline: Vec<String>,
    /// Days since the Unix epoch.
    pub day: i64,
    /// Minutes, summed across every `CLOCK:` line for this headline on this day.
    pub minutes: u32,
}

/// The stars prefix of a headline line, as a level. `None` for a non-headline.
fn headline_level(line: &str) -> Option<usize> {
    let stars = line.len() - line.trim_start_matches('*').len();
    if stars == 0 {
        return None;
    }
    // `*bold*` at the start of a line is not a headline; org requires a space.
    match line.as_bytes().get(stars) {
        Some(b' ') => Some(stars),
        _ => None,
    }
}

/// The headline's text with its stars and the following space removed.
fn headline_text(line: &str, level: usize) -> String {
    line[level..].trim().to_string()
}

/// Parse one `CLOCK:` line into `(epoch_day, minutes)`.
///
/// `None` for a running clock, a malformed line, or a negative duration — an
/// end before its start is a file that was edited by hand into a state no
/// clock-out produces, and guessing at it would put invented time in a total.
fn closed_span(line: &str) -> Option<(i64, u32)> {
    let t = line.trim_start();
    if !t.starts_with(CLOCK.trim_end()) {
        return None;
    }
    let start = timestamp::first_stamp(t)?;
    let rest = &t[start.end..];
    if !rest.trim_start().starts_with("--") {
        // Running: no end yet, so no duration to report.
        return None;
    }
    let end = timestamp::first_stamp(rest)?;
    let to_now = |s: &timestamp::Stamp| {
        let (hour, minute) = s.time.unwrap_or((0, 0));
        Now {
            year: s.year,
            month: s.month,
            day: s.day,
            hour,
            minute,
        }
    };
    let minutes = to_now(&end).epoch_minutes() - to_now(&start).epoch_minutes();
    if minutes < 0 {
        return None;
    }
    Some((
        timestamp::epoch_day(start.year, start.month, start.day),
        minutes as u32,
    ))
}

/// Every clocked span in `text`, aggregated per (headline, day).
///
/// Line-based rather than tree-based, unlike the agenda's own tree scan. The
/// two things this needs — which headline a `CLOCK:` line sits under, and the
/// outline path to it — are exactly what a stars-and-level walk gives, and it
/// works on a host with no org grammar loaded, where the tree path does not.
/// A `CLOCK:` line inside a `#+BEGIN_SRC` block is the one thing that would
/// fool it; a source block containing a literal LOGBOOK clock entry is rare
/// enough to accept where a phantom agenda ROW was not (OT.3).
pub fn scan(text: &str) -> Vec<Span> {
    // The ancestor chain: (level, headline text) for each open outline level.
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut current: Option<(u32, Vec<String>)> = None;
    // (headline line, day) → minutes. Keyed by line rather than by path so two
    // sibling headlines with identical text stay distinct.
    let mut totals: Vec<Span> = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        if let Some(level) = headline_level(raw) {
            let title = headline_text(raw, level);
            // Pop to the parent level, then push this one — the level-pop that
            // stops a sibling inheriting its neighbour's ancestors.
            while stack.last().is_some_and(|(l, _)| *l >= level) {
                stack.pop();
            }
            stack.push((level, title));
            current = Some((
                i as u32,
                stack.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>(),
            ));
            continue;
        }
        let Some((line, outline)) = current.as_ref() else {
            // A `CLOCK:` line before any headline belongs to nothing that can
            // be named in a hierarchy, so it is skipped rather than filed
            // under an invented root.
            continue;
        };
        let Some((day, minutes)) = closed_span(raw) else {
            continue;
        };
        if minutes == 0 {
            // Emacs's `:fileskip0` hides zero-duration rows; producing none is
            // the same answer one step earlier.
            continue;
        }
        match totals.iter_mut().find(|s| s.line == *line && s.day == day) {
            Some(existing) => existing.minutes += minutes,
            None => totals.push(Span {
                line: *line,
                outline: outline.clone(),
                day,
                minutes,
            }),
        }
    }
    totals
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const FILE: &str = "\
* Project
:LOGBOOK:
CLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:30] =>  1:30
:END:
** Subtask
:LOGBOOK:
CLOCK: [2026-08-27 Thu 11:00]--[2026-08-27 Thu 11:30] =>  0:30
CLOCK: [2026-08-28 Fri 09:00]--[2026-08-28 Fri 10:00] =>  1:00
:END:
* Other
";

    #[test]
    fn a_span_is_filed_under_its_headline_with_its_outline_path() {
        let spans = scan(FILE);
        assert_eq!(spans.len(), 3, "got {spans:?}");
        assert_eq!(spans[0].outline, vec!["Project"]);
        assert_eq!(spans[0].minutes, 90);
        // The child carries its ANCESTORS, which is what lets the host build
        // the report tree without a span for every parent.
        assert_eq!(spans[1].outline, vec!["Project", "Subtask"]);
        assert_eq!(spans[1].minutes, 30);
        assert_eq!(spans[2].outline, vec!["Project", "Subtask"]);
        assert_eq!(spans[2].minutes, 60);
        // Two days, two spans — the granularity a `:step day` report renders.
        assert_ne!(spans[1].day, spans[2].day);
    }

    /// Several clock lines for one headline on one day are ONE span. A report
    /// sums them anyway; doing it here keeps a file with years of history from
    /// crossing thousands of records to be summed on the far side.
    #[test]
    fn several_clock_lines_on_one_day_aggregate() {
        let spans = scan(
            "* A\n:LOGBOOK:\n\
             CLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:00] =>  1:00\n\
             CLOCK: [2026-08-27 Thu 14:00]--[2026-08-27 Thu 14:30] =>  0:30\n\
             :END:\n",
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].minutes, 90);
    }

    /// A running clock has no duration yet. Inventing one from "now" would
    /// make the report disagree with the file and change between refreshes.
    #[test]
    fn a_running_clock_contributes_nothing() {
        let spans = scan("* A\n:LOGBOOK:\nCLOCK: [2026-08-27 Thu 09:00]\n:END:\n");
        assert!(spans.is_empty(), "got {spans:?}");
    }

    /// The `=> H:MM` summary is ignored; the duration comes from the stamps.
    /// A hand-edited summary must not be able to make the total disagree with
    /// the timestamps it is supposedly derived from.
    #[test]
    fn the_duration_comes_from_the_stamps_not_the_summary() {
        let spans = scan(
            "* A\n:LOGBOOK:\nCLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:00] =>  99:00\n:END:\n",
        );
        assert_eq!(spans[0].minutes, 60);
    }

    /// An end before its start is a hand-edited file, not a clock-out. Skipped
    /// rather than counted as negative or absolute time.
    #[test]
    fn an_inverted_span_is_skipped() {
        let spans = scan(
            "* A\n:LOGBOOK:\nCLOCK: [2026-08-27 Thu 10:00]--[2026-08-27 Thu 09:00] => -1:00\n:END:\n",
        );
        assert!(spans.is_empty(), "got {spans:?}");
    }

    /// A sibling does not inherit its neighbour's ancestors — the level-pop
    /// that separates a correct outline walk from a plausible wrong one.
    #[test]
    fn a_sibling_does_not_inherit_its_neighbours_path() {
        let spans = scan(
            "* A\n** A1\n* B\n:LOGBOOK:\n\
             CLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:00] =>  1:00\n:END:\n",
        );
        assert_eq!(spans[0].outline, vec!["B"]);
    }

    /// The whole point of the seam: a headline that is NOT an agenda row still
    /// reports its time. `Notes` carries no TODO and no date.
    #[test]
    fn a_headline_that_is_not_an_agenda_row_still_reports_its_time() {
        let spans = scan(
            "* Notes\n:LOGBOOK:\n\
             CLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:00] =>  1:00\n:END:\n",
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].outline, vec!["Notes"]);
        assert_eq!(spans[0].minutes, 60);
    }

    #[test]
    fn a_file_with_no_clocks_reports_nothing() {
        assert!(scan("* TODO a\n* TODO b\n").is_empty());
    }
}

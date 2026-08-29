//! OC.5 — clock lines, and the `:LOGBOOK:` drawer they live in.
//!
//! This is the plugin's **first drawer primitive**. Nothing in the crate read or
//! wrote a drawer before it: the nearest thing was `agenda.rs`, which knows only
//! that an *inactive* timestamp is never an agenda row — which is what keeps
//! logbook and `CLOSED:` lines out of the view, and which never parses a drawer
//! at all. So `:PROPERTIES:` handling and a future `org-log-into-drawer` both
//! reuse what is here.
//!
//! ## The buffer is the record (D4)
//!
//! An unterminated `CLOCK: [start]` with no `--end` **is** a running clock.
//! Nothing else needs to be true. So clock-out and clock-cancel are pure buffer
//! operations that re-derive their target structurally, and there is no session
//! state to lose: after a restart the modeline is empty, the file is still
//! correct, and clocking out on that entry works. That is why every function
//! here takes only the buffer and answers only from it.
//!
//! ## Position is derived, never taken from the cursor (D5)
//!
//! The cursor's one job is to say *which entry*. Where the line goes is then
//! forced by the grammar: enclosing `section` → past its `plan` → past its
//! `property_drawer` → find-or-create the `LOGBOOK` drawer → insert as its
//! **first** `CLOCK:` line. Newest-first is org's own convention and it also
//! makes "find the running clock" an O(1) look at one line instead of a scan.
//!
//! No enclosing headline is a refusal, not an invented location — a clock line
//! at the top of a file belongs to nothing and cannot be clocked out of.
//!
//! ## Time is integer arithmetic, as everywhere else here
//!
//! `timestamp.rs`'s house style, for its stated reason: no `chrono`, no
//! floating point, and the day name recomputed on write rather than carried.
//! Durations are minutes in an `i64`.

use crate::headline::Headlines;
use crate::timestamp::{self, Stamp};
use crate::tree;

/// The drawer org logs clocks into. Not an option: `org-clock-into-drawer`'s
/// `nil` / numeric variants are a deliberate cut (see the slice plan), and a
/// half-supported option is worse than an honest constant.
pub const LOGBOOK: &str = "LOGBOOK";

/// The prefix every clock line carries.
pub const CLOCK: &str = "CLOCK: ";

/// The planning keywords that may sit between a headline and its drawers. The
/// tree calls the whole line a `plan`; this list is what the text fallback has
/// to recognise instead.
const PLANNING: [&str; 3] = ["DEADLINE:", "SCHEDULED:", "CLOSED:"];

/// A local wall-clock instant, to the minute — what org writes into a clock
/// line.
///
/// **Local**, which a guest cannot work out alone: `wasi:clocks` is UTC and a
/// plugin's environment carries no `TZ`. The caller shifts by the host's
/// `local-utc-offset-seconds` (OC.4) before building one of these, so everything
/// below is plain civil arithmetic with no timezone left in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl Now {
    /// From seconds since the epoch **already shifted into local time**.
    ///
    /// `div_euclid` / `rem_euclid` rather than `/` and `%`: a negative offset
    /// applied near the epoch, or simply a pre-1970 date, must floor rather than
    /// truncate toward zero, or the day rolls the wrong way and the time comes
    /// out negative.
    pub fn from_local_secs(secs: i64) -> Self {
        let day = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (year, month, d) = crate::agenda::civil_from_epoch_day(day);
        Self {
            year,
            month,
            day: d,
            hour: (rem / 3_600) as u32,
            minute: ((rem % 3_600) / 60) as u32,
        }
    }

    /// Minutes since the epoch — the base every duration here is computed in.
    pub fn epoch_minutes(&self) -> i64 {
        timestamp::epoch_day(self.year, self.month, self.day) * 1_440
            + i64::from(self.hour) * 60
            + i64::from(self.minute)
    }

    /// The inactive stamp org writes: `[2026-08-28 Fri 16:02]`.
    ///
    /// Built through [`timestamp::render`] rather than a local `format!`, so the
    /// day name comes from the same Zeller call every other stamp in the plugin
    /// uses and cannot drift from it.
    pub fn stamp(&self) -> String {
        timestamp::render(&Stamp {
            start: 0,
            end: 0,
            active: false,
            year: self.year,
            month: self.month,
            day: self.day,
            time: Some((self.hour, self.minute)),
        })
    }

    /// Read a `Now` back out of a rendered stamp. `None` for a date-only stamp —
    /// a clock line without a time is not a clock line.
    fn from_stamp(s: &Stamp) -> Option<Self> {
        let (hour, minute) = s.time?;
        Some(Self {
            year: s.year,
            month: s.month,
            day: s.day,
            hour,
            minute,
        })
    }
}

/// Org's duration format: `H:MM`, right-aligned to width 2 on the hours, so a
/// column of clock lines lines up (`=>  1:30` beside `=> 11:30`).
///
/// A negative span — a clock-out before its clock-in, which a hand-edited file
/// or a system clock that jumped backwards can produce — renders as `0:00`
/// rather than as a minus sign org has no reading for. The line still records
/// both real stamps, so the wrongness stays visible where it happened.
pub fn duration(minutes: i64) -> String {
    let m = minutes.max(0);
    format!("{:2}:{:02}", m / 60, m % 60)
}

/// A clock line that has a start and no end — a clock that is still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running {
    /// The line the `CLOCK:` sits on.
    pub line: u32,
    /// When it started.
    pub start: Now,
}

/// The text to splice in and where, for a clock-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    /// Insert **before** this line, as a zero-width edit at its column 0.
    pub line: u32,
    /// The text, newline-terminated. Carries the whole `:LOGBOOK:` / `:END:`
    /// scaffold when the entry had no drawer yet.
    pub text: String,
}

/// Everything the clock needs to ask of one entry, resolved from the tree when
/// there is one and from line logic when there is not.
///
/// The same shape as [`Headlines`] and for the same reason: the tree-or-text
/// decision for `(drawer)` and `(plan)` is made HERE, once, rather than at each
/// call site. It borrows its tree from the `Headlines` it is built on so the two
/// can never disagree about which parse they are reading.
pub struct Logbook<'a, 'h> {
    hl: &'a Headlines<'h>,
}

impl<'a, 'h> Logbook<'a, 'h> {
    pub fn new(hl: &'a Headlines<'h>) -> Self {
        Self { hl }
    }

    /// The first line of the entry's body proper: past the headline, past any
    /// planning line, past any `:PROPERTIES:` drawer, and past blank lines.
    ///
    /// This is D5's "skip `plan`, skip `property_drawer`" made concrete, and it
    /// is where a `:LOGBOOK:` either already is or is about to go. Org's own
    /// `org-log-beginning` lands in the same place.
    fn body_start(&self, headline: u32) -> u32 {
        if let Some(section) = self
            .hl
            .tree()
            .and_then(|tree| tree::enclosing(tree, headline, 0, "section"))
        {
            // The grammar's own answer: `section` has `plan` and
            // `property_drawer` as named fields, in that order, before `body`.
            let mut at = headline;
            for field in ["plan", "property_drawer"] {
                if let Some(node) = section.child_by_field(field) {
                    at = at.max(tree::last_content_line(&node.byte_range()));
                }
            }
            return self.skip_blanks(at + 1);
        }
        // Text fallback — the same two things, recognised by their spelling.
        let mut at = headline + 1;
        while self
            .hl
            .text(at)
            .is_some_and(|l| PLANNING.iter().any(|k| l.trim_start().starts_with(k)))
        {
            at += 1;
        }
        if self.is_drawer_open(at, "PROPERTIES") {
            at += 1;
            while let Some(l) = self.hl.text(at) {
                at += 1;
                if l.trim().eq_ignore_ascii_case(":END:") {
                    break;
                }
            }
        }
        self.skip_blanks(at)
    }

    fn skip_blanks(&self, mut at: u32) -> u32 {
        while self.hl.text(at).is_some_and(|l| l.trim().is_empty()) {
            at += 1;
        }
        at
    }

    /// Whether line `at` opens a drawer named `name` (`:LOGBOOK:`).
    fn is_drawer_open(&self, at: u32, name: &str) -> bool {
        self.hl.text(at).is_some_and(|l| {
            let t = l.trim();
            t.len() == name.len() + 2
                && t.starts_with(':')
                && t.ends_with(':')
                && t[1..t.len() - 1].eq_ignore_ascii_case(name)
        })
    }

    /// The `LOGBOOK` drawer's opening line for the entry at `headline`, if it
    /// already has one.
    ///
    /// Deliberately looks at exactly ONE position — the body start — and not
    /// through the entry's whole body. That is not a shortcut: a `:LOGBOOK:`
    /// written inside a `#+BEGIN_EXAMPLE` block further down is example text,
    /// and a scan that found it would clock into a code sample. Org puts the
    /// drawer immediately after the planning and property lines, so the one
    /// place worth looking is the only place it can legitimately be.
    fn drawer_open(&self, headline: u32) -> Option<u32> {
        let at = self.body_start(headline);
        self.is_drawer_open(at, LOGBOOK).then_some(at)
    }

    /// Where a new `CLOCK:` line goes for the entry containing `cursor_line`,
    /// and what to write there.
    ///
    /// `None` when the cursor is above the first headline: a clock line there
    /// would belong to no entry, so refusing is the answer and inventing a
    /// location is not.
    pub fn clock_in(&self, cursor_line: u32, now: Now) -> Option<Insertion> {
        let (headline, _) = self.hl.enclosing(cursor_line)?;
        let entry = format!("{CLOCK}{}", now.stamp());
        Some(match self.drawer_open(headline) {
            // Newest first, so the running clock is always the drawer's first
            // line — which is what makes finding it O(1) rather than a scan.
            Some(open) => Insertion {
                line: open + 1,
                text: format!("{entry}\n"),
            },
            None => Insertion {
                line: self.body_start(headline),
                text: format!(":{LOGBOOK}:\n{entry}\n:END:\n"),
            },
        })
    }

    /// The running clock in the entry containing `cursor_line`, if any.
    ///
    /// Re-derived from the buffer every time and never from remembered state
    /// (D4) — which is precisely why clocking out still works after a restart,
    /// and why a file edited by another tool is read correctly rather than
    /// argued with.
    pub fn running(&self, cursor_line: u32) -> Option<Running> {
        let (headline, _) = self.hl.enclosing(cursor_line)?;
        let open = self.drawer_open(headline)?;
        // Only the drawer's own lines, and only up to its `:END:` — so a clock
        // line in the NEXT entry's drawer can never be mistaken for this one's.
        let mut at = open + 1;
        while let Some(text) = self.hl.text(at) {
            if text.trim().eq_ignore_ascii_case(":END:") {
                return None;
            }
            if let Some(start) = running_start(&text) {
                return Some(Running { line: at, start });
            }
            at += 1;
        }
        None
    }
}

/// The start instant of a line that is a RUNNING clock — a `CLOCK:` with one
/// stamp and no `--end`.
///
/// A closed line answers `None`, which is what makes "the first `CLOCK:` in the
/// drawer" and "the running clock" the same lookup: if the newest line is
/// already closed, nothing is running.
fn running_start(text: &str) -> Option<Now> {
    let t = text.trim_start();
    if !t.starts_with(CLOCK.trim_end()) {
        return None;
    }
    let stamp = timestamp::first_stamp(t)?;
    // `--` immediately after the closing bracket is what closes a clock line.
    if t[stamp.end..].trim_start().starts_with("--") {
        return None;
    }
    Now::from_stamp(&stamp)
}

/// Rewrite a running clock line as a closed one:
/// `CLOCK: [start]--[end] =>  H:MM`.
///
/// Preserves whatever indentation the line already had, so a file written with
/// `org-adapt-indentation` set does not get straightened out from under the
/// user on clock-out.
pub fn close(text: &str, end: Now) -> Option<String> {
    let start = running_start(text)?;
    let indent = &text[..text.len() - text.trim_start().len()];
    let minutes = end.epoch_minutes() - start.epoch_minutes();
    Some(format!(
        "{indent}{CLOCK}{}--{} => {}",
        start.stamp(),
        end.stamp(),
        duration(minutes)
    ))
}

/// Whether removing the running clock line would leave an empty drawer — i.e.
/// the line above it opens `:LOGBOOK:` and the line below closes it.
///
/// A clock-cancel that leaves `:LOGBOOK:` / `:END:` with nothing between them
/// leaves visible litter in the file for an action whose whole point is "pretend
/// this never happened".
pub fn cancel_span(lb: &Logbook, running: Running) -> (u32, u32) {
    let above = running.line.checked_sub(1);
    let opens = above.is_some_and(|n| lb.is_drawer_open(n, LOGBOOK));
    let closes = lb
        .hl
        .text(running.line + 1)
        .is_some_and(|l| l.trim().eq_ignore_ascii_case(":END:"));
    match (opens, closes) {
        (true, true) => (running.line - 1, running.line + 1),
        _ => (running.line, running.line),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use crate::headline;

    /// The same stand-in `archive.rs` uses, and for the same reason: it
    /// reproduces the host's line contract INCLUDING the trailing-newline
    /// remainder that `str::lines` hides.
    fn accessor(text: &'static str) -> (impl Fn(u32) -> Option<String>, u32) {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let count = if text.ends_with('\n') {
            lines.len().saturating_sub(1)
        } else {
            lines.len()
        } as u32;
        (move |n: u32| lines.get(n as usize).cloned(), count)
    }

    /// 2026-08-28 16:02 local. Every test uses this one instant so the rendered
    /// stamps are checkable by eye.
    fn now() -> Now {
        Now {
            year: 2026,
            month: 8,
            day: 28,
            hour: 16,
            minute: 2,
        }
    }

    #[test]
    fn a_local_instant_round_trips_through_its_stamp() {
        let n = now();
        assert_eq!(n.stamp(), "[2026-08-28 Fri 16:02]");
        let parsed = timestamp::first_stamp(&n.stamp()).unwrap();
        assert_eq!(Now::from_stamp(&parsed), Some(n));
    }

    /// The seconds→civil conversion has to FLOOR, not truncate. A negative
    /// local instant is reachable with a west-of-Greenwich offset near the
    /// epoch, and truncation there rolls the day the wrong way and yields a
    /// negative hour.
    #[test]
    fn a_negative_local_instant_floors_rather_than_truncating() {
        // 1969-12-31 23:00 UTC-equivalent: one hour before the epoch.
        let n = Now::from_local_secs(-3_600);
        assert_eq!((n.year, n.month, n.day), (1969, 12, 31));
        assert_eq!((n.hour, n.minute), (23, 0));
    }

    #[test]
    fn durations_align_in_a_column() {
        assert_eq!(duration(90), " 1:30");
        assert_eq!(duration(690), "11:30");
        assert_eq!(duration(0), " 0:00");
    }

    /// A backwards span is not a negative duration — org has no reading for a
    /// minus sign here. Both real stamps still land on the line, so the
    /// wrongness stays where it happened rather than being smoothed over.
    #[test]
    fn a_backwards_span_clamps_to_zero_rather_than_going_negative() {
        assert_eq!(duration(-90), " 0:00");
    }

    #[test]
    fn clocking_in_creates_the_drawer_when_there_is_none() {
        let (line, count) = accessor("* Task\nbody\n");
        let hl = headline::Headlines::new(None, &line, count);
        let lb = Logbook::new(&hl);
        let ins = lb.clock_in(0, now()).unwrap();
        assert_eq!(ins.line, 1, "immediately after the headline");
        assert_eq!(
            ins.text,
            ":LOGBOOK:\nCLOCK: [2026-08-28 Fri 16:02]\n:END:\n"
        );
    }

    #[test]
    fn clocking_in_reuses_an_existing_drawer_newest_first() {
        let (line, count) = accessor(
            "* Task\n:LOGBOOK:\nCLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:00] =>  1:00\n:END:\n",
        );
        let hl = headline::Headlines::new(None, &line, count);
        let ins = Logbook::new(&hl).clock_in(0, now()).unwrap();
        assert_eq!(
            ins.line, 2,
            "the drawer's FIRST line — newest first is org's convention, and it \
             is what makes finding the running clock a single-line look"
        );
        assert_eq!(ins.text, "CLOCK: [2026-08-28 Fri 16:02]\n");
    }

    /// D5: past the planning line, not on top of it.
    #[test]
    fn the_drawer_goes_below_a_planning_line() {
        let (line, count) = accessor("* Task\nSCHEDULED: <2026-08-30 Sun>\nbody\n");
        let hl = headline::Headlines::new(None, &line, count);
        let ins = Logbook::new(&hl).clock_in(0, now()).unwrap();
        assert_eq!(ins.line, 2, "below SCHEDULED:, not above it");
    }

    /// D5: past `:PROPERTIES:` too, and past its `:END:` — the trap being that
    /// a naive "skip to the next `:END:`" is right here and wrong for LOGBOOK.
    #[test]
    fn the_drawer_goes_below_a_properties_drawer() {
        let (line, count) = accessor("* Task\n:PROPERTIES:\n:ID: abc\n:END:\nbody\n");
        let hl = headline::Headlines::new(None, &line, count);
        let ins = Logbook::new(&hl).clock_in(0, now()).unwrap();
        assert_eq!(ins.line, 4, "below the properties drawer's :END:");
        assert!(ins.text.starts_with(":LOGBOOK:"));
    }

    #[test]
    fn both_planning_and_properties_are_skipped_together() {
        let (line, count) =
            accessor("* Task\nSCHEDULED: <2026-08-30 Sun>\n:PROPERTIES:\n:ID: abc\n:END:\nbody\n");
        let hl = headline::Headlines::new(None, &line, count);
        let ins = Logbook::new(&hl).clock_in(0, now()).unwrap();
        assert_eq!(ins.line, 5);
    }

    /// Above the first headline there is no entry to clock into. Refusing is
    /// the answer; inventing a location would write a clock line that belongs
    /// to nothing and can never be clocked out of.
    #[test]
    fn there_is_nothing_to_clock_into_above_the_first_headline() {
        let (line, count) = accessor("#+TITLE: Notes\n\n* Task\n");
        let hl = headline::Headlines::new(None, &line, count);
        assert!(Logbook::new(&hl).clock_in(0, now()).is_none());
    }

    /// D4, the whole point: no session state exists here, so this is exactly
    /// the "after a restart" case — the buffer alone says a clock is running.
    #[test]
    fn a_running_clock_is_found_from_the_buffer_alone() {
        let (line, count) = accessor("* Task\n:LOGBOOK:\nCLOCK: [2026-08-28 Fri 09:15]\n:END:\n");
        let hl = headline::Headlines::new(None, &line, count);
        let r = Logbook::new(&hl).running(0).unwrap();
        assert_eq!(r.line, 2);
        assert_eq!((r.start.hour, r.start.minute), (9, 15));
    }

    #[test]
    fn a_closed_clock_is_not_running() {
        let (line, count) = accessor(
            "* Task\n:LOGBOOK:\nCLOCK: [2026-08-28 Fri 09:15]--[2026-08-28 Fri 10:15] =>  1:00\n:END:\n",
        );
        let hl = headline::Headlines::new(None, &line, count);
        assert!(Logbook::new(&hl).running(0).is_none());
    }

    /// The `:END:` bound is load-bearing: without it the scan would run into
    /// the NEXT entry's drawer and clock out of someone else's task.
    #[test]
    fn the_search_stops_at_the_drawers_end() {
        let (line, count) = accessor(
            "* One\n:LOGBOOK:\n:END:\n* Two\n:LOGBOOK:\nCLOCK: [2026-08-28 Fri 09:15]\n:END:\n",
        );
        let hl = headline::Headlines::new(None, &line, count);
        let lb = Logbook::new(&hl);
        assert!(
            lb.running(0).is_none(),
            "the first entry's drawer is empty; the second entry's clock is not its business"
        );
        assert_eq!(lb.running(3).unwrap().line, 5);
    }

    #[test]
    fn closing_a_line_appends_the_end_and_the_duration() {
        let closed = close(
            "CLOCK: [2026-08-28 Fri 09:15]",
            Now {
                hour: 10,
                minute: 45,
                ..now()
            },
        )
        .unwrap();
        assert_eq!(
            closed,
            "CLOCK: [2026-08-28 Fri 09:15]--[2026-08-28 Fri 10:45] =>  1:30"
        );
    }

    /// A file written with `org-adapt-indentation` keeps its indentation —
    /// clocking out must not silently reformat lines the user did not touch.
    #[test]
    fn closing_preserves_the_lines_indentation() {
        let closed = close("  CLOCK: [2026-08-28 Fri 09:15]", now()).unwrap();
        assert!(closed.starts_with("  CLOCK: "));
    }

    #[test]
    fn closing_an_already_closed_line_is_refused() {
        assert!(close(
            "CLOCK: [2026-08-28 Fri 09:15]--[2026-08-28 Fri 10:15] =>  1:00",
            now()
        )
        .is_none());
    }

    /// Cancel takes the drawer with it when the clock was its only line —
    /// otherwise an action meaning "pretend this never happened" leaves an
    /// empty `:LOGBOOK:` / `:END:` pair behind as litter.
    #[test]
    fn cancelling_the_only_clock_removes_the_drawer_too() {
        let (line, count) =
            accessor("* Task\n:LOGBOOK:\nCLOCK: [2026-08-28 Fri 09:15]\n:END:\nbody\n");
        let hl = headline::Headlines::new(None, &line, count);
        let lb = Logbook::new(&hl);
        let r = lb.running(0).unwrap();
        assert_eq!(cancel_span(&lb, r), (1, 3), "the whole drawer");
    }

    #[test]
    fn cancelling_one_of_several_clocks_removes_only_that_line() {
        let (line, count) = accessor(
            "* Task\n:LOGBOOK:\nCLOCK: [2026-08-28 Fri 09:15]\nCLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:00] =>  1:00\n:END:\n",
        );
        let hl = headline::Headlines::new(None, &line, count);
        let lb = Logbook::new(&hl);
        let r = lb.running(0).unwrap();
        assert_eq!(cancel_span(&lb, r), (2, 2), "just the running line");
    }
}

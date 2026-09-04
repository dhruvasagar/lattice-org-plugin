//! OA.15 — log mode: what you DID, beside what you plan to do.
//!
//! Design: `docs/dev/architecture/org-agenda.md` §3a. Slice plan:
//! `docs/dev/operations/slice-plans/org-agenda.md` phase 5 (both in the
//! lattice repo).
//!
//! Emacs binds this to `l` and calls it `org-agenda-log-mode`. It admits the
//! three things a file records about its own past — a headline being closed,
//! time being clocked, a TODO state changing — into the agenda, filed under
//! the day each happened.
//!
//! ## Why this is not a virtual row
//!
//! The design fragment's §3 table originally filed log entries beside the
//! clock report, as display-only virtual rows. That was wrong for this
//! feature, and the difference is not a matter of taste: **a clock report is a
//! computed aggregate with no source range, and a log entry is a line in a
//! file.** The whole use of seeing "you closed Ship the thing at 14:32" is
//! being able to press `<CR>` and go there, or `<leader>ot` and reopen it.
//! Virtual rows can do neither.
//!
//! So a log row is an ordinary `entry` over the headline it happened to, and
//! everything the agenda already does to a row — jump, act, colour, fold —
//! works on it with nothing added. The cost is that log mode re-scans, which
//! is the decision phase 6 already locked for filters and spans.
//!
//! ## The row is the HEADLINE, and the detail hangs under it
//!
//! An excerpt is verbatim source, so a log row cannot render emacs' composed
//! `Closed:    TODO Ship the thing` line — there is no such line in the file.
//! The row is therefore the headline itself (which is what you want to read
//! and what you want to jump to), and *what happened* rides HB.5's
//! `annotation`: one line under the row saying `Closed 14:32`,
//! `Clocked 2:15`, `State TODO → WAITING 09:04`.
//!
//! That is a better division than it looks. The headline is the identity of
//! the thing; the log detail is the event. Putting the event in the row would
//! have meant excerpting the `CLOCK:` line or the LOGBOOK line, which reads as
//! drawer noise and jumps you into the middle of a drawer.
//!
//! ## The window looks BACKWARD
//!
//! The agenda's span is forward-anchored — `Days(n)` is `today ..= today + n`,
//! and walking it with `f` moves the whole window. A log has nothing to say
//! about the future, so a log row filed into that window would only ever be
//! today's.
//!
//! Log mode therefore covers `today - span ..= today`: the daily agenda logs
//! today, the week view logs the last seven days, and `b` walks back through
//! earlier weeks exactly as it walks the plan. The plan looks forward and the
//! record looks back, over the same length of time — which is what "this
//! week" means in both directions.
//!
//! ## Clock lines are aggregated per (headline, day)
//!
//! Emacs emits one log row per `CLOCK:` line. This emits one per headline per
//! day, for [`clock_scan::Span`](crate::clock_scan::Span)'s stated reason: it
//! is the granularity anything actually renders, and a task picked up five
//! times in a morning is one thing you did, not five. The annotation totals
//! the day and the row orders by the FIRST clock-in, so the position in the
//! day is still the position the work started.

use crate::clock_scan;
use crate::headline;
use crate::history;
use crate::timestamp;

/// Which kinds of log entry a view admits.
///
/// Emacs' `org-agenda-log-mode-items`, whose default is `(closed clock)` —
/// state changes are the noisiest of the three (a task walked through four
/// keywords logs four lines) and are the one you opt into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogItems {
    pub closed: bool,
    pub clock: bool,
    pub state: bool,
}

impl LogItems {
    /// All three — emacs' `C-u l`.
    pub const ALL: Self = Self {
        closed: true,
        clock: true,
        state: true,
    };

    /// Emacs' default: what you finished and what you spent time on.
    pub const DEFAULT: Self = Self {
        closed: true,
        clock: true,
        state: false,
    };

    pub fn any(&self) -> bool {
        self.closed || self.clock || self.state
    }

    /// Parse a `closed,clock,state` list — the `log=` argument's value and the
    /// `org.agenda-log-mode-items` option's, which are deliberately the same
    /// spelling so a user reading one recognises the other.
    ///
    /// Separated by commas or whitespace: the view argument cannot contain a
    /// space (it is one element of `scan_args`) and an option written in
    /// emacs' habit reads `closed clock`. Accepting both costs one `matches!`
    /// and removes a way to be wrong.
    ///
    /// Returns the items alongside the words it did not recognise. An unknown
    /// word costs itself and nothing else, for `ViewArgs::parse`'s reason: a
    /// view that refuses to open because one word was misspelled answers
    /// nothing.
    pub fn parse(spec: &str) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut problems = Vec::new();
        for word in spec.split(|c: char| c == ',' || c.is_whitespace()) {
            let word = word.trim();
            if word.is_empty() {
                continue;
            }
            match word.to_ascii_lowercase().as_str() {
                "closed" => out.closed = true,
                // `clock` is emacs' spelling of the item; `clocked` is what a
                // hand writes half the time, and refusing it would cost a log
                // row for a reason nobody could see.
                "clock" | "clocked" => out.clock = true,
                "state" => out.state = true,
                "all" => out = Self::ALL,
                _ => problems.push(format!("`{word}` is not a log item")),
            }
        }
        (out, problems)
    }

    /// The `log=` argument's value, in `parse`'s own spelling so the two are
    /// inverses. Order is fixed rather than insertion-ordered — the set is
    /// what carries meaning, and a stable rendering keeps `to_args` a pure
    /// function of the view.
    pub fn to_spec(&self) -> String {
        let mut parts = Vec::new();
        if self.closed {
            parts.push("closed");
        }
        if self.clock {
            parts.push("clock");
        }
        if self.state {
            parts.push("state");
        }
        parts.join(",")
    }
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogKind {
    /// A `CLOSED: [stamp]` on the headline's planning line.
    Closed,
    /// Time clocked, summed across the day's `CLOCK:` lines for this headline.
    Clocked { minutes: u32 },
    /// A `- State "NEW" from "OLD" [stamp]` line.
    State { from: Option<String>, to: String },
}

impl LogKind {
    /// Within-day order for events at the same minute: what you finished last,
    /// after the time you spent and the states you moved through. Closed is
    /// the terminal event of a task's day and reads best as the last word on
    /// it.
    fn rank(&self) -> i64 {
        match self {
            LogKind::Clocked { .. } => 0,
            LogKind::State { .. } => 1,
            LogKind::Closed => 2,
        }
    }
}

/// One thing that happened to one headline on one day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEvent {
    /// 0-based line of the HEADLINE the event belongs to — not of the
    /// `CLOCK:` or LOGBOOK line, which is drawer interior and not somewhere to
    /// send a cursor.
    pub line: u32,
    /// Days since the Unix epoch.
    pub day: i64,
    /// Minutes past midnight, for ordering and for the annotation's clock
    /// face. `None` for a date-only stamp, which sorts to the head of its day
    /// rather than being guessed at.
    pub minute_of_day: Option<u32>,
    pub kind: LogKind,
}

impl LogEvent {
    /// `Closed 14:32` / `Clocked 2:15` / `State TODO → WAITING 09:04`, and the
    /// byte spans that colour it.
    ///
    /// Spans name THEME ELEMENTS, never tree-sitter captures: an annotation's
    /// cells carry a baked colour resolved when the row is built, so a capture
    /// name would have nothing to resolve against (`scanned-excerpt-source.wit`
    /// says so on `annotation.spans`).
    ///
    /// A state change paints each keyword in its own `org.todo.<KEYWORD>`
    /// element, so `WAITING` in a log row is the yellow it is everywhere else
    /// and a user's `todo-keyword-styles` override reaches it for free. A
    /// keyword the config does not declare has no element and renders in the
    /// row's own foreground, which is the seam's documented answer for an
    /// unknown slot.
    pub fn annotation(&self) -> (String, Vec<(u32, u32, String)>) {
        let mut text = String::new();
        let mut spans: Vec<(u32, u32, String)> = Vec::new();
        let mut push = |text: &mut String, word: &str, slot: &str| {
            let start = text.len() as u32;
            text.push_str(word);
            spans.push((start, text.len() as u32, slot.to_string()));
        };
        match &self.kind {
            LogKind::Closed => push(&mut text, "Closed", "org.log.closed"),
            LogKind::Clocked { minutes } => {
                push(&mut text, "Clocked", "org.log.clocked");
                text.push(' ');
                text.push_str(&format_minutes(*minutes));
            }
            LogKind::State { from, to } => {
                push(&mut text, "State", "org.log.state");
                text.push(' ');
                if let Some(from) = from {
                    push(&mut text, from, &format!("org.todo.{from}"));
                    // U+2192, the arrow every state change in this repo's
                    // prose already uses. Not `->`: the row is one line of
                    // display text, not something that round-trips to a file.
                    text.push_str(" \u{2192} ");
                }
                push(&mut text, to, &format!("org.todo.{to}"));
            }
        }
        if let Some(minute) = self.minute_of_day {
            text.push_str(&format!(" {:02}:{:02}", minute / 60, minute % 60));
        }
        (text, spans)
    }

    /// Within-day order: by the time it happened, then by kind, then by the
    /// line it sits on so two events at one minute never swap between scans.
    ///
    /// An untimed event sorts FIRST in its day. It is the honest place for
    /// "sometime on the 3rd" — putting it last would claim it happened after
    /// everything timed, which is a fact the file does not carry.
    ///
    /// Packed to stay under 10 000, which is the width of the within-day slot
    /// [`agenda::log_sort_key`](crate::agenda::log_sort_key) has to fit it
    /// into: `(1439 + 1) * 4 + 2 = 5762`. A wider encoding would carry into
    /// the DAY digits and file an evening event on the following morning.
    pub fn within_day(&self) -> i64 {
        let minute = self.minute_of_day.map(i64::from).unwrap_or(-1);
        (minute + 1) * 4 + self.kind.rank()
    }
}

/// `H:MM`, org's own clocktable spelling — the same one
/// `lattice-multibuffer`'s clock report writes, so a task reporting `1:30` in
/// the report does not report `1.5h` two lines above it.
fn format_minutes(minutes: u32) -> String {
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

/// Every log event in `text` that `items` admits and `days` contains.
///
/// Line-based rather than tree-based, for [`clock_scan::scan`]'s reason: what
/// this needs is which headline a drawer line sits under, which is exactly
/// what a stars walk gives, and it works on a host with no org grammar loaded.
/// The tree path has nothing to offer here anyway — a LOGBOOK line is
/// undifferentiated drawer content to the grammar.
///
/// **Filtered by day inside the walk**, not by the caller. A corpus with years
/// of history in it is mostly log lines, and carrying every one of them across
/// the seam to drop all but a week would put the whole history on a producer's
/// critical path for a view that shows seven days.
pub fn scan(text: &str, items: LogItems, days: &std::ops::RangeInclusive<i64>) -> Vec<LogEvent> {
    let mut out: Vec<LogEvent> = Vec::new();
    if !items.any() {
        return out;
    }
    // The headline the walk is currently inside, and whether it has already
    // logged a `CLOSED:`. Org writes one per headline; a second would be a
    // hand-edit, and reporting both would double a task in the log.
    let mut current: Option<u32> = None;
    let mut closed_seen = false;

    for (i, raw) in text.lines().enumerate() {
        if headline::headline_level(raw).is_some() {
            current = Some(i as u32);
            closed_seen = false;
            continue;
        }
        let Some(line) = current else {
            // A drawer line before any headline belongs to nothing that can be
            // jumped to, so it is skipped rather than filed under an invented
            // root — `clock_scan`'s rule, for the same reason.
            continue;
        };

        if items.closed && !closed_seen {
            if let Some(stamp) = closed_stamp(raw) {
                let day = timestamp::epoch_day(stamp.year, stamp.month, stamp.day);
                closed_seen = true;
                if days.contains(&day) {
                    out.push(LogEvent {
                        line,
                        day,
                        minute_of_day: minute_of_day(&stamp),
                        kind: LogKind::Closed,
                    });
                }
                continue;
            }
        }
        if items.clock {
            if let Some(clock) = clock_scan::closed_span(raw) {
                if clock.minutes > 0 && days.contains(&clock.day) {
                    let minute = clock.started.map(|(h, m)| h * 60 + m);
                    // Aggregated per (headline, day): one thing you did, not
                    // five. The earliest clock-in wins the ordering, so the
                    // row sits where the work started.
                    match out.iter_mut().find(|e| {
                        e.line == line
                            && e.day == clock.day
                            && matches!(e.kind, LogKind::Clocked { .. })
                    }) {
                        Some(LogEvent {
                            kind: LogKind::Clocked { minutes },
                            minute_of_day,
                            ..
                        }) => {
                            *minutes = minutes.saturating_add(clock.minutes);
                            *minute_of_day = match (*minute_of_day, minute) {
                                (Some(a), Some(b)) => Some(a.min(b)),
                                (a, b) => a.or(b),
                            };
                        }
                        _ => out.push(LogEvent {
                            line,
                            day: clock.day,
                            minute_of_day: minute,
                            kind: LogKind::Clocked {
                                minutes: clock.minutes,
                            },
                        }),
                    }
                }
                continue;
            }
        }
        if items.state {
            if let Some(change) = history::state_change(raw) {
                let day =
                    timestamp::epoch_day(change.stamp.year, change.stamp.month, change.stamp.day);
                if days.contains(&day) {
                    out.push(LogEvent {
                        line,
                        day,
                        minute_of_day: minute_of_day(&change.stamp),
                        kind: LogKind::State {
                            from: change.from,
                            to: change.to,
                        },
                    });
                }
            }
        }
    }
    out
}

fn minute_of_day(stamp: &timestamp::Stamp) -> Option<u32> {
    stamp.time.map(|(h, m)| h * 60 + m)
}

/// The `CLOSED: [2026-08-20 Thu 14:32]` stamp on a planning line.
///
/// Found by searching the line rather than by requiring `CLOSED:` at its
/// start: org writes the planning keywords on one line in whatever order the
/// task acquired them, so `SCHEDULED: <…> CLOSED: [<…>]` is ordinary. The
/// stamp is read from AFTER the keyword for exactly that reason — taking the
/// line's first stamp would file the closure under the scheduled date.
///
/// The stamp must be INACTIVE, which is what `CLOSED:` writes and what the
/// agenda's own scan relies on to keep closures out of the plan.
fn closed_stamp(line: &str) -> Option<timestamp::Stamp> {
    let at = line.find("CLOSED:")?;
    let rest = &line[at + "CLOSED:".len()..];
    let stamp = timestamp::first_stamp(rest)?;
    (!stamp.active).then_some(stamp)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> i64 {
        timestamp::epoch_day(y, m, d)
    }

    /// A window wide enough that nothing is dropped for being out of range —
    /// the range is tested on its own below.
    fn always() -> std::ops::RangeInclusive<i64> {
        0..=i64::MAX
    }

    fn scan_all(text: &str) -> Vec<LogEvent> {
        scan(text, LogItems::ALL, &always())
    }

    // ── what counts as a log entry ──────────────────────────────────────

    #[test]
    fn a_closed_headline_logs_the_day_it_closed() {
        let events = scan_all("* DONE Ship it\n  CLOSED: [2026-08-20 Thu 14:32]\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].line, 0, "the row anchors on the HEADLINE");
        assert_eq!(events[0].day, day(2026, 8, 20));
        assert_eq!(events[0].minute_of_day, Some(14 * 60 + 32));
        assert_eq!(events[0].kind, LogKind::Closed);
    }

    /// The whole point of the mode. These are the two things the agenda's own
    /// scan refuses by construction — a DONE headline is not a row and an
    /// inactive stamp never dates one — so if log mode did not see them,
    /// nothing would.
    #[test]
    fn log_mode_sees_exactly_what_the_agenda_scan_refuses() {
        let text = "* DONE Ship it\n  CLOSED: [2026-08-20 Thu 14:32]\n";
        assert!(
            !scan_all(text).is_empty(),
            "the log sees a closed DONE headline"
        );
        assert!(
            crate::agenda::scan_file(
                text,
                &crate::agenda::Keywords::from_spec("TODO | DONE"),
                false
            )
            .is_empty(),
            "…which the agenda scan itself does not, and must not"
        );
    }

    /// `CLOSED:` shares its line with `SCHEDULED:` on any task that had a plan
    /// before it was finished, which is most of them. Reading the line's first
    /// stamp would file every such closure under the date it was scheduled
    /// for — plausible, and wrong by however long the task was late.
    #[test]
    fn a_closed_stamp_is_read_past_the_keyword_not_from_the_line_start() {
        let events =
            scan_all("* DONE Ship it\n  SCHEDULED: <2026-08-10 Mon> CLOSED: [2026-08-20 Thu]\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].day,
            day(2026, 8, 20),
            "the day it CLOSED, not the day it was scheduled"
        );
        assert_eq!(
            events[0].minute_of_day, None,
            "a date-only stamp claims no time"
        );
    }

    #[test]
    fn a_clock_line_logs_the_time_it_recorded() {
        let events = scan_all(
            "* Task\n:LOGBOOK:\n\
             CLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:30] =>  1:30\n:END:\n",
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, LogKind::Clocked { minutes: 90 });
        assert_eq!(events[0].minute_of_day, Some(9 * 60));
    }

    /// One thing you did, not five. The annotation totals the day and the row
    /// sits where the work STARTED, so a task picked up again after lunch does
    /// not jump down the block.
    #[test]
    fn clock_lines_aggregate_per_headline_per_day() {
        let events = scan_all(
            "* Task\n:LOGBOOK:\n\
             CLOCK: [2026-08-27 Thu 14:00]--[2026-08-27 Thu 15:00] =>  1:00\n\
             CLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 09:30] =>  0:30\n:END:\n",
        );
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].kind, LogKind::Clocked { minutes: 90 });
        assert_eq!(
            events[0].minute_of_day,
            Some(9 * 60),
            "the EARLIEST clock-in orders the row"
        );
    }

    /// …but two days are two events, or a week view would report one blob.
    #[test]
    fn clock_lines_on_different_days_stay_apart() {
        let events = scan_all(
            "* Task\n:LOGBOOK:\n\
             CLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:00] =>  1:00\n\
             CLOCK: [2026-08-28 Fri 09:00]--[2026-08-28 Fri 10:00] =>  1:00\n:END:\n",
        );
        assert_eq!(events.len(), 2);
        assert_ne!(events[0].day, events[1].day);
    }

    /// Two sibling headlines with the same text are two tasks. Keyed by LINE
    /// for `clock_scan`'s reason: a path is not an identity when siblings
    /// share a name.
    #[test]
    fn two_headlines_clocked_on_one_day_stay_apart() {
        let events = scan_all(
            "* Task\n:LOGBOOK:\nCLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:00] => 1:00\n:END:\n\
             * Task\n:LOGBOOK:\nCLOCK: [2026-08-27 Thu 11:00]--[2026-08-27 Thu 12:00] => 1:00\n:END:\n",
        );
        assert_eq!(events.len(), 2, "{events:?}");
        assert_ne!(events[0].line, events[1].line);
    }

    /// A running clock has no duration yet. `clock_scan` refuses it and this
    /// inherits the refusal by reusing that parser — which is the reason the
    /// parser is shared rather than copied.
    #[test]
    fn a_running_clock_logs_nothing() {
        assert!(scan_all("* Task\n:LOGBOOK:\nCLOCK: [2026-08-28 Fri 16:02]\n:END:\n").is_empty());
    }

    #[test]
    fn a_state_change_logs_both_states() {
        let events = scan_all(
            "* WAITING Ship it\n:LOGBOOK:\n\
             - State \"WAITING\"    from \"TODO\"       [2026-09-01 Tue 09:04]\n:END:\n",
        );
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].kind,
            LogKind::State {
                from: Some("TODO".to_string()),
                to: "WAITING".to_string(),
            },
            "org pads the fields to 12 columns; the parser is `history`'s"
        );
    }

    /// Org omits `from` for a task that had no previous keyword, and the row
    /// still means something.
    #[test]
    fn a_state_change_out_of_nothing_still_logs() {
        let events =
            scan_all("* TODO Ship it\n:LOGBOOK:\n- State \"TODO\" [2026-09-01 Tue 09:04]\n:END:\n");
        assert_eq!(
            events[0].kind,
            LogKind::State {
                from: None,
                to: "TODO".to_string(),
            }
        );
    }

    // ── what it refuses ─────────────────────────────────────────────────

    /// Each item is opt-in, which is what makes `org.agenda-log-mode-items`
    /// mean anything. Emacs' default set is the first two.
    #[test]
    fn each_item_kind_is_admitted_only_when_asked_for() {
        let text = "* DONE Ship it\n  CLOSED: [2026-09-01 Tue 10:00]\n:LOGBOOK:\n\
             CLOCK: [2026-09-01 Tue 09:00]--[2026-09-01 Tue 10:00] => 1:00\n\
             - State \"DONE\" from \"TODO\" [2026-09-01 Tue 10:00]\n:END:\n";
        let kinds = |items| {
            scan(text, items, &always())
                .into_iter()
                .map(|e| e.kind)
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds(LogItems::ALL).len(), 3, "all three when asked");
        assert_eq!(
            kinds(LogItems::DEFAULT).len(),
            2,
            "emacs' default is closed + clock"
        );
        assert_eq!(
            kinds(LogItems {
                state: true,
                ..Default::default()
            }),
            vec![LogKind::State {
                from: Some("TODO".to_string()),
                to: "DONE".to_string()
            }]
        );
        assert!(
            kinds(LogItems::default()).is_empty(),
            "no items is no log, and costs no walk"
        );
    }

    /// The range is applied INSIDE the walk. A corpus with years of history is
    /// mostly log lines, and carrying them all back to drop all but a week
    /// would put the whole history on the producer's critical path.
    #[test]
    fn events_outside_the_window_never_leave_the_walk() {
        let text = "* DONE A\n  CLOSED: [2026-08-20 Thu]\n* DONE B\n  CLOSED: [2026-09-01 Tue]\n";
        let events = scan(text, LogItems::ALL, &(day(2026, 8, 25)..=day(2026, 9, 3)));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].day, day(2026, 9, 1));
    }

    /// An ACTIVE stamp is a plan, not a record. `CLOSED: <...>` is not
    /// something org writes, and reading it would put a future day in the log.
    #[test]
    fn an_active_closed_stamp_is_not_a_closure() {
        assert!(scan_all("* DONE Ship it\n  CLOSED: <2026-08-20 Thu>\n").is_empty());
    }

    /// Org writes one `CLOSED:` per headline. A second is a hand-edit, and
    /// counting both would report one task closing twice.
    #[test]
    fn only_the_first_closed_line_of_a_headline_counts() {
        let events =
            scan_all("* DONE Ship it\n  CLOSED: [2026-08-20 Thu]\n  CLOSED: [2026-08-21 Fri]\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].day, day(2026, 8, 20));
    }

    /// …and the "only the first" state resets at the next headline, or one
    /// closure per FILE is what the walk would report.
    #[test]
    fn the_closed_guard_resets_at_the_next_headline() {
        let events = scan_all(
            "* DONE A\n  CLOSED: [2026-08-20 Thu]\n* DONE B\n  CLOSED: [2026-08-21 Fri]\n",
        );
        assert_eq!(events.len(), 2, "{events:?}");
    }

    #[test]
    fn a_drawer_line_before_any_headline_is_skipped() {
        assert!(
            scan_all("CLOCK: [2026-08-27 Thu 09:00]--[2026-08-27 Thu 10:00] => 1:00\n").is_empty()
        );
    }

    // ── ordering and rendering ──────────────────────────────────────────

    /// Within a day the log reads in the order the day happened.
    #[test]
    fn events_order_by_the_time_they_happened() {
        let a = LogEvent {
            line: 0,
            day: 0,
            minute_of_day: Some(9 * 60),
            kind: LogKind::Closed,
        };
        let b = LogEvent {
            minute_of_day: Some(14 * 60),
            ..a.clone()
        };
        assert!(a.within_day() < b.within_day());
    }

    /// An untimed event sorts to the head of its day. Putting it last would
    /// claim it happened after everything timed, which the file does not say.
    #[test]
    fn an_untimed_event_leads_its_day() {
        let untimed = LogEvent {
            line: 0,
            day: 0,
            minute_of_day: None,
            kind: LogKind::Closed,
        };
        let midnight = LogEvent {
            minute_of_day: Some(0),
            ..untimed.clone()
        };
        assert!(untimed.within_day() < midnight.within_day());
    }

    #[test]
    fn the_annotation_says_what_happened_and_when() {
        let closed = LogEvent {
            line: 0,
            day: 0,
            minute_of_day: Some(14 * 60 + 32),
            kind: LogKind::Closed,
        };
        assert_eq!(closed.annotation().0, "Closed 14:32");

        let clocked = LogEvent {
            kind: LogKind::Clocked { minutes: 135 },
            ..closed.clone()
        };
        assert_eq!(
            clocked.annotation().0,
            "Clocked 2:15 14:32",
            "org's own H:MM, so the log agrees with the clock report above it"
        );

        let state = LogEvent {
            kind: LogKind::State {
                from: Some("TODO".into()),
                to: "WAITING".into(),
            },
            ..closed
        };
        assert_eq!(state.annotation().0, "State TODO \u{2192} WAITING 14:32");
    }

    /// Spans must name THEME ELEMENTS — an annotation's cells carry a baked
    /// colour, so a tree-sitter capture name would have nothing to resolve
    /// against.
    #[test]
    fn annotation_spans_name_registered_elements() {
        let event = LogEvent {
            line: 0,
            day: 0,
            minute_of_day: None,
            kind: LogKind::State {
                from: Some("TODO".into()),
                to: "WAITING".into(),
            },
        };
        let (text, spans) = event.annotation();
        for (start, end, slot) in &spans {
            assert!(
                slot.starts_with("org."),
                "a span must name a namespaced element, got {slot}"
            );
            assert!(
                (*start as usize) < text.len() && *end as usize <= text.len() && start < end,
                "span {start}..{end} must index {text:?}"
            );
        }
        // Each keyword paints in its own element, so a `todo-keyword-styles`
        // override reaches a log row for free.
        assert!(spans.iter().any(|(_, _, s)| s == "org.todo.TODO"));
        assert!(spans.iter().any(|(_, _, s)| s == "org.todo.WAITING"));
    }

    // ── the item spec, both homes ───────────────────────────────────────

    #[test]
    fn items_parse_from_commas_and_from_spaces() {
        assert_eq!(LogItems::parse("closed,clock").0, LogItems::DEFAULT);
        assert_eq!(
            LogItems::parse("closed clock").0,
            LogItems::DEFAULT,
            "the option is written in emacs' habit"
        );
        assert_eq!(LogItems::parse("all").0, LogItems::ALL);
        assert_eq!(LogItems::parse("CLOSED").0.closed, true, "case-insensitive");
    }

    #[test]
    fn an_unknown_item_is_named_and_costs_only_itself() {
        let (items, problems) = LogItems::parse("closed,sideways");
        assert!(items.closed, "the rest of the spec still applies");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("sideways"), "{problems:?}");
    }

    /// `to_spec` and `parse` are inverses, or the `l` toggle would lose an
    /// item every time another chord re-opened the view.
    #[test]
    fn the_item_spec_round_trips() {
        for items in [
            LogItems::ALL,
            LogItems::DEFAULT,
            LogItems {
                state: true,
                ..Default::default()
            },
        ] {
            assert_eq!(LogItems::parse(&items.to_spec()).0, items);
        }
    }
}

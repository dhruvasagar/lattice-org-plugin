//! HB.4 — the consistency graph: one cell per day, and what colour it is.
//!
//! Design: `docs/dev/architecture/org-habits.md` §4 (lattice repo).
//!
//! This is a **port of `org-habit-get-faces` and `org-habit-build-graph`**,
//! read from `org-habit.el` rather than reconstructed from how the graph
//! looks. That distinction earned its place: the rule is not the one you
//! would guess from the colours. `alert` (yellow) is not "today" and not "a
//! band approaching the deadline" — it is **the deadline day itself**, for
//! every column in the window, past or future. Red is only ever *past* the
//! deadline day.
//!
//! ## The rule
//!
//! For a day `d`, with `due` the day the task becomes ready and
//! `late = due + (MAX - MIN)` the deadline day:
//!
//! | condition | state |
//! |---|---|
//! | `d < due` | clear |
//! | `d < late` | ready |
//! | `d == late` | alert — unless completed that day, which is ready |
//! | `d > late` | overdue |
//!
//! A repeater with no `/MAX` gives `late == due`, so such a habit is never
//! ready: it goes clear → alert → overdue. That is org's behaviour and
//! design §2 states it.
//!
//! ## `due` moves as you look back
//!
//! A past column has to be coloured by what was true *then*, not by today's
//! schedule — otherwise every day before the last completion reads as
//! overdue. So `due` is recomputed per column from the completion that most
//! recently preceded it, and how depends on the repeater's base, because that
//! is what the bases MEAN (`.+` counts from when you did it, `+` from the
//! stamp). All three of org's branches are ported in [`due_on`].
//!
//! ## Two axes, not one
//!
//! Org draws each cell in one of EIGHT faces: four states × solid/muted. A
//! future column is muted, and so is a past column that is neither overdue
//! nor completed — which is what stops three weeks of ordinary kept days from
//! shouting as loudly as a miss. [`Day::muted`] carries it.

use crate::org_date::Date;
use crate::repeat::{self, Base, Repeater};

/// Org's `org-habit-preceding-days` / `org-habit-following-days`.
pub const PRECEDING_DAYS: i64 = 21;
pub const FOLLOWING_DAYS: i64 = 7;

/// What a day's colour says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayState {
    /// Before the task is due again.
    Clear,
    /// Due, and not yet late.
    Ready,
    /// The deadline day.
    Alert,
    /// Past the deadline day.
    Overdue,
}

impl DayState {
    /// The theme element this day resolves its colour through, as the plugin
    /// REGISTERS it.
    ///
    /// The host auto-namespaces a registration by manifest id, so this becomes
    /// `org.habit.clear` in the registry. Anything that *references* the
    /// element — a span slot crossing the seam — must therefore name the
    /// namespaced form: see [`theme_slot`](Self::theme_slot). Registering with
    /// one spelling and referencing with the other resolves to nothing, and
    /// the symptom is a graph drawn entirely in the default foreground, which
    /// is exactly the colour the design says is the whole point.
    pub fn element(self, muted: bool) -> &'static str {
        match (self, muted) {
            (DayState::Clear, false) => "habit.clear",
            (DayState::Clear, true) => "habit.clear.muted",
            (DayState::Ready, false) => "habit.ready",
            (DayState::Ready, true) => "habit.ready.muted",
            (DayState::Alert, false) => "habit.alert",
            (DayState::Alert, true) => "habit.alert.muted",
            (DayState::Overdue, false) => "habit.overdue",
            (DayState::Overdue, true) => "habit.overdue.muted",
        }
    }

    /// The element as a CONSUMER must name it: the registered name with the
    /// plugin's namespace, which the host adds at registration and does not add
    /// at lookup.
    ///
    /// Derived from [`element`](Self::element) rather than written out again,
    /// so the two cannot drift — the failure mode is silent (an unresolvable
    /// slot renders in the default foreground rather than erroring), and it is
    /// the failure this feature shipped with until it was caught.
    pub fn theme_slot(self, muted: bool) -> String {
        format!("org.{}", self.element(muted))
    }
}

/// One column of the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Day {
    pub date: Date,
    pub state: DayState,
    /// The task was completed on this day.
    pub done: bool,
    pub today: bool,
    /// Draw in the pale variant of `state`'s colour: a future column, or a
    /// past one that is neither overdue nor completed.
    pub muted: bool,
}

/// Everything the graph needs about the habit itself.
pub struct Habit<'a> {
    /// Ascending, unique — [`crate::history::completions`]'s output.
    pub completions: &'a [Date],
    /// The habit's current `SCHEDULED` date.
    pub scheduled: Date,
    pub repeater: Repeater,
}

/// The graph for `habit`, one [`Day`] per column, oldest first.
pub fn build(habit: &Habit, today: Date) -> Vec<Day> {
    let (min, max) = habit.repeater.window_days();
    let slack = i64::from(max.saturating_sub(min));
    let now = repeat::to_days(today);
    let sched = repeat::to_days(habit.scheduled);
    let dones: Vec<i64> = habit
        .completions
        .iter()
        .map(|d| repeat::to_days(*d))
        .collect();
    let first_done = dones.first().copied();

    (now - PRECEDING_DAYS..=now + FOLLOWING_DAYS)
        .map(|d| {
            let past = d < now;
            let is_today = d == now;
            let done = dones.contains(&d);
            // The completion most recently BEFORE this column — org consumes
            // a day's completions only after colouring it, so a day is never
            // coloured by its own completion.
            let last_done = dones.iter().copied().rfind(|c| *c < d);
            let remaining = dones.iter().filter(|c| **c >= d).count();

            let state = if past && last_done.is_none() && sched >= now {
                // Before any completion, on a habit whose schedule has not
                // yet passed: there is nothing to be late for. Org makes the
                // one exception below — the very first completion is `ready`
                // rather than `clear`, so the day a habit began is not drawn
                // as a day it was not due.
                if first_done == Some(d) {
                    DayState::Ready
                } else {
                    DayState::Clear
                }
            } else {
                let due = match (past, last_done) {
                    (true, Some(last)) => {
                        due_on(&dones, last, remaining, sched, min, habit.repeater.base)
                    }
                    // Today and the future are coloured by the schedule as it
                    // stands now, which is what the stamp on the line says.
                    _ => sched,
                };
                state_at(d, due, slack, done)
            };

            Day {
                date: repeat::from_days(d),
                state,
                done,
                today: is_today,
                // Solid for today, for a miss, and for a day you did it.
                // Muted otherwise — see the module doc.
                muted: !is_today && (!past || !(done || state == DayState::Overdue)),
            }
        })
        .collect()
}

/// Org's `org-habit-get-faces` cond, given the day the task became due.
fn state_at(day: i64, due: i64, slack: i64, done: bool) -> DayState {
    let late = due + slack;
    if day < due {
        DayState::Clear
    } else if day < late {
        DayState::Ready
    } else if day == late {
        // Doing it ON the deadline day is doing it in time, so the cell is
        // green rather than yellow. Org spells this out; without it the day
        // you actually kept the habit looks like the day you nearly missed.
        if done {
            DayState::Ready
        } else {
            DayState::Alert
        }
    } else {
        DayState::Overdue
    }
}

/// What the task's due day was, at a past column whose most recent preceding
/// completion is `last`.
///
/// The three branches are org's, and they follow from what the repeater bases
/// mean rather than being special cases:
///
/// - **no completions left** — `last` is the final one, so the stamp on the
///   line today IS the schedule that followed it. Use it, whatever the base.
/// - **`.+`** — counts from when you did it, so due is `last + MIN`.
/// - **`+`** — counts from the stamp, and every completion since `last`
///   shifted that stamp forward one period. Wind them back off today's.
/// - **`++`** — catches up, so a period is not a fixed number of hops. Replay
///   from the first completion, advancing by as many periods as each one
///   needed.
fn due_on(dones: &[i64], last: i64, remaining: usize, scheduled: i64, min: u32, base: Base) -> i64 {
    let step = i64::from(min).max(1);
    if remaining == 0 {
        return scheduled;
    }
    match base {
        Base::Completion => last + step,
        Base::Stamp => scheduled - (remaining as i64) * step,
        Base::StampCatchUp => {
            let Some(first) = dones.first().copied() else {
                return scheduled;
            };
            let shift = (scheduled - first).rem_euclid(step);
            let mut s = first + if shift == 0 { step } else { shift };
            if first == last {
                return s;
            }
            for done in &dones[1..] {
                s += (1 + (*done - s).max(0) / step) * step;
                if *done == last {
                    break;
                }
            }
            s
        }
    }
}

/// The glyph in a cell, in whichever palette the user has.
///
/// Both palettes occupy ONE cell so the graph's column geometry does not
/// shift when `ui.nerd_fonts` is toggled — the icon-degradation rule. The
/// fallback is org's own `*` / `!` widened to BMP shapes that read as marks
/// rather than punctuation; the plain cell is a middle dot rather than a
/// space so the bar is still visible on a monochrome terminal, where the
/// colour that carries most of the meaning is gone.
pub fn glyph(day: &Day, nerd_fonts: bool) -> char {
    match (day.done, day.today, nerd_fonts) {
        (true, _, true) => '\u{f00c}', // nf-fa-check
        (true, _, false) => '✓',
        (false, true, true) => '\u{f069}', // nf-fa-asterisk
        (false, true, false) => '✳',
        (false, false, _) => '·',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> Date {
        Date {
            year: y,
            month: m,
            day: d,
        }
    }

    fn rep(s: &str) -> Repeater {
        repeat::parse(s).expect("a repeater")
    }

    /// Build and render as a string, one char per state, so a whole window
    /// reads at a glance: `.` clear, `r` ready, `!` alert, `X` overdue,
    /// uppercase where the cell is solid rather than muted.
    fn render(habit: &Habit, today: Date) -> String {
        build(habit, today)
            .iter()
            .map(|d| {
                let c = match d.state {
                    DayState::Clear => 'c',
                    DayState::Ready => 'r',
                    DayState::Alert => 'a',
                    DayState::Overdue => 'x',
                };
                if d.muted {
                    c
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect()
    }

    #[test]
    fn the_window_is_orgs_twenty_one_plus_seven() {
        let habit = Habit {
            completions: &[],
            scheduled: date(2026, 9, 3),
            repeater: rep(".+1d/3d"),
        };
        let days = build(&habit, date(2026, 9, 3));
        assert_eq!(days.len() as i64, PRECEDING_DAYS + FOLLOWING_DAYS + 1);
        assert_eq!(days[PRECEDING_DAYS as usize].date, date(2026, 9, 3));
        assert!(days[PRECEDING_DAYS as usize].today);
        assert_eq!(days.iter().filter(|d| d.today).count(), 1);
    }

    /// The rule this module exists to get right, on its own: alert is the
    /// deadline DAY, and red only ever comes after it.
    #[test]
    fn alert_is_the_deadline_day_not_a_band_and_not_today() {
        // due = day 10, MAX-MIN = 2, so late = day 12.
        for (day, want) in [
            (9, DayState::Clear),
            (10, DayState::Ready),
            (11, DayState::Ready),
            (12, DayState::Alert),
            (13, DayState::Overdue),
        ] {
            assert_eq!(state_at(day, 10, 2, false), want, "at day {day}");
        }
    }

    /// Doing it on the deadline day is doing it in time.
    #[test]
    fn a_completion_on_the_deadline_day_is_ready_not_alert() {
        assert_eq!(state_at(12, 10, 2, true), DayState::Ready);
        // But a completion does not rescue a day that was already missed —
        // org gates that behind `org-habit-show-done-always-green`, which is
        // off by default and is not implemented (design §6: no display
        // toggles without a consumer).
        assert_eq!(state_at(13, 10, 2, true), DayState::Overdue);
    }

    /// No `/MAX`: `late == due`, so the habit is never ready. Design §2 says
    /// so, and it falls out of the same cond rather than being special-cased.
    #[test]
    fn a_repeater_without_a_range_goes_clear_then_alert_then_overdue() {
        assert_eq!(rep(".+2d").window_days(), (2, 2));
        for (day, want) in [
            (9, DayState::Clear),
            (10, DayState::Alert),
            (11, DayState::Overdue),
        ] {
            assert_eq!(state_at(day, 10, 0, false), want, "at day {day}");
        }
    }

    /// A habit kept exactly on time, every other day. No day it has LIVED
    /// through is red, which is the picture a kept habit is supposed to make.
    ///
    /// The assertion stops at today deliberately. The trailing seven columns
    /// of even a perfectly kept habit go green → yellow → red, because a
    /// future day assumes you have not done it yet — that run is the graph
    /// prompting you, not a record of failure. Asserting over the whole
    /// window instead of the lived part is how this test first read, and it
    /// failed against correct output.
    #[test]
    fn a_habit_kept_on_time_shows_no_overdue_day() {
        let today = date(2026, 9, 3);
        let now = repeat::to_days(today);
        let dones: Vec<Date> = (1..=10)
            .map(|i| repeat::from_days(now - i * 2))
            .rev()
            .collect();
        let habit = Habit {
            completions: &dones,
            scheduled: repeat::from_days(now + 1),
            repeater: rep(".+2d/4d"),
        };
        let g = render(&habit, today);
        let lived = &g[..=PRECEDING_DAYS as usize];
        assert!(
            !lived.contains('x') && !lived.contains('X'),
            "no day up to today may be missed, got {lived} (whole window {g})"
        );
        assert!(
            g.contains('x') || g.contains('X'),
            "and the future still prompts: the trailing columns pass the \
             deadline, got {g}"
        );
    }

    /// And one abandoned three weeks ago: the recent columns are red, because
    /// each was coloured by how long it had been since the last completion.
    #[test]
    fn an_abandoned_habit_goes_red_after_its_deadline_day() {
        let today = date(2026, 9, 3);
        let now = repeat::to_days(today);
        let habit = Habit {
            completions: &[repeat::from_days(now - 20)],
            scheduled: repeat::from_days(now - 18),
            repeater: rep(".+2d/4d"),
        };
        let days = build(&habit, today);
        let yesterday = days
            .iter()
            .find(|d| repeat::to_days(d.date) == now - 1)
            .expect("yesterday is in the window");
        assert_eq!(yesterday.state, DayState::Overdue);
        assert!(
            !yesterday.muted,
            "a missed day is solid — muting it would hide the thing the graph is for"
        );
    }

    /// The muted axis. A past day that was kept, and a past day that was
    /// missed, are both solid; an ordinary past day is not; the future is not.
    #[test]
    fn the_muted_axis_marks_out_what_matters() {
        let today = date(2026, 9, 3);
        let now = repeat::to_days(today);
        let habit = Habit {
            completions: &[repeat::from_days(now - 2)],
            scheduled: repeat::from_days(now),
            repeater: rep(".+2d/4d"),
        };
        let days = build(&habit, today);
        let at = |off: i64| {
            *days
                .iter()
                .find(|d| repeat::to_days(d.date) == now + off)
                .expect("in window")
        };
        assert!(!at(-2).muted, "a day it was done is solid");
        assert!(at(-10).muted, "an ordinary past day is not shouted");
        assert!(!at(0).muted, "today is always solid");
        assert!(at(3).muted, "the future is muted");
    }

    /// `.+` counts from the completion; `+` counts from the stamp. The graph
    /// has to honour the difference or the past columns of a `+` habit are
    /// coloured by a schedule that never applied.
    #[test]
    fn the_repeater_base_decides_what_a_past_column_was_due_against() {
        // Two completions remain at/after the column, step 2.
        assert_eq!(due_on(&[100, 102, 104], 98, 2, 108, 2, Base::Stamp), 104);
        assert_eq!(
            due_on(&[100, 102, 104], 98, 2, 108, 2, Base::Completion),
            100
        );
        // And with nothing left to come, today's stamp is the schedule that
        // followed the last completion — whatever the base.
        assert_eq!(due_on(&[100], 100, 0, 108, 2, Base::Stamp), 108);
    }

    /// Both palettes are one cell wide, or the column geometry shifts when
    /// `ui.nerd_fonts` is toggled.
    #[test]
    fn both_glyph_palettes_are_one_cell_wide() {
        let day = |done, today| Day {
            date: date(2026, 9, 3),
            state: DayState::Ready,
            done,
            today,
            muted: false,
        };
        for d in [day(true, false), day(false, true), day(false, false)] {
            for nerd in [true, false] {
                assert_eq!(
                    glyph(&d, nerd).to_string().chars().count(),
                    1,
                    "every cell is one char"
                );
            }
        }
        // And a completed day keeps its glyph on today, rather than today's
        // marker hiding that it was done.
        assert_eq!(
            glyph(&day(true, true), false),
            glyph(&day(true, false), false)
        );
    }

    /// The registration name and the reference name differ by the namespace the
    /// host adds at registration and does NOT add at lookup.
    ///
    /// Asserted as a relationship rather than as two literals: a span naming
    /// `habit.ready` while the registry holds `org.habit.ready` resolves to
    /// nothing and paints the default foreground — a monochrome graph, with no
    /// error anywhere. That is what shipped before this test existed.
    #[test]
    fn the_slot_a_consumer_names_is_the_registered_name_namespaced() {
        for s in [
            DayState::Clear,
            DayState::Ready,
            DayState::Alert,
            DayState::Overdue,
        ] {
            for muted in [false, true] {
                assert_eq!(
                    s.theme_slot(muted),
                    format!("org.{}", s.element(muted)),
                    "the two must stay derived from one another"
                );
                assert!(
                    s.theme_slot(muted).starts_with("org.habit."),
                    "a consumer names the namespaced element: {}",
                    s.theme_slot(muted)
                );
            }
        }
    }

    /// Every state has a distinct element in both variants — a copy-paste slip
    /// here would silently paint two states the same colour.
    #[test]
    fn every_state_names_its_own_theme_element() {
        let mut names = Vec::new();
        for s in [
            DayState::Clear,
            DayState::Ready,
            DayState::Alert,
            DayState::Overdue,
        ] {
            names.push(s.element(false));
            names.push(s.element(true));
        }
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), 8, "eight distinct elements, got {names:?}");
    }
}

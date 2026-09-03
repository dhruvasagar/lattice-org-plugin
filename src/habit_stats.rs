//! HB.6 — what the completion history says, beyond the picture.
//!
//! Design: `org-habits.md` §1. The tracking org cannot do is the reason for
//! deriving rather than storing: a `:LOGBOOK:` holds years, and a streak or a
//! rate is arithmetic over it. Nothing here is written to a file, and nothing
//! here needs to be — recomputing is cheaper than keeping two answers honest.
//!
//! Org has no equivalent. That makes these a deliberate lattice addition
//! rather than a port, so each one states its definition, and each definition
//! is chosen to stay true for a habit that is not daily.
//!
//! ## Why the units are not days
//!
//! "A five-day streak" is only meaningful for a `.+1d` habit. Water the plants
//! every three days and five in a row is fifteen days — so the streak is a
//! COUNT of kept repetitions, rendered `5×`, and the rate is measured against
//! how many repetitions the window had room for rather than against its days.
//! A number that reads naturally for a daily habit and lies for a weekly one is
//! worse than one that reads slightly oddly for both.

use crate::org_date::Date;
use crate::repeat::{self, Repeater};
use crate::timestamp;

/// The window the rate is measured over — the same past window the graph draws
/// (`habit_graph::PRECEDING_DAYS`), so the two answers describe the same
/// stretch of time and a user comparing them is not comparing two windows.
pub const RATE_WINDOW_DAYS: i64 = crate::habit_graph::PRECEDING_DAYS;

/// How many completions a weekday needs before its pattern is worth reporting.
///
/// Three is not a statistical threshold, it is an honesty one: with fewer, the
/// "weakest weekday" is whichever one the habit happened to start on.
const MIN_SAMPLES_PER_WEEKDAY: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Kept repetitions in a row, most recent first. Zero when the habit is
    /// currently overdue — a streak you have already broken is not a streak.
    pub streak: u32,
    /// Completions in the window as a percentage of the repetitions it had room
    /// for, capped at 100.
    pub rate_percent: u32,
    /// The weekday completed on least often over the WHOLE history, as
    /// `timestamp::DAY_NAMES`' index.
    ///
    /// `None` unless the habit is daily and every weekday has enough samples to
    /// mean something — see [`MIN_SAMPLES_PER_WEEKDAY`]. A `.+3d` habit's
    /// weekday distribution is an artefact of its cadence, not of the person.
    pub weakest_weekday: Option<usize>,
}

/// Derive the stats for one habit.
///
/// `completions` is [`history::completions`](crate::history::completions)'
/// output: ascending and unique.
pub fn stats(completions: &[Date], repeater: Repeater, today: Date) -> Stats {
    let (min, max) = repeater.window_days();
    let days: Vec<i64> = completions.iter().map(|d| repeat::to_days(*d)).collect();
    Stats {
        streak: streak(&days, i64::from(max), repeat::to_days(today)),
        rate_percent: rate_percent(&days, i64::from(min), repeat::to_days(today)),
        weakest_weekday: weakest_weekday(completions, min),
    }
}

/// Kept repetitions in a row, counting back from the most recent completion.
///
/// A repetition is kept when the next completion lands within `max` days of it
/// — `max` and not `min`, because the range is exactly the tolerance the user
/// wrote: `.+2d/4d` says "every two days, and four is still fine".
///
/// Zero when the habit is currently overdue, which is the case a streak counter
/// most needs to get right: showing "12×" under a habit you dropped a week ago
/// is the failure mode that makes people stop trusting the number.
fn streak(days: &[i64], max: i64, today: i64) -> u32 {
    let Some(&last) = days.last() else {
        return 0;
    };
    if today > last + max {
        return 0;
    }
    let mut n = 1;
    for pair in days.windows(2).rev() {
        if pair[1] - pair[0] > max {
            break;
        }
        n += 1;
    }
    n
}

/// Completions in the window over the repetitions it had room for.
///
/// The denominator is `window / min` — how many times a habit on this cadence
/// COULD have been done in the stretch the graph draws — rather than a count of
/// days, which would report a three-daily habit as 33% while it was being kept
/// perfectly.
///
/// Capped at 100 because the numerator is not bounded by the denominator: a
/// habit done more often than its cadence asks is at 100%, not at 150%.
fn rate_percent(days: &[i64], min: i64, today: i64) -> u32 {
    let start = today - RATE_WINDOW_DAYS;
    let done = days.iter().filter(|d| **d >= start && **d <= today).count() as i64;
    let expected = (RATE_WINDOW_DAYS / min.max(1)).max(1);
    ((done * 100 / expected).min(100)) as u32
}

/// The weekday with the fewest completions across the whole history.
///
/// Only for daily habits, and only once every weekday has enough samples. Both
/// gates exist because the alternative is a confident wrong answer: a `.+3d`
/// habit lands on a rotating subset of weekdays no matter how well it is kept,
/// and a young daily habit has not lived through enough Mondays to have a
/// Monday problem.
///
/// `None` when the counts are level, too — a habit with no weak day should say
/// nothing rather than nominate whichever weekday sorts first.
fn weakest_weekday(completions: &[Date], min: u32) -> Option<usize> {
    if min != 1 {
        return None;
    }
    let mut counts = [0u32; 7];
    for d in completions {
        counts[timestamp::weekday(d.year, d.month, d.day)] += 1;
    }
    if counts.iter().any(|c| *c < MIN_SAMPLES_PER_WEEKDAY) {
        return None;
    }
    let low = *counts.iter().min()?;
    let high = *counts.iter().max()?;
    if low == high {
        return None;
    }
    counts.iter().position(|c| *c == low)
}

/// The suffix appended to the graph row, or empty when there is nothing to say.
///
/// Empty for a habit with no completions at all: `0× · 0%` under a habit you
/// have never done is noise, and the graph already says it.
pub fn render(s: &Stats) -> String {
    if s.streak == 0 && s.rate_percent == 0 {
        return String::new();
    }
    let mut out = format!("  {}× · {}%", s.streak, s.rate_percent);
    if let Some(day) = s.weakest_weekday {
        // `↓` reads as "lowest", and the whole term is dropped rather than
        // spelled out — the graph row is already 29 cells wide.
        out.push_str(&format!(" · {}↓", timestamp::DAY_NAMES[day]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(s: &str) -> Repeater {
        repeat::parse(s).expect("a repeater")
    }

    /// `n` completions ending `end_offset` days before `today`, spaced `every`.
    fn run(today: Date, end_offset: i64, every: i64, n: i64) -> Vec<Date> {
        let last = repeat::to_days(today) - end_offset;
        (0..n)
            .map(|i| repeat::from_days(last - (n - 1 - i) * every))
            .collect()
    }

    const TODAY: Date = Date {
        year: 2026,
        month: 9,
        day: 3,
    };

    #[test]
    fn a_kept_habit_has_a_streak_of_every_completion() {
        let done = run(TODAY, 0, 2, 5);
        assert_eq!(stats(&done, rep(".+2d/4d"), TODAY).streak, 5);
    }

    /// The tolerance is MAX, not MIN — `.+2d/4d` says four days is still fine,
    /// so a three-day gap does not break the run.
    #[test]
    fn a_gap_inside_the_tolerance_does_not_break_the_streak() {
        let base = repeat::to_days(TODAY);
        let done: Vec<Date> = [base - 9, base - 6, base - 3, base]
            .iter()
            .map(|d| repeat::from_days(*d))
            .collect();
        assert_eq!(stats(&done, rep(".+2d/4d"), TODAY).streak, 4);
    }

    /// And a gap past MAX does break it — the streak counts back only to there.
    #[test]
    fn a_gap_past_the_tolerance_ends_the_streak() {
        let base = repeat::to_days(TODAY);
        let done: Vec<Date> = [base - 30, base - 4, base - 2, base]
            .iter()
            .map(|d| repeat::from_days(*d))
            .collect();
        assert_eq!(
            stats(&done, rep(".+2d/4d"), TODAY).streak,
            3,
            "the 26-day gap is the break; everything after it counts"
        );
    }

    /// The case the counter most needs to get right. A habit dropped a week ago
    /// has no streak, however good the run before it was.
    #[test]
    fn a_currently_overdue_habit_has_no_streak() {
        let done = run(TODAY, 10, 2, 12);
        assert_eq!(
            stats(&done, rep(".+2d/4d"), TODAY).streak,
            0,
            "twelve in a row, then a week off — showing 12× would be a lie"
        );
    }

    #[test]
    fn a_habit_never_done_has_no_streak_and_no_rate() {
        let s = stats(&[], rep(".+1d"), TODAY);
        assert_eq!((s.streak, s.rate_percent), (0, 0));
        assert_eq!(render(&s), "", "and says nothing at all");
    }

    /// The rate's denominator is REPETITIONS, not days. A three-daily habit
    /// kept perfectly is 100%, and would be 33% if days were counted.
    #[test]
    fn a_non_daily_habit_kept_perfectly_reads_one_hundred_percent() {
        let done = run(TODAY, 0, 3, 8);
        assert_eq!(stats(&done, rep(".+3d/5d"), TODAY).rate_percent, 100);
    }

    #[test]
    fn half_the_repetitions_reads_about_half() {
        // Room for 21; ten done.
        let done = run(TODAY, 0, 2, 10);
        let got = stats(&done, rep(".+1d"), TODAY).rate_percent;
        assert!((45..=52).contains(&got), "got {got}%");
    }

    /// Doing it more often than asked is 100%, not 150% — a rate above whole is
    /// a number nobody can act on.
    #[test]
    fn overachievement_is_capped() {
        let done = run(TODAY, 0, 1, 21);
        assert_eq!(stats(&done, rep(".+3d/5d"), TODAY).rate_percent, 100);
    }

    /// Completions before the window do not count towards it.
    #[test]
    fn the_rate_only_sees_its_own_window() {
        let old = run(TODAY, RATE_WINDOW_DAYS + 10, 1, 20);
        assert_eq!(stats(&old, rep(".+1d"), TODAY).rate_percent, 0);
    }

    /// The weekday gate. A habit that is not daily lands on a rotating subset of
    /// weekdays however well it is kept, so reporting one would be a confident
    /// wrong answer.
    #[test]
    fn a_non_daily_habit_reports_no_weekday() {
        let done = run(TODAY, 0, 3, 40);
        assert!(stats(&done, rep(".+3d/5d"), TODAY)
            .weakest_weekday
            .is_none());
    }

    /// And a young daily habit has not lived through enough Mondays.
    #[test]
    fn too_few_samples_reports_no_weekday() {
        let done = run(TODAY, 0, 1, 8);
        assert!(stats(&done, rep(".+1d"), TODAY).weakest_weekday.is_none());
    }

    /// With enough history, the weekday actually skipped is the one named.
    #[test]
    fn the_least_completed_weekday_is_the_one_named() {
        // Twelve weeks, every day except Sundays after the first three.
        let base = repeat::to_days(TODAY);
        let mut done = Vec::new();
        for i in 0..84 {
            let day = repeat::from_days(base - i);
            let wd = timestamp::weekday(day.year, day.month, day.day);
            // Keep the first three Sundays so the sample gate passes, drop the
            // rest — the point is a weekday that is present but weakest.
            if wd == 0 && i > 21 {
                continue;
            }
            done.push(day);
        }
        done.reverse();
        assert_eq!(
            stats(&done, rep(".+1d"), TODAY).weakest_weekday,
            Some(0),
            "Sunday is the one skipped"
        );
    }

    /// A perfectly level habit nominates nobody rather than whichever weekday
    /// sorts first.
    #[test]
    fn a_level_history_reports_no_weakest_weekday() {
        let done = run(TODAY, 0, 1, 70);
        assert!(stats(&done, rep(".+1d"), TODAY).weakest_weekday.is_none());
    }

    #[test]
    fn the_suffix_reads_as_designed() {
        let s = Stats {
            streak: 5,
            rate_percent: 80,
            weakest_weekday: Some(1),
        };
        assert_eq!(render(&s), "  5× · 80% · Mon↓");
        let s = Stats {
            weakest_weekday: None,
            ..s
        };
        assert_eq!(render(&s), "  5× · 80%", "the weekday term drops cleanly");
    }
}

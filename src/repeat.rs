//! HB.1 — org repeaters, parsed and applied.
//!
//! Design: `docs/dev/architecture/org-habits.md` §2 (in the lattice repo).
//!
//! [`crate::org_date`] carries a repeater as an opaque trailing string, and
//! says so — that is right for the schedule prompt, which only has to make one
//! survive a round trip. It is not enough for anything that has to *apply*
//! one, which is what completing a repeating task does.
//!
//! ## The four forms, and the one distinction that matters
//!
//! | form | meaning | shifts from |
//! |---|---|---|
//! | `+1d` | every day | the timestamp already on the line |
//! | `++1d` | every day, catching up | that timestamp, advanced until future |
//! | `.+1d` | a day after you did it | **today** |
//! | `.+1d/3d` | habit: ready after 1 day, overdue after 3 | today |
//!
//! `+` versus `.+` is the whole reason both exist: "pay the rent on the 1st"
//! wants `+1m` and must keep landing on the 1st however late you were, while
//! "water the plants every 3 days" wants `.+3d` counted from when you last
//! actually did it. Reading one as the other is not a rounding error — it is a
//! different task.
//!
//! `++` is the catch-up form: shift repeatedly until the result is in the
//! future. A single shift would leave a monthly bill that lapsed for a year
//! still overdue, one month at a time, which is exactly the case it exists for.
//!
//! ## `/MAX` is habit-only
//!
//! `.+1d/3d` says ready after one day, overdue after three. Only the
//! consistency graph reads `MAX`; the shift ignores it. A habit written
//! without one is still a habit — `MAX` defaults to `MIN`, which makes every
//! day either done or overdue, and that is the honest reading of "every day,
//! no slack".

use crate::org_date::Date;

/// Which timestamp a shift counts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// `+` — from the timestamp on the line. May land in the past, and that
    /// is the point: a monthly bill stays on the 1st.
    Stamp,
    /// `++` — from the timestamp, advanced until it is in the future.
    StampCatchUp,
    /// `.+` — from the completion date.
    Completion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Day,
    Week,
    Month,
    Year,
}

impl Unit {
    fn parse(c: char) -> Option<Unit> {
        match c {
            'd' => Some(Unit::Day),
            'w' => Some(Unit::Week),
            'm' => Some(Unit::Month),
            'y' => Some(Unit::Year),
            _ => None,
        }
    }

    fn label(self) -> char {
        match self {
            Unit::Day => 'd',
            Unit::Week => 'w',
            Unit::Month => 'm',
            Unit::Year => 'y',
        }
    }
}

/// A parsed repeater.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeater {
    pub base: Base,
    pub count: u32,
    pub unit: Unit,
    /// The habit range's upper bound, when the repeater carried `/MAX`.
    /// `None` means "no slack" — see the module doc.
    pub max: Option<(u32, Unit)>,
}

impl Repeater {
    /// Re-render exactly as org writes it, so a round trip is byte-identical.
    ///
    /// Not cosmetic: this text goes back into the user's file, which emacs
    /// also reads. A repeater that came back spelled differently would show
    /// up as a spurious diff on every completion.
    pub fn render(&self) -> String {
        let prefix = match self.base {
            Base::Stamp => "+",
            Base::StampCatchUp => "++",
            Base::Completion => ".+",
        };
        let mut s = format!("{prefix}{}{}", self.count, self.unit.label());
        if let Some((n, u)) = self.max {
            s.push('/');
            s.push_str(&format!("{n}{}", u.label()));
        }
        s
    }

    /// The habit window as `(min_days, max_days)`.
    ///
    /// Months and years are approximated in days here — the graph is a
    /// per-day picture and has nowhere to put "one month" — and the
    /// approximation is stated rather than hidden: 30 and 365. A monthly
    /// habit's bar is therefore indicative, not exact, which is true of any
    /// per-day rendering of a monthly cadence.
    pub fn window_days(&self) -> (u32, u32) {
        let min = days_of(self.count, self.unit);
        let max = self.max.map(|(n, u)| days_of(n, u)).unwrap_or(min);
        (min, min.max(max))
    }
}

fn days_of(count: u32, unit: Unit) -> u32 {
    match unit {
        Unit::Day => count,
        Unit::Week => count.saturating_mul(7),
        Unit::Month => count.saturating_mul(30),
        Unit::Year => count.saturating_mul(365),
    }
}

/// Parse a repeater token (`+1d`, `++2w`, `.+1d/3d`). `None` if it is not one.
pub fn parse(token: &str) -> Option<Repeater> {
    let (base, rest) = if let Some(r) = token.strip_prefix(".+") {
        (Base::Completion, r)
    } else if let Some(r) = token.strip_prefix("++") {
        (Base::StampCatchUp, r)
    } else if let Some(r) = token.strip_prefix('+') {
        (Base::Stamp, r)
    } else {
        return None;
    };
    // `/` splits the habit range. A trailing `/` with nothing after it is
    // malformed rather than "no max": accepting it would silently turn a typo
    // into a different cadence.
    let (min_part, max_part) = match rest.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    let (count, unit) = parse_amount(min_part)?;
    let max = match max_part {
        Some(m) => Some(parse_amount(m)?),
        None => None,
    };
    Some(Repeater {
        base,
        count,
        unit,
        max,
    })
}

fn parse_amount(s: &str) -> Option<(u32, Unit)> {
    let mut chars = s.chars();
    let unit = Unit::parse(chars.next_back()?)?;
    let digits: String = chars.collect();
    // A bare `+d` is not "once a day", it is malformed. Org requires the
    // count, and inventing a 1 would guess at what someone mistyped.
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let count: u32 = digits.parse().ok()?;
    // `+0d` would repeat forever on the same day — a shift that never
    // advances is an infinite loop wearing a valid syntax.
    (count > 0).then_some((count, unit))
}

/// Where a repeating task's timestamp goes when it is completed on `today`.
///
/// `stamp` is the date currently on the line. Returns the new date.
pub fn next_date(rep: &Repeater, stamp: Date, today: Date) -> Date {
    match rep.base {
        Base::Completion => shift(today, rep.count, rep.unit),
        Base::Stamp => shift(stamp, rep.count, rep.unit),
        Base::StampCatchUp => {
            let mut d = shift(stamp, rep.count, rep.unit);
            // Bounded: a pathological stamp (year 1) with `++1d` would
            // otherwise spin for ~700k iterations. 4096 shifts covers every
            // real lapse — eleven years of daily, three centuries of monthly
            // — and past that the honest answer is the last one computed
            // rather than a hang.
            let mut guard = 0;
            while !is_after(d, today) && guard < 4096 {
                d = shift(d, rep.count, rep.unit);
                guard += 1;
            }
            d
        }
    }
}

fn is_after(a: Date, b: Date) -> bool {
    (a.year, a.month, a.day) > (b.year, b.month, b.day)
}

/// Advance `date` by `count` units, clamping the day into the target month.
///
/// 31 January + 1 month is 28 February, not 3 March. Org clamps and so does
/// every calendar people reason with; overflowing would put a monthly task on
/// a date the user never chose, drifting a day further every month.
pub fn shift(date: Date, count: u32, unit: Unit) -> Date {
    match unit {
        Unit::Day => add_days(date, count as i64),
        Unit::Week => add_days(date, count as i64 * 7),
        Unit::Month => add_months(date, count as i64),
        Unit::Year => add_months(date, count as i64 * 12),
    }
}

fn add_months(date: Date, months: i64) -> Date {
    let total = (date.year as i64) * 12 + (date.month as i64 - 1) + months;
    let year = total.div_euclid(12) as i32;
    let month = (total.rem_euclid(12) + 1) as u32;
    let day = date.day.min(days_in_month(year, month));
    Date { year, month, day }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Civil-date arithmetic through a day number, so month/year rollover is the
/// conversion's problem rather than a chain of carries to get wrong.
pub fn add_days(date: Date, days: i64) -> Date {
    from_days(to_days(date) + days)
}

/// Days since 1970-01-01. Howard Hinnant's `days_from_civil`.
pub fn to_days(d: Date) -> i64 {
    let y = if d.month <= 2 { d.year - 1 } else { d.year } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = d.month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d.day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of [`to_days`].
pub fn from_days(z: i64) -> Date {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    Date {
        year: (if month <= 2 { y + 1 } else { y }) as i32,
        month,
        day,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn d(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    // ── parsing ─────────────────────────────────────────────────────────

    #[test]
    fn the_three_bases_are_distinguished() {
        assert_eq!(parse("+1d").unwrap().base, Base::Stamp);
        assert_eq!(parse("++1d").unwrap().base, Base::StampCatchUp);
        assert_eq!(parse(".+1d").unwrap().base, Base::Completion);
    }

    /// The user's own habit shape, from their capture template.
    #[test]
    fn a_habit_range_parses() {
        let r = parse(".+1d/3d").unwrap();
        assert_eq!(r.base, Base::Completion);
        assert_eq!((r.count, r.unit), (1, Unit::Day));
        assert_eq!(r.max, Some((3, Unit::Day)));
        assert_eq!(r.window_days(), (1, 3));
    }

    /// No `/MAX` means no slack — every day is done or overdue. Defaulting
    /// max to min is the honest reading; defaulting it to something larger
    /// would invent tolerance the user did not write.
    #[test]
    fn a_repeater_without_a_max_has_no_slack() {
        let r = parse(".+2d").unwrap();
        assert_eq!(r.max, None, "no `/MAX` was written");
        assert_eq!(r.window_days(), (2, 2));
    }

    #[test]
    fn every_unit_parses() {
        for (s, u) in [
            ("+1d", Unit::Day),
            ("+1w", Unit::Week),
            ("+1m", Unit::Month),
            ("+1y", Unit::Year),
        ] {
            assert_eq!(parse(s).unwrap().unit, u, "{s}");
        }
    }

    /// A repeater must round-trip byte-identically: it goes back into a file
    /// emacs also reads, and a re-spelling would be a spurious diff on every
    /// completion.
    #[test]
    fn rendering_round_trips() {
        for s in ["+1d", "++2w", ".+1d/3d", ".+3m", "+10y", ".+2w/1m"] {
            assert_eq!(parse(s).unwrap().render(), s, "{s}");
        }
    }

    #[test]
    fn non_repeaters_are_refused() {
        for s in ["", "1d", "-1d", "+", "+d", "+1x", "+0d", ".+1d/", "+1d/x"] {
            assert!(parse(s).is_none(), "{s:?} must not parse");
        }
    }

    // ── shifting ────────────────────────────────────────────────────────

    /// The distinction the whole module exists for. Completed late, `+`
    /// keeps the original cadence and `.+` restarts from today.
    #[test]
    fn plus_keeps_the_cadence_and_dotplus_restarts_from_today() {
        let stamp = d(2026, 9, 1);
        let today = d(2026, 9, 10);
        assert_eq!(
            next_date(&parse("+3d").unwrap(), stamp, today),
            d(2026, 9, 4),
            "`+` counts from the stamp — still in the past, deliberately"
        );
        assert_eq!(
            next_date(&parse(".+3d").unwrap(), stamp, today),
            d(2026, 9, 13),
            "`.+` counts from the completion date"
        );
    }

    /// `++` catches up: it shifts until the result is in the future, which is
    /// the case it exists for — a single shift leaves a lapsed monthly task
    /// still overdue.
    #[test]
    fn plusplus_catches_up_past_today() {
        let stamp = d(2026, 1, 1);
        let today = d(2026, 9, 10);
        let got = next_date(&parse("++1m").unwrap(), stamp, today);
        assert_eq!(got, d(2026, 10, 1));
        // …and it is genuinely a loop, not one shift: a single `+1m` would
        // have answered February.
        assert_eq!(
            next_date(&parse("+1m").unwrap(), stamp, today),
            d(2026, 2, 1)
        );
    }

    /// `++` on a stamp that is already in the future shifts exactly once —
    /// the loop must not run when there is nothing to catch up.
    #[test]
    fn plusplus_on_a_future_stamp_shifts_once() {
        let stamp = d(2026, 12, 1);
        let today = d(2026, 9, 10);
        assert_eq!(
            next_date(&parse("++1m").unwrap(), stamp, today),
            d(2027, 1, 1)
        );
    }

    /// 31 January + 1 month is 28 February. Overflowing to 3 March would put
    /// a monthly task on a date the user never chose and drift it further
    /// every month.
    #[test]
    fn a_month_shift_clamps_into_the_target_month() {
        assert_eq!(shift(d(2026, 1, 31), 1, Unit::Month), d(2026, 2, 28));
        assert_eq!(shift(d(2024, 1, 31), 1, Unit::Month), d(2024, 2, 29));
        assert_eq!(shift(d(2026, 1, 31), 3, Unit::Month), d(2026, 4, 30));
    }

    #[test]
    fn day_and_week_shifts_roll_over_months_and_years() {
        assert_eq!(shift(d(2026, 12, 30), 3, Unit::Day), d(2027, 1, 2));
        assert_eq!(shift(d(2026, 2, 26), 1, Unit::Week), d(2026, 3, 5));
        assert_eq!(shift(d(2024, 2, 28), 1, Unit::Day), d(2024, 2, 29), "leap");
        assert_eq!(shift(d(2026, 2, 28), 1, Unit::Day), d(2026, 3, 1));
    }

    #[test]
    fn a_year_shift_is_twelve_months_and_clamps_on_leap_day() {
        assert_eq!(shift(d(2026, 6, 15), 1, Unit::Year), d(2027, 6, 15));
        assert_eq!(shift(d(2024, 2, 29), 1, Unit::Year), d(2025, 2, 28));
    }

    /// The civil-date conversion is the arithmetic everything else rests on,
    /// so it is checked against known epochs rather than against itself.
    #[test]
    fn the_day_conversion_round_trips_and_matches_known_epochs() {
        assert_eq!(to_days(d(1970, 1, 1)), 0);
        assert_eq!(to_days(d(2000, 1, 1)), 10_957);
        assert_eq!(from_days(0), d(1970, 1, 1));
        for n in [-100_000i64, -1, 0, 1, 19_000, 100_000] {
            assert_eq!(to_days(from_days(n)), n, "round trip at {n}");
        }
    }

    /// The catch-up guard must terminate on a pathological input rather than
    /// spin — an editor that hangs on a malformed timestamp is worse than one
    /// that answers a slightly wrong date.
    #[test]
    fn the_catch_up_loop_is_bounded() {
        let got = next_date(&parse("++1d").unwrap(), d(1900, 1, 1), d(2026, 9, 10));
        // It cannot reach 2026 in 4096 daily shifts (~11 years), so the guard
        // fires and the answer is bounded, finite, and not a hang.
        assert!(got.year >= 1900, "terminated with a real date, got {got:?}");
    }
}

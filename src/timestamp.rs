//! OM.9 — org timestamps, and stepping the component under the cursor.
//!
//! ```org
//! <2026-08-25 Tue>
//! [2026-08-25 Tue 10:30]
//! ```
//!
//! `<…>` is active (it reaches the agenda), `[…]` inactive. Both step the
//! same way.
//!
//! ## Why the date maths is hand-rolled
//!
//! A `chrono`-shaped dependency in a wasm guest costs binary size for
//! arithmetic that is a dozen lines: days-in-month, a leap rule, and Zeller's
//! congruence for the weekday. The weekday matters — org writes it into the
//! stamp, and a stamp whose day name disagrees with its date is worse than no
//! day name at all, so every edit recomputes it rather than carrying the old
//! one forward.

pub const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Which part of a stamp the cursor is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Year,
    Month,
    Day,
    Hour,
    Minute,
}

/// A parsed org timestamp and where it sits in its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub start: usize,
    pub end: usize,
    pub active: bool,
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// `None` for a date-only stamp.
    pub time: Option<(u32, u32)>,
}

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

/// Day of week (0 = Sunday) by Zeller's congruence.
pub fn weekday(year: i32, month: u32, day: u32) -> usize {
    let (m, y) = if month < 3 {
        (month + 12, year - 1)
    } else {
        (month, year)
    };
    let k = y % 100;
    let j = y / 100;
    let h = (day as i32 + (13 * (m as i32 + 1)) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    // Zeller yields 0 = Saturday; shift to 0 = Sunday.
    (((h + 6) % 7) as usize) % 7
}

/// Find the timestamp containing `byte`, if the cursor is inside one.
pub fn stamp_at(line: &str, byte: usize) -> Option<Stamp> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let (open, close, active) = match b[i] {
            b'<' => (b'<', b'>', true),
            b'[' => (b'[', b']', false),
            _ => {
                i += 1;
                continue;
            }
        };
        let _ = open;
        let Some(end) = line[i..].find(close as char).map(|o| i + o) else {
            break;
        };
        if let Some(mut s) = parse_inner(&line[i + 1..end]) {
            s.start = i;
            s.end = end + 1;
            s.active = active;
            // Inclusive of both delimiters, so pressing on `<` works.
            if byte >= s.start && byte < s.end {
                return Some(s);
            }
        }
        i = end + 1;
    }
    None
}

/// `2026-08-25 Tue` / `2026-08-25 Tue 10:30` — the day name is optional on
/// input (a hand-typed stamp often lacks it) and always written on output.
fn parse_inner(inner: &str) -> Option<Stamp> {
    let mut parts = inner.split_whitespace();
    let date = parts.next()?;
    let mut d = date.split('-');
    let year: i32 = d.next()?.parse().ok()?;
    let month: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(1..=12).contains(&month) || day == 0 || day > 31 {
        return None;
    }
    // Anything after the date is an optional day name then an optional time.
    let mut time = None;
    for p in parts {
        if let Some((h, m)) = p.split_once(':') {
            let h: u32 = h.parse().ok()?;
            let m: u32 = m.parse().ok()?;
            if h > 23 || m > 59 {
                return None;
            }
            time = Some((h, m));
        }
    }
    Some(Stamp {
        start: 0,
        end: 0,
        active: true,
        year,
        month,
        day,
        time,
    })
}

/// Which component `byte` sits on, within `stamp`.
///
/// Defaults to the DAY when the cursor is on a delimiter or the day name:
/// `<C-a>` on a stamp almost always means "next day", and the alternative —
/// declining — would make the key do nothing on the most common target.
pub fn part_at(line: &str, stamp: &Stamp, byte: usize) -> Part {
    let inner_start = stamp.start + 1;
    // On a delimiter itself — `saturating_sub` would otherwise fold `<` onto
    // the year, and pressing on the bracket means "the day" like everywhere
    // else the cursor is not on a number.
    if byte < inner_start {
        return Part::Day;
    }
    let rel = byte - inner_start;
    let inner = &line[inner_start..stamp.end.saturating_sub(1)];
    let Some(date) = inner.split_whitespace().next() else {
        return Part::Day;
    };
    // `YYYY-MM-DD`
    if rel < 4 {
        return Part::Year;
    }
    if rel < 7 {
        return Part::Month;
    }
    if rel < date.len() {
        return Part::Day;
    }
    // Past the date: a time if the cursor is on one.
    if let Some(off) = inner.find(':') {
        if rel >= off.saturating_sub(2) && rel < off {
            return Part::Hour;
        }
        if rel > off {
            return Part::Minute;
        }
    }
    Part::Day
}

/// Step `part` by `delta`, renormalising the date and recomputing the day
/// name. Returns the rewritten LINE.
pub fn step(line: &str, stamp: &Stamp, part: Part, delta: i64) -> String {
    let mut s = stamp.clone();
    match part {
        Part::Year => s.year += delta as i32,
        Part::Month => {
            let m = s.month as i64 - 1 + delta;
            s.year += m.div_euclid(12) as i32;
            s.month = m.rem_euclid(12) as u32 + 1;
        }
        Part::Day => {
            let mut day = s.day as i64 + delta;
            // Walk whole months rather than converting to a day number: the
            // ranges here are a keypress at a time, so the loop is bounded by
            // the delta and stays exact across month lengths and leap years.
            while day < 1 {
                s.month = if s.month == 1 {
                    s.year -= 1;
                    12
                } else {
                    s.month - 1
                };
                day += days_in_month(s.year, s.month) as i64;
            }
            loop {
                let len = days_in_month(s.year, s.month) as i64;
                if day <= len {
                    break;
                }
                day -= len;
                s.month = if s.month == 12 {
                    s.year += 1;
                    1
                } else {
                    s.month + 1
                };
            }
            s.day = day as u32;
        }
        Part::Hour | Part::Minute => {
            let (h, m) = s.time.unwrap_or((0, 0));
            let total = if part == Part::Hour {
                h as i64 * 60 + m as i64 + delta * 60
            } else {
                h as i64 * 60 + m as i64 + delta
            };
            // A time crossing midnight moves the DATE, which is what makes
            // `<C-a>` on `23:30` land on tomorrow rather than wrapping in
            // place and silently lying about the day.
            let day_shift = total.div_euclid(24 * 60);
            let mins = total.rem_euclid(24 * 60);
            s.time = Some(((mins / 60) as u32, (mins % 60) as u32));
            if day_shift != 0 {
                let shifted = step(line, &s, Part::Day, day_shift);
                // Re-parse to pick up the normalised date, then render.
                if let Some(re) = stamp_at(&shifted, s.start) {
                    s.year = re.year;
                    s.month = re.month;
                    s.day = re.day;
                }
            }
        }
    }
    // Clamp a day that a month change made invalid: 31 Jan +1 month is 28/29
    // Feb, not "31 Feb". Org does the same.
    let len = days_in_month(s.year, s.month);
    if s.day > len {
        s.day = len;
    }
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..stamp.start]);
    out.push_str(&render(&s));
    out.push_str(&line[stamp.end..]);
    out
}

/// Write a stamp back out, always with a freshly computed day name.
pub fn render(s: &Stamp) -> String {
    let (open, close) = if s.active { ('<', '>') } else { ('[', ']') };
    let dow = DAY_NAMES[weekday(s.year, s.month, s.day)];
    match s.time {
        Some((h, m)) => format!(
            "{open}{:04}-{:02}-{:02} {dow} {:02}:{:02}{close}",
            s.year, s.month, s.day, h, m
        ),
        None => format!(
            "{open}{:04}-{:02}-{:02} {dow}{close}",
            s.year, s.month, s.day
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: &str, byte: usize) -> Stamp {
        stamp_at(line, byte).expect("a stamp")
    }

    #[test]
    fn parses_both_bracket_forms_with_and_without_a_time() {
        let s = at("<2026-08-25 Tue>", 2);
        assert_eq!(
            (s.year, s.month, s.day, s.time, s.active),
            (2026, 8, 25, None, true)
        );
        let s = at("[2026-08-25 Tue 10:30]", 2);
        assert_eq!(
            (s.year, s.month, s.day, s.time, s.active),
            (2026, 8, 25, Some((10, 30)), false)
        );
        // The day name is optional on input.
        assert_eq!(at("<2026-08-25>", 2).day, 25);
    }

    #[test]
    fn ignores_brackets_that_are_not_stamps() {
        assert!(stamp_at("- [X] a checkbox", 3).is_none());
        assert!(stamp_at("[[file:a.png]]", 3).is_none());
        assert!(stamp_at("* Shop [1/3]", 8).is_none());
        assert!(stamp_at("no stamp", 2).is_none());
    }

    /// The cursor is rarely on a digit; `<C-a>` on a stamp almost always
    /// means "next day", so the day is the default rather than a decline.
    #[test]
    fn the_component_defaults_to_the_day() {
        let l = "<2026-08-25 Tue>";
        let s = at(l, 0);
        assert_eq!(part_at(l, &s, 0), Part::Day, "on the `<`");
        assert_eq!(part_at(l, &s, 1), Part::Year);
        assert_eq!(part_at(l, &s, 6), Part::Month);
        assert_eq!(part_at(l, &s, 9), Part::Day);
        assert_eq!(part_at(l, &s, 13), Part::Day, "on the day name");
    }

    /// The weekday is recomputed on every edit. A stamp whose day name
    /// disagrees with its date is worse than no day name at all.
    #[test]
    fn stepping_a_day_recomputes_the_weekday() {
        let l = "<2026-08-25 Tue>";
        let s = at(l, 9);
        assert_eq!(step(l, &s, Part::Day, 1), "<2026-08-26 Wed>");
        assert_eq!(step(l, &s, Part::Day, -1), "<2026-08-24 Mon>");
    }

    #[test]
    fn day_arithmetic_crosses_months_and_years() {
        let l = "<2026-08-31 Mon>";
        assert_eq!(step(l, &at(l, 9), Part::Day, 1), "<2026-09-01 Tue>");
        let l = "<2026-12-31 Thu>";
        assert_eq!(step(l, &at(l, 9), Part::Day, 1), "<2027-01-01 Fri>");
        let l = "<2026-01-01 Thu>";
        assert_eq!(step(l, &at(l, 9), Part::Day, -1), "<2025-12-31 Wed>");
    }

    /// Leap years are real days, not an approximation.
    #[test]
    fn february_is_correct_in_leap_and_common_years() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29, "divisible by 400");
        assert_eq!(days_in_month(1900, 2), 28, "divisible by 100, not 400");
        let l = "<2024-02-28 Wed>";
        assert_eq!(step(l, &at(l, 9), Part::Day, 1), "<2024-02-29 Thu>");
    }

    /// 31 Jan + 1 month is the end of February, not "31 Feb". Org clamps the
    /// same way.
    #[test]
    fn a_month_step_clamps_an_impossible_day() {
        let l = "<2026-01-31 Sat>";
        assert_eq!(step(l, &at(l, 6), Part::Month, 1), "<2026-02-28 Sat>");
        let l = "<2024-01-31 Wed>";
        assert_eq!(step(l, &at(l, 6), Part::Month, 1), "<2024-02-29 Thu>");
    }

    /// A time crossing midnight moves the DATE. Wrapping in place would leave
    /// the stamp silently lying about which day it means.
    #[test]
    fn a_time_crossing_midnight_moves_the_date() {
        let l = "<2026-08-25 Tue 23:30>";
        let s = at(l, 17);
        assert_eq!(step(l, &s, Part::Hour, 1), "<2026-08-26 Wed 00:30>");
        let l = "<2026-08-25 Tue 00:30>";
        let s = at(l, 17);
        assert_eq!(step(l, &s, Part::Hour, -1), "<2026-08-24 Mon 23:30>");
    }

    #[test]
    fn minutes_step_and_carry_into_the_hour() {
        let l = "<2026-08-25 Tue 10:59>";
        let s = at(l, 20);
        assert_eq!(step(l, &s, Part::Minute, 1), "<2026-08-25 Tue 11:00>");
    }

    /// The surrounding text survives untouched — a stamp inside a line is
    /// rewritten in place.
    #[test]
    fn text_around_the_stamp_is_preserved() {
        let l = "SCHEDULED: <2026-08-25 Tue> and more";
        let s = at(l, 20);
        assert_eq!(
            step(l, &s, Part::Day, 1),
            "SCHEDULED: <2026-08-26 Wed> and more"
        );
    }

    /// Zeller against dates whose weekday is independently known.
    #[test]
    fn the_weekday_table_is_right() {
        assert_eq!(DAY_NAMES[weekday(2000, 1, 1)], "Sat");
        assert_eq!(DAY_NAMES[weekday(2026, 8, 25)], "Tue");
        assert_eq!(DAY_NAMES[weekday(1970, 1, 1)], "Thu");
        assert_eq!(DAY_NAMES[weekday(2024, 2, 29)], "Thu");
    }
}

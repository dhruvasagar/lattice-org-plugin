//! OA.24 — the date grammar `s` and `d` prompt with.
//!
//! Slice plan: `lattice/docs/dev/operations/slice-plans/org-agenda.md` phase 7.
//!
//! The only date parser before this was [`crate::roam_dailies::parse`], which
//! takes `YYYY-MM-DD` and nothing else. That is the right surface for "open the
//! journal for a date" and the wrong one for scheduling: most of the value of
//! pressing `s` is typing `+1d` rather than working out what Wednesday's date
//! is, and a prompt that refuses `+1d` is a prompt people stop using.
//!
//! ## The grammar
//!
//! ```text
//!   2026-09-15        an absolute date
//!   +1d  -2w  +3m  +1y   an offset from today; a bare `+3` is days
//!   .    today  tod       today
//!   tomorrow  tom         …and its neighbours
//!   yesterday  yes
//!   mon tue wed thu fri sat sun    the soonest day with that name
//!
//!   …any of the above, optionally followed by a REPEATER:
//!   +1d .+1d/3d       →  <2026-09-03 Wed> .+1d/3d
//! ```
//!
//! ## Two choices worth stating
//!
//! **A weekday name includes today.** `fri` typed on a Friday is that Friday,
//! not the next one. Emacs' documentation says "the next Wednesday" and is
//! silent on the boundary; scheduling something for `fri` on Friday morning and
//! having it land a week away is the more surprising of the two readings, and
//! `+7d` says the other thing unambiguously when it is wanted.
//!
//! **A repeater is carried, not parsed.** `.+1d/3d` is a habit's shape and org
//! owns its meaning; this module's job is to put it back on the line it came
//! from. Parsing it would mean deciding what it means, which is TK-shaped work
//! nothing has asked for.

use crate::timestamp::{self, DAY_NAMES};

/// A calendar date, resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

/// What the prompt resolved to: a date, plus whatever repeater rode along.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub date: Date,
    /// `.+1d/3d`, verbatim, or empty.
    pub repeater: String,
}

impl Plan {
    /// The org stamp this plan writes — `<2026-09-03 Wed>`, plus its repeater.
    pub fn render(&self) -> String {
        let dow = DAY_NAMES[timestamp::weekday(self.date.year, self.date.month, self.date.day)];
        let Date { year, month, day } = self.date;
        if self.repeater.is_empty() {
            format!("<{year:04}-{month:02}-{day:02} {dow}>")
        } else {
            format!("<{year:04}-{month:02}-{day:02} {dow} {}>", self.repeater)
        }
    }
}

/// Parse a prompt answer against `today`, given as `(year, month, day)`.
///
/// `today` is threaded rather than read from the clock so every test states the
/// day it is asserting against. A date test anchored on "now" is one that passes
/// until the day it does not, and finding out from a user is the expensive way.
pub fn parse(input: &str, today: Date) -> Result<Plan, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("org: no date given".to_string());
    }
    // The repeater is a trailing token, so split it off before reading the date
    // — `+1d .+1d/3d` is two tokens and only the first is a date expression.
    let (date_part, repeater) = split_repeater(trimmed);
    let date = parse_date(date_part, today)?;
    Ok(Plan {
        date,
        repeater: repeater.to_string(),
    })
}

/// Split a trailing repeater off the input. Returns `(date, repeater)`.
///
/// A repeater starts with `+`, `++` or `.+` and is never the FIRST token — a
/// leading `+1d` is an offset, and only a second such token is a repeat rule.
fn split_repeater(input: &str) -> (&str, &str) {
    let Some(idx) = input.find(char::is_whitespace) else {
        return (input, "");
    };
    let (head, tail) = input.split_at(idx);
    let tail = tail.trim();
    if is_repeater(tail) {
        (head, tail)
    } else {
        // Not a repeater: leave the input whole so the date parser can reject
        // it by name. Silently dropping a trailing token would turn a typo
        // into a date the user did not ask for.
        (input, "")
    }
}

fn is_repeater(token: &str) -> bool {
    let body = token
        .strip_prefix(".+")
        .or_else(|| token.strip_prefix("++"))
        .or_else(|| token.strip_prefix('+'));
    let Some(body) = body else {
        return false;
    };
    // `1d` or `1d/3d`; every part must be a count and a unit.
    body.split('/').all(|part| {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        !digits.is_empty()
            && part.len() == digits.len() + 1
            && matches!(part.as_bytes()[digits.len()], b'd' | b'w' | b'm' | b'y')
    })
}

fn parse_date(input: &str, today: Date) -> Result<Date, String> {
    let lower = input.to_ascii_lowercase();
    match lower.as_str() {
        "." | "today" | "tod" => return Ok(today),
        "tomorrow" | "tom" => return Ok(add_days(today, 1)),
        "yesterday" | "yes" => return Ok(add_days(today, -1)),
        _ => {}
    }
    if let Some(idx) = DAY_NAMES
        .iter()
        .position(|d| d.eq_ignore_ascii_case(&lower[..lower.len().min(3)]))
        .filter(|_| lower.len() == 3)
    {
        return Ok(next_weekday(today, idx));
    }
    if lower.starts_with('+') || lower.starts_with('-') {
        return parse_offset(&lower, today);
    }
    parse_absolute(input)
}

/// `+1d` / `-2w` / `+3m` / `+1y`, and a bare `+3` meaning days.
fn parse_offset(input: &str, today: Date) -> Result<Date, String> {
    let sign: i64 = if input.starts_with('-') { -1 } else { 1 };
    let body = &input[1..];
    let digits: String = body.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return Err(format!("org: `{input}` has no count — try `+1d`"));
    }
    let n: i64 = digits
        .parse()
        .map_err(|_| format!("org: `{digits}` is too large"))?;
    let unit = &body[digits.len()..];
    match unit {
        // A bare `+3` is days, which is what org means by it.
        "" | "d" => Ok(add_days(today, sign * n)),
        "w" => Ok(add_days(today, sign * n * 7)),
        // Months and years move the FIELD, then clamp: `+1m` on the 31st of
        // January is the 28th of February, not the 3rd of March. Adding 30 days
        // would be the other answer and is not what "next month" means.
        "m" => Ok(add_months(today, sign * n)),
        "y" => Ok(add_months(today, sign * n * 12)),
        other => Err(format!("org: `{other}` is not a unit — use d, w, m or y")),
    }
}

fn parse_absolute(input: &str) -> Result<Date, String> {
    let malformed = || format!("org: `{input}` is not a date — try `+1d`, `fri` or `2026-09-15`");
    let parts: Vec<&str> = input.split('-').collect();
    let [y, m, d] = parts.as_slice() else {
        return Err(malformed());
    };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return Err(malformed());
    }
    let year: i32 = y.parse().map_err(|_| malformed())?;
    let month: u32 = m.parse().map_err(|_| malformed())?;
    let day: u32 = d.parse().map_err(|_| malformed())?;
    if !(1..=12).contains(&month) || day < 1 || day > timestamp::days_in_month(year, month) {
        return Err(malformed());
    }
    Ok(Date { year, month, day })
}

/// The soonest day named `target`, today included — see the module header.
fn next_weekday(today: Date, target: usize) -> Date {
    let current = timestamp::weekday(today.year, today.month, today.day);
    let ahead = (target + 7 - current) % 7;
    add_days(today, ahead as i64)
}

/// `today` shifted by `delta` days, through the epoch-day domain so month and
/// year boundaries are the calendar's problem rather than this module's.
fn add_days(today: Date, delta: i64) -> Date {
    from_epoch_day(timestamp::epoch_day(today.year, today.month, today.day) + delta)
}

/// `today` shifted by `delta` months, clamping the day into the target month.
fn add_months(today: Date, delta: i64) -> Date {
    let total = i64::from(today.year) * 12 + i64::from(today.month) - 1 + delta;
    let year = (total.div_euclid(12)) as i32;
    let month = (total.rem_euclid(12) + 1) as u32;
    let day = today.day.min(timestamp::days_in_month(year, month));
    Date { year, month, day }
}

/// The inverse of [`timestamp::epoch_day`]. Civil-from-days, the companion to
/// the days-from-civil that module already has.
fn from_epoch_day(z: i64) -> Date {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    Date {
        year: (if m <= 2 { y + 1 } else { y }) as i32,
        month: m as u32,
        day: d as u32,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    /// Wednesday, 2 September 2026 — every test states the day it asserts
    /// against, because a date test anchored on "now" passes until the day it
    /// does not.
    const TODAY: Date = Date {
        year: 2026,
        month: 9,
        day: 2,
    };

    fn on(input: &str) -> Plan {
        parse(input, TODAY).unwrap_or_else(|e| panic!("`{input}`: {e}"))
    }

    #[test]
    fn the_anchor_is_the_wednesday_the_tests_assume() {
        // If this ever fails, every weekday assertion below is meaningless.
        assert_eq!(DAY_NAMES[timestamp::weekday(2026, 9, 2)], "Wed");
    }

    #[test]
    fn an_absolute_date_renders_with_its_day_name() {
        assert_eq!(on("2026-09-15").render(), "<2026-09-15 Tue>");
    }

    #[test]
    fn day_offsets_cross_a_month_boundary() {
        // The reason offsets go through the epoch-day domain: September has 30
        // days, and arithmetic that clamped instead of carrying would give the
        // 30th twice.
        assert_eq!(on("+1d").render(), "<2026-09-03 Thu>");
        assert_eq!(on("+28d").render(), "<2026-09-30 Wed>");
        assert_eq!(on("+29d").render(), "<2026-10-01 Thu>");
        assert_eq!(on("-2d").render(), "<2026-08-31 Mon>");
    }

    #[test]
    fn a_bare_count_is_days_as_org_means_it() {
        assert_eq!(on("+3").render(), on("+3d").render());
    }

    #[test]
    fn weeks_are_seven_days() {
        assert_eq!(on("+1w").render(), "<2026-09-09 Wed>");
        assert_eq!(on("-2w").render(), "<2026-08-19 Wed>");
    }

    #[test]
    fn months_move_the_field_and_clamp_rather_than_adding_thirty_days() {
        // `+1m` on the 31st of January is the 28th of February. Adding 30 days
        // gives the 2nd of March, which is not what "next month" means and is
        // the bug a days-based implementation ships with.
        let jan31 = Date {
            year: 2026,
            month: 1,
            day: 31,
        };
        assert_eq!(parse("+1m", jan31).unwrap().render(), "<2026-02-28 Sat>");
        // …and a leap year clamps one day later.
        let jan31_leap = Date {
            year: 2028,
            month: 1,
            day: 31,
        };
        assert_eq!(
            parse("+1m", jan31_leap).unwrap().render(),
            "<2028-02-29 Tue>"
        );
    }

    #[test]
    fn years_are_twelve_months_including_across_a_leap_day() {
        assert_eq!(on("+1y").render(), "<2027-09-02 Thu>");
        let leap_day = Date {
            year: 2028,
            month: 2,
            day: 29,
        };
        assert_eq!(
            parse("+1y", leap_day).unwrap().render(),
            "<2029-02-28 Wed>",
            "the 29th clamps rather than spilling into March"
        );
    }

    #[test]
    fn the_named_days_resolve() {
        assert_eq!(on("today").render(), "<2026-09-02 Wed>");
        assert_eq!(on(".").render(), "<2026-09-02 Wed>");
        assert_eq!(on("tomorrow").render(), "<2026-09-03 Thu>");
        assert_eq!(on("yesterday").render(), "<2026-09-01 Tue>");
    }

    #[test]
    fn a_weekday_name_is_the_soonest_such_day() {
        // Today is Wednesday.
        assert_eq!(on("fri").render(), "<2026-09-04 Fri>");
        assert_eq!(on("mon").render(), "<2026-09-07 Mon>", "the coming Monday");
        assert_eq!(on("sun").render(), "<2026-09-06 Sun>");
    }

    #[test]
    fn a_weekday_name_matching_today_is_today() {
        // The documented choice. Scheduling `wed` on a Wednesday morning and
        // having it land a week away is the more surprising reading, and `+7d`
        // says the other thing unambiguously when it is wanted.
        assert_eq!(on("wed").render(), "<2026-09-02 Wed>");
        assert_eq!(
            on("+7d").render(),
            "<2026-09-09 Wed>",
            "…and this is a week"
        );
    }

    #[test]
    fn weekday_names_are_case_insensitive() {
        // A prompt is prose to the person typing into it.
        assert_eq!(on("FRI").render(), on("fri").render());
        assert_eq!(on("Tomorrow").render(), on("tomorrow").render());
    }

    #[test]
    fn a_repeater_rides_along_verbatim() {
        // Org owns what `.+1d/3d` means; this module's job is to put it back on
        // the line it came from.
        assert_eq!(on("+1d .+1d/3d").render(), "<2026-09-03 Thu .+1d/3d>");
        assert_eq!(on("2026-09-15 +1w").render(), "<2026-09-15 Tue +1w>");
        assert_eq!(on("fri ++2d").render(), "<2026-09-04 Fri ++2d>");
    }

    #[test]
    fn a_trailing_token_that_is_not_a_repeater_is_not_silently_dropped() {
        // Dropping it would turn a typo into a date the user did not ask for,
        // which is the worst outcome available to a scheduling prompt.
        let err = parse("+1d sideways", TODAY).expect_err("refused");
        assert!(err.contains("sideways"), "{err}");
    }

    #[test]
    fn a_malformed_date_names_the_forms_that_would_have_worked() {
        // A prompt that says only "invalid" makes the user guess at a grammar
        // nothing has shown them.
        for bad in ["nonsense", "2026-13-01", "2026-02-30", "+", "+1q"] {
            let err = match parse(bad, TODAY) {
                Err(e) => e,
                Ok(p) => panic!("`{bad}` should not parse, got {}", p.render()),
            };
            assert!(
                err.contains("org:"),
                "`{bad}` must fail by naming the problem: {err}"
            );
        }
        // …and the general message shows the forms that would have worked.
        let err = parse("nonsense", TODAY).expect_err("refused");
        assert!(err.contains("+1d") && err.contains("fri"), "{err}");
    }

    #[test]
    fn an_empty_answer_is_refused_here_and_handled_by_the_caller() {
        // `s` then `<CR>` REMOVES the planning line, which is the caller's
        // decision to make — this module only says there is no date in it.
        assert!(parse("", TODAY).is_err());
        assert!(parse("   ", TODAY).is_err());
    }
}

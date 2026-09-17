//! CT.8 — org's `%<fmt>`: `format-time-string` for capture templates.
//!
//! Design: `lattice/docs/dev/architecture/org-capture-templates.md` §7.
//!
//! ## Why this exists
//!
//! org-roam templates name their files with it —
//! `(file "%<%Y%m%d%H%M%S>-${slug}.org")` is the form every template in the
//! reference emacs config uses, and dailies spell `%<%Y-%m-%d>`. Matching emacs
//! means a path is written the way it is written there, not reconstructed by a
//! built-in default that a user cannot see or change.
//!
//! ## The directive set
//!
//! The strftime directives `format-time-string` shares with C, checked against
//! Python's `strftime` rather than recalled. Anything else is written back
//! VERBATIM, including the `%`, for the reason `capture::expand_with` gives for
//! its own unknown placeholders: an unexpanded `%Q` in a filename is visible and
//! fixable, while silently dropping it produces a name that looks right and is
//! not.
//!
//! No `%s` and no `%Z`: both need to know the offset this local instant was
//! shifted by, which the caller has already applied and thrown away. A wrong
//! epoch or zone in a filename is worse than a visible `%s`.

use crate::datetree::{iso_week, DAY_NAMES_FULL, MONTH_NAMES};
use crate::roam_dailies::Date;

/// A local wall-clock instant, as seconds since the epoch ALREADY shifted into
/// local time — the value `local_now_secs` produces.
///
/// A newtype rather than a bare `i64` because the expansion functions used to
/// take a day number in the same position: a bare integer would let a day be
/// read as seconds (and render as 1970) without a compile error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct When {
    pub local_secs: i64,
}

impl When {
    /// Midnight of an epoch day — what tests that only care about the date pin.
    ///
    /// Test-only: production always has the full local instant.
    #[cfg(test)]
    pub fn from_epoch_day(day: i64) -> Self {
        Self {
            local_secs: day * 86_400,
        }
    }

    /// The epoch day this instant falls in. `div_euclid` so a pre-1970 instant
    /// floors rather than truncating toward zero.
    pub fn epoch_day(&self) -> i64 {
        self.local_secs.div_euclid(86_400)
    }

    fn parts(&self) -> Parts {
        let (year, month, day) = crate::agenda::civil_from_epoch_day(self.epoch_day());
        let rem = self.local_secs.rem_euclid(86_400);
        Parts {
            date: Date { year, month, day },
            hour: (rem / 3_600) as u32,
            minute: ((rem % 3_600) / 60) as u32,
            second: (rem % 60) as u32,
        }
    }
}

struct Parts {
    date: Date,
    hour: u32,
    minute: u32,
    second: u32,
}

/// Expand the strftime directives in `fmt` for `when`.
pub fn format_time(fmt: &str, when: When) -> String {
    let p = when.parts();
    let Date { year, month, day } = p.date;
    let dow = crate::timestamp::weekday(year, month, day); // 0 = Sunday
    let month_name = MONTH_NAMES[(month.clamp(1, 12) - 1) as usize];
    let day_name = DAY_NAMES_FULL[dow];
    let hour12 = match p.hour % 12 {
        0 => 12,
        h => h,
    };

    let mut out = String::with_capacity(fmt.len() + 8);
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{year:04}")),
            Some('y') => out.push_str(&format!("{:02}", year.rem_euclid(100))),
            Some('C') => out.push_str(&format!("{:02}", year.div_euclid(100))),
            Some('m') => out.push_str(&format!("{month:02}")),
            Some('d') => out.push_str(&format!("{day:02}")),
            Some('e') => out.push_str(&format!("{day:>2}")),
            Some('H') => out.push_str(&format!("{:02}", p.hour)),
            Some('I') => out.push_str(&format!("{hour12:02}")),
            Some('M') => out.push_str(&format!("{:02}", p.minute)),
            Some('S') => out.push_str(&format!("{:02}", p.second)),
            Some('p') => out.push_str(if p.hour < 12 { "AM" } else { "PM" }),
            Some('B') => out.push_str(month_name),
            Some('b') | Some('h') => out.push_str(&month_name[..3]),
            Some('A') => out.push_str(day_name),
            Some('a') => out.push_str(&day_name[..3]),
            Some('j') => {
                let doy = crate::timestamp::epoch_day(year, month, day)
                    - crate::timestamp::epoch_day(year, 1, 1)
                    + 1;
                out.push_str(&format!("{doy:03}"));
            }
            // ISO weekday: Monday = 1 … Sunday = 7.
            Some('u') => out.push_str(&format!("{}", (dow + 6) % 7 + 1)),
            Some('w') => out.push_str(&format!("{dow}")),
            Some('G') => out.push_str(&format!("{:04}", iso_week(p.date).0)),
            Some('V') => out.push_str(&format!("{:02}", iso_week(p.date).1)),
            Some('F') => out.push_str(&format!("{year:04}-{month:02}-{day:02}")),
            Some('T') => out.push_str(&format!("{:02}:{:02}:{:02}", p.hour, p.minute, p.second)),
            Some('R') => out.push_str(&format!("{:02}:{:02}", p.hour, p.minute)),
            Some('D') => out.push_str(&format!("{month:02}/{day:02}/{:02}", year.rem_euclid(100))),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ground truth from Python's `strftime`, which shares these directives
    /// with emacs's `format-time-string`.
    fn at(local_secs: i64) -> When {
        When { local_secs }
    }

    const WED: i64 = 1_789_567_509; // 2026-09-16 14:05:09
    const NEW_YEAR: i64 = 1_798_794_184; // 2027-01-01 09:03:04
    const MIDNIGHT: i64 = 1_767_571_200; // 2026-01-05 00:00:00

    /// The form every roam template in the reference config names its file with.
    #[test]
    fn the_roam_filename_stamp() {
        assert_eq!(format_time("%Y%m%d%H%M%S", at(WED)), "20260916140509");
        assert_eq!(format_time("%Y%m%d%H%M%S", at(NEW_YEAR)), "20270101090304");
        assert_eq!(format_time("%Y%m%d%H%M%S", at(MIDNIGHT)), "20260105000000");
    }

    #[test]
    fn dates_and_names() {
        assert_eq!(format_time("%Y-%m-%d", at(WED)), "2026-09-16");
        assert_eq!(format_time("%F", at(WED)), "2026-09-16");
        assert_eq!(
            format_time("%y %b %B %a %A", at(WED)),
            "26 Sep September Wed Wednesday"
        );
        assert_eq!(format_time("%D", at(WED)), "09/16/26");
    }

    /// `%e` pads with a SPACE, not a zero — a detail a hand-written formatter
    /// gets wrong because every other day directive zero-pads.
    #[test]
    fn day_of_month_space_padding() {
        assert_eq!(format_time("%e", at(NEW_YEAR)), " 1");
        assert_eq!(format_time("%d", at(NEW_YEAR)), "01");
        assert_eq!(format_time("%e", at(WED)), "16");
    }

    /// Midnight is 12 AM on a 12-hour clock, not 00.
    #[test]
    fn twelve_hour_clock() {
        assert_eq!(format_time("%I %p", at(WED)), "02 PM");
        assert_eq!(format_time("%I %p", at(NEW_YEAR)), "09 AM");
        assert_eq!(format_time("%I %p", at(MIDNIGHT)), "12 AM");
        assert_eq!(format_time("%T", at(WED)), "14:05:09");
        assert_eq!(format_time("%R", at(MIDNIGHT)), "00:00");
    }

    #[test]
    fn day_of_year_and_weekday_numbers() {
        assert_eq!(format_time("%j", at(WED)), "259");
        assert_eq!(format_time("%j", at(NEW_YEAR)), "001");
        assert_eq!(format_time("%u %w", at(WED)), "3 3");
        assert_eq!(format_time("%u %w", at(MIDNIGHT)), "1 1");
    }

    /// The ISO year is not always the calendar year — 2027-01-01 is 2026-W53.
    #[test]
    fn iso_week_across_the_year_boundary() {
        assert_eq!(format_time("%G-W%V", at(NEW_YEAR)), "2026-W53");
        assert_eq!(format_time("%G-W%V", at(WED)), "2026-W38");
        assert_eq!(format_time("%G-W%V", at(MIDNIGHT)), "2026-W02");
    }

    /// Unknown directives stay VERBATIM, `%%` is a literal percent, and a
    /// trailing `%` survives — a visible leftover beats a silently wrong name.
    #[test]
    fn unknown_directives_are_left_visible() {
        assert_eq!(format_time("%Q-%Y", at(WED)), "%Q-2026");
        assert_eq!(format_time("100%%", at(WED)), "100%");
        assert_eq!(format_time("trail%", at(WED)), "trail%");
        assert_eq!(format_time("%s %Z", at(WED)), "%s %Z", "no epoch or zone");
    }

    #[test]
    fn a_day_pins_midnight() {
        let w = When::from_epoch_day(20_457);
        assert_eq!(w.epoch_day(), 20_457);
        assert_eq!(format_time("%H:%M:%S", w), "00:00:00");
    }
}

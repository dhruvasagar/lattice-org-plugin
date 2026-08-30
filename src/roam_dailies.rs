//! OR.10 — dailies: the journal, one file per calendar day.
//!
//! Design: `docs/dev/architecture/org-roam.md` §6 and §7 in the lattice tree.
//!
//! ## The whole feature is a filename
//!
//! `daily/YYYY-MM-DD.org` under the roam directory. There is no index, no
//! registry and no state: a date names exactly one path, so "today's journal"
//! is a pure function of the clock and two options. That is why this module is
//! arithmetic and string building with no host calls in it — the seam work
//! (does the file exist, mint an id, open it) stays in `lib.rs` where the
//! effects are, and everything here is unit-testable without an editor.
//!
//! ## Local time, not UTC
//!
//! [`crate::clock::Now`] already carries the host's `local-utc-offset-seconds`
//! shift (OC.4), and dailies take their date from it rather than from
//! `wasi:clocks` directly. A journal entry filed under yesterday because the
//! user lives east of Greenwich is the midnight-anchor bug in a different
//! costume, and it is invisible until the one night it eats an entry.
//!
//! ## Yesterday and tomorrow are epoch-day arithmetic
//!
//! Not `day - 1` with a carry: month lengths, leap years and the century rule
//! are already correct inside [`crate::timestamp::epoch_day`] and
//! [`crate::agenda::civil_from_epoch_day`], and re-deriving them here would be
//! a second implementation of the calendar that can disagree with the first.

use crate::agenda::civil_from_epoch_day;
use crate::lattice::plugin_host::config::get_option;
use crate::roam_scan;
use crate::timestamp::epoch_day;

/// The dailies directory, relative to the roam directory unless absolute.
pub const DEFAULT_DAILIES_DIRECTORY: &str = "daily";

/// A calendar day. No time: a daily is a *day*, and carrying an hour would
/// invite two files for one date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    /// Today, in the user's local time. Built from a [`crate::clock::Now`]
    /// rather than from the epoch directly, so there is exactly one place in
    /// the plugin that applies the host's UTC offset.
    pub fn from_now(now: &crate::clock::Now) -> Self {
        Self {
            year: now.year,
            month: now.month,
            day: now.day,
        }
    }

    /// `days` later — negative for earlier. Crosses months, years and leap days
    /// because the calendar lives in the epoch-day conversion, not here.
    pub fn shifted(&self, days: i64) -> Self {
        let (year, month, day) =
            civil_from_epoch_day(epoch_day(self.year, self.month, self.day) + days);
        Self { year, month, day }
    }

    /// `2026-08-30` — the note's title, and the stem of its filename. The
    /// corpus's convention and org-roam's.
    pub fn title(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `2026-08-30.org`.
    pub fn file_name(&self) -> String {
        format!("{}.org", self.title())
    }
}

/// Parse `YYYY-MM-DD`, and nothing else.
///
/// Strict on purpose. `:org-roam-dailies-goto-date` writes a file into the
/// user's journal, and a lenient parser that read `8/9` as one thing while the
/// user meant another would file the entry under a day they cannot find it on.
/// The error names the format rather than the failure, because "2026-8-9 is not
/// a date" is less useful than being told what one looks like.
///
/// Impossible dates are rejected by round-tripping through the calendar:
/// `2026-02-30` converts to 2 March, which is not what was asked for, so it
/// fails. Range-checking the day against a month-length table would be a third
/// copy of the calendar.
pub fn parse(text: &str) -> Result<Date, String> {
    let malformed = || {
        format!(
            "org-roam: `{}` is not a date — dailies take `YYYY-MM-DD`",
            text.trim()
        )
    };
    let trimmed = text.trim();
    let parts: Vec<&str> = trimmed.split('-').collect();
    let [y, m, d] = parts.as_slice() else {
        return Err(malformed());
    };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return Err(malformed());
    }
    let year: i32 = y.parse().map_err(|_| malformed())?;
    let month: u32 = m.parse().map_err(|_| malformed())?;
    let day: u32 = d.parse().map_err(|_| malformed())?;
    let date = Date { year, month, day };
    if !(1..=12).contains(&month) || day == 0 {
        return Err(malformed());
    }
    // The round-trip. A day past the end of its month lands in the next one.
    if civil_from_epoch_day(epoch_day(year, month, day)) != (year, month, day) {
        return Err(format!("org-roam: {trimmed} is not a day that exists"));
    }
    Ok(date)
}

/// The directory dailies live in, or `None` when roam is not configured.
///
/// Relative to the roam directory unless it starts with `/`, which is what the
/// option's own documentation promises. An absolute override matters because a
/// journal is the one part of a zettelkasten people commonly keep somewhere
/// else — synced, encrypted, or in a different repository.
///
/// `None` rather than a guessed root: with no `org.roam-directory` there is no
/// corpus, and inventing `./daily` would create a journal wherever the editor
/// happened to be started.
pub fn directory() -> Option<String> {
    let root = roam_scan::roam_directory()?;
    let configured = get_option("roam-dailies-directory")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_DAILIES_DIRECTORY.to_string());
    Some(if configured.starts_with('/') {
        configured.trim_end_matches('/').to_string()
    } else {
        format!(
            "{}/{}",
            root.trim_end_matches('/'),
            configured.trim_matches('/')
        )
    })
}

/// The full path of one day's journal file, or `None` when roam is not
/// configured.
pub fn path(date: &Date) -> Option<String> {
    Some(format!("{}/{}", directory()?, date.file_name()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn a_date_names_its_title_and_its_file() {
        let d = date(2026, 8, 30);
        assert_eq!(d.title(), "2026-08-30");
        assert_eq!(d.file_name(), "2026-08-30.org");
    }

    /// Single-digit months and days are zero-padded, or the files sort wrongly
    /// in every directory listing and the corpus's own convention is broken.
    #[test]
    fn months_and_days_are_zero_padded() {
        assert_eq!(date(2026, 1, 9).file_name(), "2026-01-09.org");
    }

    /// The reason `shifted` goes through the epoch rather than decrementing a
    /// field: yesterday of the first is in the previous month.
    #[test]
    fn yesterday_crosses_a_month_boundary() {
        assert_eq!(date(2026, 8, 1).shifted(-1), date(2026, 7, 31));
        assert_eq!(date(2026, 3, 1).shifted(-1), date(2026, 2, 28));
    }

    #[test]
    fn tomorrow_crosses_a_month_boundary() {
        assert_eq!(date(2026, 7, 31).shifted(1), date(2026, 8, 1));
        assert_eq!(date(2026, 2, 28).shifted(1), date(2026, 3, 1));
    }

    /// And a year boundary, which is the same arithmetic but the one users
    /// notice.
    #[test]
    fn the_new_year_is_a_day_after_the_old_one() {
        assert_eq!(date(2025, 12, 31).shifted(1), date(2026, 1, 1));
        assert_eq!(date(2026, 1, 1).shifted(-1), date(2025, 12, 31));
    }

    /// A leap day exists in 2024 and does not in 2026 — the case a
    /// month-length table gets wrong on century years and this does not.
    #[test]
    fn leap_days_are_the_calendars_business_not_ours() {
        assert_eq!(date(2024, 2, 28).shifted(1), date(2024, 2, 29));
        assert_eq!(date(2024, 2, 29).shifted(1), date(2024, 3, 1));
        assert_eq!(date(2026, 2, 28).shifted(1), date(2026, 3, 1));
        assert_eq!(date(2100, 2, 28).shifted(1), date(2100, 3, 1));
        assert_eq!(date(2000, 2, 28).shifted(1), date(2000, 2, 29));
    }

    #[test]
    fn a_well_formed_date_parses() {
        assert_eq!(parse("2026-08-30"), Ok(date(2026, 8, 30)));
        assert_eq!(parse("  2026-08-30  "), Ok(date(2026, 8, 30)));
    }

    /// Unpadded, wrong separator, and not a date at all. Each names the format
    /// rather than the failure.
    #[test]
    fn a_malformed_date_is_refused_by_naming_the_format() {
        for bad in [
            "2026-8-9",
            "2026/08/30",
            "30-08-2026",
            "today",
            "",
            "2026-08",
        ] {
            let err = parse(bad).expect_err("`{bad}` is not `YYYY-MM-DD`");
            assert!(
                err.contains("YYYY-MM-DD"),
                "the error for `{bad}` should say what a date looks like, got: {err}"
            );
        }
    }

    /// A date that is well formed but does not exist. Distinct message,
    /// because the user's mistake is different and so is the fix.
    #[test]
    fn a_day_that_does_not_exist_is_refused_separately() {
        let err = parse("2026-02-30").expect_err("February has 28 days in 2026");
        assert!(
            err.contains("not a day that exists"),
            "a well-formed impossible date should not be blamed on the format, got: {err}"
        );
        assert!(parse("2026-13-01").is_err(), "there is no thirteenth month");
        assert!(parse("2026-00-10").is_err(), "there is no zeroth month");
        assert!(parse("2026-01-00").is_err(), "there is no zeroth day");
        assert!(parse("2024-02-29").is_ok(), "2024 is a leap year");
    }

    /// The date comes from the offset-shifted clock, so a `Now` late on the
    /// 30th local is the 30th — the assertion that would fail if dailies read
    /// UTC. The offset is applied before `Now` exists; this pins that dailies
    /// read the local fields rather than recomputing from the epoch.
    #[test]
    fn today_is_the_local_day_not_the_utc_one() {
        // 2026-08-30 23:30 local, which is 2026-08-31 in UTC+2 territory.
        let now = crate::clock::Now::from_local_secs(
            epoch_day(2026, 8, 30) * 86_400 + 23 * 3_600 + 30 * 60,
        );
        assert_eq!(Date::from_now(&now), date(2026, 8, 30));

        // The same instant read as UTC by a host an hour east would be the
        // 31st — the bug this exists to prevent.
        let shifted = crate::clock::Now::from_local_secs(
            epoch_day(2026, 8, 30) * 86_400 + 23 * 3_600 + 30 * 60 + 3_600,
        );
        assert_eq!(Date::from_now(&shifted), date(2026, 8, 31));
    }
}

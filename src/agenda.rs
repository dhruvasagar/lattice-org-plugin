//! OM.A2 — what makes a headline an agenda row, and where it sorts.
//!
//! The host walks files and builds excerpts; everything org about the agenda
//! is here. Given one file's text it answers: which headlines are dated, on
//! what day, in what order, and under which date heading.
//!
//! ## Text, again, and for a third reason
//!
//! `headline.rs` and `todo.rs` both argue for line logic over the parse tree.
//! The agenda adds one the others do not have: **the host hands `scan` a
//! `string`, not a `borrow<document>` and not a tree.** There is no parse of
//! another project's file to consult — it has never been opened, and parsing
//! every org file in a project to build an agenda would be the most expensive
//! thing the plugin does.
//!
//! ## What counts as a row
//!
//! An open headline (one whose TODO keyword is not a *done* keyword) carrying
//! a date, from one of three places, in priority order:
//!
//!   1. `DEADLINE: <2026-08-25 Tue>` on the planning line,
//!   2. `SCHEDULED: <2026-08-25 Tue>` on the planning line,
//!   3. a plain **active** timestamp `<2026-08-25 Tue>` on the headline itself.
//!
//! An **inactive** stamp `[2026-08-25 Tue]` never makes a row — that is what
//! inactive means in org, and treating it as one would drag every logbook
//! entry and every `CLOSED:` line into the agenda.
//!
//! A headline with NO keyword and no date is not a row. A headline with a
//! *done* keyword is not a row, dated or not: org hides completed entries by
//! default, and an agenda that lists what you finished is a log, not a plan.
//!
//! ## The excerpt spans the planning line
//!
//! `end_line` runs to the planning line when there is one, so the agenda row
//! shows `SCHEDULED: <…>` under its headline rather than a bare title the
//! user has to jump to the source to date. This is the one place OM.A1's
//! trivial guest left `end_line == line` and the real semantics do not.

use crate::timestamp::{self, Stamp};
use crate::todo;

/// Where a row's date came from. Also its within-day order: a deadline
/// outranks a scheduled item on the same day, which outranks a bare
/// timestamp. That is org's own ordering and the reason the enum is `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Deadline,
    Scheduled,
    Timestamp,
}

impl Kind {
    fn rank(self) -> i64 {
        match self {
            Kind::Deadline => 0,
            Kind::Scheduled => 1,
            Kind::Timestamp => 2,
        }
    }
}

/// One agenda row, before it becomes a WIT `entry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// 0-based line of the headline.
    pub line: u32,
    /// 0-based last line of the row's excerpt, inclusive — the planning line
    /// when there is one, else the headline itself.
    pub end_line: u32,
    /// Days since the Unix epoch.
    pub day: i64,
    pub kind: Kind,
    /// `[#A]`, if the headline carries one.
    pub priority: Option<char>,
}

/// The keyword sets a scan reads headlines against. Captured once per scan
/// from `org.todo-keywords`, because `:set` mid-scan changing what counts as
/// done halfway through a project is worse than a stale answer for one run.
#[derive(Debug, Clone)]
pub struct Keywords {
    pub all: Vec<String>,
    pub done: Vec<String>,
}

impl Keywords {
    pub fn from_spec(spec: &str) -> Self {
        let (not_done, done) = todo::split_keywords(spec);
        let mut all = not_done;
        all.extend(done.iter().cloned());
        Self { all, done }
    }

    fn is_done(&self, keyword: Option<&str>) -> bool {
        keyword.is_some_and(|k| self.done.iter().any(|d| d == k))
    }
}

/// Every agenda row in one file's text.
pub fn scan_file(text: &str, keywords: &Keywords) -> Vec<Row> {
    let lines: Vec<&str> = text.lines().collect();
    let mut rows = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(headline) = todo::parse(line, &keywords.all) else {
            continue;
        };
        if keywords.is_done(headline.keyword) {
            continue;
        }
        // The planning line is the one immediately below the headline, and
        // ONLY that one. Scanning further would let a `SCHEDULED:` written
        // inside the body — or worse, one belonging to a child headline whose
        // stars have not been reached yet — date the wrong entry.
        let planning = lines.get(i + 1).copied().unwrap_or("");
        let Some((kind, stamp, spans_planning)) = date_for(line, planning) else {
            continue;
        };
        rows.push(Row {
            line: i as u32,
            end_line: if spans_planning {
                i as u32 + 1
            } else {
                i as u32
            },
            day: timestamp::epoch_day(stamp.year, stamp.month, stamp.day),
            kind,
            priority: headline.priority,
        });
    }
    rows
}

/// The date a headline carries, and whether it came from the planning line.
fn date_for(headline: &str, planning: &str) -> Option<(Kind, Stamp, bool)> {
    let trimmed = planning.trim_start();
    // DEADLINE first: a headline with both is a deadline that happens to be
    // scheduled, and org sorts it as the deadline.
    for (keyword, kind) in [
        ("DEADLINE:", Kind::Deadline),
        ("SCHEDULED:", Kind::Scheduled),
    ] {
        if let Some(rest) = trimmed.strip_prefix(keyword) {
            if let Some(stamp) = timestamp::first_stamp(rest) {
                if stamp.active {
                    return Some((kind, stamp, true));
                }
            }
        }
    }
    // `CLOSED: [2026-08-20 Thu]` also lives on the planning line and is
    // deliberately not reachable here: it is inactive, and it is a record of
    // the past rather than a plan.
    let stamp = timestamp::first_stamp(headline)?;
    stamp.active.then_some((Kind::Timestamp, stamp, false))
}

/// The grouping KEY the host compares after its cross-file sort: the ISO
/// date, so rows from different files on the same day render under one
/// header.
///
/// Deliberately not the *label* — the host titles the first row of each run
/// with `label`, and a key that read "Today" would silently merge two
/// different days if the scan straddled midnight.
pub fn group_key(day: i64) -> String {
    let (y, m, d) = civil_from_epoch_day(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The header a date group renders under: `2026-08-25 Tue`, annotated
/// relative to the day the scan began.
///
/// `today` is the scan's anchor, captured once in `begin` — so a scan that
/// crosses midnight labels every row against one day rather than two, which
/// is the whole reason `begin` exists.
pub fn group_label(day: i64, today: i64) -> String {
    let (y, m, d) = civil_from_epoch_day(day);
    let name = timestamp::DAY_NAMES[timestamp::weekday(y, m, d)];
    let base = format!("{y:04}-{m:02}-{d:02} {name}");
    match day - today {
        0 => format!("{base} (today)"),
        // Past. Org's agenda surfaces these hardest, because a missed
        // deadline the view stays quiet about is the failure the tool exists
        // to prevent.
        n if n < 0 => format!("{base} (overdue by {} day(s))", -n),
        1 => format!("{base} (tomorrow)"),
        n => format!("{base} (in {n} day(s))"),
    }
}

/// The `sort-key` the host stable-sorts every file's rows on.
///
/// Day dominates; within a day, kind; within a kind, priority. Packed into
/// one `i64` because the ABI carries exactly one number — and packed with a
/// wide multiplier so a future tiebreaker has room rather than needing an
/// ABI change to add one.
///
/// `A` sorts before `B` before "no priority", which is org's order: an
/// unprioritised item is not urgent, it is unranked.
pub fn sort_key(row: &Row) -> i64 {
    let priority = match row.priority {
        Some(c) if c.is_ascii_alphabetic() => (c.to_ascii_uppercase() as i64) - ('A' as i64),
        _ => 100,
    };
    row.day * 10_000 + row.kind.rank() * 1_000 + priority
}

/// Inverse of [`timestamp::epoch_day`] — Hinnant's `civil_from_days`.
pub fn civil_from_epoch_day(day: i64) -> (i32, u32, u32) {
    let z = day + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    ((y + i64::from(m <= 2)) as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keywords() -> Keywords {
        Keywords::from_spec("TODO NEXT | DONE CANCELLED")
    }

    fn scan(text: &str) -> Vec<Row> {
        scan_file(text, &keywords())
    }

    // ── the calendar ────────────────────────────────────────────────────

    /// The epoch itself, plus the two dates that catch a broken leap rule:
    /// 2000 IS a leap year (divisible by 400) and 1900 is NOT.
    #[test]
    fn epoch_day_round_trips_across_the_awkward_dates() {
        for (y, m, d) in [
            (1970, 1, 1),
            (1900, 2, 28),
            (1900, 3, 1),
            (2000, 2, 29),
            (2026, 8, 25),
            (2100, 3, 1),
        ] {
            let day = timestamp::epoch_day(y, m, d);
            assert_eq!(civil_from_epoch_day(day), (y, m, d), "{y}-{m}-{d}");
        }
        assert_eq!(timestamp::epoch_day(1970, 1, 1), 0);
    }

    /// A month boundary must not reorder. This is the assertion a
    /// `(month, day)` sort key would fail, and the reason the ABI carries an
    /// epoch day.
    #[test]
    fn epoch_days_order_across_a_year_boundary() {
        let dec = timestamp::epoch_day(2026, 12, 31);
        let jan = timestamp::epoch_day(2027, 1, 1);
        assert!(dec < jan);
        assert_eq!(jan - dec, 1);
    }

    // ── what counts as a row ────────────────────────────────────────────

    #[test]
    fn a_scheduled_headline_is_a_row_spanning_its_planning_line() {
        let rows = scan("* TODO Ship it\n  SCHEDULED: <2026-08-25 Tue>\nbody\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].line, 0);
        assert_eq!(
            rows[0].end_line, 1,
            "the excerpt shows the date, not just the title"
        );
        assert_eq!(rows[0].kind, Kind::Scheduled);
        assert_eq!(rows[0].day, timestamp::epoch_day(2026, 8, 25));
    }

    /// Both present: org sorts it as the deadline, so the deadline wins.
    #[test]
    fn a_deadline_outranks_a_scheduled_date_on_the_same_headline() {
        let rows =
            scan("* TODO Ship it\n  DEADLINE: <2026-08-20 Thu> SCHEDULED: <2026-08-25 Tue>\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, Kind::Deadline);
        assert_eq!(rows[0].day, timestamp::epoch_day(2026, 8, 20));
    }

    #[test]
    fn an_active_timestamp_on_the_headline_is_a_row() {
        let rows = scan("* Meeting <2026-08-25 Tue 10:30>\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, Kind::Timestamp);
        assert_eq!(
            rows[0].end_line, 0,
            "no planning line, so the excerpt is the headline alone"
        );
    }

    /// The distinction the whole inactive-stamp syntax exists for. Counting
    /// these would drag every logbook line and every `CLOSED:` into the view.
    #[test]
    fn an_inactive_timestamp_is_never_a_row() {
        assert!(scan("* Meeting [2026-08-25 Tue]\n").is_empty());
        assert!(scan("* TODO Ship it\n  CLOSED: [2026-08-20 Thu]\n").is_empty());
    }

    /// An agenda that lists what you finished is a log, not a plan.
    #[test]
    fn a_done_headline_is_not_a_row_however_dated() {
        assert!(scan("* DONE Ship it\n  SCHEDULED: <2026-08-25 Tue>\n").is_empty());
        assert!(scan("* CANCELLED Ship it\n  DEADLINE: <2026-08-25 Tue>\n").is_empty());
        // …and the open peer of the same shape still is, so the test above is
        // about doneness and not about the parse failing.
        assert_eq!(
            scan("* NEXT Ship it\n  SCHEDULED: <2026-08-25 Tue>\n").len(),
            1
        );
    }

    #[test]
    fn an_undated_headline_is_not_a_row() {
        assert!(scan("* TODO Ship it\nbody\n* Another\n").is_empty());
    }

    /// Only the line immediately below. A `SCHEDULED:` deeper in the body
    /// belongs to nothing, and one below a CHILD headline belongs to the
    /// child — dating the parent with it would put the wrong entry in the
    /// agenda and jump the user to the wrong line.
    #[test]
    fn only_the_line_below_the_headline_is_a_planning_line() {
        let rows = scan("* TODO Parent\n\n  SCHEDULED: <2026-08-25 Tue>\n");
        assert!(
            rows.is_empty(),
            "a blank line ends the planning position, got {rows:?}"
        );

        let rows = scan("* TODO Parent\n** TODO Child\n  SCHEDULED: <2026-08-25 Tue>\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].line, 1, "the CHILD is the dated row");
    }

    /// A file with no headlines at all is empty, not an error — the scan
    /// stays silent so a project of ordinary org notes does not fill the log.
    #[test]
    fn a_file_with_no_headlines_yields_no_rows() {
        assert!(scan("#+TITLE: notes\njust prose\n").is_empty());
    }

    // ── ordering ────────────────────────────────────────────────────────

    #[test]
    fn day_dominates_kind_dominates_priority() {
        let row = |day, kind, priority| Row {
            line: 0,
            end_line: 0,
            day,
            kind,
            priority,
        };
        let d0 = timestamp::epoch_day(2026, 8, 25);
        let d1 = timestamp::epoch_day(2026, 8, 26);

        // A low-priority deadline TODAY still beats a high-priority one
        // tomorrow: the agenda is a calendar first.
        assert!(
            sort_key(&row(d0, Kind::Timestamp, None))
                < sort_key(&row(d1, Kind::Deadline, Some('A')))
        );
        // Within a day, kind.
        assert!(
            sort_key(&row(d0, Kind::Deadline, None))
                < sort_key(&row(d0, Kind::Scheduled, Some('A')))
        );
        // Within a kind, priority — and unprioritised sorts last, because it
        // is unranked rather than urgent.
        assert!(
            sort_key(&row(d0, Kind::Scheduled, Some('A')))
                < sort_key(&row(d0, Kind::Scheduled, Some('B')))
        );
        assert!(
            sort_key(&row(d0, Kind::Scheduled, Some('B')))
                < sort_key(&row(d0, Kind::Scheduled, None))
        );
    }

    /// Negative days (pre-1970) must not break the packing. `day * 10_000`
    /// with a positive kind/priority offset stays monotone only if the
    /// offsets are smaller than the multiplier — this is that assertion.
    #[test]
    fn sort_keys_stay_ordered_for_dates_before_the_epoch() {
        let row = |y, m, d| Row {
            line: 0,
            end_line: 0,
            day: timestamp::epoch_day(y, m, d),
            kind: Kind::Timestamp,
            priority: None,
        };
        assert!(sort_key(&row(1969, 12, 31)) < sort_key(&row(1970, 1, 1)));
        assert!(sort_key(&row(1900, 1, 1)) < sort_key(&row(1969, 12, 31)));
    }

    // ── grouping ────────────────────────────────────────────────────────

    /// The key is the date and nothing else, so two rows on one day group
    /// however far apart their files were in the walk.
    #[test]
    fn the_group_key_is_the_date_alone() {
        let day = timestamp::epoch_day(2026, 8, 25);
        assert_eq!(group_key(day), "2026-08-25");
        assert_eq!(group_key(day), group_key(day));
        assert_ne!(group_key(day), group_key(day + 1));
    }

    /// The label is relative to the scan's anchor. A key that read "Today"
    /// would merge two different days if a scan straddled midnight, which is
    /// why the key and the label are separate functions.
    #[test]
    fn the_label_is_relative_to_the_scans_anchor() {
        let today = timestamp::epoch_day(2026, 8, 25);
        assert_eq!(group_label(today, today), "2026-08-25 Tue (today)");
        assert_eq!(group_label(today + 1, today), "2026-08-26 Wed (tomorrow)");
        assert_eq!(
            group_label(today + 4, today),
            "2026-08-29 Sat (in 4 day(s))"
        );
        assert_eq!(
            group_label(today - 3, today),
            "2026-08-22 Sat (overdue by 3 day(s))"
        );
    }

    /// Keyword sets are user configuration, so a spec with no `|` has no done
    /// states — three open ones. Defaulting the last word to "done" would
    /// silently hide the user's final state.
    #[test]
    fn a_keyword_spec_without_a_bar_has_no_done_states() {
        let k = Keywords::from_spec("PROPOSED ACCEPTED SHIPPED");
        assert!(!k.is_done(Some("SHIPPED")));
        assert_eq!(
            scan_file("* SHIPPED It\n  SCHEDULED: <2026-08-25 Tue>\n", &k).len(),
            1
        );
    }
}

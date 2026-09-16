//! CT.6 — `file+datetree`: today's date node, created if it is not there.
//!
//! Design: `lattice/docs/dev/architecture/org-capture-templates.md` §4.2.
//!
//! ## The shape is org's, measured rather than recalled
//!
//! `org-datetree.el:138-158` builds three levels and puts a weekday name on the
//! day:
//!
//! ```org
//! * 2026
//! ** 2026-09 September
//! *** 2026-09-16 Wednesday
//! ```
//!
//! The formats are `%Y` / `%Y-%m %B` / `%Y-%m-%d %A`, and `%G-W%V` for a week
//! tree. They are reproduced here rather than approximated, because a file
//! written by this and a file written by emacs have to be the same file — a
//! user running both against one tracker must not get two parallel trees.
//!
//! ## Creation is ORDERED, not appended
//!
//! A new date is inserted among its siblings in date order. Appending would
//! work for the common case — today is usually the latest date — and would put
//! the tree permanently out of order the first time someone captures to
//! yesterday, or opens a file written on a later machine. The labels sort
//! lexicographically because every one of them is zero-padded, which is why
//! the format strings matter beyond cosmetics.
//!
//! ## Partial creation is the normal case
//!
//! On the first capture of a month, the year exists and the month does not; on
//! the first of a year, neither does. The walk descends as far as it can and
//! creates only the rest, so a file never grows a second `* 2026`.

use crate::roam_dailies::Date;

/// Which grouping the tree uses — org's `:tree-type`.
///
/// A schema `Enum`: `day` / `week` / `month` carry no `+`, so the derive's
/// kebab-casing gives org's own spellings and the host validates the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, lattice_plugin_sdk::ConfigShape)]
pub enum TreeType {
    /// `* 2026` / `** 2026-09 September` / `*** 2026-09-16 Wednesday`.
    #[default]
    Day,
    /// `* 2026` / `** 2026-W38`.
    Week,
    /// `* 2026` / `** 2026-09 September`.
    Month,
}

/// Full month names — org's `%B`. [`crate::timestamp::DAY_NAMES`] is
/// deliberately abbreviated (`Wed`) because org TIMESTAMPS use `%a`; a datetree
/// heading uses `%A`, so the full names live here rather than widening a
/// constant whose other caller wants the short form.
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Full weekday names — org's `%A`.
const DAY_NAMES_FULL: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Today, from the host clock — the one place the datetree asks what day it is.
///
/// Through [`crate::roam_dailies::Date::from_now`] so there is exactly one
/// place in the plugin that applies the host's UTC offset, and so a test can
/// pin a date by building a `Now` rather than by mocking a clock.
pub fn today(now: &crate::clock::Now) -> Date {
    Date::from_now(now)
}

/// [`today`] against the host clock.
///
/// The only place the datetree asks the host what day it is, and it lives on
/// the production side of `capture_effects`'s seam: calling it from inside
/// `capture_effects_via` put a host import back into the one function carved to
/// have none, which a unit test discovers as `entered unreachable code` from
/// the `wit_bindgen` macro rather than as anything resembling a clock problem.
pub fn today_from_host() -> Date {
    today(&crate::clock_now())
}

/// The headings for a date, outermost first.
///
/// Three for a day tree, two for week and month — org's own arity, so
/// `:tree-type week` produces `* 2026` / `** 2026-W38` and nothing deeper.
pub fn labels(date: Date, tree_type: TreeType) -> Vec<String> {
    let year = format!("{}", date.year);
    match tree_type {
        TreeType::Month => vec![year, month_label(date)],
        TreeType::Week => vec![year, week_label(date)],
        TreeType::Day => vec![year, month_label(date), day_label(date)],
    }
}

fn month_label(date: Date) -> String {
    let name = MONTH_NAMES[(date.month.clamp(1, 12) - 1) as usize];
    format!("{}-{:02} {name}", date.year, date.month)
}

fn day_label(date: Date) -> String {
    let dow = DAY_NAMES_FULL[crate::timestamp::weekday(date.year, date.month, date.day)];
    format!("{}-{:02}-{:02} {dow}", date.year, date.month, date.day)
}

/// ISO week — org's `%G-W%V`.
///
/// The ISO year is not always the calendar year: 2027-01-01 is a Friday and
/// belongs to week 53 of 2026. Getting that wrong would put one week of
/// captures under the wrong year heading every few years, which is exactly the
/// kind of error nobody notices until they go looking for a note.
fn week_label(date: Date) -> String {
    let (iso_year, week) = iso_week(date);
    format!("{iso_year}-W{week:02}")
}

/// `(ISO year, ISO week)` for a date.
fn iso_week(date: Date) -> (i32, u32) {
    // Thursday of this date's week decides the ISO year — that is the rule the
    // standard is written in terms of.
    let dow = crate::timestamp::weekday(date.year, date.month, date.day);
    // Shift so Monday = 0.
    let iso_dow = (dow + 6) % 7;
    let thursday = date.shifted(3 - iso_dow as i64);
    let jan1 = Date {
        year: thursday.year,
        month: 1,
        day: 1,
    };
    let days = epoch_day(thursday) - epoch_day(jan1);
    ((thursday.year), (days / 7 + 1) as u32)
}

/// Days since the epoch, for the week arithmetic above.
fn epoch_day(date: Date) -> i64 {
    // Howard Hinnant's civil-from-days, inverted — the same conversion
    // `roam_dailies::Date::shifted` relies on, expressed once here because the
    // ISO-week rule needs a difference rather than an offset.
    let y = if date.month <= 2 {
        date.year - 1
    } else {
        date.year
    };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let m = date.month as i64;
    let d = date.day as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146_097 + doe - 719_468
}

/// Where a capture files in the datetree, and what must be created first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatetreeSpot {
    /// The 0-based line to write before.
    pub line: u32,
    /// Heading lines to create, outermost first. Empty when the node exists.
    pub create: Vec<String>,
    /// The date node's level — what a body re-levels against (CT.5).
    pub level: usize,
}

/// Resolve the datetree node for `labels`, planning creation of whatever is
/// missing.
///
/// `scope` is the line range to search and `base_level` the level the outermost
/// label sits at minus one — both are `(0, lines.len())` and `0` for a plain
/// `file+datetree`, and the enclosing node's body and level when an `olp` puts
/// the tree underneath something (org's `file+olp+datetree`).
pub fn resolve(
    lines: &[&str],
    outline: &[crate::headline::Entry],
    scope: (u32, u32),
    base_level: usize,
    labels: &[String],
) -> DatetreeSpot {
    let (mut lo, mut hi) = scope;
    let mut level = base_level;
    let mut found_end: Option<u32> = None;

    for (depth, label) in labels.iter().enumerate() {
        let want_level = base_level + depth + 1;
        let hit = outline.iter().find(|e| {
            e.line >= lo
                && e.line < hi
                && e.level == want_level
                && heading_title(lines, e).as_deref() == Some(label.as_str())
        });
        match hit {
            Some(entry) => {
                lo = entry.line + 1;
                hi = entry.end_line + 1;
                level = entry.level;
                found_end = Some(entry.end_line);
            }
            None => {
                // Everything from here down is missing. Create it, in date
                // order among whatever siblings are already at this level.
                let at = ordered_slot(lines, outline, (lo, hi), want_level, label);
                let create = labels[depth..]
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!("{} {l}", "*".repeat(want_level + i)))
                    .collect();
                return DatetreeSpot {
                    line: at,
                    create,
                    level: base_level + labels.len(),
                };
            }
        }
    }

    DatetreeSpot {
        line: found_end.map(|e| e + 1).unwrap_or(hi),
        create: Vec::new(),
        level,
    }
}

/// Where a new `label` belongs among the headings at `level` in `scope`.
///
/// The first sibling that sorts AFTER it, or the end of the scope. Lexical
/// order is date order here because every label is zero-padded.
fn ordered_slot(
    lines: &[&str],
    outline: &[crate::headline::Entry],
    scope: (u32, u32),
    level: usize,
    label: &str,
) -> u32 {
    let (lo, hi) = scope;
    let later = outline
        .iter()
        .filter(|e| e.line >= lo && e.line < hi && e.level == level)
        .find(|e| heading_title(lines, e).is_some_and(|t| t.as_str() > label));
    match later {
        Some(entry) => entry.line,
        None => hi,
    }
}

/// A heading's text with its stars and surrounding space removed.
///
/// No TODO/tag stripping, unlike `capture_target`'s matcher: a datetree heading
/// is written by org (or by this), never hand-typed, so an exact comparison is
/// what keeps a second `* 2026` from appearing beside the first.
fn heading_title(lines: &[&str], entry: &crate::headline::Entry) -> Option<String> {
    lines
        .get(entry.line as usize)
        .and_then(|l| l.get(entry.level..))
        .map(|rest| rest.trim().to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn d(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    fn outline_of(text: &str) -> (Vec<&str>, Vec<crate::headline::Entry>) {
        let lines: Vec<&str> = text.lines().collect();
        let outline = crate::headline::outline_text(&lines);
        (lines, outline)
    }

    /// Org's three formats, verbatim. A file this writes and a file emacs
    /// writes have to be the SAME file — a user running both against one
    /// tracker must not end up with two parallel trees.
    #[test]
    fn the_labels_are_orgs_own_formats() {
        assert_eq!(
            labels(d(2026, 9, 16), TreeType::Day),
            vec!["2026", "2026-09 September", "2026-09-16 Wednesday"]
        );
        assert_eq!(
            labels(d(2026, 9, 16), TreeType::Month),
            vec!["2026", "2026-09 September"]
        );
        assert_eq!(
            labels(d(2026, 9, 16), TreeType::Week),
            vec!["2026", "2026-W38"]
        );
    }

    /// Zero-padded, because the ORDERING depends on it — `2026-9` would sort
    /// after `2026-10`, putting September below October forever.
    #[test]
    fn single_digit_months_and_days_are_padded() {
        assert_eq!(
            labels(d(2026, 1, 5), TreeType::Day),
            vec!["2026", "2026-01 January", "2026-01-05 Monday"]
        );
    }

    /// The ISO year is not always the calendar year, and both directions
    /// happen. Ground truth from Python's `date.isocalendar()`.
    #[test]
    fn the_iso_week_year_is_not_the_calendar_year() {
        // A January date belonging to the PREVIOUS ISO year.
        assert_eq!(week_label(d(2027, 1, 1)), "2026-W53");
        // A December date belonging to the NEXT ISO year.
        assert_eq!(week_label(d(2025, 12, 29)), "2026-W01");
        // And the ordinary cases either side of them.
        assert_eq!(week_label(d(2026, 1, 1)), "2026-W01");
        assert_eq!(week_label(d(2026, 12, 31)), "2026-W53");
        assert_eq!(week_label(d(2026, 9, 16)), "2026-W38");
        assert_eq!(week_label(d(2024, 2, 29)), "2024-W09");
    }

    /// An empty file grows the whole tree.
    #[test]
    fn an_empty_file_gets_every_level() {
        let (lines, outline) = outline_of("");
        let got = resolve(
            &lines,
            &outline,
            (0, 0),
            0,
            &labels(d(2026, 9, 16), TreeType::Day),
        );
        assert_eq!(
            got.create,
            vec!["* 2026", "** 2026-09 September", "*** 2026-09-16 Wednesday"]
        );
        assert_eq!(got.level, 3, "the day node is level 3, as in org");
    }

    /// An existing day node is REUSED, not duplicated — the ordinary second
    /// capture of a day.
    #[test]
    fn an_existing_day_is_reused() {
        let text = "* 2026\n** 2026-09 September\n*** 2026-09-16 Wednesday\nnote\n";
        let (lines, outline) = outline_of(text);
        let got = resolve(
            &lines,
            &outline,
            (0, lines.len() as u32),
            0,
            &labels(d(2026, 9, 16), TreeType::Day),
        );
        assert!(got.create.is_empty(), "nothing to create: {:?}", got.create);
        assert_eq!(got.line, 4, "after the day's body");
        assert_eq!(got.level, 3);
    }

    /// Partial creation — the year exists, the month does not. This is the
    /// first capture of any month, so it is the common path, not an edge.
    #[test]
    fn an_existing_year_gains_only_the_missing_levels() {
        let text = "* 2026\n** 2026-08 August\n*** 2026-08-30 Sunday\n";
        let (lines, outline) = outline_of(text);
        let got = resolve(
            &lines,
            &outline,
            (0, lines.len() as u32),
            0,
            &labels(d(2026, 9, 16), TreeType::Day),
        );
        assert_eq!(
            got.create,
            vec!["** 2026-09 September", "*** 2026-09-16 Wednesday"],
            "no second `* 2026`"
        );
    }

    /// A new date sorts AMONG its siblings rather than appending. Appending
    /// works while today is always the latest date and puts the tree
    /// permanently out of order the first time it is not.
    #[test]
    fn a_new_date_is_inserted_in_order() {
        let text = "* 2026\n** 2026-08 August\n** 2026-10 October\n";
        let (lines, outline) = outline_of(text);
        let got = resolve(
            &lines,
            &outline,
            (0, lines.len() as u32),
            0,
            &labels(d(2026, 9, 16), TreeType::Day),
        );
        assert_eq!(
            got.line, 2,
            "before `** 2026-10 October`, not at the end of the file"
        );
        assert_eq!(
            got.create,
            vec!["** 2026-09 September", "*** 2026-09-16 Wednesday"]
        );
    }

    /// And a date later than everything present still goes at the end.
    #[test]
    fn a_later_date_appends_within_its_parent() {
        let text = "* 2026\n** 2026-08 August\n* 2027\n";
        let (lines, outline) = outline_of(text);
        let got = resolve(
            &lines,
            &outline,
            (0, lines.len() as u32),
            0,
            &labels(d(2026, 9, 16), TreeType::Day),
        );
        assert_eq!(
            got.line, 2,
            "at the end of 2026's subtree, BEFORE `* 2027` — not at EOF"
        );
    }

    /// A day inserted among existing days of the same month.
    #[test]
    fn a_new_day_sorts_among_its_month() {
        let text = "\
* 2026
** 2026-09 September
*** 2026-09-10 Thursday
*** 2026-09-20 Sunday
";
        let (lines, outline) = outline_of(text);
        let got = resolve(
            &lines,
            &outline,
            (0, lines.len() as u32),
            0,
            &labels(d(2026, 9, 16), TreeType::Day),
        );
        assert_eq!(got.line, 3, "before the 20th");
        assert_eq!(got.create, vec!["*** 2026-09-16 Wednesday"]);
    }
}

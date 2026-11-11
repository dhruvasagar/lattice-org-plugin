//! AT.7 — org's tools for the assistant, the `llm-tools` seam.
//!
//! Design: `docs/dev/architecture/llm-assistant-tools.md` §6 (in the lattice
//! repo). Only what needs org's parser lives here; free-text search, reading
//! a file and the web are lattice's own tools.
//!
//! **The tools read what the agenda reads.** `org.agenda` runs the agenda's
//! own scan and [`agenda::occurrences`], so the assistant cannot answer "what
//! is on Thursday" differently from `:org-agenda`, repeats included.
//!
//! **Every answer opens with today.** A model must never infer the date, and
//! a small one asked "what's due this week" will happily assume its training
//! cut-off is the present.
//!
//! **Rows carry a ref**, `path::line::heading` (1-based line, `~` for home),
//! which `org.entry` and `org.change` take.
//!
//! This module is pure: the guest (`lib.rs`) gathers the files through
//! `host-services` and hands them in as [`OrgFile`]s, so every answer here is
//! testable on the host without one.

use lattice_plugin_sdk::llm::{input_schema, parse_args};
use lattice_plugin_sdk::ConfigShape;

use crate::agenda::{self, Filter, Keywords, Kind, Row, Section, When};
use crate::repeat::{Base, Repeater, Unit};
use crate::timestamp;
use crate::todo;

/// `org.agenda`'s name within the plugin (the assistant sees `org.agenda`).
pub const AGENDA: &str = "agenda";

/// The most rows one answer lists; the rest are counted.
const MAX_ROWS: usize = 150;
/// The widest window `from`/`to` may span.
const MAX_DAYS: i64 = 366;

/// A tool as the WIT lists it, before the guest maps it to the generated type.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: String,
}

/// The tools org offers, all `read`.
pub fn tools() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: AGENDA,
        description: "List what is scheduled, due and overdue in the user's org files for a day, \
                      a week or a date range, optionally only entries matching tags or a TODO \
                      state. Use it for questions about plans, deadlines and what is on a given \
                      day.",
        input_schema: input_schema::<AgendaArgs>().unwrap_or_default(),
    }]
}

/// One org file, read, with its agenda rows.
pub struct OrgFile {
    /// As a ref spells it: `~` for the home directory.
    pub shown: String,
    pub text: String,
    pub rows: Vec<Row>,
}

/// How far `org.agenda` looks from `from`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ConfigShape)]
pub enum Range {
    Day,
    Week,
}

/// `org.agenda`'s input.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShape)]
pub struct AgendaArgs {
    /// `day` (the default) or `week`, counted from `from`.
    pub range: Option<Range>,
    /// The first day, `YYYY-MM-DD`. Today when left out.
    pub from: Option<String>,
    /// The last day, `YYYY-MM-DD`, for any other range. Takes the place of `range`.
    pub to: Option<String>,
    /// Only entries matching this, in org's match syntax: `+work`, `+work-meeting`, `/TODO`,
    /// `+home/WAITING`, `STYLE="habit"`.
    pub r#match: Option<String>,
}

/// `org.agenda`: the text of its answer, or the error the model sees.
pub fn agenda(
    files: &[OrgFile],
    keywords: &Keywords,
    today: i64,
    args: &str,
) -> Result<String, String> {
    let args: AgendaArgs = parse_args(args).map_err(|e| e.to_string())?;
    let from = match &args.from {
        Some(s) => parse_day(s).map_err(|e| format!("from: {e}"))?,
        None => today,
    };
    let to = match &args.to {
        Some(s) => {
            let to = parse_day(s).map_err(|e| format!("to: {e}"))?;
            if to < from {
                return Err("to: before from".to_string());
            }
            if to - from >= MAX_DAYS {
                return Err(format!("to: at most {MAX_DAYS} days from from"));
            }
            to
        }
        None => match args.range.unwrap_or(Range::Day) {
            Range::Day => from,
            Range::Week => from + 6,
        },
    };
    let matcher = match args.r#match.as_deref().map(str::trim) {
        Some(m) if !m.is_empty() => {
            Some(crate::agenda_match::parse(m).map_err(|e| format!("match: {}", e.message()))?)
        }
        _ => None,
    };
    let section = |title: &str, when: When, todo_only: bool| Section {
        title: title.to_string(),
        filter: Filter {
            when,
            todo_only,
            min_priority: None,
            r#match: matcher.clone(),
        },
    };
    // Overdue only when the window starts today: "overdue" means before the
    // day the sections are anchored to, which is `from`.
    let mut sections = vec![section("Undated", When::Undated, true)];
    let overdue = from == today;
    if overdue {
        sections.push(section("Overdue", When::Overdue, true));
    }
    sections.push(section("Agenda", When::Days((to - from) as u32), false));
    let dated_rank = sections.len() - 1;

    let mut undated = 0usize;
    let mut late: Vec<Item> = Vec::new();
    let mut dated: Vec<Item> = Vec::new();
    for file in files {
        let lines: Vec<&str> = file.text.lines().collect();
        for row in &file.rows {
            for (rank, at) in agenda::occurrences(row, &sections, keywords, from) {
                if rank == 0 {
                    undated += 1;
                    continue;
                }
                let item = Item {
                    sort: agenda::sort_key_in_section(&at, rank as i64, from),
                    line: describe(&at, &lines, file, keywords, today),
                    day: at.date.map(|d| d.day).unwrap_or(today),
                };
                if rank == dated_rank {
                    dated.push(item);
                } else {
                    late.push(item);
                }
            }
        }
    }
    late.sort_by_key(|i| i.sort);
    dated.sort_by_key(|i| i.sort);

    let mut out = format!("Today is {}.\n", day_name(today));
    let window = if from == to {
        day_name(from)
    } else {
        format!("{} to {}", day_name(from), day_name(to))
    };
    out.push_str(&format!("Agenda for {window}"));
    if let Some(m) = args.r#match.as_deref().filter(|m| !m.trim().is_empty()) {
        out.push_str(&format!(", entries matching {}", m.trim()));
    }
    out.push_str(":\n");
    if late.is_empty() && dated.is_empty() {
        out.push_str("\nNothing is scheduled, due or overdue then.\n");
    }
    let mut listed = 0usize;
    let mut more = 0usize;
    let mut push = |out: &mut String, line: &str| {
        if listed < MAX_ROWS {
            out.push_str(line);
            out.push('\n');
            listed += 1;
        } else {
            more += 1;
        }
    };
    if !late.is_empty() {
        out.push_str("\nOverdue:\n");
        for item in &late {
            push(&mut out, &item.line);
        }
    }
    let mut day = None;
    for item in &dated {
        if day != Some(item.day) {
            day = Some(item.day);
            out.push_str(&format!("\n{}:\n", agenda::group_label(item.day, today)));
        }
        push(&mut out, &item.line);
    }
    if more > 0 {
        out.push_str(&format!(
            "\n{more} more not listed; ask for a shorter range or a narrower match.\n"
        ));
    }
    if undated > 0 {
        out.push_str(&format!(
            "\n{undated} TODO entr{} no date; org.query with match \"/TODO\" lists them.\n",
            if undated == 1 { "y has" } else { "ies have" }
        ));
    }
    Ok(out.trim_end().to_string())
}

struct Item {
    sort: i64,
    day: i64,
    line: String,
}

/// One row as a line: time, keyword, priority, title, tags, what dates it,
/// and its ref.
fn describe(row: &Row, lines: &[&str], file: &OrgFile, keywords: &Keywords, today: i64) -> String {
    let text = lines.get(row.line as usize).copied().unwrap_or("");
    let title = title_of(text, keywords);
    let mut line = String::from("- ");
    if let Some(time) = row.date.as_ref().and_then(agenda::time_label) {
        line.push_str(&time);
        line.push(' ');
    }
    if let Some(k) = &row.keyword {
        line.push_str(k);
        line.push(' ');
    }
    if let Some(p) = row.priority {
        line.push_str(&format!("[#{p}] "));
    }
    line.push_str(&title);
    if !row.tags.is_empty() {
        line.push_str(&format!(" :{}:", row.tags.join(":")));
    }
    if let Some(d) = row.date {
        let kind = match d.kind {
            Kind::Deadline => "deadline",
            Kind::Scheduled => "scheduled",
            Kind::Timestamp => "event",
        };
        let mut note = kind.to_string();
        if d.day < today {
            let ago = today - d.day;
            note.push_str(&format!(
                " {}, {ago} day{} ago",
                iso(d.day),
                if ago == 1 { "" } else { "s" }
            ));
        }
        if let Some(r) = d.repeater {
            note.push_str(&format!(", repeats {}", repeater_text(&r)));
        }
        line.push_str(&format!(" ({note})"));
    }
    line.push_str(&format!(
        " [ref {}]",
        reference(&file.shown, row.line, &title)
    ));
    line
}

/// A headline's title as rows and refs show it: keyword, priority and tags
/// off, and any timestamp written in it removed, since the row states its
/// date on its own.
pub fn title_of(line: &str, keywords: &Keywords) -> String {
    let mut title = todo::parse(line, &keywords.all)
        .map(|h| h.title.to_string())
        .unwrap_or_else(|| line.trim_start_matches('*').trim().to_string());
    while let Some(stamp) = timestamp::first_stamp(&title) {
        title.replace_range(stamp.start..stamp.end, "");
    }
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `path::line::heading`, the line 1-based.
pub fn reference(shown: &str, line: u32, title: &str) -> String {
    format!("{shown}::{}::{title}", line + 1)
}

fn repeater_text(r: &Repeater) -> String {
    let base = match r.base {
        Base::Stamp => "+",
        Base::StampCatchUp => "++",
        Base::Completion => ".+",
    };
    let unit = match r.unit {
        Unit::Day => 'd',
        Unit::Week => 'w',
        Unit::Month => 'm',
        Unit::Year => 'y',
    };
    format!("{base}{}{unit}", r.count)
}

fn iso(day: i64) -> String {
    agenda::group_key(day)
}

/// `Thursday 2026-11-12`.
fn day_name(day: i64) -> String {
    const NAMES: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let (y, m, d) = agenda::civil_from_epoch_day(day);
    format!("{} {}", NAMES[timestamp::weekday(y, m, d)], iso(day))
}

/// `YYYY-MM-DD` as an epoch day.
fn parse_day(text: &str) -> Result<i64, String> {
    let bad = || format!("{text:?} is not a YYYY-MM-DD date");
    let mut parts = text.trim().splitn(3, '-');
    let (Some(y), Some(m), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad());
    };
    let y: i32 = y.parse().map_err(|_| bad())?;
    let m: u32 = m.parse().map_err(|_| bad())?;
    let d: u32 = d.parse().map_err(|_| bad())?;
    let day = timestamp::epoch_day(y, m, d);
    // A day the calendar does not have (02-30) round-trips to another one.
    if !(1..=12).contains(&m) || agenda::civil_from_epoch_day(day) != (y, m, d) {
        return Err(bad());
    }
    Ok(day)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn kw() -> Keywords {
        Keywords::from_spec("TODO WAITING | DONE")
    }

    fn today() -> i64 {
        timestamp::epoch_day(2026, 11, 12)
    }

    const PLAN: &str = "#+FILETAGS: :work:\n\
* TODO [#A] Pay the invoice :money:\n\
DEADLINE: <2026-11-10 Tue>\n\
* TODO Standup\n\
SCHEDULED: <2026-11-11 Wed 09:30 +1d>\n\
* Dentist <2026-11-14 Sat 15:00>\n\
\n\
* TODO Write the report\n\
* DONE Old thing\n\
SCHEDULED: <2026-11-12 Thu>\n";

    fn files() -> Vec<OrgFile> {
        vec![OrgFile {
            shown: "~/org/plan.org".into(),
            text: PLAN.into(),
            rows: agenda::scan_file(PLAN, &kw(), false),
        }]
    }

    fn ask(args: &str) -> String {
        agenda(&files(), &kw(), today(), args).unwrap()
    }

    #[test]
    fn today_opens_with_the_date_overdue_first_and_the_undated_counted() {
        assert_eq!(
            ask("{}"),
            "Today is Thursday 2026-11-12.\n\
             Agenda for Thursday 2026-11-12:\n\
             \n\
             Overdue:\n\
             - TODO [#A] Pay the invoice :work:money: (deadline 2026-11-10, 2 days ago) \
             [ref ~/org/plan.org::2::Pay the invoice]\n\
             - 09:30 TODO Standup :work: (scheduled 2026-11-11, 1 day ago, repeats +1d) \
             [ref ~/org/plan.org::4::Standup]\n\
             \n\
             2026-11-12 Thu (today):\n\
             - 09:30 TODO Standup :work: (scheduled, repeats +1d) \
             [ref ~/org/plan.org::4::Standup]\n\
             \n\
             1 TODO entry has no date; org.query with match \"/TODO\" lists them."
        );
    }

    /// A week from a later day: no overdue block (it is not anchored on
    /// today), the repeat on each day, the event on its day.
    #[test]
    fn a_week_from_a_later_day_shows_each_repeat_and_no_overdue() {
        let out = ask(r#"{"range": "week", "from": "2026-11-13"}"#);
        assert!(
            out.starts_with(
                "Today is Thursday 2026-11-12.\n\
                 Agenda for Friday 2026-11-13 to Thursday 2026-11-19:\n"
            ),
            "{out}"
        );
        assert!(!out.contains("Overdue"), "{out}");
        assert_eq!(
            out.matches("Standup").count(),
            14,
            "a title and a ref per day: {out}"
        );
        assert!(
            out.contains("- 15:00 Dentist :work: (event) [ref ~/org/plan.org::6::Dentist]"),
            "{out}"
        );
    }

    #[test]
    fn a_match_narrows_every_block() {
        let out = ask(r#"{"match": "+money"}"#);
        assert!(out.contains("entries matching +money"), "{out}");
        assert!(out.contains("Pay the invoice"), "{out}");
        assert!(!out.contains("Standup"), "{out}");
        assert!(!out.contains("no date"), "{out}");
        let none = ask(r#"{"range": "week", "match": "+garden"}"#);
        assert!(
            none.contains("Nothing is scheduled, due or overdue then."),
            "{none}"
        );
    }

    #[test]
    fn bad_input_is_an_error_the_model_can_act_on() {
        let err = |args: &str| agenda(&files(), &kw(), today(), args).unwrap_err();
        assert_eq!(
            err(r#"{"from": "next week"}"#),
            r#"from: "next week" is not a YYYY-MM-DD date"#
        );
        assert_eq!(
            err(r#"{"from": "2026-02-30"}"#),
            r#"from: "2026-02-30" is not a YYYY-MM-DD date"#
        );
        assert_eq!(
            err(r#"{"from": "2026-11-12", "to": "2026-11-01"}"#),
            "to: before from"
        );
        assert!(err(r#"{"match": "LEVEL>2"}"#).starts_with("match: unsupported"));
        assert!(err(r#"{"range": "month"}"#).starts_with("range:"));
    }

    #[test]
    fn the_tool_list_carries_a_schema_from_the_args_struct() {
        let tools = tools();
        assert_eq!(tools[0].name, "agenda");
        assert!(
            tools[0].input_schema.contains(r#""enum":["day","week"]"#),
            "{}",
            tools[0].input_schema
        );
        assert!(tools[0].input_schema.contains("org's match syntax"));
    }
}

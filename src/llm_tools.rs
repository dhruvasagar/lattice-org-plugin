//! AT.7 — org's tools for the assistant, the `llm-tools` seam.
//!
//! Design: `docs/dev/architecture/llm-assistant-tools.md` §6 (in the lattice
//! repo). Only what needs org's parser lives here; free-text search, reading
//! a file and the web are lattice's own tools.
//!
//! **The tools read what the agenda reads.** `org.agenda` runs the agenda's
//! own scan and [`agenda::occurrences`], so the assistant cannot answer "what
//! is on Thursday" differently from `:org-agenda`, repeats included.
//! `org.query` is the agenda's tags search over the same scan, and `org.entry`
//! reads one entry whole.
//!
//! **Every answer opens with today.** A model must never infer the date, and
//! a small one asked "what's due this week" will happily assume its training
//! cut-off is the present.
//!
//! **Rows carry a ref**, `path::line::heading` (1-based line, `~` for home),
//! which `org.entry` and `org.change` take. A ref the file has moved under is
//! resolved by its heading when that is unambiguous, and refused with "query
//! again" when it is not: a guess is how an edit lands on the wrong entry.
//!
//! This module is pure: the guest (`lib.rs`) reads through `host-services`
//! behind [`Org`], so every answer here is testable on the host without one.

use lattice_plugin_sdk::llm::{input_schema, parse_args};
use lattice_plugin_sdk::ConfigShape;

use crate::agenda::{self, Filter, Keywords, Kind, Row, Section, When};
use crate::repeat::{Base, Repeater, Unit};
use crate::roam::Node;
use crate::timestamp;
use crate::todo;

/// `org.agenda`'s name within the plugin (the assistant sees `org.agenda`).
pub const AGENDA: &str = "agenda";
/// `org.query`.
pub const QUERY: &str = "query";
/// `org.entry`.
pub const ENTRY: &str = "entry";
/// `org.clock`.
pub const CLOCK: &str = "clock";
/// `org.notes`.
pub const NOTES: &str = "notes";

/// The most rows one answer lists; the rest are counted.
const MAX_ROWS: usize = 150;
/// What `org.query` lists when the model gives no `limit`.
const DEFAULT_LIMIT: usize = 50;
/// What `org.notes` lists when the model gives no `limit`.
const DEFAULT_NOTES: usize = 20;
/// The most backlinks named under one note; the rest are counted.
const MAX_BACKLINKS: usize = 10;
/// The widest window `from`/`to` may span.
const MAX_DAYS: i64 = 366;
/// The most lines of one entry `org.entry` returns.
const MAX_ENTRY_LINES: usize = 200;

/// A tool as the WIT lists it, before the guest maps it to the generated type.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: String,
}

/// The tools org offers, all `read`.
pub fn tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: AGENDA,
            description: "List what is scheduled, due and overdue in the user's org files for a \
                          day, a week or a date range, optionally only entries matching tags or \
                          a TODO state. Use it for questions about plans, deadlines and what is \
                          on a given day.",
            input_schema: input_schema::<AgendaArgs>().unwrap_or_default(),
        },
        ToolSpec {
            name: QUERY,
            description: "Find entries in the user's org files by tags, properties or TODO state, \
                          whatever their dates. Use it for questions like what is waiting on \
                          someone, what is tagged for a project, or which tasks have no date; for \
                          what falls on a day, use org.agenda.",
            input_schema: input_schema::<QueryArgs>().unwrap_or_default(),
        },
        ToolSpec {
            name: ENTRY,
            description: "Read one org entry in full: its notes, properties, log and subheadings. \
                          Give the ref another org tool listed. Use it before answering about an \
                          entry's details.",
            input_schema: input_schema::<EntryArgs>().unwrap_or_default(),
        },
        ToolSpec {
            name: CLOCK,
            description: "Total the time the user clocked in their org files over a range of \
                          days, per entry, per file or per tag. Use it for questions about where \
                          time went and for reports of work done.",
            input_schema: input_schema::<ClockArgs>().unwrap_or_default(),
        },
        ToolSpec {
            name: NOTES,
            description: "Find the user's org-roam notes by words in their title or aliases, or \
                          by tag, with the notes that link to each. Use it for questions about \
                          what the user has written on a topic; read a note with org.entry.",
            input_schema: input_schema::<NotesArgs>().unwrap_or_default(),
        },
    ]
}

/// One org file, read, with its agenda rows.
pub struct OrgFile {
    /// As a ref spells it: `~` for the home directory.
    pub shown: String,
    pub text: String,
    pub rows: Vec<Row>,
}

/// The files a call reads, and what could not be read.
#[derive(Default)]
pub struct Corpus {
    pub files: Vec<OrgFile>,
    /// One line per root or file that could not be read, told to the model so
    /// a missing answer is not mistaken for an empty one.
    pub problems: Vec<String>,
}

/// What the tools read, behind the host.
pub trait Org {
    /// The org files, scanned. `tags_search` is the agenda view's switch: a
    /// headline that only a match could take (undated, DONE, plain but
    /// tagged) becomes a row too.
    fn corpus(&self, tags_search: bool) -> Corpus;
    /// One file's text, by the path a ref spells (`~` allowed).
    fn read(&self, path: &str) -> Result<String, String>;
    /// Every node in the roam index; empty when roam is not set up or the
    /// index is not built.
    fn roam_nodes(&self) -> Vec<Node> {
        Vec::new()
    }
    /// One roam node by its ID, case-insensitive.
    fn roam_node(&self, _id: &str) -> Option<Node> {
        None
    }
    /// The IDs of the nodes linking to `id`.
    fn roam_backlinks(&self, _id: &str) -> Vec<String> {
        Vec::new()
    }
    /// A path as a ref spells it (`~` for home).
    fn shown(&self, path: &str) -> String {
        path.to_string()
    }
}

/// Run one of org's tools: the text of its answer, or the error the model
/// sees.
pub fn call(
    org: &dyn Org,
    keywords: &Keywords,
    today: i64,
    name: &str,
    args: &str,
) -> Result<String, String> {
    match name {
        AGENDA => agenda(org, keywords, today, args),
        QUERY => query(org, keywords, today, args),
        ENTRY => entry(org, keywords, today, args),
        CLOCK => clock(org, keywords, today, args),
        NOTES => notes(org, today, args),
        other => Err(format!("org has no tool `{other}`")),
    }
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

/// `org.query`'s input.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShape)]
pub struct QueryArgs {
    /// What to find, in org's match syntax: `+work`, `+work-meeting`, `/WAITING`,
    /// `+home/TODO`, `OWNER="Sam"`, `/DONE`. Tags, properties and TODO states only; for dates,
    /// use org.agenda.
    pub r#match: String,
    /// The most entries to list; 50 when left out, at most 150.
    pub limit: Option<i64>,
}

/// `org.entry`'s input.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShape)]
pub struct EntryArgs {
    /// The entry's ref as another org tool listed it (`file::line::heading`), or the value of
    /// its `ID` property.
    pub r#ref: String,
}

/// What `org.clock` totals under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ConfigShape)]
pub enum ClockBy {
    Entry,
    File,
    Tag,
}

/// `org.clock`'s input.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShape)]
pub struct ClockArgs {
    /// The first day, `YYYY-MM-DD`. Six days before `to` when left out, so a week.
    pub from: Option<String>,
    /// The last day, `YYYY-MM-DD`. Today when left out.
    pub to: Option<String>,
    /// `entry` (the default), `file` or `tag`. An entry with several tags counts under each.
    pub by: Option<ClockBy>,
}

/// `org.notes`' input.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShape)]
pub struct NotesArgs {
    /// Words that must all appear in a note's title or one of its aliases, in any case.
    pub query: Option<String>,
    /// Only notes carrying this tag, their own or inherited.
    pub tag: Option<String>,
    /// The most notes to list; 20 when left out, at most 150.
    pub limit: Option<i64>,
}

/// `org.agenda`.
fn agenda(org: &dyn Org, keywords: &Keywords, today: i64, args: &str) -> Result<String, String> {
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

    // A match makes the view scan as a tags search, so this does too: a
    // tagged DONE entry in the window is in both or in neither.
    let corpus = org.corpus(matcher.is_some());
    let mut undated = 0usize;
    let mut late: Vec<Item> = Vec::new();
    let mut dated: Vec<Item> = Vec::new();
    for file in &corpus.files {
        let lines: Vec<&str> = file.text.lines().collect();
        for row in &file.rows {
            for (rank, at) in agenda::occurrences(row, &sections, keywords, from) {
                if rank == 0 {
                    undated += 1;
                    continue;
                }
                let item = Item {
                    sort: agenda::sort_key_in_section(&at, rank as i64, from),
                    line: describe(&at, &lines, file, keywords, today, false),
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
    tell_problems(&mut out, &corpus);
    Ok(out.trim_end().to_string())
}

/// `org.query`: the agenda's tags search, over every headline a match could
/// take, in file order.
fn query(org: &dyn Org, keywords: &Keywords, today: i64, args: &str) -> Result<String, String> {
    let args: QueryArgs = parse_args(args).map_err(|e| e.to_string())?;
    let source = args.r#match.trim();
    let matcher =
        crate::agenda_match::parse(source).map_err(|e| format!("match: {}", e.message()))?;
    let limit = match args.limit {
        None => DEFAULT_LIMIT,
        Some(n) if (1..=MAX_ROWS as i64).contains(&n) => n as usize,
        Some(_) => return Err(format!("limit: between 1 and {MAX_ROWS}")),
    };
    let corpus = org.corpus(true);
    let mut found = Vec::new();
    for file in &corpus.files {
        let lines: Vec<&str> = file.text.lines().collect();
        for row in &file.rows {
            if matcher.admits(&row.tags, &row.properties, row.keyword.as_deref(), |kw| {
                keywords.is_done(kw)
            }) {
                found.push(describe(row, &lines, file, keywords, today, true));
            }
        }
    }
    let mut out = format!("Today is {}.\n", day_name(today));
    match found.len() {
        0 => out.push_str(&format!("No entry matches {source}.\n")),
        1 => out.push_str(&format!("1 entry matches {source}:\n")),
        n => out.push_str(&format!("{n} entries match {source}:\n")),
    }
    for line in found.iter().take(limit) {
        out.push_str(line);
        out.push('\n');
    }
    if found.len() > limit {
        out.push_str(&format!(
            "\n{} more not listed; raise limit (at most {MAX_ROWS}) or narrow the match.\n",
            found.len() - limit
        ));
    }
    tell_problems(&mut out, &corpus);
    Ok(out.trim_end().to_string())
}

/// `org.entry`: one entry, headline to the end of its subtree.
fn entry(org: &dyn Org, keywords: &Keywords, today: i64, args: &str) -> Result<String, String> {
    let args: EntryArgs = parse_args(args).map_err(|e| e.to_string())?;
    // Small models echo the row's wrapping (`[ref …]`) along with the ref.
    let wanted = args
        .r#ref
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let wanted = wanted.strip_prefix("ref ").unwrap_or(wanted).trim();
    if wanted.is_empty() {
        return Err("ref: empty".to_string());
    }
    let (path, text, at) = match wanted.split_once("::") {
        Some((path, rest)) => {
            let (line, heading) = rest
                .split_once("::")
                .ok_or_else(|| format!("ref: `{wanted}` is not file::line::heading"))?;
            let line: usize = line
                .trim()
                .parse()
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| format!("ref: `{line}` is not a line number"))?;
            if !crate::roam_scan::is_org(path) {
                return Err(format!("ref: `{path}` is not an org file"));
            }
            let text = org.read(path).map_err(|e| format!("ref: {path}: {e}"))?;
            let at = locate(&text, line - 1, heading.trim(), keywords).ok_or_else(|| {
                format!(
                    "ref: `{wanted}` no longer names one entry (the file changed since it was \
                     listed); query again for a fresh ref"
                )
            })?;
            (path.to_string(), text, at)
        }
        None => {
            let corpus = org.corpus(false);
            let mut hit = None;
            for file in corpus.files {
                if let Some(at) = by_id(&file.text, wanted) {
                    hit = Some((file.shown, file.text, at));
                    break;
                }
            }
            match hit {
                Some(hit) => hit,
                // Not in the agenda files: a roam note, perhaps, which may be
                // a whole file rather than a headline.
                None => match org.roam_node(wanted) {
                    Some(node) => {
                        let path = org.shown(&node.file);
                        let text = org
                            .read(&node.file)
                            .map_err(|e| format!("ref: {path}: {e}"))?;
                        if node.level == 0 {
                            return Ok(whole_note(&path, &text, &node, today));
                        }
                        let at = locate(&text, node.line as usize, &node.title, keywords)
                            .or_else(|| {
                                let at = node.line as usize;
                                text.lines().nth(at).and_then(headline_level).map(|_| at)
                            })
                            .ok_or_else(|| {
                                format!(
                                    "ref: the note `{wanted}` is not where the roam index puts \
                                     it; :org-roam-sync re-indexes"
                                )
                            })?;
                        (path, text, at)
                    }
                    None => {
                        return Err(format!(
                            "ref: no entry has the ID `{wanted}` in the agenda files or the roam \
                             notes; a ref from another org tool looks like file::line::heading"
                        ))
                    }
                },
            }
        }
    };

    let lines: Vec<&str> = text.lines().collect();
    let level = headline_level(lines[at]).unwrap_or(1);
    let end = (at + 1..lines.len())
        .find(|&j| headline_level(lines[j]).is_some_and(|l| l <= level))
        .unwrap_or(lines.len());
    let title = title_of(lines[at], keywords);
    let mut out = format!("Today is {}.\n", day_name(today));
    out.push_str(&format!(
        "Entry [ref {}] in {path}",
        reference(&path, at as u32, &title)
    ));
    let parents = ancestors(&lines, at, level, keywords);
    if !parents.is_empty() {
        out.push_str(&format!(", under {}", parents.join(" > ")));
    }
    out.push_str(":\n\n");
    let shown_end = end.min(at + MAX_ENTRY_LINES);
    for line in &lines[at..shown_end] {
        out.push_str(line);
        out.push('\n');
    }
    if end > shown_end {
        out.push_str(&format!(
            "\n{} more lines not shown; read the file for the rest.\n",
            end - shown_end
        ));
    }
    Ok(out.trim_end().to_string())
}

/// A file-level roam note: the whole file, capped.
fn whole_note(path: &str, text: &str, node: &Node, today: i64) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = format!("Today is {}.\n", day_name(today));
    out.push_str(&format!(
        "Note \"{}\" [ref {}], the whole of {path}:\n\n",
        node.title, node.id
    ));
    for line in lines.iter().take(MAX_ENTRY_LINES) {
        out.push_str(line);
        out.push('\n');
    }
    if lines.len() > MAX_ENTRY_LINES {
        out.push_str(&format!(
            "\n{} more lines not shown; read the file for the rest.\n",
            lines.len() - MAX_ENTRY_LINES
        ));
    }
    out.trim_end().to_string()
}

/// `org.notes`: roam nodes by title or alias words and by tag, with the
/// notes linking to each. A note's ref is its `ID`, which survives edits.
fn notes(org: &dyn Org, today: i64, args: &str) -> Result<String, String> {
    let args: NotesArgs = parse_args(args).map_err(|e| e.to_string())?;
    let words: Vec<String> = args
        .query
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let tag = args
        .tag
        .as_deref()
        .map(|t| t.trim().trim_matches(':').to_string())
        .filter(|t| !t.is_empty());
    if words.is_empty() && tag.is_none() {
        return Err("query: give words to look for, or a tag".to_string());
    }
    let limit = match args.limit {
        None => DEFAULT_NOTES,
        Some(n) if (1..=MAX_ROWS as i64).contains(&n) => n as usize,
        Some(_) => return Err(format!("limit: between 1 and {MAX_ROWS}")),
    };
    let nodes = org.roam_nodes();
    if nodes.is_empty() {
        return Err(
            "the roam index is empty: set org.roam-directory, or run :org-roam-sync to build it"
                .to_string(),
        );
    }
    let names = |n: &Node| {
        std::iter::once(&n.title)
            .chain(&n.aliases)
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>()
    };
    let mut found: Vec<(bool, &Node)> = nodes
        .iter()
        .filter(|n| {
            tag.as_ref()
                .is_none_or(|t| n.tags.iter().any(|x| x.eq_ignore_ascii_case(t)))
        })
        .filter_map(|n| {
            let names = names(n);
            let hit = words.iter().all(|w| names.iter().any(|s| s.contains(w)));
            // A title hit outranks an alias-only one.
            let title = n.title.to_lowercase();
            hit.then(|| (words.iter().all(|w| title.contains(w)), n))
        })
        .collect();
    found.sort_by_key(|(in_title, n)| (!*in_title, n.title.to_lowercase()));

    let what = match (words.is_empty(), &tag) {
        (false, Some(t)) => format!("\"{}\" tagged {t}", words.join(" ")),
        (false, None) => format!("\"{}\"", words.join(" ")),
        (true, Some(t)) => format!("tagged {t}"),
        (true, None) => unreachable!("refused above"),
    };
    let mut out = format!("Today is {}.\n", day_name(today));
    match found.len() {
        0 => out.push_str(&format!("No note matches {what}.\n")),
        1 => out.push_str(&format!("1 note matches {what}:\n")),
        n => out.push_str(&format!("{n} notes match {what}:\n")),
    }
    for (_, n) in found.iter().take(limit) {
        out.push_str(&format!("- {}", n.title));
        if !n.aliases.is_empty() {
            out.push_str(&format!(" (also: {})", n.aliases.join(", ")));
        }
        if !n.tags.is_empty() {
            out.push_str(&format!(" :{}:", n.tags.join(":")));
        }
        out.push_str(&format!(" [ref {}] in {}\n", n.id, org.shown(&n.file)));
        let from = org.roam_backlinks(&n.id);
        if !from.is_empty() {
            let titles: Vec<String> = from
                .iter()
                .take(MAX_BACKLINKS)
                .map(|id| {
                    org.roam_node(id)
                        .map(|b| b.title)
                        .unwrap_or_else(|| id.clone())
                })
                .collect();
            out.push_str(&format!("  Linked from: {}", titles.join("; ")));
            if from.len() > MAX_BACKLINKS {
                out.push_str(&format!(" and {} more", from.len() - MAX_BACKLINKS));
            }
            out.push('\n');
        }
    }
    if found.len() > limit {
        out.push_str(&format!(
            "\n{} more not listed; raise limit (at most {MAX_ROWS}) or narrow the query.\n",
            found.len() - limit
        ));
    }
    Ok(out.trim_end().to_string())
}

/// `org.clock`: minutes from `CLOCK:` lines whose start falls in the range,
/// totalled per entry, file or tag, largest first.
///
/// The spans are `clock_scan`'s, the agenda clock report's: a running clock
/// counts nothing, and a span is filed on the day it began.
fn clock(org: &dyn Org, keywords: &Keywords, today: i64, args: &str) -> Result<String, String> {
    let args: ClockArgs = parse_args(args).map_err(|e| e.to_string())?;
    let to = match &args.to {
        Some(s) => parse_day(s).map_err(|e| format!("to: {e}"))?,
        None => today,
    };
    let from = match &args.from {
        Some(s) => parse_day(s).map_err(|e| format!("from: {e}"))?,
        None => to - 6,
    };
    if to < from {
        return Err("to: before from".to_string());
    }
    if to - from >= MAX_DAYS {
        return Err(format!("to: at most {MAX_DAYS} days from from"));
    }
    let by = args.by.unwrap_or(ClockBy::Entry);

    // Tags come from the tags-search scan, which has a row for every headline
    // carrying one (its own or inherited); a clocked headline with none has
    // no row and no tags.
    let corpus = org.corpus(by == ClockBy::Tag);
    let mut totals: Vec<(String, u32)> = Vec::new();
    let mut add = |key: String, minutes: u32| match totals.iter_mut().find(|(k, _)| *k == key) {
        Some((_, m)) => *m += minutes,
        None => totals.push((key, minutes)),
    };
    let mut all = 0u32;
    for file in &corpus.files {
        let lines: Vec<&str> = file.text.lines().collect();
        for span in crate::clock_scan::scan(&file.text) {
            if span.day < from || span.day > to {
                continue;
            }
            all += span.minutes;
            match by {
                ClockBy::File => add(file.shown.clone(), span.minutes),
                ClockBy::Tag => {
                    let tags = file
                        .rows
                        .iter()
                        .find(|r| r.line == span.line)
                        .map(|r| r.tags.clone())
                        .unwrap_or_default();
                    if tags.is_empty() {
                        add("(no tag)".to_string(), span.minutes);
                    }
                    for tag in tags {
                        add(format!(":{tag}:"), span.minutes);
                    }
                }
                ClockBy::Entry => {
                    let at = span.line as usize;
                    let line = lines.get(at).copied().unwrap_or("");
                    let title = title_of(line, keywords);
                    let level = headline_level(line).unwrap_or(1);
                    let mut path = ancestors(&lines, at, level, keywords);
                    path.push(title.clone());
                    add(
                        format!(
                            "{} [ref {}]",
                            path.join(" > "),
                            reference(&file.shown, span.line, &title)
                        ),
                        span.minutes,
                    );
                }
            }
        }
    }
    // Largest first; ties in the order they were met (file, then line).
    totals.sort_by_key(|(_, m)| std::cmp::Reverse(*m));

    let mut out = format!(
        "Today is {}.
",
        day_name(today)
    );
    let window = if from == to {
        day_name(from)
    } else {
        format!("{} to {}", day_name(from), day_name(to))
    };
    if all == 0 {
        out.push_str(&format!("No time was clocked from {window}.\n"));
    } else {
        let per = match by {
            ClockBy::Entry => "entry",
            ClockBy::File => "file",
            ClockBy::Tag => "tag; an entry with several tags counts under each",
        };
        out.push_str(&format!(
            "Clocked from {window}: {} in total. By {per}:\n",
            hours(all)
        ));
        for (key, minutes) in totals.iter().take(MAX_ROWS) {
            out.push_str(&format!("- {} {key}\n", hours(*minutes)));
        }
        if totals.len() > MAX_ROWS {
            out.push_str(&format!(
                "\n{} more not listed; ask for a shorter range or by file or tag.\n",
                totals.len() - MAX_ROWS
            ));
        }
    }
    tell_problems(&mut out, &corpus);
    Ok(out.trim_end().to_string())
}

/// `90` → `1:30`, as org writes a duration.
fn hours(minutes: u32) -> String {
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

/// The headline a ref names: its own line if that still carries the heading,
/// else the one headline in the file that does. `None` when none or several
/// do — refusing is safer than choosing.
fn locate(text: &str, line: usize, heading: &str, keywords: &Keywords) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let is_it = |i: usize| {
        lines
            .get(i)
            .is_some_and(|l| headline_level(l).is_some() && title_of(l, keywords) == heading)
    };
    if is_it(line) {
        return Some(line);
    }
    let mut found = (0..lines.len()).filter(|&i| is_it(i));
    match (found.next(), found.next()) {
        (Some(i), None) => Some(i),
        _ => None,
    }
}

/// The headline whose drawer holds `:ID: id`.
fn by_id(text: &str, id: &str) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.iter().position(|l| {
        let l = l.trim();
        l.get(..4).is_some_and(|k| k.eq_ignore_ascii_case(":ID:")) && l[4..].trim() == id
    })?;
    (0..at).rev().find(|&i| headline_level(lines[i]).is_some())
}

/// `**` → 2. `None` for a line that is not a headline.
fn headline_level(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    (stars > 0 && line[stars..].starts_with(' ')).then_some(stars)
}

/// The titles of the headlines `at` sits under, outermost first.
fn ancestors(lines: &[&str], at: usize, level: usize, keywords: &Keywords) -> Vec<String> {
    let mut out = Vec::new();
    let mut below = level;
    for i in (0..at).rev() {
        if below == 1 {
            break;
        }
        if let Some(l) = headline_level(lines[i]).filter(|&l| l < below) {
            out.push(title_of(lines[i], keywords));
            below = l;
        }
    }
    out.reverse();
    out
}

fn tell_problems(out: &mut String, corpus: &Corpus) {
    if !corpus.problems.is_empty() {
        out.push('\n');
        for p in &corpus.problems {
            out.push_str(&format!("\nNot read: {p}"));
        }
        out.push('\n');
    }
}

struct Item {
    sort: i64,
    day: i64,
    line: String,
}

/// One row as a line: time, keyword, priority, title, tags, what dates it,
/// and its ref.
///
/// `with_date` says the date even for today and later; the agenda leaves it
/// to the day header the row sits under.
fn describe(
    row: &Row,
    lines: &[&str],
    file: &OrgFile,
    keywords: &Keywords,
    today: i64,
    with_date: bool,
) -> String {
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
        if with_date || d.day < today {
            let s = |n: i64| if n == 1 { "" } else { "s" };
            let when = match d.day - today {
                0 => "today".to_string(),
                n if n < 0 => format!("{} day{} ago", -n, s(-n)),
                n => format!("in {n} day{}", s(n)),
            };
            note.push_str(&format!(" {}, {when}", iso(d.day)));
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

    const HOME: &str = "* Projects :proj:\n\
** WAITING Reply from legal\n\
:PROPERTIES:\n\
:ID: legal-42\n\
:END:\n\
Sent the draft on Monday.\n\
*** TODO Chase them\n\
SCHEDULED: <2026-11-20 Fri>\n\
* DONE Untagged finished thing\n";

    /// The files by the name a ref spells, scanned as the guest scans them.
    struct Fake(Vec<(&'static str, &'static str)>);

    impl Org for Fake {
        fn corpus(&self, tags_search: bool) -> Corpus {
            Corpus {
                files: self
                    .0
                    .iter()
                    .map(|(shown, text)| OrgFile {
                        shown: shown.to_string(),
                        text: text.to_string(),
                        rows: agenda::scan_file(text, &kw(), tags_search),
                    })
                    .collect(),
                problems: Vec::new(),
            }
        }

        fn read(&self, path: &str) -> Result<String, String> {
            self.0
                .iter()
                .find(|(shown, _)| *shown == path)
                .map(|(_, text)| text.to_string())
                .ok_or_else(|| "no such file".to_string())
        }
    }

    fn plan() -> Fake {
        Fake(vec![("~/org/plan.org", PLAN)])
    }

    fn both() -> Fake {
        Fake(vec![("~/org/plan.org", PLAN), ("~/org/home.org", HOME)])
    }

    fn run(org: &Fake, name: &str, args: &str) -> Result<String, String> {
        call(org, &kw(), today(), name, args)
    }

    fn ask(args: &str) -> String {
        run(&plan(), AGENDA, args).unwrap()
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

    /// With a match the scan is the view's tags search, so a DONE entry on
    /// the day shows, as it does in a view section with that match.
    #[test]
    fn a_match_scans_as_the_view_does() {
        assert!(!ask("{}").contains("Old thing"));
        let out = ask(r#"{"match": "+work"}"#);
        assert!(
            out.contains("- DONE Old thing :work: (scheduled) [ref ~/org/plan.org::9::Old thing]"),
            "{out}"
        );
    }

    #[test]
    fn bad_input_is_an_error_the_model_can_act_on() {
        let err = |name: &str, args: &str| run(&both(), name, args).unwrap_err();
        assert_eq!(
            err(AGENDA, r#"{"from": "next week"}"#),
            r#"from: "next week" is not a YYYY-MM-DD date"#
        );
        assert_eq!(
            err(AGENDA, r#"{"from": "2026-02-30"}"#),
            r#"from: "2026-02-30" is not a YYYY-MM-DD date"#
        );
        assert_eq!(
            err(AGENDA, r#"{"from": "2026-11-12", "to": "2026-11-01"}"#),
            "to: before from"
        );
        assert!(err(AGENDA, r#"{"match": "LEVEL>2"}"#).starts_with("match: unsupported"));
        assert!(err(AGENDA, r#"{"range": "month"}"#).starts_with("range:"));
        assert!(err(QUERY, "{}").contains("match"));
        assert!(err(QUERY, r#"{"match": ""}"#).starts_with("match: empty"));
        assert_eq!(
            err(QUERY, r#"{"match": "+a", "limit": 0}"#),
            "limit: between 1 and 150"
        );
        assert_eq!(err("delete", "{}"), "org has no tool `delete`");
    }

    #[test]
    fn a_query_finds_entries_whatever_their_dates() {
        let out = run(&both(), QUERY, r#"{"match": "+proj/WAITING|TODO"}"#).unwrap();
        assert_eq!(
            out,
            "Today is Thursday 2026-11-12.\n\
             2 entries match +proj/WAITING|TODO:\n\
             - WAITING Reply from legal :proj: [ref ~/org/home.org::2::Reply from legal]\n\
             - TODO Chase them :proj: (scheduled 2026-11-20, in 8 days) \
             [ref ~/org/home.org::7::Chase them]"
        );
    }

    /// A DONE headline with no tags is found by its keyword: before AT.7c it
    /// was not a row at all, so `/DONE` could not reach it.
    #[test]
    fn a_query_reaches_done_entries_with_no_tags() {
        let out = run(&both(), QUERY, r#"{"match": "/DONE"}"#).unwrap();
        assert!(out.contains("2 entries match /DONE:"), "{out}");
        assert!(
            out.contains("- DONE Old thing :work: (scheduled 2026-11-12, today)"),
            "{out}"
        );
        assert!(
            out.contains(
                "- DONE Untagged finished thing [ref ~/org/home.org::9::Untagged finished thing]"
            ),
            "{out}"
        );
        let none = run(&both(), QUERY, r#"{"match": "+garden"}"#).unwrap();
        assert!(none.ends_with("No entry matches +garden."), "{none}");
    }

    #[test]
    fn a_query_lists_up_to_its_limit_and_counts_the_rest() {
        let out = run(&both(), QUERY, r#"{"match": "/TODO", "limit": 1}"#).unwrap();
        assert!(out.contains("4 entries match /TODO:"), "{out}");
        assert_eq!(out.matches("\n- ").count(), 1, "{out}");
        assert!(
            out.contains("3 more not listed; raise limit (at most 150) or narrow the match."),
            "{out}"
        );
    }

    const LEGAL: &str = "Today is Thursday 2026-11-12.\n\
Entry [ref ~/org/home.org::2::Reply from legal] in ~/org/home.org, under Projects:\n\
\n\
** WAITING Reply from legal\n\
:PROPERTIES:\n\
:ID: legal-42\n\
:END:\n\
Sent the draft on Monday.\n\
*** TODO Chase them\n\
SCHEDULED: <2026-11-20 Fri>";

    #[test]
    fn an_entry_is_read_whole_by_its_ref_or_its_id() {
        let read = |r: &str| run(&both(), ENTRY, &format!(r#"{{"ref": "{r}"}}"#)).unwrap();
        assert_eq!(read("~/org/home.org::2::Reply from legal"), LEGAL);
        assert_eq!(
            read("[ref ~/org/home.org::2::Reply from legal]"),
            LEGAL,
            "the row's wrapping, echoed"
        );
        assert_eq!(read("legal-42"), LEGAL);
    }

    /// A ref whose line moved still finds its heading when exactly one
    /// headline carries it, and the answer gives the fresh ref. Otherwise it
    /// is refused, never guessed.
    #[test]
    fn a_stale_ref_is_resolved_by_its_heading_or_refused() {
        let read = |r: &str| run(&both(), ENTRY, &format!(r#"{{"ref": "{r}"}}"#));
        assert_eq!(read("~/org/home.org::5::Reply from legal").unwrap(), LEGAL);
        let gone = read("~/org/home.org::2::Reply from finance").unwrap_err();
        assert!(gone.contains("query again"), "{gone}");
        let twice = Fake(vec![("~/a.org", "* TODO Same\n* DONE Same\n")]);
        let err = run(&twice, ENTRY, r#"{"ref": "~/a.org::7::Same"}"#).unwrap_err();
        assert!(err.contains("no longer names one entry"), "{err}");
    }

    #[test]
    fn an_entry_ref_that_cannot_be_read_says_why() {
        let err = |r: &str| run(&both(), ENTRY, &format!(r#"{{"ref": "{r}"}}"#)).unwrap_err();
        assert_eq!(
            err("~/.ssh/id_ed25519::1::x"),
            "ref: `~/.ssh/id_ed25519` is not an org file"
        );
        assert_eq!(
            err("~/org/gone.org::1::x"),
            "ref: ~/org/gone.org: no such file"
        );
        assert_eq!(
            err("~/org/home.org::two::x"),
            "ref: `two` is not a line number"
        );
        assert!(err("no-such-id").contains("no entry has the ID `no-such-id`"));
    }

    #[test]
    fn an_answer_says_what_it_could_not_read() {
        struct Partly;
        impl Org for Partly {
            fn corpus(&self, _: bool) -> Corpus {
                Corpus {
                    files: Vec::new(),
                    problems: vec!["~/org: permission denied".to_string()],
                }
            }
            fn read(&self, _: &str) -> Result<String, String> {
                Err("unused".to_string())
            }
        }
        let out = call(&Partly, &kw(), today(), QUERY, r#"{"match": "+a"}"#).unwrap();
        assert!(out.ends_with("Not read: ~/org: permission denied"), "{out}");
    }

    const WORK: &str = "#+FILETAGS: :work:\n\
* Website :web:\n\
** DONE Fix the header\n\
:LOGBOOK:\n\
CLOCK: [2026-11-10 Tue 09:00]--[2026-11-10 Tue 10:30] =>  1:30\n\
CLOCK: [2026-11-11 Wed 14:00]--[2026-11-11 Wed 14:45] =>  0:45\n\
CLOCK: [2026-11-01 Sun 09:00]--[2026-11-01 Sun 12:00] =>  3:00\n\
:END:\n\
* Admin\n\
CLOCK: [2026-11-12 Thu 08:00]--[2026-11-12 Thu 08:20] =>  0:20\n\
CLOCK: [2026-11-12 Thu 16:00]\n";

    fn work() -> Fake {
        Fake(vec![("~/org/work.org", WORK)])
    }

    /// The default range is the week to today: the 1st is outside it, the
    /// running clock counts nothing, and the largest total comes first.
    #[test]
    fn clocked_time_is_totalled_per_entry_over_the_last_week() {
        assert_eq!(
            run(&work(), CLOCK, "{}").unwrap(),
            "Today is Thursday 2026-11-12.\n\
             Clocked from Friday 2026-11-06 to Thursday 2026-11-12: 2:35 in total. By entry:\n\
             - 2:15 Website > Fix the header [ref ~/org/work.org::3::Fix the header]\n\
             - 0:20 Admin [ref ~/org/work.org::9::Admin]"
        );
    }

    #[test]
    fn clocked_time_by_tag_and_by_file() {
        let tags = run(&work(), CLOCK, r#"{"by": "tag", "from": "2026-11-01"}"#).unwrap();
        assert!(
            tags.contains(
                "5:35 in total. By tag; an entry with several tags counts under each:\n\
                 - 5:35 :work:\n\
                 - 5:15 :web:"
            ),
            "{tags}"
        );
        let files = run(&work(), CLOCK, r#"{"by": "file", "to": "2026-11-11"}"#).unwrap();
        assert!(
            files.contains(
                "Clocked from Thursday 2026-11-05 to Wednesday 2026-11-11: 2:15 in total. \
                 By file:\n- 2:15 ~/org/work.org"
            ),
            "{files}"
        );
        let none = run(
            &work(),
            CLOCK,
            r#"{"from": "2026-10-01", "to": "2026-10-07"}"#,
        )
        .unwrap();
        assert!(
            none.ends_with("No time was clocked from Thursday 2026-10-01 to Wednesday 2026-10-07."),
            "{none}"
        );
        let err = run(&work(), CLOCK, r#"{"by": "week"}"#).unwrap_err();
        assert!(err.starts_with("by:"), "{err}");
    }

    /// A roam node; `at` is `(line, level)`, `(0, 0)` for a file node.
    fn node(
        id: &str,
        title: &str,
        aliases: &[&str],
        tags: &[&str],
        file: &str,
        (line, level): (u32, u32),
    ) -> Node {
        Node {
            id: id.into(),
            title: title.into(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            refs: Vec::new(),
            file: file.into(),
            line,
            level,
        }
    }

    const ROAM_RUST: &str = "#+title: Rust\n:PROPERTIES:\n:ID: rust\n:END:\nA language.\n";
    const ROAM_NOTES: &str = "* Ownership\n:PROPERTIES:\n:ID: own\n:END:\nOne owner.\n\
                              * Lifetimes\n:PROPERTIES:\n:ID: life\n:END:\n";

    /// A roam index over two files beside the agenda's.
    struct Roam(Fake, Vec<Node>);

    impl Org for Roam {
        fn corpus(&self, tags_search: bool) -> Corpus {
            self.0.corpus(tags_search)
        }
        fn read(&self, path: &str) -> Result<String, String> {
            match path {
                "/home/u/roam/rust.org" => Ok(ROAM_RUST.into()),
                "/home/u/roam/notes.org" => Ok(ROAM_NOTES.into()),
                other => self.0.read(other),
            }
        }
        fn roam_nodes(&self) -> Vec<Node> {
            self.1.clone()
        }
        fn roam_node(&self, id: &str) -> Option<Node> {
            self.1
                .iter()
                .find(|n| n.id.eq_ignore_ascii_case(id))
                .cloned()
        }
        fn roam_backlinks(&self, id: &str) -> Vec<String> {
            match id {
                "rust" => vec!["own".into(), "life".into()],
                _ => Vec::new(),
            }
        }
        fn shown(&self, path: &str) -> String {
            path.replace("/home/u", "~")
        }
    }

    fn roam() -> Roam {
        Roam(
            both(),
            vec![
                node(
                    "rust",
                    "Rust",
                    &["rustlang"],
                    &["lang"],
                    "/home/u/roam/rust.org",
                    (0, 0),
                ),
                node(
                    "own",
                    "Ownership",
                    &[],
                    &["lang", "rust"],
                    "/home/u/roam/notes.org",
                    (0, 1),
                ),
                node(
                    "life",
                    "Lifetimes",
                    &["borrow regions"],
                    &["rust"],
                    "/home/u/roam/notes.org",
                    (5, 1),
                ),
            ],
        )
    }

    #[test]
    fn notes_are_found_by_title_or_alias_with_their_backlinks() {
        let out = call(&roam(), &kw(), today(), NOTES, r#"{"query": "RUST"}"#).unwrap();
        assert_eq!(
            out,
            "Today is Thursday 2026-11-12.\n\
             1 note matches \"rust\":\n\
             - Rust (also: rustlang) :lang: [ref rust] in ~/roam/rust.org\n  \
             Linked from: Ownership; Lifetimes"
        );
        let alias = call(&roam(), &kw(), today(), NOTES, r#"{"query": "borrow"}"#).unwrap();
        assert!(
            alias.contains("- Lifetimes (also: borrow regions) :rust: [ref life]"),
            "{alias}"
        );
        let tagged = call(&roam(), &kw(), today(), NOTES, r#"{"tag": ":rust:"}"#).unwrap();
        assert!(
            tagged.contains("2 notes match tagged rust:\n- Lifetimes"),
            "{tagged}"
        );
        let none = call(
            &roam(),
            &kw(),
            today(),
            NOTES,
            r#"{"query": "go", "tag": "lang"}"#,
        )
        .unwrap();
        assert!(
            none.ends_with("No note matches \"go\" tagged lang."),
            "{none}"
        );
    }

    #[test]
    fn notes_refuse_an_empty_question_and_an_empty_index() {
        let err = call(&roam(), &kw(), today(), NOTES, "{}").unwrap_err();
        assert_eq!(err, "query: give words to look for, or a tag");
        let err = run(&both(), NOTES, r#"{"query": "rust"}"#).unwrap_err();
        assert!(err.starts_with("the roam index is empty"), "{err}");
    }

    /// A note's ref is its ID; `org.entry` finds it in the roam index when
    /// the agenda files do not have it, a file node whole and a headline node
    /// as a subtree.
    #[test]
    fn an_entry_reads_a_roam_note_by_its_id() {
        let read = |id: &str| {
            call(
                &roam(),
                &kw(),
                today(),
                ENTRY,
                &format!(r#"{{"ref": "{id}"}}"#),
            )
        };
        let file = read("rust").unwrap();
        assert!(
            file.starts_with(
                "Today is Thursday 2026-11-12.\n\
                 Note \"Rust\" [ref rust], the whole of ~/roam/rust.org:\n\n#+title: Rust\n"
            ),
            "{file}"
        );
        let headline = read("LIFE").unwrap();
        assert!(
            headline.contains(
                "Entry [ref ~/roam/notes.org::6::Lifetimes] in ~/roam/notes.org:\n\n* Lifetimes\n"
            ),
            "{headline}"
        );
        assert!(!headline.contains("One owner."), "{headline}");
        let err = read("nowhere").unwrap_err();
        assert!(
            err.contains("in the agenda files or the roam notes"),
            "{err}"
        );
    }

    #[test]
    fn the_tool_list_carries_a_schema_from_each_args_struct() {
        let tools = tools();
        let names: Vec<_> = tools.iter().map(|t| t.name).collect();
        assert_eq!(names, ["agenda", "query", "entry", "clock", "notes"]);
        assert!(
            tools[0].input_schema.contains(r#""enum":["day","week"]"#),
            "{}",
            tools[0].input_schema
        );
        assert!(tools[0].input_schema.contains("org's match syntax"));
        assert!(
            tools[1].input_schema.contains(r#""required":["match"]"#),
            "{}",
            tools[1].input_schema
        );
        assert!(
            tools[2].input_schema.contains(r#""required":["ref"]"#),
            "{}",
            tools[2].input_schema
        );
    }
}

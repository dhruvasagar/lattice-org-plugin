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
/// `org.change`, the one tool that proposes edits.
pub const CHANGE: &str = "change";

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
    /// Whether the tool proposes edits; every other tool only reads.
    pub proposes: bool,
}

/// One workflow org contributes as an LLM profile (AT.9, design §7): a
/// system prompt and a tool selection, and nothing else. The prompts are text
/// files under `src/profiles/`, so they read as prose and diff as prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The workflow's own prompt, then the rules every org workflow shares.
    pub system: String,
    /// As a profile file spells `tools`: comma-separated.
    pub tools: &'static str,
}

/// What every org workflow tells the model about org's tools.
const COMMON: &str = include_str!("profiles/common.txt");

/// The five workflows of design §7.
pub fn profiles() -> Vec<ProfileSpec> {
    let with_common = |own: &str| {
        format!("{}\n{COMMON}", own.trim_end())
            .trim_end()
            .to_string()
    };
    vec![
        ProfileSpec {
            name: "org",
            description: "Help over your org files: tasks, plans, notes and logs",
            system: with_common(include_str!("profiles/org.txt")),
            tools: "org.*, lattice.read",
        },
        ProfileSpec {
            name: "org-plan",
            description: "Plan a day or a week from your agenda",
            system: with_common(include_str!("profiles/org-plan.txt")),
            tools: "org.agenda, org.query, org.entry, org.change",
        },
        ProfileSpec {
            name: "org-review",
            description: "A weekly review: what got done, what slipped, what is stuck",
            system: with_common(include_str!("profiles/org-review.txt")),
            tools: "org.agenda, org.query, org.clock, org.change",
        },
        ProfileSpec {
            name: "org-report",
            description: "A report of your work over a period, from the clock and the log",
            system: with_common(include_str!("profiles/org-report.txt")),
            tools: "org.clock, org.query, org.entry",
        },
        ProfileSpec {
            name: "org-research",
            description: "Research a topic into your org-roam notes",
            system: with_common(include_str!("profiles/org-research.txt")),
            tools: "org.notes, lattice.read, web.*, org.change",
        },
    ]
}

/// One proposed edit, with `lattice.edit_file`'s semantics: `old` occurs in
/// `path` exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub path: String,
    pub old: String,
    pub new: String,
}

/// A tool's answer: text for the model and the edits it proposes. The host
/// reviews and applies each proposal, and tells the model what happened.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Answer {
    pub text: String,
    pub proposals: Vec<Proposal>,
}

impl From<String> for Answer {
    fn from(text: String) -> Self {
        Answer {
            text,
            proposals: Vec::new(),
        }
    }
}

/// What a change needs from the host besides the files: the clock and the
/// TODO keywords with their logging flags.
pub struct ChangeEnv {
    /// `[2026-11-12 Thu 10:00]`, for a log line.
    pub now_stamp: String,
    /// `org.log-into-drawer`.
    pub log_into_drawer: bool,
    /// `org.todo-keywords`, with each state's logging flags.
    pub todo: crate::todo::Keywords,
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
            proposes: false,
        },
        ToolSpec {
            name: QUERY,
            description: "Find entries in the user's org files by tags, properties or TODO state, \
                          whatever their dates. Use it for questions like what is waiting on \
                          someone, what is tagged for a project, or which tasks have no date; for \
                          what falls on a day, use org.agenda.",
            input_schema: input_schema::<QueryArgs>().unwrap_or_default(),
            proposes: false,
        },
        ToolSpec {
            name: ENTRY,
            description: "Read one org entry in full: its notes, properties, log and subheadings. \
                          Give the ref another org tool listed. Use it before answering about an \
                          entry's details.",
            input_schema: input_schema::<EntryArgs>().unwrap_or_default(),
            proposes: false,
        },
        ToolSpec {
            name: CLOCK,
            description: "Total the time the user clocked in their org files over a range of \
                          days, per entry, per file or per tag. Use it for questions about where \
                          time went and for reports of work done.",
            input_schema: input_schema::<ClockArgs>().unwrap_or_default(),
            proposes: false,
        },
        ToolSpec {
            name: NOTES,
            description: "Find the user's org-roam notes by words in their title or aliases, or \
                          by tag, with the notes that link to each. Use it for questions about \
                          what the user has written on a topic; read a note with org.entry.",
            input_schema: input_schema::<NotesArgs>().unwrap_or_default(),
            proposes: false,
        },
        ToolSpec {
            name: CHANGE,
            description: "Change one org entry: set its TODO state, schedule it, give it a \
                          deadline, add and remove tags, add a note to its log, or refile it under another \
                          headline; or capture something new through one of the user's capture \
                          templates. Give the ref another org tool \
                          listed. The user reviews the edit before it is made; org writes the \
                          timestamps and log lines, so describe the change, not the text.",
            input_schema: input_schema::<ChangeArgs>().unwrap_or_default(),
            proposes: true,
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
    /// A path as a proposal names it: what `lattice.edit_file` opens.
    fn real_path(&self, path: &str) -> String {
        path.to_string()
    }
    /// The clock and keywords a change writes with.
    fn change_env(&self) -> ChangeEnv;
    /// What capturing `text` through the template keyed `template` would
    /// write, through the capture keys' own expansion and targets.
    fn capture(
        &self,
        _template: &str,
        _text: &str,
        _answers: &[String],
    ) -> Result<Captured, String> {
        Err("capture: not available here".to_string())
    }
}

/// Where a capture's text goes in its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    End,
    /// Before this 0-based line; past the end is the end.
    Line(u32),
}

/// What a capture writes: each file's insertion, and what the capture said
/// about it (a target headline it could not find, say).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Captured {
    /// `(path, anchor, text)`, the path as [`Org::read`] takes it.
    pub writes: Vec<(String, Anchor, String)>,
    pub notes: Vec<String>,
    /// The template's description, for the answer.
    pub described: String,
}

/// Run one of org's tools: the text of its answer, or the error the model
/// sees.
pub fn call(
    org: &dyn Org,
    keywords: &Keywords,
    today: i64,
    name: &str,
    args: &str,
) -> Result<Answer, String> {
    match name {
        AGENDA => agenda(org, keywords, today, args).map(Answer::from),
        QUERY => query(org, keywords, today, args).map(Answer::from),
        ENTRY => entry(org, keywords, today, args).map(Answer::from),
        CLOCK => clock(org, keywords, today, args).map(Answer::from),
        NOTES => notes(org, today, args).map(Answer::from),
        CHANGE => change(org, keywords, today, args),
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

/// What `org.change` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ConfigShape)]
pub enum ChangeOp {
    SetState,
    Schedule,
    Deadline,
    Tag,
    AddNote,
    Refile,
    Capture,
}

/// `org.change`'s input.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShape)]
pub struct ChangeArgs {
    /// `set-state`, `schedule`, `deadline`, `tag`, `add-note`, `refile` or `capture`.
    pub op: ChangeOp,
    /// The entry's ref as another org tool listed it (`file::line::heading`), or its `ID`.
    /// Every op but `capture` needs it.
    pub r#ref: Option<String>,
    /// For `set-state`: the TODO keyword to set, such as `DONE`; empty clears it.
    pub state: Option<String>,
    /// For `add-note`: the note to add to the entry's log. For `set-state`: a note, for a
    /// state that asks for one.
    pub note: Option<String>,
    /// For `schedule` and `deadline`: `2026-11-14`, `+3d`, `tomorrow` or `fri`, optionally
    /// followed by a repeater such as `+1w`; `none` removes it.
    pub date: Option<String>,
    /// For `tag`: tags to add, and `-tag` to remove one.
    pub tags: Option<Vec<String>>,
    /// For `refile`: the ref of the headline to file the entry under, or an org file's path
    /// to file it at the top level.
    pub target: Option<String>,
    /// For `capture`: the key of one of the user's capture templates.
    pub template: Option<String>,
    /// For `capture`: the text the template files, where the user would type it.
    pub text: Option<String>,
    /// For `capture`: the answers to the template's questions, in its order.
    pub answers: Option<Vec<String>>,
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
/// Where a ref leads.
struct Located {
    /// As refs and answers spell the file (`~` for home).
    shown: String,
    /// As [`Org::read`] takes it; a proposal names [`Org::real_path`] of it.
    file: String,
    text: String,
    target: Target,
}

enum Target {
    /// A headline, by its 0-based line.
    Headline(usize),
    /// A roam note that is the whole file.
    File(Node),
}

/// The entry a ref names: `file::line::heading` (a moved line found by its
/// heading when only one headline carries it), else an `ID` in the agenda
/// files, else an `ID` in the roam index.
fn resolve(org: &dyn Org, raw: &str, keywords: &Keywords) -> Result<Located, String> {
    // Small models echo the row's wrapping (`[ref …]`) along with the ref.
    let wanted = raw.trim().trim_start_matches('[').trim_end_matches(']');
    let wanted = wanted.strip_prefix("ref ").unwrap_or(wanted).trim();
    if wanted.is_empty() {
        return Err("ref: empty".to_string());
    }
    if let Some((path, rest)) = wanted.split_once("::") {
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
        return Ok(Located {
            shown: path.to_string(),
            file: path.to_string(),
            text,
            target: Target::Headline(at),
        });
    }
    for file in org.corpus(false).files {
        if let Some(at) = by_id(&file.text, wanted) {
            return Ok(Located {
                file: file.shown.clone(),
                shown: file.shown,
                text: file.text,
                target: Target::Headline(at),
            });
        }
    }
    // Not in the agenda files: a roam note, perhaps, which may be a whole
    // file rather than a headline.
    let node = org.roam_node(wanted).ok_or_else(|| {
        format!(
            "ref: no entry has the ID `{wanted}` in the agenda files or the roam notes; a ref \
             from another org tool looks like file::line::heading"
        )
    })?;
    let shown = org.shown(&node.file);
    let text = org
        .read(&node.file)
        .map_err(|e| format!("ref: {shown}: {e}"))?;
    let target = if node.level == 0 {
        Target::File(node.clone())
    } else {
        let at = locate(&text, node.line as usize, &node.title, keywords)
            .or_else(|| {
                let at = node.line as usize;
                text.lines().nth(at).and_then(headline_level).map(|_| at)
            })
            .ok_or_else(|| {
                format!(
                    "ref: the note `{wanted}` is not where the roam index puts it; \
                     :org-roam-sync re-indexes"
                )
            })?;
        Target::Headline(at)
    };
    Ok(Located {
        shown,
        file: node.file,
        text,
        target,
    })
}

/// The 0-based line after the subtree at `at`: the next headline at its
/// level or above, or the end of the file.
fn subtree_end(lines: &[&str], at: usize) -> usize {
    let level = headline_level(lines[at]).unwrap_or(1);
    (at + 1..lines.len())
        .find(|&j| headline_level(lines[j]).is_some_and(|l| l <= level))
        .unwrap_or(lines.len())
}

fn entry(org: &dyn Org, keywords: &Keywords, today: i64, args: &str) -> Result<String, String> {
    let args: EntryArgs = parse_args(args).map_err(|e| e.to_string())?;
    let Located {
        shown: path,
        text,
        target,
        ..
    } = resolve(org, &args.r#ref, keywords)?;
    let at = match target {
        Target::Headline(at) => at,
        Target::File(node) => return Ok(whole_note(&path, &text, &node, today)),
    };

    let lines: Vec<&str> = text.lines().collect();
    let level = headline_level(lines[at]).unwrap_or(1);
    let end = subtree_end(&lines, at);
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

/// `org.change`: one entry changed through the verbs the keys use, as a
/// proposal. The model says what; org writes the text.
fn change(org: &dyn Org, keywords: &Keywords, today: i64, args: &str) -> Result<Answer, String> {
    let args: ChangeArgs = parse_args(args).map_err(|e| e.to_string())?;
    if args.op == ChangeOp::Capture {
        return capture(org, &args);
    }
    let r#ref = args
        .r#ref
        .as_deref()
        .ok_or("ref: give the entry's ref, as another org tool listed it")?;
    let loc = resolve(org, r#ref, keywords)?;
    let at = match &loc.target {
        Target::Headline(at) => *at,
        Target::File(node) => {
            return Err(format!(
                "ref: `{}` is a whole-file note, with no headline to change",
                node.id
            ))
        }
    };
    let lines: Vec<&str> = loc.text.lines().collect();
    let head = lines[at];
    let title = title_of(head, keywords);
    let names = keywords.all.clone();
    let nothing = |why: String| -> Result<Answer, String> {
        Ok(Answer::from(format!("Nothing to change: {why}.")))
    };
    let (edits, mut said) = match args.op {
        ChangeOp::SetState => {
            let want = args
                .state
                .as_deref()
                .map(str::trim)
                .ok_or("state: set-state needs the keyword to set")?;
            if !want.is_empty() && !names.iter().any(|k| k == want) {
                return Err(format!(
                    "state: `{want}` is not one of the TODO keywords ({})",
                    names.join(", ")
                ));
            }
            let env = org.change_env();
            let (_, done) = env.todo.split();
            let cx = crate::complete::Context {
                today: crate::repeat::from_days(today),
                now_stamp: &env.now_stamp,
                keywords: &names,
                done_keywords: &done,
                log_into_drawer: env.log_into_drawer,
            };
            let end = subtree_end(&lines, at);
            let subtree = || lines[at..end].iter().map(|l| l.to_string()).collect();
            // An `@` state asked for a note the model did not give gets the
            // plain log line, as org writes when the note is left empty.
            let note = Some(args.note.as_deref().unwrap_or(""));
            let Some(changed) =
                crate::complete::change_state(head, subtree, want, note, &env.todo, &cx)
            else {
                return nothing(format!("\"{title}\" is already {}", or_none(want)));
            };
            let mut said = format!("Set \"{title}\" to {}", or_none(want));
            if let Some((reset_to, d)) = &changed.repeated {
                said = format!(
                    "Marked \"{title}\" {want}. It repeats, so it goes back to {reset_to}, next \
                     on {:04}-{:02}-{:02}",
                    d.year, d.month, d.day
                );
            }
            let span = if changed.headline_only {
                (at, at)
            } else {
                (at, end - 1)
            };
            (vec![propose(org, &loc, span, &changed.lines)?], said)
        }
        ChangeOp::Schedule | ChangeOp::Deadline => {
            let (field, what) = match args.op {
                ChangeOp::Schedule => (crate::planning::Field::Scheduled, "scheduled"),
                _ => (crate::planning::Field::Deadline, "deadline"),
            };
            let date = args
                .date
                .as_deref()
                .map(str::trim)
                .ok_or("date: give a date, or `none` to remove it")?;
            let value = if date.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(
                    crate::org_date::parse(date, crate::repeat::from_days(today))
                        .map_err(|e| format!("date: {e}"))?
                        .render(),
                )
            };
            let next = lines.get(at + 1).copied();
            use crate::planning::PlanEdit;
            let (span, new) =
                match crate::planning::plan_edit(at as u32, next, field, value.clone()) {
                    PlanEdit::Nothing => {
                        return nothing(match &value {
                            Some(v) => format!("\"{title}\" is already {what} {v}"),
                            None => format!("\"{title}\" has no {what} date"),
                        })
                    }
                    PlanEdit::Replace { text, .. } => ((at, at + 1), vec![head.to_string(), text]),
                    PlanEdit::Insert { text, .. } => ((at, at), vec![head.to_string(), text]),
                    PlanEdit::Delete { .. } => ((at, at + 1), vec![head.to_string()]),
                };
            let said = match &value {
                Some(v) if what == "scheduled" => format!("Scheduled \"{title}\" for {v}"),
                Some(v) => format!("Gave \"{title}\" a deadline of {v}"),
                None => format!("Removed the {what} date from \"{title}\""),
            };
            (vec![propose(org, &loc, span, &new)?], said)
        }
        // Taken before a ref is resolved, since a capture has none.
        ChangeOp::Capture => return capture(org, &args),
        ChangeOp::Refile => refile(org, keywords, &loc, at, args.target.as_deref())?,
        ChangeOp::AddNote => {
            let env = org.change_env();
            let log = args
                .note
                .as_deref()
                .and_then(|n| crate::complete::note_taken_line(&env.now_stamp, n))
                .ok_or("note: add-note needs the note's text")?;
            let end = subtree_end(&lines, at);
            let mut new: Vec<String> = lines[at..end].iter().map(|l| l.to_string()).collect();
            crate::complete::insert_log(&mut new, &log, env.log_into_drawer);
            let said = format!("Added a note to \"{title}\"");
            (vec![propose(org, &loc, (at, end - 1), &new)?], said)
        }
        ChangeOp::Tag => {
            let asked = args.tags.unwrap_or_default();
            if asked
                .iter()
                .all(|t| t.trim().trim_matches(['-', '+', ':']).is_empty())
            {
                return Err("tags: give tags to add, and `-tag` to remove one".to_string());
            }
            let h = crate::todo::parse(head, &names).ok_or("ref: not a headline")?;
            let mut tags: Vec<String> = h.tags.iter().map(|t| t.to_string()).collect();
            for t in &asked {
                let t = t.trim();
                match t.strip_prefix('-') {
                    Some(gone) => tags.retain(|x| x != gone.trim_matches(':')),
                    None => {
                        let t = t.trim_start_matches('+').trim_matches(':');
                        if t.contains(|c: char| c.is_whitespace() || c == ':') {
                            return Err(format!("tags: `{t}` is not one tag"));
                        }
                        if !t.is_empty() && !tags.iter().any(|x| x == t) {
                            tags.push(t.to_string());
                        }
                    }
                }
            }
            let new_head = crate::todo::set_tags(head, &names, &tags.join(":"))
                .ok_or("ref: not a headline")?;
            if new_head == head {
                return nothing(format!("\"{title}\" already has those tags"));
            }
            let said = if tags.is_empty() {
                format!("Removed every tag from \"{title}\"")
            } else {
                format!("Tagged \"{title}\" :{}:", tags.join(":"))
            };
            (vec![propose(org, &loc, (at, at), &[new_head])?], said)
        }
    };
    said.push_str(&format!(
        " in {}. The user reviews the edit before it is made. Lines in this file may move: \
         use the ref this tool or a query gives for a further change.",
        loc.shown
    ));
    Ok(Answer {
        text: said,
        proposals: edits,
    })
}

/// `org.change`'s refile: the subtree at `at` moved under `target`, a
/// child of it as `<leader>or` files it (`capture_target::relevel`). In one
/// file that is one proposal. Across two it is two, the copy first: a user
/// who accepts the copy and declines the removal has the entry twice, which
/// is recoverable, where the other order would lose it.
fn refile(
    org: &dyn Org,
    keywords: &Keywords,
    src: &Located,
    at: usize,
    target: Option<&str>,
) -> Result<(Vec<Proposal>, String), String> {
    let target = target
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or("target: refile needs the ref of a headline, or an org file's path")?;
    let lines: Vec<&str> = src.text.lines().collect();
    let end = subtree_end(&lines, at);
    let title = title_of(lines[at], keywords);
    let file_target = !target.contains("::") && crate::roam_scan::is_org(target);
    let (dst, level, before, under) = if file_target {
        let text = org
            .read(target)
            .map_err(|e| format!("target: {target}: {e}"))?;
        let n = text.lines().count();
        let shown = org.shown(target);
        let dst = Located {
            file: target.to_string(),
            shown: shown.clone(),
            text,
            target: Target::Headline(0),
        };
        (dst, 0, n, format!("the top of {shown}"))
    } else {
        let dst = resolve(org, target, keywords).map_err(|e| e.replacen("ref:", "target:", 1))?;
        let t = match &dst.target {
            Target::Headline(t) => *t,
            Target::File(node) => {
                return Err(format!(
                    "target: `{}` is a whole-file note; give its file's path to file at its top \
                     level",
                    node.id
                ));
            }
        };
        let tlines: Vec<&str> = dst.text.lines().collect();
        let level = headline_level(tlines[t]).unwrap_or(1);
        let before = subtree_end(&tlines, t);
        let under = format!("\"{}\"", title_of(tlines[t], keywords));
        (dst, level, before, under)
    };
    let same = org.real_path(&dst.file) == org.real_path(&src.file);
    if same && !file_target {
        if let Target::Headline(t) = dst.target {
            if (at..end).contains(&t) {
                return Err(format!("target: \"{title}\" cannot be filed under itself"));
            }
        }
    }
    let moved = crate::capture_target::relevel(&lines[at..end].join("\n"), level);
    let moved: Vec<String> = moved.lines().map(str::to_string).collect();
    let mut kept: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let said = format!("Filed \"{title}\" under {under}");
    if same {
        kept.drain(at..end);
        let before = if before > at {
            before - (end - at)
        } else {
            before
        };
        kept.splice(before..before, moved);
        let edit = propose_rewrite(org, src, &kept)?
            .ok_or_else(|| format!("Nothing to change: \"{title}\" is already there"))?;
        return Ok((vec![edit], said));
    }
    let mut filed: Vec<String> = dst.text.lines().map(str::to_string).collect();
    filed.splice(before..before, moved);
    let copy = propose_rewrite(org, &dst, &filed)?.ok_or("target: nothing would change")?;
    kept.drain(at..end);
    let removal = propose_rewrite(org, src, &kept)?.ok_or("ref: nothing would change")?;
    Ok((
        vec![copy, removal],
        format!(
            "{said}, as two edits: the copy into {} first, then its removal from {}. Declining \
             the second leaves it in both files, not in neither",
            dst.shown, src.shown
        ),
    ))
}

/// `org.change`'s capture: the template's writes as proposals. A target
/// file that is missing or empty is refused: `lattice.edit_file` cannot
/// create a file or anchor in an empty one, and creating a file the user's
/// template names is the one thing a review of a diff cannot show well.
fn capture(org: &dyn Org, args: &ChangeArgs) -> Result<Answer, String> {
    let template = args
        .template
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or("template: capture needs a capture template's key")?;
    let text = args.text.as_deref().unwrap_or("").trim();
    let answers = args.answers.clone().unwrap_or_default();
    let captured = org.capture(template, text, &answers)?;
    let mut files: Vec<(Located, Vec<String>)> = Vec::new();
    for (path, anchor, inserted) in &captured.writes {
        let i = match files.iter().position(|(l, _)| &l.file == path) {
            Some(i) => i,
            None => {
                let shown = org.shown(path);
                let text = org.read(path).map_err(|_| {
                    format!(
                        "capture: the template files into {shown}, which does not exist; \
                         create it first, or capture with the keys"
                    )
                })?;
                let lines = text.lines().map(str::to_string).collect();
                let loc = Located {
                    file: path.clone(),
                    shown,
                    text,
                    target: Target::Headline(0),
                };
                files.push((loc, lines));
                files.len() - 1
            }
        };
        let lines = &mut files[i].1;
        let at = match anchor {
            Anchor::Start => 0,
            Anchor::End => lines.len(),
            Anchor::Line(n) => (*n as usize).min(lines.len()),
        };
        lines.splice(at..at, inserted.lines().map(str::to_string));
    }
    let mut proposals = Vec::new();
    for (loc, lines) in &files {
        proposals.extend(propose_rewrite(org, loc, lines)?);
    }
    if proposals.is_empty() {
        return Err("capture: the template wrote nothing".to_string());
    }
    let into: Vec<&str> = files.iter().map(|(l, _)| l.shown.as_str()).collect();
    let mut said = format!(
        "Captured with \"{}\" into {}.",
        captured.described,
        into.join(" and ")
    );
    for note in &captured.notes {
        said.push(' ');
        said.push_str(note);
    }
    said.push_str(" The user reviews the edit before it is made.");
    Ok(Answer {
        text: said,
        proposals,
    })
}

/// The located file rewritten to `new`, as one proposal over the lines that
/// differ. A pure insertion anchors on the line before it (after, at the
/// top), and a pure deletion on the line after it (before, at the end), so
/// neither is an empty side and a deletion takes its line break with it.
/// `None` when nothing differs.
fn propose_rewrite(
    org: &dyn Org,
    loc: &Located,
    new: &[String],
) -> Result<Option<Proposal>, String> {
    let old: Vec<&str> = loc.text.lines().collect();
    if old.is_empty() {
        return Err(format!(
            "{}: the file is empty, and an edit needs text in it to anchor on; add a line to \
             it first",
            loc.shown
        ));
    }
    let mut p = old
        .iter()
        .zip(new)
        .take_while(|(a, b)| **a == b.as_str())
        .count();
    let room = old.len().min(new.len()) - p;
    let mut s = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| **a == b.as_str())
        .count();
    if p + s == old.len() && p + s == new.len() {
        return Ok(None);
    }
    if p + s == old.len() {
        // A pure insertion: nothing old to replace.
        if p > 0 {
            p -= 1;
        } else {
            s -= 1;
        }
    } else if p + s == new.len() && (s > 0 || p > 0) {
        // A pure deletion: an empty replacement would leave its line break.
        if s > 0 {
            s -= 1;
        } else {
            p -= 1;
        }
    }
    propose(org, loc, (p, old.len() - 1 - s), &new[p..new.len() - s]).map(Some)
}

fn or_none(state: &str) -> &str {
    if state.is_empty() {
        "no state"
    } else {
        state
    }
}

/// Lines `start..=end` of the located file replaced by `new`, as an edit
/// whose old text occurs once in the file: widened by whole lines around it
/// until it is unique, so a duplicate headline elsewhere cannot take the edit.
fn propose(
    org: &dyn Org,
    loc: &Located,
    (start, end): (usize, usize),
    new: &[String],
) -> Result<Proposal, String> {
    let text = &loc.text;
    let lines: Vec<&str> = text.lines().collect();
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let (mut before, mut after) = (start, end);
    loop {
        let old = lines[before..=after].join(nl);
        if text.matches(old.as_str()).count() == 1 {
            let mut replaced: Vec<&str> = lines[before..start].to_vec();
            replaced.extend(new.iter().map(String::as_str));
            replaced.extend(&lines[end + 1..=after]);
            return Ok(Proposal {
                path: org.real_path(&loc.file),
                old,
                new: replaced.join(nl),
            });
        }
        if before > 0 {
            before -= 1;
        } else if after + 1 < lines.len() {
            after += 1;
        } else {
            return Err(format!(
                "{}: the entry is repeated word for word, so no edit names it alone",
                loc.shown
            ));
        }
    }
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

        fn real_path(&self, path: &str) -> String {
            path.replace('~', "/home/u")
        }

        fn change_env(&self) -> ChangeEnv {
            env()
        }
    }

    fn env() -> ChangeEnv {
        ChangeEnv {
            now_stamp: "[2026-11-12 Thu 10:00]".to_string(),
            log_into_drawer: true,
            todo: crate::todo::parse_todo_keywords("TODO WAITING(w@) | DONE(d!)"),
        }
    }

    fn plan() -> Fake {
        Fake(vec![("~/org/plan.org", PLAN)])
    }

    fn both() -> Fake {
        Fake(vec![("~/org/plan.org", PLAN), ("~/org/home.org", HOME)])
    }

    fn run(org: &Fake, name: &str, args: &str) -> Result<String, String> {
        text(call(org, &kw(), today(), name, args))
    }

    fn text(answer: Result<Answer, String>) -> Result<String, String> {
        answer.map(|a| {
            assert!(a.proposals.is_empty(), "a read tool proposes nothing");
            a.text
        })
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
            fn change_env(&self) -> ChangeEnv {
                env()
            }
        }
        let out = text(call(&Partly, &kw(), today(), QUERY, r#"{"match": "+a"}"#)).unwrap();
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
        fn change_env(&self) -> ChangeEnv {
            env()
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
        let out = text(call(&roam(), &kw(), today(), NOTES, r#"{"query": "RUST"}"#)).unwrap();
        assert_eq!(
            out,
            "Today is Thursday 2026-11-12.\n\
             1 note matches \"rust\":\n\
             - Rust (also: rustlang) :lang: [ref rust] in ~/roam/rust.org\n  \
             Linked from: Ownership; Lifetimes"
        );
        let alias = text(call(
            &roam(),
            &kw(),
            today(),
            NOTES,
            r#"{"query": "borrow"}"#,
        ))
        .unwrap();
        assert!(
            alias.contains("- Lifetimes (also: borrow regions) :rust: [ref life]"),
            "{alias}"
        );
        let tagged = text(call(&roam(), &kw(), today(), NOTES, r#"{"tag": ":rust:"}"#)).unwrap();
        assert!(
            tagged.contains("2 notes match tagged rust:\n- Lifetimes"),
            "{tagged}"
        );
        let none = text(call(
            &roam(),
            &kw(),
            today(),
            NOTES,
            r#"{"query": "go", "tag": "lang"}"#,
        ))
        .unwrap();
        assert!(
            none.ends_with("No note matches \"go\" tagged lang."),
            "{none}"
        );
    }

    #[test]
    fn notes_refuse_an_empty_question_and_an_empty_index() {
        let err = text(call(&roam(), &kw(), today(), NOTES, "{}")).unwrap_err();
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
            text(call(
                &roam(),
                &kw(),
                today(),
                ENTRY,
                &format!(r#"{{"ref": "{id}"}}"#),
            ))
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

    /// One `org.change` call: its answer text and its one proposal.
    fn change_of(org: &Fake, args: &str) -> (String, Proposal) {
        let a = call(org, &kw(), today(), CHANGE, args).unwrap();
        assert_eq!(a.proposals.len(), 1, "{a:?}");
        (a.text, a.proposals.into_iter().next().unwrap())
    }

    fn change_err(org: &Fake, args: &str) -> String {
        call(org, &kw(), today(), CHANGE, args).unwrap_err()
    }

    /// `DONE(d!)` logs the change, as the key does: the whole subtree is the
    /// edit, the drawer under the headline.
    #[test]
    fn set_state_proposes_what_the_key_writes() {
        let (said, p) = change_of(
            &plan(),
            r#"{"op": "set-state", "ref": "~/org/plan.org::8::Write the report", "state": "DONE"}"#,
        );
        assert_eq!(
            p,
            Proposal {
                path: "/home/u/org/plan.org".into(),
                old: "* TODO Write the report".into(),
                new: "* DONE Write the report\n  \
                      :LOGBOOK:\n  \
                      - State \"DONE\" from \"TODO\" [2026-11-12 Thu 10:00]\n  \
                      :END:"
                    .into(),
            }
        );
        assert!(
            said.starts_with(
                "Set \"Write the report\" to DONE in ~/org/plan.org. The user reviews"
            ),
            "{said}"
        );
    }

    #[test]
    fn set_state_repeats_a_repeating_task_and_takes_a_note() {
        let (said, p) = change_of(
            &plan(),
            r#"{"op": "set-state", "ref": "~/org/plan.org::4::Standup", "state": "DONE"}"#,
        );
        assert!(
            said.contains("It repeats, so it goes back to TODO, next on"),
            "{said}"
        );
        assert!(
            p.old
                .starts_with("* TODO Standup\nSCHEDULED: <2026-11-11 Wed 09:30 +1d>"),
            "{p:?}"
        );
        assert!(
            p.new.starts_with("* TODO Standup\nSCHEDULED: <2026-11-12"),
            "{p:?}"
        );

        let (_, p) = change_of(
            &both(),
            r#"{"op": "set-state", "ref": "~/org/home.org::7::Chase them", "state": "WAITING", "note": "they are out until Monday"}"#,
        );
        assert!(p.new.starts_with("*** WAITING Chase them\n"), "{p:?}");
        assert!(
            p.new.contains("\\\\\n    they are out until Monday"),
            "{p:?}"
        );
    }

    #[test]
    fn schedule_and_deadline_go_through_the_date_prompt() {
        let (said, p) = change_of(
            &plan(),
            r#"{"op": "schedule", "ref": "~/org/plan.org::8::Write the report", "date": "+2d"}"#,
        );
        assert_eq!(p.old, "* TODO Write the report");
        assert_eq!(
            p.new,
            "* TODO Write the report\nSCHEDULED: <2026-11-14 Sat>"
        );
        assert!(
            said.starts_with("Scheduled \"Write the report\" for <2026-11-14 Sat>"),
            "{said}"
        );

        let (said, p) = change_of(
            &plan(),
            r#"{"op": "deadline", "ref": "~/org/plan.org::2::Pay the invoice", "date": "none"}"#,
        );
        assert_eq!(
            p.old,
            "* TODO [#A] Pay the invoice :money:\nDEADLINE: <2026-11-10 Tue>"
        );
        assert_eq!(p.new, "* TODO [#A] Pay the invoice :money:");
        assert!(said.starts_with("Removed the deadline date"), "{said}");

        let same = call(
            &plan(),
            &kw(),
            today(),
            CHANGE,
            r#"{"op": "deadline", "ref": "~/org/plan.org::2::Pay the invoice", "date": "2026-11-10"}"#,
        )
        .unwrap();
        assert!(
            same.proposals.is_empty() && same.text.starts_with("Nothing to change"),
            "{same:?}"
        );
        let err = change_err(
            &plan(),
            r#"{"op": "schedule", "ref": "~/org/plan.org::8::Write the report", "date": "someday"}"#,
        );
        assert!(err.starts_with("date: "), "{err}");
    }

    #[test]
    fn tags_are_added_and_removed() {
        let (said, p) = change_of(
            &plan(),
            r#"{"op": "tag", "ref": "~/org/plan.org::2::Pay the invoice", "tags": ["urgent", "-money"]}"#,
        );
        assert_eq!(p.old, "* TODO [#A] Pay the invoice :money:");
        assert!(
            p.new.starts_with("* TODO [#A] Pay the invoice") && p.new.ends_with(":urgent:"),
            "{p:?}"
        );
        assert!(
            said.starts_with("Tagged \"Pay the invoice\" :urgent:"),
            "{said}"
        );
        let err = change_err(
            &plan(),
            r#"{"op": "tag", "ref": "~/org/plan.org::2::Pay the invoice", "tags": ["two words"]}"#,
        );
        assert_eq!(err, "tags: `two words` is not one tag");
    }

    /// Org's `org-add-note`: into the drawer, after the planning line, the
    /// note's lines under the bullet's text.
    #[test]
    fn a_note_goes_into_the_entrys_log() {
        let (said, p) = change_of(
            &plan(),
            r#"{"op": "add-note", "ref": "~/org/plan.org::4::Standup", "note": "Moved to 10:00.\nAsk Sam first."}"#,
        );
        assert!(
            said.starts_with("Added a note to \"Standup\" in ~/org/plan.org"),
            "{said}"
        );
        assert_eq!(
            p.old,
            "* TODO Standup\nSCHEDULED: <2026-11-11 Wed 09:30 +1d>"
        );
        assert_eq!(
            p.new,
            "* TODO Standup\n\
             SCHEDULED: <2026-11-11 Wed 09:30 +1d>\n\
             \x20 :LOGBOOK:\n\
             \x20 - Note taken on [2026-11-12 Thu 10:00] \\\\\n\
             \x20   Moved to 10:00.\n\
             \x20   Ask Sam first.\n\
             \x20 :END:"
        );
        let err = change_err(
            &plan(),
            r#"{"op": "add-note", "ref": "~/org/plan.org::4::Standup", "note": "  "}"#,
        );
        assert_eq!(err, "note: add-note needs the note's text");
    }

    /// Across files: the copy first, a child of the target, deeper
    /// headings shifted with it; then the removal, taking its line break.
    #[test]
    fn a_refile_across_files_copies_then_removes() {
        let org = Fake(vec![
            ("~/org/inbox.org", "* TODO Call Sam\n** Notes\n* Keep\n"),
            ("~/org/work.org", "* Projects\n** Q3\n* Admin\n"),
        ]);
        let a = call(
            &org,
            &kw(),
            today(),
            CHANGE,
            r#"{"op": "refile", "ref": "~/org/inbox.org::1::Call Sam", "target": "~/org/work.org::1::Projects"}"#,
        )
        .unwrap();
        assert!(
            a.text.starts_with(
                "Filed \"Call Sam\" under \"Projects\", as two edits: the copy into \
                 ~/org/work.org first, then its removal from ~/org/inbox.org."
            ),
            "{}",
            a.text
        );
        assert_eq!(
            a.proposals,
            [
                Proposal {
                    path: "/home/u/org/work.org".into(),
                    old: "** Q3".into(),
                    new: "** Q3\n** TODO Call Sam\n*** Notes".into(),
                },
                Proposal {
                    path: "/home/u/org/inbox.org".into(),
                    old: "* TODO Call Sam\n** Notes\n* Keep".into(),
                    new: "* Keep".into(),
                },
            ]
        );
    }

    #[test]
    fn a_refile_in_one_file_is_one_edit() {
        let org = Fake(vec![(
            "~/t.org",
            "* Inbox\n** TODO Call Sam\n* Work\n** Q3\n",
        )]);
        let (said, p) = change_of(
            &org,
            r#"{"op": "refile", "ref": "~/t.org::2::Call Sam", "target": "~/t.org::3::Work"}"#,
        );
        assert!(
            said.starts_with("Filed \"Call Sam\" under \"Work\" in ~/t.org."),
            "{said}"
        );
        assert_eq!(p.old, "** TODO Call Sam\n* Work\n** Q3");
        assert_eq!(p.new, "* Work\n** Q3\n** TODO Call Sam");

        // To a file: its end, at the top level.
        let (_, p) = change_of(
            &org,
            r#"{"op": "refile", "ref": "~/t.org::2::Call Sam", "target": "~/t.org"}"#,
        );
        assert_eq!(p.new, "* Work\n** Q3\n* TODO Call Sam");
    }

    #[test]
    fn a_refile_the_model_got_wrong_is_refused() {
        let org = Fake(vec![
            ("~/t.org", "* Work\n** Q3\n* Admin\n"),
            ("~/empty.org", ""),
        ]);
        let err = |target: &str| {
            change_err(
                &org,
                &format!(r#"{{"op": "refile", "ref": "~/t.org::1::Work", "target": "{target}"}}"#),
            )
        };
        assert_eq!(
            err("~/t.org::2::Q3"),
            "target: \"Work\" cannot be filed under itself"
        );
        assert!(
            err("~/t.org::9::Lunch").contains("query again"),
            "{}",
            err("~/t.org::9::Lunch")
        );
        assert!(
            err("~/gone.org").starts_with("target: ~/gone.org: "),
            "{}",
            err("~/gone.org")
        );
        assert!(
            err("~/empty.org").contains("the file is empty"),
            "{}",
            err("~/empty.org")
        );
        assert!(
            change_err(&org, r#"{"op": "refile", "ref": "~/t.org::1::Work"}"#)
                .starts_with("target:")
        );
    }

    /// A capture's writes, as `capture_effects` would give them.
    struct Capturing(Fake, Captured);

    impl Org for Capturing {
        fn corpus(&self, tags_search: bool) -> Corpus {
            self.0.corpus(tags_search)
        }
        fn read(&self, path: &str) -> Result<String, String> {
            self.0.read(path)
        }
        fn real_path(&self, path: &str) -> String {
            self.0.real_path(path)
        }
        fn change_env(&self) -> ChangeEnv {
            env()
        }
        fn capture(
            &self,
            template: &str,
            text: &str,
            answers: &[String],
        ) -> Result<Captured, String> {
            assert_eq!((template, text), ("t", "Call the bank"));
            assert_eq!(answers, ["today"]);
            Ok(self.1.clone())
        }
    }

    #[test]
    fn a_capture_proposes_what_the_template_writes() {
        let into = |anchor: Anchor, notes: Vec<String>| {
            Capturing(
                Fake(vec![
                    ("~/org/inbox.org", "* Inbox\n** Old\n"),
                    ("~/org/empty.org", ""),
                ]),
                Captured {
                    writes: vec![(
                        "~/org/inbox.org".into(),
                        anchor,
                        "** TODO Call the bank\n".into(),
                    )],
                    notes,
                    described: "Todo".into(),
                },
            )
        };
        let run = |org: &Capturing| {
            call(
                org,
                &kw(),
                today(),
                CHANGE,
                r#"{"op": "capture", "template": "t", "text": "Call the bank", "answers": ["today"]}"#,
            )
        };
        let a = run(&into(Anchor::End, vec![])).unwrap();
        assert_eq!(
            a.text,
            "Captured with \"Todo\" into ~/org/inbox.org. The user reviews the edit before it \
             is made."
        );
        assert_eq!(
            a.proposals,
            [Proposal {
                path: "/home/u/org/inbox.org".into(),
                old: "** Old".into(),
                new: "** Old\n** TODO Call the bank".into(),
            }]
        );
        let a = run(&into(
            Anchor::Line(1),
            vec!["no Inbox; appended at the end".into()],
        ))
        .unwrap();
        assert_eq!(a.proposals[0].old, "* Inbox");
        assert_eq!(a.proposals[0].new, "* Inbox\n** TODO Call the bank");
        assert!(
            a.text.contains("into ~/org/inbox.org. no Inbox; appended"),
            "{}",
            a.text
        );

        let mut gone = into(Anchor::End, vec![]);
        gone.1.writes[0].0 = "~/org/gone.org".into();
        assert!(run(&gone)
            .unwrap_err()
            .contains("~/org/gone.org, which does not exist"));
        let mut empty = into(Anchor::End, vec![]);
        empty.1.writes[0].0 = "~/org/empty.org".into();
        assert!(run(&empty).unwrap_err().contains("the file is empty"));
        let none = call(
            &plan(),
            &kw(),
            today(),
            CHANGE,
            r#"{"op": "capture", "template": "t"}"#,
        );
        assert_eq!(none.unwrap_err(), "capture: not available here");
        let keyless = call(&plan(), &kw(), today(), CHANGE, r#"{"op": "capture"}"#);
        assert!(keyless.unwrap_err().starts_with("template:"));
        let refless = call(
            &plan(),
            &kw(),
            today(),
            CHANGE,
            r#"{"op": "tag", "tags": ["x"]}"#,
        );
        assert!(refless.unwrap_err().starts_with("ref:"));
    }

    /// Two identical headlines: the edit widens until its old text occurs
    /// once, so it cannot land on the other one.
    #[test]
    fn a_proposal_is_unique_in_its_file() {
        let twins = Fake(vec![("~/t.org", "* TODO Same\n* TODO Same\n")]);
        let (_, p) = change_of(
            &twins,
            r#"{"op": "tag", "ref": "~/t.org::2::Same", "tags": ["b"]}"#,
        );
        assert_eq!(p.old, "* TODO Same\n* TODO Same");
        assert!(
            p.new.starts_with("* TODO Same\n* TODO Same") && p.new.ends_with(":b:"),
            "{p:?}"
        );
    }

    #[test]
    fn a_change_the_model_got_wrong_is_refused() {
        let at = |op: &str, extra: &str| {
            let sep = if extra.is_empty() { "" } else { ", " };
            change_err(
                &plan(),
                &format!(
                    r#"{{"op": "{op}", "ref": "~/org/plan.org::8::Write the report"{sep}{extra}}}"#
                ),
            )
        };
        assert_eq!(
            at("set-state", r#""state": "FINISHED""#),
            "state: `FINISHED` is not one of the TODO keywords (TODO, WAITING, DONE)"
        );
        assert!(at("set-state", "").starts_with("state:"));
        assert!(at("schedule", "").starts_with("date:"));
        assert!(at("tag", r#""tags": []"#).starts_with("tags:"));
        assert!(at("delete", "").starts_with("op:"));
        let stale = change_err(
            &plan(),
            r#"{"op": "tag", "ref": "~/org/plan.org::8::Write the summary", "tags": ["x"]}"#,
        );
        assert!(stale.contains("query again"), "{stale}");
    }

    /// Every profile's tools are org's own, `lattice.read` or `web.*` (what
    /// a plugin profile may select, design §7), name only tools org offers,
    /// and its prompt carries the shared rules.
    #[test]
    fn the_profiles_select_only_what_a_plugin_may() {
        let ours: Vec<String> = tools().iter().map(|t| format!("org.{}", t.name)).collect();
        let all = profiles();
        let names: Vec<_> = all.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "org",
                "org-plan",
                "org-review",
                "org-report",
                "org-research"
            ]
        );
        for p in &all {
            for tool in p.tools.split(',').map(str::trim) {
                assert!(
                    tool == "org.*"
                        || tool == "lattice.read"
                        || tool == "web.*"
                        || ours.iter().any(|t| t == tool),
                    "{}: `{tool}`",
                    p.name
                );
            }
            assert!(
                p.system.contains("Pass refs back exactly as given"),
                "{}",
                p.name
            );
            assert!(!p.system.contains("\n\n\n"), "{}", p.name);
            assert!(p.description.len() < 80, "{}", p.name);
        }
        let report = all.iter().find(|p| p.name == "org-report").unwrap();
        assert!(
            !report.tools.contains("org.change"),
            "a report changes nothing"
        );
    }

    #[test]
    fn the_tool_list_carries_a_schema_from_each_args_struct() {
        let tools = tools();
        let names: Vec<_> = tools.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            ["agenda", "query", "entry", "clock", "notes", "change"]
        );
        let proposing: Vec<_> = tools
            .iter()
            .filter(|t| t.proposes)
            .map(|t| t.name)
            .collect();
        assert_eq!(proposing, ["change"], "only org.change proposes edits");
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

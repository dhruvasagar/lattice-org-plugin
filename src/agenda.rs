//! OM.A2 — what makes a headline an agenda row, and where it sorts.
//!
//! The host walks files and builds excerpts; everything org about the agenda
//! is here. Given one file's text it answers: which headlines are dated, on
//! what day, in what order, and under which date heading.
//!
//! ## The tree, and why that argument stopped holding (OT.3)
//!
//! This section used to justify line logic over the parse tree: "the host
//! hands `scan` a `string`, not a `borrow<document>` and not a tree. There is
//! no parse of another project's file to consult — it has never been opened,
//! and parsing every org file in a project to build an agenda would be the
//! most expensive thing the plugin does."
//!
//! The premise is now false — `scan` receives `option<borrow<tree-snapshot>>`
//! beside the text. The cost claim stays true: parsing IS the expensive part
//! (~1–2 ms per file against a 217 ns text copy,
//! `benches/agenda_scan_input.rs`). OT.3 pays it for one thing the line logic
//! cannot do at any price.
//!
//! **What that thing is, stated precisely, because the first three guesses were
//! wrong.** A `:PROPERTIES:` drawer between a headline and its `SCHEDULED:`
//! line is NOT a counterexample — org's grammar puts `plan` before
//! `property_drawer`, so the planning line genuinely does come first.
//! `DEADLINE:` and `SCHEDULED:` on separate lines is NOT one either — org's
//! planning info is a single line. The old `lines[i + 1]` assumption matches
//! org's real grammar.
//!
//! What it cannot match is **context**: `* TODO ` at the start of a line inside
//! a `#+BEGIN_SRC` block is example text, not a headline, and no line matcher
//! can tell, because the fact is not on the line. The text scan invents a
//! phantom agenda row there; the tree does not. That is the win, and it is
//! narrower than "the line logic was buggy" — it is "the line logic cannot see
//! structure, and one day the structure will matter".
//!
//! Structure now comes from the tree, characters from the text. [`scan_file`]
//! survives for a host with no grammar for the file, and carries the phantom
//! with it — pinned by
//! `the_text_fallback_cannot_tell_a_source_block_from_a_headline`.
//!
//! ## What counts as a row
//!
//! An open headline (one whose TODO keyword is not a *done* keyword) that
//! carries **either** a date **or** a TODO keyword.
//!
//! A date comes from one of three places, in priority order:
//!
//!   1. `DEADLINE: <2026-08-25 Tue>` on the planning line,
//!   2. `SCHEDULED: <2026-08-25 Tue>` on the planning line,
//!   3. a plain **active** timestamp `<2026-08-25 Tue>` on the headline itself.
//!
//! An **inactive** stamp `[2026-08-25 Tue]` never dates a row — that is what
//! inactive means in org, and treating it as a date would drag every logbook
//! entry and every `CLOSED:` line into the agenda.
//!
//! **AS.1 made the date optional**, and it is the largest behavioural change
//! this module has had. A `* TODO Write the thing` with no plan was previously
//! not a row under any configuration, so the most ordinary line in anyone's
//! org file could not reach the view whose job is to show you your tasks. It
//! is an undated row now, and the "Unscheduled" section displays it.
//!
//! A headline with NO keyword and no date is still not a row: that is prose
//! structure, and admitting it would make the agenda a table of contents. A
//! headline with a *done* keyword is not a row, dated or not — org hides
//! completed entries by default, and an agenda that lists what you finished is
//! a log, not a plan.
//!
//! ## Sections (AS.1)
//!
//! A row is a CANDIDATE. Each [`Section`] decides whether it wants it, so one
//! headline can produce several `entry` values — an overdue `[#A]` TODO is
//! emitted three times, under Overdue, under its date, and under the priority
//! block. That is a dashboard behaving as intended rather than a duplicate:
//! `append_excerpts` does not dedup, so two excerpts over one source range are
//! simply two rows of one view.
//!
//! **The mechanism needs no ABI change and no host change**, which is why it
//! lives entirely here. The host stable-sorts on `sort-key`, groups rows into
//! runs of equal `group`, and titles the first row of each run with `label` —
//! all three opaque and guest-owned, as `scanned-excerpt-source.wit` says.
//! Making a section's rows a contiguous run is therefore just "give them the
//! same leading digits", which is what [`sort_key_in_section`] does.
//!
//! ## A row is one line (OA.1)
//!
//! `end_line == line`, always. The agenda is an index: one line per entry,
//! carrying the keyword, priority, title and tags, and you press `<CR>` to go
//! to the source when you want the rest.
//!
//! It used to run to the planning line so the row showed `SCHEDULED: <…>`
//! beneath its headline. That was a reasonable default when a date had
//! nowhere else to appear, but it makes every dated row two lines tall in a
//! view whose whole job is to be scannable — and the date is what the row is
//! GROUPED under, so it was being shown twice. The plan node is still read;
//! only what the excerpt spans changed.

use crate::lattice::plugin_host::tree_sitter::Node;
use crate::timestamp::{self, Stamp};
use crate::todo;
use crate::tree;

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

/// When a row is due, and where that date came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dated {
    /// Days since the Unix epoch.
    pub day: i64,
    pub kind: Kind,
}

/// One agenda row, before it becomes one or more WIT `entry` values.
///
/// AS.1: "one or more". A row is a candidate, and each SECTION decides whether
/// it wants it — so a `[#A]` item scheduled for yesterday is emitted three
/// times, under Overdue, under its date, and under the priority block. The
/// host does not dedup excerpts, which is what makes that legal rather than a
/// hack: two excerpts over the same source range are two rows of one view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// 0-based line of the headline.
    pub line: u32,
    /// 0-based last line of the row's excerpt, inclusive — the planning line
    /// when there is one, else the headline itself.
    pub end_line: u32,
    /// The date this row is filed under, or `None` for a TODO carrying no plan
    /// and no inline stamp.
    ///
    /// AS.1 made this optional and that is the slice's largest behavioural
    /// change. Undated rows were previously not rows at all — `row_for_section`
    /// returned `None` the moment it found no stamp — so a `* TODO Write the
    /// thing` with no `SCHEDULED:` could never appear in the agenda under any
    /// circumstances. A whole class of task was invisible to the view whose
    /// job is to show you your tasks.
    pub date: Option<Dated>,
    /// `[#A]`, if the headline carries one.
    pub priority: Option<char>,
    /// The TODO keyword the headline carries, if any. `None` means a plain
    /// dated headline — an appointment rather than a task.
    ///
    /// Needed because a section filters on it: "unscheduled TODOs" must not
    /// sweep up every undated plain headline in the corpus, which is most of
    /// them.
    pub keyword: Option<String>,
}

impl Row {
    /// The day this row sorts under. Undated rows sort as if today, so they
    /// land beside — not centuries away from — everything else in whatever
    /// section takes them.
    fn sort_day(&self, today: i64) -> i64 {
        self.date.map(|d| d.day).unwrap_or(today)
    }

    fn kind_rank(&self) -> i64 {
        // Undated ranks after every dated kind: within a section that mixes
        // them, a thing with a date is the more urgent thing.
        self.date.map(|d| d.kind.rank()).unwrap_or(3)
    }
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

/// Every agenda row in one file, resolved from its **parse tree**.
///
/// OT.3. `plan` is a **field on the section**, so this asks the grammar which
/// plan belongs to this headline rather than assuming it is the next line —
/// and, more importantly, it only ever sees real sections, so example org
/// inside a `#+BEGIN_SRC` block cannot become a row. See the module header for
/// why that context case is the actual win and the more obvious candidates are
/// not.
///
/// Structure comes from the tree; characters come from `text`. The seam
/// exposes node kinds and ranges but no node text, and reading a TODO keyword
/// through one boundary crossing per headline would cost far more than the
/// whole-file string the host already hands over — see `scanned-excerpt-source.wit`.
pub fn scan_tree(root: &Node, text: &str, keywords: &Keywords) -> Vec<Row> {
    let lines: Vec<&str> = text.lines().collect();
    let mut rows = Vec::new();
    walk_sections(root, &lines, keywords, &mut rows);
    rows
}

/// Recurse through `(section)` nodes, which nest: a subsection is a `section`
/// child of its parent, so a flat pass over the root's children would see only
/// top-level headlines and quietly drop every nested one.
fn walk_sections(node: &Node, lines: &[&str], keywords: &Keywords, rows: &mut Vec<Row>) {
    for i in 0..node.named_child_count() {
        let Some(child) = node.named_child(i) else {
            continue;
        };
        if child.kind() == "section" {
            if let Some(row) = row_for_section(&child, lines, keywords) {
                rows.push(row);
            }
        }
        // Recurse either way: a `section` holds its subsections, and a file may
        // open with leading content before the first headline, so sections are
        // not always direct children of the root.
        walk_sections(&child, lines, keywords, rows);
    }
}

/// The agenda row one section contributes, if it contributes one.
fn row_for_section(section: &Node, lines: &[&str], keywords: &Keywords) -> Option<Row> {
    let headline = section.child_by_field("headline")?;
    let headline_line = headline.byte_range().start.line;
    let headline_text = lines.get(headline_line as usize).copied()?;

    let parsed = todo::parse(headline_text, &keywords.all)?;
    if keywords.is_done(parsed.keyword) {
        return None;
    }

    // The section's OWN plan, by field — not "the next line", which is the
    // assumption this migration exists to delete.
    //
    // OA.1 deleted the plan's EXTENT along with the row that spanned it; the
    // plan is still read for its date.
    let plan = section.child_by_field("plan");

    // OT.5: the plan's entries, read as nodes. See `plan_date`.
    let from_plan = plan.as_ref().and_then(|p| plan_date(p, lines));
    let dated = match from_plan {
        Some((kind, stamp)) => Some((kind, stamp, true)),
        // No dated plan entry: the headline's own inline timestamp, which the
        // grammar does NOT model — see `plan_date`.
        None => timestamp::first_stamp(headline_text)
            .filter(|s| s.active)
            .map(|stamp| (Kind::Timestamp, stamp, false)),
    };

    // AS.1: no date is no longer no row. Before this, an undated headline
    // returned `None` here and left the agenda — so `* TODO Write the thing`
    // was unreachable from the view whose job is to show your tasks. It is a
    // row now, with `date: None`, and the SECTIONS decide whether anything
    // wants it. A headline with neither a date NOR a keyword is still not a
    // row: that is ordinary prose structure, and admitting it would make the
    // agenda a table of contents.
    if dated.is_none() && parsed.keyword.is_none() {
        return None;
    }

    Some(Row {
        line: headline_line,
        // OA.1: one line per entry. The plan is still parsed — it is where the
        // date comes from — but the excerpt does not span down to it.
        end_line: headline_line,
        date: dated.map(|(kind, stamp, _)| Dated {
            day: timestamp::epoch_day(stamp.year, stamp.month, stamp.day),
            kind,
        }),
        priority: parsed.priority,
        keyword: parsed.keyword.map(str::to_string),
    })
}

/// The date a section's `plan` carries, from the plan's own nodes.
///
/// ## What the tree gives here, and what it does not (OT.5)
///
/// `grammar.js` models a planning line as `plan: repeat1(entry)` where an
/// `entry` is `name?: entry_name, ':', timestamp: timestamp`. So the keyword and
/// the stamp are both named nodes, and neither has to be found by scanning: the
/// old path did `line.trim_start().strip_prefix("SCHEDULED:")` and then walked
/// bytes looking for a `<` or `[`.
///
/// **The timestamp node exists ONLY here.** A stamp in a headline
/// (`* TODO Task <2026-09-05 Sat>`) parses as `item: (item (expr) (expr) …)`,
/// and one in body text as `(paragraph (expr) …)` — undifferentiated tokens in
/// both cases. That is why the headline's inline date, and `timestamp.rs`'s
/// whole `<C-a>` / `<C-x>` stepping path, stay on the text scanner: there is no
/// node to migrate them to, and half-migrating would leave two parsers of one
/// construct where there is now one.
///
/// **A plan is one line.** `plan` is `seq(repeat1(entry), _eol)`, so several
/// entries on one line are several `entry` nodes and a `SCHEDULED:` written on a
/// SECOND line is body text, not a plan. That matches org: `org-element` parses
/// a single planning element from the line following the headline.
///
/// DEADLINE outranks SCHEDULED, as before — an entry with both is a deadline
/// that happens to be scheduled, and org sorts it as the deadline.
///
/// Names are matched case-sensitively because org's are: the grammar accepts
/// `scheduled:` as an `entry_name` and org does not treat it as one.
/// `CLOSED:` is deliberately unreachable — it is inactive, and a record of the
/// past rather than a plan.
fn plan_date(plan: &Node, lines: &[&str]) -> Option<(Kind, Stamp)> {
    let mut best: Option<(Kind, Stamp)> = None;
    for i in 0..plan.named_child_count() {
        let Some(entry) = plan.named_child(i) else {
            continue;
        };
        if entry.kind() != "entry" {
            continue;
        }
        // An entry with no name is a bare timestamp on the planning line. org
        // gives it no planning meaning, and neither did the prefix match this
        // replaces, so it contributes nothing rather than a guessed kind.
        let Some(name) = entry
            .child_by_field("name")
            .as_ref()
            .and_then(|n| tree::node_text(lines, n))
        else {
            continue;
        };
        let kind = match name {
            "DEADLINE" => Kind::Deadline,
            "SCHEDULED" => Kind::Scheduled,
            _ => continue,
        };
        let Some(stamp) = entry
            .child_by_field("timestamp")
            .as_ref()
            .and_then(|n| tree::node_text(lines, n))
            // Characters from the text: the node says where the stamp is, and
            // `first_stamp` reads what it says. The seam exposes no node text,
            // and the date's own `date` / `time` fields would still need
            // parsing — so one parse of a bounded slice is the cheap answer.
            .and_then(timestamp::first_stamp)
        else {
            continue;
        };
        if !stamp.active {
            continue;
        }
        // Lower `Kind` wins, which is the same precedence `Kind`'s `Ord`
        // already encodes for the cross-file sort.
        if best.as_ref().is_none_or(|(k, _)| kind < *k) {
            best = Some((kind, stamp));
        }
    }
    best
}

/// Every agenda row in one file's text, without a parse tree.
///
/// The fallback for a host that had no grammar for this file
/// (`scanned-excerpt-source.wit` keeps a source independent of the `language` seam). For
/// org itself [`scan_tree`] is the real path — this one carries the old
/// line-offset assumption, and its bug, and is reached only when there is
/// nothing better.
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
        let dated = date_for(line, planning);
        // AS.1, mirroring `row_for_section`: undated is a row when it carries
        // a keyword, and not a row otherwise. The two paths must agree on what
        // IS a row or the agenda would gain and lose whole sections depending
        // on whether the host happened to have an org grammar loaded.
        if dated.is_none() && headline.keyword.is_none() {
            continue;
        }
        rows.push(Row {
            line: i as u32,
            // OA.1: one line, matching `row_for_section`. The two paths must
            // agree on the shape of a row as well as on what IS one.
            end_line: i as u32,
            date: dated.map(|(kind, stamp, _)| Dated {
                day: timestamp::epoch_day(stamp.year, stamp.month, stamp.day),
                kind,
            }),
            priority: headline.priority,
            keyword: headline.keyword.map(str::to_string),
        });
    }
    rows
}

/// The date a headline carries, and whether it came from the planning line —
/// the TEXT path's answer. [`plan_date`] is the tree's.
///
/// `planning` is the single line below the headline. Both keywords are tried
/// against it because org allows both on one line, and DEADLINE first because
/// an entry with both is a deadline that happens to be scheduled.
///
/// This is where the prefix match survives, and with it the flaw OT.5 removed
/// from the tree path: `strip_prefix` requires the keyword at the START of the
/// line, so `SCHEDULED: <b> DEADLINE: <a>` reports the SCHEDULED date. The
/// grammar sees two `entry` nodes and does not care which came first. Left
/// alone here because this path runs only when there is no grammar for the
/// file at all, and a second parser fixed to agree with a tree it cannot see is
/// the thing this phase exists to stop writing.
fn date_for(headline: &str, planning: &str) -> Option<(Kind, Stamp, bool)> {
    // DEADLINE first, across the WHOLE plan before falling back to SCHEDULED:
    // an entry with both is a deadline that happens to be scheduled, and org
    // sorts it as the deadline — which stays true when they are on two lines.
    for (keyword, kind) in [
        ("DEADLINE:", Kind::Deadline),
        ("SCHEDULED:", Kind::Scheduled),
    ] {
        for line in planning.lines() {
            if let Some(rest) = line.trim_start().strip_prefix(keyword) {
                if let Some(stamp) = timestamp::first_stamp(rest) {
                    if stamp.active {
                        return Some((kind, stamp, true));
                    }
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
/// Section dominates; then day; within a day, kind; within a kind, priority.
/// Packed into one `i64` because the ABI carries exactly one number — and
/// packed with wide multipliers so a future tiebreaker has room rather than
/// needing an ABI change to add one.
///
/// **Section rank in the high digits is the whole of the multi-section
/// mechanism** (AS.1). The host stable-sorts on this number and knows nothing
/// else about ordering, so making a section's rows sort as one contiguous run
/// is exactly "give them all the same leading digits". No ABI change, no host
/// change: `sort-key` was always documented as "the guest owns what it means".
///
/// `A` sorts before `B` before "no priority", which is org's order: an
/// unprioritised item is not urgent, it is unranked.
///
/// `DAY_BIAS` keeps the day term non-negative so a pre-epoch date cannot
/// borrow into the section digits and file a 1969 row under the wrong
/// section. It covers ±1000 years, well past any date org's parser accepts.
const DAY_BIAS: i64 = 400_000;

pub fn sort_key_in_section(row: &Row, section_rank: i64, today: i64) -> i64 {
    let priority = match row.priority {
        Some(c) if c.is_ascii_alphabetic() => (c.to_ascii_uppercase() as i64) - ('A' as i64),
        _ => 100,
    };
    section_rank * 10_000_000_000_000
        + (row.sort_day(today) + DAY_BIAS) * 10_000
        + row.kind_rank() * 1_000
        + priority
}

/// Which rows a section takes. Deliberately DATA rather than a predicate
/// closure: AS.2 makes this set writable from `lattice.toml` and from
/// `init.rs`, and a filter that is data can be parsed from a config file
/// while a closure cannot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Filter {
    pub when: When,
    /// Only rows carrying a not-done TODO keyword. `false` also admits plain
    /// dated headlines — appointments, which belong in a date view and not in
    /// a task list.
    pub todo_only: bool,
    /// Highest priority letter admitted, inclusive: `Some('A')` takes only
    /// `[#A]`, `Some('B')` takes `[#A]` and `[#B]`.
    pub min_priority: Option<char>,
}

/// The date window a section admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum When {
    /// Dated strictly before today. Org surfaces these hardest, because a
    /// missed deadline the view stays quiet about is the failure the tool
    /// exists to prevent.
    Overdue,
    /// `today ..= today + n`. `Days(0)` is emacs's daily agenda, `Days(6)` its
    /// week.
    Days(u32),
    /// No date at all — the class that was invisible before AS.1.
    Undated,
    /// Every row, dated or not.
    Any,
}

impl When {
    /// Does this window group its rows by DATE (a header per day, as the
    /// classic agenda does) or as one block under the section's own title?
    ///
    /// Date-grouping only makes sense where every row has a date and the dates
    /// differ, so it follows from the window rather than being a second knob
    /// the two could disagree on.
    fn groups_by_date(self) -> bool {
        matches!(self, When::Overdue | When::Days(_))
    }

    fn admits(self, row: &Row, today: i64) -> bool {
        match (self, row.date) {
            (When::Any, _) => true,
            (When::Undated, None) => true,
            (When::Undated, Some(_)) => false,
            (_, None) => false,
            (When::Overdue, Some(d)) => d.day < today,
            (When::Days(n), Some(d)) => d.day >= today && d.day <= today + i64::from(n),
        }
    }
}

/// One block of the agenda: a title and the rows it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub filter: Filter,
}

impl Section {
    fn admits(&self, row: &Row, keywords: &Keywords, today: i64) -> bool {
        if !self.filter.when.admits(row, today) {
            return false;
        }
        if self.filter.todo_only {
            // A not-done keyword. `row_for_section` already dropped DONE rows,
            // so the test that matters here is "carries a keyword at all".
            match row.keyword.as_deref() {
                Some(k) if !keywords.is_done(Some(k)) => {}
                _ => return false,
            }
        }
        if let Some(min) = self.filter.min_priority {
            let Some(p) = row.priority else {
                return false;
            };
            if p.to_ascii_uppercase() > min.to_ascii_uppercase() {
                return false;
            }
        }
        true
    }
}

/// The shipped section set, in render order.
///
/// Chosen against emacs org-agenda's own defaults and the composite
/// `org-agenda-custom-commands` dashboards people actually write:
///
/// 1. **Overdue** first, because it is the one org surfaces hardest and the
///    one a user most needs to see before deciding what to do today.
/// 2. **The dated agenda**, which is what this view was before AS.1, scoped to
///    a span rather than to every date the corpus happens to contain.
/// 3. **Unscheduled TODOs**, previously invisible entirely.
/// 4. **Priority A**, a standing "what matters" block that deliberately
///    overlaps the three above — a row appearing twice is the point of a
///    dashboard, not a bug in one.
///
/// AS.2 makes this the FALLBACK rather than the law: a user's own section list
/// replaces it wholesale.
pub fn default_sections(span: u32) -> Vec<Section> {
    vec![
        Section {
            title: "Overdue".to_string(),
            filter: Filter {
                when: When::Overdue,
                todo_only: true,
                min_priority: None,
            },
        },
        Section {
            title: "Agenda".to_string(),
            filter: Filter {
                when: When::Days(span),
                todo_only: false,
                min_priority: None,
            },
        },
        Section {
            title: "Unscheduled".to_string(),
            filter: Filter {
                when: When::Undated,
                todo_only: true,
                min_priority: None,
            },
        },
        Section {
            title: "Priority A".to_string(),
            filter: Filter {
                when: When::Any,
                todo_only: true,
                min_priority: Some('A'),
            },
        },
    ]
}

/// What one row contributes to the view: `(group_key, label, sort_key)` per
/// section that takes it, in section order.
///
/// The group key is prefixed with the section's rank even when the section
/// groups by date, and that prefix is load-bearing: two sections can both
/// contain 2026-08-25, and an unprefixed ISO key would make the host see one
/// run spanning a section boundary and title only the first of them.
pub fn entries_for_row(
    row: &Row,
    sections: &[Section],
    keywords: &Keywords,
    today: i64,
) -> Vec<(String, String, i64)> {
    let mut out = Vec::new();
    for (rank, section) in sections.iter().enumerate() {
        if !section.admits(row, keywords, today) {
            continue;
        }
        let rank = rank as i64;
        let (key, label) = match (section.filter.when.groups_by_date(), row.date) {
            (true, Some(d)) => (
                format!("{rank}:{}", group_key(d.day)),
                format!("{} — {}", section.title, group_label(d.day, today)),
            ),
            // One block under the section's own title. Also the honest answer
            // for a date-grouping section handed an undated row, which
            // `When::admits` makes unreachable but which must not silently
            // produce a header reading the epoch if that ever changes.
            _ => (format!("{rank}:"), section.title.clone()),
        };
        out.push((key, label, sort_key_in_section(row, rank, today)));
    }
    out
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

    /// OT.5's twin: what the TEXT path answers for the same input the tree
    /// path gets right, so the difference is documented rather than only the
    /// good half asserted (`a_deadline_outranks_a_scheduled_written_before_it`
    /// in `tests/org_agenda.rs` is the other half).
    ///
    /// `strip_prefix` requires the keyword at the start of the trimmed line, so
    /// with SCHEDULED written first the deadline is never seen and the entry is
    /// filed on the later date. The grammar sees two `entry` nodes and has no
    /// opinion about their order.
    #[test]
    fn the_text_path_reads_the_first_keyword_on_the_plan_line_not_the_strongest() {
        let rows =
            scan("* TODO Ship it\n  SCHEDULED: <2026-08-30 Sun> DEADLINE: <2026-08-26 Wed>\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].date.expect("dated").kind,
            Kind::Scheduled,
            "the text path takes the keyword it finds first"
        );
        assert_eq!(
            rows[0].date.expect("dated").day,
            timestamp::epoch_day(2026, 8, 30),
            "...and with it the later date"
        );
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
    fn a_scheduled_headline_is_a_one_line_row_dated_by_its_planning_line() {
        let rows = scan("* TODO Ship it\n  SCHEDULED: <2026-08-25 Tue>\nbody\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].line, 0);
        // OA.1: the planning line DATES the row without being part of it.
        // This asserted `end_line == 1` — the excerpt spanning down to show
        // `SCHEDULED:` — which made every dated row two lines tall.
        assert_eq!(
            rows[0].end_line, 0,
            "the planning line is read for its date, not composed into the row"
        );
        assert_eq!(rows[0].date.expect("dated").kind, Kind::Scheduled);
        assert_eq!(
            rows[0].date.expect("dated").day,
            timestamp::epoch_day(2026, 8, 25)
        );
    }

    /// Both present: org sorts it as the deadline, so the deadline wins.
    #[test]
    fn a_deadline_outranks_a_scheduled_date_on_the_same_headline() {
        let rows =
            scan("* TODO Ship it\n  DEADLINE: <2026-08-20 Thu> SCHEDULED: <2026-08-25 Tue>\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date.expect("dated").kind, Kind::Deadline);
        assert_eq!(
            rows[0].date.expect("dated").day,
            timestamp::epoch_day(2026, 8, 20)
        );
    }

    #[test]
    fn an_active_timestamp_on_the_headline_is_a_row() {
        let rows = scan("* Meeting <2026-08-25 Tue 10:30>\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date.expect("dated").kind, Kind::Timestamp);
        assert_eq!(
            rows[0].end_line, 0,
            "no planning line, so the excerpt is the headline alone"
        );
    }

    /// The distinction the whole inactive-stamp syntax exists for. Counting
    /// these would drag every logbook line and every `CLOSED:` into the view.
    ///
    /// AS.1 narrowed what this can assert, and the narrowing is the feature.
    /// An inactive stamp still contributes no DATE — but a headline carrying a
    /// TODO keyword is now an undated row rather than nothing, so the second
    /// case below is a row whose `date` is `None`. The claim being pinned is
    /// therefore "an inactive stamp never dates a row", which is what the
    /// syntax means; "an inactive stamp means no row at all" was only ever
    /// true because undated rows did not exist.
    #[test]
    fn an_inactive_timestamp_never_dates_a_row() {
        // No keyword and no active stamp: still not a row at all.
        assert!(scan("* Meeting [2026-08-25 Tue]\n").is_empty());

        // A keyword makes it a row — but `CLOSED:` did not date it.
        let rows = scan("* TODO Ship it\n  CLOSED: [2026-08-20 Thu]\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].date, None,
            "an inactive stamp must not date the row it sits under"
        );
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

    /// AS.1 inverted this, and it is the slice's headline behaviour change.
    ///
    /// An undated TODO used to be invisible to the agenda under every
    /// configuration — so `* TODO Write the thing`, the single most ordinary
    /// line in anyone's org file, could not reach the view whose job is to
    /// show you your tasks. It is a row now, with no date, and the
    /// "Unscheduled" section is what displays it.
    ///
    /// The old rule survives for headlines with NO keyword: those are prose
    /// structure, and admitting them would turn the agenda into a table of
    /// contents.
    #[test]
    fn an_undated_todo_is_a_row_but_an_undated_plain_headline_is_not() {
        let rows = scan("* TODO Ship it\nbody\n* Another\n");
        assert_eq!(rows.len(), 1, "the TODO is a row, got {rows:?}");
        assert_eq!(rows[0].line, 0);
        assert_eq!(rows[0].date, None, "and it carries no date");
        assert_eq!(rows[0].keyword.as_deref(), Some("TODO"));
    }

    /// Only the line immediately below. A `SCHEDULED:` deeper in the body
    /// belongs to nothing, and one below a CHILD headline belongs to the
    /// child — dating the parent with it would put the wrong entry in the
    /// agenda and jump the user to the wrong line.
    #[test]
    fn only_the_line_below_the_headline_is_a_planning_line() {
        // AS.1: the parent is now an undated row rather than no row, so the
        // claim is that the stray `SCHEDULED:` did not DATE it — which is the
        // thing this test was always about.
        let rows = scan("* TODO Parent\n\n  SCHEDULED: <2026-08-25 Tue>\n");
        assert_eq!(rows.len(), 1, "got {rows:?}");
        assert_eq!(
            rows[0].date, None,
            "a blank line ends the planning position, so nothing dated it"
        );

        let rows = scan("* TODO Parent\n** TODO Child\n  SCHEDULED: <2026-08-25 Tue>\n");
        assert_eq!(rows.len(), 2, "both headlines are rows; got {rows:?}");
        assert_eq!(rows[0].line, 0);
        assert_eq!(rows[0].date, None, "the PARENT is undated");
        assert_eq!(rows[1].line, 1, "the CHILD is the dated row");
        assert_eq!(
            rows[1].date.expect("child is dated").day,
            timestamp::epoch_day(2026, 8, 25)
        );
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
            date: Some(Dated { day, kind }),
            priority,
            keyword: Some("TODO".to_string()),
        };
        // AS.1 packs a section rank above the day term. Rank 0 throughout
        // here: this test is about ordering WITHIN a section, and the
        // cross-section ordering has its own test below.
        let sort_key = |r: &Row| sort_key_in_section(r, 0, 0);
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
    ///
    /// AS.1 raised the stakes. A section rank now sits ABOVE the day term, so
    /// a negative day does not merely mis-sort within a section: it borrows
    /// into the rank digits and files a 1969 row under a different section
    /// entirely. `DAY_BIAS` is what stops that, and the last assertion is the
    /// one that catches its removal.
    #[test]
    fn sort_keys_stay_ordered_for_dates_before_the_epoch() {
        let row = |y, m, d| Row {
            line: 0,
            end_line: 0,
            date: Some(Dated {
                day: timestamp::epoch_day(y, m, d),
                kind: Kind::Timestamp,
            }),
            priority: None,
            keyword: Some("TODO".to_string()),
        };
        let sort_key = |r: &Row| sort_key_in_section(r, 0, 0);
        assert!(sort_key(&row(1969, 12, 31)) < sort_key(&row(1970, 1, 1)));
        assert!(sort_key(&row(1900, 1, 1)) < sort_key(&row(1969, 12, 31)));

        // A pre-epoch row in section 1 still sorts after EVERY row of section
        // 0, however far in the future those are.
        assert!(
            sort_key_in_section(&row(2999, 12, 31), 0, 0)
                < sort_key_in_section(&row(1900, 1, 1), 1, 0),
            "section rank must dominate the day term in both directions"
        );
    }

    // ── sections (AS.1) ─────────────────────────────────────────────────

    /// **The shipped default set, pinned by name and in order.**
    ///
    /// Every other section test asserts BEHAVIOUR — that a window admits the
    /// rows it says it does — which is the right shape for a filter but leaves
    /// the actual default configuration asserted only in aggregate. This one
    /// states it: four blocks, these titles, this order, these filters. It is
    /// the thing a user reads in `doc/org.md` and the thing
    /// `agenda_sections::resolve` falls back to, so a change here is a change
    /// to what everyone's agenda looks like and should have to be typed twice.
    #[test]
    fn the_shipped_default_set_is_these_four_blocks_in_this_order() {
        let s = default_sections(7);
        let shape: Vec<_> = s
            .iter()
            .map(|s| {
                (
                    s.title.as_str(),
                    s.filter.when,
                    s.filter.todo_only,
                    s.filter.min_priority,
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                // Past-dated and not done. First, because a missed deadline
                // the view stays quiet about is the failure org exists to
                // prevent.
                ("Overdue", When::Overdue, true, None),
                // Today through `org.agenda-span`. NOT `todo_only`: a plain
                // dated headline is an appointment, and a calendar that hides
                // your appointments is not one.
                ("Agenda", When::Days(7), false, None),
                // TODOs with no date — invisible to the agenda entirely
                // before AS.1.
                ("Unscheduled", When::Undated, true, None),
                // `[#A]`, whenever it is due. Deliberately overlaps the three
                // above: a row appearing twice is what a dashboard is for.
                ("Priority A", When::Any, true, Some('A')),
            ]
        );
        // The span is the option's, not a constant baked in beside it.
        assert_eq!(default_sections(0)[1].filter.when, When::Days(0));
    }

    fn kw() -> Keywords {
        Keywords::from_spec("TODO NEXT | DONE")
    }

    fn r(day: Option<i64>, priority: Option<char>, keyword: Option<&str>) -> Row {
        Row {
            line: 0,
            end_line: 0,
            date: day.map(|day| Dated {
                day,
                kind: Kind::Scheduled,
            }),
            priority,
            keyword: keyword.map(str::to_string),
        }
    }

    /// The four shipped sections, each taking what it says it takes.
    #[test]
    fn each_default_section_admits_exactly_its_own_rows() {
        let today = 20_000;
        let s = default_sections(7);
        let titles = |row: &Row| -> Vec<String> {
            entries_for_row(row, &s, &kw(), today)
                .into_iter()
                .enumerate()
                .map(|(_, (key, _, _))| key.split(':').next().unwrap().to_string())
                .collect()
        };

        // Yesterday's TODO: Overdue (0) only — it is outside the forward span.
        assert_eq!(titles(&r(Some(today - 1), None, Some("TODO"))), ["0"]);
        // Today's TODO: the dated section (1).
        assert_eq!(titles(&r(Some(today), None, Some("TODO"))), ["1"]);
        // Day 7 is inside a span of 7; day 8 is outside and lands nowhere.
        assert_eq!(titles(&r(Some(today + 7), None, Some("TODO"))), ["1"]);
        assert!(titles(&r(Some(today + 8), None, Some("TODO"))).is_empty());
        // Undated TODO: Unscheduled (2) — the class AS.1 made visible.
        assert_eq!(titles(&r(None, None, Some("TODO"))), ["2"]);
        // A plain dated headline is an appointment: the date section takes it,
        // the task sections do not.
        assert_eq!(titles(&r(Some(today), None, None)), ["1"]);
    }

    /// A row landing in several sections is the point of a dashboard, and the
    /// host permits it because `append_excerpts` does not dedup.
    #[test]
    fn one_row_can_appear_in_several_sections() {
        let today = 20_000;
        let s = default_sections(7);
        // Overdue, a TODO, and `[#A]`: Overdue + Priority A. Not the dated
        // section — it is in the past.
        let out = entries_for_row(
            &r(Some(today - 3), Some('A'), Some("TODO")),
            &s,
            &kw(),
            today,
        );
        let ranks: Vec<&str> = out
            .iter()
            .map(|(k, _, _)| k.split(':').next().unwrap())
            .collect();
        assert_eq!(ranks, ["0", "3"], "overdue and priority-A, got {out:?}");

        // Every emitted key sorts in its own section's band, so the host's
        // stable sort cannot interleave them.
        assert!(out[0].2 < out[1].2);
    }

    /// `min_priority` is a CEILING on the letter: `B` admits A and B.
    #[test]
    fn min_priority_admits_everything_at_least_that_urgent() {
        let today = 20_000;
        let s = vec![Section {
            title: "Important".to_string(),
            filter: Filter {
                when: When::Any,
                todo_only: true,
                min_priority: Some('B'),
            },
        }];
        let took = |p: Option<char>| {
            !entries_for_row(&r(None, p, Some("TODO")), &s, &kw(), today).is_empty()
        };
        assert!(took(Some('A')));
        assert!(took(Some('B')));
        assert!(!took(Some('C')));
        assert!(!took(None), "unprioritised is unranked, not urgent");
    }

    /// Two sections can both contain the same DATE, and the group key must
    /// keep them apart — otherwise the host sees one run spanning a section
    /// boundary and titles only the first of them, silently merging the two.
    #[test]
    fn group_keys_do_not_collide_across_sections_on_one_date() {
        let today = 20_000;
        // Two date-grouping sections whose windows overlap on `today`.
        let s = vec![
            Section {
                title: "First".to_string(),
                filter: Filter {
                    when: When::Days(0),
                    todo_only: true,
                    min_priority: None,
                },
            },
            Section {
                title: "Second".to_string(),
                filter: Filter {
                    when: When::Days(7),
                    todo_only: true,
                    min_priority: None,
                },
            },
        ];
        let out = entries_for_row(&r(Some(today), None, Some("TODO")), &s, &kw(), today);
        assert_eq!(out.len(), 2);
        assert_ne!(
            out[0].0, out[1].0,
            "same date in two sections must not share a group key"
        );
        assert!(out[0].0.starts_with("0:") && out[1].0.starts_with("1:"));
    }

    /// A non-date-grouping section renders ONE header, its own title — not a
    /// header per day, which for "Unscheduled" would be a header per nothing.
    #[test]
    fn a_block_section_labels_every_row_with_one_title() {
        let today = 20_000;
        let s = default_sections(7);
        let a = entries_for_row(&r(None, None, Some("TODO")), &s, &kw(), today);
        let b = entries_for_row(&r(None, None, Some("NEXT")), &s, &kw(), today);
        assert_eq!(a[0].0, b[0].0, "one group key for the whole block");
        assert_eq!(a[0].1, "Unscheduled");
        assert_eq!(b[0].1, "Unscheduled");
    }

    /// A done row is not a row at all, so no section can resurrect it — the
    /// scan drops it before sections are consulted.
    #[test]
    fn no_section_can_admit_a_done_row() {
        assert!(scan("* DONE Ship it\n").is_empty());
        assert!(scan("* DONE Ship it\n  SCHEDULED: <2026-08-25 Tue>\n").is_empty());
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

    /// OT.3, the half that is a LIMITATION rather than a feature.
    ///
    /// The text fallback matches `* TODO ` at the start of any line, so example
    /// org inside a `#+BEGIN_SRC` block becomes a phantom agenda row. No care in
    /// the line matcher can fix it — whether a line sits inside a block is not
    /// information the line carries.
    ///
    /// `scan_tree` gets it right (pinned end-to-end by
    /// `a_headline_inside_a_source_block_is_not_a_row` in `tests/org_agenda.rs`,
    /// which returns ONE row for this same corpus). This test exists so the pair
    /// documents the difference rather than only asserting the good half — and
    /// so that if someone ever "fixes" the fallback, they find out here that the
    /// tree path is the one that matters.
    #[test]
    fn the_text_fallback_cannot_tell_a_source_block_from_a_headline() {
        let k = Keywords::from_spec("TODO | DONE");
        let corpus = "* TODO Real task\n  SCHEDULED: <2026-08-25 Tue>\n\
                      #+BEGIN_SRC org\n\
                      * TODO Fake task inside a block\n  SCHEDULED: <2026-08-25 Tue>\n\
                      #+END_SRC\n";
        assert_eq!(
            scan_file(corpus, &k).len(),
            2,
            "the text scan cannot see the block, so it invents a second row"
        );
    }
}

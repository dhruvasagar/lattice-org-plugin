//! OA.19 — the agenda view's own arguments, as data.
//!
//! Slice plan: `lattice/docs/dev/operations/slice-plans/org-agenda.md` phase 6.
//!
//! `scan_args` (OA.11a) carries per-view arguments from the opener to `begin`.
//! It held exactly one positional value — the custom-command key — and phase 6
//! adds two more kinds: a span the user walked to, and the filters they
//! narrowed by. Three features inventing three encodings in the same `Vec<String>`
//! is the outcome this module exists to prevent.
//!
//! ## The grammar
//!
//! ```text
//!   ["r"]                            the `r` agenda, as today
//!   ["r", "span=7", "offset=1"]      …one span forward
//!   ["", "tag:work", "file:a.org"]   the default agenda, filtered
//! ```
//!
//! A **bare token is the command key**, which is what every existing caller
//! sends and what the OA.12 transient will keep sending. Backward compatibility
//! here is not politeness: the transient is a guest export that builds its rows
//! from configuration, and making it re-learn an encoding to say the same thing
//! would be churn with no user on the other end of it.
//!
//! ## Why an unknown argument is ignored
//!
//! A view that refuses to open because it did not recognise one argument is a
//! worse failure than one that opens slightly wrong: the agenda is where you
//! find out what you are meant to be doing, and "it did not open" answers
//! nothing. An unrecognised key is dropped and named in `problems`, which
//! OA.22's headerline is the place to surface. `gr` is the recovery either way.

/// What a filter narrows on.
///
/// Kept as data rather than folded into a `match` string at parse time because
/// the two are not the same thing: a `file:` term is not expressible in org's
/// tags/todo syntax at all, and a `tag:` term has to survive round-tripping
/// back into `scan_args` when the user narrows again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterTerm {
    /// `tag:work` — the row must carry it. Case-sensitive, as org's tag
    /// matching is everywhere else.
    Tag(String),
    /// `file:notes.org` — the row's source file. Matched on the file NAME, not
    /// the full path: the user filtered from a row they were looking at, and
    /// the name is what they saw.
    File(String),
    /// OA.29 `title:ship` — the headline's own text, stars / keyword / tags
    /// stripped. Case-INSENSITIVE substring, unlike `tag:`: a tag is an
    /// identifier the user typed exactly once when they wrote it, and a title
    /// is prose they are half-remembering.
    Title(String),
    /// OA.29 `body:invoice` — the entry's text BELOW the headline, down to the
    /// next headline. Case-insensitive substring, like [`Self::Title`].
    ///
    /// The one filter that reads text no row displays, which is the point:
    /// "the task where I wrote down the account number" is a real way to look
    /// for something, and it is unanswerable from the agenda's own lines.
    Body(String),
    /// OA.29 `cat:work` — the row's org CATEGORY. Exact match,
    /// case-insensitive.
    ///
    /// Resolved per org's own precedence: the headline's `CATEGORY` property,
    /// else the file's `#+CATEGORY:`, else the file's stem.
    Category(String),
    /// OA.29 `re:^\*+ TODO` — a regexp over the row's SOURCE LINE.
    ///
    /// The source line rather than the title, because that is the closest
    /// thing lattice has to what emacs' `org-agenda-filter-by-regexp` matches:
    /// emacs applies it to the rendered agenda line, and here the rendered row
    /// IS the headline line — stars, keyword, priority, title and tags. So a
    /// pattern a user brings over from emacs mostly means the same thing.
    Regexp(String),
}

impl FilterTerm {
    /// The wire spelling, which is also what the headerline shows for
    /// everything except a tag (org spells that `+work`).
    fn key(&self) -> &'static str {
        match self {
            Self::Tag(_) => "tag",
            Self::File(_) => "file",
            Self::Title(_) => "title",
            Self::Body(_) => "body",
            Self::Category(_) => "cat",
            Self::Regexp(_) => "re",
        }
    }

    fn value(&self) -> &str {
        match self {
            Self::Tag(v)
            | Self::File(v)
            | Self::Title(v)
            | Self::Body(v)
            | Self::Category(v)
            | Self::Regexp(v) => v,
        }
    }
}

/// The agenda view's arguments, parsed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewArgs {
    /// The custom-command key, or empty for the default agenda.
    pub command: String,
    /// The span in days this view is showing, overriding `org.agenda-span`.
    /// `None` leaves the option in charge, which is what a fresh agenda does.
    pub span: Option<u32>,
    /// How many spans forward (or back) of today the view is anchored.
    /// `0` is today, which is where every agenda opens.
    ///
    /// **Superseded by [`Self::anchor`] as the authority**, and kept because
    /// `offset=` is a written arg a custom command may already use. `begin`
    /// consults it only when no `date=` was given. It cannot be the carried
    /// state: it is in units of SPANS, so the same `offset` means a different
    /// DAY in a day view and a week view, and carrying it between views with
    /// different spans silently moves the reader to another date.
    pub offset: i32,
    /// The day this view is anchored to, as an epoch day — the first day it
    /// renders. `None` means "not specified": `begin` inherits the day the
    /// previous view was on, or falls back to today.
    ///
    /// A DATE rather than an offset because that is the thing that has to
    /// survive a view switch. "Which day am I looking at" is answerable
    /// without knowing the span; "offset 1" is not.
    pub anchor: Option<i64>,
    /// The filters the user has narrowed by, in the order they added them.
    pub filters: Vec<FilterTerm>,
    /// OA.15: which log items this view admits, or `None` for log mode off —
    /// which is every agenda nobody has pressed `l` in.
    ///
    /// A view ARGUMENT rather than a minor mode, and that is the slice's whole
    /// shape. A log row is an ordinary excerpt over the headline the event
    /// happened to, so turning log mode on is "re-open this view asking for
    /// more rows" — which is exactly what `span` and `filters` already are.
    /// Modelling it as a mode would have made it the one display toggle that
    /// could not survive `gr`, could not differ between two open agendas, and
    /// could not be written down.
    pub log: Option<crate::agenda_log::LogItems>,
    /// Arguments that were not understood, named for the headerline. Never
    /// fatal — see the module header.
    pub problems: Vec<String>,
    /// Set when this arg list came from [`Self::to_args`] — i.e. it is the
    /// COMPLETE state of a view being re-opened, not a fresh request.
    ///
    /// This is what makes inheritance decidable. `to_args` omits everything
    /// at its default (no `offset=0`, no `span=` on the option default, no
    /// filter tokens when the user cleared them), so "absent" cannot
    /// distinguish "the caller did not say" from "the caller said none" —
    /// and inheriting on absence would make `.` (back to today) and
    /// filter-clear silently do nothing. Rather than teach every field to
    /// carry an explicit-default spelling, the LIST says whether it is
    /// complete, and `begin` inherits only when it is not.
    pub complete: bool,
}

impl ViewArgs {
    /// The default agenda, unwalked and unfiltered. `const` so it can seed a
    /// `thread_local` without a lazy init.
    pub const fn new() -> Self {
        Self {
            command: String::new(),
            span: None,
            offset: 0,
            anchor: None,
            filters: Vec::new(),
            log: None,
            problems: Vec::new(),
            complete: false,
        }
    }

    /// Parse what the opener handed the scan.
    pub fn parse(args: &[String]) -> Self {
        let mut out = ViewArgs::default();
        for raw in args {
            let arg = raw.trim();
            if arg.is_empty() {
                continue;
            }
            // `key:value` before `key=value`: `tag:` and `file:` are the two
            // that read naturally with a colon, and a path can contain `=`.
            if let Some((key, value)) = arg.split_once(':') {
                // `split_once` takes the FIRST colon, so a value may contain
                // more of them — `re:^\*: ` and `title:notes: q3` both survive.
                match key {
                    "tag" if !value.is_empty() => {
                        out.filters.push(FilterTerm::Tag(value.to_string()));
                        continue;
                    }
                    "file" if !value.is_empty() => {
                        out.filters.push(FilterTerm::File(value.to_string()));
                        continue;
                    }
                    "title" if !value.is_empty() => {
                        out.filters.push(FilterTerm::Title(value.to_string()));
                        continue;
                    }
                    "body" if !value.is_empty() => {
                        out.filters.push(FilterTerm::Body(value.to_string()));
                        continue;
                    }
                    "cat" if !value.is_empty() => {
                        out.filters.push(FilterTerm::Category(value.to_string()));
                        continue;
                    }
                    // OA.29: validated HERE rather than where it is applied.
                    //
                    // A pattern that does not compile has three possible
                    // behaviours and two of them are traps: matching nothing
                    // empties the agenda while the header says it is filtered
                    // (the worst thing this view can say incorrectly), and
                    // matching everything shows an unfiltered agenda under a
                    // filter the user believes is on. So it is DROPPED and
                    // named, and the headerline carries the ⚠ — the same
                    // treatment every other unusable argument gets.
                    "re" if !value.is_empty() => {
                        match regex_lite::Regex::new(value) {
                            Ok(_) => out.filters.push(FilterTerm::Regexp(value.to_string())),
                            Err(e) => out.problems.push(format!(
                                "regexp `{value}` did not compile ({})",
                                first_line(&e.to_string())
                            )),
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            if let Some((key, value)) = arg.split_once('=') {
                match key {
                    "span" => match value.parse::<u32>() {
                        Ok(n) => out.span = Some(n),
                        Err(_) => out
                            .problems
                            .push(format!("span `{value}` is not a day count")),
                    },
                    "offset" => match value.parse::<i32>() {
                        Ok(n) => out.offset = n,
                        Err(_) => out
                            .problems
                            .push(format!("offset `{value}` is not a number")),
                    },
                    // The anchor day, as an epoch day. Emitted by `to_args`
                    // on every re-open so a carried view always states where
                    // it is, rather than leaving it to be inferred.
                    "date" => match value.parse::<i64>() {
                        Ok(n) => out.anchor = Some(n),
                        Err(_) => out
                            .problems
                            .push(format!("date `{value}` is not an epoch day")),
                    },
                    // Written only by `to_args`. Says "this list is the whole
                    // state", which is what lets `begin` tell a re-open from a
                    // fresh request and inherit only for the latter.
                    "state" => out.complete = value == "complete",
                    // OA.15. `log=` with nothing after it is log mode OFF
                    // rather than a problem: it is what an empty item set
                    // renders to, and a round trip that turned "no items" into
                    // a warning would put a ⚠ in the headerline for a state
                    // the user reached by pressing `l` twice.
                    "log" => {
                        let (items, problems) = crate::agenda_log::LogItems::parse(value);
                        out.problems.extend(problems);
                        out.log = items.any().then_some(items);
                    }
                    _ => out.problems.push(format!("unknown view argument `{key}`")),
                }
                continue;
            }
            // A bare token. The FIRST one is the command key; a second is a
            // mistake worth naming rather than silently letting the later win,
            // because "my agenda opened the wrong command" is hard to trace
            // back to an argument list nobody prints.
            if out.command.is_empty() {
                out.command = arg.to_string();
            } else {
                out.problems
                    .push(format!("ignoring a second command key `{arg}`"));
            }
        }
        out
    }

    /// Render back to `scan_args`, so a chord can re-open the view with one
    /// thing changed and everything else carried along.
    ///
    /// The inverse of [`Self::parse`] for everything it understood; `problems`
    /// are dropped, because re-emitting an argument the parse already rejected
    /// would make a typo permanent.
    pub fn to_args(&self) -> Vec<String> {
        // The command key stays positional and FIRST, so the output is
        // something `parse` reads back identically and something a human
        // reading a log recognises.
        let mut out = vec![self.command.clone()];
        if let Some(span) = self.span {
            out.push(format!("span={span}"));
        }
        // The anchor is emitted whenever it is known, INCLUDING when it is
        // today: a carried view states where it is, so `.` (back to today) is
        // distinguishable from "the caller said nothing".
        //
        // `offset` is emitted only in its absence, and the pair is
        // exclusive-or on purpose: emitting both would let them disagree, and
        // dropping `offset` outright would stop `to_args` being the inverse of
        // `parse` for a view that carries one — which a written custom command
        // still can. `begin` resolves an anchor onto every view it scans, so
        // the `offset` branch is the not-yet-scanned case only.
        match self.anchor {
            Some(day) => out.push(format!("date={day}")),
            None if self.offset != 0 => out.push(format!("offset={}", self.offset)),
            None => {}
        }
        // OA.15: emitted only when log mode is ON. An `log=` on every agenda
        // would make the default view's arguments say something about a mode
        // it is not in, and `parse` reads its absence as off anyway.
        if let Some(items) = self.log {
            out.push(format!("log={}", items.to_spec()));
        }
        for f in &self.filters {
            out.push(format!("{}:{}", f.key(), f.value()));
        }
        // Last, so a human reading a log sees the interesting arguments first.
        out.push("state=complete".to_string());
        out
    }

    /// Fill this view's unset display state from `prev` — the view the user
    /// was just looking at.
    ///
    /// Span, anchor day, filters and log mode are properties of the READER,
    /// not of the view: someone looking at next week, narrowed to `tag:work`,
    /// who opens a different agenda is still looking at next week and still
    /// cares about work. Resetting them makes every view switch cost the
    /// reader their place.
    ///
    /// `command` is deliberately NOT inherited — it IS the view's identity,
    /// and inheriting it would make every view open as the previous one.
    ///
    /// A no-op when `self.complete`: those args came from [`Self::to_args`]
    /// and already say everything, so inheriting would resurrect state the
    /// user just cleared.
    pub fn inherit_display_state(&mut self, prev: &Self) {
        if self.complete {
            return;
        }
        if self.span.is_none() {
            self.span = prev.span;
        }
        if self.anchor.is_none() && self.offset == 0 {
            self.anchor = prev.anchor;
        }
        if self.filters.is_empty() {
            self.filters = prev.filters.clone();
        }
        if self.log.is_none() {
            self.log = prev.log;
        }
    }

    /// Whether `path` survives the `file:` filters.
    ///
    /// Matched on the file NAME rather than the full path: the user filtered
    /// from a row they were looking at, and the name is what they saw. Several
    /// `file:` terms are an OR — narrowing to two files is a sensible thing to
    /// ask for, and an AND would be unsatisfiable.
    pub fn admits_file(&self, path: &str) -> bool {
        let names: Vec<&String> = self
            .filters
            .iter()
            .filter_map(|f| match f {
                FilterTerm::File(n) => Some(n),
                // Every other term is a ROW test, and this is a FILE test. A
                // catch-all would be wrong the day someone adds a second
                // file-shaped term, so they are named.
                FilterTerm::Tag(_)
                | FilterTerm::Title(_)
                | FilterTerm::Body(_)
                | FilterTerm::Category(_)
                | FilterTerm::Regexp(_) => None,
            })
            .collect();
        if names.is_empty() {
            return true;
        }
        let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
        names
            .iter()
            .any(|n| base == n.as_str() || path == n.as_str())
    }

    /// Whether `tags` satisfies the `tag:` filters.
    ///
    /// Several `tag:` terms are an AND: `\` NARROWS an existing filter, which
    /// is the whole reason it is a separate key from `/`.
    pub fn admits_tags(&self, tags: &[String]) -> bool {
        self.filters.iter().all(|f| match f {
            FilterTerm::Tag(t) => tags.iter().any(|x| x == t),
            // Not a tag test — answered by `admits_file` and `RowMatcher`
            // respectively. Enumerated rather than `_`: a new term that is
            // silently admitted here is a filter that does not filter.
            FilterTerm::File(_)
            | FilterTerm::Title(_)
            | FilterTerm::Body(_)
            | FilterTerm::Category(_)
            | FilterTerm::Regexp(_) => true,
        })
    }

    /// OA.15: the inclusive epoch-day range log mode reports over, given the
    /// view's resolved `span` and the day it is anchored to.
    ///
    /// **Backward, where the plan is forward.** A section's window is
    /// `anchor ..= anchor + span`; a log has nothing to say about the future,
    /// so filing log rows into that window would only ever surface today's.
    /// The daily agenda logs today, the week view logs the last seven days,
    /// and `b` walks back through earlier weeks exactly as it walks the plan.
    ///
    /// Length matches what the headerline CALLS the span (`window` renders
    /// `anchor ..= anchor + span - 1`), so a header reading `Week` covers
    /// seven days in both directions rather than seven forward and eight back.
    pub fn log_days(span: u32, anchor: i64) -> std::ops::RangeInclusive<i64> {
        let back = i64::from(span.max(1)) - 1;
        (anchor - back)..=anchor
    }

    /// Whether any filter is active at all — what OA.22's headerline asks, and
    /// what lets the scan skip the work entirely when none is.
    pub fn is_filtered(&self) -> bool {
        !self.filters.is_empty()
    }

    /// The command key as `agenda_custom_commands::resolve` wants it — a slice
    /// whose first element is the key, or empty for the default agenda.
    pub fn command_args(&self) -> Vec<String> {
        if self.command.is_empty() {
            Vec::new()
        } else {
            vec![self.command.clone()]
        }
    }
}

/// OA.29 — the text facets of one row, for the filters that read them.
///
/// Borrowed rather than owned: the scan holds the file's text and the row's
/// line for the length of the walk, so every field here is a slice into
/// something already in hand. A row that survives no filter should cost no
/// allocation.
#[derive(Debug, Clone, Copy)]
pub struct RowText<'a> {
    /// The row's SOURCE LINE, verbatim — what `re:` matches, and what the view
    /// actually renders.
    pub headline: &'a str,
    /// The headline's own text: stars, TODO keyword and trailing tags removed.
    pub title: &'a str,
    /// The entry's text below the headline, down to the next headline.
    pub body: &'a str,
    /// The resolved org CATEGORY for this row.
    pub category: &'a str,
}

/// OA.29 — a view's text filters, compiled once for a whole scan.
///
/// **Built once per scan, not once per row.** `re:` has to compile, and
/// compiling the same handful of patterns for every headline in a 700-file
/// corpus is the kind of per-row cost that does not show up in a test and does
/// show up in a scan. Lowercasing the substring needles once is the same trade.
#[derive(Debug, Default)]
pub struct RowMatcher {
    /// Lowercased needles, ANDed — each `sT` narrows.
    titles: Vec<String>,
    /// Lowercased needles, ANDed.
    bodies: Vec<String>,
    /// Lowercased, ORed. A row has exactly ONE category, so ANDing two would be
    /// unsatisfiable — the same reasoning `file:` terms are ORed under.
    categories: Vec<String>,
    /// Compiled patterns, ANDed. Only ones that compiled: `ViewArgs::parse`
    /// drops and reports the rest, so an unusable pattern never reaches here.
    regexes: Vec<regex_lite::Regex>,
}

impl RowMatcher {
    /// Compile `view`'s text filters. Cheap and total — a view with none
    /// produces an empty matcher that admits everything.
    pub fn new(view: &ViewArgs) -> Self {
        let mut out = Self::default();
        for f in &view.filters {
            match f {
                FilterTerm::Title(t) => out.titles.push(t.to_lowercase()),
                FilterTerm::Body(b) => out.bodies.push(b.to_lowercase()),
                FilterTerm::Category(c) => out.categories.push(c.to_lowercase()),
                // `parse` already rejected anything that does not compile, so
                // this cannot normally fail. Skipping rather than unwrapping if
                // it somehow does: a panic here takes the whole scan down, and
                // the term was already reported.
                FilterTerm::Regexp(r) => {
                    if let Ok(re) = regex_lite::Regex::new(r) {
                        out.regexes.push(re);
                    }
                }
                FilterTerm::Tag(_) | FilterTerm::File(_) => {}
            }
        }
        out
    }

    /// Whether this matcher tests anything. An empty one admits every row, so
    /// the scan can skip building [`RowText`] entirely.
    pub fn is_empty(&self) -> bool {
        self.titles.is_empty()
            && self.bodies.is_empty()
            && self.categories.is_empty()
            && self.regexes.is_empty()
    }

    /// Whether `row` survives every text filter.
    ///
    /// Substring tests are case-INSENSITIVE and regexp tests are not. That is
    /// not an inconsistency: a substring filter is someone half-remembering
    /// prose, and a regexp is someone stating a pattern exactly — a `re:` that
    /// silently ignored case could not express "the SHOUTING ones", and there
    /// is no way to ask for case-sensitivity back. `(?i)` is how a regexp asks
    /// for the other behaviour, which is the spelling its users already know.
    pub fn admits(&self, row: &RowText<'_>) -> bool {
        // Each block lowercases its haystack only when it has a needle for it.
        // A body can be an entire subtree, so folding one per row for a view
        // that never asked about bodies is the kind of cost that hides inside
        // a 700-file scan.
        if !self.titles.is_empty() {
            let title = row.title.to_lowercase();
            if !self.titles.iter().all(|n| title.contains(n.as_str())) {
                return false;
            }
        }
        if !self.bodies.is_empty() {
            let body = row.body.to_lowercase();
            if !self.bodies.iter().all(|n| body.contains(n.as_str())) {
                return false;
            }
        }
        if !self.categories.is_empty() {
            let cat = row.category.to_lowercase();
            if !self.categories.iter().any(|c| *c == cat) {
                return false;
            }
        }
        self.regexes.iter().all(|re| re.is_match(row.headline))
    }
}

/// The first line of a multi-line error, for a headerline that is one line.
///
/// `regex-lite` reports a syntax error across several lines with a caret under
/// the offending character; all of that in a modeline would push the window and
/// the filter off the end of it.
fn first_line(msg: &str) -> String {
    msg.lines().next().unwrap_or(msg).trim().to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_bare_key_still_selects_its_command() {
        // The compatibility that lets every existing caller keep working. The
        // OA.12 transient sends exactly this and has no reason to stop.
        let a = ViewArgs::parse(&args(&["r"]));
        assert_eq!(a.command, "r");
        assert_eq!(a.command_args(), vec!["r".to_string()]);
        assert!(a.problems.is_empty());
        assert_eq!(a.offset, 0, "a fresh agenda is anchored on today");
        assert_eq!(a.span, None, "and takes its span from the option");
    }

    #[test]
    fn no_args_at_all_is_the_default_agenda() {
        let a = ViewArgs::parse(&[]);
        assert_eq!(a.command, "");
        assert!(
            a.command_args().is_empty(),
            "which resolve reads as unnamed"
        );
    }

    #[test]
    fn span_and_offset_parse_and_offset_may_be_negative() {
        let a = ViewArgs::parse(&args(&["", "span=7", "offset=-2"]));
        assert_eq!(a.span, Some(7));
        assert_eq!(a.offset, -2, "`b` walks backwards");
        assert!(a.problems.is_empty());
    }

    #[test]
    fn filters_keep_the_order_they_were_added_in() {
        // `\` narrows an existing filter rather than replacing it, so the list
        // is a conjunction and its order is what the user built.
        let a = ViewArgs::parse(&args(&["r", "tag:work", "file:notes.org", "tag:urgent"]));
        assert_eq!(
            a.filters,
            vec![
                FilterTerm::Tag("work".into()),
                FilterTerm::File("notes.org".into()),
                FilterTerm::Tag("urgent".into()),
            ]
        );
        assert_eq!(a.command, "r", "…alongside the command, not instead of it");
    }

    #[test]
    fn an_unknown_argument_is_named_and_dropped_rather_than_fatal() {
        // A view that refuses to open because it did not recognise one
        // argument answers nothing. The agenda is where you find out what you
        // are meant to be doing.
        let a = ViewArgs::parse(&args(&["r", "sideways=3", "span=nope"]));
        assert_eq!(a.command, "r", "the rest of the arguments still apply");
        assert_eq!(a.span, None);
        assert_eq!(a.problems.len(), 2, "{:?}", a.problems);
        assert!(a.problems[0].contains("sideways"), "{:?}", a.problems);
        assert!(a.problems[1].contains("span"), "{:?}", a.problems);
    }

    #[test]
    fn a_second_bare_key_is_named_rather_than_silently_winning() {
        // "My agenda opened the wrong command" is hard to trace back to an
        // argument list nobody prints.
        let a = ViewArgs::parse(&args(&["r", "n"]));
        assert_eq!(a.command, "r", "the first wins");
        assert!(a.problems[0].contains('n'), "{:?}", a.problems);
    }

    #[test]
    fn arguments_round_trip_so_a_chord_can_change_one_thing() {
        // Every phase-6 chord is "re-open this view with one argument
        // different", so what `to_args` writes must parse back to what it
        // meant — otherwise walking a span would quietly drop a filter.
        let original = ViewArgs::parse(&args(&[
            "r",
            "span=30",
            "offset=2",
            "log=closed,clock",
            "tag:work",
            "file:a.org",
        ]));
        // `complete` is the one field the trip is meant to change: `original`
        // came from a caller's raw args, and `to_args` stamps its output as
        // the full state so `begin` knows not to inherit into it.
        let mut expected = original.clone();
        expected.complete = true;
        let round = ViewArgs::parse(&original.to_args());
        assert_eq!(round, expected);
    }

    /// The anchor day is what survives a view switch, so it has to survive the
    /// round trip that every switch goes through.
    #[test]
    fn an_anchor_day_round_trips_and_supersedes_a_stale_offset() {
        let mut v = ViewArgs::parse(&args(&["r", "span=7", "offset=2"]));
        // What `begin` does once it has resolved where the view actually is.
        v.anchor = Some(20_000);
        let round = ViewArgs::parse(&v.to_args());
        assert_eq!(round.anchor, Some(20_000), "the day is carried");
        assert_eq!(
            round.offset, 0,
            "and the offset it superseded is NOT re-emitted — two spellings of \
             the same thing are two chances to disagree"
        );
    }

    /// A fresh opener (a different agenda view) says nothing about span, day
    /// or filters, so it inherits the reader's. This is the reported bug: set
    /// a span, open another view, lose it.
    #[test]
    fn a_fresh_view_inherits_the_readers_span_day_and_filters() {
        let prev = {
            let mut p = ViewArgs::parse(&args(&["", "span=7", "tag:work"]));
            p.anchor = Some(20_000);
            p
        };
        let mut fresh = ViewArgs::parse(&args(&["refile"]));
        fresh.inherit_display_state(&prev);
        assert_eq!(fresh.command, "refile", "the view's own identity is kept");
        assert_eq!(fresh.span, Some(7), "the span the reader chose");
        assert_eq!(fresh.anchor, Some(20_000), "and the day they are reading");
        assert_eq!(fresh.filters.len(), 1, "and the narrowing they applied");
    }

    /// The trap that makes inheritance-on-absence wrong without the marker:
    /// a view the user just CLEARED emits nothing for the cleared field, which
    /// looks identical to a caller that said nothing. `state=complete` is what
    /// tells them apart, so `.` and filter-clear are not silently undone.
    #[test]
    fn a_carried_view_does_not_inherit_what_the_user_just_cleared() {
        let prev = {
            let mut p = ViewArgs::parse(&args(&["", "span=7", "tag:work"]));
            p.anchor = Some(20_000);
            p
        };
        // The user cleared the filter; the chord re-opens with the full state.
        let cleared = {
            let mut c = prev.clone();
            c.filters.clear();
            c
        };
        let mut reopened = ViewArgs::parse(&cleared.to_args());
        assert!(reopened.complete, "a re-open states that it is complete");
        reopened.inherit_display_state(&prev);
        assert!(
            reopened.filters.is_empty(),
            "the cleared filter must stay cleared rather than being inherited \
             back from the view it was cleared on"
        );
    }

    // ── OA.15: log mode is an argument, so it round-trips like one ──────

    #[test]
    fn log_mode_parses_its_item_set() {
        let a = ViewArgs::parse(&args(&["", "log=closed,state"]));
        let items = a.log.expect("log mode is on");
        assert!(items.closed && items.state);
        assert!(!items.clock, "…and only what was asked for");
        assert!(a.problems.is_empty());
    }

    /// Absent means OFF, which is what every agenda that has never been shown
    /// a log row carries.
    #[test]
    fn no_log_argument_is_log_mode_off() {
        assert_eq!(ViewArgs::parse(&args(&["r", "span=7"])).log, None);
    }

    /// `l` pressed twice renders an empty item set. That has to read back as
    /// OFF rather than as a malformed argument, or turning the mode off would
    /// put a ⚠ in the headerline.
    #[test]
    fn an_empty_item_set_is_off_rather_than_a_problem() {
        let a = ViewArgs::parse(&args(&["", "log="]));
        assert_eq!(a.log, None);
        assert!(a.problems.is_empty(), "{:?}", a.problems);
        assert!(
            !a.to_args().iter().any(|s| s.starts_with("log")),
            "and it is not re-emitted: {:?}",
            a.to_args()
        );
    }

    #[test]
    fn an_unknown_log_item_is_named_and_the_rest_survives() {
        let a = ViewArgs::parse(&args(&["", "log=closed,sideways"]));
        assert!(a.log.expect("the rest still applies").closed);
        assert_eq!(a.problems.len(), 1, "{:?}", a.problems);
        assert!(a.problems[0].contains("sideways"), "{:?}", a.problems);
    }

    /// The log window looks BACKWARD. A forward window would only ever contain
    /// today, because nothing is logged in the future.
    #[test]
    fn the_log_window_covers_the_span_ending_today() {
        assert_eq!(ViewArgs::log_days(0, 100), 100..=100, "the daily agenda");
        assert_eq!(ViewArgs::log_days(1, 100), 100..=100);
        assert_eq!(ViewArgs::log_days(7, 100), 94..=100, "the last seven days");
    }

    /// The headerline has to say it: a view listing yesterday's closures with
    /// nothing to explain them reads as an agenda that has started showing
    /// finished work.
    #[test]
    fn the_header_says_log_mode_is_on() {
        let a = ViewArgs::parse(&args(&["", "log=closed,clock"]));
        let header = describe(&a, 7, 0);
        assert!(header.contains("log closed,clock"), "{header}");
        assert!(
            !describe(&ViewArgs::parse(&[]), 7, 0).contains("log"),
            "…and stays quiet when it is off"
        );
    }

    #[test]
    fn a_default_view_round_trips_to_something_parse_reads_back() {
        // The empty command is still emitted positionally, so the output is
        // never mistaken for a filter and never shifts what follows it.
        let a = ViewArgs {
            command: String::new(),
            span: None,
            offset: 0,
            anchor: None,
            filters: vec![FilterTerm::Tag("work".into())],
            log: None,
            problems: Vec::new(),
            // What `to_args` stamps on its output; the round trip has to read
            // it back or the re-parsed view would ask to inherit.
            complete: true,
        };
        assert_eq!(
            a.to_args(),
            vec![
                "".to_string(),
                "tag:work".to_string(),
                "state=complete".to_string()
            ]
        );
        assert_eq!(ViewArgs::parse(&a.to_args()), a);
    }

    #[test]
    fn a_rejected_argument_is_not_re_emitted() {
        // Re-emitting one would make a typo permanent: every subsequent chord
        // carries the arguments forward, so a bad one would survive every
        // refresh until the view was closed.
        let a = ViewArgs::parse(&args(&["r", "sideways=3"]));
        assert!(!a.problems.is_empty());
        assert!(
            !a.to_args().iter().any(|s| s.contains("sideways")),
            "got {:?}",
            a.to_args()
        );
    }
}

#[cfg(test)]
mod span_walk_tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    /// What the OA.20 handlers do, as data — so the arithmetic is testable
    /// without a host, an editor or a scan. The handlers themselves are a
    /// `match` over which of these to apply and one `OpenProviderView`.
    fn later(v: &mut ViewArgs) {
        v.offset = v.offset.saturating_add(1);
    }
    fn earlier(v: &mut ViewArgs) {
        v.offset = v.offset.saturating_sub(1);
    }
    fn today(v: &mut ViewArgs) {
        v.offset = 0;
    }
    fn span_view(v: &mut ViewArgs, days: u32) {
        v.span = Some(days);
        v.offset = 0;
    }

    #[test]
    fn f_then_b_returns_to_where_it_started() {
        // An off-by-one in the offset arithmetic is invisible in either
        // direction alone: walking forward "works" and walking back "works",
        // and only the round trip shows they disagree.
        let start = ViewArgs::parse(&["r".to_string(), "span=7".to_string()]);
        let mut v = start.clone();
        later(&mut v);
        assert_eq!(v.offset, 1);
        earlier(&mut v);
        assert_eq!(v, start, "f then b is the identity");
    }

    #[test]
    fn walking_carries_the_command_and_filters_along() {
        // The reason the arguments are one list: a span chord must not drop
        // the agenda you are in or the filter you narrowed by.
        let mut v = ViewArgs::parse(&[
            "r".to_string(),
            "tag:work".to_string(),
            "file:a.org".to_string(),
        ]);
        later(&mut v);
        let round = ViewArgs::parse(&v.to_args());
        assert_eq!(round.command, "r");
        assert_eq!(round.filters.len(), 2);
        assert_eq!(round.offset, 1);
    }

    #[test]
    fn today_resets_where_you_are_not_what_you_are_looking_at() {
        // `.` is "take me home", not "forget my view". Losing a week view to
        // get back to today would make the key cost more than it gives.
        let mut v =
            ViewArgs::parse(&["".to_string(), "span=7".to_string(), "offset=3".to_string()]);
        today(&mut v);
        assert_eq!(v.offset, 0);
        assert_eq!(v.span, Some(7), "the span survives");
    }

    #[test]
    fn changing_the_span_re_anchors_on_today() {
        // An `offset=2` held across a day→month switch means two MONTHS out,
        // which is never what the person pressing `v m` meant.
        let mut v = ViewArgs::parse(&["".to_string(), "offset=2".to_string()]);
        span_view(&mut v, 30);
        assert_eq!(v.span, Some(30));
        assert_eq!(v.offset, 0);
    }

    #[test]
    fn walking_far_out_saturates_rather_than_wrapping() {
        // A held key is a real input. Wrapping would put the agenda centuries
        // away from today with no way back but `.`, and the row labels
        // ("overdue by N day(s)") would be nonsense on the way.
        let mut v = ViewArgs::parse(&["".to_string(), format!("offset={}", i32::MAX)]);
        later(&mut v);
        assert_eq!(v.offset, i32::MAX);
        let mut v = ViewArgs::parse(&["".to_string(), format!("offset={}", i32::MIN)]);
        earlier(&mut v);
        assert_eq!(v.offset, i32::MIN);
    }

    #[test]
    fn the_daily_agenda_still_walks_a_day_at_a_time() {
        // `span = 0` is the daily view and the one people walk most. The
        // anchor moves by `span.max(1)` days, so a step of zero — which is
        // what a literal reading gives — would make `f` a no-op on exactly
        // that view. Pinned as the arithmetic `begin` performs.
        let step = |span: u32, offset: i32| i64::from(offset) * i64::from(span.max(1));
        assert_eq!(step(0, 1), 1, "one day forward, not zero");
        assert_eq!(step(0, -2), -2);
        assert_eq!(step(7, 1), 7, "a week view walks a week");
        assert_eq!(step(30, 2), 60);
    }
}

#[cfg(test)]
mod filter_tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn v(items: &[&str]) -> ViewArgs {
        ViewArgs::parse(&items.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn tags(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_filter_admits_everything() {
        let a = v(&["r"]);
        assert!(!a.is_filtered());
        assert!(a.admits_tags(&[]));
        assert!(a.admits_file("/anywhere/notes.org"));
    }

    #[test]
    fn tag_terms_narrow_rather_than_widen() {
        // `\` NARROWS, which is the whole reason it is a separate key from
        // `/`. Two tag terms are an AND; treating them as an OR would make
        // every press of `\` show MORE, which is the opposite of the gesture.
        let a = v(&["", "tag:work", "tag:urgent"]);
        assert!(a.admits_tags(&tags(&["work", "urgent", "other"])));
        assert!(!a.admits_tags(&tags(&["work"])), "one of two is not both");
        assert!(!a.admits_tags(&tags(&["urgent"])));
    }

    #[test]
    fn tag_matching_is_case_sensitive_like_everywhere_else_in_org() {
        let a = v(&["", "tag:refile"]);
        assert!(a.admits_tags(&tags(&["refile"])));
        assert!(!a.admits_tags(&tags(&["REFILE"])));
    }

    #[test]
    fn file_terms_widen_rather_than_narrow() {
        // The opposite of tags, and deliberately: narrowing to two files is a
        // sensible thing to ask for, and an AND would be unsatisfiable — a row
        // comes from exactly one file.
        let a = v(&["", "file:a.org", "file:b.org"]);
        assert!(a.admits_file("/org/a.org"));
        assert!(a.admits_file("/elsewhere/b.org"));
        assert!(!a.admits_file("/org/c.org"));
    }

    #[test]
    fn a_file_filter_matches_the_name_the_user_saw() {
        // They filtered from a row they were looking at; the name is what was
        // on screen, not the absolute path the host resolved it to.
        let a = v(&["", "file:notes.org"]);
        assert!(a.admits_file("/home/me/org/notes.org"));
        assert!(a.admits_file("notes.org"));
        assert!(
            !a.admits_file("/home/me/org/other-notes.org"),
            "a suffix is not a name — `other-notes.org` is a different file"
        );
    }

    #[test]
    fn the_two_kinds_of_filter_are_independent() {
        // A `file:` term must not gate a tag test or vice versa, or narrowing
        // by one would silently drop the other.
        let a = v(&["", "tag:work", "file:a.org"]);
        assert!(
            a.admits_tags(&tags(&["work"])),
            "the file term is not a tag test"
        );
        assert!(a.admits_file("/x/a.org"), "the tag term is not a file test");
        assert!(!a.admits_tags(&tags(&["home"])));
        assert!(!a.admits_file("/x/b.org"));
    }

    #[test]
    fn a_filter_composes_with_a_command_rather_than_replacing_it() {
        // `r` then `/work` is refile AND work. Getting this wrong silently
        // shows one of the two, which looks like a working feature.
        let a = v(&["r", "tag:work"]);
        assert_eq!(a.command, "r");
        assert_eq!(a.command_args(), vec!["r".to_string()]);
        assert!(a.is_filtered());
        // …and it survives the round trip every chord makes, which is what
        // makes `gr` preserve the filter for free.
        //
        // `complete` is the one field the round trip is expected to CHANGE:
        // `a` was parsed from a caller's raw args, and `to_args` stamps its
        // output as the full state so `begin` knows not to inherit into it.
        let mut expected = a.clone();
        expected.complete = true;
        assert_eq!(ViewArgs::parse(&a.to_args()), expected);
    }
}

/// OA.22 — what this view IS, for its headerline.
///
/// A filtered agenda that looks like an unfiltered one is a trap: "nothing
/// scheduled" under a filter the user forgot is this view saying "you have no
/// tasks" when they have plenty, and that is the single worst thing it can say
/// incorrectly.
///
/// The host prefixes its own counts, so this names only what the host cannot
/// know: WHEN the view is looking, which command built it, and what is
/// filtering it.
///
/// ## The window is always shown
///
/// `anchor` is the epoch day the scan anchored on — `today + offset * span`,
/// the same expression the scan uses, because a header that computed the
/// window differently would eventually disagree with the rows under it. The
/// dates are the one fact that orients a reader instantly, they are what `f` /
/// `b` / `.` change, and after two presses of `f` nothing else on screen says
/// where you are.
///
/// Everything else appears only when it is not the ordinary case: naming the
/// default span on every agenda is noise, and an eye that learns to skip the
/// header skips the filter too.
///
/// `problems` is LAST and always shown. It is the only channel a bad view
/// argument has: a guest `logging::log` call makes the component import
/// `logging`, which org's multi-seam linker does not wire on every seam, and
/// the whole component then fails to instantiate. Dropping the argument
/// silently would show the default agenda while looking like the one asked for.
pub fn describe(view: &ViewArgs, default_span: u32, anchor: i64) -> String {
    let mut parts: Vec<String> = Vec::new();
    let span = view.span.unwrap_or(default_span);
    parts.push(format!("{} {}", span_name(span), window(anchor, span)));
    if !view.command.is_empty() {
        parts.push(view.command.clone());
    }
    for f in &view.filters {
        parts.push(match f {
            // `+work` is org's own spelling for a tag filter, so what the
            // header shows is what the user would type to reproduce it. The
            // rest carry their wire key, which is the same thing one step
            // further: `cat:work` is exactly what `to_args` writes.
            FilterTerm::Tag(t) => format!("+{t}"),
            other => format!("{}:{}", other.key(), other.value()),
        });
    }
    // OA.15: log mode changes which rows exist, so the header has to say it —
    // a view showing yesterday's closures with nothing to explain them reads
    // as an agenda that has started listing finished work.
    if let Some(items) = view.log {
        parts.push(format!("log {}", items.to_spec()));
    }
    for p in &view.problems {
        parts.push(format!("\u{26a0} {p}"));
    }
    parts.join(" \u{b7} ")
}

/// What the span is CALLED — `Day`, `Week`, `Month`, `Year`, or a plain count.
///
/// The dates alone were the whole header, and a reader had to subtract them to
/// learn which span they were looking at. Emacs names it (`Week-agenda`), and
/// `gD` switches between four of them, so the name is what tells you the key
/// worked.
///
/// The four the span keys set (0/1, 7, 30, 365) get names; anything else — a
/// user's `:set org.agenda-span=10` — reports its own count rather than being
/// rounded into the nearest word it is not.
fn span_name(span: u32) -> String {
    match span {
        0 | 1 => "Day".to_string(),
        7 => "Week".to_string(),
        // 28–31 so a month-length span set by hand still reads as a month;
        // `gDm` sends 30.
        28..=31 => "Month".to_string(),
        365 | 366 => "Year".to_string(),
        n => format!("{n}-day"),
    }
}

/// The dates the view covers: one day, or a range.
///
/// A single day for `span <= 1` — "2026-09-02 – 2026-09-02" is a range with
/// nothing in it, and reads as a bug rather than as a daily agenda.
fn window(anchor: i64, span: u32) -> String {
    let day = |d: i64| {
        let (y, m, dd) = crate::agenda::civil_from_epoch_day(d);
        format!("{y:04}-{m:02}-{dd:02}")
    };
    if span <= 1 {
        day(anchor)
    } else {
        format!(
            "{} \u{2013} {}",
            day(anchor),
            day(anchor + i64::from(span) - 1)
        )
    }
}

#[cfg(test)]
mod row_filter_tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn v(items: &[&str]) -> ViewArgs {
        ViewArgs::parse(&items.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn row<'a>(headline: &'a str, title: &'a str, body: &'a str, category: &'a str) -> RowText<'a> {
        RowText {
            headline,
            title,
            body,
            category,
        }
    }

    #[test]
    fn no_text_filter_admits_everything_and_builds_nothing() {
        let m = RowMatcher::new(&v(&["", "tag:work", "file:a.org"]));
        assert!(
            m.is_empty(),
            "tag and file terms are not text filters — the scan must be able to \
             skip building row text entirely"
        );
        assert!(m.admits(&row("* TODO x", "x", "", "a")));
    }

    #[test]
    fn a_title_filter_is_a_case_insensitive_substring() {
        let m = RowMatcher::new(&v(&["", "title:SHIP"]));
        assert!(m.admits(&row("* TODO Ship it", "Ship it", "", "work")));
        assert!(m.admits(&row("* TODO shipping", "shipping", "", "work")));
        assert!(!m.admits(&row("* TODO Invoice", "Invoice", "", "work")));
    }

    /// Title matches the TITLE, not the whole line — otherwise `sT TODO`
    /// would match every unfinished row, which is not a title search.
    #[test]
    fn a_title_filter_does_not_see_the_keyword_or_tags() {
        let m = RowMatcher::new(&v(&["", "title:todo"]));
        assert!(!m.admits(&row("* TODO Ship it :todo:", "Ship it", "", "work")));
    }

    #[test]
    fn a_body_filter_reads_below_the_headline() {
        let m = RowMatcher::new(&v(&["", "body:invoice"]));
        assert!(m.admits(&row(
            "* TODO Ship it",
            "Ship it",
            "  the INVOICE is due\n",
            "w"
        )));
        assert!(!m.admits(&row("* TODO Ship it", "Ship it", "  nothing here\n", "w")));
    }

    /// Two title terms NARROW. `sT` replaces its own kind on submit, so a view
    /// carrying two came from written args — and there the only sane reading of
    /// two substrings is "both".
    #[test]
    fn title_terms_narrow_rather_than_widen() {
        let m = RowMatcher::new(&v(&["", "title:ship", "title:q3"]));
        assert!(m.admits(&row("*", "Ship the Q3 report", "", "w")));
        assert!(!m.admits(&row("*", "Ship the Q2 report", "", "w")));
    }

    /// Categories are ORed, for `file:`'s reason: a row has exactly ONE, so an
    /// AND of two could never match anything.
    #[test]
    fn category_terms_widen_because_a_row_has_only_one() {
        let m = RowMatcher::new(&v(&["", "cat:work", "cat:home"]));
        assert!(m.admits(&row("*", "x", "", "work")));
        assert!(m.admits(&row("*", "x", "", "home")));
        assert!(!m.admits(&row("*", "x", "", "someday")));
    }

    #[test]
    fn category_matching_ignores_case() {
        let m = RowMatcher::new(&v(&["", "cat:Work"]));
        assert!(m.admits(&row("*", "x", "", "WORK")));
    }

    /// The regexp matches the SOURCE LINE — stars, keyword, priority, title and
    /// tags — which is the nearest thing here to emacs' "the agenda line".
    #[test]
    fn a_regexp_matches_the_whole_source_line() {
        let m = RowMatcher::new(&v(&["", r"re:^\*+ TODO \[#A\]"]));
        assert!(m.admits(&row("** TODO [#A] Ship it", "Ship it", "", "w")));
        assert!(!m.admits(&row("** TODO [#B] Ship it", "Ship it", "", "w")));
    }

    /// Regexps are case-SENSITIVE where substrings are not, and `(?i)` is how
    /// the other behaviour is asked for. A `re:` that folded case could not
    /// express "the SHOUTING ones" and offered no way to get it back.
    #[test]
    fn a_regexp_is_case_sensitive_unless_it_says_otherwise() {
        assert!(!RowMatcher::new(&v(&["", "re:ship"])).admits(&row("* SHIP", "SHIP", "", "w")));
        assert!(RowMatcher::new(&v(&["", "re:(?i)ship"])).admits(&row("* SHIP", "SHIP", "", "w")));
    }

    /// A pattern that does not compile is DROPPED and named — never kept as a
    /// filter that matches nothing, which would empty the agenda while the
    /// header claims it is filtered.
    #[test]
    fn an_uncompilable_regexp_is_reported_and_not_applied() {
        let view = v(&["", "re:[unclosed"]);
        assert!(
            view.filters.is_empty(),
            "the term must not survive: {:?}",
            view.filters
        );
        assert_eq!(view.problems.len(), 1, "{:?}", view.problems);
        assert!(
            view.problems[0].contains("did not compile"),
            "{:?}",
            view.problems
        );
        assert!(
            !view.to_args().iter().any(|a| a.starts_with("re:")),
            "and it is not re-emitted, so a typo does not become permanent"
        );
    }

    /// Every new term round-trips, or a filter would quietly vanish the next
    /// time any chord re-opened the view.
    #[test]
    fn every_filter_kind_round_trips() {
        let original = v(&[
            "r",
            "tag:work",
            "file:a.org",
            "title:ship",
            "body:invoice",
            "cat:home",
            "re:^x",
        ]);
        assert_eq!(original.filters.len(), 6, "{:?}", original.filters);
        let mut expected = original.clone();
        expected.complete = true;
        assert_eq!(ViewArgs::parse(&original.to_args()), expected);
    }

    /// A value may contain colons — `split_once` takes the first one only.
    /// A regexp with a `:` in it is ordinary, and so is a title.
    #[test]
    fn a_filter_value_may_contain_colons() {
        let view = v(&["", "title:q3: revenue"]);
        assert_eq!(
            view.filters,
            vec![FilterTerm::Title("q3: revenue".to_string())]
        );
    }

    /// The headerline names every kind. A filtered agenda that looks unfiltered
    /// is the trap OA.22 exists for, and four new kinds are four new ways in.
    #[test]
    fn the_header_names_every_filter_kind() {
        let said = describe(
            &v(&["", "title:ship", "body:inv", "cat:work", "re:^x"]),
            7,
            20698,
        );
        for want in ["title:ship", "body:inv", "cat:work", "re:^x"] {
            assert!(said.contains(want), "{want} missing from {said:?}");
        }
    }
}

#[cfg(test)]
mod describe_tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    /// 2026-09-02, so every assertion states the day it expects.
    const ANCHOR: i64 = 20698;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// The span is NAMED, not left to be inferred from the dates. `gD` switches
    /// between four of them and the name is what tells you the key worked —
    /// subtracting two dates to learn you are in a month is not a reading.
    #[test]
    fn the_span_is_named() {
        let named = |span: u32| {
            let v = ViewArgs::parse(&args(&["", &format!("span={span}")]));
            describe(&v, 7, ANCHOR)
        };
        assert!(named(1).starts_with("Day "));
        assert!(named(7).starts_with("Week "));
        assert!(named(30).starts_with("Month "));
        assert!(named(365).starts_with("Year "));
    }

    /// A hand-set span reports its own count rather than being rounded into a
    /// word it is not. `:set org.agenda-span=10` is not a week.
    #[test]
    fn an_unusual_span_names_its_own_length() {
        let v = ViewArgs::parse(&args(&["", "span=10"]));
        assert!(describe(&v, 7, ANCHOR).starts_with("10-day "));
    }

    /// A month set by hand at 28 or 31 still reads as a month — `gDm` sends 30,
    /// but a user writing the real length of February should not get "28-day".
    #[test]
    fn a_month_length_span_still_reads_as_a_month() {
        for n in [28, 29, 30, 31] {
            let v = ViewArgs::parse(&args(&["", &format!("span={n}")]));
            assert!(
                describe(&v, 7, ANCHOR).starts_with("Month "),
                "span={n} should read as a month"
            );
        }
    }

    #[test]
    fn the_anchor_is_the_day_it_says() {
        assert_eq!(window(ANCHOR, 1), "2026-09-02");
    }

    /// The window is ALWAYS shown: it is what `f` / `b` / `.` change, and after
    /// two presses nothing else on screen says where you are.
    #[test]
    fn the_ordinary_agenda_still_says_when_it_is_looking() {
        assert_eq!(
            describe(&ViewArgs::parse(&args(&[])), 7, ANCHOR),
            "Week 2026-09-02 \u{2013} 2026-09-08"
        );
    }

    /// A one-day agenda is a date, not a range with nothing in it.
    #[test]
    fn a_daily_agenda_shows_one_date() {
        let v = ViewArgs::parse(&args(&["", "span=1"]));
        assert_eq!(describe(&v, 7, ANCHOR), "Day 2026-09-02");
    }

    /// The case the slice exists for: a filter must be visible, because
    /// "nothing scheduled" under a forgotten one is a lie.
    #[test]
    fn a_filter_is_always_named() {
        let v = ViewArgs::parse(&args(&["", "tag:work"]));
        assert!(describe(&v, 7, ANCHOR).ends_with("\u{b7} +work"));
        let v = ViewArgs::parse(&args(&["", "tag:work", "file:notes.org"]));
        assert!(describe(&v, 7, ANCHOR).ends_with("+work \u{b7} file:notes.org"));
    }

    /// Composed filters all appear — `\` narrows by ANDing another tag, and a
    /// header showing only the first would misdescribe what is on screen.
    #[test]
    fn composed_tag_filters_are_all_named() {
        let v = ViewArgs::parse(&args(&["", "tag:work", "tag:urgent"]));
        assert!(describe(&v, 7, ANCHOR).ends_with("+work \u{b7} +urgent"));
    }

    #[test]
    fn a_command_names_itself_after_the_window() {
        let v = ViewArgs::parse(&args(&["waiting"]));
        assert_eq!(
            describe(&v, 7, ANCHOR),
            "Week 2026-09-02 \u{2013} 2026-09-08 \u{b7} waiting"
        );
    }

    /// An offset moves the DATES rather than adding a word — which is the
    /// point of showing them, and why "next" was not worth saying.
    #[test]
    fn an_offset_moves_the_window() {
        let v = ViewArgs::parse(&args(&["", "offset=1"]));
        assert_eq!(
            describe(&v, 7, ANCHOR + 7),
            "Week 2026-09-09 \u{2013} 2026-09-15",
            "the caller anchors; this renders what it was given"
        );
    }

    /// A bad argument has nowhere else to go — the guest cannot log without
    /// making the whole component fail to instantiate.
    #[test]
    fn an_unrecognised_argument_is_reported_here_or_nowhere() {
        let v = ViewArgs::parse(&args(&["", "spam=1"]));
        assert!(!v.problems.is_empty(), "the parser recorded it");
        let said = describe(&v, 7, ANCHOR);
        assert!(
            said.contains('\u{26a0}') && said.contains("spam"),
            "the headerline names it: {said:?}"
        );
    }

    /// Everything at once, in a stable order: window, command, filters, then
    /// problems last.
    #[test]
    fn the_parts_come_in_a_stable_order() {
        let v = ViewArgs::parse(&args(&["waiting", "span=1", "tag:work", "nope=1"]));
        assert_eq!(
            describe(&v, 7, ANCHOR),
            "Day 2026-09-02 \u{b7} waiting \u{b7} +work \u{b7} \u{26a0} unknown view argument `nope`"
        );
    }
}

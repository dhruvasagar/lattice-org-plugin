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
    pub offset: i32,
    /// The filters the user has narrowed by, in the order they added them.
    pub filters: Vec<FilterTerm>,
    /// Arguments that were not understood, named for the headerline. Never
    /// fatal — see the module header.
    pub problems: Vec<String>,
}

impl ViewArgs {
    /// The default agenda, unwalked and unfiltered. `const` so it can seed a
    /// `thread_local` without a lazy init.
    pub const fn new() -> Self {
        Self {
            command: String::new(),
            span: None,
            offset: 0,
            filters: Vec::new(),
            problems: Vec::new(),
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
                match key {
                    "tag" if !value.is_empty() => {
                        out.filters.push(FilterTerm::Tag(value.to_string()));
                        continue;
                    }
                    "file" if !value.is_empty() => {
                        out.filters.push(FilterTerm::File(value.to_string()));
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
        if self.offset != 0 {
            out.push(format!("offset={}", self.offset));
        }
        for f in &self.filters {
            out.push(match f {
                FilterTerm::Tag(t) => format!("tag:{t}"),
                FilterTerm::File(f) => format!("file:{f}"),
            });
        }
        out
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
                FilterTerm::Tag(_) => None,
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
            FilterTerm::File(_) => true,
        })
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
            "tag:work",
            "file:a.org",
        ]));
        let round = ViewArgs::parse(&original.to_args());
        assert_eq!(round, original);
    }

    #[test]
    fn a_default_view_round_trips_to_something_parse_reads_back() {
        // The empty command is still emitted positionally, so the output is
        // never mistaken for a filter and never shifts what follows it.
        let a = ViewArgs {
            command: String::new(),
            span: None,
            offset: 0,
            filters: vec![FilterTerm::Tag("work".into())],
            problems: Vec::new(),
        };
        assert_eq!(a.to_args(), vec!["".to_string(), "tag:work".to_string()]);
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
        assert_eq!(ViewArgs::parse(&a.to_args()), a);
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
    parts.push(window(anchor, span));
    if !view.command.is_empty() {
        parts.push(view.command.clone());
    }
    for f in &view.filters {
        parts.push(match f {
            // `+work` and `file:notes.org` — org's own spellings, so what the
            // header shows is what the user would type to reproduce it.
            FilterTerm::Tag(t) => format!("+{t}"),
            FilterTerm::File(f) => format!("file:{f}"),
        });
    }
    for p in &view.problems {
        parts.push(format!("\u{26a0} {p}"));
    }
    parts.join(" \u{b7} ")
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
mod describe_tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    /// 2026-09-02, so every assertion states the day it expects.
    const ANCHOR: i64 = 20698;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
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
            "2026-09-02 \u{2013} 2026-09-08"
        );
    }

    /// A one-day agenda is a date, not a range with nothing in it.
    #[test]
    fn a_daily_agenda_shows_one_date() {
        let v = ViewArgs::parse(&args(&["", "span=1"]));
        assert_eq!(describe(&v, 7, ANCHOR), "2026-09-02");
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
            "2026-09-02 \u{2013} 2026-09-08 \u{b7} waiting"
        );
    }

    /// An offset moves the DATES rather than adding a word — which is the
    /// point of showing them, and why "next" was not worth saying.
    #[test]
    fn an_offset_moves_the_window() {
        let v = ViewArgs::parse(&args(&["", "offset=1"]));
        assert_eq!(
            describe(&v, 7, ANCHOR + 7),
            "2026-09-09 \u{2013} 2026-09-15",
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
            "2026-09-02 \u{b7} waiting \u{b7} +work \u{b7} \u{26a0} unknown view argument `nope`"
        );
    }
}

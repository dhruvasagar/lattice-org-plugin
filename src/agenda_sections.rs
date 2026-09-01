//! AS.2 — the agenda's section set, and where it is declared.
//!
//! `org.agenda-sections` is a **string option whose value is TOML**. Forced
//! rather than preferred, for `capture-templates`' reason (OC.2): a section is
//! a record — a title and a filter — and an option is
//! `boolean | integer | string`, with list support restricted to scalar
//! elements. An array-of-tables cannot reach an option at all.
//!
//! ```toml
//! [org]
//! agenda-sections = '''
//! [[section]]
//! title = "Overdue"
//! when = "overdue"
//! todo-only = true
//!
//! [[section]]
//! title = "This week"
//! when = "days"
//! days = 7
//!
//! [[section]]
//! title = "Unscheduled"
//! when = "undated"
//! todo-only = true
//!
//! [[section]]
//! title = "Priority A"
//! when = "any"
//! todo-only = true
//! min-priority = "A"
//! '''
//! ```
//!
//! ## One option, both homes
//!
//! This is the whole of the "TOML *and* `init.rs`" answer, and it needed no new
//! seam. `lattice.toml` sets the option declaratively; `init.rs` sets the
//! identical string through `config::set_option("org.agenda-sections", …)`,
//! which is already how a user reaches any plugin option (`auto-pair.style` is
//! the shipped example). Precedence between the two is the config resolver's
//! existing layering rather than anything invented here — so there is no second
//! ordering rule to learn, and no third place to look.
//!
//! A Rust raw string literal carries the payload verbatim from `init.rs`, and a
//! TOML `'''` literal block does the same from `lattice.toml`. One format, two
//! homes.
//!
//! ## Failure is not silence, and not an empty agenda
//!
//! **The guest cannot log.** Calling `logging::log` makes the component IMPORT
//! `logging`, and org's multi-seam linker does not wire that import, so the
//! whole component fails to instantiate — tried, reverted, and documented at
//! `lib.rs`'s capture-template resolver as the fifth repeat of the TC.6
//! multi-seam-linker rule. That is a host fix, not something to work around
//! here.
//!
//! So a broken set reports itself through the only channel this code owns: the
//! **section titles**, which are the view's own headers. A malformed set falls
//! back to the built-in sections with the parse error prefixed onto the first
//! one, so the agenda still works and says why it is not the one you wrote.
//!
//! Falling back rather than showing nothing is deliberate. An empty agenda and
//! a correct-but-empty agenda look identical, and "you have no tasks" is the
//! single worst thing this view can say incorrectly.
//!
//! Design: `docs/dev/architecture/org-mode.md` §6.1a.

use serde::Deserialize;

use crate::agenda::{default_sections, Filter, Section, When};

// ---- The on-the-wire shape, deserialised then validated ----

#[derive(Deserialize)]
struct RawSet {
    #[serde(default)]
    section: Vec<RawSection>,
}

#[derive(Deserialize)]
struct RawSection {
    #[serde(default)]
    title: String,
    /// `overdue` | `days` | `undated` | `any`. Lower-cased before matching, so
    /// `"Any"` works — a config file is prose to the person writing it.
    #[serde(default)]
    when: String,
    /// Only meaningful for `when = "days"`. Absent means "use
    /// `org.agenda-span`", which is what keeps that option meaningful once a
    /// user writes their own sections.
    #[serde(default)]
    days: Option<u32>,
    /// Dashed, matching org's own `:`-keyword spellings rather than inventing
    /// a snake_case variant of a name users already read elsewhere.
    #[serde(default, rename = "todo-only")]
    todo_only: Option<bool>,
    #[serde(default, rename = "min-priority")]
    min_priority: Option<String>,
    /// OA.10: org's tags/todo match — `"-CANCELLED+WAITING|HOLD/!"`. Absent
    /// means the section does not constrain tags, which is not the same as
    /// requiring none.
    #[serde(default)]
    r#match: Option<String>,
}

/// A parsed set, plus what was dropped getting there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSet {
    pub sections: Vec<Section>,
    /// Sections that parsed but were unusable, named. Rides back rather than
    /// being logged, for the module header's reason.
    pub skipped: Vec<String>,
}

/// Why a section set could not be read. Mirrors `TemplateError`'s two levels,
/// and for the same reason: "you have not configured this" and "your
/// configuration is broken" are different problems with different fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionError {
    /// Unset or blank — the ordinary case, and not an error to the user. The
    /// built-in set is the documented default.
    Unset,
    /// The TOML did not parse. Carries the parser's own message: the line and
    /// column in it are the whole value of reporting this at all.
    Malformed(String),
    /// It parsed, but nothing in it was usable.
    Empty,
}

impl SectionError {
    /// The one-line notice prefixed onto the fallback set's first header.
    pub fn notice(&self) -> Option<String> {
        match self {
            // Not a problem: no configuration means the defaults, which is
            // what the overwhelming majority of users want and never touch.
            SectionError::Unset => None,
            SectionError::Malformed(e) => {
                Some(format!("⚠ org.agenda-sections: {e} — using defaults"))
            }
            SectionError::Empty => {
                Some("⚠ org.agenda-sections: no usable sections — using defaults".to_string())
            }
        }
    }
}

/// Parse the option's value into a section set.
///
/// Two failure levels, deliberately different:
///
/// - **The whole set is malformed** ⇒ `Err`. The user's configuration does not
///   parse, and composing an agenda from the half of it that happened to
///   survive would be guessing at what they meant.
/// - **One section is unusable** (no title, an unknown `when`) ⇒ skipped, named
///   in `skipped`, and the rest survive. One typo should not cost the feature.
///
/// `default_days` is `org.agenda-span`, used by any `when = "days"` section
/// that does not name its own.
pub fn parse(source: &str, default_days: u32) -> Result<ParsedSet, SectionError> {
    if source.trim().is_empty() {
        return Err(SectionError::Unset);
    }
    let raw: RawSet =
        toml::from_str(source).map_err(|e| SectionError::Malformed(e.message().to_string()))?;

    let mut sections: Vec<Section> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for (i, s) in raw.section.into_iter().enumerate() {
        let title = s.title.trim().to_string();
        if title.is_empty() {
            // Named by position, since there is no title to name it by. A
            // section with no header is not renderable: the host titles the
            // first row of each group run with it, and an empty title is how
            // a row says "I continue the group above".
            skipped.push(format!("section {} has no `title`", i + 1));
            continue;
        }
        let Some(when) = parse_when(&s.when, s.days, default_days) else {
            skipped.push(format!(
                "`{title}` has an unknown `when = \"{}\"` (expected overdue, days, undated or any)",
                s.when
            ));
            continue;
        };
        let min_priority = match s.min_priority.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(p) => {
                let mut chars = p.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase()),
                    _ => {
                        skipped.push(format!(
                            "`{title}` has a `min-priority = \"{p}\"` that is not a single letter"
                        ));
                        continue;
                    }
                }
            }
        };
        // OA.10: a malformed match costs THIS section and names it, the same
        // as an unknown `when`. Failing the whole set would let one typo in
        // one block cost you every other one.
        let r#match = match s.r#match.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(src) => match crate::agenda_match::parse(src) {
                Ok(m) => Some(m),
                Err(e) => {
                    skipped.push(format!("`{title}` has a bad `match`: {}", e.message()));
                    continue;
                }
            },
        };
        sections.push(Section {
            title,
            filter: Filter {
                when,
                // Defaults to false — a section says what it takes, and
                // "everything in the window" is the less surprising silence.
                todo_only: s.todo_only.unwrap_or(false),
                min_priority,
                r#match,
            },
        });
    }

    if sections.is_empty() {
        return Err(SectionError::Empty);
    }
    Ok(ParsedSet { sections, skipped })
}

fn parse_when(raw: &str, days: Option<u32>, default_days: u32) -> Option<When> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "overdue" => Some(When::Overdue),
        "days" => Some(When::Days(days.unwrap_or(default_days))),
        "undated" => Some(When::Undated),
        "any" => Some(When::Any),
        _ => None,
    }
}

/// The section set a scan runs with: the user's if it parsed, the built-ins
/// otherwise, with any complaint prefixed onto the first header.
///
/// One function so `begin` has a single call and cannot accidentally skip the
/// fallback — an agenda with an empty section list shows nothing at all, and
/// "you have no tasks" is the worst thing this view can say incorrectly.
pub fn resolve(source: &str, default_days: u32) -> Vec<Section> {
    match parse(source, default_days) {
        Ok(set) => {
            // A partial set is used as-is: the sections that parsed are what
            // the user asked for, and the skips ride in `set.skipped` for a
            // caller that has somewhere to put them. Nothing does yet — see
            // the module header on why the guest cannot log.
            set.sections
        }
        Err(e) => with_notice(default_sections(default_days), e.notice()),
    }
}

/// Prefix `notice` onto the first section's title, or return the set unchanged.
///
/// The first section rather than a synthetic one of its own, because a section
/// with no rows renders no header: the host attaches a group title to a ROW, so
/// a notice-only section would be invisible in exactly the situation where a
/// user most needs to read it.
fn with_notice(mut sections: Vec<Section>, notice: Option<String>) -> Vec<Section> {
    if let (Some(notice), Some(first)) = (notice, sections.first_mut()) {
        first.title = format!("{notice} — {}", first.title);
    }
    sections
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const SPAN: u32 = 7;

    #[test]
    fn a_full_set_parses_into_sections_in_order() {
        let set = parse(
            r#"
[[section]]
title = "Overdue"
when = "overdue"
todo-only = true

[[section]]
title = "This week"
when = "days"
days = 3

[[section]]
title = "Unscheduled"
when = "undated"
todo-only = true

[[section]]
title = "Important"
when = "any"
todo-only = true
min-priority = "b"
"#,
            SPAN,
        )
        .unwrap();
        assert!(set.skipped.is_empty(), "got {:?}", set.skipped);
        let shapes: Vec<_> = set
            .sections
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
            shapes,
            [
                ("Overdue", When::Overdue, true, None),
                ("This week", When::Days(3), false, None),
                ("Unscheduled", When::Undated, true, None),
                // Lower-cased in the config, upper-cased here: a priority
                // letter is a letter, not a spelling.
                ("Important", When::Any, true, Some('B')),
            ]
        );
    }

    /// `when = "days"` with no `days` inherits `org.agenda-span`, which is what
    /// keeps that option meaningful once a user writes their own sections.
    #[test]
    fn a_days_section_without_its_own_count_inherits_the_span() {
        let set = parse("[[section]]\ntitle = \"Week\"\nwhen = \"days\"\n", 9).unwrap();
        assert_eq!(set.sections[0].filter.when, When::Days(9));
    }

    /// One bad section is skipped and named; the rest survive. A typo must not
    /// cost the whole feature.
    #[test]
    fn an_unusable_section_is_skipped_and_named() {
        let set = parse(
            r#"
[[section]]
title = "Good"
when = "any"

[[section]]
when = "any"

[[section]]
title = "Bad when"
when = "someday"

[[section]]
title = "Bad priority"
when = "any"
min-priority = "AA"
"#,
            SPAN,
        )
        .unwrap();
        assert_eq!(set.sections.len(), 1);
        assert_eq!(set.sections[0].title, "Good");
        assert_eq!(set.skipped.len(), 3, "got {:?}", set.skipped);
        assert!(set.skipped[0].contains("section 2"));
        assert!(set.skipped[1].contains("Bad when") && set.skipped[1].contains("someday"));
        assert!(set.skipped[2].contains("Bad priority"));
    }

    /// Unset is not an error to the user — it is how nearly everyone runs.
    #[test]
    fn unset_and_blank_are_the_default_set_without_a_complaint() {
        assert_eq!(parse("", SPAN), Err(SectionError::Unset));
        assert_eq!(parse("   \n\n", SPAN), Err(SectionError::Unset));
        assert_eq!(SectionError::Unset.notice(), None);
        assert_eq!(resolve("", SPAN), default_sections(SPAN));
    }

    /// Malformed falls back to the defaults AND says so in the first header —
    /// the only channel this code owns, since the guest cannot log.
    #[test]
    fn a_malformed_set_falls_back_to_defaults_and_says_so() {
        let err = parse("[[section]]\ntitle = ", SPAN).unwrap_err();
        assert!(matches!(err, SectionError::Malformed(_)), "got {err:?}");

        let resolved = resolve("[[section]]\ntitle = ", SPAN);
        let defaults = default_sections(SPAN);
        assert_eq!(
            resolved.len(),
            defaults.len(),
            "the agenda still works on the built-in set"
        );
        assert!(
            resolved[0].title.starts_with("⚠ org.agenda-sections:"),
            "the first header carries the complaint, got {:?}",
            resolved[0].title
        );
        assert!(
            resolved[0].title.ends_with(&defaults[0].title),
            "…and still names the section it is, got {:?}",
            resolved[0].title
        );
        // Only the first: a notice repeated on every header is noise.
        assert_eq!(resolved[1].title, defaults[1].title);
        // The FILTERS are untouched — a broken config costs you your layout,
        // never your rows.
        for (r, d) in resolved.iter().zip(defaults.iter()) {
            assert_eq!(r.filter, d.filter);
        }
    }

    /// Parses, but nothing usable in it. Distinct from malformed because the
    /// fix is different: the TOML is fine, the sections are not.
    #[test]
    fn a_set_with_no_usable_sections_is_empty_not_malformed() {
        assert_eq!(
            parse("[[section]]\nwhen = \"any\"\n", SPAN),
            Err(SectionError::Empty)
        );
        // An array of nothing is the same class.
        assert_eq!(parse("other = 1\n", SPAN), Err(SectionError::Empty));

        let resolved = resolve("[[section]]\nwhen = \"any\"\n", SPAN);
        assert!(resolved[0].title.contains("no usable sections"));
    }

    /// OA.10: a section's `match` reaches `Filter` as parsed data.
    #[test]
    fn a_section_match_parses_into_the_filter() {
        let set = parse(
            "[[section]]\ntitle = \"Waiting\"\nwhen = \"any\"\nmatch = \"-CANCELLED+WAITING|HOLD/!\"\n",
            7,
        )
        .expect("parses");
        let m = set.sections[0]
            .filter
            .r#match
            .as_ref()
            .expect("the match reached the filter");
        assert_eq!(m.alternatives.len(), 2, "`|` is two alternatives");
        assert_eq!(m.todo, crate::agenda_match::TodoMatch::NotDone);
    }

    /// A bad match costs its own section and names it — the same shape an
    /// unknown `when` already has. Failing the whole set would let one typo
    /// in one block cost every other block too.
    #[test]
    fn a_bad_match_skips_its_section_and_says_which() {
        let set = parse(
            "[[section]]\ntitle = \"Good\"\nwhen = \"any\"\n\n\
             [[section]]\ntitle = \"Bad\"\nwhen = \"any\"\nmatch = \"{^work}\"\n",
            7,
        )
        .expect("the set still parses");
        assert_eq!(set.sections.len(), 1, "the good one survives");
        assert_eq!(set.sections[0].title, "Good");
        assert_eq!(set.skipped.len(), 1);
        assert!(
            set.skipped[0].contains("Bad") && set.skipped[0].contains("match"),
            "the notice names the section and the problem: {:?}",
            set.skipped[0]
        );
    }

    /// Absent and empty both mean "does not constrain tags", which is not the
    /// same as "requires none".
    #[test]
    fn an_absent_match_constrains_nothing() {
        for body in [
            "[[section]]\ntitle = \"A\"\nwhen = \"any\"\n",
            "[[section]]\ntitle = \"A\"\nwhen = \"any\"\nmatch = \"\"\n",
        ] {
            let set = parse(body, 7).expect("parses");
            assert!(set.sections[0].filter.r#match.is_none(), "on {body:?}");
        }
    }
}

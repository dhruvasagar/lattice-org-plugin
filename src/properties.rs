//! OE.1 — reading and writing an entry's `:PROPERTIES:` drawer.
//!
//! Design: `docs/dev/architecture/org-mode.md` §5.5 in the lattice tree.
//! Slice plan: `docs/dev/operations/slice-plans/org-entry-editing.md`.
//!
//! ## Why this is a module and not a second `set_property`
//!
//! Properties were already written in two places and read in three, with no
//! two agreeing on what "write a property" means. `roam_index::id_drawer_insert`
//! finds-or-creates a drawer but refuses when the key is present, because a
//! second `:ID:` is a file org cannot read. `complete.rs`'s private
//! `set_property` replaces in place but will NOT create a drawer, because
//! manufacturing three lines on a plain repeating task to record a
//! `LAST_REPEAT` is worse than not recording it.
//!
//! Both are right for their caller and neither is what a user who typed
//! `:org-set-property` wants, which is: put this key here, whatever is there
//! now. So the general answer lives here, the special cases stay decisions
//! their callers make, and **the one thing all three must agree on** — where a
//! drawer starts — is [`drawer_line_for`], written once.
//!
//! ## The answer is an edit, not a rewritten file
//!
//! Every caller here holds a `Document` and returns `Effect::ApplyEdit`, so a
//! function that returned new text would be handing back something the caller
//! has to diff. [`PropertyEdit`] names the line and what happens to it, which
//! is what the effect wants and what a test can assert without an editor.
//!
//! The file is read through an accessor rather than materialised, for
//! `headline.rs`'s reason: a drawer is a handful of lines under its headline,
//! and collecting the document to look at four of them scales the cost with
//! the file instead of with the drawer.

/// What writing one property comes to, as an edit over lines.
///
/// `text` is whole lines with **no trailing newline**, because the caller
/// anchors an insert on the line above and joins with one — the shape
/// `plan_submit` already uses, and the one that stays correct at end of
/// document where the line being inserted before does not exist yet.
///
/// Two arms rather than one "replace this range" because the callers differ in
/// what they must not do: an insert must not overwrite the line it lands on,
/// and a replace must not leave the old value behind. Collapsing them into a
/// range would make both spellings look the same at the call site, and the
/// difference is the whole of what OE.0 got wrong in the other direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyEdit {
    /// Insert `text` *before* line `at`, pushing what is there down.
    Insert { at: u32, text: String },
    /// Replace line `at` entirely with `text`.
    Replace { at: u32, text: String },
}

/// OE.0 — the line an entry's `:PROPERTIES:` drawer starts on, whether or not
/// one is there yet.
///
/// One past the headline, unless that line is the entry's **planning line**, in
/// which case one past that. tree-sitter-org's `section` rule is
/// `headline, [plan], [property_drawer], [body], subsection*` — a SEQ, so the
/// plan comes first, and a drawer written above it takes the `plan` field out
/// of the tree entirely: `SCHEDULED: <…>` becomes a paragraph in `body` with
/// its timestamp untokenised, and `agenda.rs`, which reads the date from that
/// field, stops seeing it.
///
/// A plan is ONE line to this grammar — `SCHEDULED:` on one line and
/// `DEADLINE:` on the next parses only the first — so this needs no loop.
///
/// [`crate::planning::parse`] decides what counts, and its strictness is load
/// bearing in one direction: a prose line that merely mentions `SCHEDULED:`
/// must not push the drawer past it.
pub fn drawer_line_for(line: &dyn Fn(u32) -> Option<String>, headline_line: u32) -> u32 {
    let first = headline_line + 1;
    match line(first) {
        Some(text) if crate::planning::parse(&text).is_some() => first + 1,
        _ => first,
    }
}

/// `:KEY:` padded the way org writes it.
///
/// `org-property-format` defaults to `"%-10s %s"`, which is why every drawer a
/// user has seen has its values in one column. Matching it means a property
/// this writes sits flush with the ones org wrote; not matching it would make
/// every touched drawer look edited.
pub fn format_property(indent: &str, key: &str, value: &str) -> String {
    let name = format!(":{key}:");
    // `{:<10}` leaves a long key unpadded rather than truncating it — a
    // truncated key is a different property.
    format!("{indent}{name:<10} {value}").trim_end().to_string()
}

/// Is `text` the opener of a properties drawer?
fn opens_drawer(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case(":properties:")
}

/// The whitespace `text` begins with.
fn leading_ws(text: &str) -> String {
    text.chars().take_while(|c| c.is_whitespace()).collect()
}

/// Does this drawer line declare `key`?
///
/// Case-insensitively: org is not consistent about property-name case, and
/// writing a second `:id:` beside an existing `:ID:` produces a drawer with two
/// values for one property — which org reads as the first and the user reads as
/// the second.
fn declares(text: &str, key: &str) -> bool {
    text.trim()
        .strip_prefix(':')
        .and_then(|rest| rest.split_once(':'))
        .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(key))
}

/// The edit that makes `:KEY: value` true of the entry at `headline_line`.
///
/// Three cases, and the third is the one `id_drawer_insert` cannot do:
///
/// | State | Answer |
/// |---|---|
/// | No drawer | insert a whole `:PROPERTIES:` / `:KEY:` / `:END:` block |
/// | Drawer, key absent | insert the line just inside the opener |
/// | Drawer, key present | **replace** that line |
///
/// Replacing is right here and wrong for `:ID:`, which is why that caller keeps
/// its own refusal rather than this function guessing which caller it has.
///
/// A drawer that is never closed stops the walk at the next headline: scanning
/// past it would set the property on the WRONG entry, which is the failure this
/// shares with [`crate::roam_index::id_drawer_insert`] and the reason both walk
/// the same way.
pub fn set_entry_property(
    line: &dyn Fn(u32) -> Option<String>,
    headline_line: u32,
    key: &str,
    value: &str,
) -> PropertyEdit {
    let first = drawer_line_for(line, headline_line);
    let opener = line(first);

    let Some(opener) = opener.filter(|text| opens_drawer(text)) else {
        // No drawer: write a whole one, unindented, as org does under a
        // headline at column 0.
        return PropertyEdit::Insert {
            at: first,
            text: format!(":PROPERTIES:\n{}\n:END:", format_property("", key, value)),
        };
    };

    // The drawer's own indentation, so an added line sits with the ones already
    // there rather than announcing which tool wrote it.
    let indent = leading_ws(&opener);
    let mut i = first + 1;
    while let Some(text) = line(i) {
        let trimmed = text.trim();
        if trimmed.eq_ignore_ascii_case(":end:") || trimmed.starts_with('*') {
            break;
        }
        if declares(&text, key) {
            return PropertyEdit::Replace {
                at: i,
                text: format_property(&leading_ws(&text), key, value),
            };
        }
        i += 1;
    }

    // Present but not declared: just inside the opener. The top rather than
    // above `:END:` because that is where `id_drawer_insert` puts an `:ID:`,
    // and a drawer whose entries land in different places depending on which
    // command wrote them reads as disordered.
    PropertyEdit::Insert {
        at: first + 1,
        text: format_property(&indent, key, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file as the accessor the production caller passes.
    fn lines(text: &str) -> impl Fn(u32) -> Option<String> + '_ {
        let owned: Vec<String> = text.lines().map(str::to_string).collect();
        move |n: u32| owned.get(n as usize).cloned()
    }

    #[test]
    fn a_headline_with_no_drawer_gets_a_whole_one() {
        let l = lines("* Topic\nbody\n");
        assert_eq!(
            set_entry_property(&l, 0, "CATEGORY", "work"),
            PropertyEdit::Insert {
                at: 1,
                text: ":PROPERTIES:\n:CATEGORY: work\n:END:".to_string(),
            }
        );
    }

    /// OE.0's rule, inherited rather than re-derived — the drawer goes below
    /// the plan, or the agenda stops seeing the date.
    #[test]
    fn a_new_drawer_goes_below_the_planning_line() {
        let l = lines("* TODO Task\nSCHEDULED: <2026-09-04 Fri>\nbody\n");
        let PropertyEdit::Insert { at, .. } = set_entry_property(&l, 0, "CATEGORY", "work") else {
            panic!("a headline with no drawer gets one");
        };
        assert_eq!(at, 2);
    }

    #[test]
    fn an_existing_drawer_is_extended_at_the_top() {
        let l = lines("* Topic\n:PROPERTIES:\n:ID:       ABC\n:END:\n");
        assert_eq!(
            set_entry_property(&l, 0, "CATEGORY", "work"),
            PropertyEdit::Insert {
                at: 2,
                text: ":CATEGORY: work".to_string(),
            }
        );
    }

    /// The case `id_drawer_insert` refuses and this one must not: a user who
    /// typed the command wants the value they typed.
    #[test]
    fn an_existing_key_is_replaced_not_duplicated() {
        let l = lines("* Topic\n:PROPERTIES:\n:CATEGORY: old\n:END:\n");
        assert_eq!(
            set_entry_property(&l, 0, "CATEGORY", "new"),
            PropertyEdit::Replace {
                at: 2,
                text: ":CATEGORY: new".to_string(),
            }
        );
    }

    /// Org is not consistent about property-name case, and a drawer carrying
    /// `:category:` beside `:CATEGORY:` has two values for one property.
    #[test]
    fn a_key_differing_only_in_case_is_the_same_key() {
        let l = lines("* Topic\n:PROPERTIES:\n:category: old\n:END:\n");
        let PropertyEdit::Replace { at, .. } = set_entry_property(&l, 0, "CATEGORY", "new") else {
            panic!("the same property, however it is spelled");
        };
        assert_eq!(at, 2);
    }

    /// A value with a colon in it. A writer that split the LINE on `:` to find
    /// the key would truncate this to `http`, and it is the most obvious thing
    /// anyone puts in a property.
    #[test]
    fn a_value_may_contain_colons() {
        let l = lines("* Topic\n:PROPERTIES:\n:URL: old\n:END:\n");
        assert_eq!(
            set_entry_property(&l, 0, "URL", "https://example.com:8443/x"),
            PropertyEdit::Replace {
                at: 2,
                text: ":URL:      https://example.com:8443/x".to_string(),
            }
        );
    }

    /// An indented drawer keeps its indent, so the added line does not
    /// announce which tool wrote it.
    #[test]
    fn an_added_line_matches_the_drawers_indent() {
        let l = lines("* Topic\n  :PROPERTIES:\n  :ID:       ABC\n  :END:\n");
        assert_eq!(
            set_entry_property(&l, 0, "CATEGORY", "work"),
            PropertyEdit::Insert {
                at: 2,
                text: "  :CATEGORY: work".to_string(),
            }
        );
    }

    /// Malformed input: an unterminated drawer. Walking past the next headline
    /// would set the property on the entry BELOW, which is silent damage —
    /// both entries still look plausible afterwards.
    #[test]
    fn an_unterminated_drawer_stops_at_the_next_headline() {
        let l = lines("* One\n:PROPERTIES:\n* Two\n:PROPERTIES:\n:CATEGORY: theirs\n:END:\n");
        assert_eq!(
            set_entry_property(&l, 0, "CATEGORY", "mine"),
            PropertyEdit::Insert {
                at: 2,
                text: ":CATEGORY: mine".to_string(),
            },
            "the first entry's own drawer, not the second's line"
        );
    }

    /// Prose is not a plan — `planning::parse`'s strictness, relied on here.
    #[test]
    fn prose_mentioning_scheduled_does_not_move_the_drawer() {
        let l = lines("* Topic\nSCHEDULED: is how org spells it\n");
        let PropertyEdit::Insert { at, .. } = set_entry_property(&l, 0, "CATEGORY", "work") else {
            panic!("no drawer here");
        };
        assert_eq!(at, 1);
    }

    /// org's `org-property-format` (`%-10s %s`), which is why every drawer a
    /// user has seen has its values in one column.
    #[test]
    fn properties_are_padded_the_way_org_pads_them() {
        assert_eq!(format_property("", "ID", "ABC"), ":ID:       ABC");
        assert_eq!(format_property("", "CATEGORY", "work"), ":CATEGORY: work");
        assert_eq!(
            format_property("", "A_VERY_LONG_KEY", "v"),
            ":A_VERY_LONG_KEY: v",
            "a long key is not truncated — a truncated key is a different property"
        );
    }
}

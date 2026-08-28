//! Org headline structure: asked of the parse tree, with the line logic behind
//! it as the fallback.
//!
//! ## Two answers to every question, and which one wins
//!
//! Everything in the first half of this file resolves a headline by matching
//! `*` at the start of a line. [`Headlines`], at the bottom, asks the same
//! questions of the org grammar and falls back to the line logic only when
//! there is no tree to ask.
//!
//! ### Why the tree is the primary answer (OT.4)
//!
//! This module's doc-comment used to argue the opposite — that a line whose
//! first non-`*` character is a space, at column 0, **is** a headline in org
//! "including inside `#+BEGIN_SRC`, which is precisely why org makes you escape
//! such lines as `,*`". That is a claim about org's *escaping convention*, and
//! it does not survive contact with org's own grammar: `grammar.js` puts
//! `block` inside `body`, so a `* TODO` line between `#+BEGIN_SRC` and
//! `#+END_SRC` parses as block content and is not a `section`. The text matcher
//! sees a headline there; the grammar does not, and the grammar is what
//! highlights, folds and (since OT.3) scans the file.
//!
//! The consequence is not cosmetic. `ar` resolves a span from the phantom
//! headline to the "end of its subtree", which is a span that is not a subtree —
//! `dar` inside a source block deletes across the block's boundary. No care in
//! the line matcher fixes it, because the fact that decides it is not on the
//! line.
//!
//! That is the whole argument for OT.x in one case: **a bespoke parser must
//! agree with a grammar it never consults, and it does not.**
//!
//! ### Why the line logic stays
//!
//! Two of the three objections the old doc-comment raised were real, and both
//! are answered by keeping the text path as the fallback rather than deleting
//! it:
//!
//! * **The tree can be absent.** `none` when the parse is pending, or when the
//!   host has no org grammar registered at all. A chord that silently no-ops
//!   right after a paste is the worst kind of bug to report, so every method on
//!   [`Headlines`] degrades to the line answer instead of to nothing (goal #2).
//! * **Cost on the keystroke path.** Answered by *not* using a query: every walk
//!   here is `enclosing` plus a handful of `parent` / sibling steps, which is
//!   O(nesting depth) — a fixed handful of host calls whatever the file size,
//!   against the text path's "read lines until you find one" (goal #1).
//!
//! ## Why these take a line accessor and not a `&[String]`
//!
//! Reading a line crosses the WASM boundary (`document.line(n)`), so
//! materialising the buffer to find one headline costs one guest→host call per
//! line — 10,000 of them on a 10,000-line org file, on every press of a key
//! that ends up editing three of them. At the seam's own budget that is
//! milliseconds, which is a missed frame (paramount goal #1).
//!
//! So every function here takes `line: impl Fn(u32) -> Option<String>` and
//! reads only what it needs: upward until a headline appears, downward until
//! the subtree ends. A headline operation touches a handful of lines whatever
//! the file size; a subtree operation touches its subtree, which it must read
//! anyway in order to rewrite it.

/// The level of `line` if it is a headline, else `None`.
///
/// A headline is one or more `*` at column 0 followed by a space. The trailing
/// space matters: `**bold**` at the start of a line is not a level-2 headline,
/// and neither is a lone `*` on its own line (org requires the space, and a
/// bare `*` is a list bullet).
pub fn headline_level(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|b| *b == b'*').count();
    if stars == 0 {
        return None;
    }
    match line.as_bytes().get(stars) {
        Some(b' ') => Some(stars),
        _ => None,
    }
}

/// The headline at or above `from`, as `(line_index, level)`.
///
/// Walks backwards, so it answers "which headline am I under" from anywhere in
/// a subtree — the cursor is rarely sitting on the headline itself when you
/// promote it. `None` when nothing above the cursor is a headline (the preamble
/// before a file's first headline).
///
/// Reads one line at a time and stops at the first headline, so the cost is the
/// distance to it, not the size of the file.
pub fn enclosing_headline(line: impl Fn(u32) -> Option<String>, from: u32) -> Option<(u32, usize)> {
    (0..=from)
        .rev()
        .find_map(|i| headline_level(&line(i)?).map(|lvl| (i, lvl)))
}

/// The last line of the subtree rooted at headline line `start` (inclusive).
///
/// A subtree runs to just before the next headline of the same level or
/// shallower, so a level-2 headline's subtree swallows its level-3 children and
/// stops at the next level-2 or level-1. Runs to the last line when no such
/// headline follows.
///
/// Scans forward one line at a time and stops at the subtree's end, so a
/// headline near the top of a long file does not read the rest of it.
pub fn subtree_end(line: impl Fn(u32) -> Option<String>, start: u32, line_count: u32) -> u32 {
    let Some(level) = line(start).as_deref().and_then(headline_level) else {
        return start;
    };
    (start + 1..line_count)
        .find(|i| {
            line(*i)
                .as_deref()
                .and_then(headline_level)
                .is_some_and(|lvl| lvl <= level)
        })
        .map(|i| i - 1)
        .unwrap_or(line_count.saturating_sub(1))
}

/// Re-star a headline by `delta` levels, or `None` if `line` is not a headline
/// or the shift is refused.
///
/// Refused at level 1 going up: org has no level-0 headline, and turning `*
/// Title` into `Title` would silently convert a headline into body text of the
/// headline above — a destructive surprise from a key that means "promote".
/// Demotion has no ceiling; org files nest as deep as you like and only the
/// theme's size ramp runs out (it holds at level 6, see `doc/org.md`).
pub fn restar(line: &str, delta: isize) -> Option<String> {
    let level = headline_level(line)?;
    let new_level = level as isize + delta;
    if new_level < 1 {
        return None;
    }
    let rest = &line[level..];
    Some(format!("{}{}", "*".repeat(new_level as usize), rest))
}

/// Rewrite `lines[start..=end]`, shifting every headline among them by `delta`.
///
/// `start` must be the ROOT headline of the span, and if it cannot shift the
/// whole operation is refused — `None`, no edit at all.
///
/// That all-or-nothing rule is org's, and it matters. Promoting a level-1
/// subtree by moving only the descendants that *can* move would turn
///
/// ```org
/// * One
/// ** Child
/// ```
///
/// into two level-1 siblings: the child escapes its parent. Emacs refuses the
/// whole promote ("Cannot promote to level 0") for exactly this reason, and a
/// key that silently restructures a document is worse than one that declines.
///
/// Returning `None` rather than an unchanged string also lets the caller
/// decline the chord instead of pushing a no-op edit onto the undo stack.
///
/// Non-headline lines are copied verbatim, so a subtree's body text is
/// untouched even though the rewritten span covers it. The whole span is
/// replaced as ONE edit deliberately: N separate edits would be N undo steps,
/// and `u` after demoting a subtree must put every star back at once, not one
/// headline at a time.
pub fn shift_headlines(
    line: impl Fn(u32) -> Option<String>,
    start: u32,
    end: u32,
    delta: isize,
) -> Option<(String, u32)> {
    // The root gates the whole span. Every descendant is deeper, so if the
    // root can move they all can.
    restar(&line(start)?, delta)?;
    let mut out = String::new();
    let mut last_len = 0u32;
    for i in start..=end {
        let text = line(i)?;
        if i > start {
            out.push('\n');
        }
        // The ORIGINAL length: the edit's range is expressed in the document
        // as it stands, so the span ends at the old last line's end, not the
        // rewritten one. Capturing it here saves reading that line across the
        // boundary a second time.
        last_len = text.len() as u32;
        out.push_str(&restar(&text, delta).unwrap_or(text));
    }
    Some((out, last_len))
}

/// The next headline strictly after `from`, or `None` at the last one.
///
/// Any level — `]]` in org walks every headline, not only siblings. Stops at
/// the first hit, so cost is the distance to it.
pub fn next_headline(
    line: impl Fn(u32) -> Option<String>,
    from: u32,
    line_count: u32,
) -> Option<u32> {
    (from + 1..line_count).find(|i| line(*i).as_deref().and_then(headline_level).is_some())
}

/// The previous headline strictly before `from`, or `None` at the first one.
pub fn prev_headline(line: impl Fn(u32) -> Option<String>, from: u32) -> Option<u32> {
    (0..from)
        .rev()
        .find(|i| line(*i).as_deref().and_then(headline_level).is_some())
}

/// The parent of the headline enclosing `from` — the nearest headline above it
/// at a strictly shallower level.
///
/// `None` when the enclosing headline is already level 1, or when there is no
/// enclosing headline at all. Emacs's `outline-up-heading`, and the reason it
/// is a separate walk rather than "the previous headline": from a level-3
/// headline, the previous headline may be a level-3 sibling, and `g{` must skip
/// it to reach the level-2 parent.
pub fn parent_headline(line: impl Fn(u32) -> Option<String>, from: u32) -> Option<u32> {
    let (start, level) = enclosing_headline(&line, from)?;
    if level <= 1 {
        return None;
    }
    (0..start).rev().find(|i| {
        line(*i)
            .as_deref()
            .and_then(headline_level)
            .is_some_and(|lvl| lvl < level)
    })
}

// ── OM.6: subtree move, meta-return, toggle heading ──

/// The previous SIBLING of the headline at `start`: the nearest headline above
/// it at the same level, without crossing a shallower one.
///
/// Not "the previous headline" — from a level-2 headline the previous headline
/// may be a level-3 child of an earlier sibling, and moving the subtree past it
/// would interleave two trees. Stopping at a shallower level is what keeps the
/// move inside one parent.
pub fn prev_sibling(line: impl Fn(u32) -> Option<String>, start: u32, level: usize) -> Option<u32> {
    (0..start).rev().find_map(|i| {
        match line(i).as_deref().and_then(headline_level) {
            Some(lvl) if lvl == level => Some(Some(i)),
            // A shallower headline is the parent boundary: there is no earlier
            // sibling under it. `Some(None)` stops the scan; `find_map` on the
            // outer `Option` then yields `None`.
            Some(lvl) if lvl < level => Some(None),
            _ => None,
        }
    })?
}

/// The next SIBLING of the headline at `start`. The mirror of
/// [`prev_sibling`]; a shallower headline ends the parent's children.
pub fn next_sibling(
    line: impl Fn(u32) -> Option<String>,
    start: u32,
    level: usize,
    line_count: u32,
) -> Option<u32> {
    (start + 1..line_count).find_map(|i| match line(i).as_deref().and_then(headline_level) {
        Some(lvl) if lvl == level => Some(Some(i)),
        Some(lvl) if lvl < level => Some(None),
        _ => None,
    })?
}

/// Turn a line into a headline, or a headline back into a plain line.
///
/// `org-toggle-heading`. A body line becomes a headline at `level`; a headline
/// loses its stars. `None` for a line that is neither (an empty line has
/// nothing to promote and nothing to strip).
pub fn toggle_heading(line: &str, level: usize) -> Option<String> {
    if let Some(lvl) = headline_level(line) {
        // Strip the stars AND the single space that separates them, which is
        // part of the marker rather than part of the title.
        return Some(
            line[lvl..]
                .strip_prefix(' ')
                .unwrap_or(&line[lvl..])
                .to_string(),
        );
    }
    if line.trim().is_empty() {
        return None;
    }
    Some(format!("{} {line}", "*".repeat(level)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The accessor the real callers pass, over an in-memory buffer.
    fn buf(text: &str) -> (impl Fn(u32) -> Option<String> + use<>, u32) {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let count = lines.len() as u32;
        (move |i: u32| lines.get(i as usize).cloned(), count)
    }

    #[test]
    fn recognises_headlines_and_rejects_look_alikes() {
        assert_eq!(headline_level("* Top"), Some(1));
        assert_eq!(headline_level("*** Third"), Some(3));
        // No space ⇒ not a headline. `**bold**` at line start is the case that
        // would otherwise corrupt markup-heavy files.
        assert_eq!(headline_level("**bold** text"), None);
        assert_eq!(headline_level("*"), None);
        assert_eq!(headline_level(" * indented"), None);
        assert_eq!(headline_level("body text"), None);
        assert_eq!(headline_level(""), None);
        // A headline with no title is still a headline.
        assert_eq!(headline_level("** "), Some(2));
    }

    #[test]
    fn finds_the_enclosing_headline_from_inside_a_subtree() {
        let (l, _) = buf("* One\nbody\n** Two\nmore body\n");
        assert_eq!(enclosing_headline(&l, 0), Some((0, 1)));
        assert_eq!(enclosing_headline(&l, 1), Some((0, 1)));
        assert_eq!(enclosing_headline(&l, 3), Some((2, 2)));
    }

    /// The reason these take an accessor: the walk must stop at the headline,
    /// not read the file. Each read crosses the WASM boundary.
    #[test]
    fn the_upward_walk_stops_at_the_first_headline() {
        use std::cell::Cell;
        let reads = Cell::new(0u32);
        let lines: Vec<String> = std::iter::once("* Top".to_string())
            .chain((0..10_000).map(|i| format!("body {i}")))
            .collect();
        let accessor = |i: u32| {
            reads.set(reads.get() + 1);
            lines.get(i as usize).cloned()
        };
        // From the very bottom of a 10k-line file, one line above a headline.
        assert_eq!(enclosing_headline(accessor, 5), Some((0, 1)));
        assert!(
            reads.get() <= 6,
            "read {} lines to find a headline 5 lines up",
            reads.get()
        );
    }

    #[test]
    fn a_preamble_has_no_enclosing_headline() {
        let (l, _) = buf("#+TITLE: Notes\n\n* First\n");
        assert_eq!(enclosing_headline(&l, 0), None);
        assert_eq!(enclosing_headline(&l, 1), None);
    }

    #[test]
    fn a_subtree_swallows_children_and_stops_at_a_peer() {
        let (l, n) = buf("* One\nbody\n** Child\nkid body\n* Two\ntail\n");
        assert_eq!(subtree_end(&l, 0, n), 3, "stops before the next level-1");
        assert_eq!(
            subtree_end(&l, 2, n),
            3,
            "the child ends where its parent does"
        );
        assert_eq!(subtree_end(&l, 4, n), 5, "the last subtree runs to the end");
    }

    #[test]
    fn a_shallower_headline_also_terminates_a_subtree() {
        let (l, n) = buf("* One\n*** Deep\nbody\n** Mid\n");
        assert_eq!(
            subtree_end(&l, 1, n),
            2,
            "level 3 stops at the level 2 below it"
        );
    }

    #[test]
    fn restar_shifts_and_refuses_to_promote_past_level_one() {
        assert_eq!(restar("** Two", -1).as_deref(), Some("* Two"));
        assert_eq!(restar("* One", 1).as_deref(), Some("** One"));
        assert_eq!(
            restar("* One", -1),
            None,
            "org has no level 0; promoting would turn a headline into body text"
        );
        assert_eq!(restar("body", 1), None);
    }

    #[test]
    fn shifting_a_subtree_leaves_body_text_alone() {
        let (l, _) = buf("* One\nbody\n** Child\n");
        let (out, last_len) = shift_headlines(&l, 0, 2, 1).expect("something changed");
        assert_eq!(out, "** One\nbody\n*** Child");
        assert_eq!(
            last_len,
            "** Child".len() as u32,
            "the ORIGINAL last line's length, since the edit range is in \
             current-document coordinates"
        );
    }

    #[test]
    fn shifting_reports_no_change_rather_than_a_no_op_edit() {
        let (l, _) = buf("* One\nbody\n");
        assert_eq!(
            shift_headlines(&l, 0, 1, -1),
            None,
            "a level-1 root that cannot promote yields no edit at all"
        );
        // A span whose root is not a headline at all is refused too.
        let (not_a_headline, _) = buf("body\n* Later\n");
        assert_eq!(shift_headlines(&not_a_headline, 0, 1, 1), None);
        let (body_only, _) = buf("body\nmore\n");
        assert_eq!(shift_headlines(&body_only, 0, 1, 1), None);
    }

    #[test]
    fn headline_motions_walk_every_level() {
        let (l, n) = buf("* One\nbody\n** Child\nkid\n* Two\n");
        assert_eq!(next_headline(&l, 0, n), Some(2), "]] from a headline");
        assert_eq!(next_headline(&l, 1, n), Some(2), "]] from body text");
        assert_eq!(next_headline(&l, 2, n), Some(4));
        assert_eq!(next_headline(&l, 4, n), None, "no headline after the last");

        assert_eq!(prev_headline(&l, 4), Some(2));
        assert_eq!(prev_headline(&l, 2), Some(0));
        assert_eq!(prev_headline(&l, 0), None, "nothing before the first");
        assert_eq!(prev_headline(&l, 1), Some(0), "[[ from body text");
    }

    #[test]
    fn parent_skips_siblings_to_reach_a_shallower_headline() {
        // From the second level-3, the PREVIOUS headline is its level-3
        // sibling; the parent is the level-2 above both. This is the whole
        // reason `g{` is not just `[[`.
        let (l, _) = buf("* One\n** Two\n*** A\n*** B\nbody\n");
        assert_eq!(parent_headline(&l, 3), Some(1));
        assert_eq!(parent_headline(&l, 4), Some(1), "from body under B");
        assert_eq!(parent_headline(&l, 1), Some(0));
        assert_eq!(parent_headline(&l, 0), None, "a level-1 has no parent");
    }

    #[test]
    fn parent_is_none_outside_any_headline() {
        let (l, _) = buf("#+TITLE: Notes\nprose\n");
        assert_eq!(parent_headline(&l, 1), None);
    }

    #[test]
    fn a_subtree_promote_is_all_or_nothing() {
        // If the root cannot promote, nothing moves. Shifting only the
        // children would make `** Child` a level-1 sibling of `* One` — the
        // child escapes its parent, which is document corruption from a key
        // that means "promote".
        let (l, _) = buf("* One\n** Child\n*** Grand\n");
        assert_eq!(
            shift_headlines(&l, 0, 2, -1),
            None,
            "org refuses the whole promote rather than restructuring"
        );
        // Demote has no such ceiling; everything moves together.
        let (out, _) = shift_headlines(&l, 0, 2, 1).expect("demote always fits");
        assert_eq!(out, "** One\n*** Child\n**** Grand");
    }

    // ── OM.6 ──

    /// The distinction the whole slice turns on: a SIBLING is not "the
    /// previous headline". `** Two`'s predecessor by line order is `*** Kid`,
    /// but its sibling is `** One`. Swapping with the former would splice a
    /// level-2 subtree into the middle of another one's children.
    #[test]
    fn siblings_skip_over_a_previous_subtrees_children() {
        let (l, n) = buf("* Root\n** One\n*** Kid\nbody\n** Two\n*** Kid2\n** Three\n");
        assert_eq!(
            prev_sibling(&l, 4, 2),
            Some(1),
            "`** Two`'s sibling is `** One`"
        );
        assert_eq!(next_sibling(&l, 4, 2, n), Some(6), "and `** Three` follows");
        // The ends of the sibling chain.
        assert_eq!(prev_sibling(&l, 1, 2), None, "`** One` is the first child");
        assert_eq!(next_sibling(&l, 6, 2, n), None, "`** Three` is the last");
    }

    /// A shallower headline is a hard boundary. `** B` lives under a different
    /// parent from `** A`, so neither is the other's sibling — moving one past
    /// the other would silently reparent it.
    #[test]
    fn a_shallower_headline_ends_the_sibling_chain() {
        let (l, n) = buf("* First\n** A\n* Second\n** B\n");
        assert_eq!(
            prev_sibling(&l, 3, 2),
            None,
            "`* Second` blocks the scan up"
        );
        assert_eq!(next_sibling(&l, 1, 2, n), None, "and blocks the scan down");
        // Level-1 headlines ARE siblings of each other.
        assert_eq!(next_sibling(&l, 0, 1, n), Some(2));
        assert_eq!(prev_sibling(&l, 2, 1), Some(0));
    }

    #[test]
    fn toggle_heading_round_trips() {
        assert_eq!(
            toggle_heading("body text", 1).as_deref(),
            Some("* body text")
        );
        assert_eq!(
            toggle_heading("* body text", 1).as_deref(),
            Some("body text")
        );
        // The level comes from the caller (the enclosing headline's), so a body
        // line under `** Two` becomes a level-3 headline, not a level-1 one.
        assert_eq!(toggle_heading("note", 3).as_deref(), Some("*** note"));
        // Stripping takes exactly one space — the marker's — and leaves any
        // deliberate indentation in the title alone.
        assert_eq!(
            toggle_heading("**   spaced", 1).as_deref(),
            Some("  spaced")
        );
        // A titleless headline degrades to an empty line, not to `" "`.
        assert_eq!(toggle_heading("** ", 1).as_deref(), Some(""));
        // Nothing to do on a blank line: no stars to strip, no text to promote.
        assert_eq!(toggle_heading("", 1), None);
        assert_eq!(toggle_heading("   ", 1), None);
    }

    /// Reading a whole 10k-line file to find a sibling would cost 10k boundary
    /// crossings per keypress. Both scans stop at the parent boundary.
    #[test]
    fn the_sibling_scan_stops_at_the_parent() {
        use std::cell::Cell;
        let mut lines = vec!["* Parent".to_string(), "** Child".to_string()];
        lines.extend((0..10_000).map(|i| format!("body {i}")));
        let reads = Cell::new(0u32);
        let accessor = |i: u32| {
            reads.set(reads.get() + 1);
            lines.get(i as usize).cloned()
        };
        assert_eq!(prev_sibling(accessor, 1, 2), None);
        assert!(
            reads.get() <= 2,
            "read {} lines to hit the parent one line up",
            reads.get()
        );
    }
}

// --- OT.4: the same questions, asked of the parse tree ---------------------
//
// The shape of org's grammar is what makes every one of these a few steps
// rather than a query (`grammar-src/grammar.js`):
//
//     document: optional(body), repeat(section)
//     section:  headline, optional(plan), optional(property_drawer),
//               optional(body), repeat(subsection: section)
//
// So a `section` node IS a subtree — headline plus everything beneath it,
// nested sections included — and the questions the line logic reconstructs by
// scanning become node navigation: "which subtree am I in" is `enclosing`,
// "where does it end" is that node's extent, "who is my parent" is the nearest
// `section` ancestor, and siblings are sibling nodes.
//
// A section's named children are at most `headline`, `plan`, `property_drawer`,
// `body` and its subsections — `body` collapses a whole run of paragraphs,
// lists and blocks into ONE node — so scanning a section's children is a
// handful of steps and not proportional to its text.

use crate::lattice::plugin_host::tree_sitter::{Node, TreeSnapshot};
use crate::tree;

const SECTION: &str = "section";

/// The `section` node enclosing `line`, if the cursor is inside one.
///
/// Column 0 is the right probe here and only here: a `section` starts at its
/// headline's first star, which is column 0 by definition. See
/// [`tree::enclosing`] for why that is not the default assumption.
fn enclosing_section(tree: &TreeSnapshot, line: u32) -> Option<Node> {
    tree::enclosing(tree, line, 0, SECTION)
}

/// A section's headline line and level, read from the grammar.
///
/// The level is the WIDTH of the `stars` node rather than a re-count of `*`
/// characters: the grammar already decided where the stars end, and counting
/// again is how the two drift apart.
fn headline_of(section: &Node) -> Option<(u32, usize)> {
    let headline = section.child_by_field("headline")?;
    let line = headline.byte_range().start.line;
    let stars = headline.child_by_field("stars")?.byte_range();
    let level = stars.end.byte.saturating_sub(stars.start.byte) as usize;
    (level > 0).then_some((line, level))
}

/// Just the line a section's headline sits on.
fn headline_line(section: &Node) -> Option<u32> {
    headline_of(section).map(|(line, _)| line)
}

/// The section's first nested subsection, or `None` for a leaf.
fn first_subsection(node: &Node) -> Option<Node> {
    tree::first_child_of_kind(node, SECTION)
}

/// The section's last nested subsection, or `None` for a leaf.
fn last_subsection(node: &Node) -> Option<Node> {
    tree::last_child_of_kind(node, SECTION)
}

/// The next sibling that is a section, skipping a parent's non-section children.
fn next_sibling_section(node: &Node) -> Option<Node> {
    tree::next_sibling_of_kind(node, SECTION)
}

/// [`next_sibling_section`] backwards.
fn prev_sibling_section(node: &Node) -> Option<Node> {
    tree::prev_sibling_of_kind(node, SECTION)
}

/// The nearest `section` ancestor. `None` at a top-level section, whose parent
/// is the `document`.
fn parent_section(node: &Node) -> Option<Node> {
    tree::ancestor(node, SECTION)
}

/// The last headline *inside* `section` in document order: its last subsection's
/// last subsection, all the way down. The pre-order predecessor of whatever
/// follows `section`.
fn deepest_last_subsection(section: Node) -> Node {
    let mut node = section;
    while let Some(deeper) = last_subsection(&node) {
        node = deeper;
    }
    node
}

/// [`enclosing_headline`], asked of the tree.
///
/// `None` when the cursor is in a file's preamble — and, unlike the text path,
/// also when it is inside a source block, which is the point.
fn enclosing_headline_tree(tree: &TreeSnapshot, from: u32) -> Option<(u32, usize)> {
    headline_of(&enclosing_section(tree, from)?)
}

/// [`subtree_end`], asked of the tree.
///
/// A `section` spans its headline plus everything beneath it *including nested
/// sections* — the definition `subtree_end` reconstructs by scanning for the
/// next headline of the same level or shallower. Here it is the node's extent.
fn subtree_end_tree(tree: &TreeSnapshot, from: u32) -> Option<u32> {
    Some(tree::last_content_line(
        &enclosing_section(tree, from)?.byte_range(),
    ))
}

/// [`next_headline`], asked of the tree: the pre-order successor section.
///
/// Nested first, then siblings, then the siblings of ancestors — which is
/// exactly what "the next headline at any level" means in document order, and
/// why `]]` walks into a subtree rather than over it.
fn next_headline_tree(tree: &TreeSnapshot, from: u32) -> Option<u32> {
    let Some(start) = enclosing_section(tree, from) else {
        // The preamble before the first headline: the successor is the
        // document's first section.
        return headline_line(&first_subsection(&tree.root())?);
    };
    if let Some(line) = first_subsection(&start).as_ref().and_then(headline_line) {
        if line > from {
            return Some(line);
        }
    }
    let mut cur = start;
    loop {
        if let Some(line) = next_sibling_section(&cur).as_ref().and_then(headline_line) {
            if line > from {
                return Some(line);
            }
        }
        cur = parent_section(&cur)?;
    }
}

/// [`prev_headline`], asked of the tree.
///
/// From body text the answer is the enclosing headline itself — the cursor is
/// already past it. From a headline line it is the pre-order predecessor: the
/// previous sibling's deepest last descendant, or failing that the parent.
fn prev_headline_tree(tree: &TreeSnapshot, from: u32) -> Option<u32> {
    let start = enclosing_section(tree, from)?;
    let head = headline_line(&start)?;
    if head < from {
        return Some(head);
    }
    if let Some(prev) = prev_sibling_section(&start) {
        return headline_line(&deepest_last_subsection(prev));
    }
    headline_line(&parent_section(&start)?)
}

/// [`parent_headline`], asked of the tree.
fn parent_headline_tree(tree: &TreeSnapshot, from: u32) -> Option<u32> {
    headline_line(&parent_section(&enclosing_section(tree, from)?)?)
}

/// [`prev_sibling`], asked of the tree — and note it needs no `level` argument.
///
/// The text version takes one because it has to reconstruct "same parent" from
/// star counts, stopping the scan at a shallower headline. A sibling node is
/// a sibling; there is nothing to reconstruct.
fn prev_sibling_tree(tree: &TreeSnapshot, from: u32) -> Option<u32> {
    headline_line(&prev_sibling_section(&enclosing_section(tree, from)?)?)
}

/// [`next_sibling`], asked of the tree. See [`prev_sibling_tree`].
fn next_sibling_tree(tree: &TreeSnapshot, from: u32) -> Option<u32> {
    headline_line(&next_sibling_section(&enclosing_section(tree, from)?)?)
}

/// One headline in a file's outline: where it starts, how deep it is, and where
/// its subtree ends.
///
/// Carries no title. The two consumers want different text from the same line —
/// refile keeps the TODO keyword and the tags because you type them into a
/// picker, capture strips both because a template names its target in config and
/// must not stop matching the day someone adds `:drill:`. So the entry is
/// structure and each caller reads `lines[entry.line]` for characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// 0-based line the headline is on.
    pub line: u32,
    /// Star count.
    pub level: usize,
    /// 0-based last line of the subtree rooted here, inclusive.
    pub end_line: u32,
}

/// Every headline in a parsed file, in document order (OT.8).
///
/// The off-buffer peer of [`Headlines`]: `tree-sitter.parse-file` hands back a
/// root for a file that is not an open buffer, and this walks it. Recursion is
/// required rather than tidy — a `section` holds its subsections as children, so
/// a flat pass over the root sees only top-level headlines and silently drops
/// every nested one.
pub fn outline(root: &Node, out: &mut Vec<Entry>) {
    for child in tree::children_of_kind(root, SECTION) {
        if let Some((line, level)) = headline_of(&child) {
            out.push(Entry {
                line,
                level,
                end_line: tree::last_content_line(&child.byte_range()),
            });
        }
        outline(&child, out);
    }
    // A file may open with content before its first headline, so top-level
    // sections are not always direct children of the root. Anything that is not
    // a section is descended into for that reason.
    for i in 0..root.named_child_count() {
        if let Some(child) = root.named_child(i) {
            if child.kind() != SECTION {
                outline(&child, out);
            }
        }
    }
}

/// [`outline`] over text, for a file with no grammar behind it.
///
/// The same shape, so both consumers have one body and only their structure
/// source differs — which is what stops refile's insertion point and capture's
/// from drifting apart (`org-capture.md` §4 makes the point about
/// `subtree_end`; it is more true, not less, once one of them is a tree walk).
pub fn outline_text(lines: &[&str]) -> Vec<Entry> {
    let count = lines.len() as u32;
    let line = |n: u32| lines.get(n as usize).map(|s| s.to_string());
    (0..count)
        .filter_map(|n| {
            let level = lines.get(n as usize).and_then(|l| headline_level(l))?;
            Some(Entry {
                line: n,
                level,
                end_line: subtree_end(line, n, count),
            })
        })
        .collect()
}

/// Every headline question a caller can ask, resolved from the tree when there
/// is one and from the line logic when there is not.
///
/// Callers hold one of these instead of a bare line accessor, so the
/// tree-or-text decision is made in ONE place. The three-way `match tree
/// { Some => …, None => … }` was written out at three call sites before this
/// existed, and a fourth site that forgot it would silently be the only one
/// still hand-parsing — which is the divergence OT.x is about, reproduced
/// inside the plugin.
pub struct Headlines<'a> {
    tree: Option<&'a TreeSnapshot>,
    line: &'a dyn Fn(u32) -> Option<String>,
    line_count: u32,
}

impl<'a> Headlines<'a> {
    pub fn new(
        tree: Option<&'a TreeSnapshot>,
        line: &'a dyn Fn(u32) -> Option<String>,
        line_count: u32,
    ) -> Self {
        Self {
            tree,
            line,
            line_count,
        }
    }

    /// Read one line through the accessor the caller supplied.
    pub fn text(&self, n: u32) -> Option<String> {
        (self.line)(n)
    }

    /// Whether `n` is a headline line — the tree's answer being "a section
    /// starts here", which a `* TODO` line inside a source block does not.
    pub fn is_headline(&self, n: u32) -> bool {
        match self.tree {
            Some(tree) => enclosing_headline_tree(tree, n).is_some_and(|(line, _)| line == n),
            None => self.text(n).as_deref().and_then(headline_level).is_some(),
        }
    }

    /// See [`enclosing_headline`].
    pub fn enclosing(&self, from: u32) -> Option<(u32, usize)> {
        match self.tree {
            Some(tree) => enclosing_headline_tree(tree, from),
            None => enclosing_headline(self.line, from),
        }
    }

    /// See [`subtree_end`]. `start` is the subtree's headline line.
    pub fn subtree_end(&self, start: u32) -> u32 {
        self.tree
            .and_then(|tree| subtree_end_tree(tree, start))
            .unwrap_or_else(|| subtree_end(self.line, start, self.line_count))
    }

    /// See [`next_headline`].
    pub fn next(&self, from: u32) -> Option<u32> {
        match self.tree {
            Some(tree) => next_headline_tree(tree, from),
            None => next_headline(self.line, from, self.line_count),
        }
    }

    /// See [`prev_headline`].
    pub fn prev(&self, from: u32) -> Option<u32> {
        match self.tree {
            Some(tree) => prev_headline_tree(tree, from),
            None => prev_headline(self.line, from),
        }
    }

    /// See [`parent_headline`].
    pub fn parent(&self, from: u32) -> Option<u32> {
        match self.tree {
            Some(tree) => parent_headline_tree(tree, from),
            None => parent_headline(self.line, from),
        }
    }

    /// See [`prev_sibling`]. `start` is the subtree's headline line and `level`
    /// its star count — both only used by the text fallback.
    pub fn prev_sibling(&self, start: u32, level: usize) -> Option<u32> {
        match self.tree {
            Some(tree) => prev_sibling_tree(tree, start),
            None => prev_sibling(self.line, start, level),
        }
    }

    /// See [`next_sibling`].
    pub fn next_sibling(&self, start: u32, level: usize) -> Option<u32> {
        match self.tree {
            Some(tree) => next_sibling_tree(tree, start),
            None => next_sibling(self.line, start, level, self.line_count),
        }
    }
}

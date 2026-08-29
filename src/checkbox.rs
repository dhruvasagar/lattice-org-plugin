//! OM.8 — checkbox items and their statistics cookies, as pure line logic.
//!
//! ```org
//! * Shopping [1/3]
//!   - [X] bread
//!   - [ ] milk
//!   - [ ] eggs
//! ```
//!
//! Toggling `milk` must both flip its box AND recompute `[1/3]` to `[2/3]`,
//! in ONE edit, so a single `u` puts both back. That coupling is the whole
//! reason this is a module rather than a two-line string replace.

/// A checkbox's three states. `Partial` is org's `[-]`: some children ticked,
/// not all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    Partial,
}

impl Check {
    fn as_char(self) -> char {
        match self {
            Check::Off => ' ',
            Check::On => 'X',
            Check::Partial => '-',
        }
    }
}

/// A parsed checkbox list item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Columns of leading whitespace — the nesting level.
    pub indent: usize,
    /// Byte offset of the `[` in the line.
    pub box_at: usize,
    pub state: Check,
}

/// Parse `line` as a checkbox list item.
///
/// A bullet (`-`, `+`) or an ordered marker (`1.`, `1)`) followed by
/// `[ ]` / `[X]` / `[x]` / `[-]`. Note `*` is NOT accepted as a bullet at
/// column 0 — that is a headline, and treating `* [ ] x` as a checkbox would
/// make `<C-Space>` silently rewrite a heading.
pub fn parse_item(line: &str) -> Option<Item> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let after_bullet = strip_bullet(rest, indent)?;
    let box_off = rest.len() - after_bullet.len();
    let b = after_bullet.as_bytes();
    if b.first() != Some(&b'[') || b.get(2) != Some(&b']') {
        return None;
    }
    let state = match b.get(1)? {
        b' ' => Check::Off,
        b'X' | b'x' => Check::On,
        b'-' => Check::Partial,
        _ => return None,
    };
    Some(Item {
        indent,
        box_at: indent + box_off,
        state,
    })
}

/// Strip a list bullet, returning what follows it (whitespace trimmed).
fn strip_bullet(rest: &str, indent: usize) -> Option<&str> {
    // `*` is a bullet ONLY when indented — at column 0 it is a headline.
    for (marker, needs_indent) in [("- ", false), ("+ ", false), ("* ", true)] {
        if let Some(r) = rest.strip_prefix(marker) {
            if needs_indent && indent == 0 {
                return None;
            }
            return Some(r.trim_start());
        }
    }
    // Ordered: digits then `.` or `)` then a space.
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let after = &rest[digits..];
        for marker in [". ", ") "] {
            if let Some(r) = after.strip_prefix(marker) {
                return Some(r.trim_start());
            }
        }
    }
    None
}

/// Rewrite `line`'s checkbox to `state`. `None` if it is not an item.
pub fn set_state(line: &str, state: Check) -> Option<String> {
    let item = parse_item(line)?;
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..item.box_at + 1]);
    out.push(state.as_char());
    out.push_str(&line[item.box_at + 2..]);
    Some(out)
}

/// The state a toggle moves to. `Partial` counts as unticked, so pressing on
/// a half-done parent completes it — which is what a user means by toggling a
/// `[-]`.
pub fn toggled(state: Check) -> Check {
    match state {
        Check::On => Check::Off,
        Check::Off | Check::Partial => Check::On,
    }
}

/// A statistics cookie found on a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cookie {
    /// Byte range of the cookie text, e.g. the `[1/3]`.
    pub start: usize,
    pub end: usize,
    pub percent: bool,
}

/// Find a `[n/m]` or `[p%]` cookie on `line`.
pub fn find_cookie(line: &str) -> Option<Cookie> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'[' {
            i += 1;
            continue;
        }
        let Some(close) = line[i..].find(']').map(|o| i + o) else {
            break;
        };
        let inner = &line[i + 1..close];
        let is_percent = inner.ends_with('%')
            && inner[..inner.len() - 1].chars().all(|c| c.is_ascii_digit())
            && inner.len() > 1;
        let is_ratio = inner.split_once('/').is_some_and(|(a, c)| {
            !a.is_empty()
                && a.chars().all(|ch| ch.is_ascii_digit())
                && c.chars().all(|ch| ch.is_ascii_digit())
        });
        if is_percent || is_ratio {
            return Some(Cookie {
                start: i,
                end: close + 1,
                percent: is_percent,
            });
        }
        i = close + 1;
    }
    None
}

/// Rewrite `line`'s cookie for `done` of `total`. `None` if it has none.
///
/// Keeps the cookie's existing FORM: a `[n/m]` stays a ratio and a `[p%]`
/// stays a percentage. Rewriting one into the other would silently change a
/// document's style on a keypress.
pub fn update_cookie(line: &str, done: usize, total: usize) -> Option<String> {
    let c = find_cookie(line)?;
    let text = if c.percent {
        // Integer percent, truncated — org's own behaviour. 2/3 shows 66%,
        // not 67%, so a cookie only reads 100% when everything is done.
        let pct = if total == 0 { 0 } else { done * 100 / total };
        format!("[{pct}%]")
    } else {
        format!("[{done}/{total}]")
    };
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..c.start]);
    out.push_str(&text);
    out.push_str(&line[c.end..]);
    Some(out)
}

// --- OT.6: the list's shape comes from the tree ---------------------------
//
// `grammar.js` gives a checkbox list real structure:
//
//     list:     listitem+
//     listitem: bullet: bullet, checkbox: checkbox?, contents: (paragraph | list)*
//     checkbox: '[', status: expr?, ']'
//
// A nested list is a `contents` child of the item it belongs to, so "the direct
// children of this item" is a node walk. The line logic above reconstructs the
// same fact from leading whitespace, which works for well-formed org and cannot
// see the one thing that decides it: a `- [ ] example` line between
// `#+BEGIN_SRC` and `#+END_SRC` is block text, and indentation does not say so.
// The text path counts it into the parent's cookie and lets `<C-Space>` rewrite
// it; the tree has no `listitem` there at all.
//
// Cookies stay text. `* Shopping [1/3]` parses as `item: (item (expr) (expr))` —
// the grammar does not model a statistics cookie, so `find_cookie` /
// `update_cookie` have no node to migrate to, exactly as OT.5 found for stamps.

use crate::lattice::plugin_host::tree_sitter::{Node, TreeSnapshot};
use crate::tree;

const LIST: &str = "list";
const LISTITEM: &str = "listitem";

/// Something whose statistics cookie a toggle may have to update: a checkbox
/// item that owns a nested list, or the headline that owns a top-level one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parent {
    /// A list item, with the indent its text carries (read by the fallback).
    Item { line: u32, indent: usize },
    /// A headline. It owns every list beneath it whatever their indent.
    Headline { line: u32 },
}

impl Parent {
    pub fn line(self) -> u32 {
        match self {
            Parent::Item { line, .. } | Parent::Headline { line } => line,
        }
    }
}

/// The list structure a toggle needs, from the tree when there is one and from
/// indentation when there is not.
///
/// Both halves answer only STRUCTURE — which lines are items, which items are
/// whose children. The state in each box is read from the caller's text, and
/// that split is load-bearing rather than stylistic: the cookie must be
/// recomputed against the buffer as it will be AFTER the toggle, and the tree
/// describes the buffer as it is BEFORE it. Asking the tree for a tick would
/// leave every cookie one keypress behind.
pub struct Checkboxes<'a> {
    tree: Option<&'a TreeSnapshot>,
    line: &'a dyn Fn(u32) -> Option<String>,
    line_count: u32,
}

impl<'a> Checkboxes<'a> {
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

    pub fn text(&self, n: u32) -> Option<String> {
        (self.line)(n)
    }

    /// The checkbox item whose box sits on line `n`.
    ///
    /// The box, not the item: a `listitem` spans its continuation lines too,
    /// and toggling from the middle of a wrapped item is not what `<C-Space>`
    /// means. `box_at` still comes from the text, because the caller rewrites
    /// the line and needs a byte offset into it.
    pub fn item_at(&self, n: u32) -> Option<Item> {
        let text = self.text(n)?;
        let item = parse_item(&text)?;
        match self.tree {
            // The tree is the veto: it says whether this line is a list item at
            // all. What the box CONTAINS is still read from the text.
            Some(snapshot) => {
                let node = enclosing_listitem(snapshot, n, item.box_at as u32)?;
                (checkbox_start_line(&node) == Some(n)).then_some(item)
            }
            None => Some(item),
        }
    }

    /// The cookie-bearing ancestors of the item on line `n`, innermost first,
    /// ending at the enclosing headline.
    pub fn ancestors(&self, n: u32) -> Vec<Parent> {
        match self.tree {
            Some(snapshot) => self.tree_ancestors(snapshot, n),
            None => self.text_ancestors(n),
        }
    }

    /// The lines carrying `parent`'s DIRECT child checkboxes.
    ///
    /// Direct only — a grandchild's state is already reflected in its own
    /// parent's box, so counting it again double-counts a deep list.
    pub fn child_item_lines(&self, parent: Parent) -> Vec<u32> {
        match self.tree {
            Some(snapshot) => self.tree_children(snapshot, parent),
            None => self.text_children(parent),
        }
    }

    // ── the tree ──

    fn tree_ancestors(&self, snapshot: &TreeSnapshot, n: u32) -> Vec<Parent> {
        let mut out = Vec::new();
        let Some(item) = self.listitem_on(snapshot, n) else {
            return out;
        };
        let mut node = item;
        while let Some(ancestor) = tree::ancestor(&node, LISTITEM) {
            if let Some(line) = checkbox_start_line(&ancestor) {
                out.push(Parent::Item {
                    line,
                    indent: self.indent_of(line),
                });
            }
            node = ancestor;
        }
        // The section that owns the outermost list. A list in a file's
        // preamble has none, and then there is no cookie above it.
        if let Some(headline) =
            tree::ancestor(&node, "section").and_then(|section| section.child_by_field("headline"))
        {
            out.push(Parent::Headline {
                line: headline.byte_range().start.line,
            });
        }
        out
    }

    fn tree_children(&self, snapshot: &TreeSnapshot, parent: Parent) -> Vec<u32> {
        let node = match parent {
            // The lists directly under the headline's section — every one of
            // them, not only those at the first indent the text path locks on
            // to. A section holding a `-` list and a `1.` list holds two `list`
            // nodes and org counts both.
            // A headline starts at column 0, so this needs no indent probe.
            Parent::Headline { line } => {
                let Some(section) = tree::enclosing(snapshot, line, 0, "section") else {
                    return Vec::new();
                };
                match section.child_by_field("body") {
                    Some(body) => body,
                    None => return Vec::new(),
                }
            }
            Parent::Item { line, .. } => match self.listitem_on(snapshot, line) {
                Some(item) => item,
                None => return Vec::new(),
            },
        };
        let mut out = Vec::new();
        for list in tree::children_of_kind(&node, LIST) {
            for item in tree::children_of_kind(&list, LISTITEM) {
                if let Some(line) = checkbox_start_line(&item) {
                    out.push(line);
                }
            }
        }
        out
    }

    // ── the fallback ──

    /// The `listitem` whose checkbox is on line `n`.
    ///
    /// Probes at the box's own column, NOT at byte 0. An indented item's
    /// `listitem` node starts at its bullet, so a point at column 0 is in the
    /// enclosing `list` (or `body`) and the ancestor walk from there never
    /// finds a `listitem` at all — which showed up as `<C-Space>` silently
    /// doing nothing on every indented list in the suite.
    fn listitem_on(&self, snapshot: &TreeSnapshot, n: u32) -> Option<Node> {
        let col = self.text(n).as_deref().and_then(parse_item)?.box_at as u32;
        let node = enclosing_listitem(snapshot, n, col)?;
        (checkbox_start_line(&node) == Some(n)).then_some(node)
    }

    fn indent_of(&self, n: u32) -> usize {
        self.text(n)
            .map(|t| t.len() - t.trim_start().len())
            .unwrap_or(0)
    }

    fn text_ancestors(&self, n: u32) -> Vec<Parent> {
        let mut out = Vec::new();
        let Some(mut current_indent) = self
            .text(n)
            .as_deref()
            .and_then(parse_item)
            .map(|i| i.indent)
        else {
            return out;
        };
        let mut scan = n;
        while scan > 0 {
            scan -= 1;
            let Some(above) = self.text(scan) else { break };
            if above.trim().is_empty() {
                continue;
            }
            let indent = above.len() - above.trim_start().len();
            // A headline owns the whole list beneath it whatever its indent,
            // and is the top: nothing above it carries this list's cookie.
            if crate::headline::headline_level(&above).is_some() {
                out.push(Parent::Headline { line: scan });
                break;
            }
            if indent >= current_indent {
                continue;
            }
            out.push(Parent::Item { line: scan, indent });
            current_indent = indent;
        }
        out
    }

    /// The fallback's children, by indentation.
    ///
    /// **A headline has no indent requirement.** It owns every list beneath it
    /// until the next headline, whatever column they start at. Requiring
    /// `indent > 0` here — which the original did, because a headline's indent
    /// reads as 0 — meant a list written flush at column 0 under a headline had
    /// no children at all and its cookie never moved. That is ordinary org, and
    /// it went unnoticed only because every test in the suite happened to indent
    /// its lists. It surfaced when the OT.7 staleness gate started routing the
    /// keystroke straight after an edit through this path.
    ///
    /// An ITEM parent does require a deeper indent: that is what distinguishes
    /// its own children from its siblings.
    fn text_children(&self, parent: Parent) -> Vec<u32> {
        let mut out = Vec::new();
        let mut child_indent: Option<usize> = None;
        for i in (parent.line() + 1)..self.line_count {
            let Some(text) = self.text(i) else { break };
            if text.trim().is_empty() {
                continue;
            }
            // The next headline ends the region whatever its indent, or a
            // cookie would count the following section's items.
            if crate::headline::headline_level(&text).is_some() {
                break;
            }
            let indent = text.len() - text.trim_start().len();
            if let Parent::Item {
                indent: parent_indent,
                ..
            } = parent
            {
                if indent <= parent_indent {
                    break;
                }
            }
            let Some(item) = parse_item(&text) else {
                continue;
            };
            // Lock on to the FIRST child level seen; deeper items belong to a
            // nested list and are counted by their own parent.
            let level = *child_indent.get_or_insert(item.indent);
            if item.indent == level {
                out.push(i);
            }
        }
        out
    }
}

/// Count `lines` as ticked-of-total, reading each box's state from `state_of`.
///
/// Separate from [`Checkboxes`] because the caller supplies an accessor
/// overlaying the edit it is about to make — see the type's doc-comment.
pub fn tally_lines(lines: &[u32], state_of: impl Fn(u32) -> Option<String>) -> (usize, usize) {
    let mut done = 0;
    let mut total = 0;
    for &n in lines {
        let Some(item) = state_of(n).as_deref().and_then(parse_item) else {
            continue;
        };
        total += 1;
        if item.state == Check::On {
            done += 1;
        }
    }
    (done, total)
}

/// The innermost `listitem` covering `(line, byte)`.
fn enclosing_listitem(snapshot: &TreeSnapshot, line: u32, byte: u32) -> Option<Node> {
    tree::enclosing(snapshot, line, byte, LISTITEM)
}

/// The line a `listitem`'s checkbox sits on. `None` for an item with no box —
/// a plain bullet is not part of any tally.
fn checkbox_start_line(item: &Node) -> Option<u32> {
    Some(item.child_by_field("checkbox")?.byte_range().start.line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_bullet_forms_org_accepts() {
        for line in ["- [ ] a", "+ [X] a", "  * [-] a", "1. [ ] a", "12) [x] a"] {
            assert!(parse_item(line).is_some(), "{line:?}");
        }
    }

    /// `* [ ] x` at column 0 is a HEADLINE. Treating it as a checkbox would
    /// make `<C-Space>` silently rewrite a heading.
    #[test]
    fn a_star_at_column_zero_is_a_headline_not_a_bullet() {
        assert!(parse_item("* [ ] not a checkbox").is_none());
        assert!(parse_item("  * [ ] but indented is").is_some());
    }

    #[test]
    fn rejects_lines_that_merely_look_like_items() {
        for line in [
            "- no box",
            "[ ] no bullet",
            "- [] too short",
            "- [?] bad",
            "",
        ] {
            assert!(parse_item(line).is_none(), "{line:?}");
        }
    }

    #[test]
    fn toggling_writes_the_box_and_leaves_the_text_alone() {
        assert_eq!(
            set_state("  - [ ] milk", Check::On).unwrap(),
            "  - [X] milk"
        );
        assert_eq!(
            set_state("  - [X] milk", Check::Off).unwrap(),
            "  - [ ] milk"
        );
        assert_eq!(set_state("1. [ ] a", Check::Partial).unwrap(), "1. [-] a");
    }

    /// `[-]` counts as unticked, so pressing on a half-done parent completes
    /// it — which is what toggling a partial box means.
    #[test]
    fn partial_toggles_to_done() {
        assert_eq!(toggled(Check::Partial), Check::On);
        assert_eq!(toggled(Check::Off), Check::On);
        assert_eq!(toggled(Check::On), Check::Off);
    }

    #[test]
    fn finds_both_cookie_forms_and_ignores_other_brackets() {
        assert_eq!(find_cookie("* Shop [1/3]").map(|c| c.percent), Some(false));
        assert_eq!(find_cookie("* Shop [33%]").map(|c| c.percent), Some(true));
        // A checkbox is not a cookie, and neither is a link.
        assert!(find_cookie("- [X] a").is_none());
        assert!(find_cookie("see [[file:a.png]]").is_none());
        assert!(find_cookie("no cookie here").is_none());
    }

    /// A `[n/m]` stays a ratio and a `[p%]` stays a percentage — rewriting one
    /// into the other would change a document's style on a keypress.
    #[test]
    fn updating_keeps_the_cookies_existing_form() {
        assert_eq!(update_cookie("* S [0/0]", 2, 3).unwrap(), "* S [2/3]");
        assert_eq!(update_cookie("* S [0%]", 2, 3).unwrap(), "* S [66%]");
    }

    /// Truncated, like org: 2/3 is 66%, so a cookie reads 100% only when
    /// everything is actually done.
    #[test]
    fn percentages_truncate_so_100_means_complete() {
        assert_eq!(update_cookie("[0%]", 2, 3).unwrap(), "[66%]");
        assert_eq!(update_cookie("[0%]", 3, 3).unwrap(), "[100%]");
        assert_eq!(
            update_cookie("[0%]", 0, 0).unwrap(),
            "[0%]",
            "no division by zero"
        );
    }

    fn buf(text: &str) -> (impl Fn(u32) -> Option<String> + use<>, u32) {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let n = lines.len() as u32;
        (move |i: u32| lines.get(i as usize).cloned(), n)
    }

    /// The tally, over the INDENT fallback — `Checkboxes::new(None, ..)`.
    ///
    /// The tree half cannot be unit-tested here (a `TreeSnapshot` is a host
    /// resource), so it is covered by dispatch through a real editor in
    /// `tests/org_structure.rs`. These keep the fallback honest, which matters
    /// because it is what runs when a buffer's parse is pending.
    fn tally(l: &dyn Fn(u32) -> Option<String>, parent: Parent, count: u32) -> (usize, usize) {
        let cb = Checkboxes::new(None, l, count);
        tally_lines(&cb.child_item_lines(parent), |i| cb.text(i))
    }

    fn headline(line: u32) -> Parent {
        Parent::Headline { line }
    }

    fn item(line: u32, indent: usize) -> Parent {
        Parent::Item { line, indent }
    }

    #[test]
    fn tallies_the_direct_children_of_a_headline() {
        let (l, n) = buf("* Shop [0/0]\n  - [X] bread\n  - [ ] milk\n  - [ ] eggs\n");
        assert_eq!(tally(&l, headline(0), n), (1, 3));
    }

    /// A grandchild's state is already reflected in its own parent's box, so
    /// counting it again would double-count deep lists.
    #[test]
    fn nested_items_are_counted_by_their_own_parent_only() {
        let (l, n) = buf("* Top [0/0]\n  - [-] a\n    - [X] a1\n    - [ ] a2\n  - [ ] b\n");
        assert_eq!(tally(&l, headline(0), n), (0, 2), "only `a` and `b`");
        // `a`'s own children are two, one done.
        assert_eq!(tally(&l, item(1, 2), n), (1, 2));
    }

    /// A following headline ends the region even when it is not indented
    /// less — otherwise a cookie would count the next section's items.
    #[test]
    fn a_following_headline_ends_the_tally() {
        let (l, n) = buf("* One [0/0]\n  - [X] a\n* Two\n  - [X] b\n");
        assert_eq!(tally(&l, headline(0), n), (1, 1));
    }

    #[test]
    fn blank_lines_do_not_end_a_list() {
        let (l, n) = buf("* One [0/0]\n  - [X] a\n\n  - [ ] b\n");
        assert_eq!(tally(&l, headline(0), n), (1, 2));
    }

    /// The headline case the indent rule used to lose: a list flush at column
    /// 0 is still that headline's list. It read `(0, 0)` until OT.7, and only
    /// because every other test in this file indents its lists.
    #[test]
    fn a_headline_owns_a_list_written_at_column_zero() {
        let (l, n) = buf("* Shop [0/0]\n- [X] bread\n- [ ] milk\n");
        assert_eq!(tally(&l, headline(0), n), (1, 2));
    }

    /// An ITEM parent still requires a deeper indent — that is what tells its
    /// children from its siblings.
    #[test]
    fn an_item_only_owns_what_is_indented_under_it() {
        let (l, n) = buf("* Top [0/0]\n- [ ] a [0/0]\n  - [X] a1\n- [ ] b\n");
        assert_eq!(tally(&l, item(1, 0), n), (1, 1), "just `a1`");
        assert_eq!(tally(&l, headline(0), n), (0, 2), "`a` and `b`");
    }

    /// The ancestor chain the fallback walks: innermost item first, then the
    /// headline that owns the whole list. The tree half answers the same
    /// question by walking `listitem` parents to the enclosing `section`.
    #[test]
    fn the_fallback_walks_out_to_the_headline() {
        let (l, n) = buf("* Top [0/0]\n  - [-] a [0/2]\n    - [X] a1\n    - [ ] a2\n");
        let cb = Checkboxes::new(None, &l, n);
        assert_eq!(cb.ancestors(2), vec![item(1, 2), headline(0)]);
        // From the outer item there is only the headline above it.
        assert_eq!(cb.ancestors(1), vec![headline(0)]);
    }
}

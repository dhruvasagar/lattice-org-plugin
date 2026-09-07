//! OS.3 — what a list item IS: bullet shape, nesting, extent, numbering.
//!
//! ```org
//! - one
//!   continued
//!   - one-a
//!   - one-b
//! - two
//! ```
//!
//! Every list verb OS.4–OS.10 adds asks the same handful of questions of that
//! shape — "which item am I in", "where does it end", "who are its siblings",
//! "what number should this be" — and each of them is a scan that can be
//! written two ways. `checkbox.rs` already had one such scan; a second verb
//! with its own copy is how the two drift, and the symptom of drift here is a
//! statistics cookie that quietly stops matching the boxes under it. So the
//! scan lives once, and `Checkboxes` is built on it rather than beside it.
//!
//! Structure comes from the tree when the buffer has one and from indentation
//! when it does not — the same split, for the same reason, as
//! [`crate::checkbox::Checkboxes`] and [`crate::headline::Headlines`]. The
//! fallback is what runs while a parse is pending, so it is not a degraded
//! path; it is the one that answers on the keystroke after an edit.

use crate::checkbox::Check;
use crate::lattice::plugin_host::tree_sitter::{Node, TreeSnapshot};
use crate::tree;

const LIST: &str = "list";
const LISTITEM: &str = "listitem";

/// How an ordered item punctuates its number: `1.` or `1)`.
///
/// Org accepts both, so a renumber must give back the one the document already
/// uses — rewriting `1)` as `1.` would restyle a file on a keypress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delim {
    Dot,
    Paren,
}

impl Delim {
    fn as_char(self) -> char {
        match self {
            Delim::Dot => '.',
            Delim::Paren => ')',
        }
    }
}

/// The marker that opens a list item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bullet {
    Dash,
    Plus,
    Star,
    Ordered { n: u32, delim: Delim },
}

impl Bullet {
    /// The literal text, e.g. `-`, `+`, `3.`, `3)`.
    pub fn render(self) -> String {
        match self {
            Bullet::Dash => "-".to_string(),
            Bullet::Plus => "+".to_string(),
            Bullet::Star => "*".to_string(),
            Bullet::Ordered { n, delim } => format!("{n}{}", delim.as_char()),
        }
    }

    /// The next shape in the cycle:
    ///     Dash -> Plus -> Ordered{Dot} -> Ordered{Paren} -> Dash
    /// `n` restarts at 1 on entry to an ordered form.
    ///
    /// `Star` is DELIBERATELY not in the cycle, though it is a legal bullet. It
    /// is legal only when indented — at column 0 it is a headline — so cycling
    /// a top-level list into it would turn every item into a heading. It is
    /// parsed because org files contain it; it is not somewhere the cycle will
    /// take you. A `Star` item that is cycled therefore ENTERS the cycle at
    /// `Dash` and never comes back.
    // Unused until OS.7 binds bullet cycling. The model is this slice's whole
    // deliverable and its shape is fixed by the plan, so the alternative to an
    // allow is shipping the gate without the thing it gates.
    // Unused until OS.9 binds bullet cycling.
    #[allow(dead_code)]
    pub fn cycled(self) -> Bullet {
        match self {
            Bullet::Dash => Bullet::Plus,
            Bullet::Plus => Bullet::Ordered {
                n: 1,
                delim: Delim::Dot,
            },
            Bullet::Ordered {
                delim: Delim::Dot, ..
            } => Bullet::Ordered {
                n: 1,
                delim: Delim::Paren,
            },
            Bullet::Ordered {
                delim: Delim::Paren,
                ..
            } => Bullet::Dash,
            Bullet::Star => Bullet::Dash,
        }
    }
}

/// One list item, as the line it starts on describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub line: u32,
    /// Columns of leading whitespace — the nesting level.
    pub indent: usize,
    pub bullet: Bullet,
    pub checkbox: Option<Check>,
    /// Byte offset where the item's own text begins, after the bullet and any
    /// checkbox.
    // Read by OS.4's meta-return, which splits an item at its content.
    pub content_byte: u32,
}

/// [`parse_line`]'s full answer: the item, plus the byte offsets a caller that
/// REWRITES the line needs.
///
/// Crate-internal because those offsets are a rewriting concern and not part of
/// the model — but they come from this walker rather than a second one, which
/// is the whole point. `checkbox.rs` re-deriving "where is the `[`" was one of
/// the two scans this module exists to collapse.
pub(crate) struct Parsed {
    pub item: Item,
    /// Bytes the marker itself occupies — `-` is 1, `12)` is 3 — NOT counting
    /// the space after it, so a rewrite can swap the marker and leave whatever
    /// spacing the line already had.
    pub marker_len: usize,
    /// Byte offset of the checkbox's `[`, when the item has one.
    pub box_byte: Option<usize>,
}

/// The bullet marker at the start of `rest`, and the bytes it occupies.
///
/// A marker must be followed by a space, exactly as `checkbox.rs`'s
/// `strip_bullet` required before this absorbed it: `-foo` is not an item and
/// `1.5` is not item 1.
fn marker(rest: &str, indent: usize) -> Option<(Bullet, usize)> {
    // `*` is a bullet ONLY when indented — at column 0 it is a headline, and
    // treating it as a bullet is how a list verb would silently eat an outline.
    for (ch, bullet, needs_indent) in [
        ('-', Bullet::Dash, false),
        ('+', Bullet::Plus, false),
        ('*', Bullet::Star, true),
    ] {
        if rest.starts_with(ch) {
            if needs_indent && indent == 0 {
                return None;
            }
            return rest[1..].starts_with(' ').then_some((bullet, 1));
        }
    }
    // Ordered: digits then `.` or `)` then a space.
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let after = &rest[digits..];
        for (ch, delim) in [('.', Delim::Dot), (')', Delim::Paren)] {
            if after.starts_with(ch) && after[1..].starts_with(' ') {
                let n = rest[..digits].parse().ok()?;
                return Some((Bullet::Ordered { n, delim }, digits + 1));
            }
        }
    }
    None
}

/// `[ ]` / `[X]` / `[x]` / `[-]` at the start of `body`, and the bytes it takes.
fn parse_checkbox(body: &str) -> Option<(Check, usize)> {
    let b = body.as_bytes();
    if b.first() != Some(&b'[') || b.get(2) != Some(&b']') {
        return None;
    }
    let state = match b.get(1)? {
        b' ' => Check::Off,
        b'X' | b'x' => Check::On,
        b'-' => Check::Partial,
        _ => return None,
    };
    Some((state, 3))
}

/// Parse one line as a list item, given the indent already measured.
///
/// The single walker. [`parse_bullet`] is its public projection and
/// `checkbox.rs` reads its offsets; neither re-scans the line.
pub(crate) fn parse_line(line: &str, indent: usize) -> Option<Parsed> {
    let rest = line.get(indent..)?;
    let (bullet, marker_len) = marker(rest, indent)?;
    let after_marker = rest.get(marker_len..)?;
    let body = after_marker.trim_start();
    let body_byte = indent + marker_len + (after_marker.len() - body.len());
    let (checkbox, box_byte, content_byte) = match parse_checkbox(body) {
        Some((state, used)) => {
            let after_box = body.get(used..).unwrap_or("");
            let gap = after_box.len() - after_box.trim_start().len();
            (Some(state), Some(body_byte), body_byte + used + gap)
        }
        None => (None, None, body_byte),
    };
    Some(Parsed {
        item: Item {
            // This sees one line's TEXT and not its number; `Lists` fills it in.
            line: 0,
            indent,
            bullet,
            checkbox,
            content_byte: content_byte as u32,
        },
        marker_len,
        box_byte,
    })
}

/// Parse one line as a list item, given the indent already measured.
///
/// `None` when the line is not an item — including `* Heading` at column 0,
/// which is a headline and not a `Star` bullet.
///
/// [`Item::line`] comes back **0**, because this is handed text and not a line
/// number. Read it only from an [`Item`] that [`Lists`] produced.
pub fn parse_bullet(line: &str, indent: usize) -> Option<Item> {
    parse_line(line, indent).map(|p| p.item)
}

/// The public peer of [`indent_of`], for callers outside this module that need
/// to read an indent off a line this module produced.
pub fn indent_of_public(text: &str) -> usize {
    indent_of(text)
}

fn indent_of(text: &str) -> usize {
    text.len() - text.trim_start().len()
}

/// Every list question a caller can ask, resolved from the tree when there is
/// one and from indentation when there is not.
///
/// The same shape as [`crate::checkbox::Checkboxes`] and
/// [`crate::headline::Headlines`], deliberately: callers hold one of these
/// instead of a bare line accessor, so the tree-or-text decision is made in ONE
/// place and a new verb cannot be the only site still hand-scanning.
pub struct Lists<'a> {
    tree: Option<&'a TreeSnapshot>,
    line: &'a dyn Fn(u32) -> Option<String>,
    line_count: u32,
}

impl<'a> Lists<'a> {
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

    /// The item whose BULLET is on line `n`. `None` on a continuation line, so
    /// acting from mid-item is a deliberate decision a caller makes with
    /// [`Lists::enclosing_item`], not an accident.
    pub fn item_at(&self, n: u32) -> Option<Item> {
        let text = self.text(n)?;
        let mut item = parse_bullet(&text, indent_of(&text))?;
        item.line = n;
        match self.tree {
            // The tree is the veto: it says whether this line opens a list item
            // at all, which a `- x` between `#+BEGIN_SRC` and `#+END_SRC` does
            // not. What the item CONTAINS is still read from the text — see
            // `Checkboxes`'s type doc for why state must not come from the tree.
            Some(snapshot) => {
                let indent = item.indent;
                self.listitem_at(snapshot, n, indent).map(|_| item)
            }
            None => Some(item),
        }
    }

    /// The item line `n` belongs to, walking back over continuation lines. The
    /// "which item am I in" answer.
    pub fn enclosing_item(&self, n: u32) -> Option<Item> {
        if let Some(item) = self.item_at(n) {
            return Some(item);
        }
        match self.tree {
            Some(snapshot) => {
                let text = self.text(n)?;
                let node = tree::enclosing(snapshot, n, indent_of(&text) as u32, LISTITEM)?;
                self.item_at(node.byte_range().start.line)
            }
            None => self.text_enclosing_item(n),
        }
    }

    /// Last line of the item at `start`, INCLUDING its continuation lines and
    /// its nested children. The unit a move or an indent carries.
    pub fn item_end(&self, start: u32) -> u32 {
        let Some(item) = self.item_at(start) else {
            return start;
        };
        if let Some(snapshot) = self.tree {
            if let Some(node) = self.listitem_at(snapshot, start, item.indent) {
                return tree::last_content_line(&node.byte_range());
            }
        }
        self.text_item_end(&item)
    }

    /// Item lines at the same indent, in order, within one list.
    // Unused until OS.6 moves an item among its siblings; see `Bullet::cycled`.
    // Unused until OS.6/OS.7 bind indent and move.
    #[allow(dead_code)]
    pub fn siblings(&self, n: u32) -> Vec<u32> {
        let Some(item) = self.enclosing_item(n) else {
            return Vec::new();
        };
        if let Some(snapshot) = self.tree {
            if let Some(lines) = self.tree_siblings(snapshot, &item) {
                return lines;
            }
        }
        self.text_siblings(&item)
    }

    /// Item lines one level deeper, directly under the item at `n`.
    ///
    /// Direct only: a grandchild belongs to its own parent, and counting it
    /// here would make every deep list double-counted.
    // Unused until OS.4 indents a subtree; see `Bullet::cycled`.
    // Unused until OS.6/OS.7 bind indent and move.
    #[allow(dead_code)]
    pub fn children(&self, n: u32) -> Vec<u32> {
        let Some(item) = self.enclosing_item(n) else {
            return Vec::new();
        };
        if let Some(snapshot) = self.tree {
            if let Some(lines) = self.tree_children(snapshot, &item) {
                return lines;
            }
        }
        self.text_children(&item)
    }

    /// First and last line of the whole list containing `n`.
    // Unused until OS.6 renumbers a list after a move.
    pub fn list_span(&self, n: u32) -> Option<(u32, u32)> {
        let item = self.enclosing_item(n)?;
        if let Some(snapshot) = self.tree {
            if let Some(span) = self.tree_list_span(snapshot, &item) {
                return Some(span);
            }
        }
        self.text_list_span(&item)
    }

    // ── the tree ──

    /// The `listitem` node whose bullet is on line `n`.
    ///
    /// Probes at the BULLET's own column, not byte 0. An indented item's
    /// `listitem` starts at its bullet, so a point at column 0 sits in the
    /// enclosing `list` and the ancestor walk never finds a `listitem` at all —
    /// which showed up once as `<C-Space>` silently doing nothing on every
    /// indented list in the suite (OT.6).
    fn listitem_at(&self, snapshot: &TreeSnapshot, n: u32, indent: usize) -> Option<Node> {
        let node = tree::enclosing(snapshot, n, indent as u32, LISTITEM)?;
        (node.byte_range().start.line == n).then_some(node)
    }

    /// The `listitem` node OPENING line `n`, for callers that must walk the
    /// tree from it rather than read the item's fields.
    ///
    /// This is [`item_at`](Self::item_at)'s own question, handed back as a node
    /// instead of an `Item`: same bullet-column probe, same "does an item start
    /// here" veto. It exists so `Checkboxes` can reach the ancestor walk without
    /// asking that question a second way — before OS.3's fix-round it asked at a
    /// different column with a different predicate, and the two could disagree.
    pub(crate) fn listitem_node_at(&self, snapshot: &TreeSnapshot, n: u32) -> Option<Node> {
        let text = self.text(n)?;
        let item = parse_bullet(&text, indent_of(&text))?;
        self.listitem_at(snapshot, n, item.indent)
    }

    // Dead only because its public caller is; see `Lists::siblings`.
    // Dead only because its public caller is; see `Lists::siblings`.
    #[allow(dead_code)]
    fn tree_siblings(&self, snapshot: &TreeSnapshot, item: &Item) -> Option<Vec<u32>> {
        let node = self.listitem_at(snapshot, item.line, item.indent)?;
        let list = tree::ancestor(&node, LIST)?;
        Some(
            tree::children_of_kind(&list, LISTITEM)
                .iter()
                .map(|i| i.byte_range().start.line)
                .collect(),
        )
    }

    // Dead only because its public caller is; see `Lists::children`.
    // Dead only because its public caller is; see `Lists::children`.
    #[allow(dead_code)]
    fn tree_children(&self, snapshot: &TreeSnapshot, item: &Item) -> Option<Vec<u32>> {
        let node = self.listitem_at(snapshot, item.line, item.indent)?;
        let mut out = Vec::new();
        for list in tree::children_of_kind(&node, LIST) {
            for child in tree::children_of_kind(&list, LISTITEM) {
                out.push(child.byte_range().start.line);
            }
        }
        Some(out)
    }

    /// The OUTERMOST `list` ancestor's extent — a nested list is a child of the
    /// item it hangs off, so the innermost one would bound the sublist and not
    /// the list the user means.
    fn tree_list_span(&self, snapshot: &TreeSnapshot, item: &Item) -> Option<(u32, u32)> {
        let node = self.listitem_at(snapshot, item.line, item.indent)?;
        let mut list = tree::ancestor(&node, LIST)?;
        while let Some(outer) = tree::ancestor(&list, LIST) {
            list = outer;
        }
        let range = list.byte_range();
        Some((range.start.line, tree::last_content_line(&range)))
    }

    // ── the fallback ──

    fn text_enclosing_item(&self, n: u32) -> Option<Item> {
        let text = self.text(n)?;
        if crate::headline::headline_level(&text).is_some() {
            return None;
        }
        let indent = indent_of(&text);
        let mut scan = n;
        while scan > 0 {
            scan -= 1;
            let above = self.text(scan)?;
            if above.trim().is_empty() {
                continue;
            }
            // A headline is the top of anything beneath it: whatever this line
            // is, it is not inside a list item that started above the heading.
            if crate::headline::headline_level(&above).is_some() {
                return None;
            }
            // Level with us or deeper is still this item's body — the owner is
            // the first thing STRICTLY shallower, which is what "continuation"
            // means in org.
            if indent_of(&above) >= indent {
                continue;
            }
            return self.item_at(scan);
        }
        None
    }

    fn text_item_end(&self, item: &Item) -> u32 {
        let mut end = item.line;
        for n in (item.line + 1)..self.line_count {
            let Some(text) = self.text(n) else { break };
            // A blank line does not end an item — org lists survive one — but
            // it is not the item's last line either, so `end` only follows
            // content.
            if text.trim().is_empty() {
                continue;
            }
            if crate::headline::headline_level(&text).is_some() {
                break;
            }
            if indent_of(&text) <= item.indent {
                break;
            }
            end = n;
        }
        end
    }

    fn text_siblings(&self, item: &Item) -> Vec<u32> {
        let mut before = Vec::new();
        let mut scan = item.line;
        while scan > 0 {
            scan -= 1;
            let Some(text) = self.text(scan) else { break };
            if text.trim().is_empty() {
                continue;
            }
            if crate::headline::headline_level(&text).is_some() {
                break;
            }
            // Shallower than us means we have left this sublist for its parent.
            if indent_of(&text) < item.indent {
                break;
            }
            if self.item_at(scan).is_some_and(|o| o.indent == item.indent) {
                before.push(scan);
            }
        }
        before.reverse();
        let mut out = before;
        out.push(item.line);
        for n in (item.line + 1)..self.line_count {
            let Some(text) = self.text(n) else { break };
            if text.trim().is_empty() {
                continue;
            }
            if crate::headline::headline_level(&text).is_some() {
                break;
            }
            if indent_of(&text) < item.indent {
                break;
            }
            if self.item_at(n).is_some_and(|o| o.indent == item.indent) {
                out.push(n);
            }
        }
        out
    }

    // Dead only because its public caller is; see `Lists::children`.
    // Dead only because its public caller is; see `Lists::children`.
    #[allow(dead_code)]
    fn text_children(&self, item: &Item) -> Vec<u32> {
        let mut out = Vec::new();
        let mut level: Option<usize> = None;
        for n in (item.line + 1)..=self.item_end(item.line) {
            let Some(child) = self.item_at(n) else {
                continue;
            };
            if child.indent <= item.indent {
                break;
            }
            // Lock on to the FIRST deeper level seen; anything below it belongs
            // to a nested list and is counted by its own parent.
            let want = *level.get_or_insert(child.indent);
            if child.indent == want {
                out.push(n);
            }
        }
        out
    }

    fn text_list_span(&self, item: &Item) -> Option<(u32, u32)> {
        let mut top = item.clone();
        while let Some(parent) = self.text_parent_item(&top) {
            top = parent;
        }
        let siblings = self.text_siblings(&top);
        let first = *siblings.first()?;
        let last = *siblings.last()?;
        Some((first, self.item_end(last)))
    }

    /// The item one nesting level out from `item`.
    fn text_parent_item(&self, item: &Item) -> Option<Item> {
        let mut scan = item.line;
        while scan > 0 {
            scan -= 1;
            let text = self.text(scan)?;
            if text.trim().is_empty() {
                continue;
            }
            if crate::headline::headline_level(&text).is_some() {
                return None;
            }
            if indent_of(&text) >= item.indent {
                continue;
            }
            return self.item_at(scan);
        }
        None
    }
}

/// Rewrite ordered-list numbering across `span`, per indent level, each level
/// restarting at 1. Answers only the lines that CHANGE, so a caller can fold
/// them into one edit — which is what lets a single `u` put a whole renumber
/// back. Unordered items are untouched.
// Unused until OS.6 renumbers after a move; see `Bullet::cycled`.
pub fn renumber(lists: &Lists<'_>, span: (u32, u32)) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    // One counter per open indent level. A Vec and not a map because leaving a
    // level must DROP its counter: two sublists at the same indent under
    // different parents each start at 1, and a map would run them together.
    let mut levels: Vec<(usize, u32)> = Vec::new();
    for n in span.0..=span.1 {
        let Some(item) = lists.item_at(n) else {
            continue;
        };
        while levels
            .last()
            .is_some_and(|(indent, _)| *indent > item.indent)
        {
            levels.pop();
        }
        let Bullet::Ordered { n: current, delim } = item.bullet else {
            continue;
        };
        if levels
            .last()
            .is_none_or(|(indent, _)| *indent != item.indent)
        {
            levels.push((item.indent, 1));
        }
        let Some((_, next)) = levels.last_mut() else {
            continue;
        };
        let want = *next;
        *next += 1;
        if want == current {
            continue;
        }
        if let Some(text) = renumbered(lists, &item, Bullet::Ordered { n: want, delim }) {
            out.push((n, text));
        }
    }
    out
}

/// `item`'s line with its marker replaced and everything else — indent,
/// spacing, checkbox, text — left exactly as it was written.
fn renumbered(lists: &Lists<'_>, item: &Item, bullet: Bullet) -> Option<String> {
    let text = lists.text(item.line)?;
    let parsed = parse_line(&text, item.indent)?;
    let marker_end = item.indent + parsed.marker_len;
    let mut out = String::with_capacity(text.len() + 2);
    out.push_str(text.get(..item.indent)?);
    out.push_str(&bullet.render());
    out.push_str(text.get(marker_end..)?);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_bullet_shape_org_accepts() {
        assert_eq!(parse_bullet("- milk", 0).unwrap().bullet, Bullet::Dash);
        assert_eq!(parse_bullet("+ milk", 0).unwrap().bullet, Bullet::Plus);
        assert_eq!(
            parse_bullet("1. milk", 0).unwrap().bullet,
            Bullet::Ordered {
                n: 1,
                delim: Delim::Dot
            }
        );
        assert_eq!(
            parse_bullet("12) milk", 0).unwrap().bullet,
            Bullet::Ordered {
                n: 12,
                delim: Delim::Paren
            }
        );
    }

    /// The existing rule, carried over from `checkbox.rs` verbatim: `*` is a
    /// bullet only when INDENTED. At column 0 it is a headline, and treating it
    /// as a bullet is how a list verb would silently eat an outline.
    #[test]
    fn a_star_at_column_zero_is_a_headline_not_a_bullet() {
        assert!(parse_bullet("* Heading", 0).is_none());
        assert_eq!(parse_bullet("  * nested", 2).unwrap().bullet, Bullet::Star);
    }

    #[test]
    fn an_item_carries_its_checkbox_when_it_has_one() {
        assert_eq!(
            parse_bullet("- [ ] milk", 0).unwrap().checkbox,
            Some(Check::Off)
        );
        assert_eq!(parse_bullet("- milk", 0).unwrap().checkbox, None);
    }

    /// A marker needs a space after it, which is what keeps `1.5` from being
    /// item 1 and `-foo` from being an item at all. Carried over with
    /// `strip_bullet`.
    #[test]
    fn rejects_markers_that_are_not_followed_by_a_space() {
        assert!(parse_bullet("-foo", 0).is_none());
        assert!(parse_bullet("1.5 litres", 0).is_none());
        assert!(parse_bullet("-", 0).is_none());
    }

    /// `content_byte` is where a split lands, so it must clear the checkbox and
    /// whatever spacing the line happens to use.
    #[test]
    fn content_byte_lands_after_the_bullet_and_the_box() {
        assert_eq!(parse_bullet("- milk", 0).unwrap().content_byte, 2);
        assert_eq!(parse_bullet("-   milk", 0).unwrap().content_byte, 4);
        assert_eq!(parse_bullet("- [ ] milk", 0).unwrap().content_byte, 6);
        assert_eq!(parse_bullet("  * [X] a", 2).unwrap().content_byte, 8);
    }

    /// `Star` is a legal bullet but not a destination: cycling a top-level list
    /// into it would turn every item into a heading.
    #[test]
    fn the_cycle_closes_and_never_lands_on_star() {
        let mut seen = Vec::new();
        let mut b = Bullet::Dash;
        for _ in 0..4 {
            b = b.cycled();
            seen.push(b);
        }
        assert_eq!(
            seen,
            vec![
                Bullet::Plus,
                Bullet::Ordered {
                    n: 1,
                    delim: Delim::Dot
                },
                Bullet::Ordered {
                    n: 1,
                    delim: Delim::Paren
                },
                Bullet::Dash,
            ],
            "four steps return to the start, and none of them is `*`"
        );
        assert_eq!(
            Bullet::Star.cycled(),
            Bullet::Dash,
            "a `*` item joins the cycle rather than staying outside it"
        );
    }

    /// `n` restarts at 1 on entry to an ordered form — the number a cycle lands
    /// on is a fresh list's, not the one the old marker happened to carry.
    #[test]
    fn entering_an_ordered_form_restarts_the_count() {
        assert_eq!(
            Bullet::Plus.cycled(),
            Bullet::Ordered {
                n: 1,
                delim: Delim::Dot
            }
        );
        assert_eq!(
            Bullet::Ordered {
                n: 9,
                delim: Delim::Dot
            }
            .cycled(),
            Bullet::Ordered {
                n: 1,
                delim: Delim::Paren
            }
        );
    }

    #[test]
    fn renders_each_shape_back_to_its_literal_text() {
        assert_eq!(Bullet::Dash.render(), "-");
        assert_eq!(Bullet::Plus.render(), "+");
        assert_eq!(Bullet::Star.render(), "*");
        assert_eq!(
            Bullet::Ordered {
                n: 3,
                delim: Delim::Dot
            }
            .render(),
            "3."
        );
        assert_eq!(
            Bullet::Ordered {
                n: 3,
                delim: Delim::Paren
            }
            .render(),
            "3)"
        );
    }

    // ── the structural half ──
    //
    // **What is covered here, and what is not.** These build a `Lists` with
    // `tree: None`, so they exercise the INDENT FALLBACK only. The tree half
    // cannot be reached from a unit test at all: a `TreeSnapshot` is a host WIT
    // resource with no guest-side constructor, and off `wasm32` wit-bindgen
    // compiles every one of its methods to `unreachable!()` — which is the only
    // reason this test binary links. `checkbox.rs` and `headline.rs` say the
    // same thing about their own tree halves, and no test in `tests/` names a
    // `TreeSnapshot` either. This is not a gap someone can close by trying
    // harder in this file; tree coverage comes only from dispatch through a
    // real editor.
    //
    // Where the tree half IS covered, after `Checkboxes` was rebuilt on this
    // module (OS.3 step 7):
    //
    //     item_at                       fallback here | tree via the existing
    //                                   checkbox integration tests, which now
    //                                   reach it through `Checkboxes::item_at`
    //     enclosing_item, item_end,     fallback here | NO tree coverage yet:
    //     siblings, children,           nothing calls them until OS.4 binds the
    //     list_span, renumber,          list verbs, so there is no chord to
    //     Bullet::cycled                press
    //
    // That second row is real debt and it is OS.4's and OS.6's to pay: each
    // must add at least one integration test that reaches these through a
    // PARSED buffer. Note this is narrower than the OS.3 plan predicted — the
    // plan expected `enclosing_item` and `children` to be covered too, but
    // `Checkboxes::ancestors` and `child_item_lines` deliberately did not move
    // onto `Lists` in this slice, so nothing routes to them yet.

    const NESTED: &str = "\
- one
  continued
  - one-a
  - one-b
- two
";

    /// Build a `Lists` over `text` and hand it to `check`.
    ///
    /// A callback rather than a return because `Lists` borrows its line
    /// accessor and cannot outlive it. Each test below names only a behaviour,
    /// so the SOURCE is this one function's business: swapping or adding one
    /// would not touch a single assertion.
    fn over(text: &str, check: impl FnOnce(&Lists<'_>)) {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let count = lines.len() as u32;
        let line = move |i: u32| lines.get(i as usize).cloned();
        check(&Lists::new(None, &line, count));
    }

    #[test]
    fn item_end_spans_continuation_lines_and_children() {
        over(NESTED, |lists| {
            assert_eq!(lists.item_end(0), 3, "`- one` runs through `- one-b`");
            assert_eq!(lists.item_end(2), 2, "`- one-a` is one line");
        });
    }

    #[test]
    fn siblings_skip_nested_children() {
        over(NESTED, |lists| {
            assert_eq!(lists.siblings(0), vec![0, 4]);
            assert_eq!(lists.children(0), vec![2, 3]);
        });
    }

    #[test]
    fn enclosing_item_walks_back_over_a_continuation_line() {
        over(NESTED, |lists| {
            assert!(
                lists.item_at(1).is_none(),
                "a continuation line is not an item"
            );
            assert_eq!(lists.enclosing_item(1).unwrap().line, 0);
        });
    }

    #[test]
    fn list_span_bounds_the_whole_list() {
        over(NESTED, |lists| {
            assert_eq!(lists.list_span(2), Some((0, 4)));
        });
    }

    /// A nested item's siblings are the ones beside it, not its parent's — the
    /// scan has to stop going out when the indent drops.
    #[test]
    fn siblings_of_a_nested_item_stay_inside_the_sublist() {
        over(NESTED, |lists| {
            assert_eq!(lists.siblings(2), vec![2, 3]);
            assert_eq!(lists.children(2), Vec::<u32>::new(), "a leaf has none");
        });
    }

    /// A headline bounds everything beneath it, or a list verb would reach into
    /// the next section.
    #[test]
    fn a_following_headline_ends_the_list() {
        over("- a\n- b\n* Next\n- c\n", |lists| {
            assert_eq!(lists.siblings(0), vec![0, 1]);
            assert_eq!(lists.item_end(1), 1);
        });
    }

    #[test]
    fn renumber_fixes_gaps_and_leaves_unordered_alone() {
        over("1. a\n5. b\n9. c\n", |lists| {
            assert_eq!(
                renumber(lists, (0, 2)),
                vec![(1, "2. b".to_string()), (2, "3. c".to_string())],
                "only the lines that change"
            );
        });
        over("- a\n- b\n", |plain| {
            assert!(renumber(plain, (0, 1)).is_empty());
        });
    }

    #[test]
    fn each_indent_level_restarts_at_one() {
        over("1. a\n   1. a-a\n   7. a-b\n2. b\n", |lists| {
            assert_eq!(renumber(lists, (0, 3)), vec![(2, "   2. a-b".to_string())]);
        });
    }

    #[test]
    fn renumber_preserves_the_delimiter_each_item_uses() {
        over("1) a\n4) b\n", |lists| {
            assert_eq!(renumber(lists, (0, 1)), vec![(1, "2) b".to_string())]);
        });
    }

    /// Two sublists at the same indent under different parents each start at 1.
    /// A counter kept per indent WITHOUT dropping it on the way out would run
    /// them together and number the second 3, 4.
    #[test]
    fn leaving_a_level_drops_its_counter() {
        over("1. a\n   5. x\n2. b\n   9. y\n", |lists| {
            assert_eq!(
                renumber(lists, (0, 3)),
                vec![(1, "   1. x".to_string()), (3, "   1. y".to_string())],
                "each sublist restarts"
            );
        });
    }

    /// A renumber rewrites the marker and nothing else — the checkbox and the
    /// spacing after it are the user's.
    #[test]
    fn renumber_leaves_a_checkbox_and_its_spacing_alone() {
        over("1. [X] a\n7. [ ] b\n", |lists| {
            assert_eq!(renumber(lists, (0, 1)), vec![(1, "2. [ ] b".to_string())]);
        });
    }

    // ── OS.6: shift_item ──────────────────────────────────────────────────

    #[test]
    fn indenting_an_item_carries_its_children() {
        over("- a\n- b\n  - b-a\n", |lists| {
            let out = shift_item(lists, 1, 1, true).unwrap();
            assert_eq!(
                out,
                vec![(1, "  - b".to_string()), (2, "    - b-a".to_string())]
            );
        });
    }

    /// Without `with_children` the nested item stays where it is — the same
    /// split `<leader>ol` / `<leader>oL` make for a headline and its subtree.
    #[test]
    fn indenting_without_children_leaves_them_behind() {
        over("- a\n- b\n  - b-a\n", |lists| {
            let out = shift_item(lists, 1, 1, false).unwrap();
            assert_eq!(out, vec![(1, "  - b".to_string())]);
        });
    }

    /// A continuation line is the item's own text and always travels with it,
    /// children or not — otherwise the wrap would detach from its bullet.
    #[test]
    fn a_continuation_line_travels_with_its_item() {
        over("- a\n- b\n  more b\n", |lists| {
            let out = shift_item(lists, 1, 1, false).unwrap();
            assert_eq!(
                out,
                vec![(1, "  - b".to_string()), (2, "    more b".to_string())]
            );
        });
    }

    /// The new indent is the previous sibling's CONTENT column, not a fixed
    /// step: under `2. b` that is 3, so the nested item lines up with the text
    /// it belongs to.
    #[test]
    fn the_new_indent_is_the_previous_siblings_content_column() {
        over("1. a\n2. b\n1. c\n", |lists| {
            let out = shift_item(lists, 2, 1, true).unwrap();
            assert_eq!(out, vec![(2, "   1. c".to_string())]);
        });
    }

    /// §5.6.6: an outdent at column zero is REFUSED, not silently turned into a
    /// headline. `<leader>o*` is how an item becomes a headline and it is a
    /// different gesture on purpose.
    #[test]
    fn outdenting_a_top_level_item_is_refused() {
        over("- a\n", |lists| {
            assert!(shift_item(lists, 0, -1, true).is_none());
        });
    }

    /// The first item of a list has nothing to nest under, and inventing a
    /// parent would produce a sublist with no owner.
    #[test]
    fn indenting_the_first_item_of_a_list_is_refused() {
        over("- a\n- b\n", |lists| {
            assert!(shift_item(lists, 0, 1, true).is_none());
        });
    }

    #[test]
    fn outdenting_returns_to_the_parents_own_indent() {
        over("- a\n  - a-a\n", |lists| {
            let out = shift_item(lists, 1, -1, true).unwrap();
            assert_eq!(out, vec![(1, "- a-a".to_string())]);
        });
    }

    // ── OS.7: move_item ───────────────────────────────────────────────────

    #[test]
    fn moving_an_item_carries_its_children_and_skips_theirs() {
        over("- a\n  - a-a\n- b\n", |lists| {
            // `- a` moves DOWN past `- b`; `- a-a` travels with it, and `- b`
            // is a sibling to step over rather than something to descend into.
            assert_eq!(move_item(lists, 0, 1).unwrap(), "- b\n- a\n  - a-a");
            assert_eq!(move_span(lists, 0, 1).unwrap(), (0, 2));
        });
    }

    #[test]
    fn moving_up_swaps_with_the_previous_sibling() {
        over("- a\n- b\n  - b-a\n", |lists| {
            assert_eq!(move_item(lists, 1, -1).unwrap(), "- b\n  - b-a\n- a");
            assert_eq!(move_span(lists, 1, -1).unwrap(), (0, 2));
        });
    }

    /// §5.6.6: a move stops at its parent rather than splicing the item into a
    /// neighbouring list.
    #[test]
    fn moving_the_last_sibling_down_is_refused() {
        over("- a\n- b\n", |lists| {
            assert!(move_item(lists, 1, 1).is_none());
        });
    }

    #[test]
    fn moving_the_first_sibling_up_is_refused() {
        over("- a\n- b\n", |lists| {
            assert!(move_item(lists, 0, -1).is_none());
        });
    }

    /// A nested item's siblings are its own sublist, so a move must not reach
    /// out to the items around its parent.
    #[test]
    fn a_nested_item_moves_only_among_its_own_siblings() {
        over("- a\n  - a-a\n  - a-b\n- b\n", |lists| {
            assert_eq!(move_item(lists, 1, 1).unwrap(), "  - a-b\n  - a-a");
            assert!(
                move_item(lists, 2, 1).is_none(),
                "`- b` is the PARENT's sibling, not this item's"
            );
        });
    }
}

/// The last line of the item at `start` NOT counting its nested children —
/// the item's own text plus its continuation lines.
///
/// The peer of [`Lists::item_end`], which includes children. The two exist
/// because the meta-arrows split exactly there: `<M-Right>` indents the item,
/// `<M-S-Right>` indents the item and everything under it, and that is the same
/// split `<leader>ol` / `<leader>oL` already make for headlines.
fn own_end(lists: &Lists<'_>, item: &Item) -> u32 {
    let mut end = item.line;
    for n in (item.line + 1)..lists.line_count {
        let Some(text) = lists.text(n) else { break };
        if text.trim().is_empty() {
            break;
        }
        if crate::headline::headline_level(&text).is_some() {
            break;
        }
        // A deeper BULLET is a child; a deeper non-bullet line is this item's
        // own wrapped text.
        if lists.item_at(n).is_some() {
            break;
        }
        if indent_of(&text) <= item.indent {
            break;
        }
        end = n;
    }
    end
}

/// The indent an outdent moves to: the enclosing parent item's own indent.
fn parent_indent(lists: &Lists<'_>, item: &Item) -> usize {
    let mut n = item.line;
    while n > 0 {
        n -= 1;
        let Some(text) = lists.text(n) else { break };
        if text.trim().is_empty() {
            continue;
        }
        if crate::headline::headline_level(&text).is_some() {
            break;
        }
        if let Some(above) = lists.item_at(n) {
            if above.indent < item.indent {
                return above.indent;
            }
        }
    }
    0
}

/// Re-indent the item at `start` by `delta` levels, carrying its continuation
/// lines and — when `with_children` — its nested items. Answers only the lines
/// that CHANGE, so a caller can fold them into one edit. `None` when the shift
/// is refused.
///
/// A "level" is not a fixed number of spaces. Indenting nests the item under
/// its previous sibling, so the new indent is that sibling's CONTENT column:
/// under `- a` that is 2, under `2. b` it is 3. Using a fixed step would leave
/// an item that does not line up with the text it belongs to, which is what
/// org's own indentation means.
///
/// Refused, each for its own reason:
/// - **an outdent at column zero** — §5.6.6. A top-level item has nowhere to go,
///   and silently turning it into a headline would be a restructure from a key
///   that means "move left". `<leader>o*` is how an item becomes a headline and
///   it is a different gesture on purpose.
/// - **an indent with no previous sibling** — the first item of a list has
///   nothing to nest under, and inventing a parent would produce a sublist with
///   no owner.
pub fn shift_item(
    lists: &Lists<'_>,
    start: u32,
    delta: isize,
    with_children: bool,
) -> Option<Vec<(u32, String)>> {
    let item = lists.item_at(start)?;
    let target = match delta.cmp(&0) {
        std::cmp::Ordering::Greater => {
            let prev = lists
                .siblings(start)
                .into_iter()
                .take_while(|&n| n < start)
                .last()?;
            lists.item_at(prev)?.content_byte as usize
        }
        std::cmp::Ordering::Less => {
            if item.indent == 0 {
                return None;
            }
            parent_indent(lists, &item)
        }
        std::cmp::Ordering::Equal => return None,
    };
    let shift = target as isize - item.indent as isize;
    if shift == 0 {
        return None;
    }

    let end = if with_children {
        lists.item_end(start)
    } else {
        own_end(lists, &item)
    };
    let mut out = Vec::new();
    for n in start..=end {
        let Some(text) = lists.text(n) else { break };
        if text.trim().is_empty() {
            continue;
        }
        // `max(0)`: an outdent whose carried children sit shallower than the
        // shift would otherwise underflow. Clamping keeps the block's shape.
        let new_indent = (indent_of(&text) as isize + shift).max(0) as usize;
        out.push((
            n,
            format!("{}{}", " ".repeat(new_indent), text.trim_start()),
        ));
    }
    Some(out)
}

/// The text of the item at `n` and everything it carries — continuation lines
/// and nested children — as it currently stands.
fn block(lists: &Lists<'_>, n: u32) -> Option<String> {
    let end = lists.item_end(n);
    let mut out = Vec::new();
    for i in n..=end {
        out.push(lists.text(i)?);
    }
    Some(out.join("\n"))
}

/// Swap the item at `start` with its previous (`delta < 0`) or next
/// (`delta > 0`) SIBLING, carrying continuation lines and children. `None` at
/// either end of the sibling chain.
///
/// Answers the rewritten BLOCK, not the changed lines [`shift_item`] and
/// [`renumber`] answer. That is not an inconsistency: a move rewrites one
/// contiguous span in which every line has shifted position, so "which lines
/// changed" is all of them.
///
/// **No trailing newline** — the caller writes this over a LINE RANGE, where a
/// trailing newline would insert a blank line on every move.
///
/// §5.6.6: the move stops at the sibling chain, and refusing at either end is
/// what keeps it from splicing the item into a neighbouring list. A sibling is
/// something to step OVER rather than descend into: `- b` sitting below `- a`'s
/// child is `- a`'s peer, not its next line.
pub fn move_item(lists: &Lists<'_>, start: u32, delta: isize) -> Option<String> {
    let siblings = lists.siblings(start);
    let idx = siblings.iter().position(|&n| n == start)?;
    match delta.cmp(&0) {
        std::cmp::Ordering::Less => {
            let prev = *siblings.get(idx.checked_sub(1)?)?;
            Some(format!("{}\n{}", block(lists, start)?, block(lists, prev)?))
        }
        std::cmp::Ordering::Greater => {
            let next = *siblings.get(idx + 1)?;
            Some(format!("{}\n{}", block(lists, next)?, block(lists, start)?))
        }
        std::cmp::Ordering::Equal => None,
    }
}

/// The line span [`move_item`]'s block must be written over: from the first of
/// the two swapped siblings through the end of the second.
pub fn move_span(lists: &Lists<'_>, start: u32, delta: isize) -> Option<(u32, u32)> {
    let siblings = lists.siblings(start);
    let idx = siblings.iter().position(|&n| n == start)?;
    match delta.cmp(&0) {
        std::cmp::Ordering::Less => {
            let prev = *siblings.get(idx.checked_sub(1)?)?;
            Some((prev, lists.item_end(start)))
        }
        std::cmp::Ordering::Greater => {
            let next = *siblings.get(idx + 1)?;
            Some((start, lists.item_end(next)))
        }
        std::cmp::Ordering::Equal => None,
    }
}

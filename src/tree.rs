//! The handful of tree walks every OT.x module needs, in one place.
//!
//! Each of `headline.rs`, `agenda.rs`, `checkbox.rs` and `table.rs` asks the
//! same three questions of the parse tree — "which node of kind K encloses this
//! point", "which of this node's children are kind K", "where does this node
//! actually stop" — and by OT.6 the third had three separate copies, each with
//! its own paragraph explaining the same off-by-one. That is the shape of a
//! rule about to drift, so it lives here once.
//!
//! Nothing org-specific belongs in this file. The node kinds are the callers'.

use crate::lattice::plugin_host::tree_sitter::{Node, TreeSnapshot};
use crate::lattice::plugin_host::types::{Position, Range};

/// The last line a node has content on.
///
/// A tree-sitter node's end position is **exclusive**, and several of org's
/// rules swallow their trailing newline — `plan` is `seq(repeat1(entry), _eol)`,
/// and a `section` runs to the start of the next one. So a node ending at byte 0
/// of a line really ended on the line before it.
///
/// Taken literally the raw end line makes every span one line too tall, which is
/// how this was first caught: a scheduled agenda row's excerpt covered two lines
/// where org means one.
pub fn last_content_line(range: &Range) -> u32 {
    if range.end.byte == 0 {
        range.end.line.saturating_sub(1)
    } else {
        range.end.line
    }
}

/// The innermost node of kind `kind` covering `(line, byte)`.
///
/// **`byte` is a column within the line, and passing 0 is not always right.**
/// A node that starts after some indentation — a `listitem` at its bullet — does
/// not contain column 0, so the walk lands in whatever encloses it and never
/// finds the kind being asked for. That silently disabled every indented
/// checkbox list once (OT.6). Nodes that genuinely start at column 0 (`section`)
/// can pass 0.
pub fn enclosing(tree: &TreeSnapshot, line: u32, byte: u32, kind: &str) -> Option<Node> {
    tree.enclosing(Position { line, byte }, &[kind.to_string()])
}

/// The nearest STRICT ancestor of `node` with kind `kind` — `node` itself never
/// matches, which is what makes it usable for "walk out one level at a time".
pub fn ancestor(node: &Node, kind: &str) -> Option<Node> {
    let mut cur = node.parent();
    while let Some(n) = cur {
        if n.kind() == kind {
            return Some(n);
        }
        cur = n.parent();
    }
    None
}

/// This node's named children of kind `kind`, in document order.
///
/// A named-child scan and not a query: org's containers have a handful of named
/// children (a `section` collapses its whole body into ONE `body` node), so this
/// is a fixed few host calls rather than something proportional to the text.
pub fn children_of_kind(node: &Node, kind: &str) -> Vec<Node> {
    (0..node.named_child_count())
        .filter_map(|i| node.named_child(i))
        .filter(|c| c.kind() == kind)
        .collect()
}

/// The first named child of kind `kind`.
pub fn first_child_of_kind(node: &Node, kind: &str) -> Option<Node> {
    (0..node.named_child_count())
        .filter_map(|i| node.named_child(i))
        .find(|c| c.kind() == kind)
}

/// The last named child of kind `kind`.
pub fn last_child_of_kind(node: &Node, kind: &str) -> Option<Node> {
    (0..node.named_child_count())
        .rev()
        .filter_map(|i| node.named_child(i))
        .find(|c| c.kind() == kind)
}

/// The next sibling of kind `kind`, skipping siblings of any other kind.
pub fn next_sibling_of_kind(node: &Node, kind: &str) -> Option<Node> {
    let mut cur = node.next_named_sibling();
    while let Some(n) = cur {
        if n.kind() == kind {
            return Some(n);
        }
        cur = n.next_named_sibling();
    }
    None
}

/// [`next_sibling_of_kind`] backwards.
pub fn prev_sibling_of_kind(node: &Node, kind: &str) -> Option<Node> {
    let mut cur = node.prev_named_sibling();
    while let Some(n) = cur {
        if n.kind() == kind {
            return Some(n);
        }
        cur = n.prev_named_sibling();
    }
    None
}

/// The text a node covers, when it lies on one line.
///
/// Multi-line returns `None` rather than a truncation: every caller here reads
/// a node that is within one line by construction (an `entry_name`, a
/// `timestamp`, a `cell`), so a multi-line one means the caller's assumption
/// broke and should say so.
pub fn node_text<'a>(lines: &[&'a str], node: &Node) -> Option<&'a str> {
    let range = node.byte_range();
    if range.start.line != range.end.line {
        return None;
    }
    let line = lines.get(range.start.line as usize).copied()?;
    line.get(range.start.byte as usize..range.end.byte as usize)
}

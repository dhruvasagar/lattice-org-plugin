//! OR.4 — harvesting [`Outline`] from a parse tree.
//!
//! The thin half of the split [`crate::roam`] describes: everything here is
//! tree navigation, everything there is pure. Nothing in this file decides what
//! a node *is* — it reports where the headlines, drawers and `#+keyword:` lines
//! are, and `roam::extract` reads their contents from the text.
//!
//! That boundary is not tidiness. A `tree-snapshot` is a WIT resource with no
//! constructor, so anything reached through one is unreachable from a host-side
//! unit test. Keeping the rules on the other side of this line is what lets
//! them be tested at all; this file is covered by the integration tests, which
//! run a real editor over a real file.
//!
//! ## The node kinds, and why they were dumped rather than read
//!
//! These come from printing a real parse, not from `grammar.js`. Two of them do
//! not say what the grammar source reads like, and both would have been guessed
//! wrong — see [`crate::roam`]'s module note for what and why.

use crate::lattice::plugin_host::tree_sitter::{Node, TreeSnapshot};
use crate::roam::{Outline, Section};
use crate::tree;

const SECTION: &str = "section";
const DIRECTIVE: &str = "directive";
const DRAWER: &str = "drawer";
const PROPERTY_DRAWER: &str = "property_drawer";

/// Harvest the structural facts [`crate::roam::extract`] needs.
///
/// `lines` is the file's text split by line, used only to read node text —
/// `node_text` needs it, and re-slicing the whole text per node would be the
/// expensive shape.
pub fn outline_from_tree(tree: &TreeSnapshot, lines: &[&str]) -> Outline {
    let root = tree.root();
    let mut outline = Outline::default();

    // The document `body` — everything before the first headline. Both the
    // file-level property drawer and the `#+title:` / `#+filetags:` lines live
    // here, which is why they are read together.
    if let Some(body) = tree::first_child_of_kind(&root, "body") {
        outline.file_drawer = file_property_drawer(&body, lines);
        outline.directives = directives_in(&body, lines);
    }

    // Sections, depth-first so a parent is always pushed before its children —
    // `Section::parent` is an index into this vec, so the order is part of the
    // contract rather than an accident of the walk.
    for child in tree::children_of_kind(&root, SECTION) {
        walk_section(&child, None, lines, &mut outline);
    }
    outline
}

/// The line range of a file-level `:PROPERTIES:` drawer's contents.
///
/// **A generic `drawer`, not a `property_drawer`.** The grammar attaches
/// `property_drawer` to a `section`, so a drawer above the first headline is a
/// plain `drawer` whose name happens to be `PROPERTIES`. Reading it as the
/// other kind finds nothing, and finding nothing here would drop 81% of the
/// reference corpus without a word.
fn file_property_drawer(body: &Node, lines: &[&str]) -> Option<(u32, u32)> {
    for drawer in tree::children_of_kind(body, DRAWER) {
        if !drawer_is_named_properties(&drawer, lines) {
            continue;
        }
        let range = drawer.byte_range();
        return Some((range.start.line, tree::last_content_line(&range)));
    }
    None
}

/// Whether a `drawer`'s name is `PROPERTIES`, case-insensitively.
///
/// The dump showed the name arriving as a bare `expr` holding the word without
/// its surrounding colons, so this compares that word rather than the `:…:`
/// form the file actually contains.
fn drawer_is_named_properties(drawer: &Node, lines: &[&str]) -> bool {
    drawer
        .child_by_field("name")
        .and_then(|name| tree::node_text(lines, &name))
        .is_some_and(|text| text.trim().eq_ignore_ascii_case("PROPERTIES"))
}

/// Every `#+name: value` line in the document body, name as written.
fn directives_in(body: &Node, lines: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut visit = |node: &Node| {
        if node.kind() != DIRECTIVE {
            return;
        }
        // Read the LINE rather than the `name`/`value` fields: the value's own
        // node is itself split into `expr`s on whitespace (the dump again), so
        // reassembling it from children would lose the spacing a title needs.
        let line_no = node.byte_range().start.line;
        let Some(line) = lines.get(line_no as usize) else {
            return;
        };
        let Some(rest) = line.trim_start().strip_prefix("#+") else {
            return;
        };
        let Some((name, value)) = rest.split_once(':') else {
            return;
        };
        out.push((name.trim().to_string(), value.trim().to_string()));
    };
    for child in named_children(body) {
        visit(&child);
        // A directive may be attached to the element that follows it
        // (`_directive_list` is a prefix of `paragraph`, `drawer`, `block`, …),
        // so one level down is where the `#+title:` above a paragraph lands.
        for grandchild in named_children(&child) {
            visit(&grandchild);
        }
    }
    out
}

/// Push `section` and every subsection beneath it into `outline`.
fn walk_section(section: &Node, parent: Option<usize>, lines: &[&str], outline: &mut Outline) {
    let range = section.byte_range();
    let index = outline.sections.len();

    let headline = section.child_by_field("headline");
    let headline_line = headline
        .as_ref()
        .map(|h| h.byte_range().start.line)
        .unwrap_or(range.start.line);
    // The level is the WIDTH of the `stars` node rather than a recount of `*`
    // characters — the grammar already decided where the stars end, and
    // counting again is how the two drift apart (`headline.rs`'s rule).
    let level = headline
        .as_ref()
        .and_then(|h| h.child_by_field("stars"))
        .map(|s| {
            let r = s.byte_range();
            r.end.byte.saturating_sub(r.start.byte)
        })
        .unwrap_or(1);

    let raw_title = headline
        .as_ref()
        .and_then(|h| h.child_by_field("item"))
        .and_then(|item| tree::node_text(lines, &item))
        .unwrap_or("")
        .to_string();

    let own_tags = headline
        .as_ref()
        .and_then(|h| tree::first_child_of_kind(h, "tag_list"))
        .map(|list| {
            tree::children_of_kind(&list, "tag")
                .iter()
                .filter_map(|t| tree::node_text(lines, t))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // A headline's drawer IS a `property_drawer` — this is the grain the
    // grammar models structurally, and the one the file grain is not.
    let drawer = section
        .child_by_field(PROPERTY_DRAWER)
        .or_else(|| tree::first_child_of_kind(section, PROPERTY_DRAWER))
        .map(|d| {
            let r = d.byte_range();
            (r.start.line, tree::last_content_line(&r))
        });

    outline.sections.push(Section {
        headline_line,
        end_line: tree::last_content_line(&range),
        level,
        raw_title,
        own_tags,
        drawer,
        parent,
    });

    for child in tree::children_of_kind(section, SECTION) {
        walk_section(&child, Some(index), lines, outline);
    }
}

/// A node's named children, as a vec — the walks here are over a handful of
/// children each (org's containers collapse whole runs into one `body`), so
/// this is a few host calls rather than something proportional to the text.
fn named_children(node: &Node) -> Vec<Node> {
    (0..node.named_child_count())
        .filter_map(|i| node.named_child(i))
        .collect()
}

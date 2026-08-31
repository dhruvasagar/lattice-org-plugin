//! OR.7 — org-roam nodes as an **insert-mode completion source**.
//!
//! Design: `docs/dev/architecture/org-roam.md` §5 in the lattice tree.
//!
//! ## Why completion and not a picker
//!
//! You insert a link *mid-sentence*. A normal-mode chord that opens a picker
//! makes you leave Insert, pick, and come back; emacs does not ask that either
//! — you type `[[` and completion offers nodes. The slice plan's original
//! `:org-roam-insert-node` picker is deferred (OR.7c) rather than dropped, for
//! the case completion cannot serve.
//!
//! ## The two things a completion source has to get right
//!
//! **When it applies.** A plugin's source is offered to *every* buffer, and
//! this one must contribute nothing outside an org buffer and nothing outside
//! a link. `generate-context` carries `language` and `line-before-cursor` for
//! exactly this: the host does not know what `[[` means and must not learn.
//!
//! **What it replaces.** The popup's replacement region is `[anchor, cursor]`,
//! fixed when the popup opens, and the host's anchor scan stops at any
//! non-word byte — so opening right after `[[` anchors there and the query
//! grows across spaces as you type, which is what makes multi-word titles
//! work. Opening *mid-title* anchors at the last word instead, and accepting
//! would splice the link over that word alone:
//!
//! ```text
//!     see [[Ti               anchor lands after "[[", region is "Ti"
//!     see [[Some Ti          anchor lands at "Ti" — the "[[" is far behind
//! ```
//!
//! In the first line the region to replace is exactly what was typed after
//! the opener; in the second it is the last word, and splicing a link over it
//! would leave `see [[Some ` stranded in front.
//!
//! [`link_prefix`] is the guard: it answers `Some(query)` only when the text
//! between `[[` and the cursor **is** the query the host handed us, i.e. only
//! when the anchor already covers what we mean to replace. Auto-trigger
//! part-way through a title fails that check and this source declines, which
//! is the correct answer — not a bug to work around.
//!
//! ## Matching stays native, as with the picker
//!
//! The guest returns every node once per popup-open; the host's fuzzy matcher
//! filters on each keystroke. Matching in the guest would put a WASM crossing
//! on every keystroke (paramount #1).
//!
//! Candidates match on the **title** and insert the **link** — the
//! `insert-text` field OR.7 added to `raw-candidate`. Matching on the link
//! instead would score every node on its uuid.

use crate::lattice::plugin_host::types::{
    CandidateData, CandidateKind, GenerateContext, RawCandidate,
};
use crate::roam::Node;
use crate::roam_index;
use crate::roam_scan;

/// The registered id. Dashed and namespaced like the picker's, and prefixed
/// `gen:` to match the host's own generator ids (`gen:lsp-completion`,
/// `gen:snippet`) — the string a user types in
/// `:set completion.source.<id>.priority=…`.
pub const NODE_SOURCE: &str = "gen:org-roam-node";

/// The opener this source completes inside.
const LINK_OPEN: &str = "[[";

/// The source's declaration.
pub fn spec() -> crate::lattice::plugin_host::types::CompletionSourceSpec {
    crate::lattice::plugin_host::types::CompletionSourceSpec {
        id: NODE_SOURCE.to_string(),
        doc: "Org-roam nodes, inside an [[…]] link".to_string(),
        // Node titles are phrases — "Honey Garlic Chicken Breast". Without
        // this the host dismisses the popup at the first space and no
        // multi-word title can ever be narrowed to.
        accepts_non_word_query: true,
    }
}

/// The query this source should complete, or `None` when it does not apply.
///
/// `Some(q)` requires all of:
///
/// * an unclosed `[[` on the line before the cursor, and
/// * the text after it equal to `prefix` — the anchor guard described in the
///   module docs. Equality, not `ends_with`: the two differ exactly when the
///   anchor sits mid-title, which is the case that must decline.
///
/// A closed link earlier on the line is not an opener: `[[a]] [[b` completes
/// `b`, and `[[a]] x` completes nothing.
pub fn link_prefix(line_before_cursor: &str, prefix: &str) -> Option<String> {
    let open = line_before_cursor.rfind(LINK_OPEN)?;
    let after = &line_before_cursor[open + LINK_OPEN.len()..];
    // A `]]` between the opener and the cursor means that link is closed and
    // the cursor is outside it.
    if after.contains("]]") {
        return None;
    }
    if after != prefix {
        return None;
    }
    Some(after.to_string())
}

/// One candidate per node: matched on its title, inserted as an id link.
///
/// The description is the title as it stands *now*, which is the point of the
/// id model — the link keeps resolving when the note is renamed, and the
/// description is a snapshot the user is free to edit.
fn candidate(node: &Node) -> RawCandidate {
    RawCandidate {
        text: node.title.clone(),
        // NO leading `[[`. The replacement region is `[anchor, cursor]` and
        // the host's anchor scan stops at `[`, so the opener sits *before*
        // the anchor and stays in the buffer untouched. Emitting it again
        // would write `see [[[[id:…][…]]`. `link_prefix` is what makes this
        // safe to rely on: it answers only when the text after the opener is
        // exactly the region being replaced.
        insert_text: Some(format!("id:{}][{}]]", node.id, node.title)),
        display: node.title.clone(),
        source: Some(NODE_SOURCE.to_string()),
        kind: CandidateKind::Plain,
        data: CandidateData::Plain,
        annotations: Vec::new(),
        // PS.1: the completion popup shows node titles too, and a title that
        // is a headline in the buffer should not become plain text because it
        // is being offered rather than read.
        display_spans: crate::roam::title_display_spans(&node.title, &node.title),
    }
}

/// Produce the candidate set for `ctx`, or an empty set when this source does
/// not apply.
///
/// Empty rather than an error in every declining case: an `err` is logged by
/// the host and means "this source is broken", which "the cursor is not in a
/// link" is not.
pub fn generate(ctx: &GenerateContext) -> Vec<RawCandidate> {
    if ctx.language != "org" {
        return Vec::new();
    }
    if link_prefix(&ctx.line_before_cursor, &ctx.prefix).is_none() {
        return Vec::new();
    }
    // Configured-ness is checked after the cheap guards: an unconfigured roam
    // must cost nothing on a keystroke path, and reading an option is a host
    // call.
    if roam_scan::roam_directory().is_none() {
        return Vec::new();
    }
    roam_index::all_nodes().iter().map(candidate).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, title: &str) -> Node {
        Node {
            id: id.to_string(),
            title: title.to_string(),
            aliases: Vec::new(),
            tags: Vec::new(),
            refs: Vec::new(),
            file: "/notes/a.org".to_string(),
            line: 0,
            level: 0,
        }
    }

    #[test]
    fn a_link_opener_yields_the_typed_query() {
        assert_eq!(link_prefix("see [[Ti", "Ti"), Some("Ti".to_string()));
        assert_eq!(link_prefix("[[", ""), Some(String::new()));
    }

    #[test]
    fn a_multi_word_query_is_carried_whole() {
        // The case the anchor makes possible: opening right after `[[`
        // anchors there, so the query grows across the space.
        assert_eq!(
            link_prefix("see [[Some Ti", "Some Ti"),
            Some("Some Ti".to_string())
        );
    }

    #[test]
    fn an_anchor_short_of_the_opener_declines() {
        // Auto-trigger part-way through a title: the host anchored at the
        // last word, so accepting would splice the link over "Ti" alone and
        // leave "see [[Some " in front of it.
        assert_eq!(link_prefix("see [[Some Ti", "Ti"), None);
    }

    #[test]
    fn text_outside_a_link_declines() {
        assert_eq!(link_prefix("plain words", "words"), None);
        assert_eq!(link_prefix("", ""), None);
    }

    #[test]
    fn a_closed_link_is_not_an_opener() {
        assert_eq!(link_prefix("[[id:a][A]] and", "and"), None);
    }

    #[test]
    fn a_second_opener_after_a_closed_link_completes() {
        assert_eq!(link_prefix("[[id:a][A]] [[Ti", "Ti"), Some("Ti".into()));
    }

    #[test]
    fn a_candidate_matches_the_title_and_inserts_the_link() {
        let c = candidate(&node("abc-123", "Honey Garlic Chicken"));
        assert_eq!(c.text, "Honey Garlic Chicken");
        assert_eq!(c.display, "Honey Garlic Chicken");
        assert_eq!(
            c.insert_text.as_deref(),
            Some("id:abc-123][Honey Garlic Chicken]]")
        );
        assert_eq!(c.source.as_deref(), Some(NODE_SOURCE));
    }

    #[test]
    fn the_insert_does_not_repeat_the_opener_already_in_the_buffer() {
        // The opener sits before the anchor, so it is not part of what gets
        // replaced. Emitting it again writes `[[[[id:…`.
        let c = candidate(&node("x", "T"));
        assert!(!c.insert_text.as_deref().unwrap().starts_with("[["));
    }

    #[test]
    fn accepting_reconstructs_the_whole_link() {
        // The composition the two halves have to satisfy: buffer text up to
        // the anchor, plus the insert, is a well-formed id link.
        let line = "see [[Ti";
        let query = link_prefix(line, "Ti").expect("in a link");
        let anchor = line.len() - query.len();
        let c = candidate(&node("abc", "Title"));
        let accepted = format!("{}{}", &line[..anchor], c.insert_text.unwrap());
        assert_eq!(accepted, "see [[id:abc][Title]]");
    }
}

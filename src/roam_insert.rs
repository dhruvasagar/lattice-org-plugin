//! OR.7c — `:org-roam-insert-node`: pick a note, insert a link to it.
//!
//! Design: `docs/dev/architecture/org-roam.md` §5 in the lattice tree.
//!
//! ## Why this exists after OR.7 said it would not
//!
//! OR.7 shipped node insertion as an **insert-mode completion source** and
//! deferred this picker with a condition attached: *"Revisit only if
//! completion proves insufficient for inserting a link — not on the assumption
//! that it will."* It did prove insufficient, and the reason is a property of
//! the corpus rather than of the implementation.
//!
//! A completion popup offers every node the moment you type `[[`. On the
//! reference corpus that is 585 rows, ranked by a fuzzy matcher against a query
//! that is empty at the instant the popup opens — so the first thing you see is
//! 585 notes in arbitrary order, and you narrow by typing into a surface that
//! was never meant to be searched, only completed. The picker is the surface
//! for "find the one I mean among hundreds": full-height, its own prompt, and
//! the query is the point rather than a side effect of typing a title you are
//! trying to remember.
//!
//! **Both stay.** Completion is right for the case it was built for — you know
//! the title, you are mid-sentence, and typing four characters finishes it.
//! This is right when you do not. `C-c n i` is emacs' own binding for
//! `org-roam-node-insert`, so the muscle memory is already there.
//!
//! ## The candidate set is `roam_find`'s, deliberately
//!
//! Same nodes, same title-and-alias matching, same annotations — reusing
//! [`crate::roam_find::candidate_for`] rather than growing a parallel one.
//! Two pickers over one corpus that disagreed about which notes exist would be
//! a bug nobody could see from either of them.
//!
//! What differs is the ROUTING, and only that: `find` carries a location and
//! jumps, this carries a link and inserts.
//!
//! ## Why the accept routes through an ex-command
//!
//! `picker-accept-outcome` has no "insert this text at the cursor" arm, and it
//! should not grow one for this: the picker seam has no cursor and no document
//! (`init` receives args and a `picker-context`, not the buffer you came from),
//! which is the same wall OR.9's backlinks hit. So the accept returns
//! `invoke-command`, and the ex-command — which runs on the grammar seam, where
//! there IS a cursor and a buffer id — performs the edit.
//!
//! That is the create row's existing mechanism (OR.5 routes into
//! `:org-roam-create-node`), used a second time rather than invented.

use crate::lattice::plugin_host::types::{
    Args, CommandRef, PickerAcceptOutcome, PickerSourceSpec, RawCandidate, RoutingPayload,
};
use crate::roam::Node;
use crate::roam_find;
use crate::roam_index;
use crate::roam_scan;

/// The registered id. Dashed and namespaced like `roam_find`'s, and distinct
/// from it — two sources over the same corpus that shared an id would have the
/// host resolve one of them and silently drop the other.
pub const INSERT_NODE_PICKER: &str = "org-roam-insert";

/// The ex-command an accepted row routes into.
pub const INSERT_LINK_COMMAND: &str = "org-roam-insert-link";

/// The ex-command the CREATE row routes into: mint a node, then link it.
pub const CREATE_AND_INSERT_COMMAND: &str = "org-roam-create-and-insert";

/// The source's declaration.
pub fn spec() -> PickerSourceSpec {
    PickerSourceSpec {
        id: INSERT_NODE_PICKER.to_string(),
        doc: "Insert a link to an org-roam note".to_string(),
        args_schema: Vec::new(),
        args_hint: String::new(),
        // Not live, for `roam_find`'s reason: the candidate set is the index,
        // which cannot change while the picker is open.
        live: false,
        // OR.5's offer, carried here too. Linking to a note you have not
        // written yet is how a roam corpus actually grows — you are writing a
        // sentence, you reference a thing, the note for it comes later.
        create_label: Some("Create and link: %s".to_string()),
    }
}

/// `[[id:<uuid>][<title>]]` — the link org-roam writes.
///
/// **The description is the node's title AT INSERT TIME**, which is org-roam's
/// own behaviour and worth saying out loud because the alternative looks
/// tempting: a bare `[[id:<uuid>]]` would always render the node's *current*
/// title and never go stale. Org does not do that, and the reason is that the
/// description is part of the SENTENCE you are writing — you may want "the
/// chicken recipe" where the node is titled "Honey Garlic Chicken Breast", and
/// a link that rewrote itself would edit your prose.
///
/// The id is what resolves (OR.8), so a later title change breaks nothing.
pub fn link_for(node: &Node) -> String {
    format!("[[id:{}][{}]]", node.id, node.title)
}

/// Build the candidate set: one row per node, routed to insert its link.
pub fn init() -> Result<Vec<(RawCandidate, RoutingPayload)>, String> {
    if roam_scan::roam_directory().is_none() {
        // Said rather than shown as an empty list, for `roam_find`'s reason:
        // "no notes" and "roam is not configured" look identical in an empty
        // picker and have entirely different fixes.
        return Err(
            "org-roam: set `org.roam-directory` to the folder your notes live in".to_string(),
        );
    }
    let nodes = roam_index::all_nodes();
    Ok(nodes
        .iter()
        .map(|node| {
            // The ROW is find's, verbatim — same display, same alias matching,
            // same annotations. Only the routing is replaced.
            let (candidate, _) = roam_find::candidate_for(node);
            (
                RawCandidate {
                    // The source id has to be this picker's, or an accept would
                    // be attributed to `find` and jump instead of inserting.
                    source: Some(INSERT_NODE_PICKER.to_string()),
                    ..candidate
                },
                RoutingPayload::InvokeCommand(CommandRef {
                    id: INSERT_LINK_COMMAND.to_string(),
                    // The whole link, resolved HERE where the node is in hand.
                    // Carrying the id alone would make the ex-command consult
                    // the index again to learn a title the picker already had.
                    args: Args::String(link_for(node)),
                }),
            )
        })
        .collect())
}

/// Resolve a chosen row.
pub fn accept(routing: RoutingPayload) -> Result<PickerAcceptOutcome, String> {
    match routing {
        // An existing node: the link is already built, so this is a forward.
        RoutingPayload::InvokeCommand(cmd) => Ok(PickerAcceptOutcome::InvokeCommand(cmd)),
        // OR.5's create row. Routes into a DIFFERENT command from
        // `roam_find`'s: that one creates and opens the new note, this one
        // creates it and links it from where you were standing. Opening it
        // would be wrong here — you are mid-sentence, and being navigated away
        // from the paragraph you were writing is not what "insert a link"
        // means.
        RoutingPayload::Create(title) => Ok(PickerAcceptOutcome::InvokeCommand(CommandRef {
            id: CREATE_AND_INSERT_COMMAND.to_string(),
            args: Args::String(title),
        })),
        _ => Err("org-roam: insert-node got a routing token it did not emit".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, title: &str) -> Node {
        Node {
            id: id.to_string(),
            title: title.to_string(),
            file: "/notes/a.org".to_string(),
            line: 0,
            aliases: Vec::new(),
            tags: Vec::new(),
            refs: Vec::new(),
            level: 0,
        }
    }

    /// The link org-roam writes: an `id:` target and the title as description.
    #[test]
    fn a_link_carries_the_id_and_the_title() {
        assert_eq!(
            link_for(&node("abc-123", "Honey Garlic Chicken Breast")),
            "[[id:abc-123][Honey Garlic Chicken Breast]]"
        );
    }

    /// The id is what resolves, so a title with link-ish characters in it
    /// cannot break the target half.
    #[test]
    fn a_title_never_corrupts_the_targets_half() {
        let l = link_for(&node("abc-123", "Notes [draft]"));
        assert!(
            l.starts_with("[[id:abc-123]["),
            "the id half is intact: {l}"
        );
    }

    /// An existing node forwards its prepared command rather than rebuilding
    /// one — the picker already resolved the link.
    #[test]
    fn accepting_a_node_forwards_the_insert_command() {
        let routing = RoutingPayload::InvokeCommand(CommandRef {
            id: INSERT_LINK_COMMAND.to_string(),
            args: Args::String("[[id:x][T]]".to_string()),
        });
        match accept(routing).unwrap() {
            PickerAcceptOutcome::InvokeCommand(c) => {
                assert_eq!(c.id, INSERT_LINK_COMMAND);
                assert!(matches!(c.args, Args::String(ref s) if s == "[[id:x][T]]"));
            }
            other => panic!("expected an invoke-command, got {other:?}"),
        }
    }

    /// **The create row does NOT route where `find`'s does.** Creating from
    /// the insert picker must link from where you are standing, not navigate
    /// you to the new note — pinned because the two commands differ by one
    /// string and reusing `find`'s would look correct and read wrong.
    #[test]
    fn the_create_row_creates_and_inserts_rather_than_opening() {
        match accept(RoutingPayload::Create("Rust Async".to_string())).unwrap() {
            PickerAcceptOutcome::InvokeCommand(c) => {
                assert_eq!(c.id, CREATE_AND_INSERT_COMMAND);
                assert_ne!(
                    c.id,
                    roam_find::CREATE_NODE_COMMAND,
                    "find's create OPENS the note; insert's must not"
                );
            }
            other => panic!("expected an invoke-command, got {other:?}"),
        }
    }

    /// A token this source never emitted is refused rather than guessed at.
    #[test]
    fn a_foreign_routing_token_is_refused() {
        assert!(accept(RoutingPayload::Buffer(7)).is_err());
    }
}

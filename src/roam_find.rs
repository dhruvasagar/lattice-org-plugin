//! OR.6 — `:org-roam-find-node`: the picker over the index.
//!
//! Design: `docs/dev/architecture/org-roam.md` §5 in the lattice tree.
//!
//! ## What it searches, and what it deliberately does not
//!
//! **Titles and aliases. Never filenames.** The corpus is the argument:
//! `20250603103551-chicken_breast_honey_garlic.org` holds a node titled *Honey
//! Garlic Chicken Breast*, and the slug is a fossil of a title the note had
//! years ago. Matching filenames would rank notes by what they used to be
//! called — which is not a bug in the slug, it is the id model working, and it
//! is exactly why the picker must ignore the filename.
//!
//! Aliases matter for the same reason and are not a nicety: 71 nodes in the
//! reference corpus are findable under a name that is not their title.
//!
//! ## Matching stays native
//!
//! The guest hands the host every node's title as candidate text and lets the
//! picker's own fuzzy ranker do the work. Matching in the guest would put a
//! WASM crossing on **every keystroke of the query**, which is the one thing a
//! picker must not do (paramount #1). The guest is called once per open.
//!
//! ## The two accept paths
//!
//! Picking a node **jumps** — `jump-to-location(path, line, col)`, so a
//! headline node lands on its headline rather than on its file's first line.
//!
//! Picking the **create row** cannot jump, because there is nothing there yet.
//! It routes `invoke-command` into org's own `:org-roam-create-node`, which is
//! refile's bridge and is used here for a sharper reason: creating a note has
//! to mint an id and write a file, and `picker-accept-outcome` can express
//! neither. The ex-command can — it runs on the grammar seam, where
//! `new-uuid` is reachable (that is the whole reason OR.3 is host-side) and
//! where an `Effect::WriteToFile` is the host's job rather than the guest's.

use crate::lattice::plugin_host::types::{
    Annotation, AnnotationCustom, CandidateData, CandidateKind, CommandRef, Location,
    PickerAcceptOutcome, PickerSourceSpec, RawCandidate, RoutingPayload,
};
use crate::roam::Node;
use crate::roam_index;
use crate::roam_scan;

/// The registered id. Dashed and namespaced, per the naming rule — no
/// collapsed form, no generic `find-node` alias.
pub const FIND_NODE_PICKER: &str = "org-roam-node";

/// The ex-command the create row routes into.
pub const CREATE_NODE_COMMAND: &str = "org-roam-create-node";

/// The source's declaration.
pub fn spec() -> PickerSourceSpec {
    PickerSourceSpec {
        id: FIND_NODE_PICKER.to_string(),
        doc: "Org-roam notes, by title or alias".to_string(),
        args_schema: Vec::new(),
        args_hint: String::new(),
        // Not live: the candidate set is the index, which does not change while
        // the picker is open. A live source would re-read the whole `nodes`
        // blob on every keystroke to produce the same rows.
        live: false,
        // OR.5: the offer to create what was not found. Present whenever the
        // query is non-empty and pinned last — so `<CR>` never creates a
        // duplicate by ranking accident, and *Rust* can still be created while
        // *Rust Async* exists.
        create_label: Some("Create note: %s".to_string()),
    }
}

/// Build the candidate set: one row per node.
///
/// Reads the `nodes` blob once — the entire reason §4.2 keeps it beside the
/// per-id records. 585 nodes is one `get` and one deserialize, not 585 host
/// calls.
pub fn init() -> Result<Vec<(RawCandidate, RoutingPayload)>, String> {
    if roam_scan::roam_directory().is_none() {
        // Say so rather than showing an empty picker. "No notes" and "roam is
        // not configured" look identical in an empty list, and they have
        // entirely different fixes.
        return Err(
            "org-roam: set `org.roam-directory` to the folder your notes live in".to_string(),
        );
    }
    let nodes = roam_index::all_nodes();
    Ok(nodes.iter().map(candidate_for).collect())
}

/// One node's row.
///
/// `text` is the title and `display` is the title: the picker matches on
/// `display`, and both are the title because that is what the user is looking
/// for. Aliases and tags ride as **annotations**, which the picker shows beside
/// the row without matching them positionally.
pub fn candidate_for(node: &Node) -> (RawCandidate, RoutingPayload) {
    let mut annotations = Vec::new();
    if !node.aliases.is_empty() {
        annotations.push(Annotation::Custom(AnnotationCustom {
            text: node.aliases.join(", "),
            // A REAL annotation slot key, not a syntax element name.
            // `BuiltinElementIds::annotation_slot` maps the
            // `completion.annotation.*` vocabulary and falls back to
            // `.custom` for anything else — so the `comment` this used to
            // pass resolved to the same fixed colour as the tags below, which
            // is why the picker looked unstyled. `.doc` is the descriptive
            // column, which is what an alias is.
            slot: "completion.annotation.doc".to_string(),
        }));
    }
    if !node.tags.is_empty() {
        annotations.push(Annotation::Custom(AnnotationCustom {
            text: format!(":{}:", node.tags.join(":")),
            // `.kind` for the same reason: it is the classifying column, and
            // it resolves to a different element from `.doc`, so aliases and
            // tags are finally distinguishable.
            slot: "completion.annotation.kind".to_string(),
        }));
    }
    // An alias is searchable by being part of the matched text, not by being an
    // annotation — annotations are shown, not matched. A node findable ONLY
    // under an alias is 12% of the reference corpus, so this is the difference
    // between the feature working and appearing to.
    let display = if node.aliases.is_empty() {
        node.title.clone()
    } else {
        format!("{}  ({})", node.title, node.aliases.join(", "))
    };
    // PS.1: a node title is a headline's text, so it renders as one. Without
    // this the row is plain — there are no stars and no file line for a
    // grammar to match by the time a title reaches a picker.
    let display_spans = crate::roam::title_display_spans(&display, &node.title);
    (
        RawCandidate {
            insert_text: None,
            text: node.title.clone(),
            display,
            source: Some(FIND_NODE_PICKER.to_string()),
            kind: CandidateKind::Plain,
            data: CandidateData::Plain,
            annotations,
            display_spans,
        },
        // The node's location, not its id: `accept` should not have to consult
        // the index again to answer where to go.
        RoutingPayload::FileLocation(Location {
            path: node.file.clone(),
            line: node.line,
            col: 0,
        }),
    )
}

/// Resolve a chosen row.
pub fn accept(routing: RoutingPayload) -> Result<PickerAcceptOutcome, String> {
    match routing {
        // A node. Jump to its file AND line, so a headline node lands on its
        // headline rather than at the top of a file it shares with others.
        RoutingPayload::FileLocation(loc) => Ok(PickerAcceptOutcome::JumpToLocation(loc)),
        // OR.5's create row. Route into org's own ex-command: creating a note
        // mints an id and writes a file, and `picker-accept-outcome` can
        // express neither.
        RoutingPayload::Create(title) => Ok(PickerAcceptOutcome::InvokeCommand(CommandRef {
            id: CREATE_NODE_COMMAND.to_string(),
            args: crate::lattice::plugin_host::types::Args::String(title),
        })),
        _ => Err("org-roam: find-node got a routing token it did not emit".to_string()),
    }
}

/// The filename slug for a new note's title.
///
/// `Honey Garlic Chicken Breast` → `honey_garlic_chicken_breast`. Lowercased,
/// non-alphanumerics collapsed to one underscore, trimmed — org-roam's own
/// `org-roam-node-slug` shape.
///
/// The slug is cosmetic and known to be so: it is a **fossil** the moment the
/// title changes, which is the id model working rather than a defect. It exists
/// so a directory listing is legible, not so anything can be found by it.
pub fn slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut pending_sep = false;
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            pending_sep = false;
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
        } else {
            pending_sep = true;
        }
    }
    out
}

/// The body a newly-created node starts with.
///
/// A **minimal built-in** template, deliberately: user templates land at OR.11,
/// and keeping them apart means a template bug cannot masquerade as a picker
/// bug. `id` is host-minted (OR.3) and `title` is what the user typed.
pub fn new_node_text(id: &str, title: &str) -> String {
    format!(":PROPERTIES:\n:ID:       {id}\n:END:\n#+title: {title}\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_is_lowercase_words_joined_by_underscores() {
        assert_eq!(
            slug("Honey Garlic Chicken Breast"),
            "honey_garlic_chicken_breast"
        );
        assert_eq!(slug("Rust"), "rust");
    }

    /// Punctuation collapses rather than doubling up, and a title that is all
    /// punctuation does not produce a file called `.org`.
    #[test]
    fn punctuation_collapses_and_never_leaves_a_bare_extension() {
        assert_eq!(slug("C++: the good parts"), "c_the_good_parts");
        assert_eq!(slug("  spaced  out  "), "spaced_out");
        assert_eq!(
            slug("!!!"),
            "",
            "an empty slug is the caller's problem to name"
        );
    }

    /// Non-ASCII survives — a corpus is not all English, and transliterating
    /// would make the slug wrong in a way the user cannot correct.
    #[test]
    fn non_ascii_titles_keep_their_letters() {
        assert_eq!(slug("Über Alles"), "über_alles");
        assert_eq!(slug("日本語 note"), "日本語_note");
    }

    /// The new-note body is what OR.4's extraction reads back: a file-level
    /// drawer with an `:ID:`, and a `#+title:`. If these two ever disagree, a
    /// created note is invisible to the picker that created it.
    #[test]
    fn a_new_note_carries_a_file_level_id_and_title() {
        let text = new_node_text("ABC-123", "Honey Garlic");
        assert!(text.starts_with(":PROPERTIES:\n"));
        assert!(text.contains(":ID:       ABC-123\n"));
        assert!(text.contains("#+title: Honey Garlic\n"));
        assert!(text.contains(":END:\n"), "the drawer is closed");
    }

    /// The round trip that matters: what `new_node_text` writes, `roam::extract`
    /// reads. A change to either that breaks the other makes a created note
    /// unfindable — and nothing else in the suite would notice.
    #[test]
    fn a_created_note_is_extractable_as_a_node() {
        let text = new_node_text("ABC-123", "Honey Garlic");
        let outline = crate::roam::Outline {
            // The drawer spans lines 0..2 (`:PROPERTIES:`, `:ID:`, `:END:`).
            file_drawer: Some((0, 2)),
            directives: vec![("title".to_string(), "Honey Garlic".to_string())],
            sections: Vec::new(),
        };
        let got = crate::roam::extract("/roam/honey_garlic.org", &text, &outline, &[]);
        assert_eq!(got.len(), 1, "the note it just wrote IS a node");
        assert_eq!(got[0].node.id, "ABC-123");
        assert_eq!(got[0].node.title, "Honey Garlic");
    }
}

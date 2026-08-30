//! OR.9 — `:org-roam-backlinks`: what points at the note you are in.
//!
//! Design: `docs/dev/architecture/org-roam.md` §6.1 in the lattice tree.
//!
//! ## A picker, and what that costs
//!
//! §6.1 specifies a **multibuffer** view — one excerpt per linking node, read
//! and edited in place. This slice ships a **picker** instead, which is a jump
//! list: you can see what links here and go there, but not read the linking
//! text in place and not edit it from the view.
//!
//! **That is a choice about backlinks, not a limitation.** When this was
//! written a guest could not own a multibuffer at all — `ProviderViewOpener` is
//! a native Rust closure — but MV.1 has since built the
//! `multibuffer-view-source` seam, so the constraint is gone. A picker is still
//! right here: backlinks is navigation. You look at what points here and go
//! read it; you do not sit in the list editing. The multibuffer's affordances —
//! read in place, edit propagates to source, `gr` — are what the agenda needs
//! and what a jump list does not.
//!
//! The read-in-place peer is a real thing someone may want later, and it is now
//! buildable on the seam rather than blocked on one.
//!
//! ## What the index already knows, and what it does not
//!
//! `b/<id>` holds the ids of the nodes that link TO `<id>` — that is the whole
//! query, one `get`. What it does not hold is **which line** the link sits on,
//! so a row points at the linking node's own anchor (its headline, or its
//! file's first line) rather than at the link itself. Emacs shows the link's
//! line because its database stores a point per link.
//!
//! Storing the line is the honest fix and it belongs with the multibuffer
//! slice, because that is the slice that actually needs it: an excerpt must
//! show the linking *line*, whereas a jump to the linking *note* is a useful
//! answer on its own. Adding the column now would change the `b/<id>` schema
//! for a consumer that cannot use it yet.

use crate::lattice::plugin_host::types::{
    Annotation, AnnotationCustom, CandidateData, CandidateKind, Location, PickerAcceptOutcome,
    PickerSourceSpec, RawCandidate, RoutingPayload,
};
use crate::roam::Node;
use crate::roam_index;
use crate::roam_scan;

/// The registered id. Dashed and namespaced, per the naming rule.
pub const BACKLINKS_PICKER: &str = "org-roam-backlinks";

/// The source's declaration.
pub fn spec() -> PickerSourceSpec {
    PickerSourceSpec {
        id: BACKLINKS_PICKER.to_string(),
        doc: "Notes that link to the note at point".to_string(),
        args_schema: Vec::new(),
        args_hint: String::new(),
        // Not live: the candidate set is one index read, and the index does not
        // change while the picker is open. `gr`-style refresh is the
        // multibuffer's affordance, not a picker's.
        live: false,
        create_label: None,
    }
}

/// Build the rows: one per node that links to `id`.
///
/// An empty result is an **honest empty picker**, not an error: "nothing links
/// here yet" is a true and useful answer about a real node. That is the
/// opposite of `find_node`'s unconfigured case, which errors precisely because
/// an empty list there would be indistinguishable from "you have no notes".
pub fn init(args: &[String]) -> Result<Vec<(RawCandidate, RoutingPayload)>, String> {
    if roam_scan::roam_directory().is_none() {
        return Err(
            "org-roam: set `org.roam-directory` to the folder your notes live in".to_string(),
        );
    }
    let Some(id) = args.first().filter(|s| !s.trim().is_empty()) else {
        // The ex-command resolves the node at point and passes its id. Reaching
        // here means it did not, which is a wiring fault rather than a user one.
        return Err("org-roam: backlinks needs a node id".to_string());
    };
    let sources = roam_index::backlinks(id);
    Ok(sources
        .iter()
        .filter_map(|source_id| roam_index::node(source_id).map(|n| row(&n)))
        .collect())
}

/// One linking node's row: its title, with its file as context.
fn row(node: &Node) -> (RawCandidate, RoutingPayload) {
    let mut raw = RawCandidate {
        text: node.title.clone(),
        insert_text: None,
        display: node.title.clone(),
        source: Some(BACKLINKS_PICKER.to_string()),
        kind: CandidateKind::Plain,
        data: CandidateData::Plain,
        annotations: Vec::new(),
    };
    // The file basename beside the title, so two notes with similar titles are
    // still tellable apart. Shown, not matched — the title is what you search.
    raw.annotations.push(Annotation::Custom(AnnotationCustom {
        text: file_label(&node.file),
        slot: "comment".to_string(),
    }));
    (
        raw,
        RoutingPayload::FileLocation(Location {
            path: node.file.clone(),
            line: node.line,
            col: 0,
        }),
    )
}

/// The basename of `path`, for the row's context column.
fn file_label(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Resolve a chosen row — jump to the linking node, file AND line, so a
/// headline node lands on its headline.
pub fn accept(routing: RoutingPayload) -> Result<PickerAcceptOutcome, String> {
    match routing {
        RoutingPayload::FileLocation(loc) => Ok(PickerAcceptOutcome::JumpToLocation(loc)),
        _ => Err("org-roam: backlinks got a routing token it did not emit".to_string()),
    }
}

/// The id of the innermost node containing `cursor_line`, or `None` when the
/// cursor is not inside a node.
///
/// Walks UP: the nearest headline at or above the cursor whose drawer carries
/// an `:ID:` wins, and failing that the FILE's own drawer. That order is what
/// makes a file with both a file id and headline ids answer "the entry I am
/// in" rather than "the file I am in" — 19% of the reference corpus is headline
/// nodes, and attributing their backlinks to the file would silently merge
/// several notes' answers into one.
///
/// **A headline without an `:ID:` is not a node**, so the walk continues to its
/// ANCESTORS — and ancestry is by outline level, not by "the previous headline
/// in the file".
///
/// That distinction is the whole of this function's difficulty. Walking up
/// line-by-line and taking the first identified headline finds the previous
/// SIBLING, whose subtree the cursor is not in at all:
///
/// ```org
/// * First          :ID: HEAD-1
/// body
/// * Second         (no id)
/// body  ← the cursor here belongs to the FILE's node, not to HEAD-1
/// ```
///
/// So the walk tracks the deepest level it is still allowed to accept: passing
/// an unidentified headline of level L means only its ancestors — level < L —
/// can answer. `an_unidentified_headline_falls_back_to_its_file` failed against
/// exactly the sibling bug before this existed.
pub fn node_id_at(
    line: &dyn Fn(u32) -> Option<String>,
    line_count: u32,
    cursor_line: u32,
) -> Option<String> {
    let mut n = cursor_line.min(line_count.saturating_sub(1));
    // `u32::MAX` = "any level", before the first headline has been seen.
    let mut max_level = u32::MAX;
    loop {
        let text = line(n).unwrap_or_default();
        if let Some(level) = headline_level(&text) {
            if level < max_level {
                if let Some(id) = drawer_id(line, n + 1) {
                    return Some(id);
                }
                // Unidentified: only its ancestors may answer from here.
                max_level = level;
            }
        }
        if n == 0 {
            break;
        }
        n -= 1;
    }
    // No identified ancestor headline: the file's own drawer, which org writes
    // before the first headline.
    drawer_id(line, 0)
}

/// The outline level of a headline line (`** Foo` → 2), or `None` when the line
/// is not a headline.
///
/// A run of `*` must be followed by a space — `**bold**` at column 0 is body
/// text, and treating it as a level-2 headline would silently reparent
/// everything under it.
fn headline_level(text: &str) -> Option<u32> {
    let stars = text.chars().take_while(|c| *c == '*').count();
    if stars == 0 {
        return None;
    }
    match text.chars().nth(stars) {
        Some(' ') => Some(stars as u32),
        _ => None,
    }
}

/// The `:ID:` of the property drawer starting at `first`, if there is one.
///
/// Returns `None` when `first` does not open a drawer, so a caller can use it
/// as "does this headline have an id" without checking twice.
fn drawer_id(line: &dyn Fn(u32) -> Option<String>, first: u32) -> Option<String> {
    if !line(first)?.trim().eq_ignore_ascii_case(":properties:") {
        return None;
    }
    let mut i = first + 1;
    while let Some(text) = line(i) {
        let trimmed = text.trim();
        if trimmed.eq_ignore_ascii_case(":end:") || trimmed.starts_with('*') {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix(':') {
            if let Some((name, value)) = rest.split_once(':') {
                if name.trim().eq_ignore_ascii_case("id") {
                    let value = value.trim();
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> (impl Fn(u32) -> Option<String> + '_, u32) {
        let owned: Vec<String> = text.lines().map(str::to_string).collect();
        let n = owned.len() as u32;
        (move |i: u32| owned.get(i as usize).cloned(), n)
    }

    const FILE_AND_HEADLINE: &str = ":PROPERTIES:\n\
         :ID:       FILE-1\n\
         :END:\n\
         #+title: A file\n\
         \n\
         * First\n\
         :PROPERTIES:\n\
         :ID:       HEAD-1\n\
         :END:\n\
         body of first\n\
         * Second, unidentified\n\
         body of second\n";

    #[test]
    fn the_cursor_inside_a_headline_node_gets_the_headline_id() {
        let (l, n) = lines(FILE_AND_HEADLINE);
        assert_eq!(node_id_at(&l, n, 9).as_deref(), Some("HEAD-1"));
    }

    #[test]
    fn the_cursor_above_the_first_headline_gets_the_file_id() {
        let (l, n) = lines(FILE_AND_HEADLINE);
        assert_eq!(node_id_at(&l, n, 3).as_deref(), Some("FILE-1"));
    }

    /// A headline WITHOUT an id is not a node — the walk continues to the file
    /// rather than reporting "not in a node".
    #[test]
    fn an_unidentified_headline_falls_back_to_its_file() {
        let (l, n) = lines(FILE_AND_HEADLINE);
        assert_eq!(node_id_at(&l, n, 11).as_deref(), Some("FILE-1"));
    }

    /// A CHILD of an identified headline belongs to it — ancestry, not
    /// proximity. The sibling case above and this one are the two halves of
    /// the same rule and they pull in opposite directions.
    #[test]
    fn a_sub_headline_belongs_to_its_identified_ancestor() {
        let (l, n) = lines(
            "* Parent\n             :PROPERTIES:\n             :ID:       PARENT-1\n             :END:\n             ** Child, unidentified\n             body of child\n",
        );
        assert_eq!(node_id_at(&l, n, 5).as_deref(), Some("PARENT-1"));
    }

    /// `**bold**` at column 0 is body text, not a level-2 headline. Reading it
    /// as one would reparent everything below it.
    #[test]
    fn a_bold_run_at_column_zero_is_not_a_headline() {
        let (l, n) = lines(
            "* Topic\n             :PROPERTIES:\n             :ID:       TOP-1\n             :END:\n             **bold** text\n             more\n",
        );
        assert_eq!(node_id_at(&l, n, 5).as_deref(), Some("TOP-1"));
    }

    #[test]
    fn a_file_with_no_ids_at_all_is_not_a_node() {
        let (l, n) = lines("#+title: Plain\n\n* A headline\nbody\n");
        assert_eq!(node_id_at(&l, n, 3), None);
    }

    #[test]
    fn an_id_is_read_whatever_the_drawers_case() {
        let (l, n) = lines("* Topic\n:properties:\n:id:       lower\n:end:\n");
        assert_eq!(node_id_at(&l, n, 0).as_deref(), Some("lower"));
    }

    /// An `:ID:` with no value is not an id — treating `":ID:"` as one would
    /// send the picker looking up the empty key.
    #[test]
    fn an_empty_id_property_is_not_an_id() {
        let (l, n) = lines("* Topic\n:PROPERTIES:\n:ID:\n:END:\n");
        assert_eq!(node_id_at(&l, n, 0), None);
    }

    #[test]
    fn a_row_shows_the_title_and_annotates_the_file() {
        let node = Node {
            id: "X".into(),
            title: "Linking note".into(),
            aliases: vec![],
            tags: vec![],
            refs: vec![],
            file: "/notes/20250603-linking.org".into(),
            line: 12,
            level: 1,
        };
        let (raw, routing) = row(&node);
        assert_eq!(raw.display, "Linking note");
        assert_eq!(raw.annotations.len(), 1);
        match routing {
            RoutingPayload::FileLocation(loc) => {
                assert_eq!(loc.line, 12, "jumps to the node's own line");
                assert_eq!(loc.path, "/notes/20250603-linking.org");
            }
            other => panic!("expected a file location, got {other:?}"),
        }
    }

    #[test]
    fn the_file_label_is_the_basename() {
        assert_eq!(file_label("/a/b/c.org"), "c.org");
        assert_eq!(file_label("c.org"), "c.org");
    }
}

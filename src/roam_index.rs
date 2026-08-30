//! OR.4 — the index: one writer, four key families.
//!
//! Design: `docs/dev/architecture/org-roam.md` §4 in the lattice tree.
//!
//! ## One indexer, many readers
//!
//! **The async event seam is the sole writer.** Every other seam — picker,
//! grammar, multibuffer — only reads. That is what makes the records safe to
//! denormalize: the same node appears in the all-nodes blob, under its own key,
//! and in its targets' backlink lists, and all three are written in one pass by
//! one instance. A multi-writer design would need either a transaction or a
//! reconciliation pass, and would get one of them wrong.
//!
//! It is also the only shape that works. There is no single org guest instance:
//! `spawn_event_plugin`, `spawn_config_plugin` and `instantiate_grammar_plugin`
//! build separate `wasmtime::Store`s with separate memory, so an index kept in
//! guest state would be N copies drifting — with find-node offering a note
//! `<CR>` cannot open, and nothing reporting an error because each instance
//! stays internally consistent. The store is host-side precisely so there is
//! one of it.
//!
//! ## The keys
//!
//! | Key | Value | Read by |
//! |---|---|---|
//! | `nodes` | every node record, one blob | the picker, once per open |
//! | `n/<id>` | one node record | `<CR>`, one exact lookup |
//! | `b/<id>` | ids linking *to* `<id>` | the backlinks view |
//! | `f/<path>` | that file's content hash + what it produced | the indexer |
//!
//! `n/<id>` exists separately from `nodes` because `<CR>` is on the keystroke
//! path and must not deserialize a 90 KB blob to answer one question. The
//! measured cost of the lookup it enables is 76.6 ns.
//!
//! **`f/<path>` is what makes deletion work**, and it is the row a cache-shaped
//! design forgets. It holds the ids that file produced *and their outgoing
//! links*, so when the file changes the indexer knows exactly which node
//! records to retract and which backlink entries to withdraw. Without it a node
//! deleted from a file stays in the index forever and the picker offers a
//! destination that does not exist.
//!
//! ## Ids are compared case-insensitively
//!
//! The **key** is lowercased; the **value** keeps the id verbatim. Org is not
//! consistent about case across platforms, and a link that fails to resolve
//! over letter case looks exactly like a missing note — the worst way for this
//! to fail, because it is indistinguishable from data loss.
//!
//! ## Inert until configured
//!
//! With `org.roam-directory` unset there is no walk, no watcher and no store
//! write. An org user who keeps no zettelkasten pays nothing for this existing.

use serde::{Deserialize, Serialize};

use crate::lattice::plugin_host::host_services;
use crate::roam::{self, Node};

/// The store key holding every node record, as one blob.
const NODES_KEY: &str = "nodes";
/// Key prefixes. Short on purpose — they are repeated once per node.
const NODE_PREFIX: &str = "n/";
const BACKLINK_PREFIX: &str = "b/";
const FILE_PREFIX: &str = "f/";

/// What one file contributed, remembered so a later pass can retract it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileRow {
    /// Hash of the text these records were derived from. Content hash, not
    /// mtime: the text is already in hand, so hashing costs microseconds
    /// against the parse it protects, and there is no filesystem clock to lie.
    pub hash: u64,
    /// The nodes this file produced, each with what it linked to.
    pub nodes: Vec<FileNodeRow>,
}

/// One node's identity + outgoing links, as recorded against its file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNodeRow {
    pub id: String,
    pub links: Vec<String>,
}

/// The key a node record lives under. Lowercased — see the module note.
fn node_key(id: &str) -> String {
    format!("{NODE_PREFIX}{}", id.to_lowercase())
}

/// The key a node's backlink list lives under.
fn backlink_key(id: &str) -> String {
    format!("{BACKLINK_PREFIX}{}", id.to_lowercase())
}

/// The key a file's row lives under. NOT lowercased: paths are case-sensitive
/// on the filesystems that matter, and folding them would merge two real files.
fn file_key(path: &str) -> String {
    format!("{FILE_PREFIX}{path}")
}

/// A cheap content fingerprint. Not cryptographic and does not need to be — the
/// input is a file just read, and the cost of a collision is one stale record
/// until the file changes again.
pub fn content_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn encode<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    rmp_serde::to_vec(value).ok()
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<T> {
    rmp_serde::from_slice(bytes).ok()
}

fn get_decoded<T: for<'de> Deserialize<'de>>(key: &str) -> Option<T> {
    host_services::store_get(key).and_then(|b| decode(&b))
}

fn put_encoded<T: Serialize>(key: &str, value: &T) {
    let Some(bytes) = encode(value) else {
        // A record that will not serialise is a bug in the record, not in the
        // store. Skipping it costs one node; failing the batch costs the index.
        return;
    };
    let _ = host_services::store_put(key, &bytes);
}

/// Index one file's already-read text.
///
/// Returns `true` when anything changed, so a caller can decide whether the
/// `nodes` blob needs rebuilding — rebuilding it per file would be O(n²) over a
/// cold index.
///
/// The hash check is the whole point of `f/<path>`: on a warm pass every file
/// whose bytes are unchanged costs one `get` and one comparison, and the parse
/// — the expensive part, 1–2 ms — never happens.
pub fn index_file(path: &str, text: &str, extracted: &[roam::Extracted]) -> bool {
    let hash = content_hash(text);
    let previous: Option<FileRow> = get_decoded(&file_key(path));
    if previous.as_ref().is_some_and(|row| row.hash == hash) {
        return false;
    }

    // Retract what this file used to say BEFORE writing what it says now. The
    // order matters when a node moved between files or lost its id: retracting
    // afterwards would delete the record just written.
    if let Some(previous) = previous {
        retract(&previous);
    }

    let row = FileRow {
        hash,
        nodes: extracted
            .iter()
            .map(|e| FileNodeRow {
                id: e.node.id.clone(),
                links: e.links.clone(),
            })
            .collect(),
    };
    for e in extracted {
        put_encoded(&node_key(&e.node.id), &e.node);
        for target in &e.links {
            add_backlink(target, &e.node.id);
        }
    }
    put_encoded(&file_key(path), &row);
    true
}

/// Forget everything a file's previous contents contributed.
///
/// Both halves are needed and the second is the one that gets forgotten: the
/// node records go, AND every backlink entry those nodes created is withdrawn
/// from its target. Skipping the second leaves a backlinks view citing a note
/// that no longer links there.
fn retract(previous: &FileRow) {
    for node in &previous.nodes {
        let _ = host_services::store_delete(&node_key(&node.id));
        for target in &node.links {
            remove_backlink(target, &node.id);
        }
    }
}

/// Record that `source` links to `target`.
fn add_backlink(target: &str, source: &str) {
    let key = backlink_key(target);
    let mut list: Vec<String> = get_decoded(&key).unwrap_or_default();
    if list.iter().any(|s| s.eq_ignore_ascii_case(source)) {
        return;
    }
    list.push(source.to_string());
    put_encoded(&key, &list);
}

/// Withdraw `source`'s link to `target`.
fn remove_backlink(target: &str, source: &str) {
    let key = backlink_key(target);
    let Some(mut list): Option<Vec<String>> = get_decoded(&key) else {
        return;
    };
    list.retain(|s| !s.eq_ignore_ascii_case(source));
    if list.is_empty() {
        // An empty list is indistinguishable from no list to every reader, so
        // deleting it keeps the store from accumulating one key per node that
        // was ever linked to and then unlinked.
        let _ = host_services::store_delete(&key);
    } else {
        put_encoded(&key, &list);
    }
}

/// Every node, for a reader that wants the whole set (OR.6's picker).
///
/// One `get` and one deserialize rather than one call per node — which is the
/// entire reason the blob exists beside the per-id records.
pub fn all_nodes() -> Vec<Node> {
    get_decoded::<Vec<Node>>(NODES_KEY).unwrap_or_default()
}

/// One node by its id, or `None` when the index does not hold it.
///
/// OR.8: **this is the keystroke-path lookup** — `<CR>` on an `[[id:…]]` link
/// calls it. One exact-key `get` and one decode, never a walk of `nodes`:
/// deserialising a 90 KB blob to answer one question is not a thing to do while
/// someone is holding a key down, and it is the whole reason §4.2 keeps
/// `n/<id>` as a separate key beside the blob.
///
/// Case-insensitive, because [`node_key`] lowercases: org ids are written
/// however the tool that minted them felt, and a link that differs from its
/// drawer only in case is the same link to a reader.
pub fn node(id: &str) -> Option<Node> {
    get_decoded::<Node>(&node_key(id))
}

/// Whether the index holds anything at all.
///
/// OR.8 needs this to tell "no such id" from "the index has not been built
/// yet", which are different problems with different fixes — one is a broken
/// link, the other is a `:org-roam-sync` away. A message that conflated them
/// would send the reader looking in the wrong place.
pub fn is_empty() -> bool {
    host_services::store_keys(NODE_PREFIX).is_empty()
}

/// Rebuild the all-nodes blob from the `n/<id>` records.
///
/// Called **once per batch**, not once per file: it is O(nodes) in host calls,
/// so doing it per file would make a cold index quadratic. That is why
/// [`index_file`] reports whether it changed anything rather than rebuilding
/// itself.
pub fn rebuild_nodes_blob() {
    let keys = host_services::store_keys(NODE_PREFIX);
    let mut nodes: Vec<Node> = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(node) = get_decoded::<Node>(&key) {
            nodes.push(node);
        }
    }
    put_encoded(NODES_KEY, &nodes);
}

/// What the store last recorded for `path`.
pub fn file_row(path: &str) -> Option<FileRow> {
    get_decoded(&file_key(path))
}

/// Forget a file entirely — the deletion path the watcher takes when a path it
/// reported no longer exists.
pub fn forget_file(path: &str) -> bool {
    let Some(previous) = file_row(path) else {
        return false;
    };
    retract(&previous);
    let _ = host_services::store_delete(&file_key(path));
    true
}

/// OR.8 — where an `:ID:` drawer goes for the headline on `headline_line`, and
/// whether one is needed at all.
///
/// `None` means the headline already carries an `:ID:` and nothing should be
/// written. That is the no-op case rather than an error: running
/// `:org-roam-id-create` twice is something a user does, and answering it with
/// a second drawer would produce a file org itself cannot read.
///
/// `Some((line, text))` is the line to insert `text` before — always
/// `headline_line + 1`, because org requires the drawer to be the first thing
/// under its headline. An existing drawer WITHOUT an `:ID:` is extended in
/// place instead, by inserting the `:ID:` line just inside its opener; that is
/// why the answer is a line rather than a fixed offset.
///
/// `line` is the file read through an accessor rather than a materialised
/// `Vec`, matching `headline.rs`'s shape: a drawer is a handful of lines under
/// its headline, and collecting the whole file to look at four of them would
/// scale the cost with the document instead of with the drawer.
pub fn id_drawer_insert(
    line: &dyn Fn(u32) -> Option<String>,
    headline_line: u32,
    id: &str,
) -> Option<(u32, String)> {
    let first = headline_line + 1;
    let opens_drawer = line(first).is_some_and(|l| l.trim().eq_ignore_ascii_case(":properties:"));

    if !opens_drawer {
        // No drawer at all: write a whole one.
        return Some((first, format!(":PROPERTIES:\n:ID:       {id}\n:END:\n")));
    }

    // A drawer exists. Walk to its `:END:`, and stop at a new headline in case
    // the drawer was never closed — an unterminated drawer is malformed, and
    // scanning past the next headline would attach this id to the wrong entry.
    let mut i = first + 1;
    while let Some(text) = line(i) {
        let trimmed = text.trim();
        if trimmed.eq_ignore_ascii_case(":end:") {
            break;
        }
        if trimmed.starts_with('*') {
            break;
        }
        if trimmed
            .strip_prefix(':')
            .and_then(|rest| rest.split_once(':'))
            .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("id"))
        {
            // Already a node.
            return None;
        }
        i += 1;
    }
    // Extend the existing drawer rather than opening a second one.
    Some((first + 1, format!(":ID:       {id}\n")))
}

#[cfg(test)]
mod id_create_tests {
    use super::*;

    /// The file as the accessor the production caller passes.
    fn lines(text: &str) -> impl Fn(u32) -> Option<String> + '_ {
        let owned: Vec<String> = text.lines().map(str::to_string).collect();
        move |n: u32| owned.get(n as usize).cloned()
    }

    #[test]
    fn a_headline_with_no_drawer_gets_a_whole_one() {
        let l = lines("* Topic\nbody\n");
        let (at, text) = id_drawer_insert(&l, 0, "ABC").expect("needs an id");
        assert_eq!(at, 1);
        assert_eq!(text, ":PROPERTIES:\n:ID:       ABC\n:END:\n");
    }

    #[test]
    fn a_headline_that_already_has_an_id_is_a_no_op() {
        // The second-drawer case: answering this with an insert would produce
        // a file org cannot read.
        let l = lines("* Topic\n:PROPERTIES:\n:ID:       OLD\n:END:\nbody\n");
        assert_eq!(id_drawer_insert(&l, 0, "NEW"), None);
    }

    #[test]
    fn an_existing_id_is_recognised_whatever_its_case() {
        let l = lines("* Topic\n:properties:\n:id:       old\n:end:\n");
        assert_eq!(id_drawer_insert(&l, 0, "NEW"), None);
    }

    #[test]
    fn a_drawer_without_an_id_is_extended_not_replaced() {
        let l = lines("* Topic\n:PROPERTIES:\n:CATEGORY: work\n:END:\n");
        let (at, text) = id_drawer_insert(&l, 0, "ABC").expect("needs an id");
        assert_eq!(at, 2, "just inside the opener, above the existing property");
        assert_eq!(text, ":ID:       ABC\n");
    }

    #[test]
    fn an_unterminated_drawer_stops_at_the_next_headline() {
        // Malformed input: scanning past the headline would read the NEXT
        // entry's id and wrongly call this one already-identified.
        let l = lines("* One\n:PROPERTIES:\n* Two\n:PROPERTIES:\n:ID:       OTHER\n:END:\n");
        assert!(
            id_drawer_insert(&l, 0, "ABC").is_some(),
            "the first headline still needs its own id"
        );
    }

    #[test]
    fn a_headline_at_the_end_of_the_file_still_gets_a_drawer() {
        let l = lines("* Topic");
        let (at, _) = id_drawer_insert(&l, 0, "ABC").expect("needs an id");
        assert_eq!(at, 1);
    }
}

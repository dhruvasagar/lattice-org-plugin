//! CD.4: capture drafts — every capture is a file, its state is in the store.
//!
//! Design: `lattice/docs/dev/architecture/org-capture-drafts.md` §§2, 4, 5, 12.
//!
//! A capture used to be one synthetic buffer at a time, its destination held in
//! a guest `thread_local`. Now each capture is a real file,
//! `{drafts}/{id}.org`, and what it files into is recorded under
//! `capture/{id}` in the plugin store. Three things follow from that without
//! being built: `:w` saves a draft, the file and the store entry both survive a
//! restart, and "which capture is this?" is answered by `document.path()`.
//!
//! This module is the pure half — identity, paths and the encoding — so every
//! rule is testable without a host. The host calls live in `lib.rs`.

use serde::{Deserialize, Serialize};

/// The store prefix every capture's state lives under.
pub const STATE_PREFIX: &str = "capture/";

/// Where drafts go when no directory is configured.
///
/// **A path that cannot be written, on purpose.** The capture still opens and
/// can still be committed — its text is filed from the buffer, not from the
/// file — but `:w` fails, and the error names this path, which names the fix.
/// The alternative, a synthetic buffer, would have no path to identify the
/// capture by; a writable fallback such as the temp directory would save drafts
/// somewhere the user never chose and would never find again.
pub const UNSET_DRAFTS_DIR: &str = "/set-org.directory-to-save-capture-drafts";

/// The store key for capture `id`.
pub fn state_key(id: &str) -> String {
    format!("{STATE_PREFIX}{id}")
}

/// A capture id: six lowercase hex digits.
///
/// Six is enough for session-scoped uniqueness with a counter behind it, and
/// short enough to read in a path. Collisions are not assumed away: the caller
/// re-rolls while an id is live (store entry or file present).
pub fn capture_id(prefix: &str, key: &str, title: &str, counter: u64, salt: i64) -> String {
    // FNV-1a over the parts, separated so ("ab","c") and ("a","bc") differ.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    feed(prefix.as_bytes());
    feed(key.as_bytes());
    feed(title.as_bytes());
    feed(&counter.to_le_bytes());
    feed(&salt.to_le_bytes());
    format!("{:06x}", hash & 0x00ff_ffff)
}

/// Is `s` a capture id?
pub fn is_capture_id(s: &str) -> bool {
    s.len() == 6
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The capture a buffer belongs to, from its path: the basename, when it is a
/// capture id with an `.org` extension. The store entry is what confirms it.
pub fn id_from_path(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let stem = name.strip_suffix(".org")?;
    is_capture_id(stem).then(|| stem.to_string())
}

/// The drafts directory, from the options — `None` when none is configured.
///
/// `org.capture-drafts-directory` wins. Otherwise `{org.directory}/captures`.
/// Otherwise `captures/` beside `org.capture-file`, which is the directory a
/// user who configured only a capture file has told us about. A `captures`
/// SUBdirectory in both fallbacks, because the agenda scans a configured
/// directory one level deep (OA.0d): drafts beside real org files would show up
/// in `gr`, a level down they do not.
pub fn drafts_dir(explicit: &str, org_directory: &str, capture_file: &str) -> Option<String> {
    let trim = |s: &str| s.trim().trim_end_matches('/').to_string();
    if !explicit.trim().is_empty() {
        return Some(trim(explicit));
    }
    if !org_directory.trim().is_empty() {
        return Some(format!("{}/captures", trim(org_directory)));
    }
    let file = capture_file.trim();
    let (dir, _) = file.rsplit_once('/')?;
    if dir.is_empty() {
        return None;
    }
    Some(format!("{dir}/captures"))
}

/// The draft file for `id` in `dir`.
pub fn draft_path(dir: &str, id: &str) -> String {
    format!("{dir}/{id}.org")
}

/// Is `path` a draft under `dir`? A path test, not carried state (design §8).
pub fn is_under(path: &str, dir: &str) -> bool {
    let dir = dir.trim_end_matches('/');
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// A range in the caller's buffer — `Range`'s shape, owned and serializable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start_line: u32,
    pub start_byte: u32,
    pub end_line: u32,
    pub end_byte: u32,
}

impl Span {
    /// A collapsed span: an insertion point.
    pub fn at(line: u32, byte: u32) -> Self {
        Self {
            start_line: line,
            start_byte: byte,
            end_line: line,
            end_byte: byte,
        }
    }
}

/// Where a capture was fired from, and what to write there when it commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    /// Valid this session: `focus-buffer`'s argument, `apply-edit`'s target.
    pub buffer: u32,
    /// Survives a restart, when the caller had a path.
    pub path: Option<String>,
    /// Collapsed = an insertion point; non-empty = a region to replace.
    pub at: Span,
    /// `Some` only when committing writes something back (a roam link).
    pub on_commit: Option<String>,
}

/// Encode a value for the store.
pub fn encode<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    rmp_serde::to_vec_named(value).ok()
}

/// Decode a value from the store. `None` for bytes this build cannot read — an
/// entry written by an older, incompatible build is treated as absent rather
/// than as a crash.
pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<T> {
    rmp_serde::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_six_lowercase_hex_digits() {
        let id = capture_id("org-capture", "t", "", 0, 0);
        assert!(is_capture_id(&id), "{id}");
    }

    /// The counter alone separates two captures of the same template and
    /// title — the case simultaneity is for.
    #[test]
    fn the_counter_separates_identical_captures() {
        let a = capture_id("org-capture", "t", "", 0, 7);
        let b = capture_id("org-capture", "t", "", 1, 7);
        assert_ne!(a, b);
    }

    #[test]
    fn every_part_feeds_the_id() {
        let base = capture_id("org-capture", "t", "x", 0, 0);
        assert_ne!(base, capture_id("org-roam-capture", "t", "x", 0, 0));
        assert_ne!(base, capture_id("org-capture", "n", "x", 0, 0));
        assert_ne!(base, capture_id("org-capture", "t", "y", 0, 0));
        assert_ne!(base, capture_id("org-capture", "t", "x", 0, 1));
    }

    #[test]
    fn an_id_is_read_back_from_a_draft_path() {
        assert_eq!(
            id_from_path("/home/u/org/captures/a3f9c1.org").as_deref(),
            Some("a3f9c1")
        );
        assert_eq!(id_from_path("/home/u/org/inbox.org"), None);
        assert_eq!(id_from_path("/home/u/org/captures/A3F9C1.org"), None);
        assert_eq!(id_from_path("/home/u/org/captures/a3f9c1.txt"), None);
    }

    #[test]
    fn the_drafts_directory_resolves_in_order() {
        assert_eq!(
            drafts_dir("/d/", "/org", "/org/inbox.org").as_deref(),
            Some("/d")
        );
        assert_eq!(
            drafts_dir("", "/org/", "/x/inbox.org").as_deref(),
            Some("/org/captures")
        );
        assert_eq!(
            drafts_dir("", "", "/notes/inbox.org").as_deref(),
            Some("/notes/captures")
        );
        assert_eq!(drafts_dir("", "", ""), None);
        assert_eq!(drafts_dir("", "", "inbox.org"), None);
    }

    #[test]
    fn a_draft_is_under_its_directory_and_nothing_else_is() {
        assert!(is_under("/org/captures/a3f9c1.org", "/org/captures"));
        assert!(is_under("/org/captures/a3f9c1.org", "/org/captures/"));
        assert!(!is_under("/org/captures-old/a3f9c1.org", "/org/captures"));
        assert!(!is_under("/org/inbox.org", "/org/captures"));
    }

    #[test]
    fn a_caller_round_trips_through_the_store_encoding() {
        let caller = Caller {
            buffer: 7,
            path: Some("/org/inbox.org".into()),
            at: Span::at(3, 4),
            on_commit: None,
        };
        let back: Caller = decode(&encode(&caller).unwrap()).unwrap();
        assert_eq!(back, caller);
    }

    #[test]
    fn unreadable_bytes_decode_to_nothing() {
        assert_eq!(decode::<Caller>(b"not msgpack"), None);
    }
}

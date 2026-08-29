//! OM/IM.7 — finding org's inline image links.
//!
//! An org file references an image as a link on its own line:
//!
//! ```org
//! [[file:img/diagram.png]]
//! [[file:diagram.png][a wiring diagram]]
//! ```
//!
//! ## Why only a link ALONE on its line becomes an image
//!
//! A block occupies whole display rows, so it can only hang below a line — it
//! cannot sit inside one. A link with text around it (`see [[file:a.png]] for
//! detail`) would have its image appear on the following row, detached from the
//! sentence that introduced it, which reads worse than leaving the link alone.
//!
//! Emacs draws the same line for the same reason: `org-display-inline-images`
//! replaces the link's own text, and there is nothing to replace when the link
//! is embedded in a paragraph that must keep flowing.
//!
//! ## Why not the parse tree
//!
//! Same argument `headline.rs` makes — the tree can be absent (a pending
//! parse), and this runs on a producer trigger where an absent tree would
//! silently yield no images until something re-triggered it. Matching text has
//! no such state.

/// One image link found in a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLink {
    /// 0-based line the link occupies, which is the line the block hangs below.
    pub line: u32,
    /// The path exactly as written; the HOST resolves it against the buffer's
    /// directory.
    pub path: String,
    /// The link's description, when it has one — org's `[[path][description]]`.
    /// It becomes the block's alt text, which is what a terminal shows.
    pub description: Option<String>,
}

/// Extensions treated as images. Anything else is a link, not a picture.
///
/// An allow-list rather than "try to decode and see": a `[[file:notes.org]]`
/// link must not become a failed image block with a placeholder box, and
/// probing every linked file to find out would read files the user never asked
/// about.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];

/// Parse an org link on `line`, if the line is nothing but an image link.
pub fn image_link(line: &str, line_no: u32) -> Option<ImageLink> {
    let t = line.trim();
    let inner = t.strip_prefix("[[")?.strip_suffix("]]")?;
    // `[[path][description]]` — split on the FIRST `][`, so a description
    // containing brackets survives.
    let (path, description) = match inner.find("][") {
        Some(i) => (&inner[..i], Some(inner[i + 2..].to_string())),
        None => (inner, None),
    };
    // `file:` is org's explicit prefix; a bare relative path is also a file
    // link. Any other scheme (`http:`, `id:`, `mailto:`) is not ours.
    let path = match path.strip_prefix("file:") {
        Some(p) => p,
        None if !has_scheme(path) => path,
        None => return None,
    };
    if path.trim().is_empty() || !is_image(path) {
        return None;
    }
    Some(ImageLink {
        line: line_no,
        path: path.to_string(),
        description: description.filter(|d| !d.trim().is_empty()),
    })
}

/// True when `s` starts with a `scheme:` that is not a bare Windows-style
/// drive letter or a relative path.
fn has_scheme(s: &str) -> bool {
    match s.find(':') {
        // A single leading character before `:` is a drive letter, not a
        // scheme — `c:/img/a.png` is a path.
        Some(i) if i > 1 => s[..i]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+'),
        _ => false,
    }
}

fn is_image(path: &str) -> bool {
    let ext = path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    // Reject `foo` (no dot at all): `rsplit` yields the whole string.
    path.contains('.') && IMAGE_EXTENSIONS.contains(&ext.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_file_link_alone_on_its_line_is_an_image() {
        let l = image_link("[[file:img/diagram.png]]", 4).expect("is an image");
        assert_eq!(l.line, 4);
        assert_eq!(l.path, "img/diagram.png");
        assert_eq!(l.description, None);
    }

    /// Org's `[[path][description]]` — the description becomes the alt text,
    /// which is the whole of what a terminal shows.
    #[test]
    fn a_description_becomes_the_alt_text() {
        let l = image_link("[[file:a.png][a wiring diagram]]", 0).unwrap();
        assert_eq!(l.path, "a.png");
        assert_eq!(l.description.as_deref(), Some("a wiring diagram"));
    }

    /// A block occupies whole rows, so it can only hang BELOW a line. A link
    /// embedded in a sentence would put its image on the next row, detached
    /// from the text that introduced it.
    #[test]
    fn a_link_with_text_around_it_is_left_as_a_link() {
        assert!(image_link("see [[file:a.png]] for detail", 0).is_none());
        assert!(image_link("[[file:a.png]] and more", 0).is_none());
    }

    /// Leading whitespace is fine — an image under a headline is indented in
    /// plenty of org files.
    #[test]
    fn indentation_does_not_disqualify_a_link() {
        assert!(image_link("   [[file:a.png]]  ", 0).is_some());
    }

    /// An allow-list, not "try to decode and see". `[[file:notes.org]]` must
    /// not become a failed image block, and probing to find out would read
    /// files the user never asked about.
    #[test]
    fn only_image_extensions_qualify() {
        for ok in ["a.png", "a.JPG", "a.jpeg", "a.gif", "a.webp", "a.svg"] {
            assert!(
                image_link(&format!("[[file:{ok}]]"), 0).is_some(),
                "{ok} should qualify"
            );
        }
        for no in ["notes.org", "a.txt", "a", "a.png.bak", "archive.tar.gz"] {
            assert!(
                image_link(&format!("[[file:{no}]]"), 0).is_none(),
                "{no} should not"
            );
        }
    }

    /// Other schemes are links, not pictures — following an http image would
    /// mean network I/O the user did not ask for.
    #[test]
    fn non_file_schemes_are_not_images() {
        assert!(image_link("[[https://example.com/a.png]]", 0).is_none());
        assert!(image_link("[[id:abcd-1234]]", 0).is_none());
        assert!(image_link("[[mailto:a@b.com]]", 0).is_none());
    }

    /// A bare relative path with no `file:` prefix is still a file link in
    /// org, and a Windows drive letter is a path rather than a scheme.
    #[test]
    fn a_bare_path_is_a_file_link() {
        assert_eq!(image_link("[[img/a.png]]", 0).unwrap().path, "img/a.png");
        assert_eq!(
            image_link("[[c:/img/a.png]]", 0).unwrap().path,
            "c:/img/a.png"
        );
    }

    #[test]
    fn malformed_links_are_refused_rather_than_guessed() {
        for bad in ["[[file:]]", "[[]]", "[[file:a.png", "file:a.png]]", ""] {
            assert!(image_link(bad, 0).is_none(), "{bad:?}");
        }
    }
}

// ── OM.10: opening a link at point ──

/// What a link points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A file on disk, path as written (the host resolves it).
    File(String),
    /// An external URI — `http:`, `https:`, `mailto:` and friends.
    Uri(String),
    /// An internal `*Headline` reference, resolved by searching this buffer.
    Headline(String),
    /// OL.1: `[[id:6F398E54-…]]` — an org-id reference.
    ///
    /// **Recognised here, resolvable nowhere yet.** An `:ID:` is a key
    /// into a corpus, and finding the file that holds it needs an index
    /// — `org-roam.md`'s subject, not this module's.
    ///
    /// It is a variant rather than an omission because the status quo
    /// was actively misleading: with no `id:` arm, `classify` fell
    /// through to the file branch below and opening the link reported
    /// *"no such file: id:6F398E54-…"*, which blames the filesystem for
    /// a missing index and sends the reader after a file that was never
    /// meant to exist. Recognising the kind and failing on it honestly
    /// is strictly better than resolving it wrongly.
    Id(String),
}

/// A link found under the cursor, with its byte span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub start: usize,
    pub end: usize,
    pub target: Target,
}

/// The link containing `byte`, if the cursor is inside one.
///
/// Unlike [`image_link`], this does NOT require the link to be alone on its
/// line — opening a link in the middle of a sentence is exactly what the key
/// is for.
pub fn link_at(line: &str, byte: usize) -> Option<Link> {
    let mut i = 0;
    while let Some(open) = line[i..].find("[[").map(|o| i + o) {
        let Some(close) = line[open..].find("]]").map(|o| open + o + 2) else {
            break;
        };
        if byte >= open && byte < close {
            let inner = &line[open + 2..close - 2];
            let path = inner.split_once("][").map(|(p, _)| p).unwrap_or(inner);
            return classify(path).map(|target| Link {
                start: open,
                end: close,
                target,
            });
        }
        i = close;
    }
    None
}

fn classify(path: &str) -> Option<Target> {
    let p = path.trim();
    if p.is_empty() {
        return None;
    }
    if let Some(h) = p.strip_prefix('*') {
        // `[[*Headline]]` — an internal reference.
        let h = h.trim();
        return (!h.is_empty()).then(|| Target::Headline(h.to_string()));
    }
    if let Some(f) = p.strip_prefix("file:") {
        return (!f.trim().is_empty()).then(|| Target::File(f.to_string()));
    }
    // OL.1: BEFORE `has_scheme`, which would otherwise classify `id:` as
    // a generic URI and hand it to the platform opener.
    if let Some(id) = p.strip_prefix("id:") {
        let id = id.trim();
        return (!id.is_empty()).then(|| Target::Id(id.to_string()));
    }
    if has_scheme(p) {
        return Some(Target::Uri(p.to_string()));
    }
    Some(Target::File(p.to_string()))
}

/// The 0-based line of the headline whose title matches `title`.
///
/// Compares the TITLE — stars, and any TODO keyword or priority, are stripped
/// by the caller's parse. Case-sensitive and exact, like org's own internal
/// links: a fuzzy match that jumped to the wrong heading would be worse than
/// not jumping.
pub fn find_headline(
    line: impl Fn(u32) -> Option<String>,
    line_count: u32,
    title: &str,
    keywords: &[String],
) -> Option<u32> {
    (0..line_count).find(|i| {
        line(*i)
            .and_then(|t| crate::todo::parse(&t, keywords).map(|h| h.title == title))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn classifies_the_three_kinds() {
        assert_eq!(
            link_at("[[file:a.org]]", 3).unwrap().target,
            Target::File("a.org".into())
        );
        assert_eq!(
            link_at("[[https://example.com]]", 5).unwrap().target,
            Target::Uri("https://example.com".into())
        );
        assert_eq!(
            link_at("[[*Some Heading]]", 5).unwrap().target,
            Target::Headline("Some Heading".into())
        );
        // A bare path is a file link, as in org.
        assert_eq!(
            link_at("[[notes/a.org]]", 4).unwrap().target,
            Target::File("notes/a.org".into())
        );
    }

    /// Opening a link mid-sentence is exactly what the key is for — unlike an
    /// image, which must be alone on its line to become a block.
    #[test]
    fn a_link_inside_a_sentence_still_opens() {
        let l = "see [[file:a.org][the notes]] for detail";
        assert!(link_at(l, 10).is_some());
        // Outside the link's span, there is nothing to open.
        assert!(link_at(l, 0).is_none());
        assert!(link_at(l, 35).is_none());
    }

    #[test]
    fn the_description_is_ignored_when_resolving_the_target() {
        let l = "[[file:a.org][a description with ][ brackets]]";
        assert_eq!(link_at(l, 3).unwrap().target, Target::File("a.org".into()));
    }

    #[test]
    fn picks_the_link_the_cursor_is_actually_in() {
        let l = "[[file:a.org]] and [[file:b.org]]";
        assert_eq!(link_at(l, 3).unwrap().target, Target::File("a.org".into()));
        assert_eq!(link_at(l, 22).unwrap().target, Target::File("b.org".into()));
        assert!(link_at(l, 16).is_none(), "between them");
    }

    #[test]
    fn malformed_and_empty_links_are_refused() {
        assert!(link_at("[[]]", 2).is_none());
        assert!(link_at("[[*]]", 2).is_none());
        assert!(link_at("[[file:]]", 3).is_none());
        assert!(link_at("[[id:]]", 3).is_none());
        assert!(link_at("no link", 2).is_none());
    }

    // ---- OL.1: `id:` is a link kind ----

    #[test]
    fn an_id_link_classifies_as_an_id() {
        assert_eq!(
            link_at("[[id:6F398E54-7E63-4492-9EB6-89C8A90E7DD3]]", 5)
                .unwrap()
                .target,
            Target::Id("6F398E54-7E63-4492-9EB6-89C8A90E7DD3".into())
        );
        assert_eq!(
            link_at("see [[id:ABC][Some Note]] here", 10)
                .unwrap()
                .target,
            Target::Id("ABC".into())
        );
    }

    /// The bug OL.1 exists to end: without the `id:` arm this fell
    /// through to `Target::File`, and opening it reported "no such file:
    /// id:6F39…" — blaming the filesystem for a missing index and
    /// sending the reader after a file that was never meant to exist.
    #[test]
    fn an_id_link_is_not_a_file() {
        let t = link_at("[[id:6F39]]", 5).unwrap().target;
        assert!(
            !matches!(t, Target::File(_)),
            "an id must never reach the file branch: {t:?}"
        );
    }

    /// `id:` is checked BEFORE the generic scheme test, or the platform
    /// URI opener would be handed `id:6F39`.
    #[test]
    fn an_id_link_is_not_a_uri() {
        assert!(!matches!(
            link_at("[[id:6F39]]", 5).unwrap().target,
            Target::Uri(_)
        ));
    }

    /// The arm keys on the SCHEME, not on the substring — a file whose
    /// name merely contains `id:` is still a file, and `file:` still
    /// wins where both could match.
    #[test]
    fn only_the_id_scheme_is_an_id() {
        assert_eq!(
            link_at("[[file:id:weird.org]]", 5).unwrap().target,
            Target::File("id:weird.org".into())
        );
        assert_eq!(
            link_at("[[notes/id-list.org]]", 5).unwrap().target,
            Target::File("notes/id-list.org".into())
        );
    }

    /// Exact and case-sensitive, like org's own internal links: jumping to
    /// the wrong heading is worse than not jumping.
    #[test]
    fn internal_links_resolve_against_headline_titles() {
        let lines = ["* One", "** TODO Deep Work", "text", "** Other"];
        let get = |i: u32| lines.get(i as usize).map(|s| s.to_string());
        let kw = crate::todo::parse_keywords("TODO | DONE");

        assert_eq!(
            find_headline(get, 4, "Deep Work", &kw),
            Some(1),
            "the TODO keyword is not part of the title"
        );
        assert_eq!(find_headline(get, 4, "Other", &kw), Some(3));
        assert_eq!(
            find_headline(get, 4, "deep work", &kw),
            None,
            "case matters"
        );
        assert_eq!(find_headline(get, 4, "Missing", &kw), None);
    }
}

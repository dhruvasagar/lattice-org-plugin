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

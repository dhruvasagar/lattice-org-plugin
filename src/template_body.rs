//! CT.1 — where a template's text comes from, for capture and roam alike.
//!
//! Design: `lattice/docs/dev/architecture/org-capture-templates.md` §6.
//!
//! ## Why this is not in `roam_templates`
//!
//! It was, and that was the bug. OR.14 gave roam templates a `body-file` —
//! emacs org-roam's `(file "…/template.org")` — because ten of the reference
//! templates are declared exactly that way, each naming an org file the user
//! edits directly. Capture templates got nothing, and there is no reason for
//! the asymmetry that survives being stated: a template's body SOURCE is
//! orthogonal to its DESTINATION. Roam owns the destination question (a file
//! that does not exist yet, named after the node); it does not own this one.
//!
//! So the rules live here and both template readers call them. What stays with
//! each caller is the part that genuinely differs: roam refuses a template with
//! no body at all, capture allows one (an empty body is a valid capture — the
//! user types into the draft), and only roam has a `${…}` pass to interpolate
//! with.

/// Where a template's text comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BodySource {
    /// Declared neither. Resolves to the empty string; whether that is an error
    /// is the CALLER's policy, because the two callers disagree.
    #[default]
    Empty,
    /// `body = "…"`, used as written.
    Inline(String),
    /// `body-file = "…"`, read at draft time. Unexpanded: the path may still
    /// carry `${…}`, which only means something once the node using it exists.
    File(String),
}

/// A template that declared both a `body` and a `body-file`.
///
/// A unit type rather than a message, because the useful message names the
/// template's KEY and the key is the caller's to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BothSet;

/// Classify a declared `body` / `body-file` pair.
///
/// **Blank counts as absent for both**, matching the rule `file` already had:
/// `body = ""` in a TOML table is far likelier to be a half-finished edit than
/// a deliberate empty template, and treating it as a declaration would make the
/// mutual-exclusion check below fire on a template that declared one thing.
///
/// Both set is [`BothSet`] rather than a precedence rule. A template cannot
/// mean both "here is the text" and "read the text from there", and resolving
/// it silently in either direction uses the one the user did NOT mean about
/// half the time.
pub fn classify(body: Option<&str>, body_file: Option<&str>) -> Result<BodySource, BothSet> {
    let inline = body.map(str::trim).filter(|b| !b.is_empty());
    let file = body_file.map(str::trim).filter(|f| !f.is_empty());
    match (inline, file) {
        (Some(_), Some(_)) => Err(BothSet),
        (Some(b), None) => Ok(BodySource::Inline(b.to_string())),
        (None, Some(f)) => Ok(BodySource::File(f.to_string())),
        (None, None) => Ok(BodySource::Empty),
    }
}

/// Resolve a source to its text, reading the file when that is the source.
///
/// ## Why the read is here and not at config-read time
///
/// A `body-file` path may carry `${…}`, which cannot be expanded before the
/// node using it exists. Reading at config time would also read EVERY
/// template's file whenever the set is parsed — and the set is parsed to build
/// the menu, which lists templates without using them. One selected template,
/// one read.
///
/// ## `read_file` and `interpolate` are injected
///
/// So this is testable without a host, the same reason each caller's
/// `from_declared` is split from its `read`. `interpolate` is roam's `${…}`
/// pass; capture has none and passes identity. It applies to the PATH as well
/// as the content, because a per-node template path is the whole point of
/// interpolating a path at all.
///
/// `Err(())` from `read_file` rather than a carried message: the host's error
/// detail is not for the user, only the path is, and this function has that.
///
/// ## A missing, unreadable or empty file is `Err`, never a panic
///
/// The caller turns it into a warn. A template that silently produced an empty
/// note is worse than one that names the path and says it could not be read —
/// an empty result is indistinguishable from a failed create, and the user
/// finds out only when they look for the note later.
pub fn resolve(
    source: &BodySource,
    key: &str,
    interpolate: impl Fn(&str) -> String,
    read_file: impl FnOnce(&str) -> Result<String, ()>,
) -> Result<String, String> {
    let raw = match source {
        BodySource::Empty => String::new(),
        BodySource::Inline(text) => text.clone(),
        BodySource::File(pattern) => {
            let path = crate::roam_scan::expand_tilde(&interpolate(pattern));
            let text = read_file(&path)
                .map_err(|()| format!("`{key}`'s file could not be read: {path}"))?;
            if text.trim().is_empty() {
                return Err(format!("`{key}`'s file is empty: {path}"));
            }
            text
        }
    };
    Ok(interpolate(&raw))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn id(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn one_declaration_each_way() {
        assert_eq!(
            classify(Some("* TODO %?"), None),
            Ok(BodySource::Inline("* TODO %?".to_string()))
        );
        assert_eq!(
            classify(None, Some("~/t.org")),
            Ok(BodySource::File("~/t.org".to_string()))
        );
        assert_eq!(classify(None, None), Ok(BodySource::Empty));
    }

    /// Both set is the configuration error the caller names by key.
    #[test]
    fn both_set_is_refused() {
        assert_eq!(classify(Some("x"), Some("~/t.org")), Err(BothSet));
    }

    /// Blank is absent for both — so a half-finished `body = ""` beside a real
    /// `body-file` is NOT the mutual-exclusion error, it is a file template.
    #[test]
    fn blank_counts_as_absent() {
        assert_eq!(
            classify(Some("   "), Some("~/t.org")),
            Ok(BodySource::File("~/t.org".to_string()))
        );
        assert_eq!(classify(Some("  "), None), Ok(BodySource::Empty));
        assert_eq!(classify(None, Some("\t")), Ok(BodySource::Empty));
    }

    #[test]
    fn an_inline_body_needs_no_read() {
        let got = resolve(&BodySource::Inline(id("* TODO %?")), "t", id, |_| {
            panic!("must not read a file for an inline body")
        });
        assert_eq!(got.unwrap(), "* TODO %?");
    }

    #[test]
    fn a_file_body_is_read() {
        let got = resolve(&BodySource::File(id("/tmp/t.org")), "t", id, |p| {
            assert_eq!(p, "/tmp/t.org");
            Ok("* from the file\n".to_string())
        });
        assert_eq!(got.unwrap(), "* from the file\n");
    }

    /// The interpolation runs on the PATH, not only the content — which is what
    /// makes a per-node template path possible.
    #[test]
    fn the_path_is_interpolated_before_the_read() {
        let got = resolve(
            &BodySource::File(id("~/tpl/${slug}.org")),
            "t",
            |s| s.replace("${slug}", "recipe"),
            |p| {
                assert!(
                    p.ends_with("/tpl/recipe.org"),
                    "the path was interpolated and tilde-expanded: {p}"
                );
                Ok("body".to_string())
            },
        );
        assert_eq!(got.unwrap(), "body");
    }

    /// And on the content afterwards, so a file template gets the same pass an
    /// inline one does.
    #[test]
    fn the_content_is_interpolated_after_the_read() {
        let got = resolve(
            &BodySource::File(id("/tmp/t.org")),
            "t",
            |s| s.replace("${title}", "Chicken"),
            |_| Ok("#+title: ${title}\n".to_string()),
        );
        assert_eq!(got.unwrap(), "#+title: Chicken\n");
    }

    /// An unreadable file names the KEY and the RESOLVED path — the path is the
    /// only part the user can act on.
    #[test]
    fn an_unreadable_file_names_the_key_and_the_path() {
        let err = resolve(&BodySource::File(id("/nope.org")), "c", id, |_| Err(())).unwrap_err();
        assert!(err.contains('c'), "{err}");
        assert!(err.contains("/nope.org"), "{err}");
    }

    /// An empty file is an error rather than an empty note: the two are
    /// indistinguishable afterwards, and only one of them is what was meant.
    #[test]
    fn an_empty_file_is_an_error() {
        let err = resolve(&BodySource::File(id("/e.org")), "c", id, |_| {
            Ok("  \n\t\n".to_string())
        })
        .unwrap_err();
        assert!(err.contains("is empty"), "{err}");
    }

    /// `Empty` resolves to nothing WITHOUT reading. Capture allows it (the user
    /// types into the draft); roam refuses it. That policy split is the
    /// caller's, which is why this returns `Ok`.
    #[test]
    fn an_empty_source_resolves_to_nothing() {
        let got = resolve(&BodySource::Empty, "t", id, |_| {
            panic!("must not read a file for an empty source")
        });
        assert_eq!(got.unwrap(), "");
    }
}

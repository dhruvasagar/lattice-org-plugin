//! OR.11b — `org.roam-capture-templates`: what a new roam note starts as.
//!
//! Design: `lattice/docs/dev/architecture/org-roam.md` §6.3.
//!
//! ## Why this is not `org.capture-templates`
//!
//! Emacs keeps `org-roam-capture-templates` separate from
//! `org-capture-templates`, and the reason is structural rather than
//! historical: a capture template says WHERE its text lands — a file, a
//! headline under it — and a roam template cannot, because the destination is
//! a file that does not exist yet and whose name is derived from the node
//! being made. `target` is the field the two sets disagree about, and it is
//! the field capture is organised around.
//!
//! So a roam template has no `target`. It has an optional `file`, which names
//! the note's FILENAME rather than a place inside an existing file, and which
//! interpolates `${slug}` / `${id}` / `${title}` like the body does.
//!
//! ## Both placeholder syntaxes, in one order
//!
//! [`crate::roam_capture`] expands `${…}` (the node being made) and
//! [`crate::capture`] expands `%…` (the capture context). Roam templates get
//! both, `${}` first — see `roam_capture::expand_fields` for why that ordering
//! is the safe one.
//!
//! ## Unset is not an error
//!
//! With nothing configured, creating a note writes the built-in stub it always
//! wrote (`:PROPERTIES:` / `:ID:` / `#+title:`) and does not prompt. Making
//! the feature depend on configuration would break note creation for every
//! user who has never heard of templates, to add a menu with one row in it.

use lattice_plugin_sdk::ConfigShape as ConfigShapeDerive;

/// One declared roam template.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShapeDerive)]
pub struct RawRoamTemplate {
    /// The keystroke that selects it in the menu.
    pub key: String,
    /// What the menu row says. Absent falls back to the key, so a template is
    /// never an unlabelled row.
    pub description: Option<String>,
    /// The note's text, with `${…}` and `%…` placeholders.
    pub body: Option<String>,
    /// The note's text, read from a FILE instead of inlined — emacs org-roam's
    /// `(file "…/template.org")`. `${…}` expands on this PATH the same way it
    /// does on `file` below, so a per-node template path is possible; the file
    /// CONTENT gets both placeholder passes, exactly as `body` does. Mutually
    /// exclusive with `body` — setting both is a configuration error.
    pub body_file: Option<String>,
    /// The note's FILENAME, with `${…}` placeholders — org-roam's
    /// `:target (file+head "${slug}.org" …)` without the head, which is what
    /// `body` already is. Absent uses the timestamped default, which is what
    /// org-roam itself defaults to.
    pub file: Option<String>,
}

/// The declared shape of `org.roam-capture-templates`.
pub type Declared = Vec<RawRoamTemplate>;

/// A usable template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoamTemplate {
    pub key: String,
    pub description: String,
    /// Inline text. Empty when [`Self::body_file`] is the source instead —
    /// the two are mutually exclusive, enforced in [`from_declared`].
    pub body: String,
    /// A path to read at draft time — see [`resolve_body`]. Unexpanded: it may
    /// still carry `${…}`, resolved once the node making it exists.
    pub body_file: Option<String>,
    pub file: Option<String>,
}

/// A read set, plus what was dropped getting there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoamTemplateSet {
    pub templates: Vec<RoamTemplate>,
    /// Named rather than dropped silently — the menu is where a missing row is
    /// noticeable, and this is what the footer says.
    pub skipped: Vec<String>,
}

impl RoamTemplateSet {
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&RoamTemplate> {
        self.templates.iter().find(|t| t.key == key)
    }
}

/// Read `org.roam-capture-templates`.
///
/// Never an error. Unset, malformed and empty all mean "no templates", and the
/// caller writes the built-in stub — see the module header. That is the
/// difference from [`crate::capture_templates::read`], which errors: a capture
/// with no template has nothing to do, while a roam note without one has a
/// perfectly good default.
pub fn read() -> RoamTemplateSet {
    let raw = match crate::config_shape::read_option::<Declared>("roam-capture-templates") {
        Some(Ok(raw)) => raw,
        // The host validated the tree against the schema before storing it, so
        // an `Err` here means the schema and `from_value` disagree — a bug, not
        // a user's typo. Degrade to the stub rather than trapping.
        _ => return RoamTemplateSet::default(),
    };
    from_declared(raw)
}

/// The rules, split from the read so they are testable without a host.
pub fn from_declared(raw: Declared) -> RoamTemplateSet {
    let mut out = RoamTemplateSet::default();
    for (i, t) in raw.into_iter().enumerate() {
        let key = t.key.trim().to_string();
        if key.is_empty() {
            // Named by position, since there is no key to name it by. A
            // template with no key is unreachable: the menu is keyed.
            out.skipped.push(format!("template {} has no `key`", i + 1));
            continue;
        }
        if out.templates.iter().any(|e| e.key == key) {
            // First wins, and the loser is NAMED. The menu cannot resolve two
            // rows on one keystroke, and firing whichever came last silently is
            // worse than saying so.
            out.skipped
                .push(format!("`{key}` is defined twice; the first one wins"));
            continue;
        }
        // Blank is the same as absent for both — `a_blank_file_is_the_same_as_none`
        // already established that pattern for `file`.
        let inline_body = t.body.as_deref().map(str::trim).filter(|b| !b.is_empty());
        let body_file = t
            .body_file
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty());
        let (body, body_file) = match (inline_body, body_file) {
            (Some(_), Some(_)) => {
                // A template cannot say both "here is the text" and "read the
                // text from here" — resolving the conflict silently in either
                // direction would use whichever the user did NOT mean half the
                // time.
                out.skipped
                    .push(format!("`{key}` sets both `body` and `body_file`"));
                continue;
            }
            (Some(b), None) => (b.to_string(), None),
            (None, Some(f)) => (String::new(), Some(f.to_string())),
            (None, None) => {
                // A template that writes nothing would make an empty note,
                // which is indistinguishable from a failed create.
                out.skipped.push(format!("`{key}` has no `body`"));
                continue;
            }
        };
        let description = match t.description.as_deref().map(str::trim) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => key.clone(),
        };
        let file = t
            .file
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(str::to_string);
        out.templates.push(RoamTemplate {
            key,
            description,
            body,
            body_file,
            file,
        });
    }
    out
}

/// Resolve a template's body text for one node, reading `body_file` off disk
/// when that is the source instead of `body`.
///
/// ## Why the read happens here, not in [`read`]
///
/// `body_file`'s PATH may carry `${…}` — the same shape `file` (the note's own
/// filename) already has, and for the same reason: org-roam interpolates its
/// target paths, and a per-node template path is nothing more than that
/// applied to the template instead of the note. The node does not exist until
/// a create is underway, so there is nothing to expand, and nothing to read,
/// before then. `roam_draft` calls this once the node — title, slug, minted
/// id — is known.
///
/// ## `read_file` is injected
///
/// So this is testable without a host, the same reason [`from_declared`] is
/// split from [`read`]: production passes a closure around
/// `host_services::read_file`, tests pass a fixture. `Err(())` rather than a
/// carried message — the host's error detail is not for the user, only the
/// PATH is, and this function already has that.
///
/// ## A missing or unreadable file is `Err`, never a panic
///
/// The caller turns that into a warn — the same channel `roam_draft` already
/// uses for every other reason a note cannot be made. A template that
/// silently wrote an empty note would be worse than one that says why it
/// could not.
pub fn resolve_body(
    template: &RoamTemplate,
    node: &crate::roam_capture::Node<'_>,
    read_file: impl FnOnce(&str) -> Result<String, ()>,
) -> Result<String, String> {
    let raw = match &template.body_file {
        Some(pattern) => {
            let path =
                crate::roam_scan::expand_tilde(&crate::roam_capture::expand_fields(pattern, node));
            let text = read_file(&path)
                .map_err(|()| format!("`{}`'s file could not be read: {path}", template.key))?;
            if text.trim().is_empty() {
                // Same reasoning as the inline `has no body` skip in
                // `from_declared` — an empty file is indistinguishable from a
                // failed create, just discovered a hop later.
                return Err(format!("`{}`'s file is empty: {path}", template.key));
            }
            text
        }
        None => template.body.clone(),
    };
    Ok(crate::roam_capture::expand_fields(&raw, node))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn t(key: &str, body: &str) -> RawRoamTemplate {
        RawRoamTemplate {
            key: key.to_string(),
            description: None,
            body: Some(body.to_string()),
            body_file: None,
            file: None,
        }
    }

    fn node<'a>(title: &'a str, slug: &'a str, id: &'a str) -> crate::roam_capture::Node<'a> {
        crate::roam_capture::Node { title, slug, id }
    }

    #[test]
    fn a_template_without_a_description_is_labelled_by_its_key() {
        let set = from_declared(vec![t("d", "#+title: ${title}")]);
        assert_eq!(set.templates[0].description, "d");
    }

    #[test]
    fn a_keyless_template_is_skipped_and_named_by_position() {
        let set = from_declared(vec![t("", "body")]);
        assert!(set.is_empty());
        assert_eq!(set.skipped, vec!["template 1 has no `key`"]);
    }

    /// The menu cannot resolve two rows on one keystroke, so the second is
    /// dropped — but NAMED, because a row that silently is not there reads as
    /// the feature being broken.
    #[test]
    fn a_duplicate_key_keeps_the_first_and_says_so() {
        let set = from_declared(vec![t("d", "first"), t("d", "second")]);
        assert_eq!(set.templates.len(), 1);
        assert_eq!(set.templates[0].body, "first");
        assert_eq!(
            set.skipped,
            vec!["`d` is defined twice; the first one wins"]
        );
    }

    /// An empty body would make an empty note, which is indistinguishable from
    /// a create that failed.
    #[test]
    fn a_bodyless_template_is_skipped() {
        let set = from_declared(vec![t("d", "   ")]);
        assert!(set.is_empty());
        assert_eq!(set.skipped, vec!["`d` has no `body`"]);
    }

    #[test]
    fn a_blank_file_is_the_same_as_none() {
        let mut raw = t("d", "body");
        raw.file = Some("  ".to_string());
        assert_eq!(from_declared(vec![raw]).templates[0].file, None);
    }

    #[test]
    fn a_file_pattern_survives_verbatim_for_the_caller_to_expand() {
        let mut raw = t("d", "body");
        raw.file = Some("${slug}.org".to_string());
        assert_eq!(
            from_declared(vec![raw]).templates[0].file.as_deref(),
            Some("${slug}.org")
        );
    }

    /// A template naming a `body_file` instead of an inline `body` is valid —
    /// `body` stays empty and `body_file` carries the pattern, unread until a
    /// node exists to read it for.
    #[test]
    fn a_body_file_template_is_accepted_with_an_empty_inline_body() {
        let raw = RawRoamTemplate {
            key: "s".to_string(),
            description: None,
            body: None,
            body_file: Some("~/templates/source.org".to_string()),
            file: None,
        };
        let set = from_declared(vec![raw]);
        assert_eq!(set.skipped, Vec::<String>::new());
        assert_eq!(set.templates[0].body, "");
        assert_eq!(
            set.templates[0].body_file.as_deref(),
            Some("~/templates/source.org")
        );
    }

    /// `body` and `body_file` naming the same template is a configuration
    /// error, not a coin flip between them — skip it and say so, the same as
    /// every other config mistake this module catches.
    #[test]
    fn setting_both_body_and_body_file_is_a_configuration_error() {
        let mut raw = t("d", "inline text");
        raw.body_file = Some("~/templates/d.org".to_string());
        let set = from_declared(vec![raw]);
        assert!(set.is_empty());
        assert_eq!(set.skipped, vec!["`d` sets both `body` and `body_file`"]);
    }

    /// Blank is the same as absent for `body_file` too — a template with a
    /// blank `body` and a blank `body_file` has no source at all, same
    /// message as the plain bodyless case.
    #[test]
    fn a_blank_body_file_with_no_body_is_the_bodyless_skip() {
        let raw = RawRoamTemplate {
            key: "d".to_string(),
            description: None,
            body: None,
            body_file: Some("   ".to_string()),
            file: None,
        };
        let set = from_declared(vec![raw]);
        assert!(set.is_empty());
        assert_eq!(set.skipped, vec!["`d` has no `body`"]);
    }

    /// A blank `body` alongside a real `body_file` is not "both set" — the
    /// blank one does not count, the same rule `file` already has.
    #[test]
    fn a_blank_body_alongside_a_body_file_is_not_a_conflict() {
        let raw = RawRoamTemplate {
            key: "d".to_string(),
            description: None,
            body: Some("   ".to_string()),
            body_file: Some("~/templates/d.org".to_string()),
            file: None,
        };
        let set = from_declared(vec![raw]);
        assert_eq!(set.skipped, Vec::<String>::new());
        assert_eq!(
            set.templates[0].body_file.as_deref(),
            Some("~/templates/d.org")
        );
    }

    #[test]
    fn resolve_body_reads_and_expands_a_file_sourced_template() {
        let raw = RawRoamTemplate {
            key: "s".to_string(),
            description: None,
            body: None,
            body_file: Some("~/templates/${slug}.org".to_string()),
            file: None,
        };
        let template = &from_declared(vec![raw]).templates[0];
        let n = node("Rust Async", "rust_async", "ABC-123");
        let body = resolve_body(template, &n, |path| {
            assert!(
                path.contains("rust_async.org") && !path.contains("${"),
                "the path is expanded before it is read: {path}"
            );
            assert!(
                !path.starts_with('~'),
                "the path is tilde-expanded before it is read: {path}"
            );
            Ok("#+title: ${title}\n".to_string())
        })
        .expect("a readable file resolves");
        assert_eq!(body, "#+title: Rust Async\n");
    }

    #[test]
    fn resolve_body_reads_inline_text_without_a_reader_call() {
        let template = &from_declared(vec![t("d", "#+title: ${title}")]).templates[0];
        let n = node("Rust", "rust", "I");
        let body = resolve_body(template, &n, |_| {
            panic!("body_file is unset — the reader must not be called")
        })
        .expect("an inline body resolves without reading anything");
        assert_eq!(body, "#+title: Rust");
    }

    /// A missing or unreadable file is `Err`, never a panic — `roam_draft`
    /// turns this into a skip (a warn), the same channel every other reason a
    /// note cannot be made already uses.
    #[test]
    fn resolve_body_reports_an_unreadable_file_by_name_and_path() {
        let raw = RawRoamTemplate {
            key: "s".to_string(),
            description: None,
            body: None,
            body_file: Some("~/templates/source.org".to_string()),
            file: None,
        };
        let template = &from_declared(vec![raw]).templates[0];
        let n = node("T", "t", "I");
        let err = resolve_body(template, &n, |_| Err(())).unwrap_err();
        assert!(err.contains('s'), "names the template: {err:?}");
        assert!(
            err.contains("templates/source.org"),
            "names the path: {err:?}"
        );
    }

    /// An empty file would silently produce an empty note — indistinguishable
    /// from a failed create, same reasoning as the inline bodyless skip.
    #[test]
    fn resolve_body_reports_an_empty_file() {
        let raw = RawRoamTemplate {
            key: "s".to_string(),
            description: None,
            body: None,
            body_file: Some("~/templates/source.org".to_string()),
            file: None,
        };
        let template = &from_declared(vec![raw]).templates[0];
        let n = node("T", "t", "I");
        let err = resolve_body(template, &n, |_| Ok("   \n".to_string())).unwrap_err();
        assert!(err.contains("empty"), "{err:?}");
    }
}

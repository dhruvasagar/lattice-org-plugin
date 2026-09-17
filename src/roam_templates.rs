//! OR.11b / CT.8 — `org.roam-capture-templates`: the SAME template type as
//! `org.capture-templates`, held in a second option.
//!
//! Design: `lattice/docs/dev/architecture/org-capture-templates.md` §1, §7.
//!
//! ## One type, two options — as emacs has it
//!
//! This file used to declare its own `RawRoamTemplate`, justified by the claim
//! that the split was org's: "a capture template says WHERE its text lands, and
//! a roam template cannot". That was wrong. `org-roam-capture-templates`
//! documents the same `keys / description / type / template` tuple as
//! `org-capture-templates`, and its `:target` is COMPULSORY
//! (`org-roam-capture--get-target` raises "Template needs to specify
//! `:target'"). Roam adds target kinds and a `${…}` pass; it never forked the
//! type. So a roam template is a [`capture_templates::Template`], declared in
//! a different option, resolved with [`TemplateList::Roam`].
//!
//! What stays here is what is genuinely roam's:
//!
//! - the `${title}` / `${slug}` / `${id}` pass, which means something only
//!   because a node is being made;
//! - resolving a target PATH against `org.roam-directory`, as org-roam's
//!   `expand-file-name path org-roam-directory` does;
//! - reporting a set that failed to load, instead of quietly writing the
//!   built-in stub for a user who configured templates.
//!
//! [`capture_templates::Template`]: crate::capture_templates::Template
//! [`TemplateList::Roam`]: crate::capture_templates::TemplateList::Roam

use crate::capture_templates::{ParsedSet, Template, TemplateError, TemplateList};

/// The declared shape of `org.roam-capture-templates` — capture's, exactly.
pub type Declared = crate::capture_templates::Declared;

/// Read `org.roam-capture-templates`.
///
/// `Err(TemplateError::Unset)` is the ONE result that means "write the
/// built-in stub": the user has never configured templates, and note creation
/// predates this option. Every other error means they HAVE configured some and
/// none can be used — and writing the stub then is the silent-misplacement
/// failure CT.2 fixed one layer down, a note created from a template the user
/// did not choose. The caller warns instead.
pub fn read() -> Result<ParsedSet, TemplateError> {
    crate::capture_templates::read_for(TemplateList::Roam, "roam-capture-templates")
}

/// The template's body, for one node — read from its `body-file` if it has one.
///
/// Roam's `${…}` pass runs on the path AND the content, before capture's `%`
/// pass, which `open_capture_buffer` applies to the whole draft.
pub fn resolve_body(
    template: &Template,
    node: &crate::roam_capture::Node<'_>,
    read_file: impl FnOnce(&str) -> Result<String, ()>,
) -> Result<String, String> {
    let source = match &template.body_file {
        Some(path) => crate::template_body::BodySource::File(path.clone()),
        None if template.body.is_empty() => crate::template_body::BodySource::Empty,
        None => crate::template_body::BodySource::Inline(template.body.clone()),
    };
    crate::template_body::resolve(
        &source,
        &template.key,
        |s| crate::roam_capture::expand_fields(s, node),
        read_file,
    )
}

/// Where a roam template's note lives: its target path with every placeholder
/// filled, made absolute.
///
/// org-roam's order: fill the template (`${…}` and `%…`), then
/// `expand-file-name` against `org-roam-directory`. A relative path lands in
/// the roam directory; an absolute or `~/…` one is used as written.
pub fn target_path(
    pattern: &str,
    node: &crate::roam_capture::Node<'_>,
    now: crate::time_format::When,
    dir: &str,
) -> String {
    let filled =
        crate::time_format::expand_time(&crate::roam_capture::expand_fields(pattern, node), now);
    let path = crate::roam_scan::expand_tilde(filled.trim());
    if path.starts_with('/') {
        path
    } else {
        format!("{}/{path}", dir.trim_end_matches('/'))
    }
}

/// The text a roam capture writes, given whether its file is new.
///
/// org-roam's `file+head` inserts the head only "if the node is a newly
/// captured one" (`org-roam-capture.el:495-503`), newline-terminated, and
/// prescribes the file an `:ID:`. A capture into a file that already exists
/// writes the body alone: the head and the id are already there.
pub fn draft_text(
    template: &Template,
    node: &crate::roam_capture::Node<'_>,
    body: &str,
    id: &str,
    is_new_file: bool,
) -> String {
    if !is_new_file {
        return body.to_string();
    }
    let mut text = String::new();
    if let Some(head) = &template.head {
        let head = crate::roam_capture::expand_fields(head, node);
        text.push_str(&head);
        if !head.ends_with('\n') {
            text.push('\n');
        }
    }
    text.push_str(body);
    // The `:ID:` is org-roam's to guarantee, not the template's. Applied after
    // the head, so the drawer opens the file — where `org-id-get-create` at
    // `point-min` puts it — and after `${…}`, so a template spelling
    // `:ID: ${id}` itself is seen as already having one.
    crate::roam_capture::ensure_id(&text, id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use crate::capture_templates::{from_declared_for, RawTarget, RawTemplate};

    fn raw(key: &str, body: Option<&str>, body_file: Option<&str>) -> RawTemplate {
        RawTemplate {
            key: key.to_string(),
            body: body.map(str::to_string),
            body_file: body_file.map(str::to_string),
            target: RawTarget {
                kind: Some("file".to_string()),
                file: "%<%Y%m%d%H%M%S>-${slug}.org".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn set(templates: Vec<RawTemplate>) -> ParsedSet {
        from_declared_for(TemplateList::Roam, templates).expect("a usable roam set")
    }

    fn node<'a>(title: &'a str, slug: &'a str, id: &'a str) -> crate::roam_capture::Node<'a> {
        crate::roam_capture::Node {
            title,
            slug,
            id,
            origin: "",
        }
    }

    /// 2026-09-16 14:05:09 local.
    fn at() -> crate::time_format::When {
        crate::time_format::When {
            local_secs: 1_789_567_509,
        }
    }

    /// A roam template is a capture `Template`: the reference config's shape
    /// resolves, target and all.
    #[test]
    fn a_roam_template_is_the_shared_type() {
        let s = set(vec![raw("c", None, Some("~/t/pkos-concept.org"))]);
        let t = &s.templates[0];
        assert_eq!(t.key, "c");
        assert_eq!(t.body_file.as_deref(), Some("~/t/pkos-concept.org"));
        assert_eq!(t.target.file(), "%<%Y%m%d%H%M%S>-${slug}.org");
    }

    /// The menu cannot resolve two rows on one keystroke, so the second is
    /// dropped — but NAMED, because a row that silently is not there reads as
    /// the feature being broken.
    #[test]
    fn a_duplicate_key_keeps_the_first_and_says_so() {
        let s = set(vec![
            raw("d", Some("first"), None),
            raw("d", Some("second"), None),
        ]);
        assert_eq!(s.templates.len(), 1);
        assert_eq!(s.templates[0].body, "first");
        assert_eq!(s.skipped.len(), 1);
    }

    /// CT.8: a bodyless roam template is USABLE now, as it is in emacs — an
    /// empty `plain` template defaults to `%?`. The old refusal argued that an
    /// empty body makes an empty note, but a new note always gets its `:ID:`
    /// drawer (and a head, if declared), so it is never empty.
    #[test]
    fn a_bodyless_roam_template_is_usable() {
        let s = set(vec![raw("d", None, None)]);
        assert_eq!(s.templates.len(), 1);
        assert_eq!(s.templates[0].body, "");
    }

    /// Both body sources is a configuration error, named.
    #[test]
    fn setting_both_body_and_body_file_is_a_configuration_error() {
        let result = from_declared_for(
            TemplateList::Roam,
            vec![
                raw("x", Some("inline"), Some("~/t.org")),
                raw("ok", Some("b"), None),
            ],
        )
        .expect("the survivor keeps the set");
        assert_eq!(result.templates.len(), 1);
        assert!(
            result.skipped[0].contains("sets both"),
            "{:?}",
            result.skipped
        );
    }

    /// CT.8: org-roam's `:target` is COMPULSORY, so a roam template without
    /// one is refused — by the host, structurally, as a missing required field.
    /// That is the same rule the capture list has.
    #[test]
    fn the_target_is_required_in_the_roam_list_too() {
        use lattice_plugin_sdk::shape::{ConfigShape, Schema};
        let Schema::List(inner) = <Declared as ConfigShape>::schema() else {
            panic!("a list of templates");
        };
        let Schema::Record(fields) = inner.as_ref() else {
            panic!("each template is a record");
        };
        let target = fields.iter().find(|f| f.name == "target").expect("target");
        assert!(target.required, "org-roam requires :target");
    }

    #[test]
    fn resolve_body_reads_and_expands_a_file_sourced_template() {
        let s = set(vec![raw("s", None, Some("~/templates/${slug}.org"))]);
        let n = node("Rust Async", "rust_async", "ABC-123");
        let body = resolve_body(&s.templates[0], &n, |path| {
            assert!(
                path.contains("rust_async.org") && !path.contains("${"),
                "the path is expanded before it is read: {path}"
            );
            assert!(!path.starts_with('~'), "tilde-expanded: {path}");
            Ok("#+title: ${title}\n".to_string())
        })
        .expect("a readable file resolves");
        assert_eq!(body, "#+title: Rust Async\n");
    }

    #[test]
    fn resolve_body_reads_inline_text_without_a_reader_call() {
        let s = set(vec![raw("d", Some("#+title: ${title}"), None)]);
        let n = node("Rust", "rust", "I");
        let body = resolve_body(&s.templates[0], &n, |_| panic!("no body-file")).unwrap();
        assert_eq!(body, "#+title: Rust");
    }

    #[test]
    fn resolve_body_reports_an_unreadable_file_by_name_and_path() {
        let s = set(vec![raw("s", None, Some("~/templates/source.org"))]);
        let err = resolve_body(&s.templates[0], &node("T", "t", "I"), |_| Err(())).unwrap_err();
        assert!(err.contains('s'), "names the template: {err:?}");
        assert!(
            err.contains("templates/source.org"),
            "names the path: {err:?}"
        );
    }

    #[test]
    fn resolve_body_reports_an_empty_file() {
        let s = set(vec![raw("s", None, Some("~/templates/source.org"))]);
        let err = resolve_body(&s.templates[0], &node("T", "t", "I"), |_| {
            Ok("   \n".to_string())
        })
        .unwrap_err();
        assert!(err.contains("empty"), "{err:?}");
    }

    /// The reference config's path: `%<…>` and `${slug}` both filled, and the
    /// result placed under the roam directory — org-roam's
    /// `expand-file-name path org-roam-directory`.
    #[test]
    fn a_relative_target_path_lands_in_the_roam_directory() {
        let path = target_path(
            "%<%Y%m%d%H%M%S>-${slug}.org",
            &node("Rust Async", "rust_async", "I"),
            at(),
            "/org/roam/",
        );
        assert_eq!(path, "/org/roam/20260916140509-rust_async.org");
    }

    /// An absolute path is used as written; `~` is expanded.
    #[test]
    fn an_absolute_target_path_is_kept() {
        let n = node("T", "t", "I");
        assert_eq!(
            target_path("/abs/${slug}.org", &n, at(), "/org/roam"),
            "/abs/t.org"
        );
        let home = std::env::var("HOME").expect("HOME");
        assert_eq!(
            target_path("~/notes/%<%Y-%m-%d>.org", &n, at(), "/org/roam"),
            format!("{home}/notes/2026-09-16.org")
        );
    }

    /// org-roam's `file+head`: a NEW file gets the head, then the body, with
    /// the `:ID:` drawer opening the file.
    #[test]
    fn a_new_file_gets_its_head_and_an_id() {
        let s = from_declared_for(
            TemplateList::Roam,
            vec![RawTemplate {
                key: "d".to_string(),
                body: Some("%?".to_string()),
                target: RawTarget {
                    kind: Some("file+head".to_string()),
                    file: "${slug}.org".to_string(),
                    head: Some("#+title: ${title}".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }],
        )
        .unwrap();
        let n = node("Rust", "rust", "ID-1");
        let text = draft_text(&s.templates[0], &n, "%?", "ID-1", true);
        assert_eq!(
            text, ":PROPERTIES:\n:ID:       ID-1\n:END:\n#+title: Rust\n%?",
            "drawer, then the head (newline-terminated), then the body"
        );
    }

    /// A capture into a file that already exists writes the body alone — the
    /// head and the id are already there. This is org-roam's "head content
    /// will be inserted if the node is a newly captured one".
    #[test]
    fn an_existing_file_gets_the_body_alone() {
        let s = from_declared_for(
            TemplateList::Roam,
            vec![RawTemplate {
                key: "d".to_string(),
                body: Some("* entry".to_string()),
                target: RawTarget {
                    kind: Some("file+head".to_string()),
                    file: "%<%Y-%m-%d>.org".to_string(),
                    head: Some("#+title: %<%Y-%m-%d>".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }],
        )
        .unwrap();
        let text = draft_text(
            &s.templates[0],
            &node("T", "t", "ID"),
            "* entry",
            "ID",
            false,
        );
        assert_eq!(text, "* entry");
    }

    /// No head declared: a new file still gets its id, as org-roam's plain
    /// `file` target "will be created, and prescribed an ID".
    #[test]
    fn a_plain_file_target_still_prescribes_an_id() {
        let s = set(vec![raw("c", Some("#+title: ${title}"), None)]);
        let text = draft_text(
            &s.templates[0],
            &node("T", "t", "ID-2"),
            "#+title: T\n",
            "ID-2",
            true,
        );
        assert!(
            text.starts_with(":PROPERTIES:\n:ID:       ID-2\n:END:\n"),
            "{text:?}"
        );
        assert!(text.ends_with("#+title: T\n"), "{text:?}");
    }
}

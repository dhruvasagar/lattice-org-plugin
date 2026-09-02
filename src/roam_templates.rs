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
    pub body: String,
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
        let body = t.body.unwrap_or_default();
        if body.trim().is_empty() {
            // A template that writes nothing would make an empty note, which
            // is indistinguishable from a failed create.
            out.skipped.push(format!("`{key}` has no `body`"));
            continue;
        }
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
            file,
        });
    }
    out
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
            file: None,
        }
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
}

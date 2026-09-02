//! OC.2 / TC.5 — the capture template set, and where it is declared.
//!
//! `org.capture-templates` is a **structured option**: it declares a schema and
//! its value crosses as a tree. Until TC.3 it could not be — an option was
//! `boolean | integer | string`, a template is a record, and an array-of-tables
//! could not reach an option at all — so it was TOML inside a string with a
//! parser in this file. That parser is gone; what is left here is the part a
//! schema cannot express.
//!
//! The declared shape:
//!
//! ```text
//! list<record {
//!     key:         string,
//!     description: string?,
//!     target:      record { file: string, headline: string? },
//!     body:        string?,
//!     clock-in:    bool?,
//! }>
//! ```
//!
//! and in `lattice.toml` it is written as itself, natively:
//!
//! ```toml
//! [[org.capture-templates]]
//! key = "t"
//! description = "todo"
//! target = { file = "~/org/refile.org" }
//! body = """
//! * TODO %?
//! %U
//! """
//! ```
//!
//! `init.rs` builds the same tree from a Rust struct through the SDK derive.
//! The two homes no longer share one string; what they share is the schema,
//! which is declared once and shown by `:describe-option`.
//!
//! **Structure is now the host's problem, semantics stay ours.** A missing
//! `key`, a `target` that is not a record, a `clock-in` that is not a boolean —
//! all rejected before this file sees them, with a path
//! (`capture-templates[2].target.file: expected string, got integer`). What
//! remains below is the checking a schema has no way to express: a key that is
//! blank after trimming, and two templates claiming the same key.
//!
//! **One behaviour deliberately changed.** Before, a template missing its `key`
//! or its `target` was SKIPPED and named in `skipped`, on the principle that one
//! typo should not cost the feature. That principle was compelling when the
//! alternative was a parser error with no location. It is not compelling
//! against `[2].key: required field is missing` — a silently absent menu row is
//! the failure that sends a user looking in the wrong place, and now that the
//! message says exactly which template and which field, refusing is the kinder
//! answer. Duplicate keys are still a skip, because there the value IS usable
//! and the only question is which row wins.
//!
//! **Parsed on read, never cached.** `:set org.capture-templates=…` must take
//! effect on the next capture, and a cache would need an `OptionChanged`
//! subscription to stay honest — the `todo-keywords` precedent (OM.7). The set
//! is read when the menu opens and when a capture is submitted, both explicit
//! user actions and neither on a typing path.
//!
//! Design: `docs/dev/architecture/org-capture.md` §2 and §4.

use lattice_plugin_sdk::ConfigShape as ConfigShapeDerive;

/// Where a capture lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Append at the end of the file.
    File { file: String },
    /// Insert after the named headline's whole subtree.
    ///
    /// A headline that is absent **appends and says so** rather than creating
    /// it or refusing — the note is not lost, and the echo is what tells the
    /// user their target moved (§4).
    FileHeadline { file: String, headline: String },
}

impl Target {
    /// The file this target writes to, whichever shape it is.
    pub fn file(&self) -> &str {
        match self {
            Target::File { file } | Target::FileHeadline { file, .. } => file,
        }
    }
}

/// One capture template: a key to press, a label for the menu, somewhere to
/// put it, and the text to expand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// The keystroke that selects it in the menu. A multi-character key is
    /// allowed — the transient resolver walks keys one keystroke at a time.
    pub key: String,
    /// What the menu row says.
    pub description: String,
    pub target: Target,
    /// The template text, placeholders unexpanded.
    pub body: String,
    /// OC.11 — org's `:clock-in`: start a clock on the entry this template
    /// captures, as part of capturing it.
    ///
    /// The clock line is written INTO the captured text rather than edited in
    /// afterwards, which is what makes this possible at all: capture files into
    /// another file, and an `apply-edit` names a buffer id that an unopened file
    /// does not have. Building the `:LOGBOOK:` drawer into the entry sidesteps
    /// the whole question — one write, and the durable record is correct the
    /// moment it lands (design D4).
    pub clock_in: bool,
}

/// Why a template set could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// The option is unset or blank. Distinct from a parse failure on purpose:
    /// "you have not configured capture" and "your configuration is broken"
    /// are different problems with different fixes, and a menu that says the
    /// wrong one sends the user looking in the wrong place.
    Unset,
    /// The stored value did not fit the declared shape in a way the host's
    /// own validation did not catch. Carries the message with its PATH,
    /// because the location is the whole value of reporting this at all.
    Malformed(String),
    /// It parsed, but every template in it was rejected.
    Empty,
}

impl TemplateError {
    /// What the user sees echoed.
    pub fn message(&self) -> String {
        match self {
            TemplateError::Unset => {
                "org: no capture templates — set `org.capture-templates`".to_string()
            }
            TemplateError::Malformed(e) => format!("org.capture-templates: {e}"),
            TemplateError::Empty => {
                "org.capture-templates: no usable templates in the set".to_string()
            }
        }
    }
}

// ---- The on-the-wire shape, declared then validated ----
//
// `#[derive(ConfigShape)]` gives these a schema, a `to_value` and a
// `from_value`. Field names cross kebab-cased, so `clock_in` is `clock-in` on
// the wire — matching org's own `:clock-in` rather than inventing a snake-case
// spelling for a key users already know.
//
// Optionality is the TYPE's: `Option<String>` is a field a user may omit,
// `String` is one they may not. `description` and `body` are optional because
// they have sensible empty defaults; `key` and `target` are not, because a
// template without either is not a template.

/// `target = { file = "…", headline = "…" }`.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShapeDerive)]
pub struct RawTarget {
    /// The file the capture is written to.
    pub file: String,
    /// Insert under this headline's subtree instead of appending to the file.
    pub headline: Option<String>,
}

/// One `[[org.capture-templates]]` entry, as declared.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShapeDerive)]
pub struct RawTemplate {
    /// The keystroke that selects this template in the capture menu.
    pub key: String,
    /// What the menu row says.
    pub description: Option<String>,
    /// Where the capture lands.
    pub target: RawTarget,
    /// The template text, with `%?` / `%U` / `%^{…}` placeholders.
    pub body: Option<String>,
    /// Start a clock on the entry this template captures (org's `:clock-in`).
    pub clock_in: Option<bool>,
}

/// The declared shape of `org.capture-templates`, for the registration call.
pub type Declared = Vec<RawTemplate>;

/// Read the option's value into the template set.
///
/// Structural failure cannot reach here — the host validated the tree against
/// the schema before it was stored, and reports a path. What this does is the
/// rest: trim, reject a blank key, resolve the two target shapes, and refuse a
/// duplicate key.
///
/// A **duplicate key** is a skip rather than an error: the first wins, the later
/// one is named in `skipped`. The menu cannot resolve two rows on one keystroke,
/// and silently firing whichever came last is worse than saying so — but the set
/// as a whole is still usable, which is what separates this from the structural
/// failures the host now catches.
pub fn from_declared(raw: Declared) -> Result<ParsedSet, TemplateError> {
    if raw.is_empty() {
        return Err(TemplateError::Unset);
    }

    let mut templates: Vec<Template> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for t in raw {
        let key = t.key.trim().to_string();
        let description = t.description.unwrap_or_default().trim().to_string();
        if key.is_empty() {
            // Present but blank — `key = ""` satisfies `required` and means
            // nothing. The schema cannot say "non-empty"; this can.
            skipped.push(format!(
                "a template with a blank `key` ({})",
                if description.is_empty() {
                    "unnamed"
                } else {
                    &description
                }
            ));
            continue;
        }
        let file = t.target.file.trim().to_string();
        if file.is_empty() {
            skipped.push(format!("`{key}` has a blank `target.file`"));
            continue;
        }
        if let Some(prior) = templates.iter().find(|p| p.key == key) {
            skipped.push(format!(
                "`{key}` is already taken by \"{}\"; skipping \"{description}\"",
                prior.description,
            ));
            continue;
        }
        let target = match t.target.headline.map(|h| h.trim().to_string()) {
            Some(headline) if !headline.is_empty() => Target::FileHeadline { file, headline },
            _ => Target::File { file },
        };
        templates.push(Template {
            key,
            description,
            target,
            body: t.body.unwrap_or_default(),
            clock_in: t.clock_in.unwrap_or(false),
        });
    }

    if templates.is_empty() {
        return Err(TemplateError::Empty);
    }
    Ok(ParsedSet { templates, skipped })
}

/// Read `org.capture-templates` and resolve it into a usable set.
///
/// The one entry point production code uses; `from_declared` is split out so
/// the resolution rules are testable without a host.
pub fn read() -> Result<ParsedSet, TemplateError> {
    match crate::config_shape::read_option::<Declared>("capture-templates") {
        Some(Ok(raw)) => from_declared(raw),
        // The host validated the write, so this is the residue a schema cannot
        // express. Carry the path it came with rather than flattening it to
        // "malformed" — the location is the whole value of reporting it.
        Some(Err(e)) => Err(TemplateError::Malformed(e.to_string())),
        None => Err(TemplateError::Unset),
    }
}

/// A parsed set plus what it could not use. The skips ride along rather than
/// being logged here so the CALLER decides where they surface — the menu echoes
/// them once at open; a unit test asserts on them directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSet {
    pub templates: Vec<Template>,
    pub skipped: Vec<String>,
}

impl ParsedSet {
    /// The template that key selects.
    pub fn by_key(&self, key: &str) -> Option<&Template> {
        self.templates.iter().find(|t| t.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lattice_plugin_sdk::shape::{ConfigShape, Schema, Value};

    fn t(
        key: &str,
        description: &str,
        file: &str,
        headline: Option<&str>,
        body: &str,
    ) -> RawTemplate {
        RawTemplate {
            key: key.to_string(),
            description: Some(description.to_string()),
            target: RawTarget {
                file: file.to_string(),
                headline: headline.map(str::to_string),
            },
            body: Some(body.to_string()),
            clock_in: None,
        }
    }

    /// The shape a real nine-template org config has, trimmed to the cases
    /// that differ: a plain file target, a headline target, a multi-line body,
    /// and a `%^{}` prompt (unexpanded here — OC.4 does that).
    fn real_set() -> Declared {
        vec![
            t("t", "todo", "~/org/refile.org", None, "* TODO %?\n%U\n%a\n"),
            t(
                "m",
                "Meeting",
                "~/org/refile.org",
                Some("Meetings"),
                "* MEETING with %? :meeting:\n%U\n",
            ),
            t(
                "v",
                "Vocab (French)",
                "~/org/vocab-french.org",
                Some("Vocabulary"),
                "* %^{Word} :fc:\n- Context: %^{Context sentence}\n",
            ),
        ]
    }

    #[test]
    fn a_real_template_set_resolves() {
        let set = from_declared(real_set()).expect("it resolves");
        assert!(set.skipped.is_empty(), "nothing skipped: {:?}", set.skipped);
        assert_eq!(set.templates.len(), 3);

        let t = set.by_key("t").expect("the todo template");
        assert_eq!(t.description, "todo");
        assert_eq!(
            t.target,
            Target::File {
                file: "~/org/refile.org".into()
            }
        );

        let m = set.by_key("m").expect("the meeting template");
        assert_eq!(
            m.target,
            Target::FileHeadline {
                file: "~/org/refile.org".into(),
                headline: "Meetings".into()
            }
        );
        assert_eq!(m.target.file(), "~/org/refile.org");
    }

    /// A multi-line body survives the crossing. This used to be the reason the
    /// option COULD be a string — a `"""` block keeps its newlines through the
    /// TOML-inside-TOML round trip — and it is now the field a TREE could
    /// plausibly regress instead. The test outlives the mechanism it was
    /// written for, which is why it is still here.
    #[test]
    fn a_bodys_newlines_survive_the_option() {
        let set = from_declared(real_set()).expect("it resolves");
        let body = &set.by_key("t").expect("todo").body;
        assert!(body.contains('\n'), "the body kept its newlines: {body:?}");
        assert_eq!(body.lines().count(), 3);
        assert!(body.starts_with("* TODO %?"));
    }

    #[test]
    fn the_declared_shape_is_what_the_option_promises() {
        // The schema IS the documentation now — `:describe-option` renders it
        // and `lattice.toml` is validated against it — so a field renamed or a
        // required/optional flipped is a user-visible change, not an internal
        // one.
        let Schema::List(inner) = <Declared as ConfigShape>::schema() else {
            panic!("the option is a list of templates");
        };
        let Schema::Record(fields) = inner.as_ref() else {
            panic!("each template is a record");
        };
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["key", "description", "target", "body", "clock-in"],
            "field names cross kebab-cased — `clock-in`, matching org's own \
             `:clock-in` rather than a snake-case spelling invented here"
        );
        let required: Vec<bool> = fields.iter().map(|f| f.required).collect();
        assert_eq!(
            required,
            vec![true, false, true, false, false],
            "`key` and `target` are the two a template cannot do without"
        );
    }

    #[test]
    fn unset_is_distinct_from_a_set_with_nothing_usable() {
        // "You have not configured capture" and "your configuration is broken"
        // are different problems with different fixes, and a menu that says the
        // wrong one sends the user looking in the wrong place.
        assert_eq!(from_declared(vec![]), Err(TemplateError::Unset));
        assert_eq!(
            from_declared(vec![t("", "", "~/org/x.org", None, "")]),
            Err(TemplateError::Empty)
        );
        assert!(TemplateError::Unset
            .message()
            .contains("no capture templates"));
        assert!(TemplateError::Empty
            .message()
            .contains("no usable templates"));
    }

    #[test]
    fn a_blank_key_is_skipped_by_name_with_the_others_intact() {
        // A blank key is what the SCHEMA cannot catch: `key = ""` satisfies
        // `required` and means nothing. The structural cases it CAN catch — a
        // missing `key`, a `target` that is not a record — never reach here at
        // all now; the host refuses the write with a path.
        let set = from_declared(vec![
            t("t", "todo", "~/org/refile.org", None, "* TODO"),
            t("  ", "nameless", "~/org/x.org", None, ""),
            t("n", "note", "~/org/notes.org", None, "* %?"),
        ])
        .expect("the usable ones survive");
        assert_eq!(set.templates.len(), 2, "one typo does not cost the feature");
        assert!(set.by_key("t").is_some());
        assert!(set.by_key("n").is_some());
        assert_eq!(set.skipped.len(), 1);
        assert!(
            set.skipped[0].contains("nameless"),
            "the skip names the row so it can be found: {:?}",
            set.skipped
        );
    }

    #[test]
    fn a_blank_target_file_is_skipped_by_key() {
        let set = from_declared(vec![
            t("t", "todo", "~/org/refile.org", None, ""),
            t("b", "broken", "   ", None, ""),
        ])
        .expect("the usable one survives");
        assert_eq!(set.templates.len(), 1);
        assert!(
            set.skipped[0].contains("`b`") && set.skipped[0].contains("target.file"),
            "{:?}",
            set.skipped
        );
    }

    #[test]
    fn a_duplicate_key_keeps_the_first_and_names_both() {
        // The menu cannot resolve two rows on one keystroke, and silently
        // firing whichever came last is worse than saying so. Still a SKIP
        // rather than an error: the set is usable, and the only question is
        // which row wins.
        let set = from_declared(vec![
            t("t", "todo", "~/org/refile.org", None, "* TODO"),
            t("t", "task", "~/org/other.org", None, "* TASK"),
        ])
        .expect("the first survives");
        assert_eq!(set.templates.len(), 1);
        assert_eq!(set.by_key("t").expect("first").description, "todo");
        assert_eq!(set.skipped.len(), 1);
        assert!(set.skipped[0].contains("todo"), "{:?}", set.skipped);
        assert!(set.skipped[0].contains("task"), "{:?}", set.skipped);
    }

    #[test]
    fn a_blank_headline_degrades_to_the_plain_file_target() {
        let set = from_declared(vec![t("t", "todo", "~/org/x.org", Some("   "), "")])
            .expect("it resolves");
        assert_eq!(
            set.templates[0].target,
            Target::File {
                file: "~/org/x.org".into()
            },
            "a whitespace headline is no headline, not a headline named \"   \""
        );
    }

    #[test]
    fn an_omitted_optional_field_takes_its_empty_default() {
        // `description`, `body` and `clock-in` are `Option<_>` in the declared
        // shape, so a user may leave them out. What they must NOT do is arrive
        // as the string "None" or as a missing template.
        let set = from_declared(vec![RawTemplate {
            key: "t".to_string(),
            description: None,
            target: RawTarget {
                file: "~/org/x.org".to_string(),
                headline: None,
            },
            body: None,
            clock_in: None,
        }])
        .expect("it resolves");
        assert_eq!(set.templates[0].description, "");
        assert_eq!(set.templates[0].body, "");
        assert!(!set.templates[0].clock_in);
    }

    #[test]
    fn a_declared_template_round_trips_through_the_tree() {
        // The contract the derive rests on. If `to_value` and `from_value`
        // disagreed, a template would change on the way through the option and
        // nothing else here would notice.
        let raw = real_set();
        let value = raw.to_value();
        assert_eq!(<Declared as ConfigShape>::from_value(&value), Ok(raw));

        // …and a value of the wrong shape is refused with a path, which is the
        // message a user now gets instead of a parser's line number.
        let mut bad = real_set().to_value();
        if let Value::List(items) = &mut bad {
            if let Value::Record(map) = &mut items[1] {
                map.insert("key".to_string(), Value::Int(7));
            }
        }
        let err = <Declared as ConfigShape>::from_value(&bad).expect_err("refused");
        assert_eq!(err.path, "[1].key");
    }
}

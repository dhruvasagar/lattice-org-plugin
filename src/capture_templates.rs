//! OC.2 — the capture template set, and where it is declared.
//!
//! `org.capture-templates` is a **string option whose value is TOML**. That is
//! forced rather than preferred: a template is a record — key, description,
//! target, body — and an option is `boolean | integer | string`, with list
//! support restricted to scalar elements. An array-of-tables cannot reach an
//! option at all.
//!
//! What makes it tolerable is that TOML carries the payload verbatim: a `'''`
//! literal block preserves newlines and nested `"""` blocks, so the option
//! value re-parses here with bodies intact. `init.rs` sets the identical string
//! as a Rust raw literal — one format, both homes, no third place to look.
//!
//! ```toml
//! [org]
//! capture-templates = '''
//! [[template]]
//! key = "t"
//! description = "todo"
//! target = { file = "~/org/refile.org" }
//! body = """
//! * TODO %?
//! %U
//! """
//! '''
//! ```
//!
//! **Parsed on read, never cached.** `:set org.capture-templates=…` must take
//! effect on the next capture, and a cache would need an `OptionChanged`
//! subscription to stay honest — the `todo-keywords` precedent (OM.7). The set
//! is parsed when the menu opens and when a capture is submitted, both of which
//! are explicit user actions and neither of which is on a typing path.
//!
//! Design: `docs/dev/architecture/org-capture.md` §2 and §4.

use serde::Deserialize;

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
    /// The TOML did not parse. Carries the parser's own message, because the
    /// line and column in it are the whole value of reporting this at all.
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

// ---- The on-the-wire shape, deserialised then validated ----

#[derive(Deserialize)]
struct RawSet {
    #[serde(default)]
    template: Vec<RawTemplate>,
}

#[derive(Deserialize)]
struct RawTemplate {
    #[serde(default)]
    key: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    target: Option<RawTarget>,
    #[serde(default)]
    body: String,
    /// `clock-in = true`. Dashed, matching org's `:clock-in` rather than
    /// inventing a snake-case spelling for a key users already know.
    #[serde(default, rename = "clock-in")]
    clock_in: bool,
}

#[derive(Deserialize)]
struct RawTarget {
    #[serde(default)]
    file: String,
    #[serde(default)]
    headline: Option<String>,
}

/// Parse the option's value into the template set.
///
/// Two failure levels, deliberately different:
///
/// - **The whole set is malformed** ⇒ `Err`. The user's configuration does not
///   parse, and offering a menu built from the half of it that happened to
///   survive would be guessing at what they meant.
/// - **One template is unusable** (no key, no target file) ⇒ skipped, named in
///   `skipped`, and the rest survive. One typo should not cost the feature.
///
/// A **duplicate key** is the same class: the first wins, the later one is
/// skipped and named. The menu cannot resolve two rows on one keystroke, and
/// silently firing whichever came last is worse than saying so.
pub fn parse(source: &str) -> Result<ParsedSet, TemplateError> {
    if source.trim().is_empty() {
        return Err(TemplateError::Unset);
    }
    let raw: RawSet =
        toml::from_str(source).map_err(|e| TemplateError::Malformed(e.message().to_string()))?;

    let mut templates: Vec<Template> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for t in raw.template {
        let key = t.key.trim().to_string();
        if key.is_empty() {
            skipped.push(format!(
                "a template with no `key` ({})",
                if t.description.trim().is_empty() {
                    "unnamed"
                } else {
                    t.description.trim()
                }
            ));
            continue;
        }
        let Some(target) = t.target else {
            skipped.push(format!("`{key}` has no `target`"));
            continue;
        };
        let file = target.file.trim().to_string();
        if file.is_empty() {
            skipped.push(format!("`{key}` has no `target.file`"));
            continue;
        }
        if let Some(prior) = templates.iter().find(|p| p.key == key) {
            skipped.push(format!(
                "`{key}` is already taken by \"{}\"; skipping \"{}\"",
                prior.description,
                t.description.trim()
            ));
            continue;
        }
        let target = match target.headline.map(|h| h.trim().to_string()) {
            Some(headline) if !headline.is_empty() => Target::FileHeadline { file, headline },
            _ => Target::File { file },
        };
        templates.push(Template {
            key,
            description: t.description.trim().to_string(),
            target,
            body: t.body,
            clock_in: t.clock_in,
        });
    }

    if templates.is_empty() {
        return Err(TemplateError::Empty);
    }
    Ok(ParsedSet { templates, skipped })
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

    /// The shape a real nine-template org config has, trimmed to the cases
    /// that differ: a plain file target, a headline target, a multi-line body,
    /// and a `%^{}` prompt (unexpanded here — OC.4 does that).
    const REAL_SET: &str = r#"
[[template]]
key = "t"
description = "todo"
target = { file = "~/org/refile.org" }
body = """
* TODO %?
%U
%a
"""

[[template]]
key = "m"
description = "Meeting"
target = { file = "~/org/refile.org", headline = "Meetings" }
body = """
* MEETING with %? :meeting:
%U
"""

[[template]]
key = "v"
description = "Vocab (French)"
target = { file = "~/org/vocab-french.org", headline = "Vocabulary" }
body = """
* %^{Word} :fc:
- Context: %^{Context sentence}
- Translation: %^{Translation}
"""
"#;

    #[test]
    fn a_real_template_set_round_trips() {
        let set = parse(REAL_SET).expect("it parses");
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

    /// The reason the option can be a string at all: a `"""` body keeps its
    /// newlines through the TOML-inside-TOML round trip. If this ever stopped
    /// holding, every multi-line template would silently collapse to one line.
    #[test]
    fn a_bodys_newlines_survive_the_option() {
        let set = parse(REAL_SET).unwrap();
        let v = set.by_key("v").unwrap();
        assert_eq!(
            v.body,
            "* %^{Word} :fc:\n- Context: %^{Context sentence}\n- Translation: %^{Translation}\n"
        );
    }

    /// Unset and malformed are different problems with different fixes, so
    /// they are different errors — a menu that says "not configured" about a
    /// typo sends the user looking in the wrong place.
    #[test]
    fn unset_and_malformed_are_distinct() {
        assert_eq!(parse(""), Err(TemplateError::Unset));
        assert_eq!(parse("   \n  "), Err(TemplateError::Unset));

        let err = parse("[[template]\nkey = \"t\"").expect_err("malformed TOML");
        let TemplateError::Malformed(msg) = err else {
            panic!("expected a parse error, got {err:?}");
        };
        assert!(!msg.is_empty(), "the parser's own message is carried");
    }

    /// One bad template costs that template, not the feature.
    #[test]
    fn a_malformed_entry_is_skipped_by_key_with_the_others_intact() {
        let set = parse(
            r#"
[[template]]
key = "t"
description = "todo"
target = { file = "/tmp/a.org" }
body = "* TODO %?"

[[template]]
key = "x"
description = "no target at all"
body = "* X"

[[template]]
key = ""
description = "no key"
target = { file = "/tmp/a.org" }
body = "* Y"

[[template]]
key = "n"
description = "note"
target = { file = "/tmp/a.org" }
body = "* %?"
"#,
        )
        .expect("the set parses");
        assert_eq!(
            set.templates
                .iter()
                .map(|t| t.key.as_str())
                .collect::<Vec<_>>(),
            vec!["t", "n"]
        );
        assert_eq!(set.skipped.len(), 2);
        assert!(
            set.skipped[0].contains("`x`"),
            "the skip names the key: {:?}",
            set.skipped
        );
        assert!(
            set.skipped[1].contains("no key"),
            "a keyless template is named by its description instead: {:?}",
            set.skipped
        );
    }

    /// The menu cannot resolve two rows on one keystroke. First wins, and the
    /// loser is named — silently firing whichever came last would make the
    /// menu's behaviour depend on declaration order nobody was told mattered.
    #[test]
    fn a_duplicate_key_keeps_the_first_and_names_both() {
        let set = parse(
            r#"
[[template]]
key = "t"
description = "todo"
target = { file = "/tmp/a.org" }
body = "* TODO %?"

[[template]]
key = "t"
description = "task"
target = { file = "/tmp/b.org" }
body = "* TASK %?"
"#,
        )
        .unwrap();
        assert_eq!(set.templates.len(), 1);
        assert_eq!(set.by_key("t").unwrap().description, "todo");
        assert!(
            set.skipped[0].contains("todo") && set.skipped[0].contains("task"),
            "the warning names both: {:?}",
            set.skipped
        );
    }

    /// A set whose every template is unusable is `Empty`, not an empty menu.
    /// A menu with no rows tells the user nothing about why.
    #[test]
    fn a_set_with_nothing_usable_is_an_error_not_an_empty_menu() {
        assert_eq!(
            parse("[[template]]\nkey = \"\"\nbody = \"x\"\n"),
            Err(TemplateError::Empty)
        );
        // Well-formed TOML with no templates at all is the same outcome.
        assert_eq!(parse("# just a comment\n"), Err(TemplateError::Empty));
    }

    /// A blank `headline` is a plain file target rather than a headline target
    /// that can never match — `headline = ""` in a hand-written config is an
    /// unfinished edit, not a request to search for the empty headline.
    #[test]
    fn a_blank_headline_degrades_to_the_plain_file_target() {
        let set = parse(
            "[[template]]\nkey = \"t\"\ntarget = { file = \"/tmp/a.org\", headline = \"  \" }\nbody = \"x\"\n",
        )
        .unwrap();
        assert_eq!(
            set.by_key("t").unwrap().target,
            Target::File {
                file: "/tmp/a.org".into()
            }
        );
    }

    /// Every error carries a message that names the option, so the echo is
    /// actionable without the user having to guess which setting is at fault.
    #[test]
    fn every_error_names_the_option() {
        for e in [
            TemplateError::Unset,
            TemplateError::Malformed("expected `]`".into()),
            TemplateError::Empty,
        ] {
            assert!(
                e.message().contains("capture-templates"),
                "the echo names the option: {}",
                e.message()
            );
        }
    }
}

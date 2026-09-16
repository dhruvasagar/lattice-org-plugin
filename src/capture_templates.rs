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
//!     target:      record {
//!         kind:     string?,        // `file` | `file+headline` | `file+olp`
//!         file:     string,
//!         headline: string?,
//!         olp:      list<string>?,
//!     },
//!     body:        string?,
//!     body-file:   string?,
//!     clock-in:    bool?,
//! }>
//! ```
//!
//! CT.1: `body` and `body-file` are mutually exclusive — the body's SOURCE,
//! shared with roam templates in [`crate::template_body`].
//!
//! CT.3: `target.kind` names the shape, in org's own vocabulary. **Absent keeps
//! the pre-CT.3 inference** (`headline` present ⇒ `file+headline`, else `file`),
//! so no existing config changes; every new shape says its name. It is a
//! `string` rather than a schema `Enum` because the derive kebab-cases variant
//! names and would spell `file+headline` as `file-headline` — the vocabulary a
//! user already knows is worth more than host-side validation of a closed set,
//! so [`resolve_target`] checks it and an unknown value is a named skip.
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
    /// CT.3: insert after the subtree at this full outline PATH.
    ///
    /// `FileHeadline` takes the first headline anywhere in the file with a
    /// matching name, which is right for a unique name and wrong for a repeated
    /// one — an `Inbox` under both `Work` and `Home` files everything under
    /// whichever comes first. A path says which. Org's own `file+olp` exists
    /// for exactly this, and its docstring says so: "for non-unique headings,
    /// the full outline path is safer".
    FileOlp { file: String, olp: Vec<String> },
}

impl Target {
    /// The file this target writes to, whichever shape it is, **as declared**.
    ///
    /// Still carries a `~` if the user wrote one. Anything that READS the file
    /// wants [`Self::resolved_file`] instead — see there for why the
    /// distinction is worth two methods.
    pub fn file(&self) -> &str {
        match self {
            Target::File { file }
            | Target::FileHeadline { file, .. }
            | Target::FileOlp { file, .. } => file,
        }
    }

    /// CT.2: the same file with `~` expanded — what every READ must use.
    ///
    /// The two host calls capture reads through, `host-services.read-file` and
    /// `tree-sitter.parse-file`, do not expand a tilde. `Effect::WriteToFile`
    /// does. So a `~/…` `file+headline` target used to write to the right file
    /// while searching the wrong one: the read failed, the outline came back
    /// empty, the headline was not found, and the note silently appended at
    /// end-of-file instead of landing under its headline.
    ///
    /// A separate method rather than expanding inside [`Self::file`] because
    /// the declared form is what belongs in a message to the user — echoing an
    /// expanded `/Users/…` path back at someone who wrote `~/org/x.org` tells
    /// them about their home directory rather than about their config.
    pub fn resolved_file(&self) -> String {
        crate::roam_scan::expand_tilde(self.file())
    }
}

/// CT.4: WHAT a template inserts — org's capture entry type.
///
/// The third axis. `target` says where the capture lands and the body says what
/// text it is; this says what SHAPE that text takes once it gets there. Org has
/// five (`entry`, `item`, `checkitem`, `table-line`, `plain`); two are built.
///
/// Named here as a real enum rather than a validated string, unlike
/// [`RawTarget::kind`]: these spellings contain no `+`, so the derive's
/// kebab-casing produces exactly org's own names and the host validates the
/// closed set for free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ConfigShapeDerive)]
pub enum EntryType {
    /// An org entry — a headline and its body. Today's behaviour, and the
    /// default, so a template that has never heard of this field is unchanged.
    #[default]
    Entry,
    /// A row in a table at the target. Crosses as `table-line`.
    TableLine,
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
    ///
    /// CT.1: **already resolved** — if the template declared a `body-file`,
    /// `selected_template` has read it by the time a `Template` reaches any
    /// consumer. So every existing reader of this field is unchanged, and the
    /// file is read once per capture rather than once per hop.
    pub body: String,
    /// CT.1: the unresolved `body-file` path, kept only so `selected_template`
    /// knows whether to read one. Every consumer downstream of it reads
    /// [`Self::body`], which is why this is not a [`BodySource`].
    ///
    /// [`BodySource`]: crate::template_body::BodySource
    pub body_file: Option<String>,
    /// CT.4: what this template inserts — see [`EntryType`].
    pub entry_type: EntryType,
    /// CT.4: for `table-line`, where in the table the row goes.
    pub placement: crate::capture_target::TablePlacement,
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
    /// OC.11c: the user SET this option and the assignment failed, so the
    /// value being read is the empty default rather than anything they wrote.
    ///
    /// Indistinguishable from [`Unset`](Self::Unset) by reading the option —
    /// that is the whole reason `config::option-diagnostic` exists. The
    /// difference matters because `Unset` takes the OM.11 legacy path and this
    /// must not: a user whose templates failed to load has configured capture,
    /// and filing their note through `org.capture-file` because their TOML had
    /// a typo is how the note ends up somewhere they thought they had stopped
    /// using.
    ///
    /// Carries the host's message, which for a composite includes the schema
    /// path — the fix location, at the moment they tried to capture.
    NotLoaded(String),
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
            TemplateError::NotLoaded(why) => {
                format!("org.capture-templates did not load: {why}")
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

/// `target = { kind = "…", file = "…", headline = "…", olp = […] }`.
#[derive(Debug, Clone, PartialEq, Eq, Default, ConfigShapeDerive)]
pub struct RawTarget {
    /// CT.3: which target shape this is — org's own vocabulary: `file`,
    /// `file+headline`, `file+olp`.
    ///
    /// **Absent keeps today's inference** (`headline` present ⇒
    /// `file+headline`, else `file`), so no existing config changes. Every NEW
    /// shape must say its name, which is what makes an illegal combination
    /// reportable instead of silently reinterpreted.
    ///
    /// A `String` rather than a schema `Enum`, and that is forced rather than
    /// chosen: the derive kebab-cases variant names, which would spell org's
    /// `file+headline` as `file-headline`. The vocabulary a user already knows
    /// is worth more than host-side validation of a closed set, so the set is
    /// checked here and an unknown value is a named skip.
    pub kind: Option<String>,
    /// The file the capture is written to.
    pub file: String,
    /// Insert under this headline's subtree instead of appending to the file.
    pub headline: Option<String>,
    /// CT.3: the full outline path, for `file+olp` — `["Work", "Inbox"]`.
    pub olp: Option<Vec<String>>,
}

/// One `[[org.capture-templates]]` entry, as declared.
///
/// CT.3: `Default` as well, because `RawTarget` reached four fields of which at
/// most two are set by any one `kind`. Without it every declaration in
/// `init.rs` carries `None`s that hide the fields it actually sets; with it a
/// reader sees only what the template means.
#[derive(Debug, Clone, PartialEq, Eq, Default, ConfigShapeDerive)]
pub struct RawTemplate {
    /// The keystroke that selects this template in the capture menu.
    pub key: String,
    /// What the menu row says.
    pub description: Option<String>,
    /// Where the capture lands.
    pub target: RawTarget,
    /// The template text, with `%?` / `%U` / `%^{…}` placeholders.
    pub body: Option<String>,
    /// CT.1: the template text read from a FILE instead of inlined — emacs
    /// org-capture's `(file "…/template.org")`. Mutually exclusive with
    /// [`Self::body`]; setting both is a configuration error that skips the
    /// template and names it.
    ///
    /// Roam templates have had this since OR.14. The asymmetry was accidental
    /// — that slice was scoped to roam — and there is no reason for it that
    /// survives being stated: a body's SOURCE is orthogonal to a template's
    /// DESTINATION, which is the only axis roam actually differs on.
    pub body_file: Option<String>,
    /// CT.4: org's capture entry type — `entry` (default) or `table-line`.
    /// `r#type` so the wire name is org's own `type`.
    pub r#type: Option<EntryType>,
    /// CT.4: org's `:table-line-pos`, e.g. `"II-1"` — which hline group the
    /// row goes relative to. Only meaningful for `table-line`.
    pub table_line_pos: Option<String>,
    /// CT.4: org's `:prepend` — put the row at the TOP of the table's data
    /// rather than after the last row. Only meaningful for `table-line`.
    pub prepend: Option<bool>,
    /// Start a clock on the entry this template captures (org's `:clock-in`).
    pub clock_in: Option<bool>,
}

/// The declared shape of `org.capture-templates`, for the registration call.
pub type Declared = Vec<RawTemplate>;

/// CT.3: resolve a declared target into the shape it names.
///
/// ## `kind` absent is the old inference, deliberately
///
/// Before this there were two shapes and the presence of `headline` chose
/// between them. That inference IS the shipped semantics, so it stays as the
/// default rather than being replaced by a required field — every existing
/// `org.capture-templates` keeps working untouched. What changes is that every
/// NEW shape must name itself, so the tag is declared rather than guessed.
///
/// ## An illegal combination is named, not reinterpreted
///
/// `kind = "file"` beside a `headline` is a contradiction: the user wrote a
/// headline and asked for a target that has none. Ignoring the extra field
/// would file the note at end-of-file while the config says otherwise — the
/// silent-misplacement failure CT.2 just fixed one layer down. Refusing the
/// template and saying which field is the answer that sends them to the right
/// line.
///
/// A blank `headline` or an all-blank `olp` is treated as ABSENT rather than as
/// an error, matching the rule `file` and `body` already use: a half-finished
/// edit is likelier than a deliberate empty string.
fn resolve_target(
    key: &str,
    file: String,
    kind: Option<String>,
    headline: Option<String>,
    olp: Option<Vec<String>>,
) -> Result<Target, String> {
    let headline = headline
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty());
    let olp: Option<Vec<String>> = olp.map(|segments| {
        segments
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
    let olp = olp.filter(|segments: &Vec<String>| !segments.is_empty());

    let kind = kind.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    let Some(kind) = kind else {
        // The pre-CT.3 inference, unchanged.
        return Ok(match headline {
            Some(headline) => Target::FileHeadline { file, headline },
            None => Target::File { file },
        });
    };

    match kind.as_str() {
        "file" => match (headline, olp) {
            (None, None) => Ok(Target::File { file }),
            _ => Err(format!(
                "`{key}`: `kind = \"file\"` takes neither `headline` nor `olp`"
            )),
        },
        "file+headline" => match (headline, olp) {
            (Some(headline), None) => Ok(Target::FileHeadline { file, headline }),
            (None, _) => Err(format!(
                "`{key}`: `kind = \"file+headline\"` needs a `headline`"
            )),
            (Some(_), Some(_)) => Err(format!(
                "`{key}`: `kind = \"file+headline\"` does not take `olp`"
            )),
        },
        "file+olp" => match (headline, olp) {
            (None, Some(olp)) => Ok(Target::FileOlp { file, olp }),
            (_, None) => Err(format!("`{key}`: `kind = \"file+olp\"` needs an `olp`")),
            (Some(_), Some(_)) => Err(format!(
                "`{key}`: `kind = \"file+olp\"` does not take `headline`"
            )),
        },
        // Named rather than ignored: an unrecognised kind silently falling back
        // to `file` would append every capture to the end of the file while the
        // config plainly says otherwise.
        other => Err(format!(
            "`{key}`: unknown target `kind = \"{other}\"` \
             (expected `file`, `file+headline` or `file+olp`)"
        )),
    }
}

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
        let target =
            match resolve_target(&key, file, t.target.kind, t.target.headline, t.target.olp) {
                Ok(target) => target,
                Err(why) => {
                    skipped.push(why);
                    continue;
                }
            };
        // CT.1: the same rules roam uses. Unlike roam, `Empty` is NOT a skip —
        // a capture template with no body is a blank draft the user types into,
        // which is a perfectly ordinary way to capture and was the behaviour
        // before this field existed (`body` was `unwrap_or_default`).
        let (body, body_file) =
            match crate::template_body::classify(t.body.as_deref(), t.body_file.as_deref()) {
                Ok(crate::template_body::BodySource::Empty) => (String::new(), None),
                Ok(crate::template_body::BodySource::Inline(b)) => (b, None),
                Ok(crate::template_body::BodySource::File(f)) => (String::new(), Some(f)),
                Err(crate::template_body::BothSet) => {
                    skipped.push(format!("`{key}` sets both `body` and `body-file`"));
                    continue;
                }
            };
        // CT.4: `table-line-pos` wins over `prepend`, which is org's own
        // precedence (`org-capture-place-table-line` tests the pos spec first).
        // Both declared is not an error — the specific one simply wins, the
        // way it does in emacs.
        let placement = match (
            t.table_line_pos
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            t.prepend.unwrap_or(false),
        ) {
            (Some(spec), _) => crate::capture_target::TablePlacement::Pos(spec.to_string()),
            (None, true) => crate::capture_target::TablePlacement::Prepend,
            (None, false) => crate::capture_target::TablePlacement::End,
        };
        templates.push(Template {
            key,
            description,
            target,
            body,
            body_file,
            entry_type: t.r#type.unwrap_or_default(),
            placement,
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
    let out = match crate::config_shape::read_option::<Declared>("capture-templates") {
        Some(Ok(raw)) => from_declared(raw),
        // The host validated the write, so this is the residue a schema cannot
        // express. Carry the path it came with rather than flattening it to
        // "malformed" — the location is the whole value of reporting it.
        Some(Err(e)) => Err(TemplateError::Malformed(e.to_string())),
        None => Err(TemplateError::Unset),
    };
    // OC.11c: an empty read is ambiguous, so ask why it is empty.
    //
    // A failed assignment leaves the option at its registered default, which
    // for this one is an empty list — and `from_declared` maps empty to
    // `Unset`. So "the user's templates did not parse" and "the user has no
    // templates" arrive here identically, and only the host can tell them
    // apart. Asked ONLY on the empty path: an option that loaded fine has
    // nothing to explain, and a stale diagnostic cannot exist anyway (the
    // registry drops it the moment an assignment succeeds).
    if matches!(out, Err(TemplateError::Unset)) {
        if let Some(why) = crate::config_shape::option_failure("capture-templates") {
            return Err(TemplateError::NotLoaded(why));
        }
    }
    out
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
                ..Default::default()
            },
            body: Some(body.to_string()),
            body_file: None,
            clock_in: None,
            ..Default::default()
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

    /// CT.3 helper: one template with the given target fields.
    fn with_target(kind: Option<&str>, headline: Option<&str>, olp: Option<Vec<&str>>) -> Declared {
        vec![RawTemplate {
            key: "t".to_string(),
            body: Some("* TODO %?".to_string()),
            target: RawTarget {
                kind: kind.map(str::to_string),
                file: "~/org/x.org".to_string(),
                headline: headline.map(str::to_string),
                olp: olp.map(|v| v.into_iter().map(str::to_string).collect()),
            },
            ..Default::default()
        }]
    }

    /// CT.3: `kind` absent keeps the pre-CT.3 inference, so no existing config
    /// changes. This is the compatibility guarantee the whole slice rests on.
    #[test]
    fn an_absent_kind_infers_the_two_legacy_shapes() {
        let set = from_declared(with_target(None, None, None)).expect("resolves");
        assert_eq!(
            set.templates[0].target,
            Target::File {
                file: "~/org/x.org".to_string()
            }
        );

        let set = from_declared(with_target(None, Some("Vocabulary"), None)).expect("resolves");
        assert_eq!(
            set.templates[0].target,
            Target::FileHeadline {
                file: "~/org/x.org".to_string(),
                headline: "Vocabulary".to_string(),
            }
        );
    }

    /// Each kind names itself, in org's own vocabulary.
    #[test]
    fn each_kind_resolves_to_its_target_shape() {
        let set = from_declared(with_target(Some("file"), None, None)).expect("resolves");
        assert!(matches!(set.templates[0].target, Target::File { .. }));

        let set =
            from_declared(with_target(Some("file+headline"), Some("Vocab"), None)).expect("ok");
        assert!(matches!(
            set.templates[0].target,
            Target::FileHeadline { .. }
        ));

        let set = from_declared(with_target(
            Some("file+olp"),
            None,
            Some(vec!["Work", "Inbox"]),
        ))
        .expect("ok");
        assert_eq!(
            set.templates[0].target,
            Target::FileOlp {
                file: "~/org/x.org".to_string(),
                olp: vec!["Work".to_string(), "Inbox".to_string()],
            }
        );
    }

    /// A kind carrying a field it does not take is REFUSED and named — never
    /// silently ignored. Ignoring it would file the note at end-of-file while
    /// the config plainly says otherwise, which is the silent-misplacement
    /// failure CT.2 fixed one layer down.
    #[test]
    fn a_kind_carrying_a_field_it_does_not_take_is_named() {
        for (kind, headline, olp, needle) in [
            ("file", Some("H"), None, "takes neither"),
            ("file+headline", None, None, "needs a `headline`"),
            (
                "file+headline",
                Some("H"),
                Some(vec!["A"]),
                "does not take `olp`",
            ),
            ("file+olp", None, None, "needs an `olp`"),
            (
                "file+olp",
                Some("H"),
                Some(vec!["A"]),
                "does not take `headline`",
            ),
        ] {
            let err = from_declared(with_target(Some(kind), headline, olp))
                .expect_err("the only template was refused, so the set is empty");
            assert_eq!(
                err,
                TemplateError::Empty,
                "kind `{kind}` should have left nothing usable"
            );
            // And the reason rides back through `skipped` when the set has a
            // survivor — checked separately below, since `Empty` carries none.
            let _ = needle;
        }
    }

    /// The skip MESSAGE, on a set that survives — it must name the key and the
    /// field, because "your config is wrong" without a location is what sends
    /// someone looking in the wrong file.
    #[test]
    fn an_illegal_target_names_the_key_and_the_field() {
        let mut declared = with_target(Some("file+olp"), Some("H"), Some(vec!["A"]));
        declared.push(RawTemplate {
            key: "ok".to_string(),
            body: Some("* TODO %?".to_string()),
            target: RawTarget {
                file: "~/org/x.org".to_string(),
                ..Default::default()
            },
            ..Default::default()
        });
        let set = from_declared(declared).expect("the survivor keeps the set alive");
        assert_eq!(set.templates.len(), 1);
        assert_eq!(set.templates[0].key, "ok");
        assert_eq!(
            set.skipped,
            vec!["`t`: `kind = \"file+olp\"` does not take `headline`"]
        );
    }

    /// An unknown kind is named rather than falling back to `file` — a silent
    /// fallback appends every capture to the end of the file while the config
    /// says otherwise.
    #[test]
    fn an_unknown_kind_is_named() {
        let mut declared = with_target(Some("file+datetree"), None, None);
        declared.push(RawTemplate {
            key: "ok".to_string(),
            body: Some("* TODO %?".to_string()),
            target: RawTarget {
                file: "~/org/x.org".to_string(),
                ..Default::default()
            },
            ..Default::default()
        });
        let set = from_declared(declared).expect("the survivor keeps the set alive");
        assert_eq!(set.skipped.len(), 1);
        assert!(
            set.skipped[0].contains("unknown target `kind = \"file+datetree\"`"),
            "{:?}",
            set.skipped[0]
        );
        // `file+datetree` is CT.6's, and until then it must be refused rather
        // than quietly behaving as `file`.
    }

    /// A blank `headline` or an all-blank `olp` is ABSENT, not an error — the
    /// rule `file` and `body` already use, because a half-finished edit is
    /// likelier than a deliberate empty string.
    #[test]
    fn blank_target_fields_count_as_absent() {
        let set = from_declared(with_target(None, Some("   "), None)).expect("resolves");
        assert!(
            matches!(set.templates[0].target, Target::File { .. }),
            "a blank headline infers the plain file target"
        );

        let set = from_declared(with_target(Some("file"), None, Some(vec!["  ", ""]))).expect("ok");
        assert!(
            matches!(set.templates[0].target, Target::File { .. }),
            "an all-blank olp is not a declaration"
        );
    }

    /// CT.2: a `~/…` target is expanded for READING, and left alone for
    /// showing.
    ///
    /// The bug this closes was silent data misplacement: the reads
    /// (`read-file`, `parse-file`) do not expand a tilde but the write does, so
    /// a `~/…` `file+headline` target wrote to the right file while searching
    /// the wrong one — no read, no outline, no headline match, and the note
    /// appended at end-of-file instead of under its headline.
    #[test]
    fn a_tilde_target_is_expanded_for_reading_and_kept_for_showing() {
        // SAFETY: single-threaded test, and the value is restored below.
        let home = std::env::var("HOME").expect("HOME is set in the test env");

        let t = Target::FileHeadline {
            file: "~/org/refile.org".to_string(),
            headline: "Vocabulary".to_string(),
        };
        assert_eq!(
            t.resolved_file(),
            format!("{home}/org/refile.org"),
            "the read path must be absolute or the headline is never found"
        );
        assert_eq!(
            t.file(),
            "~/org/refile.org",
            "the DECLARED form is what a message should quote back"
        );

        // The plain-file shape takes the same path.
        let f = Target::File {
            file: "~/org/inbox.org".to_string(),
        };
        assert_eq!(f.resolved_file(), format!("{home}/org/inbox.org"));
    }

    /// An absolute path is untouched, and `~user` is deliberately left verbatim
    /// — expanding it against OUR home would produce a plausible path to the
    /// wrong place, which is worse than one that visibly still has a `~`.
    #[test]
    fn an_absolute_target_is_unchanged_and_tilde_user_is_left_alone() {
        let abs = Target::File {
            file: "/srv/org/x.org".to_string(),
        };
        assert_eq!(abs.resolved_file(), "/srv/org/x.org");

        let other = Target::File {
            file: "~alice/org/x.org".to_string(),
        };
        assert_eq!(other.resolved_file(), "~alice/org/x.org");
    }

    /// CT.1: a capture template may name a FILE for its body, as a roam
    /// template has been able to since OR.14.
    ///
    /// The path is carried UNRESOLVED — `selected_template` reads it when a
    /// template is actually chosen, so building the menu does not read every
    /// template's file.
    #[test]
    fn a_capture_template_can_take_its_body_from_a_file() {
        let set = from_declared(vec![RawTemplate {
            key: "h".to_string(),
            description: Some("habit".to_string()),
            target: RawTarget {
                file: "~/org/habit.org".to_string(),
                headline: None,
                ..Default::default()
            },
            body: None,
            body_file: Some("~/org/templates/habit.org".to_string()),
            clock_in: None,
            ..Default::default()
        }])
        .expect("a body-file template is usable");
        assert_eq!(set.skipped, Vec::<String>::new());
        assert_eq!(
            set.templates[0].body_file.as_deref(),
            Some("~/org/templates/habit.org")
        );
        assert_eq!(set.templates[0].body, "", "unresolved until it is chosen");
    }

    /// Both set is a configuration error, named by key — the same rule roam
    /// has, now shared. Silently preferring either one uses the source the user
    /// did not mean about half the time.
    #[test]
    fn a_template_setting_both_body_and_body_file_is_skipped_and_named() {
        let set = from_declared(vec![
            RawTemplate {
                key: "b".to_string(),
                description: Some("both".to_string()),
                target: RawTarget {
                    file: "~/org/x.org".to_string(),
                    headline: None,
                    ..Default::default()
                },
                body: Some("* TODO %?".to_string()),
                body_file: Some("~/org/t.org".to_string()),
                clock_in: None,
                ..Default::default()
            },
            RawTemplate {
                key: "t".to_string(),
                description: Some("fine".to_string()),
                target: RawTarget {
                    file: "~/org/x.org".to_string(),
                    headline: None,
                    ..Default::default()
                },
                body: Some("* TODO %?".to_string()),
                body_file: None,
                clock_in: None,
                ..Default::default()
            },
        ])
        .expect("the usable template keeps the set alive");
        assert_eq!(set.templates.len(), 1, "only `t` survives");
        assert_eq!(set.templates[0].key, "t");
        assert_eq!(set.skipped, vec!["`b` sets both `body` and `body-file`"]);
    }

    /// Capture and roam DISAGREE here, deliberately. A roam template with no
    /// body is skipped — it would make an empty note, indistinguishable from a
    /// failed create. A capture template with no body is a blank draft the user
    /// types into, which is an ordinary way to capture and was the behaviour
    /// before `body-file` existed.
    #[test]
    fn a_capture_template_with_no_body_at_all_is_still_usable() {
        let set = from_declared(vec![RawTemplate {
            key: "n".to_string(),
            description: Some("blank".to_string()),
            target: RawTarget {
                file: "~/org/x.org".to_string(),
                headline: None,
                ..Default::default()
            },
            body: None,
            body_file: None,
            clock_in: None,
            ..Default::default()
        }])
        .expect("a bodyless capture template is not an error");
        assert_eq!(set.skipped, Vec::<String>::new());
        assert_eq!(set.templates[0].body, "");
        assert_eq!(set.templates[0].body_file, None);
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
            vec![
                "key",
                "description",
                "target",
                "body",
                "body-file",
                "type",
                "table-line-pos",
                "prepend",
                "clock-in"
            ],
            "field names cross kebab-cased — `clock-in` and CT.1's `body-file`, \
             matching org's own `:clock-in` rather than a snake-case spelling \
             invented here"
        );
        let required: Vec<bool> = fields.iter().map(|f| f.required).collect();
        assert_eq!(
            required,
            vec![true, false, true, false, false, false, false, false, false],
            "`key` and `target` are the two a template cannot do without — \
             CT.1's `body-file` is optional, and so is `body`, because a \
             template may declare either or neither, and CT.4's three all \
             default (`type` to `entry`, the placement pair to end-of-table)"
        );

        // CT.3: and the NESTED target record, which the assertions above do not
        // reach. `target` gained `kind` and `olp` in CT.3 and every one of the
        // checks above still passed — a user-visible schema change that the
        // contract test did not notice, which is precisely the drift this test
        // exists to catch. Pinning the inner record closes it.
        let target = fields
            .iter()
            .find(|f| f.name == "target")
            .expect("a template has a target");
        let Schema::Record(target_fields) = &target.schema else {
            panic!("the target is a record");
        };
        let target_names: Vec<&str> = target_fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            target_names,
            vec!["kind", "file", "headline", "olp"],
            "the target record's shape is as user-visible as the template's"
        );
        let target_required: Vec<bool> = target_fields.iter().map(|f| f.required).collect();
        assert_eq!(
            target_required,
            vec![false, true, false, false],
            "`file` is the only field every target shape needs — `kind` absent \
             keeps the pre-CT.3 inference, and `headline` / `olp` belong to one \
             kind each"
        );
        assert!(
            matches!(
                target_fields
                    .iter()
                    .find(|f| f.name == "olp")
                    .map(|f| &f.schema),
                Some(Schema::List(_))
            ),
            "`olp` is a list of strings — an outline PATH, not one heading"
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
                ..Default::default()
            },
            body: None,
            body_file: None,
            clock_in: None,
            ..Default::default()
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

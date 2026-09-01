//! OA.11 — `org.agenda-custom-commands`, and which agenda a scan is running.
//!
//! A TOML-string option shaped like `org.agenda-sections` and forced into a
//! string for the identical reason (AS.2): an option is
//! `boolean | integer | string`, so an array-of-tables cannot reach one at all.
//! One option serves `lattice.toml` and `init.rs` alike, with no second seam.
//!
//! ```toml
//! [org]
//! agenda-custom-commands = '''
//! [[command]]
//! key = "w"
//! description = "Waiting and Postponed"
//!
//!   [[command.section]]
//!   title = "Waiting"
//!   when = "any"
//!   match = "-CANCELLED+WAITING|HOLD/!"
//!
//! [[command]]
//! key = "r"
//! description = "Refile"
//!
//!   [[command.section]]
//!   title = "Tasks to Refile"
//!   when = "any"
//!   match = "REFILE"
//! '''
//! ```
//!
//! ## A command is a named section set, which is why this is not a new concept
//!
//! `[[command.section]]` deserialises through `agenda_sections`' OWN
//! `RawSection`, not through a copy of it. A custom command's sections are
//! sections in every respect — same `when`, same `todo-only`, same
//! `min-priority`, same `match`, same skip-and-name rules — and declaring the
//! shape twice would be two places for `todo-only` to be spelled and one of
//! them to be spelled wrong.
//!
//! What a command adds is a `key` and a `description`: an entry in the
//! dispatcher (OA.12) and a name for the thing you chose.
//!
//! ## Failure is inherited wholesale from `agenda_sections`
//!
//! Deliberately, because the reasoning transfers unchanged and a second
//! failure model for the same class of configuration is a second thing to
//! learn. The guest cannot log — calling `logging::log` makes the component
//! import `logging`, which org's multi-seam linker does not wire, so the whole
//! component fails to instantiate — so a broken set reports itself through the
//! section titles, which are the view's own headers.
//!
//! - **The whole set is malformed** ⇒ the command is not found, and the scan
//!   falls back to `org.agenda-sections` with the parse error ridden onto the
//!   first header. The agenda still works and says why it is not the one you
//!   asked for.
//! - **One command is unusable** (no key, no usable sections) ⇒ skipped and
//!   named; the rest survive. One typo must not cost the feature.
//!
//! Falling back rather than showing nothing is the AS.2 rule and the sharpest
//! version of it: an empty agenda and a correct-but-empty agenda look
//! identical, and "you have no tasks" is the single worst thing this view can
//! say incorrectly. A user who asked for "Waiting" and has nothing waiting must
//! not be shown the same screen as a user whose config failed to parse.
//!
//! ## A notice can still go unseen, and that is inherited too
//!
//! The notice rides the FIRST section's title, and the host attaches a group
//! title to a ROW — so a first section that admits no rows renders no header,
//! and the complaint disappears with it. A user whose agenda-files hold nothing
//! overdue gets the fallback silently.
//!
//! This is AS.2's mechanism working as designed rather than a defect
//! introduced here, and it is left alone deliberately: fixing it belongs in
//! `agenda_sections`, where it would fix both, and the fix is not obvious —
//! the sections are resolved in `begin`, before any row exists, so the guest
//! cannot know which of them will render. Prefixing every title instead is
//! ruled out by AS.2's own test ("a notice repeated on every header is
//! noise"). Recorded here because the failure is silent, which is exactly the
//! kind that gets rediscovered as a bug.
//!
//! Design: `docs/dev/architecture/org-agenda.md` §8.

use serde::Deserialize;

use crate::agenda::Section;
use crate::agenda_sections::{self, RawSection};

// ---- The on-the-wire shape, deserialised then validated ----

#[derive(Deserialize)]
struct RawSet {
    #[serde(default)]
    command: Vec<RawCommand>,
}

#[derive(Deserialize)]
struct RawCommand {
    /// The dispatcher key. A STRING rather than a char: emacs keys these with
    /// `" "` and `"C-a"` as readily as `"w"`, and a char would refuse the first
    /// spelling a user copies out of their old config.
    #[serde(default)]
    key: String,
    /// What the dispatcher row reads. Falls back to the key when absent —
    /// a row you cannot identify is worse than a terse one.
    #[serde(default)]
    description: String,
    #[serde(default)]
    section: Vec<RawSection>,
}

/// One configured agenda: a key to reach it by, a name, and the blocks it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommand {
    pub key: String,
    pub description: String,
    pub sections: Vec<Section>,
}

/// A parsed set, plus what was dropped getting there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommands {
    pub commands: Vec<CustomCommand>,
    /// Commands that parsed but were unusable, named. Rides back rather than
    /// being logged, for the module header's reason.
    pub skipped: Vec<String>,
}

impl ParsedCommands {
    /// The command a key names, if the set has one.
    ///
    /// Exact match, not a prefix or a fold: a dispatcher key is a keystroke,
    /// and `w` and `W` are two of them. Emacs agrees.
    pub fn by_key(&self, key: &str) -> Option<&CustomCommand> {
        self.commands.iter().find(|c| c.key == key)
    }
}

/// Why a command set could not be read. Mirrors `SectionError`'s three levels,
/// and for its reason: "you have not configured this" and "your configuration
/// is broken" are different problems with different fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    /// Unset or blank — the ordinary case, and not an error to the user.
    Unset,
    /// The TOML did not parse. Carries the parser's own message: the line and
    /// column in it are the whole value of reporting this at all.
    Malformed(String),
    /// It parsed, but nothing in it was usable.
    Empty,
}

impl CommandError {
    /// The one-line notice prefixed onto the fallback set's first header.
    pub fn notice(&self) -> Option<String> {
        match self {
            // Not a problem: no custom commands means the default agenda,
            // which is how nearly everyone runs.
            CommandError::Unset => None,
            CommandError::Malformed(e) => Some(format!(
                "⚠ org.agenda-custom-commands: {e} — using the default agenda"
            )),
            CommandError::Empty => Some(
                "⚠ org.agenda-custom-commands: no usable commands — using the default agenda"
                    .to_string(),
            ),
        }
    }
}

/// Parse the option's value into a command set.
///
/// `default_days` is `org.agenda-span`, used by any `when = "days"` section
/// that does not name its own — so the span option stays meaningful inside a
/// custom command exactly as it is inside `org.agenda-sections`.
pub fn parse(source: &str, default_days: u32) -> Result<ParsedCommands, CommandError> {
    if source.trim().is_empty() {
        return Err(CommandError::Unset);
    }
    let raw: RawSet =
        toml::from_str(source).map_err(|e| CommandError::Malformed(e.message().to_string()))?;

    let mut commands: Vec<CustomCommand> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for (i, c) in raw.command.into_iter().enumerate() {
        let key = c.key.trim().to_string();
        if key.is_empty() {
            // Named by position, since there is no key to name it by. A
            // command with no key is unreachable: the dispatcher is keyed, and
            // a row nothing can select is a row that is not there.
            skipped.push(format!("command {} has no `key`", i + 1));
            continue;
        }
        if commands.iter().any(|existing| existing.key == key) {
            // First wins, and the loser is NAMED. Silently shadowing would
            // give a menu two identical rows of which one does nothing —
            // indistinguishable from the feature being broken.
            skipped.push(format!("`{key}` is defined twice; the first one wins"));
            continue;
        }
        // A command's sections obey the section rules, including the skips:
        // one bad section inside a command costs that section, not the
        // command. The notices are namespaced by key so a user reading them
        // knows WHICH agenda complained.
        let mut section_skips: Vec<String> = Vec::new();
        let sections =
            agenda_sections::sections_from_raw(c.section, default_days, &mut section_skips);
        skipped.extend(section_skips.into_iter().map(|s| format!("`{key}`: {s}")));
        if sections.is_empty() {
            // A command with no usable sections would open an empty agenda,
            // which is the one thing this view must never do by accident.
            skipped.push(format!("`{key}` has no usable sections"));
            continue;
        }
        let description = if c.description.trim().is_empty() {
            key.clone()
        } else {
            c.description.trim().to_string()
        };
        commands.push(CustomCommand {
            key,
            description,
            sections,
        });
    }

    if commands.is_empty() {
        return Err(CommandError::Empty);
    }
    Ok(ParsedCommands { commands, skipped })
}

/// The sections a scan runs with, given what its view was opened for.
///
/// `args` is what OA.11a carries from the view: empty means the default
/// agenda, and a first element is a custom command's key. `fallback` is the
/// `org.agenda-sections` set the default agenda uses, already resolved.
///
/// One function so `begin` has a single call and cannot accidentally skip the
/// fallback — the AS.2 rule, and it matters more here because there are now
/// three ways to end up with no sections (unset option, broken option, a key
/// naming nothing) and all three must land on a working agenda that says what
/// happened.
pub fn resolve(
    args: &[String],
    source: &str,
    default_days: u32,
    fallback: Vec<Section>,
) -> Vec<Section> {
    let Some(key) = args.first().map(|k| k.trim()).filter(|k| !k.is_empty()) else {
        // No command named: the default agenda, with no complaint. A user who
        // never configured a custom command must see nothing about them.
        return fallback;
    };
    match parse(source, default_days) {
        Ok(set) => match set.by_key(key) {
            Some(command) => command.sections.clone(),
            // The key named nothing. This is NOT the same as a broken config,
            // and the notice says so: the set parsed, so what the user has is
            // a stale binding or a typo in the key, and naming the keys that
            // do exist is the shortest path to the fix.
            None => with_notice(
                fallback,
                Some(format!(
                    "⚠ org.agenda-custom-commands: no command `{key}` (have: {}) — using the default agenda",
                    set.commands
                        .iter()
                        .map(|c| c.key.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            ),
        },
        Err(e) => with_notice(fallback, e.notice()),
    }
}

/// Prefix `notice` onto the first section's title, or return the set unchanged.
///
/// The first section rather than a synthetic one of its own, because a section
/// with no rows renders no header: the host attaches a group title to a ROW, so
/// a notice-only section would be invisible in exactly the situation where a
/// user most needs to read it. Same shape and same reason as `agenda_sections`.
fn with_notice(mut sections: Vec<Section>, notice: Option<String>) -> Vec<Section> {
    if let (Some(notice), Some(first)) = (notice, sections.first_mut()) {
        first.title = format!("{notice} — {}", first.title);
    }
    sections
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::agenda::{default_sections, When};

    const SPAN: u32 = 7;

    const TWO: &str = r#"
[[command]]
key = "w"
description = "Waiting and Postponed"

  [[command.section]]
  title = "Waiting"
  when = "any"
  match = "-CANCELLED+WAITING|HOLD/!"

[[command]]
key = "r"
description = "Refile"

  [[command.section]]
  title = "Tasks to Refile"
  when = "any"
  match = "REFILE"

  [[command.section]]
  title = "This week"
  when = "days"
"#;

    #[test]
    fn a_set_parses_into_commands_in_order() {
        let set = parse(TWO, SPAN).unwrap();
        assert!(set.skipped.is_empty(), "got {:?}", set.skipped);
        assert_eq!(
            set.commands
                .iter()
                .map(|c| (c.key.as_str(), c.description.as_str(), c.sections.len()))
                .collect::<Vec<_>>(),
            [("w", "Waiting and Postponed", 1), ("r", "Refile", 2)]
        );
        // The sections went through the SAME validation as `agenda-sections`:
        // a `days` section with no `days` inherits `org.agenda-span`, which is
        // what keeps that option meaningful inside a custom command too.
        assert_eq!(set.commands[1].sections[1].filter.when, When::Days(SPAN));
        // …and the match reached the filter as parsed data, not a string.
        assert!(set.commands[0].sections[0].filter.r#match.is_some());
    }

    #[test]
    fn a_command_is_found_by_its_key_exactly() {
        let set = parse(TWO, SPAN).unwrap();
        assert_eq!(
            set.by_key("w").unwrap().description,
            "Waiting and Postponed"
        );
        // A dispatcher key is a keystroke: `w` and `W` are two of them.
        assert!(set.by_key("W").is_none());
        assert!(set.by_key("").is_none());
    }

    /// A description is optional; the key stands in. A row you cannot identify
    /// is worse than a terse one.
    #[test]
    fn a_command_without_a_description_is_named_by_its_key() {
        let set = parse(
            "[[command]]\nkey = \"x\"\n\n  [[command.section]]\n  title = \"T\"\n  when = \"any\"\n",
            SPAN,
        )
        .unwrap();
        assert_eq!(set.commands[0].description, "x");
    }

    /// One bad command is skipped and named; the rest survive. A typo must not
    /// cost the whole feature.
    #[test]
    fn an_unusable_command_is_skipped_and_named() {
        let set = parse(
            r#"
[[command]]
key = "g"
  [[command.section]]
  title = "Good"
  when = "any"

[[command]]
description = "no key"
  [[command.section]]
  title = "T"
  when = "any"

[[command]]
key = "n"
description = "no sections"

[[command]]
key = "b"
  [[command.section]]
  title = "Bad when"
  when = "someday"
"#,
            SPAN,
        )
        .unwrap();
        assert_eq!(set.commands.len(), 1, "got {:?}", set.commands);
        assert_eq!(set.commands[0].key, "g");
        // command 2 (no key), `n` (no sections), and `b` — which reports BOTH
        // the bad section and the resulting empty command, because the two are
        // different facts and the second is not implied by the first.
        assert!(
            set.skipped.iter().any(|s| s.contains("command 2")),
            "got {:?}",
            set.skipped
        );
        assert!(set.skipped.iter().any(|s| s.contains("`n`")));
        assert!(
            set.skipped
                .iter()
                .any(|s| s.contains("`b`") && s.contains("someday")),
            "a bad section inside a command names the command: {:?}",
            set.skipped
        );
    }

    /// A duplicate key is named rather than silently shadowed. Two identical
    /// rows of which one does nothing is indistinguishable from a broken menu.
    #[test]
    fn a_duplicate_key_keeps_the_first_and_says_so() {
        let set = parse(
            "[[command]]\nkey = \"w\"\ndescription = \"first\"\n\
             \n  [[command.section]]\n  title = \"A\"\n  when = \"any\"\n\
             \n[[command]]\nkey = \"w\"\ndescription = \"second\"\n\
             \n  [[command.section]]\n  title = \"B\"\n  when = \"any\"\n",
            SPAN,
        )
        .unwrap();
        assert_eq!(set.commands.len(), 1);
        assert_eq!(set.commands[0].description, "first");
        assert!(set.skipped[0].contains("twice"), "got {:?}", set.skipped);
    }

    /// Unset is not an error to the user — it is how nearly everyone runs.
    #[test]
    fn unset_and_blank_are_not_a_complaint() {
        assert_eq!(parse("", SPAN), Err(CommandError::Unset));
        assert_eq!(parse("  \n\n", SPAN), Err(CommandError::Unset));
        assert_eq!(CommandError::Unset.notice(), None);
    }

    #[test]
    fn a_set_with_nothing_usable_is_empty_not_malformed() {
        assert_eq!(parse("[[command]]\n", SPAN), Err(CommandError::Empty));
        assert_eq!(parse("other = 1\n", SPAN), Err(CommandError::Empty));
        assert!(matches!(
            parse("[[command]]\nkey = ", SPAN),
            Err(CommandError::Malformed(_))
        ));
    }

    // ---- `resolve`: which agenda a scan actually runs ----

    /// No command named is the default agenda, with nothing said about custom
    /// commands at all. A user who has never configured one must never read
    /// about them.
    #[test]
    fn no_command_named_is_the_default_agenda_in_silence() {
        let defaults = default_sections(SPAN);
        assert_eq!(
            resolve(&[], TWO, SPAN, defaults.clone()),
            defaults,
            "no args"
        );
        assert_eq!(
            resolve(&["  ".to_string()], TWO, SPAN, defaults.clone()),
            defaults,
            "a blank key is not a key"
        );
    }

    /// The headline: a named command's sections are what the scan runs.
    #[test]
    fn a_named_command_supplies_its_own_sections() {
        let sections = resolve(&["w".to_string()], TWO, SPAN, default_sections(SPAN));
        assert_eq!(
            sections
                .iter()
                .map(|s| s.title.as_str())
                .collect::<Vec<_>>(),
            ["Waiting"],
            "the command's sections, not the default set"
        );
        assert!(
            sections[0].filter.r#match.is_some(),
            "…carrying its match, which is the whole point of the command"
        );
    }

    /// A key that names nothing falls back AND says so, distinctly from a
    /// broken config: the set parsed, so this is a stale binding or a typo,
    /// and naming the keys that DO exist is the shortest path to the fix.
    #[test]
    fn an_unknown_key_falls_back_and_names_what_does_exist() {
        let defaults = default_sections(SPAN);
        let sections = resolve(&["zzz".to_string()], TWO, SPAN, defaults.clone());
        assert_eq!(sections.len(), defaults.len(), "the agenda still works");
        assert!(
            sections[0].title.contains("no command `zzz`"),
            "got {:?}",
            sections[0].title
        );
        assert!(
            sections[0].title.contains("w, r"),
            "the notice lists the keys that exist: {:?}",
            sections[0].title
        );
        // Only the first header: a notice repeated on every one is noise.
        assert_eq!(sections[1].title, defaults[1].title);
        // The FILTERS are untouched — a bad key costs you your layout, never
        // your rows.
        for (r, d) in sections.iter().zip(defaults.iter()) {
            assert_eq!(r.filter, d.filter);
        }
    }

    /// A malformed set costs you the command you asked for, never your rows —
    /// AS.2's rule, inherited whole.
    #[test]
    fn a_malformed_set_falls_back_to_the_default_agenda_and_says_so() {
        let defaults = default_sections(SPAN);
        let sections = resolve(
            &["w".to_string()],
            "[[command]]\nkey = ",
            SPAN,
            defaults.clone(),
        );
        assert_eq!(sections.len(), defaults.len());
        assert!(
            sections[0]
                .title
                .starts_with("⚠ org.agenda-custom-commands:"),
            "got {:?}",
            sections[0].title
        );
        assert!(
            sections[0].title.ends_with(&defaults[0].title),
            "…and still names the section it is: {:?}",
            sections[0].title
        );
        for (r, d) in sections.iter().zip(defaults.iter()) {
            assert_eq!(r.filter, d.filter);
        }
    }

    /// Asking for a command when NOTHING is configured is a complaint too —
    /// unlike asking for nothing, which is silent. The user pressed a key that
    /// meant something once; being told the option is unset is the fix.
    #[test]
    fn a_key_with_no_configuration_at_all_says_so() {
        let defaults = default_sections(SPAN);
        let sections = resolve(&["w".to_string()], "", SPAN, defaults.clone());
        assert_eq!(
            sections, defaults,
            "unset is not an error, so there is no notice even here — the \
             fallback IS the answer"
        );
    }
}

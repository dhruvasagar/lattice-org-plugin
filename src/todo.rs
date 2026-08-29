//! A headline's *contents* — keyword, priority, title, tags — as pure line
//! logic.
//!
//! `headline.rs` is about a headline's place in the tree (level, subtree,
//! siblings). This is about what is written on the line:
//!
//! ```org
//! ** TODO [#A] Ship the thing :work:urgent:
//! ```
//!
//! Same argument for text over the parse tree as `headline.rs` makes, and one
//! more that is specific to this file: the TODO keywords are **user
//! configuration** (`org.todo-keywords`), so which word is a keyword is not a
//! property of the grammar at all. `TODO` is only a keyword because the option
//! says so; with `#+TODO: PROPOSED ACCEPTED` it is plain title text. A parse
//! tree cannot answer that, and org's own implementation does the same
//! string comparison.
//!
//! Everything here round-trips through [`parse`] and [`render`], so an
//! operation is "change one field and re-render" rather than a substring
//! rewrite. That is what keeps `TODO` → `DONE` from disturbing the tags at the
//! other end of the line.

/// The parts of a headline line, borrowed from it.
#[derive(Debug, PartialEq, Eq)]
pub struct Headline<'a> {
    /// The stars, without the separating space: `"**"`.
    pub stars: &'a str,
    /// A TODO keyword, only if it is one of the configured set.
    pub keyword: Option<&'a str>,
    /// The priority letter from `[#A]`.
    pub priority: Option<char>,
    /// What is left after the keyword and priority, tags removed, trimmed.
    pub title: &'a str,
    /// Tags from a trailing `:a:b:`, in order.
    pub tags: Vec<&'a str>,
}

// ── TK.2: the `org-todo-keywords` grammar ────────────────────────────────────

/// What a sequence line declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceKind {
    /// `sequence:` — a workflow, cycled in order.
    Sequence,
    /// `type:` — a set of alternatives rather than a progression.
    Type,
}

/// What org logs when a state is entered or left.
///
/// **Parsed and deliberately inert.** Acting on these means writing
/// `:LOGBOOK:` notes and timestamps on every state change, which is its own
/// slice with its own tests. Parsing them now is what lets an emacs
/// configuration be pasted with nothing silently misread — the failure this
/// replaces is `WAITING(w@/!)` becoming a keyword whose *name* is
/// `WAITING(w@/!)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Logging {
    /// `@` — prompt for a note when the state is entered.
    pub note_on_entry: bool,
    /// `!` — record a timestamp when the state is entered.
    pub stamp_on_entry: bool,
    /// `/@` — prompt for a note when the state is left.
    pub note_on_exit: bool,
    /// `/!` — record a timestamp when the state is left.
    pub stamp_on_exit: bool,
}

impl Logging {
    fn is_empty(self) -> bool {
        self == Logging::default()
    }
}

/// One TODO state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyword {
    /// The bare word as it appears on a headline — `WAITING`.
    pub name: String,
    /// The fast-select key from `(w)`, if any (TK.6).
    pub key: Option<char>,
    /// The logging spec from `(@/!)`.
    pub logging: Logging,
    /// Right of the `|` in its sequence.
    pub done: bool,
    /// Which declaration line it came from, and of what kind.
    pub kind: SequenceKind,
    /// 0-based index of the sequence that declared it.
    pub sequence: usize,
}

/// A parsed `org.todo-keywords`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keywords {
    pub all: Vec<Keyword>,
    /// One message per refused line or keyword, for a one-shot `warn` at
    /// load. Never fatal: one bad line must not cost a user every keyword.
    pub problems: Vec<String>,
}

impl Keywords {
    pub fn names(&self) -> Vec<String> {
        self.all.iter().map(|k| k.name.clone()).collect()
    }

    /// `(not_done, done)` names, in declaration order.
    pub fn split(&self) -> (Vec<String>, Vec<String>) {
        let mut nd = Vec::new();
        let mut d = Vec::new();
        for k in &self.all {
            if k.done { &mut d } else { &mut nd }.push(k.name.clone());
        }
        (nd, d)
    }
}

/// Parse emacs' `org-todo-keywords`, one sequence per line.
///
/// ```text
/// sequence: TODO(t) NEXT(n) | DONE(d)
/// sequence: WAITING(w@/!) HOLD(h@/!) | CANCELLED(c@/!) PHONE MEETING
/// type: PROJECT TO-READ READING(!/!) TO-WATCH WATCHING(!/!)
/// ```
///
/// A line with no `sequence:` / `type:` prefix is a `sequence:`, so the old
/// flat `"TODO | DONE"` spelling still parses to exactly what it always meant.
/// That is what lets this replace the old option rather than sit beside it.
pub fn parse_todo_keywords(spec: &str) -> Keywords {
    let mut out = Keywords::default();
    let mut seen_keys: Vec<(char, String)> = Vec::new();

    for (line_no, raw) in spec.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (kind, body) = match line.split_once(':') {
            Some((head, rest)) if head.trim().eq_ignore_ascii_case("sequence") => {
                (SequenceKind::Sequence, rest)
            }
            Some((head, rest)) if head.trim().eq_ignore_ascii_case("type") => {
                (SequenceKind::Type, rest)
            }
            // A bare list is a sequence — the old flat spelling.
            _ => (SequenceKind::Sequence, line),
        };

        let seq = out.all.last().map(|k| k.sequence + 1).unwrap_or(0);
        let mut past_bar = false;
        let mut any = false;
        for word in body.split_whitespace() {
            if word == "|" {
                past_bar = true;
                continue;
            }
            match parse_keyword(word, past_bar, kind, seq) {
                Ok(kw) => {
                    if out.all.iter().any(|k| k.name == kw.name) {
                        out.problems.push(format!(
                            "line {}: duplicate keyword `{}` — keeping the first",
                            line_no + 1,
                            kw.name
                        ));
                        continue;
                    }
                    let clash = kw.key.and_then(|key| {
                        seen_keys
                            .iter()
                            .find(|(k, _)| *k == key)
                            .map(|(_, owner)| (key, owner.clone()))
                    });
                    let mut kw = kw;
                    if let Some((key, owner)) = clash {
                        out.problems.push(format!(
                            "line {}: fast-select key `{}` already belongs to `{}` — \
                             `{}` keeps its state but loses the shortcut",
                            line_no + 1,
                            key,
                            owner,
                            kw.name
                        ));
                        kw.key = None;
                    } else if let Some(key) = kw.key {
                        seen_keys.push((key, kw.name.clone()));
                    }
                    out.all.push(kw);
                    any = true;
                }
                Err(why) => out.problems.push(format!("line {}: {why}", line_no + 1)),
            }
        }
        if !any {
            out.problems
                .push(format!("line {}: no keywords", line_no + 1));
        }
    }
    out
}

/// One `WORD`, `WORD(k)`, `WORD(@/!)` or `WORD(k@/!)`.
fn parse_keyword(
    word: &str,
    done: bool,
    kind: SequenceKind,
    sequence: usize,
) -> Result<Keyword, String> {
    let (name, spec) = match word.split_once('(') {
        Some((n, rest)) => match rest.strip_suffix(')') {
            Some(inner) => (n, Some(inner)),
            None => return Err(format!("`{word}`: unclosed `(`")),
        },
        None => (word, None),
    };
    if name.is_empty() {
        return Err(format!("`{word}`: no keyword before `(`"));
    }
    // A keyword has to be able to match a headline's first expr, so it is a
    // plain word. Refusing here is also what keeps TK.4's generated query
    // from ever interpolating something that needs escaping.
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "`{name}`: a keyword must be letters, digits, `_` or `-` — it has to \
             match the first word of a headline"
        ));
    }

    let mut key = None;
    let mut logging = Logging::default();
    if let Some(spec) = spec {
        let (entry, exit) = match spec.split_once('/') {
            Some((a, b)) => (a, Some(b)),
            None => (spec, None),
        };
        // The fast-select key is a leading char that is not a log marker.
        let mut entry_chars = entry.chars().peekable();
        let leading = entry_chars.peek().copied();
        if let Some(c) = leading {
            if c != '@' && c != '!' {
                key = Some(c);
                entry_chars.next();
            }
        }
        for c in entry_chars {
            match c {
                '@' => logging.note_on_entry = true,
                '!' => logging.stamp_on_entry = true,
                other => {
                    return Err(format!("`{word}`: `{other}` is not `@`, `!` or `/`"));
                }
            }
        }
        for c in exit.unwrap_or("").chars() {
            match c {
                '@' => logging.note_on_exit = true,
                '!' => logging.stamp_on_exit = true,
                other => {
                    return Err(format!("`{word}`: `{other}` is not `@` or `!` after `/`"));
                }
            }
        }
        if key.is_none() && logging.is_empty() {
            return Err(format!("`{word}`: `()` declares nothing"));
        }
    }

    Ok(Keyword {
        name: name.to_string(),
        key,
        logging,
        done,
        kind,
        sequence,
    })
}

// ── TK.5: `org.todo-keyword-styles` ──────────────────────────────────────────

/// One user-declared per-keyword style.
///
/// The emacs shape this spells is
/// `("WAITING" :foreground "orange" :weight bold)`; here it is
/// `WAITING: fg=orange bold`, one keyword per line, matching how
/// `org.todo-keywords` is written rather than importing elisp's punctuation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeywordStyle {
    /// A palette key (`orange`) or a literal `#rrggbb`.
    pub fg: Option<String>,
    pub bg: Option<String>,
    /// `Some(true)` sets, `Some(false)` CLEARS an inherited one, `None`
    /// leaves it alone — the three-way distinction the theme seam needs so a
    /// keyword inheriting a bold parent can turn bold off.
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub dim: Option<bool>,
}

/// Parse `org.todo-keyword-styles`.
///
/// ```text
/// TODO: fg=red bold
/// WAITING: fg=orange italic
/// CANCELLED: fg=overlay dim no-bold
/// ```
///
/// Returns `(keyword, style)` pairs plus one message per refused line. A bad
/// line costs itself and nothing else, for the same proportionality reason a
/// bad conceal rule does: this is cosmetic configuration, and losing every
/// colour over one typo is the disproportionate answer.
pub fn parse_keyword_styles(spec: &str) -> (Vec<(String, KeywordStyle)>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for (i, raw) in spec.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, rest)) = line.split_once(':') else {
            problems.push(format!("line {}: expected `KEYWORD: …`", i + 1));
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            problems.push(format!("line {}: no keyword before `:`", i + 1));
            continue;
        }
        let mut st = KeywordStyle::default();
        let mut bad = None;
        for word in rest.split_whitespace() {
            match word {
                "bold" => st.bold = Some(true),
                "no-bold" => st.bold = Some(false),
                "italic" => st.italic = Some(true),
                "no-italic" => st.italic = Some(false),
                "underline" => st.underline = Some(true),
                "no-underline" => st.underline = Some(false),
                "dim" => st.dim = Some(true),
                "no-dim" => st.dim = Some(false),
                _ => match word.split_once('=') {
                    Some(("fg", v)) if !v.is_empty() => st.fg = Some(v.to_string()),
                    Some(("bg", v)) if !v.is_empty() => st.bg = Some(v.to_string()),
                    _ => {
                        bad = Some(word.to_string());
                        break;
                    }
                },
            }
        }
        if let Some(w) = bad {
            problems.push(format!(
                "line {}: `{w}` is not `fg=…`, `bg=…` or a modifier",
                i + 1
            ));
            continue;
        }
        if st == KeywordStyle::default() {
            problems.push(format!("line {}: `{name}` declares no style", i + 1));
            continue;
        }
        out.push((name.to_string(), st));
    }
    (out, problems)
}

/// The configured keyword list, e.g. `"TODO NEXT | DONE"`.
///
/// The `|` is org's separator between "not done" and "done" states. It matters
/// to the agenda, not to cycling, so it is dropped here and the slice that
/// needs the distinction can re-read the option.
/// The keyword list split at `|` into (not-done, done).
///
/// [`parse_keywords`] drops the separator because cycling does not care; the
/// AGENDA does — org hides completed entries by default, and "completed"
/// means "the keyword is on the right of the bar". A spec with no `|` has no
/// done states at all, which is org's rule and not a degradation: the user
/// who writes `#+TODO: A B C` meant three open states.
///
/// TK.2: a thin accessor over [`parse_todo_keywords`] rather than a second
/// parser. Two implementations of "what is a keyword" is exactly the drift
/// this file's header warns about for the parse tree, one layer down.
pub fn split_keywords(spec: &str) -> (Vec<String>, Vec<String>) {
    parse_todo_keywords(spec).split()
}

/// Every keyword's name, in declaration order. See [`split_keywords`].
pub fn parse_keywords(spec: &str) -> Vec<String> {
    parse_todo_keywords(spec).names()
}

/// Split a headline into its parts, or `None` if `line` is not a headline.
pub fn parse<'a>(line: &'a str, keywords: &[String]) -> Option<Headline<'a>> {
    let level = crate::headline::headline_level(line)?;
    let (stars, rest) = line.split_at(level);
    let mut rest = rest.strip_prefix(' ').unwrap_or(rest);

    // Tags first, from the right: `:a:b:` at the end of the line. Taking them
    // off before reading the title is what stops a tagless title ending in a
    // colon from being misread.
    let mut tags = Vec::new();
    let trimmed = rest.trim_end();
    if let Some(open) = tags_start(trimmed) {
        tags = trimmed[open..]
            .trim_matches(':')
            .split(':')
            .filter(|t| !t.is_empty())
            .collect();
        rest = &rest[..open];
    }

    let keyword = first_word(rest).filter(|w| keywords.iter().any(|k| k == w));
    if let Some(k) = keyword {
        rest = rest[k.len()..].trim_start();
    }

    let priority = read_priority(rest);
    if priority.is_some() {
        rest = rest[5.min(rest.len())..].trim_start();
    }

    Some(Headline {
        stars,
        keyword,
        priority,
        title: rest.trim(),
        tags,
    })
}

/// Write a headline back out. The inverse of [`parse`] for any line it parsed.
pub fn render(h: &Headline) -> String {
    // Collect the present segments and join with single spaces, rather than
    // appending a trailing space after each. Appending is what produced
    // `**** NEXT  :tag:` — two spaces — for a headline with a keyword and no
    // title, and a round-trip test caught it. Empty segments must not leave a
    // separator behind.
    let mut parts: Vec<String> = vec![h.stars.to_string()];
    if let Some(k) = h.keyword {
        parts.push(k.to_string());
    }
    if let Some(p) = h.priority {
        parts.push(format!("[#{p}]"));
    }
    if !h.title.is_empty() {
        parts.push(h.title.to_string());
    }
    if !h.tags.is_empty() {
        // One space before the tag block. Org right-aligns tags to a column;
        // that is a display convention rather than a file format, and aligning
        // here would rewrite the line every time the title changed.
        parts.push(format!(":{}:", h.tags.join(":")));
    }
    parts.join(" ")
}

/// `Some(byte index)` of a trailing `:a:b:` tag block.
///
/// Requires at least `:x:` and that the block is preceded by whitespace, so a
/// title like `see foo:bar:` — no leading space before the colon run — is left
/// alone rather than silently becoming two tags.
fn tags_start(trimmed: &str) -> Option<usize> {
    if !trimmed.ends_with(':') {
        return None;
    }
    let start = trimmed.rfind(char::is_whitespace).map(|i| i + 1)?;
    let block = &trimmed[start..];
    let inner = block.strip_prefix(':')?.strip_suffix(':')?;
    if inner.is_empty()
        || !inner
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '%' | ':'))
    {
        return None;
    }
    Some(start)
}

fn first_word(s: &str) -> Option<&str> {
    let w = s.split_whitespace().next()?;
    (!w.is_empty()).then_some(w)
}

/// The letter in a leading `[#A]`, uppercased by org convention.
fn read_priority(s: &str) -> Option<char> {
    let b = s.as_bytes();
    if b.len() >= 4 && &b[..2] == b"[#" && b[3] == b']' {
        let c = b[2] as char;
        return c.is_ascii_alphabetic().then(|| c.to_ascii_uppercase());
    }
    None
}

/// Step through a cyclic sequence: `None` → first → … → last → `None`.
///
/// The empty state is part of the cycle, in both directions, so a keyword can
/// always be cleared without deleting text by hand — org's own behaviour.
fn step<'a>(current: Option<&str>, items: &'a [String], forward: bool) -> Option<&'a str> {
    if items.is_empty() {
        return None;
    }
    let at = current.and_then(|c| items.iter().position(|k| k == c));
    let next = match (at, forward) {
        (None, true) => Some(0),
        (None, false) => Some(items.len() - 1),
        (Some(i), true) if i + 1 < items.len() => Some(i + 1),
        (Some(_), true) => None,
        (Some(0), false) => None,
        (Some(i), false) => Some(i - 1),
    };
    next.map(|i| items[i].as_str())
}

/// Advance the TODO keyword on `line`. `None` if it is not a headline.
pub fn cycle_keyword(line: &str, keywords: &[String], forward: bool) -> Option<String> {
    let mut h = parse(line, keywords)?;
    h.keyword = step(h.keyword, keywords, forward);
    Some(render(&h))
}

/// TK.6 — set `line`'s keyword to `name`, or clear it when `name` is empty.
///
/// The counterpart to [`cycle_keyword`] for fast select: cycling walks the
/// sequence, this jumps straight to the state you chose. Both go through
/// [`parse`] and [`render`], so setting a keyword cannot disturb the
/// priority or the tags at the other end of the line.
///
/// `None` when `line` is not a headline, or when `name` is not a configured
/// keyword — a menu row can only offer configured states, so the second case
/// means the option changed between the menu opening and the key landing, and
/// writing an unknown word onto the headline would be worse than doing
/// nothing.
pub fn set_keyword(line: &str, keywords: &[String], name: &str) -> Option<String> {
    let mut h = parse(line, keywords)?;
    if name.is_empty() {
        h.keyword = None;
        return Some(render(&h));
    }
    let found = keywords.iter().find(|k| k.as_str() == name)?;
    h.keyword = Some(found.as_str());
    Some(render(&h))
}

/// Advance the priority on `line` through `A` … `highest`, then off.
pub fn cycle_priority(
    line: &str,
    keywords: &[String],
    highest: char,
    forward: bool,
) -> Option<String> {
    let letters: Vec<String> = ('A'..=highest.to_ascii_uppercase())
        .map(|c| c.to_string())
        .collect();
    let mut h = parse(line, keywords)?;
    let current = h.priority.map(|c| c.to_string());
    h.priority = step(current.as_deref(), &letters, forward).and_then(|s| s.chars().next());
    Some(render(&h))
}

/// Replace the tag block on `line` with `tags` (a `:a:b:` or `a b` string).
///
/// An empty `tags` removes the block, which is how a tag is deleted.
pub fn set_tags(line: &str, keywords: &[String], tags: &str) -> Option<String> {
    let mut h = parse(line, keywords)?;
    let split: Vec<&str> = tags
        .split(|c: char| c == ':' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    h.tags = split;
    Some(render(&h))
}

/// The current tags as the `:a:b:` string the prompt is pre-filled with.
pub fn tags_string(h: &Headline) -> String {
    if h.tags.is_empty() {
        return String::new();
    }
    format!(":{}:", h.tags.join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kw() -> Vec<String> {
        parse_keywords("TODO NEXT | DONE")
    }

    // ---- TK.6: setting a specific state ----

    #[test]
    fn tk6_set_keyword_jumps_straight_to_a_state() {
        let kw = parse_keywords("TODO NEXT | DONE");
        assert_eq!(
            set_keyword("** TODO ship it", &kw, "DONE").as_deref(),
            Some("** DONE ship it")
        );
        // From no state to a state.
        assert_eq!(
            set_keyword("** ship it", &kw, "NEXT").as_deref(),
            Some("** NEXT ship it")
        );
    }

    /// Clearing a state IS a state — a menu that can set every keyword but
    /// never remove one is a one-way door.
    #[test]
    fn tk6_an_empty_name_clears_the_state() {
        let kw = parse_keywords("TODO | DONE");
        assert_eq!(
            set_keyword("** TODO ship it", &kw, "").as_deref(),
            Some("** ship it")
        );
    }

    /// The round-trip property `cycle_keyword` relies on, asserted for the
    /// jump too: setting a state must not disturb the priority or the tags
    /// at the other end of the line.
    #[test]
    fn tk6_setting_a_state_leaves_the_rest_of_the_headline_alone() {
        let kw = parse_keywords("TODO | DONE");
        assert_eq!(
            set_keyword("** TODO [#A] ship it :work:urgent:", &kw, "DONE").as_deref(),
            Some("** DONE [#A] ship it :work:urgent:")
        );
    }

    /// A menu row can only offer configured states, so an unknown name means
    /// the option changed between the menu opening and the key landing.
    /// Writing an unknown word onto the headline would be worse than nothing.
    #[test]
    fn tk6_an_unconfigured_name_is_refused() {
        let kw = parse_keywords("TODO | DONE");
        assert!(set_keyword("** TODO ship it", &kw, "WAITING").is_none());
    }

    #[test]
    fn tk6_a_line_that_is_not_a_headline_is_refused() {
        let kw = parse_keywords("TODO | DONE");
        assert!(set_keyword("plain prose", &kw, "DONE").is_none());
    }

    // ---- TK.5: `org.todo-keyword-styles` ----

    #[test]
    fn tk5_the_emacs_config_transcribes_line_by_line() {
        // Dhruva's own org-todo-keyword-faces, in this spelling.
        let (styles, problems) = parse_keyword_styles(
            "TODO: fg=red bold\n\
             NEXT: fg=blue bold\n\
             DONE: fg=green bold\n\
             WAITING: fg=orange bold\n\
             CANCELLED: fg=overlay dim",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(styles.len(), 5);
        let get = |n: &str| styles.iter().find(|(k, _)| k == n).unwrap().1.clone();
        assert_eq!(get("TODO").fg.as_deref(), Some("red"));
        assert_eq!(get("TODO").bold, Some(true));
        assert_eq!(get("CANCELLED").dim, Some(true));
        assert_eq!(get("CANCELLED").fg.as_deref(), Some("overlay"));
    }

    /// The three-way modifier matters: a keyword inheriting a bold parent
    /// must be able to turn bold OFF, which a plain bool cannot express.
    #[test]
    fn tk5_a_modifier_can_be_cleared_not_just_set() {
        let (styles, _) = parse_keyword_styles("DONE: no-bold no-dim italic");
        let st = &styles[0].1;
        assert_eq!(st.bold, Some(false), "cleared, not merely unset");
        assert_eq!(st.dim, Some(false));
        assert_eq!(st.italic, Some(true));
        assert_eq!(st.underline, None, "untouched stays None");
    }

    #[test]
    fn tk5_a_literal_colour_is_allowed_beside_palette_keys() {
        let (styles, problems) = parse_keyword_styles("TODO: fg=#ff8800\nNEXT: fg=blue");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(styles[0].1.fg.as_deref(), Some("#ff8800"));
        assert_eq!(styles[1].1.fg.as_deref(), Some("blue"));
    }

    #[test]
    fn tk5_a_bad_line_costs_only_itself() {
        let (styles, problems) =
            parse_keyword_styles("TODO: fg=red\nNEXT: sparkly\nDONE: fg=green");
        assert_eq!(
            styles.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["TODO", "DONE"]
        );
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("sparkly"), "{problems:?}");
    }

    #[test]
    fn tk5_a_line_that_declares_nothing_is_refused() {
        let (styles, problems) = parse_keyword_styles("TODO:");
        assert!(styles.is_empty());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn tk5_blank_and_comment_lines_are_ignored() {
        let (styles, problems) = parse_keyword_styles("\n# my colours\nTODO: fg=red\n\n");
        assert_eq!(styles.len(), 1);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn tk5_an_unset_option_is_no_styles_and_no_complaints() {
        let (styles, problems) = parse_keyword_styles("");
        assert!(styles.is_empty());
        assert!(problems.is_empty());
    }

    // ---- TK.2: the `org-todo-keywords` grammar ----

    /// Dhruva's own emacs configuration, transcribed one sequence per
    /// line. This is the fixture because it is the thing that did not
    /// work: pasted into the old flat option, `WAITING(w@/!)` became a
    /// keyword whose NAME was `WAITING(w@/!)` and matched nothing.
    const REAL: &str = "\
sequence: TODO(t) NEXT(n) | DONE(d)
sequence: WAITING(w@/!) HOLD(h@/!) | CANCELLED(c@/!) PHONE MEETING
type: PROJECT TO-READ READING(!/!) TO-WATCH WATCHING(!/!)";

    #[test]
    fn tk2_the_real_configuration_parses_whole() {
        let k = parse_todo_keywords(REAL);
        assert!(k.problems.is_empty(), "{:?}", k.problems);
        assert_eq!(
            k.names(),
            [
                "TODO",
                "NEXT",
                "DONE",
                "WAITING",
                "HOLD",
                "CANCELLED",
                "PHONE",
                "MEETING",
                "PROJECT",
                "TO-READ",
                "READING",
                "TO-WATCH",
                "WATCHING"
            ]
        );
    }

    #[test]
    fn tk2_the_bar_decides_done() {
        let (not_done, done) = parse_todo_keywords(REAL).split();
        assert_eq!(done, ["DONE", "CANCELLED", "PHONE", "MEETING"]);
        assert!(not_done.contains(&"TODO".to_string()));
        assert!(not_done.contains(&"WAITING".to_string()));
        // A `type:` line has no bar, so nothing on it is done.
        assert!(not_done.contains(&"PROJECT".to_string()));
        assert!(not_done.contains(&"WATCHING".to_string()));
    }

    #[test]
    fn tk2_type_is_distinguished_from_sequence() {
        let k = parse_todo_keywords(REAL);
        let by = |n: &str| k.all.iter().find(|x| x.name == n).unwrap().kind;
        assert_eq!(by("TODO"), SequenceKind::Sequence);
        assert_eq!(by("PROJECT"), SequenceKind::Type);
    }

    #[test]
    fn tk2_fast_select_keys_are_recovered() {
        let k = parse_todo_keywords(REAL);
        let key = |n: &str| k.all.iter().find(|x| x.name == n).unwrap().key;
        assert_eq!(key("TODO"), Some('t'));
        assert_eq!(key("NEXT"), Some('n'));
        assert_eq!(key("DONE"), Some('d'));
        assert_eq!(key("WAITING"), Some('w'));
        assert_eq!(key("CANCELLED"), Some('c'));
        // No `(k)` at all, and `(!/!)` which is logging only.
        assert_eq!(key("PHONE"), None);
        assert_eq!(key("READING"), None);
    }

    /// The specific failure TK.2 exists to end: the logging spec must
    /// not end up part of the keyword's name.
    #[test]
    fn tk2_logging_specs_are_parsed_and_not_part_of_the_name() {
        let k = parse_todo_keywords(REAL);
        let w = k.all.iter().find(|x| x.name == "WAITING").unwrap();
        assert_eq!(w.name, "WAITING", "not `WAITING(w@/!)`");
        assert_eq!(
            w.logging,
            Logging {
                note_on_entry: true,
                stamp_on_exit: true,
                ..Default::default()
            }
        );
        let r = k.all.iter().find(|x| x.name == "READING").unwrap();
        assert_eq!(
            r.logging,
            Logging {
                stamp_on_entry: true,
                stamp_on_exit: true,
                ..Default::default()
            }
        );
    }

    /// The old flat spelling still means exactly what it meant, which
    /// is what lets this replace the option rather than sit beside it.
    #[test]
    fn tk2_the_old_flat_spelling_is_unchanged() {
        assert_eq!(parse_keywords("TODO NEXT | DONE"), ["TODO", "NEXT", "DONE"]);
        assert_eq!(
            split_keywords("TODO NEXT | DONE"),
            (
                vec!["TODO".to_string(), "NEXT".to_string()],
                vec!["DONE".to_string()]
            )
        );
    }

    #[test]
    fn tk2_a_bad_keyword_costs_only_itself() {
        let k = parse_todo_keywords("sequence: TODO BAD(unclosed NEXT | DONE");
        assert_eq!(k.names(), ["TODO", "NEXT", "DONE"]);
        assert_eq!(k.problems.len(), 1);
        assert!(k.problems[0].contains("unclosed"), "{:?}", k.problems);
    }

    #[test]
    fn tk2_a_keyword_that_could_never_match_a_headline_is_refused() {
        let k = parse_todo_keywords("sequence: TODO IN[PROGRESS] | DONE");
        assert_eq!(k.names(), ["TODO", "DONE"]);
        assert_eq!(k.problems.len(), 1);
    }

    #[test]
    fn tk2_a_duplicate_keyword_keeps_the_first() {
        let k = parse_todo_keywords("sequence: TODO | DONE\nsequence: TODO NEXT | DONE");
        assert_eq!(k.names(), ["TODO", "DONE", "NEXT"]);
        assert_eq!(k.problems.len(), 2, "{:?}", k.problems);
        assert!(k.problems.iter().all(|p| p.contains("duplicate")));
    }

    /// A key clash costs the shortcut, never the state — a keyword the
    /// file already contains must stay reachable.
    #[test]
    fn tk2_a_duplicate_fast_select_key_keeps_the_state() {
        let k = parse_todo_keywords("sequence: TODO(t) TASK(t) | DONE(d)");
        assert_eq!(k.names(), ["TODO", "TASK", "DONE"]);
        let key = |n: &str| k.all.iter().find(|x| x.name == n).unwrap().key;
        assert_eq!(key("TODO"), Some('t'));
        assert_eq!(key("TASK"), None, "loses the shortcut, keeps the state");
        assert_eq!(k.problems.len(), 1);
    }

    #[test]
    fn tk2_blank_and_comment_lines_are_ignored() {
        let k = parse_todo_keywords("\n# my states\nsequence: TODO | DONE\n\n");
        assert_eq!(k.names(), ["TODO", "DONE"]);
        assert!(k.problems.is_empty(), "{:?}", k.problems);
    }

    #[test]
    fn tk2_an_empty_parens_declares_nothing_and_says_so() {
        let k = parse_todo_keywords("sequence: TODO() | DONE");
        assert_eq!(k.names(), ["DONE"]);
        assert_eq!(k.problems.len(), 1);
    }

    #[test]
    fn tk2_a_sequence_with_no_bar_has_no_done_states() {
        // org's rule, not a degradation: `A B C` means three open states.
        let (not_done, done) = parse_todo_keywords("type: A B C").split();
        assert_eq!(not_done, ["A", "B", "C"]);
        assert!(done.is_empty());
    }

    #[test]
    fn the_separator_is_not_a_keyword() {
        assert_eq!(kw(), ["TODO", "NEXT", "DONE"]);
        assert_eq!(parse_keywords("  TODO   DONE  "), ["TODO", "DONE"]);
        assert!(parse_keywords("").is_empty());
    }

    #[test]
    fn parses_every_part_of_a_full_headline() {
        let h = parse("** TODO [#A] Ship it :work:urgent:", &kw()).expect("headline");
        assert_eq!(h.stars, "**");
        assert_eq!(h.keyword, Some("TODO"));
        assert_eq!(h.priority, Some('A'));
        assert_eq!(h.title, "Ship it");
        assert_eq!(h.tags, ["work", "urgent"]);
    }

    /// A word is only a keyword because the OPTION says so. With a different
    /// `org.todo-keywords`, `TODO` is ordinary title text — which is the whole
    /// reason this cannot come from the grammar.
    #[test]
    fn a_word_is_a_keyword_only_if_configured() {
        let custom = parse_keywords("PROPOSED ACCEPTED");
        let h = parse("* TODO not a keyword here", &custom).expect("headline");
        assert_eq!(h.keyword, None);
        assert_eq!(h.title, "TODO not a keyword here");

        let h = parse("* PROPOSED design", &custom).expect("headline");
        assert_eq!(h.keyword, Some("PROPOSED"));
        assert_eq!(h.title, "design");
    }

    /// The case that makes tag detection worth writing carefully: a colon in
    /// running text must not become a tag block.
    #[test]
    fn a_colon_in_the_title_is_not_a_tag_block() {
        let h = parse("* see foo:bar:", &kw()).expect("headline");
        assert!(h.tags.is_empty(), "no whitespace before the colon run");
        assert_eq!(h.title, "see foo:bar:");

        // Nor is prose that merely ends in a colon.
        let h = parse("* Note:", &kw()).expect("headline");
        assert!(h.tags.is_empty());
        assert_eq!(h.title, "Note:");

        // But a real block is found.
        let h = parse("* Title :a:", &kw()).expect("headline");
        assert_eq!(h.tags, ["a"]);
        assert_eq!(h.title, "Title");
    }

    #[test]
    fn parse_and_render_round_trip() {
        for line in [
            "* Plain",
            "** TODO Something",
            "*** DONE [#C] Done thing :x:",
            "* [#B] Priority only",
            "**** NEXT :tag:",
        ] {
            let h = parse(line, &kw()).expect("headline");
            assert_eq!(render(&h), line, "round trip of {line:?}");
        }
    }

    /// The empty state is part of the cycle in BOTH directions, so a keyword
    /// can always be cleared without editing text by hand.
    #[test]
    fn keyword_cycles_through_none_at_both_ends() {
        let k = kw();
        let mut line = "* Task".to_string();
        for want in ["* TODO Task", "* NEXT Task", "* DONE Task", "* Task"] {
            line = cycle_keyword(&line, &k, true).expect("headline");
            assert_eq!(line, want);
        }
        // Backwards is the mirror image.
        for want in ["* DONE Task", "* NEXT Task", "* TODO Task", "* Task"] {
            line = cycle_keyword(&line, &k, false).expect("headline");
            assert_eq!(line, want);
        }
    }

    /// Cycling the keyword must leave priority and tags exactly where they
    /// were — the reason this goes through parse/render instead of a substring
    /// replace.
    #[test]
    fn cycling_a_keyword_disturbs_nothing_else() {
        let k = kw();
        let out = cycle_keyword("** TODO [#A] Ship it :work:urgent:", &k, true).unwrap();
        assert_eq!(out, "** NEXT [#A] Ship it :work:urgent:");
    }

    #[test]
    fn priority_cycles_and_clears() {
        let k = kw();
        let mut line = "* TODO Task".to_string();
        for want in [
            "* TODO [#A] Task",
            "* TODO [#B] Task",
            "* TODO [#C] Task",
            "* TODO Task",
        ] {
            line = cycle_priority(&line, &k, 'C', true).expect("headline");
            assert_eq!(line, want);
        }
    }

    #[test]
    fn tags_are_set_parsed_loosely_and_removed_when_empty() {
        let k = kw();
        // Accepts either the `:a:b:` form or plain words, since the prompt
        // takes free text.
        assert_eq!(
            set_tags("* Task", &k, "work urgent").as_deref(),
            Some("* Task :work:urgent:")
        );
        assert_eq!(
            set_tags("* Task", &k, ":work:urgent:").as_deref(),
            Some("* Task :work:urgent:")
        );
        // Replacing, not appending.
        assert_eq!(
            set_tags("* Task :old:", &k, "new").as_deref(),
            Some("* Task :new:")
        );
        // Empty clears the block entirely.
        assert_eq!(set_tags("* Task :old:", &k, "").as_deref(), Some("* Task"));
    }

    #[test]
    fn nothing_here_touches_a_line_that_is_not_a_headline() {
        let k = kw();
        assert!(parse("just prose", &k).is_none());
        assert!(cycle_keyword("just prose", &k, true).is_none());
        assert!(cycle_priority("just prose", &k, 'C', true).is_none());
        assert!(set_tags("just prose", &k, "x").is_none());
    }

    #[test]
    fn tags_string_prefills_the_prompt() {
        let k = kw();
        let h = parse("* Task :a:b:", &k).unwrap();
        assert_eq!(tags_string(&h), ":a:b:");
        let h = parse("* Task", &k).unwrap();
        assert_eq!(tags_string(&h), "");
    }
}

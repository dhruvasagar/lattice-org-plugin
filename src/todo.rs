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
pub fn split_keywords(spec: &str) -> (Vec<String>, Vec<String>) {
    let mut not_done = Vec::new();
    let mut done = Vec::new();
    let mut past_bar = false;
    for word in spec.split_whitespace() {
        if word == "|" {
            past_bar = true;
            continue;
        }
        if past_bar { &mut done } else { &mut not_done }.push(word.to_string());
    }
    (not_done, done)
}

pub fn parse_keywords(spec: &str) -> Vec<String> {
    spec.split_whitespace()
        .filter(|w| *w != "|")
        .map(str::to_string)
        .collect()
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

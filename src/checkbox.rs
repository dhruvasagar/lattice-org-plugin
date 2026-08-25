//! OM.8 — checkbox items and their statistics cookies, as pure line logic.
//!
//! ```org
//! * Shopping [1/3]
//!   - [X] bread
//!   - [ ] milk
//!   - [ ] eggs
//! ```
//!
//! Toggling `milk` must both flip its box AND recompute `[1/3]` to `[2/3]`,
//! in ONE edit, so a single `u` puts both back. That coupling is the whole
//! reason this is a module rather than a two-line string replace.

/// A checkbox's three states. `Partial` is org's `[-]`: some children ticked,
/// not all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    Partial,
}

impl Check {
    fn as_char(self) -> char {
        match self {
            Check::Off => ' ',
            Check::On => 'X',
            Check::Partial => '-',
        }
    }
}

/// A parsed checkbox list item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Columns of leading whitespace — the nesting level.
    pub indent: usize,
    /// Byte offset of the `[` in the line.
    pub box_at: usize,
    pub state: Check,
}

/// Parse `line` as a checkbox list item.
///
/// A bullet (`-`, `+`) or an ordered marker (`1.`, `1)`) followed by
/// `[ ]` / `[X]` / `[x]` / `[-]`. Note `*` is NOT accepted as a bullet at
/// column 0 — that is a headline, and treating `* [ ] x` as a checkbox would
/// make `<C-Space>` silently rewrite a heading.
pub fn parse_item(line: &str) -> Option<Item> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let after_bullet = strip_bullet(rest, indent)?;
    let box_off = rest.len() - after_bullet.len();
    let b = after_bullet.as_bytes();
    if b.first() != Some(&b'[') || b.get(2) != Some(&b']') {
        return None;
    }
    let state = match b.get(1)? {
        b' ' => Check::Off,
        b'X' | b'x' => Check::On,
        b'-' => Check::Partial,
        _ => return None,
    };
    Some(Item {
        indent,
        box_at: indent + box_off,
        state,
    })
}

/// Strip a list bullet, returning what follows it (whitespace trimmed).
fn strip_bullet(rest: &str, indent: usize) -> Option<&str> {
    // `*` is a bullet ONLY when indented — at column 0 it is a headline.
    for (marker, needs_indent) in [("- ", false), ("+ ", false), ("* ", true)] {
        if let Some(r) = rest.strip_prefix(marker) {
            if needs_indent && indent == 0 {
                return None;
            }
            return Some(r.trim_start());
        }
    }
    // Ordered: digits then `.` or `)` then a space.
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let after = &rest[digits..];
        for marker in [". ", ") "] {
            if let Some(r) = after.strip_prefix(marker) {
                return Some(r.trim_start());
            }
        }
    }
    None
}

/// Rewrite `line`'s checkbox to `state`. `None` if it is not an item.
pub fn set_state(line: &str, state: Check) -> Option<String> {
    let item = parse_item(line)?;
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..item.box_at + 1]);
    out.push(state.as_char());
    out.push_str(&line[item.box_at + 2..]);
    Some(out)
}

/// The state a toggle moves to. `Partial` counts as unticked, so pressing on
/// a half-done parent completes it — which is what a user means by toggling a
/// `[-]`.
pub fn toggled(state: Check) -> Check {
    match state {
        Check::On => Check::Off,
        Check::Off | Check::Partial => Check::On,
    }
}

/// A statistics cookie found on a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cookie {
    /// Byte range of the cookie text, e.g. the `[1/3]`.
    pub start: usize,
    pub end: usize,
    pub percent: bool,
}

/// Find a `[n/m]` or `[p%]` cookie on `line`.
pub fn find_cookie(line: &str) -> Option<Cookie> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'[' {
            i += 1;
            continue;
        }
        let Some(close) = line[i..].find(']').map(|o| i + o) else {
            break;
        };
        let inner = &line[i + 1..close];
        let is_percent = inner.ends_with('%')
            && inner[..inner.len() - 1].chars().all(|c| c.is_ascii_digit())
            && inner.len() > 1;
        let is_ratio = inner.split_once('/').is_some_and(|(a, c)| {
            !a.is_empty()
                && a.chars().all(|ch| ch.is_ascii_digit())
                && c.chars().all(|ch| ch.is_ascii_digit())
        });
        if is_percent || is_ratio {
            return Some(Cookie {
                start: i,
                end: close + 1,
                percent: is_percent,
            });
        }
        i = close + 1;
    }
    None
}

/// Rewrite `line`'s cookie for `done` of `total`. `None` if it has none.
///
/// Keeps the cookie's existing FORM: a `[n/m]` stays a ratio and a `[p%]`
/// stays a percentage. Rewriting one into the other would silently change a
/// document's style on a keypress.
pub fn update_cookie(line: &str, done: usize, total: usize) -> Option<String> {
    let c = find_cookie(line)?;
    let text = if c.percent {
        // Integer percent, truncated — org's own behaviour. 2/3 shows 66%,
        // not 67%, so a cookie only reads 100% when everything is done.
        let pct = if total == 0 { 0 } else { done * 100 / total };
        format!("[{pct}%]")
    } else {
        format!("[{done}/{total}]")
    };
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..c.start]);
    out.push_str(&text);
    out.push_str(&line[c.end..]);
    Some(out)
}

/// Tally the direct children of the item or headline at `parent`.
///
/// Only DIRECT children — items indented more than the parent, stopping at
/// the first line indented at or below it. A grandchild's state is already
/// reflected in its own parent's box, so counting it again would double-count
/// deep lists.
pub fn tally(
    line: impl Fn(u32) -> Option<String>,
    parent: u32,
    parent_indent: usize,
    line_count: u32,
) -> (usize, usize) {
    let mut done = 0;
    let mut total = 0;
    let mut child_indent: Option<usize> = None;
    for i in (parent + 1)..line_count {
        let Some(text) = line(i) else { break };
        if text.trim().is_empty() {
            continue;
        }
        let indent = text.len() - text.trim_start().len();
        // A headline always ends the region, whatever its indent.
        if crate::headline::headline_level(&text).is_some() {
            break;
        }
        if indent <= parent_indent {
            break;
        }
        let Some(item) = parse_item(&text) else {
            continue;
        };
        // Lock on to the FIRST child level seen; deeper items belong to a
        // nested list and are counted by their own parent.
        let level = *child_indent.get_or_insert(item.indent);
        if item.indent != level {
            continue;
        }
        total += 1;
        if item.state == Check::On {
            done += 1;
        }
    }
    (done, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_bullet_forms_org_accepts() {
        for line in ["- [ ] a", "+ [X] a", "  * [-] a", "1. [ ] a", "12) [x] a"] {
            assert!(parse_item(line).is_some(), "{line:?}");
        }
    }

    /// `* [ ] x` at column 0 is a HEADLINE. Treating it as a checkbox would
    /// make `<C-Space>` silently rewrite a heading.
    #[test]
    fn a_star_at_column_zero_is_a_headline_not_a_bullet() {
        assert!(parse_item("* [ ] not a checkbox").is_none());
        assert!(parse_item("  * [ ] but indented is").is_some());
    }

    #[test]
    fn rejects_lines_that_merely_look_like_items() {
        for line in [
            "- no box",
            "[ ] no bullet",
            "- [] too short",
            "- [?] bad",
            "",
        ] {
            assert!(parse_item(line).is_none(), "{line:?}");
        }
    }

    #[test]
    fn toggling_writes_the_box_and_leaves_the_text_alone() {
        assert_eq!(
            set_state("  - [ ] milk", Check::On).unwrap(),
            "  - [X] milk"
        );
        assert_eq!(
            set_state("  - [X] milk", Check::Off).unwrap(),
            "  - [ ] milk"
        );
        assert_eq!(set_state("1. [ ] a", Check::Partial).unwrap(), "1. [-] a");
    }

    /// `[-]` counts as unticked, so pressing on a half-done parent completes
    /// it — which is what toggling a partial box means.
    #[test]
    fn partial_toggles_to_done() {
        assert_eq!(toggled(Check::Partial), Check::On);
        assert_eq!(toggled(Check::Off), Check::On);
        assert_eq!(toggled(Check::On), Check::Off);
    }

    #[test]
    fn finds_both_cookie_forms_and_ignores_other_brackets() {
        assert_eq!(find_cookie("* Shop [1/3]").map(|c| c.percent), Some(false));
        assert_eq!(find_cookie("* Shop [33%]").map(|c| c.percent), Some(true));
        // A checkbox is not a cookie, and neither is a link.
        assert!(find_cookie("- [X] a").is_none());
        assert!(find_cookie("see [[file:a.png]]").is_none());
        assert!(find_cookie("no cookie here").is_none());
    }

    /// A `[n/m]` stays a ratio and a `[p%]` stays a percentage — rewriting one
    /// into the other would change a document's style on a keypress.
    #[test]
    fn updating_keeps_the_cookies_existing_form() {
        assert_eq!(update_cookie("* S [0/0]", 2, 3).unwrap(), "* S [2/3]");
        assert_eq!(update_cookie("* S [0%]", 2, 3).unwrap(), "* S [66%]");
    }

    /// Truncated, like org: 2/3 is 66%, so a cookie reads 100% only when
    /// everything is actually done.
    #[test]
    fn percentages_truncate_so_100_means_complete() {
        assert_eq!(update_cookie("[0%]", 2, 3).unwrap(), "[66%]");
        assert_eq!(update_cookie("[0%]", 3, 3).unwrap(), "[100%]");
        assert_eq!(
            update_cookie("[0%]", 0, 0).unwrap(),
            "[0%]",
            "no division by zero"
        );
    }

    fn buf(text: &str) -> (impl Fn(u32) -> Option<String> + use<>, u32) {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let n = lines.len() as u32;
        (move |i: u32| lines.get(i as usize).cloned(), n)
    }

    #[test]
    fn tallies_the_direct_children_of_a_headline() {
        let (l, n) = buf("* Shop [0/0]\n  - [X] bread\n  - [ ] milk\n  - [ ] eggs\n");
        assert_eq!(tally(&l, 0, 0, n), (1, 3));
    }

    /// A grandchild's state is already reflected in its own parent's box, so
    /// counting it again would double-count deep lists.
    #[test]
    fn nested_items_are_counted_by_their_own_parent_only() {
        let (l, n) = buf("* Top [0/0]\n  - [-] a\n    - [X] a1\n    - [ ] a2\n  - [ ] b\n");
        assert_eq!(tally(&l, 0, 0, n), (0, 2), "only `a` and `b`");
        // `a`'s own children are two, one done.
        assert_eq!(tally(&l, 1, 2, n), (1, 2));
    }

    /// A following headline ends the region even when it is not indented
    /// less — otherwise a cookie would count the next section's items.
    #[test]
    fn a_following_headline_ends_the_tally() {
        let (l, n) = buf("* One [0/0]\n  - [X] a\n* Two\n  - [X] b\n");
        assert_eq!(tally(&l, 0, 0, n), (1, 1));
    }

    #[test]
    fn blank_lines_do_not_end_a_list() {
        let (l, n) = buf("* One [0/0]\n  - [X] a\n\n  - [ ] b\n");
        assert_eq!(tally(&l, 0, 0, n), (1, 2));
    }
}

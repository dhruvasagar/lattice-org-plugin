//! Objects: what text inside an element holds. Emphasis, verbatim and code,
//! links, footnote references, entities, LaTeX fragments, sub- and
//! superscripts, line breaks, timestamps, targets and radio targets,
//! statistics cookies, macros and export snippets.
//!
//! **One pass, left to right.** At each byte the parser tries the objects
//! that can start there, in org's precedence, and otherwise extends the
//! text run. Nesting past [`MAX_DEPTH`] is left as text.
//!
//! **No opener rescans.** An object that never closes must not cost a scan
//! to the end of its line per opener, or one long line of them is
//! quadratic (a 60 KB line of ` *a` took over a second natively, which in
//! the guest is a fuel trap). Whether a marker can close emphasis depends
//! only on the characters beside it, so the closing positions, and the
//! newlines that bound the one-newline window, are found once per text
//! ([`Scan`]) and looked up. The other closers stop where the object's
//! grammar says it cannot continue: a target at the first `<`, `>` or
//! newline, `\(` at the next `\(`, a macro at the next `{{{`, a footnote
//! reference at the next `[fn:`.
//!
//! The emphasis rules are org's `org-emphasis-regexp-components`: an opening
//! marker follows the start of the text, whitespace or one of `-('"{`, and is
//! followed by a non-space; a closing marker follows a non-space and is
//! followed by the end, whitespace or one of `-.,:!?;'")}\[`. So `a*b*c` is
//! text, and `=x *y* z=` is verbatim whose stars are characters.

use super::MAX_DEPTH;
use crate::links::close_before;

/// An inline object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Object {
    Text(String),
    Emphasis {
        kind: Emphasis,
        contents: Vec<Object>,
    },
    /// `=verbatim=`, contents as written.
    Verbatim(String),
    /// `~code~`, contents as written.
    Code(String),
    Link {
        /// The path as written, escapes taken off: `https://…`, `file:a.org`,
        /// `*Headline`, `#custom-id`, `id:…`, or a name or target.
        path: String,
        /// Empty when the link has none.
        description: Vec<Object>,
    },
    /// `[fn:LABEL]`, `[fn:LABEL:DEF]` or `[fn::DEF]`.
    FootnoteReference {
        label: Option<String>,
        /// An inline definition's contents.
        definition: Vec<Object>,
    },
    /// `\alpha`, `\nbsp{}`: a named character.
    Entity {
        name: String,
        /// The character it stands for.
        utf8: &'static str,
        /// Written with `{}` after the name.
        braces: bool,
    },
    /// `\(…\)`, `\[…\]`, `$…$`, `$$…$$`, or a `\command{…}` org does not
    /// know as an entity: the source, delimiters included.
    LatexFragment(String),
    /// `_x` or `_{x}`.
    Subscript {
        braced: bool,
        contents: Vec<Object>,
    },
    /// `^x` or `^{x}`.
    Superscript {
        braced: bool,
        contents: Vec<Object>,
    },
    /// `\\` at the end of a line.
    LineBreak,
    /// `<2026-01-05 Mon>`, `[2026-01-05 Mon 10:00]`, ranges included, as
    /// written.
    Timestamp(String),
    /// `<<target>>`'s text.
    Target(String),
    /// `<<<radio>>>`.
    RadioTarget(Vec<Object>),
    /// `[2/5]` or `[40%]`, as written.
    StatisticsCookie(String),
    /// `{{{name(a, b)}}}`.
    Macro {
        name: String,
        args: Vec<String>,
    },
    /// `@@backend:value@@`.
    ExportSnippet {
        backend: String,
        value: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Emphasis {
    Bold,
    Italic,
    Underline,
    Strike,
}

/// The schemes a plain or angle link may have.
const SCHEMES: &[&str] = &[
    "https", "http", "ftp", "mailto", "file", "doi", "news", "id", "irc", "shell",
];

/// Parse `text` into objects, every `_` / `^` that can be a script read as
/// one (`^:t`). A file's `^:{}` or `^:nil` comes from `#+OPTIONS:`, which may
/// sit below the text it governs, so it is applied after the parse, by
/// reverting the scripts it does not allow (as ox does).
pub(crate) fn parse(text: &str) -> Vec<Object> {
    Parser.objects(text, 0, true)
}

struct Parser;

/// What is found once per text: where each emphasis marker can close, and
/// where the newlines are.
struct Scan {
    /// By marker, in the order of [`MARKERS`].
    closes: [Vec<usize>; 6],
    newlines: Vec<usize>,
}

const MARKERS: [u8; 6] = [b'*', b'/', b'_', b'+', b'=', b'~'];

impl Scan {
    fn of(s: &str) -> Self {
        let mut scan = Scan {
            closes: Default::default(),
            newlines: Vec::new(),
        };
        let mut prev: Option<char> = None;
        let mut chars = s.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '\n' {
                scan.newlines.push(i);
            } else if let Some(k) = MARKERS.iter().position(|&m| m as char == c) {
                let next = chars.peek().map(|&(_, n)| n);
                if prev.is_some_and(|p| !p.is_whitespace())
                    && next.is_none_or(|n| n.is_whitespace() || "-.,:!?;'\")}\\[".contains(n))
                {
                    scan.closes[k].push(i);
                }
            }
            prev = Some(c);
        }
        scan
    }

    /// The end of the window an opener at `i` may close in: its line and the
    /// next.
    fn window_end(&self, i: usize, len: usize) -> usize {
        let at = self.newlines.partition_point(|&n| n < i);
        self.newlines.get(at + 1).copied().unwrap_or(len)
    }

    /// The first position `marker` can close at, from `from`.
    fn close(&self, marker: u8, from: usize) -> Option<usize> {
        let list = &self.closes[MARKERS.iter().position(|&m| m == marker)?];
        list.get(list.partition_point(|&c| c < from)).copied()
    }
}

/// Collects text between objects.
struct Out {
    objects: Vec<Object>,
    text: String,
}

impl Out {
    fn push(&mut self, object: Object) {
        self.flush();
        self.objects.push(object);
    }

    fn flush(&mut self) {
        if !self.text.is_empty() {
            self.objects
                .push(Object::Text(std::mem::take(&mut self.text)));
        }
    }

    fn finish(mut self) -> Vec<Object> {
        self.flush();
        self.objects
    }
}

impl Parser {
    /// `s` as objects; `links` is false inside a link's description, which
    /// may not hold another.
    fn objects(&self, s: &str, depth: usize, links: bool) -> Vec<Object> {
        if depth >= MAX_DEPTH {
            return vec![Object::Text(s.to_string())];
        }
        let mut out = Out {
            objects: Vec::new(),
            text: String::new(),
        };
        let scan = Scan::of(s);
        let mut i = 0;
        while i < s.len() {
            if let Some((object, len)) = self.object_at(s, i, &scan, depth, links) {
                out.push(object);
                i += len;
                continue;
            }
            let c = s[i..].chars().next().unwrap_or(' ');
            out.text.push(c);
            i += c.len_utf8();
        }
        out.finish()
    }

    /// The object starting at byte `i` of `s`, and its length.
    fn object_at(
        &self,
        s: &str,
        i: usize,
        scan: &Scan,
        depth: usize,
        links: bool,
    ) -> Option<(Object, usize)> {
        let rest = &s[i..];
        let before = s[..i].chars().next_back();
        match rest.as_bytes()[0] {
            b'*' | b'/' | b'_' | b'+' | b'=' | b'~' => {
                if let Some(found) = self.emphasis(s, i, scan, before, depth, links) {
                    return Some(found);
                }
                if matches!(rest.as_bytes()[0], b'_') {
                    return self.script(rest, before, depth, links);
                }
                None
            }
            b'^' => self.script(rest, before, depth, links),
            b'[' => {
                if links && rest.starts_with("[[") {
                    let link = crate::links::bracket_link(rest)?;
                    let description = link
                        .description
                        .map(|d| self.objects(d, depth + 1, false))
                        .unwrap_or_default();
                    return Some((
                        Object::Link {
                            path: link.path,
                            description,
                        },
                        link.len,
                    ));
                }
                if rest.starts_with("[fn:") {
                    return self.footnote_reference(rest, depth);
                }
                timestamp(rest).or_else(|| cookie(rest))
            }
            b'<' => {
                if let Some(found) = radio_target(rest) {
                    let (inner, len) = found;
                    return Some((
                        Object::RadioTarget(self.objects(inner, depth + 1, links)),
                        len,
                    ));
                }
                target(rest)
                    .or_else(|| timestamp(rest))
                    .or_else(|| links.then(|| angle_link(rest)).flatten())
            }
            b'\\' => line_break(rest)
                .or_else(|| latex_delimited(rest))
                .or_else(|| entity_or_command(rest)),
            b'$' => latex_dollar(rest, before, scan.window_end(i, s.len()) - i),
            b'{' => macro_call(rest),
            b'@' => export_snippet(rest),
            _ if links && before.is_none_or(|c| !c.is_alphanumeric()) => plain_link(rest),
            _ => None,
        }
    }

    fn emphasis(
        &self,
        s: &str,
        i: usize,
        scan: &Scan,
        before: Option<char>,
        depth: usize,
        links: bool,
    ) -> Option<(Object, usize)> {
        if !before.is_none_or(|c| c.is_whitespace() || "-('\"{".contains(c)) {
            return None;
        }
        let rest = &s[i..];
        let marker = rest.as_bytes()[0];
        let body_start = 1;
        let first = rest[body_start..].chars().next()?;
        if first.is_whitespace() {
            return None;
        }
        // The body may run onto the next line, no further.
        let at = scan.close(marker, i + body_start + first.len_utf8())?;
        if at >= scan.window_end(i, s.len()) {
            return None;
        }
        let close = at - i;
        let marker = marker as char;
        let body = &rest[body_start..close];
        let len = close + 1;
        let object = match marker {
            '=' => Object::Verbatim(body.to_string()),
            '~' => Object::Code(body.to_string()),
            m => Object::Emphasis {
                kind: match m {
                    '*' => Emphasis::Bold,
                    '/' => Emphasis::Italic,
                    '_' => Emphasis::Underline,
                    _ => Emphasis::Strike,
                },
                contents: self.objects(body, depth + 1, links),
            },
        };
        Some((object, len))
    }

    /// `_x` / `^x` after a non-space: braced `{…}`, `*`, or a run of
    /// letters and digits (with `.` `,` `\` inside, ending on an alphanumeric).
    fn script(
        &self,
        rest: &str,
        before: Option<char>,
        depth: usize,
        links: bool,
    ) -> Option<(Object, usize)> {
        if before.is_none_or(char::is_whitespace) {
            return None;
        }
        let sub = rest.starts_with('_');
        let after = &rest[1..];
        let braced = after.starts_with('{');
        let (contents, len) = if let Some(inner) = after.strip_prefix('{') {
            let end = script_braces(inner)?;
            (
                self.objects(&inner[..end], depth + 1, links),
                1 + 1 + end + 1,
            )
        } else if after.starts_with('*') {
            (vec![Object::Text("*".to_string())], 2)
        } else {
            let sign = usize::from(after.starts_with(['+', '-']));
            let run: usize = after[sign..]
                .char_indices()
                .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '.' | ',' | '\\'))
                .map(|(i, c)| i + c.len_utf8())
                .last()?;
            // Trailing `.` / `,` are punctuation, not the script's.
            let mut end = sign + run;
            while end > sign && !after[..end].ends_with(char::is_alphanumeric) {
                end -= after[..end].chars().next_back()?.len_utf8();
            }
            if end == sign {
                return None;
            }
            (vec![Object::Text(after[..end].to_string())], 1 + end)
        };
        let object = if sub {
            Object::Subscript { braced, contents }
        } else {
            Object::Superscript { braced, contents }
        };
        Some((object, len))
    }

    fn footnote_reference(&self, rest: &str, depth: usize) -> Option<(Object, usize)> {
        let inner = &rest[1..];
        // A reference cannot hold another, so the next `[fn:` bounds the
        // search for this one's close.
        let bound = inner[3..].find("[fn:").map_or(inner.len(), |o| 3 + o);
        let end = balanced(&inner[..bound], '[', ']')?;
        let body = inner[..end].strip_prefix("fn:")?;
        let len = 1 + end + 1;
        let (label, definition) = match body.split_once(':') {
            Some((label, def)) => (label, Some(def)),
            None => (body, None),
        };
        let valid = |l: &str| {
            l.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        };
        if !valid(label) || (label.is_empty() && definition.is_none()) {
            return None;
        }
        Some((
            Object::FootnoteReference {
                label: (!label.is_empty()).then(|| label.to_string()),
                definition: definition
                    .map(|d| self.objects(d, depth + 1, true))
                    .unwrap_or_default(),
            },
            len,
        ))
    }
}

/// The index in `s` of the `close` that balances an `open` already passed,
/// on one line.
fn balanced(s: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        if c == '\n' {
            return None;
        }
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
        }
    }
    None
}

/// The `}` closing a script's `{` already passed: org allows one level of
/// braces inside, and no newline.
fn script_braces(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'\n' => return None,
            b'{' if depth == 1 => return None,
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// `<YYYY-MM-DD …>` or `[YYYY-MM-DD …]`, and a `--` range onto a second.
fn timestamp(rest: &str) -> Option<(Object, usize)> {
    let one = |s: &str| -> Option<usize> {
        let close = match s.as_bytes().first()? {
            b'<' => '>',
            b'[' => ']',
            _ => return None,
        };
        let date = s.get(1..11)?.as_bytes();
        let digits = |r: std::ops::Range<usize>| date[r].iter().all(u8::is_ascii_digit);
        if !(digits(0..4) && date[4] == b'-' && digits(5..7) && date[7] == b'-' && digits(8..10)) {
            return None;
        }
        let end = s[11..].find([close, '\n', '<', '[']).map(|o| 11 + o)?;
        (s[end..].starts_with(close)).then_some(end + 1)
    };
    let first = one(rest)?;
    let len = match rest[first..].strip_prefix("--") {
        Some(second) => one(second).map_or(first, |n| first + 2 + n),
        None => first,
    };
    Some((Object::Timestamp(rest[..len].to_string()), len))
}

/// `[N/M]`, `[N%]`, either number possibly absent.
fn cookie(rest: &str) -> Option<(Object, usize)> {
    let end = 1 + rest[1..]
        .bytes()
        .take_while(|b| b.is_ascii_digit() || matches!(b, b'/' | b'%'))
        .count();
    if rest.as_bytes().get(end) != Some(&b']') {
        return None;
    }
    let inner = &rest[1..end];
    let ok = if let Some(pct) = inner.strip_suffix('%') {
        pct.chars().all(|c| c.is_ascii_digit())
    } else {
        let (a, b) = inner.split_once('/')?;
        a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit())
    };
    ok.then(|| (Object::StatisticsCookie(rest[..=end].to_string()), end + 1))
}

/// `<<<text>>>`: the inner text and the length.
fn radio_target(rest: &str) -> Option<(&str, usize)> {
    let inner = rest.strip_prefix("<<<")?;
    let end = inner.find(['<', '>', '\n'])?;
    let text = &inner[..end];
    (inner[end..].starts_with(">>>") && target_text_ok(text)).then_some((text, 3 + end + 3))
}

fn target(rest: &str) -> Option<(Object, usize)> {
    let inner = rest.strip_prefix("<<")?;
    if inner.starts_with('<') {
        return None;
    }
    let end = inner.find(['<', '>', '\n'])?;
    let text = &inner[..end];
    (inner[end..].starts_with(">>") && target_text_ok(text))
        .then(|| (Object::Target(text.to_string()), 2 + end + 2))
}

fn target_text_ok(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with(char::is_whitespace)
        && !text.ends_with(char::is_whitespace)
        && !text.contains(['<', '>', '\n'])
}

/// `<scheme:…>`.
fn angle_link(rest: &str) -> Option<(Object, usize)> {
    let inner = &rest[1..];
    scheme_of(inner)?;
    let end = inner.find(['>', '<', '\n'])?;
    let path = &inner[..end];
    if !inner[end..].starts_with('>') {
        return None;
    }
    Some((
        Object::Link {
            path: path.to_string(),
            description: Vec::new(),
        },
        1 + end + 1,
    ))
}

/// The scheme `s` starts with, if it is one a link may have.
fn scheme_of(s: &str) -> Option<&'static str> {
    SCHEMES
        .iter()
        .find(|scheme| {
            s.strip_prefix(**scheme)
                .and_then(|r| r.strip_prefix(':'))
                .is_some_and(|r| !r.is_empty())
        })
        .copied()
}

/// `scheme:path` in running text: to whitespace or a bracket, keeping one
/// level of parentheses, and leaving trailing punctuation outside.
fn plain_link(rest: &str) -> Option<(Object, usize)> {
    scheme_of(rest)?;
    let mut depth = 0usize;
    let mut end = 0;
    for (i, c) in rest.char_indices() {
        match c {
            c if c.is_whitespace() => break,
            '[' | ']' | '<' | '>' | '"' => break,
            '(' => depth += 1,
            ')' if depth == 0 => break,
            ')' => depth -= 1,
            _ => {}
        }
        end = i + c.len_utf8();
    }
    while end > 0 && rest[..end].ends_with(['.', ',', ';', ':', '!', '?', '\'']) {
        end -= 1;
    }
    let path = &rest[..end];
    // The scheme and colon alone are not a link.
    if path.split_once(':').is_none_or(|(_, p)| p.is_empty()) {
        return None;
    }
    Some((
        Object::Link {
            path: path.to_string(),
            description: Vec::new(),
        },
        end,
    ))
}

/// `\\` followed by nothing but spaces to the end of the line.
fn line_break(rest: &str) -> Option<(Object, usize)> {
    let after = rest.strip_prefix("\\\\")?;
    let spaces = after.len() - after.trim_start_matches([' ', '\t']).len();
    let tail = &after[spaces..];
    (tail.is_empty() || tail.starts_with('\n')).then_some((Object::LineBreak, 2 + spaces))
}

/// `\(…\)` and `\[…\]`.
fn latex_delimited(rest: &str) -> Option<(Object, usize)> {
    let (open, close) = match rest.get(..2)? {
        "\\(" => ("\\(", "\\)"),
        "\\[" => ("\\[", "\\]"),
        _ => return None,
    };
    let end = 2 + close_before(&rest[2..], close, open)? + 2;
    Some((Object::LatexFragment(rest[..end].to_string()), end))
}

/// `$$…$$`, or `$…$` with org's border rules: not after a `$`, no space
/// just inside either `$`, and the closing one followed by the end,
/// whitespace or punctuation.
fn latex_dollar(rest: &str, before: Option<char>, window: usize) -> Option<(Object, usize)> {
    if let Some(inner) = rest.strip_prefix("$$") {
        let end = inner.find("$$")?;
        if end == 0 {
            return None;
        }
        let len = 2 + end + 2;
        return Some((Object::LatexFragment(rest[..len].to_string()), len));
    }
    if before == Some('$') {
        return None;
    }
    let inner = &rest[1..];
    let first = inner.chars().next()?;
    if first.is_whitespace() || first == '$' {
        return None;
    }
    let end = inner[..window.saturating_sub(1).min(inner.len())].find('$')?;
    let body = &inner[..end];
    let next = inner[end + 1..].chars().next();
    if body.ends_with(char::is_whitespace)
        || body.ends_with(',')
        || body.ends_with('.')
        || !next.is_none_or(|c| c.is_whitespace() || c.is_ascii_punctuation())
    {
        return None;
    }
    let len = 1 + end + 1;
    Some((Object::LatexFragment(rest[..len].to_string()), len))
}

/// `\name` as an entity when org knows the name (`{}` after it is eaten),
/// else as a LaTeX command with its `[…]` and `{…}` arguments.
fn entity_or_command(rest: &str) -> Option<(Object, usize)> {
    let name_len = rest[1..]
        .bytes()
        .take_while(u8::is_ascii_alphabetic)
        .count();
    if name_len == 0 {
        return None;
    }
    let name = &rest[1..1 + name_len];
    let mut len = 1 + name_len;
    if let Some(utf8) = entity(name) {
        let braces = rest[len..].starts_with("{}");
        if braces {
            len += 2;
        } else if rest[len..].starts_with(|c: char| c.is_alphabetic()) {
            return None;
        }
        return Some((
            Object::Entity {
                name: name.to_string(),
                utf8,
                braces,
            },
            len,
        ));
    }
    // Arguments as org's fragment grammar has them: `[…]` holding no
    // brackets or braces, `{…}` holding no braces, neither a newline.
    loop {
        let tail = &rest[len..];
        let (close, forbidden): (u8, &[u8]) = match tail.as_bytes().first() {
            Some(b'[') => (b']', b"[{}\n"),
            Some(b'{') => (b'}', b"{\n"),
            _ => break,
        };
        let Some(end) = tail[1..]
            .bytes()
            .position(|b| b == close || forbidden.contains(&b))
            .filter(|&e| tail.as_bytes()[1 + e] == close)
        else {
            break;
        };
        len += 1 + end + 1;
    }
    Some((Object::LatexFragment(rest[..len].to_string()), len))
}

/// `{{{name}}}` or `{{{name(args)}}}`; arguments split at commas a
/// backslash does not escape, and trimmed.
fn macro_call(rest: &str) -> Option<(Object, usize)> {
    let inner = rest.strip_prefix("{{{")?;
    // The name comes first, so a `{{{` that does not start with one is
    // refused before any search.
    if !inner.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let end = close_before(inner, "}}}", "{{{")?;
    let body = &inner[..end];
    let (name, args) = match body.split_once('(') {
        Some((name, args)) => (name, Some(args.strip_suffix(')')?)),
        None => (body, None),
    };
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let args = args.map(split_macro_args).unwrap_or_default();
    Some((
        Object::Macro {
            name: name.to_ascii_lowercase(),
            args,
        },
        3 + end + 3,
    ))
}

fn split_macro_args(s: &str) -> Vec<String> {
    let mut args = vec![String::new()];
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let push = match c {
            '\\' if chars.peek() == Some(&',') => {
                chars.next();
                ','
            }
            ',' => {
                args.push(String::new());
                continue;
            }
            c => c,
        };
        if let Some(a) = args.last_mut() {
            a.push(push);
        }
    }
    args.into_iter().map(|a| a.trim().to_string()).collect()
}

/// `@@backend:value@@`.
fn export_snippet(rest: &str) -> Option<(Object, usize)> {
    let inner = rest.strip_prefix("@@")?;
    let (backend, after) = inner.split_once(':')?;
    if backend.is_empty()
        || !backend
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    let end = after.find("@@")?;
    Some((
        Object::ExportSnippet {
            backend: backend.to_ascii_lowercase(),
            value: after[..end].to_string(),
        },
        2 + backend.len() + 1 + end + 2,
    ))
}

/// The character an entity name stands for. Names are LaTeX's and HTML's;
/// the set is the common ones: Greek, arrows, relations and operators, and
/// typography.
fn entity(name: &str) -> Option<&'static str> {
    ENTITIES
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| ENTITIES[i].1)
}

/// Sorted by name (byte order), for [`entity`]'s binary search.
const ENTITIES: &[(&str, &str)] = &[
    ("AA", "Å"),
    ("AE", "Æ"),
    ("Alpha", "Α"),
    ("Beta", "Β"),
    ("Chi", "Χ"),
    ("Dagger", "‡"),
    ("Delta", "Δ"),
    ("Epsilon", "Ε"),
    ("Eta", "Η"),
    ("Gamma", "Γ"),
    ("Iota", "Ι"),
    ("Kappa", "Κ"),
    ("Lambda", "Λ"),
    ("Leftarrow", "⇐"),
    ("Leftrightarrow", "⇔"),
    ("Mu", "Μ"),
    ("Nu", "Ν"),
    ("OE", "Œ"),
    ("Omega", "Ω"),
    ("Omicron", "Ο"),
    ("Phi", "Φ"),
    ("Pi", "Π"),
    ("Psi", "Ψ"),
    ("Rho", "Ρ"),
    ("Rightarrow", "⇒"),
    ("Sigma", "Σ"),
    ("Tau", "Τ"),
    ("Theta", "Θ"),
    ("Upsilon", "Υ"),
    ("Xi", "Ξ"),
    ("Zeta", "Ζ"),
    ("aa", "å"),
    ("ae", "æ"),
    ("alpha", "α"),
    ("amp", "&"),
    ("approx", "≈"),
    ("ast", "∗"),
    ("beta", "β"),
    ("bullet", "•"),
    ("cap", "∩"),
    ("cdot", "⋅"),
    ("cdots", "⋯"),
    ("cent", "¢"),
    ("checkmark", "✓"),
    ("chi", "χ"),
    ("copy", "©"),
    ("cup", "∪"),
    ("dagger", "†"),
    ("darr", "↓"),
    ("deg", "°"),
    ("delta", "δ"),
    ("div", "÷"),
    ("dots", "…"),
    ("downarrow", "↓"),
    ("ell", "ℓ"),
    ("emptyset", "∅"),
    ("emsp", "\u{2003}"),
    ("ensp", "\u{2002}"),
    ("epsilon", "ε"),
    ("equiv", "≡"),
    ("eta", "η"),
    ("euro", "€"),
    ("exists", "∃"),
    ("forall", "∀"),
    ("frac12", "½"),
    ("frac14", "¼"),
    ("frac34", "¾"),
    ("gamma", "γ"),
    ("ge", "≥"),
    ("geq", "≥"),
    ("gt", ">"),
    ("harr", "↔"),
    ("hellip", "…"),
    ("iexcl", "¡"),
    ("in", "∈"),
    ("infin", "∞"),
    ("infty", "∞"),
    ("int", "∫"),
    ("iota", "ι"),
    ("iquest", "¿"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("laquo", "«"),
    ("larr", "←"),
    ("ldots", "…"),
    ("ldquo", "“"),
    ("le", "≤"),
    ("leftarrow", "←"),
    ("leftrightarrow", "↔"),
    ("leq", "≤"),
    ("lsquo", "‘"),
    ("lt", "<"),
    ("mdash", "—"),
    ("middot", "·"),
    ("minus", "−"),
    ("mp", "∓"),
    ("mu", "μ"),
    ("nabla", "∇"),
    ("nbsp", "\u{a0}"),
    ("ndash", "–"),
    ("ne", "≠"),
    ("neq", "≠"),
    ("ni", "∋"),
    ("notin", "∉"),
    ("nu", "ν"),
    ("oe", "œ"),
    ("omega", "ω"),
    ("omicron", "ο"),
    ("para", "¶"),
    ("partial", "∂"),
    ("phi", "φ"),
    ("pi", "π"),
    ("pm", "±"),
    ("pound", "£"),
    ("prime", "′"),
    ("prod", "∏"),
    ("prop", "∝"),
    ("psi", "ψ"),
    ("quot", "\""),
    ("raquo", "»"),
    ("rarr", "→"),
    ("rdquo", "”"),
    ("reg", "®"),
    ("rho", "ρ"),
    ("rightarrow", "→"),
    ("rsquo", "’"),
    ("sect", "§"),
    ("shy", "\u{ad}"),
    ("sigma", "σ"),
    ("sim", "∼"),
    ("sqrt", "√"),
    ("star", "⋆"),
    ("sub", "⊂"),
    ("subset", "⊂"),
    ("sum", "∑"),
    ("sup", "⊃"),
    ("supset", "⊃"),
    ("tau", "τ"),
    ("theta", "θ"),
    ("thinsp", "\u{2009}"),
    ("times", "×"),
    ("to", "→"),
    ("trade", "™"),
    ("uarr", "↑"),
    ("uparrow", "↑"),
    ("upsilon", "υ"),
    ("varepsilon", "ε"),
    ("varphi", "ϕ"),
    ("xi", "ξ"),
    ("yen", "¥"),
    ("zeta", "ζ"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(s: &str) -> String {
        super::super::dump::objects(&parse(s))
    }

    #[test]
    fn the_entity_table_is_sorted() {
        assert!(ENTITIES.windows(2).all(|w| w[0].0 < w[1].0));
    }

    /// The classic traps, each against what org makes of it.
    #[test]
    fn the_traps() {
        let cases: &[(&str, &str)] = &[
            ("a*b*c", r#""a*b*c""#),
            ("=code with *stars*=", r#"verbatim("code with *stars*")"#),
            (
                "[[https://x.y][a [1] b]]",
                r#"link("https://x.y")["a [1] b"]"#,
            ),
            ("*bold\nacross*", r#"bold["bold\nacross"]"#),
            ("*not\nacross\nthree*", r#""*not\nacross\nthree*""#),
            ("é*b*", r#""é*b*""#),
            ("(*b*)", r#""(" bold["b"] ")""#),
            ("* b*", r#""* b*""#),
            ("*b *", r#""*b *""#),
            ("x *b*.", r#""x " bold["b"] ".""#),
            ("*/both/*", r#"bold[italic["both"]]"#),
            ("~a*b*~", r#"code("a*b*")"#),
            ("snake_case", r#""snake" sub["case"]"#),
            (" _u_ ", r#"" " underline["u"] " ""#),
            ("E=mc^2.", r#""E=mc" sup["2"] ".""#),
            ("a^{b c}", r#""a" sup{}["b c"]"#),
            ("price $5 and $6", r#""price $5 and $6""#),
            ("$x^2$ ok", r#"latex("$x^2$") " ok""#),
        ];
        for (input, want) in cases {
            assert_eq!(&dump(input), want, "{input:?}");
        }
    }

    #[test]
    fn links() {
        let cases: &[(&str, &str)] = &[
            ("see https://a.b/c.", r#""see " link("https://a.b/c") ".""#),
            (
                "(https://a.b/x_(y))",
                r#""(" link("https://a.b/x_(y)") ")""#,
            ),
            ("<mailto:a@b.c> x", r#"link("mailto:a@b.c") " x""#),
            ("[[*Plan]]", r#"link("*Plan")"#),
            ("[[#id][*b*]]", r##"link("#id")[bold["b"]]"##),
            ("myhttps://x", r#""myhttps://x""#),
            ("https:", r#""https:""#),
        ];
        for (input, want) in cases {
            assert_eq!(&dump(input), want, "{input:?}");
        }
    }

    #[test]
    fn the_rest() {
        let cases: &[(&str, &str)] = &[
            ("a[fn:1] b", r#""a" fn("1") " b""#),
            ("[fn::inline *def*]", r#"fn(None)["inline " bold["def"]]"#),
            ("[fn:n:def [x]]", r#"fn("n")["def [x]"]"#),
            (
                "\\alpha{}b \\to x",
                r#"entity(alpha{}) "b " entity(to) " x""#,
            ),
            ("\\alphabet", r#"latex("\\alphabet")"#),
            ("\\frac{a}{b}", r#"latex("\\frac{a}{b}")"#),
            (
                "\\(x\\) \\[y\\]",
                r#"latex("\\(x\\)") " " latex("\\[y\\]")"#,
            ),
            ("$$e$$", r#"latex("$$e$$")"#),
            ("end\\\\\nnext", r#""end" linebreak "\nnext""#),
            (
                "<2026-01-05 Mon>--<2026-01-06 Tue>",
                r#"timestamp("<2026-01-05 Mon>--<2026-01-06 Tue>")"#,
            ),
            (
                "[2026-01-05 Mon 10:00]",
                r#"timestamp("[2026-01-05 Mon 10:00]")"#,
            ),
            ("<2026-1-5>", r#""<2026-1-5>""#),
            (
                "<<here>> <<<radio>>>",
                r#"target("here") " " radio["radio"]"#,
            ),
            ("<< no>>", r#""<< no>>""#),
            (
                "[2/5] [40%] [/] [a/b]",
                r#"cookie("[2/5]") " " cookie("[40%]") " " cookie("[/]") " [a/b]""#,
            ),
            (
                "{{{kbd(C-x\\, C-s, b)}}}",
                r#"macro(kbd ["C-x, C-s", "b"])"#,
            ),
            ("{{{date}}}", r#"macro(date [])"#),
            ("@@html:<b>@@", r#"snippet(html "<b>")"#),
        ];
        for (input, want) in cases {
            assert_eq!(&dump(input), want, "{input:?}");
        }
    }

    /// Nothing panics on text built from the characters objects turn on.
    #[test]
    fn arbitrary_text_parses_without_panicking() {
        const PIECES: &[&str] = &[
            "*",
            "/",
            "_",
            "+",
            "=",
            "~",
            "^",
            "[",
            "]",
            "[[",
            "]]",
            "][",
            "<",
            ">",
            "<<",
            ">>",
            "\\",
            "\\\\",
            "$",
            "$$",
            "{",
            "}",
            "{{{",
            "}}}",
            "(",
            ")",
            "@@",
            ":",
            "fn:",
            "https:",
            "//a.b",
            "2026-01-05",
            " ",
            "\n",
            "a",
            "bc",
            "é",
            "✓",
            "\\alpha",
            "%",
            "1",
        ];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..5000 {
            let mut s = String::new();
            for _ in 0..(next() % 40) {
                s.push_str(PIECES[(next() % PIECES.len() as u64) as usize]);
            }
            let _ = parse(&s);
        }
    }

    /// One long line of openers that never close is linear: each kind,
    /// 60 KB, well inside a second even unoptimised.
    #[test]
    fn unclosed_openers_on_one_line_do_not_rescan_it() {
        for unit in [
            " *a", " =a", "<<a ", "<<<a ", "\\(a ", "{{{a(", "[fn:a ", "[1/", " $a", "[[a][b ",
        ] {
            let s = unit.repeat(60_000 / unit.len());
            let start = std::time::Instant::now();
            let _ = parse(&s);
            let spent = start.elapsed();
            assert!(
                spent < std::time::Duration::from_secs(1),
                "{unit:?}: {spent:?}"
            );
        }
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let n = 5_000;
        let s = format!("{}x{}", "*/".repeat(n), "/*".repeat(n));
        let _ = parse(&s);
        let s = format!("{}x{}", "a^{".repeat(n), "}".repeat(n));
        let _ = parse(&s);
    }
}

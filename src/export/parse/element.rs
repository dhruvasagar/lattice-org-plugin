//! Elements within one section: everything but headlines,
//! planning and the property drawer, which `mod.rs` reads off the section's
//! top.

use std::collections::HashMap;

use super::{
    keyword_parts, Affiliated, Align, Block, BlockKind, Checkbox, Element, ElementKind, Item, List,
    ListKind, Row, Table, MAX_DEPTH,
};

/// Parse a section body: `lines`, the first of which is file line
/// `first_line`.
pub(super) fn parse(lines: &[&str], first_line: u32) -> Vec<Element> {
    let index = Index::build(lines, first_line);
    let view: Vec<Line<'_>> = lines
        .iter()
        .enumerate()
        .map(|(i, text)| Line {
            abs: first_line + i as u32,
            text,
        })
        .collect();
    Parser { index: &index }.elements(&view, 0)
}

/// One line of a view: its line in the file, and its text with whatever
/// indentation or bullet the enclosing element owns taken off.
///
/// A view's lines are always consecutive in the file, so a file line found
/// in the index is at `abs - view[0].abs` within it.
#[derive(Clone, Copy)]
struct Line<'a> {
    abs: u32,
    text: &'a str,
}

/// Where each end marker in the section is, by file line, so the end of a
/// block is a lookup rather than a scan.
struct Index {
    /// `#+end_NAME`, by lowercased name.
    blocks: HashMap<String, Vec<u32>>,
    /// `:END:`.
    drawers: Vec<u32>,
    /// `\end{NAME}`, by name.
    latex: HashMap<String, Vec<u32>>,
}

impl Index {
    fn build(lines: &[&str], first_line: u32) -> Self {
        let mut index = Index {
            blocks: HashMap::new(),
            drawers: Vec::new(),
            latex: HashMap::new(),
        };
        for (i, line) in lines.iter().enumerate() {
            let abs = first_line + i as u32;
            let t = line.trim();
            if let Some(name) = strip_prefix_ci(t, "#+end_") {
                let name = name.split_whitespace().next().unwrap_or("");
                index
                    .blocks
                    .entry(name.to_ascii_lowercase())
                    .or_default()
                    .push(abs);
            } else if t.eq_ignore_ascii_case(":end:") {
                index.drawers.push(abs);
            } else if let Some(name) = t.strip_prefix("\\end{").and_then(|r| r.split_once('}')) {
                index.latex.entry(name.0.to_string()).or_default().push(abs);
            }
        }
        index
    }

    /// The first of `ends` after `abs` and no later than `last`.
    fn next(ends: Option<&Vec<u32>>, abs: u32, last: u32) -> Option<u32> {
        let ends = ends?;
        let at = ends.partition_point(|&e| e <= abs);
        ends.get(at).copied().filter(|&e| e <= last)
    }
}

struct Parser<'i> {
    index: &'i Index,
}

impl Parser<'_> {
    /// The elements of `view`.
    fn elements(&self, view: &[Line<'_>], depth: usize) -> Vec<Element> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < view.len() {
            if view[i].text.trim().is_empty() {
                i += 1;
                continue;
            }
            // Affiliated keywords belong to the element right under them;
            // with a blank line or nothing under them, they stand alone.
            let mut affiliated = Affiliated::default();
            let mut j = i;
            while j < view.len() && add_affiliated(&mut affiliated, view[j].text) {
                j += 1;
            }
            if j > i {
                let attaches = j < view.len() && !view[j].text.trim().is_empty();
                if !attaches {
                    out.extend(view[i..j].iter().filter_map(|l| keyword(l)));
                    i = j;
                    continue;
                }
            }
            let (kind, next) = self.element_at(view, j, depth);
            out.push(Element {
                line: view[i].abs,
                affiliated,
                kind,
            });
            i = next;
        }
        out
    }

    /// The element starting at `view[i]`, and the index after it.
    fn element_at(&self, view: &[Line<'_>], i: usize, depth: usize) -> (ElementKind, usize) {
        let line = view[i];
        let t = line.text.trim_start();
        if let Some(end) = self.block_end(view, i) {
            return (self.block(view, i, end, depth), end + 1);
        }
        if let Some(end) = self.latex_end(view, i) {
            let text = view[i..=end]
                .iter()
                .map(|l| l.text)
                .collect::<Vec<_>>()
                .join("\n");
            return (ElementKind::LatexEnvironment(text), end + 1);
        }
        if let Some((name, end)) = self.drawer_end(view, i) {
            let contents = self.nested(&view[i + 1..end], depth);
            return (ElementKind::Drawer { name, contents }, end + 1);
        }
        if let Some((label, rest)) = footnote_start(line.text) {
            return self.footnote(view, i, label, rest, depth);
        }
        if t.starts_with('|') {
            return table(view, i);
        }
        if is_table_el(t) {
            let end = run(view, i, |l| {
                let t = l.trim_start();
                t.starts_with('+') || t.starts_with('|')
            });
            let lines = view[i..end].iter().map(|l| l.text.to_string()).collect();
            return (ElementKind::TableEl(lines), end);
        }
        if item_start(line.text).is_some() {
            return self.list(view, i, depth);
        }
        if is_rule(t) {
            return (ElementKind::HorizontalRule, i + 1);
        }
        if let Some(first) = fixed_width(t) {
            let end = run(view, i, |l| fixed_width(l.trim_start()).is_some());
            let mut lines = vec![first.to_string()];
            lines.extend(
                view[i + 1..end]
                    .iter()
                    .filter_map(|l| fixed_width(l.text.trim_start()).map(str::to_string)),
            );
            return (ElementKind::FixedWidth(lines), end);
        }
        if let Some(first) = comment(t) {
            let end = run(view, i, |l| comment(l.trim_start()).is_some());
            let mut lines = vec![first.to_string()];
            lines.extend(
                view[i + 1..end]
                    .iter()
                    .filter_map(|l| comment(l.text.trim_start()).map(str::to_string)),
            );
            return (ElementKind::Comment(lines), end);
        }
        if let Some(k) = keyword(&line) {
            return (k.kind, i + 1);
        }
        if let Some(rest) = t.strip_prefix("CLOCK:") {
            return (ElementKind::Clock(rest.trim().to_string()), i + 1);
        }
        self.paragraph(view, i)
    }

    /// Lines up to a blank line or the start of another element. The first
    /// line is always the paragraph's, even when it looks like an element
    /// that does not close (`#+begin_src` with no `#+end_src`).
    fn paragraph(&self, view: &[Line<'_>], i: usize) -> (ElementKind, usize) {
        let mut end = i + 1;
        while end < view.len()
            && !view[end].text.trim().is_empty()
            && !self.starts_element(view, end)
        {
            end += 1;
        }
        let text = view[i..end]
            .iter()
            .map(|l| l.text.trim_start())
            .collect::<Vec<_>>()
            .join("\n");
        (ElementKind::Paragraph(text), end)
    }

    /// Whether `view[i]` begins an element, so ends a paragraph above it.
    fn starts_element(&self, view: &[Line<'_>], i: usize) -> bool {
        let text = view[i].text;
        let t = text.trim_start();
        if t.starts_with("#+") {
            return self.block_end(view, i).is_some()
                || keyword_parts(t).is_some()
                || is_affiliated_line(t);
        }
        t.starts_with('|')
            || is_table_el(t)
            || item_start(text).is_some()
            || footnote_start(text).is_some()
            || is_rule(t)
            || fixed_width(t).is_some()
            || comment(t).is_some()
            || t.starts_with("CLOCK:")
            || self.latex_end(view, i).is_some()
            || self.drawer_end(view, i).is_some()
    }

    /// Parse `view` one level deeper, or as one paragraph past [`MAX_DEPTH`].
    fn nested(&self, view: &[Line<'_>], depth: usize) -> Vec<Element> {
        if depth + 1 < MAX_DEPTH {
            return self.elements(view, depth + 1);
        }
        let text = view
            .iter()
            .map(|l| l.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        match view.first() {
            Some(first) if !text.is_empty() => vec![Element {
                line: first.abs,
                affiliated: Affiliated::default(),
                kind: ElementKind::Paragraph(text),
            }],
            _ => Vec::new(),
        }
    }

    /// The view index of the `#+end_NAME` closing a `#+begin_NAME` at `i`.
    fn block_end(&self, view: &[Line<'_>], i: usize) -> Option<usize> {
        let name = begin_name(view[i].text.trim_start())?;
        let last = view.last()?.abs;
        let end = Index::next(self.index.blocks.get(&name), view[i].abs, last)?;
        Some(i + (end - view[i].abs) as usize)
    }

    fn latex_end(&self, view: &[Line<'_>], i: usize) -> Option<usize> {
        let t = view[i].text.trim_start();
        let (name, _) = t.strip_prefix("\\begin{")?.split_once('}')?;
        let last = view.last()?.abs;
        let end = Index::next(self.index.latex.get(name), view[i].abs, last)?;
        Some(i + (end - view[i].abs) as usize)
    }

    fn drawer_end(&self, view: &[Line<'_>], i: usize) -> Option<(String, usize)> {
        let t = view[i].text.trim();
        let name = t.strip_prefix(':')?.strip_suffix(':')?;
        if name.is_empty()
            || name.eq_ignore_ascii_case("end")
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
        let last = view.last()?.abs;
        let end = Index::next(Some(&self.index.drawers), view[i].abs, last)?;
        Some((name.to_string(), i + (end - view[i].abs) as usize))
    }

    fn block(&self, view: &[Line<'_>], i: usize, end: usize, depth: usize) -> ElementKind {
        let t = view[i].text.trim();
        let after = &t[2 + "begin_".len()..];
        let (name, rest) = after
            .split_once(char::is_whitespace)
            .map_or((after, ""), |(n, r)| (n, r.trim()));
        let name = name.to_ascii_lowercase();
        let first_word = |s: &str| -> (String, String) {
            match s.split_once(char::is_whitespace) {
                Some((w, r)) => (w.to_string(), r.trim().to_string()),
                None => (s.to_string(), String::new()),
            }
        };
        let (kind, parameters) = match name.as_str() {
            "src" => {
                let (language, parameters) = first_word(rest);
                (BlockKind::Src { language }, parameters)
            }
            "export" => {
                let (backend, parameters) = first_word(rest);
                (
                    BlockKind::Export {
                        backend: backend.to_ascii_lowercase(),
                    },
                    parameters,
                )
            }
            "example" => (BlockKind::Example, rest.to_string()),
            "verse" => (BlockKind::Verse, rest.to_string()),
            "comment" => (BlockKind::Comment, rest.to_string()),
            "quote" => (BlockKind::Quote, rest.to_string()),
            "center" => (BlockKind::Center, rest.to_string()),
            _ => (BlockKind::Special(name.clone()), rest.to_string()),
        };
        let inner = &view[i + 1..end];
        let (body, contents) = if kind.is_greater() {
            (String::new(), self.nested(inner, depth))
        } else {
            (
                block_body(
                    inner,
                    matches!(kind, BlockKind::Src { .. } | BlockKind::Example),
                ),
                Vec::new(),
            )
        };
        ElementKind::Block(Block {
            kind,
            parameters,
            body,
            contents,
        })
    }

    /// A footnote definition: to the next definition, two blank lines, or the
    /// end of the section.
    fn footnote(
        &self,
        view: &[Line<'_>],
        i: usize,
        label: String,
        rest: &str,
        depth: usize,
    ) -> (ElementKind, usize) {
        let mut end = i + 1;
        let mut blanks = 0;
        while end < view.len() {
            let text = view[end].text;
            if text.trim().is_empty() {
                blanks += 1;
                if blanks == 2 {
                    break;
                }
            } else {
                if footnote_start(text).is_some() {
                    break;
                }
                blanks = 0;
            }
            end += 1;
        }
        let mut body: Vec<Line<'_>> = vec![Line {
            abs: view[i].abs,
            text: rest,
        }];
        body.extend_from_slice(&view[i + 1..end]);
        let contents = self.nested(&body, depth);
        (ElementKind::FootnoteDefinition { label, contents }, end)
    }

    /// A plain list: items at the first item's indentation, to a line indented
    /// no deeper that is not one of its items, or to two blank lines.
    fn list(&self, view: &[Line<'_>], i: usize, depth: usize) -> (ElementKind, usize) {
        let base = indent(view[i].text);
        let mut items = Vec::new();
        let mut k = i;
        loop {
            let Some(start) = item_start(view[k].text).filter(|s| s.indent == base) else {
                break;
            };
            let mut end = k + 1;
            let mut blanks = 0;
            while end < view.len() {
                let text = view[end].text;
                if text.trim().is_empty() {
                    blanks += 1;
                    if blanks == 2 {
                        break;
                    }
                } else {
                    if indent(text) <= base {
                        break;
                    }
                    blanks = 0;
                }
                end += 1;
            }
            let mut last = end;
            while last > k + 1 && view[last - 1].text.trim().is_empty() {
                last -= 1;
            }
            let more = view[k + 1..last]
                .iter()
                .filter(|l| !l.text.trim().is_empty())
                .map(|l| indent(l.text))
                .min()
                .unwrap_or(0);
            let mut body: Vec<Line<'_>> = vec![Line {
                abs: view[k].abs,
                text: &view[k].text[start.content..],
            }];
            body.extend(view[k + 1..last].iter().map(|l| Line {
                abs: l.abs,
                text: &l.text[more.min(indent(l.text))..],
            }));
            items.push(Item {
                line: view[k].abs,
                bullet: start.bullet.to_string(),
                counter: start.counter,
                checkbox: start.checkbox,
                tag: start.tag.map(str::to_string),
                contents: self.nested(&body, depth),
            });
            k = end;
            if blanks == 2 || k >= view.len() {
                break;
            }
        }
        let first = &items[0];
        let ordered = first.bullet.starts_with(|c: char| c.is_ascii_digit());
        let kind = if ordered {
            ListKind::Ordered
        } else if first.tag.is_some() {
            ListKind::Descriptive
        } else {
            ListKind::Unordered
        };
        (ElementKind::PlainList(List { kind, items }), k.max(i + 1))
    }
}

/// `#+begin_NAME` as `NAME` lowercased.
fn begin_name(t: &str) -> Option<String> {
    let after = strip_prefix_ci(t, "#+begin_")?;
    let name = after.split_whitespace().next()?;
    Some(name.to_ascii_lowercase())
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// A lesser block's body as text: common indentation removed and, for source
/// and example blocks, one escaping comma taken off a line that begins
/// `,*` or `,#+` (org's `org-unescape-code-in-string`).
fn block_body(lines: &[Line<'_>], unescape: bool) -> String {
    let common = lines
        .iter()
        .filter(|l| !l.text.trim().is_empty())
        .map(|l| indent(l.text))
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            let text = &l.text[common.min(indent(l.text))..];
            if unescape {
                let t = text.trim_start();
                let lead = text.len() - t.len();
                let after = t.trim_start_matches(',');
                if t.starts_with(',') && (after.starts_with('*') || after.starts_with("#+")) {
                    return format!("{}{}", &text[..lead], &t[1..]);
                }
            }
            text.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn indent(text: &str) -> usize {
    text.len() - text.trim_start().len()
}

/// How far a run of lines matching `keep` goes from `i` (which matches).
fn run(view: &[Line<'_>], i: usize, keep: impl Fn(&str) -> bool) -> usize {
    let mut end = i + 1;
    while end < view.len() && keep(view[end].text) {
        end += 1;
    }
    end
}

fn table(view: &[Line<'_>], i: usize) -> (ElementKind, usize) {
    let end = run(view, i, |l| l.trim_start().starts_with('|'));
    let mut rows = Vec::new();
    let mut align: Vec<Option<Align>> = Vec::new();
    for l in &view[i..end] {
        let t = l.text.trim();
        if t.starts_with("|-") {
            rows.push(Row::Rule);
            continue;
        }
        let inner = t.strip_prefix('|').unwrap_or(t);
        let inner = inner.strip_suffix('|').unwrap_or(inner);
        let cells: Vec<String> = inner.split('|').map(|c| c.trim().to_string()).collect();
        // A row of nothing but cookies and empty cells, with at least one
        // cookie, is the alignment row; it is not exported.
        let cookies: Option<Vec<Option<Align>>> = cells
            .iter()
            .map(|c| if c.is_empty() { Some(None) } else { cookie(c) })
            .collect();
        match cookies {
            Some(cols) if cells.iter().any(|c| !c.is_empty()) => align = cols,
            _ => rows.push(Row::Cells(cells)),
        }
    }
    let mut next = end;
    let mut formulas = Vec::new();
    while next < view.len() {
        match keyword_parts(view[next].text) {
            Some((key, value)) if key == "TBLFM" => formulas.push(value.to_string()),
            _ => break,
        }
        next += 1;
    }
    (
        ElementKind::Table(Table {
            rows,
            align,
            formulas,
        }),
        next,
    )
}

/// A column cookie, `<l>`, `<c10>`, `<r>`, `<10>`: its alignment, `None`
/// inside for a width-only cookie.
fn cookie(cell: &str) -> Option<Option<Align>> {
    let inner = cell.strip_prefix('<')?.strip_suffix('>')?;
    let (align, width) = match inner.chars().next() {
        Some('l') => (Some(Align::Left), &inner[1..]),
        Some('c') => (Some(Align::Center), &inner[1..]),
        Some('r') => (Some(Align::Right), &inner[1..]),
        _ => (None, inner),
    };
    (width.chars().all(|c| c.is_ascii_digit()) && (align.is_some() || !width.is_empty()))
        .then_some(align)
}

fn is_table_el(t: &str) -> bool {
    t.starts_with("+-") && t.trim_end().ends_with('+')
}

/// Five or more dashes, alone.
fn is_rule(t: &str) -> bool {
    let t = t.trim_end();
    t.len() >= 5 && t.bytes().all(|b| b == b'-')
}

/// A `: ` line's text.
fn fixed_width(t: &str) -> Option<&str> {
    if t.trim_end() == ":" {
        return Some("");
    }
    t.strip_prefix(": ")
}

/// A `# ` comment line's text.
fn comment(t: &str) -> Option<&str> {
    if t.trim_end() == "#" {
        return Some("");
    }
    t.strip_prefix("# ")
}

/// `[fn:LABEL]` at column 0, and what follows it.
fn footnote_start(text: &str) -> Option<(String, &str)> {
    let rest = text.strip_prefix("[fn:")?;
    let close = rest.find(']')?;
    let label = &rest[..close];
    if label.is_empty()
        || !label
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((label.to_string(), rest[close + 1..].trim_start()))
}

/// The keywords that belong to the element below them, and their synonyms.
fn affiliated_key(key: &str) -> Option<&'static str> {
    let key = key.split_once('[').map_or(key, |(k, _)| k);
    Some(match key {
        "CAPTION" => "CAPTION",
        "NAME" | "DATA" | "LABEL" | "RESNAME" | "SOURCE" | "SRCNAME" | "TBLNAME" => "NAME",
        "RESULTS" | "RESULT" => "RESULTS",
        "HEADER" | "HEADERS" => "HEADER",
        "PLOT" => "PLOT",
        k if k.starts_with("ATTR_") => "ATTR",
        _ => return None,
    })
}

fn is_affiliated_line(t: &str) -> bool {
    keyword_parts(t).is_some_and(|(key, _)| affiliated_key(&key).is_some())
}

/// Fold an affiliated keyword line into `into`; `false` when it is not one.
fn add_affiliated(into: &mut Affiliated, text: &str) -> bool {
    let Some((key, value)) = keyword_parts(text) else {
        return false;
    };
    let Some(kind) = affiliated_key(&key) else {
        return false;
    };
    let value = value.to_string();
    match kind {
        "CAPTION" => into.caption.push(value),
        "NAME" => into.name = Some(value),
        "RESULTS" => into.results = Some(value),
        "HEADER" => into.header.push(value),
        "ATTR" => into
            .attr
            .push((key["ATTR_".len()..].to_ascii_lowercase(), value)),
        // `#+PLOT:` is gnuplot's, which export does not run.
        _ => {}
    }
    true
}

fn keyword(line: &Line<'_>) -> Option<Element> {
    let (key, value) = keyword_parts(line.text)?;
    Some(Element {
        line: line.abs,
        affiliated: Affiliated::default(),
        kind: ElementKind::Keyword {
            key,
            value: value.to_string(),
        },
    })
}

/// What an item's first line says before its contents.
struct ItemStart<'a> {
    indent: usize,
    bullet: &'a str,
    counter: Option<u32>,
    checkbox: Option<Checkbox>,
    tag: Option<&'a str>,
    /// Where the contents start, in the line.
    content: usize,
}

/// The parts of a list item's first line, or `None` when `text` is not one.
/// `*` is a bullet only when indented: at column 0 it is a headline.
fn item_start(text: &str) -> Option<ItemStart<'_>> {
    let indent = indent(text);
    let t = &text[indent..];
    let bullet_len = match t.as_bytes().first()? {
        b'-' | b'+' => 1,
        b'*' if indent > 0 => 1,
        b'0'..=b'9' => {
            let digits = t.bytes().take_while(u8::is_ascii_digit).count();
            match t.as_bytes().get(digits) {
                Some(b'.' | b')') => digits + 1,
                _ => return None,
            }
        }
        _ => return None,
    };
    match t.as_bytes().get(bullet_len) {
        None | Some(b' ' | b'\t') => {}
        _ => return None,
    }
    let bullet = &t[..bullet_len];
    let mut pos = indent + bullet_len;
    let skip = |pos: usize| pos + (text.len() - pos - text[pos..].trim_start().len());
    pos = skip(pos);
    let mut counter = None;
    if let Some(rest) = text[pos..].strip_prefix("[@") {
        if let Some(close) = rest.find(']') {
            if let Ok(n) = rest[..close].parse::<u32>() {
                counter = Some(n);
                pos = skip(pos + 2 + close + 1);
            }
        }
    }
    let mut checkbox = None;
    let rest = &text[pos..];
    if rest.len() >= 3 && rest.as_bytes()[0] == b'[' && rest.as_bytes()[2] == b']' {
        let after = rest.as_bytes().get(3);
        if matches!(after, None | Some(b' ' | b'\t')) {
            checkbox = match rest.as_bytes()[1] {
                b' ' => Some(Checkbox::Off),
                b'X' | b'x' => Some(Checkbox::On),
                b'-' => Some(Checkbox::Partial),
                _ => None,
            };
            if checkbox.is_some() {
                pos = skip(pos + 3);
            }
        }
    }
    let mut tag = None;
    let ordered = bullet.as_bytes()[0].is_ascii_digit();
    if !ordered {
        let rest = &text[pos..];
        if let Some(sep) = rest
            .find(" ::")
            .filter(|&s| matches!(rest.as_bytes().get(s + 3), None | Some(b' ' | b'\t')))
        {
            tag = Some(rest[..sep].trim());
            pos = skip(pos + sep + 3);
        }
    }
    Some(ItemStart {
        indent,
        bullet,
        counter,
        checkbox,
        tag,
        content: pos,
    })
}

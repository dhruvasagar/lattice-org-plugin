//! The Markdown transcoder: GitHub-flavoured, because that is where Markdown
//! is read. Its shape is ox-md's (ATX headings, headlines below `H:` as list
//! items, the table of contents as a nested list, anchors only where
//! something links), and where ox-md falls back to raw HTML it uses GFM's
//! own: pipe tables, `[^n]` footnotes, task-list boxes, `~~strike~~`
//! (lattice's `org-export.md` §1).
//!
//! It is written as the render walks, a headline at a time, like the HTML
//! writer: [`Markdown::start`] writes the contents and the preamble,
//! [`Markdown::headline`] one visited headline, [`Markdown::finish`] the
//! footnotes. Each element is a block of text; blocks are separated by one
//! blank line, and a block inside a list item or a quote is indented or
//! prefixed as a whole.

use std::collections::HashSet;

use super::common::{self, Definition, Footnotes, Outline};
use super::filter::{self, View};
use super::options::{Options, Tags, Tex};
use super::parse::objects::{Emphasis, Object};
use super::parse::{
    Align, Block, BlockKind, Checkbox, Doc, Element, ElementKind, Headline, Item, ListKind, Row,
    Table,
};
use super::IdFile;

/// What a Markdown export takes from outside the tree.
pub(crate) struct Settings {
    /// The exported file's path, which other files' links are relative to.
    pub(crate) source: String,
    /// Leave out the table of contents.
    pub(crate) body_only: bool,
    /// The anchors some link goes to (`Targets::referred`).
    pub(crate) referred: HashSet<String>,
}

/// The document-wide state of one Markdown export.
pub(crate) struct Markdown {
    outline: Outline,
    footnotes: Footnotes,
    /// Anchors a heading or a named element writes: the linked-to ones and
    /// the contents'. Markdown has no anchor of its own, and an `<a id>`
    /// on every heading would be noise in a file people read as text.
    anchored: HashSet<String>,
    dir: String,
    /// The levels of the list-item headlines (below `H:`) open around the
    /// next one: its nesting is how many are, not its level, since levels
    /// may skip and an indent with no item above it is a code block.
    deep: Vec<usize>,
    body_only: bool,
    /// The next contents entry [`Markdown::index`] writes.
    toc_next: usize,
}

/// `text` with every line after the first indented by `width` spaces, as a
/// list item's or a footnote's continuation is.
fn hang(text: &str, width: usize) -> String {
    let pad = " ".repeat(width);
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(&pad);
            }
        }
        out.push_str(line);
    }
    out
}

/// `text` with every line indented by `width` spaces.
fn indent(text: &str, width: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    format!("{}{}", " ".repeat(width), hang(text, width))
}

/// The longest run of backticks in `s`.
fn longest_backticks(s: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in s.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// A code fence longer than any run of backticks in `body`, at least three.
fn fence(body: &str) -> String {
    "`".repeat((longest_backticks(body) + 1).max(3))
}

/// `body` fenced, ending in a newline before the closing fence.
fn fenced(info: &str, body: &str) -> String {
    let body = if body.is_empty() || body.ends_with('\n') {
        body.to_string()
    } else {
        format!("{body}\n")
    };
    let f = fence(&body);
    format!("{f}{info}\n{body}{f}")
}

/// `code` as a code span: delimited by more backticks than it holds in a
/// row, padded when it starts or ends with one.
fn code_span(code: &str, out: &mut String) {
    let ticks = "`".repeat(longest_backticks(code) + 1);
    let pad = code.starts_with('`') || code.ends_with('`');
    out.push_str(&ticks);
    if pad {
        out.push(' ');
    }
    out.push_str(code);
    if pad {
        out.push(' ');
    }
    out.push_str(&ticks);
}

/// Text: Markdown's inline punctuation escaped, `<` and `&` as entities so
/// no tag or entity forms, `--` / `---` / `...` typeset when `-:` is on,
/// and line breaks hard when `\n:` is.
fn text(t: &str, o: &Options, out: &mut String) {
    let mut rest = t;
    while let Some(c) = rest.chars().next() {
        if o.special_strings {
            let special = [
                ("\\-", "\u{ad}"),
                ("---", "\u{2014}"),
                ("--", "\u{2013}"),
                ("...", "\u{2026}"),
            ]
            .into_iter()
            .find(|(from, _)| rest.starts_with(from));
            if let Some((from, to)) = special {
                out.push_str(to);
                rest = &rest[from.len()..];
                continue;
            }
        }
        if c == '\n' && o.preserve_breaks {
            out.push_str("\\\n");
        } else {
            escape_char(c, out);
        }
        rest = &rest[c.len_utf8()..];
    }
}

/// One character of text, escaped if Markdown would read it as markup. A
/// `|` is escaped everywhere, since in a table cell it ends the cell, and a
/// `$`, which GitHub reads as math.
fn escape_char(c: char, out: &mut String) {
    match c {
        '\\' | '`' | '*' | '_' | '[' | ']' | '~' | '|' | '$' => {
            out.push('\\');
            out.push(c);
        }
        '<' => out.push_str("&lt;"),
        '&' => out.push_str("&amp;"),
        c => out.push(c),
    }
}

/// A paragraph's lines, each escaped where it would otherwise start a
/// heading, a quote, a list item or a setext underline.
fn guard_line_starts(block: &str) -> String {
    let mut out = String::with_capacity(block.len());
    for (i, line) in block.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start_matches(' ');
        let lead = &line[..line.len() - trimmed.len()];
        let bytes = trimmed.as_bytes();
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        let ordered = digits > 0
            && matches!(bytes.get(digits), Some(b'.' | b')'))
            && matches!(bytes.get(digits + 1), None | Some(b' '));
        let bullet =
            matches!(bytes.first(), Some(b'-' | b'+')) && matches!(bytes.get(1), None | Some(b' '));
        let marker = matches!(bytes.first(), Some(b'#' | b'>' | b'=')) || bullet;
        if ordered {
            out.push_str(lead);
            out.push_str(&trimmed[..digits]);
            out.push('\\');
            out.push_str(&trimmed[digits..]);
        } else if marker {
            out.push_str(lead);
            out.push('\\');
            out.push_str(trimmed);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Append a block and the blank line after it.
fn push_block(out: &mut String, block: &str) {
    if block.is_empty() {
        return;
    }
    out.push_str(block);
    out.push_str("\n\n");
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// The backends whose raw output Markdown passes through: its own, and
/// HTML, which every Markdown renderer accepts inline.
fn is_raw_backend(backend: &str) -> bool {
    matches!(
        backend.to_ascii_lowercase().as_str(),
        "md" | "markdown" | "html"
    )
}

impl Markdown {
    /// The writer for a numbered outline.
    pub(crate) fn new(outline: Outline, settings: Settings) -> Markdown {
        let Settings {
            source,
            body_only,
            referred,
        } = settings;
        let dir = std::path::Path::new(&source)
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        Markdown {
            outline,
            footnotes: Footnotes::default(),
            anchored: referred,
            dir,
            deep: Vec::new(),
            body_only,
            toc_next: 0,
        }
    }

    /// Write the next contents entries, about `budget` bytes of them, into
    /// `toc`: `true` once all are. A nested list of links, four spaces a
    /// level; a numbered entry is an ordered item numbered as its headline,
    /// so the nested lists read as the outline's numbers (ox-md's shape).
    /// Every listed heading is anchored, since the contents links to it.
    pub(crate) fn index(&mut self, view: &mut View, toc: &mut String, budget: usize) -> bool {
        if self.body_only {
            return true;
        }
        let from = toc.len();
        let top = self.outline.toc_top;
        // Out of the outline while `self` writes, and back after.
        let entries = std::mem::take(&mut self.outline.toc);
        while self.toc_next < entries.len() && toc.len() - from < budget {
            let e = &entries[self.toc_next];
            self.toc_next += 1;
            let mut title = String::new();
            self.plain_objects(&e.title, view, &mut title);
            let bullet = match e.number.as_deref().and_then(|n| n.rsplit('.').next()) {
                Some(last) => format!("{last}."),
                None => "-".to_string(),
            };
            if !toc.is_empty() {
                toc.push('\n');
            }
            toc.push_str(&format!(
                "{}{bullet} [{}](#{})",
                " ".repeat(4 * (e.level - top)),
                title.trim(),
                e.anchor
            ));
            if !e.tags.is_empty() {
                toc.push_str(&format!("   `:{}:`", e.tags.join(":")));
            }
            self.anchored.insert(e.anchor.clone());
        }
        let done = self.toc_next == entries.len();
        self.outline.toc = entries;
        done
    }

    /// Write the contents and the text before the first headline.
    pub(crate) fn begin(&mut self, doc: &Doc, view: &mut View, toc: String, out: &mut String) {
        push_block(out, &toc);
        let preamble = filter::elements(&doc.preamble, view);
        let body = self.blocks(&preamble, view);
        push_block(out, &body);
    }

    /// One visited headline, already filtered (`filter::own`).
    pub(crate) fn headline(&mut self, h: &Headline, view: &mut View, out: &mut String) {
        if common::is_footnote_section(h) {
            // Collected for the footnotes; the heading is not exported, as
            // in ox.
            self.footnotes.collect(&h.section);
            return;
        }
        let level = self.outline.level(h);
        let anchor = view.targets().headline_anchor(h.line);
        let number = self.outline.number(h);
        let mut heading = String::new();
        if self.anchored.contains(&anchor) {
            heading.push_str(&format!("<a id=\"{anchor}\"></a>"));
        }
        let mut body = self.meta(h, view);
        let section = self.blocks(&h.section, view);
        if !section.is_empty() {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(&section);
        }
        // ATX headings stop at six; past that, as below `H:`, a headline is
        // an item in a list of its siblings, numbered by its own number
        // alone: ox-md's shape. The bullet is four wide, so the body and
        // the next level down sit four spaces in.
        let deepest = view.options.headline_levels.min(6);
        if level > deepest {
            let bullet = match number.as_deref().and_then(|n| n.rsplit('.').next()) {
                Some(last) => format!("{last}."),
                None => "-".to_string(),
            };
            self.headline_text(h, view, &mut heading);
            let mut item = format!("{bullet:<4}{}", heading.trim_end());
            if !body.is_empty() {
                item.push_str("\n\n");
                item.push_str(&indent(&body, 4));
            }
            while self.deep.last().is_some_and(|&l| l >= level) {
                self.deep.pop();
            }
            push_block(out, &indent(&item, 4 * self.deep.len()));
            self.deep.push(level);
        } else {
            self.deep.clear();
            if let Some(n) = &number {
                heading.push_str(n);
                heading.push_str(". ");
            }
            self.headline_text(h, view, &mut heading);
            push_block(
                out,
                &format!("{} {}", "#".repeat(level), heading.trim_end()),
            );
            push_block(out, &body);
        }
    }

    fn headline_text(&mut self, h: &Headline, view: &mut View, out: &mut String) {
        let o = &view.options;
        if o.todo {
            if let Some(kw) = &h.todo {
                out.push_str(kw);
                out.push(' ');
            }
        }
        if o.priority {
            if let Some(p) = h.priority {
                out.push_str(&format!("\\[#{p}\\] "));
            }
        }
        let tags = o.tags != Tags::Off && !h.tags.is_empty();
        let mut title = String::new();
        self.objects(&h.title, view, &mut title);
        out.push_str(title.trim_end());
        if tags {
            out.push_str(&format!("   `:{}:`", h.tags.join(":")));
        }
    }

    /// Planning and properties, when the options kept them.
    fn meta(&mut self, h: &Headline, view: &mut View) -> String {
        let mut out = String::new();
        let p = &h.planning;
        if !p.is_empty() {
            let mut parts = Vec::new();
            for (kw, stamp) in [
                ("SCHEDULED:", &p.scheduled),
                ("DEADLINE:", &p.deadline),
                ("CLOSED:", &p.closed),
            ] {
                if let Some(stamp) = stamp {
                    let mut part = format!("**{kw}** ");
                    text(stamp, &view.options, &mut part);
                    parts.push(part);
                }
            }
            out.push_str(&parts.join(" "));
        }
        if !h.properties.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            let body: String = h
                .properties
                .iter()
                .map(|(k, v)| format!("{k}: {v}\n"))
                .collect();
            out.push_str(&fenced("", &body));
        }
        out
    }

    /// Elements as blocks, one blank line apart. Two lists in a row are
    /// two lists in org (a blank line or two ended the first), but one in
    /// Markdown unless something stands between them: an empty comment,
    /// CommonMark's own answer.
    fn blocks(&mut self, list: &[Element], view: &mut View) -> String {
        let mut out = String::new();
        let mut after_list = false;
        for e in list {
            let block = self.element(e, view);
            if block.is_empty() {
                continue;
            }
            let is_list = matches!(e.kind, ElementKind::PlainList(_));
            if !out.is_empty() {
                out.push_str(if is_list && after_list {
                    "\n\n<!-- -->\n\n"
                } else {
                    "\n\n"
                });
            }
            out.push_str(&block);
            after_list = is_list;
        }
        out
    }

    /// The anchor a `#+NAME:`d element writes, when something links to it.
    fn named(&self, e: &Element, view: &View) -> String {
        let Some(name) = &e.affiliated.name else {
            return String::new();
        };
        let anchor = view.targets().name_anchor(name);
        if self.anchored.contains(&anchor) {
            format!("<a id=\"{anchor}\"></a>\n")
        } else {
            String::new()
        }
    }

    /// One element, as a block (no blank lines around it).
    fn element(&mut self, e: &Element, view: &mut View) -> String {
        let block = match &e.kind {
            ElementKind::Paragraph(objects) => match common::standalone_image(objects) {
                Some(path) => self.figure(e, path, view),
                None => {
                    let mut p = String::new();
                    self.objects(objects, view, &mut p);
                    guard_line_starts(p.trim_end())
                }
            },
            ElementKind::PlainList(list) => self.list(list.kind, &list.items, view),
            ElementKind::Table(t) => self.table(t, view),
            ElementKind::TableEl(lines) | ElementKind::FixedWidth(lines) => {
                fenced("", &(lines.join("\n") + "\n"))
            }
            ElementKind::Block(b) => self.block(b, view),
            ElementKind::Drawer { contents, .. } => self.blocks(contents, view),
            ElementKind::Keyword { key, value } => match key.as_str() {
                "MD" | "MARKDOWN" | "HTML" => value.clone(),
                "TOC" => {
                    view.options.ignore("#+TOC".to_string());
                    String::new()
                }
                _ => String::new(),
            },
            ElementKind::FootnoteDefinition { .. } => {
                self.footnotes.collect(std::slice::from_ref(e));
                String::new()
            }
            ElementKind::Comment(_) => String::new(),
            ElementKind::HorizontalRule => "---".to_string(),
            ElementKind::LatexEnvironment(tex) => match view.options.tex {
                Tex::On => format!("$$\n{}\n$$", tex.trim_end()),
                Tex::Verbatim => fenced("", tex.trim_end()),
                Tex::Off => String::new(),
            },
            ElementKind::Clock(value) => {
                let mut out = String::from("**CLOCK:** ");
                text(value, &view.options, &mut out);
                out
            }
        };
        if block.is_empty() {
            return block;
        }
        self.named(e, view) + &block
    }

    /// A paragraph that is one image, its caption the alt text (ox-md's):
    /// Markdown has no figure to hold a caption apart.
    fn figure(&mut self, e: &Element, path: &str, view: &mut View) -> String {
        let mut alt = String::new();
        for (i, c) in e.affiliated.caption.iter().enumerate() {
            if i > 0 {
                alt.push(' ');
            }
            self.plain_objects(c, view, &mut alt);
        }
        let alt = if alt.trim().is_empty() {
            let mut name = String::new();
            text(file_name(path), &view.options, &mut name);
            name
        } else {
            alt.trim().to_string()
        };
        format!("![{alt}]({})", self.href(path).replace(' ', "%20"))
    }

    fn list(&mut self, kind: ListKind, items: &[Item], view: &mut View) -> String {
        let mut out = String::new();
        let mut n = 0;
        for item in items {
            n = item.counter.map_or(n + 1, |c| c as usize);
            let bullet = match kind {
                ListKind::Ordered => format!("{n}."),
                ListKind::Unordered | ListKind::Descriptive => "-".to_string(),
            };
            let mut head = String::new();
            if let Some(c) = item.checkbox {
                head.push_str(match c {
                    Checkbox::On => "[x] ",
                    Checkbox::Off => "[ ] ",
                    // A GFM task box has two states; the partial one is
                    // written as the cookie, as ox-md does.
                    Checkbox::Partial => "\\[-\\] ",
                });
            }
            if kind == ListKind::Descriptive {
                if let Some(tag) = &item.tag {
                    // The term is bold already; a bold term is not bolder.
                    let tag = match &tag[..] {
                        [Object::Emphasis {
                            kind: Emphasis::Bold,
                            contents,
                        }] => contents,
                        _ => tag,
                    };
                    let mut term = String::new();
                    self.objects(tag, view, &mut term);
                    head.push_str(&format!("**{}:** ", term.trim()));
                }
            }
            let body = self.blocks(&item.contents, view);
            let content = format!("{head}{body}");
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&bullet);
            out.push(' ');
            out.push_str(&hang(content.trim_end(), bullet.len() + 1));
        }
        out
    }

    /// A GFM pipe table. GFM requires a header row: the rows before the
    /// first rule are org's header, and a table without one gets an empty
    /// header so its first row stays data. GFM has one header row, so
    /// further header rows become body rows.
    fn table(&mut self, t: &Table, view: &mut View) -> String {
        let cells = |r: &Row| match r {
            Row::Cells(c) => Some(c.clone()),
            Row::Rule => None,
        };
        let columns = t
            .rows
            .iter()
            .filter_map(cells)
            .map(|c| c.len())
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return String::new();
        }
        let first_rule = t.rows.iter().position(|r| matches!(r, Row::Rule));
        let (header, body): (Vec<_>, Vec<_>) = match first_rule {
            Some(i) if i > 0 => (
                t.rows[..i].iter().filter_map(cells).collect(),
                t.rows[i + 1..].iter().filter_map(cells).collect(),
            ),
            _ => (Vec::new(), t.rows.iter().filter_map(cells).collect()),
        };
        let mut out = self.row(header.first().map(Vec::as_slice), columns, view);
        out.push_str("\n|");
        for i in 0..columns {
            out.push_str(match t.align.get(i).copied().flatten() {
                Some(Align::Left) => " :--- |",
                Some(Align::Center) => " :---: |",
                Some(Align::Right) => " ---: |",
                None => " --- |",
            });
        }
        for row in header.iter().skip(1).chain(&body) {
            out.push('\n');
            out.push_str(&self.row(Some(row), columns, view));
        }
        out
    }

    /// One table row, padded to `columns` cells. A cell holds no raw `|`:
    /// org splits cells on every one, and the text and entities that give
    /// one back are escaped.
    fn row(&mut self, cells: Option<&[Vec<Object>]>, columns: usize, view: &mut View) -> String {
        let mut line = String::from("|");
        for i in 0..columns {
            let mut cell = String::new();
            if let Some(c) = cells.and_then(|c| c.get(i)) {
                self.objects(c, view, &mut cell);
            }
            line.push(' ');
            line.push_str(cell.replace('\n', " ").trim());
            line.push_str(" |");
        }
        line
    }

    fn block(&mut self, b: &Block, view: &mut View) -> String {
        match &b.kind {
            BlockKind::Src { language } => fenced(language, &b.body),
            BlockKind::Example => fenced("", &b.body),
            BlockKind::Export { backend } if is_raw_backend(backend) => {
                b.body.trim_end_matches('\n').to_string()
            }
            BlockKind::Export { .. } | BlockKind::Comment => String::new(),
            BlockKind::Verse => {
                let objects = super::parse::objects::parse(b.body.trim_end_matches('\n'));
                let objects = filter::objects(&objects, view);
                let mut verse = String::new();
                self.objects(&objects, view, &mut verse);
                // Hard breaks keep the lines, and no-break spaces their
                // indentation, which Markdown would otherwise collapse.
                let lines: Vec<String> = verse
                    .split('\n')
                    .map(|line| {
                        let spaces = line.len() - line.trim_start_matches(' ').len();
                        format!("{}{}", "\u{a0}".repeat(spaces), &line[spaces..])
                    })
                    .collect();
                guard_line_starts(&lines.join("\\\n"))
            }
            BlockKind::Quote => self
                .blocks(&b.contents, view)
                .split('\n')
                .map(|l| {
                    if l.is_empty() {
                        ">".to_string()
                    } else {
                        format!("> {l}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            // Markdown has no centring or custom blocks; the contents stand.
            BlockKind::Center | BlockKind::Special(_) => self.blocks(&b.contents, view),
        }
    }

    /// Objects for a place that holds no links: a contents entry, an
    /// image's alt text.
    fn plain_objects(&mut self, list: &[Object], view: &mut View, out: &mut String) {
        for o in list {
            match o {
                Object::Link {
                    path, description, ..
                } => {
                    if description.is_empty() {
                        text(path, &view.options, out);
                    } else {
                        self.plain_objects(description, view, out);
                    }
                }
                Object::FootnoteReference { .. } | Object::Target(_) => {}
                Object::RadioTarget(c) => self.plain_objects(c, view, out),
                other => self.object(other, view, out),
            }
        }
    }

    /// Objects, as Markdown.
    pub(crate) fn objects(&mut self, list: &[Object], view: &mut View, out: &mut String) {
        for o in list {
            self.object(o, view, out);
        }
    }

    fn object(&mut self, obj: &Object, view: &mut View, out: &mut String) {
        match obj {
            Object::Text(t) => text(t, &view.options, out),
            Object::Emphasis { kind, contents } => {
                let (open, close) = match kind {
                    Emphasis::Bold => ("**", "**"),
                    Emphasis::Italic => ("*", "*"),
                    Emphasis::Underline => ("<u>", "</u>"),
                    Emphasis::Strike => ("~~", "~~"),
                };
                out.push_str(open);
                self.objects(contents, view, out);
                out.push_str(close);
            }
            Object::Verbatim(t) | Object::Code(t) => code_span(t, out),
            Object::Link {
                path,
                description,
                target,
            } => self.link(path, description, target.as_deref(), view, out),
            Object::FootnoteReference { label, definition } => {
                let (n, _) = self.footnotes.reference(label.as_deref(), definition);
                out.push_str(&format!("[^{n}]"));
            }
            // `\vert` is a `|`, `\lt` a `<`: still text.
            Object::Entity { utf8, .. } => utf8.chars().for_each(|c| escape_char(c, out)),
            Object::LatexFragment(t) => match view.options.tex {
                // GitHub renders `$…$` and `$$…$$`; the other delimiters are
                // written as those.
                Tex::On => {
                    if let Some(inner) = t.strip_prefix("\\(").and_then(|r| r.strip_suffix("\\)")) {
                        out.push_str(&format!("${inner}$"));
                    } else if let Some(inner) =
                        t.strip_prefix("\\[").and_then(|r| r.strip_suffix("\\]"))
                    {
                        out.push_str(&format!("$${inner}$$"));
                    } else {
                        out.push_str(t);
                    }
                }
                Tex::Verbatim => code_span(t, out),
                Tex::Off => {}
            },
            Object::Subscript { contents, .. } => {
                out.push_str("<sub>");
                self.objects(contents, view, out);
                out.push_str("</sub>");
            }
            Object::Superscript { contents, .. } => {
                out.push_str("<sup>");
                self.objects(contents, view, out);
                out.push_str("</sup>");
            }
            // A hard break; the newline after it is the text's own.
            Object::LineBreak => out.push('\\'),
            Object::Timestamp(t) => text(t, &view.options, out),
            Object::Target(t) => {
                out.push_str(&format!(
                    "<a id=\"{}\"></a>",
                    view.targets().target_anchor(t)
                ));
            }
            Object::RadioTarget(contents) => {
                let name = filter::plain(contents);
                out.push_str(&format!(
                    "<a id=\"{}\"></a>",
                    view.targets().target_anchor(&name)
                ));
                self.objects(contents, view, out);
            }
            Object::StatisticsCookie(c) => text(c, &view.options, out),
            // The filter expands every macro before this sees the tree.
            Object::Macro { .. } => {}
            Object::ExportSnippet { backend, value } => {
                if is_raw_backend(backend) {
                    out.push_str(value);
                }
            }
        }
    }

    fn link(
        &mut self,
        path: &str,
        description: &[Object],
        target: Option<&str>,
        view: &mut View,
        out: &mut String,
    ) {
        if let Some(anchor) = target {
            out.push('[');
            if description.is_empty() {
                let label = self
                    .outline
                    .label(anchor)
                    .unwrap_or_else(|| path.trim_start_matches(['#', '*']))
                    .to_string();
                text(&label, &view.options, out);
            } else {
                self.objects(description, view, out);
            }
            out.push_str(&format!("](#{anchor})"));
            return;
        }
        if let Some(image) = common::inline_image(path, description) {
            let mut alt = String::new();
            text(file_name(image), &view.options, &mut alt);
            out.push_str(&format!(
                "![{alt}]({})",
                self.href(image).replace(' ', "%20")
            ));
            return;
        }
        let href = if let Some(id) = path.strip_prefix("id:") {
            // The headline's own anchor is the HTML export's; another
            // file's Markdown has no `ID-` anchor to land on, so the link
            // goes to the file.
            match view.host().id_file(id) {
                IdFile::In(file) => Some(self.md_of(&file)),
                IdFile::NoIndex | IdFile::Unknown => None,
            }
        } else if common::is_file_path(path) {
            let file = path.strip_prefix("file:").unwrap_or(path);
            let (file, _search) = file.split_once("::").unwrap_or((file, ""));
            Some(self.href(file))
        } else {
            Some(path.to_string())
        };
        let Some(href) = href else {
            // Nowhere to point: the description alone.
            if description.is_empty() {
                text(path, &view.options, out);
            } else {
                self.objects(description, view, out);
            }
            return;
        };
        if description.is_empty() && (href.contains("://") || href.starts_with("mailto:")) {
            out.push_str(&format!("<{href}>"));
            return;
        }
        out.push('[');
        if description.is_empty() {
            text(path, &view.options, out);
        } else {
            self.objects(description, view, out);
        }
        out.push_str(&format!("]({})", href.replace(' ', "%20")));
    }

    /// A file path as a link target: an org file becomes its Markdown
    /// export.
    fn href(&self, file: &str) -> String {
        file.strip_suffix(".org")
            .map_or_else(|| file.to_string(), |stem| format!("{stem}.md"))
    }

    /// Another org file's export, relative to this one when it is beside or
    /// under it.
    fn md_of(&self, file: &str) -> String {
        let prefix = format!("{}/", self.dir);
        self.href(file.strip_prefix(&prefix).unwrap_or(file))
    }

    /// The footnotes, after the last headline: each referenced one's
    /// definition, in the order first referenced, its continuation lines
    /// indented four spaces as GFM reads them.
    pub(crate) fn finish(&mut self, view: &mut View, out: &mut String) {
        let mut notes = String::new();
        while let Some((n, definition)) = self.footnotes.next() {
            let body = match definition {
                Some(Definition::Elements(elements)) => {
                    let elements = filter::elements(&elements, view);
                    self.blocks(&elements, view)
                }
                Some(Definition::Inline(objects)) => {
                    let mut p = String::new();
                    self.objects(&objects, view, &mut p);
                    p.trim().to_string()
                }
                None => String::new(),
            };
            if !notes.is_empty() {
                notes.push('\n');
            }
            notes.push_str(format!("[^{n}]: {}", hang(&body, 4)).trim_end());
        }
        push_block(out, &notes);
        while out.ends_with("\n\n") {
            out.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::rc::Rc;

    use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options as Gfm, Parser, Tag};

    use super::super::html::Stylesheet;
    use super::super::pipeline::{Backend, Configured, Request, Run, Step};
    use super::super::{Host, IdFile};
    use crate::lattice::plugin_host::types::Effect;
    use crate::time_format::When;

    /// A host with no files and one roam node, `n1`, in `/notes/sub/n1.org`.
    struct Notes;

    impl Host for Notes {
        fn read_file(&self, _: &str) -> Result<String, String> {
            Err("no files".to_string())
        }
        fn id_file(&self, id: &str) -> IdFile {
            match id {
                "n1" => IdFile::In("/notes/sub/n1.org".to_string()),
                _ => IdFile::Unknown,
            }
        }
        fn output_buffer(&self, _: Backend) -> Option<(u32, String)> {
            None
        }
    }

    /// `text` exported to Markdown from `/notes/plan.org`.
    fn export_with(text: &str, body_only: bool) -> String {
        let request = Request {
            buffer: 1,
            source: "/notes/plan.org".to_string(),
            target: "/notes/plan.md".to_string(),
            backend: Backend::Markdown,
            subtree: false,
            body_only,
            line: 0,
            sink: crate::export::sink::Sink::File,
        };
        let mut run = Run::new(
            1,
            request,
            Some(text.to_string()),
            vec!["TODO NEXT | DONE".to_string()],
            When { local_secs: 0 },
            Rc::new(Notes),
            Configured {
                stylesheet: Stylesheet::None,
                ..Configured::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        loop {
            if let Step::Done(effects) = run.step() {
                return effects
                    .iter()
                    .find_map(|e| match e {
                        Effect::WriteToFile(w) => Some(w.text.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("{effects:?}"));
            }
        }
    }

    fn export(text: &str) -> String {
        export_with(text, false)
    }

    fn body(text: &str) -> String {
        export_with(text, true)
    }

    /// `md` as a GFM reader sees it.
    fn events(md: &str) -> Vec<Event<'_>> {
        let gfm = Gfm::ENABLE_TABLES
            | Gfm::ENABLE_FOOTNOTES
            | Gfm::ENABLE_STRIKETHROUGH
            | Gfm::ENABLE_TASKLISTS;
        Parser::new_ext(md, gfm).collect()
    }

    /// The `id`s the raw HTML in `md` defines, and the `#fragment`s its
    /// links go to.
    fn anchors(md: &str) -> (HashSet<String>, Vec<String>) {
        let (mut ids, mut links) = (HashSet::new(), Vec::new());
        for e in events(md) {
            match e {
                Event::InlineHtml(h) | Event::Html(h) => {
                    let mut rest = &*h;
                    while let Some(at) = rest.find("id=\"") {
                        rest = &rest[at + 4..];
                        let end = rest.find('"').unwrap_or(rest.len());
                        assert!(ids.insert(rest[..end].to_string()), "id twice\n{md}");
                    }
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    if let Some(f) = dest_url.strip_prefix('#') {
                        links.push(f.to_string());
                    }
                }
                _ => {}
            }
        }
        (ids, links)
    }

    /// Compare the export of `testdata/NAME.org` with `testdata/NAME.md`;
    /// `UPDATE_GOLDEN=1` rewrites the `.md` instead. Each golden's internal
    /// links land on an anchor it writes.
    fn golden(name: &str) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/export/parse/testdata");
        let org = std::fs::read_to_string(dir.join(format!("{name}.org"))).expect("corpus file");
        let got = export(&org);
        let (ids, links) = anchors(&got);
        for link in &links {
            assert!(ids.contains(link), "{name}: #{link} goes nowhere\n{got}");
        }
        let path = dir.join(format!("{name}.md"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &got).expect("write golden");
            return;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            got == want,
            "{name}.md differs (UPDATE_GOLDEN=1 rewrites it)\n--- got\n{got}"
        );
    }

    #[test]
    fn the_corpus_exports_as_its_goldens() {
        for name in ["headlines", "lists", "blocks", "misc", "objects"] {
            golden(name);
        }
    }

    /// Headings are ATX to `H:`; below it, a headline is a list item, and
    /// the contents link to every heading through an anchor.
    #[test]
    fn headlines_are_headings_then_items_and_the_contents_link_them() {
        let md = export("#+OPTIONS: H:2 num:nil\n* One\n** Two\n*** Three\nx\n");
        let evs = events(&md);
        let levels: Vec<HeadingLevel> = evs
            .iter()
            .filter_map(|e| match e {
                Event::Start(Tag::Heading { level, .. }) => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, [HeadingLevel::H1, HeadingLevel::H2], "{md}");
        assert!(md.contains("\n-   Three\n\n    x\n"), "{md}");
        assert!(md.starts_with("- [One](#"), "{md}");
        let (ids, links) = anchors(&md);
        assert_eq!(links.len(), 2, "{md}");
        assert!(links.iter().all(|l| ids.contains(l)), "{md}");
    }

    /// With no contents, a heading writes an anchor only when a link goes
    /// to it — wherever the link is, before the heading or after.
    #[test]
    fn a_heading_is_anchored_only_when_something_links_to_it() {
        let md = body("#+OPTIONS: num:nil\nSee [[*Later]].\n* Early\n* Later\n");
        assert!(md.contains("# Early\n"), "{md}");
        assert!(md.contains("# <a id="), "{md}");
        let (ids, links) = anchors(&md);
        assert_eq!(ids.len(), 1, "{md}");
        assert!(ids.contains(&links[0]), "{md}");
    }

    /// Below `H:`, a headline nests under the item-headline above it, by
    /// four spaces, however many levels it skips.
    #[test]
    fn deep_headlines_nest_by_the_items_above_them() {
        let md = body("#+OPTIONS: H:1\n* Top\n**** Skipped\n***** Under\n** Back\n");
        assert!(
            md.ends_with("# 1. Top\n\n1.  Skipped\n\n    1.  Under\n\n1.  Back\n"),
            "{md}"
        );
        let code = events(&md)
            .into_iter()
            .any(|e| matches!(e, Event::Start(Tag::CodeBlock(_))));
        assert!(!code, "{md}");
    }

    #[test]
    fn numbers_todo_priority_and_tags_head_the_heading() {
        let md = body("#+OPTIONS: pri:t\n* TODO [#A] Ship it :work:\n");
        assert!(
            md.contains("# 1. TODO \\[#A\\] Ship it   `:work:`\n"),
            "{md}"
        );
    }

    /// A table is a GFM table: a header from the rows before the rule (an
    /// empty one when there is none), alignment from the cookies, and `|`
    /// in a cell, in text or in code, kept in the cell.
    #[test]
    fn tables_are_pipe_tables_that_keep_pipes_in_cells() {
        let md = body(
            "| <l> | <r> |\n| Name | Count |\n|------+-------|\n| a \\vert{} b | 1 |\n| =x= *b* | 2 |\n",
        );
        let evs = events(&md);
        let cells = evs
            .iter()
            .filter(|e| matches!(e, Event::Start(Tag::TableCell)))
            .count();
        assert_eq!(cells, 6, "two cells a row, three rows\n{md}");
        assert!(md.contains("| :--- | ---: |"), "{md}");
        assert!(md.contains("| a \\| b | 1 |"), "{md}");
        assert!(md.contains("| `x` **b** | 2 |"), "{md}");
        let md = body("| a | b |\n");
        assert!(md.starts_with("|  |  |\n| --- | --- |\n| a | b |"), "{md}");
    }

    #[test]
    fn source_blocks_are_fences_longer_than_their_backticks() {
        let md = body("#+begin_src rust\nlet s = \"```\";\n#+end_src\n");
        assert!(md.starts_with("````rust\n"), "{md}");
        let fenced = events(&md).into_iter().any(|e| {
            matches!(e, Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(l))) if &*l == "rust")
        });
        assert!(fenced, "{md}");
        assert!(body("~a`b~").contains("``a`b``"));
    }

    /// Footnotes are numbered by first reference and defined at the end,
    /// each reference landing on its definition.
    #[test]
    fn footnotes_are_gfm_footnotes_in_reference_order() {
        let md = body("b[fn:b] a[fn:a] again[fn:b]\n\n[fn:a] Eh.\n\n[fn:b] Bee.\n");
        assert!(md.starts_with("b[^1] a[^2] again[^1]\n"), "{md}");
        assert!(md.ends_with("[^1]: Bee.\n[^2]: Eh.\n"), "{md}");
        let evs = events(&md);
        let defs = evs
            .iter()
            .filter(|e| matches!(e, Event::Start(Tag::FootnoteDefinition(_))))
            .count();
        let refs = evs
            .iter()
            .filter(|e| matches!(e, Event::FootnoteReference(_)))
            .count();
        assert_eq!((defs, refs), (2, 3), "{md}");
    }

    #[test]
    fn checkboxes_are_task_items() {
        let md = body("- [X] done\n- [ ] open\n- [-] half\n");
        let marks: Vec<bool> = events(&md)
            .into_iter()
            .filter_map(|e| match e {
                Event::TaskListMarker(on) => Some(on),
                _ => None,
            })
            .collect();
        assert_eq!(marks, [true, false], "{md}");
        assert!(md.contains("- \\[-\\] half"), "{md}");
    }

    /// Text that Markdown would read as markup stays text.
    #[test]
    fn markdown_syntax_in_text_is_escaped() {
        let md = body("a *b* _c_ 2*3*4 <tag> [x]\n# not\n");
        assert!(
            md.starts_with("a **b** <u>c</u> 2\\*3\\*4 &lt;tag> \\[x\\]\n"),
            "{md}"
        );
        let paragraph_only = events(&md)
            .iter()
            .all(|e| !matches!(e, Event::Start(Tag::Heading { .. })));
        assert!(paragraph_only, "{md}");
        // Lines org reads as a paragraph that would start a quote or a
        // setext heading.
        let md = body("x\n> not a quote\ny\n==\n");
        assert!(md.starts_with("x\n\\> not a quote\ny\n\\==\n"), "{md}");
        let quote_or_heading = events(&md).iter().any(|e| {
            matches!(
                e,
                Event::Start(Tag::BlockQuote(_)) | Event::Start(Tag::Heading { .. })
            )
        });
        assert!(!quote_or_heading, "{md}");
    }

    #[test]
    fn links_point_at_markdown_and_urls_autolink() {
        let md = body("[[https://example.com]] [[file:other.org][other]] [[id:n1][node]]\n");
        assert!(
            md.starts_with("<https://example.com> [other](other.md) [node](sub/n1.md)\n"),
            "{md}"
        );
    }

    #[test]
    fn raw_markdown_and_html_pass_through() {
        let md = body(
            "#+begin_export markdown\n> raw *md*\n#+end_export\n#+begin_export latex\n\\x\n#+end_export\n@@md:**b**@@\n",
        );
        assert!(md.starts_with("> raw *md*\n\n**b**\n"), "{md}");
        assert!(!md.contains("\\x"), "{md}");
    }

    #[test]
    fn math_uses_dollar_delimiters() {
        let md = body("\\(a\\) and \\[b\\] and $c$\n");
        assert!(md.starts_with("$a$ and $$b$$ and $c$\n"), "{md}");
    }

    #[test]
    fn quotes_and_verse_keep_their_shape() {
        let md =
            body("#+begin_quote\none\n\ntwo\n#+end_quote\n#+begin_verse\n a\n  b\n#+end_verse\n");
        assert!(md.starts_with("> one\n>\n> two\n\n"), "{md}");
        // The common indentation is the block's, not the verse's.
        assert!(md.contains("\na\\\n\u{a0}b"), "{md}");
    }
}

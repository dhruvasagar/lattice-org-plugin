//! The HTML transcoder: ox-html's document shape and class names (`#content`,
//! `.title`, `#table-of-contents`, `.outline-2`, `.outline-text-2`, `.src
//! .src-python`, `.org-ul`, `#footnotes`), so a stylesheet written for emacs's
//! output styles this one (lattice's `org-export.md` §1). The default
//! stylesheet is this file's own, not `org-html-style-default`, which is GPL'd.
//!
//! The output is well-formed XML as well as HTML: void elements close
//! themselves and the only entities are numeric, so a strict parser reads it
//! back, which is how the tests check it.
//!
//! It is written as the render walks, a headline at a time: [`Html::start`]
//! writes the head, the title, the table of contents and the preamble;
//! [`Html::headline`] one visited headline's own parts; [`Html::finish`] the
//! closing tags, the footnotes and the postamble. What spans the document
//! (open containers, footnote numbers, the figure and table counters, whether
//! any math needs MathJax) lives on [`Html`] between steps.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::filter::{self, Selection, View};
use super::options::{Depth, Options, Tags, Tex};
use super::parse::objects::{Emphasis, Object};
use super::parse::{
    Align, Block, BlockKind, Checkbox, Doc, Element, ElementKind, Headline, ListKind, Row, Table,
};
use super::IdFile;

/// The stylesheet `org.export.html-stylesheet = "default"` inlines.
pub(crate) const DEFAULT_STYLESHEET: &str = include_str!("html.css");

/// MathJax 3, configured for the delimiters this writes.
const MATHJAX: &str = "<script>\nwindow.MathJax = {\n  tex: {\n    inlineMath: [['\\\\(', '\\\\)']],\n    \
                       displayMath: [['\\\\[', '\\\\]']],\n    tags: 'ams'\n  }\n};\n</script>\n\
                       <script id=\"MathJax-script\" async=\"async\" \
                       src=\"https://cdn.jsdelivr.net/npm/mathjax@3/es5/tex-mml-chtml.js\"></script>\n";

/// What the head links or inlines for style: `org.export.html-stylesheet`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stylesheet {
    /// The bundled stylesheet, inline.
    Default,
    /// None.
    None,
    /// `<link rel="stylesheet">` to this.
    Link(String),
}

impl Stylesheet {
    pub(crate) fn from_option(value: &str) -> Self {
        match value.trim() {
            "" | "default" => Stylesheet::Default,
            "none" => Stylesheet::None,
            other => Stylesheet::Link(other.to_string()),
        }
    }
}

/// What an HTML export takes from outside the tree.
pub(crate) struct Settings {
    /// The TODO keywords that are done, for the `done` class.
    pub(crate) done: Vec<String>,
    /// The exported file's path, which other files' links are relative to.
    pub(crate) source: String,
    /// Leave out everything outside `#content`.
    pub(crate) body_only: bool,
    pub(crate) stylesheet: Stylesheet,
}

/// An open container the next headlines may close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    /// `<div class="outline-N">`.
    Section(usize),
    /// A list of headlines deeper than `H:`, ordered or not.
    List(usize, bool),
    /// One such headline's `<li>`.
    Item(usize),
}

/// A footnote's definition, as it is found.
enum Definition {
    Elements(Vec<Element>),
    Inline(Vec<Object>),
}

/// The document-wide state of one HTML export.
pub(crate) struct Html {
    open: Vec<Open>,
    /// Headline line → its section number, for the numbered ones.
    numbers: HashMap<u32, Vec<usize>>,
    /// Added to a headline's level to give its level in the export: a
    /// subtree export's top headline is level 1.
    offset: i64,
    /// An internal link's text when it has no description: the number of a
    /// numbered headline, else its title.
    labels: HashMap<String, String>,
    done: Vec<String>,
    /// The exported file, and its directory, for links to other files.
    source: String,
    dir: String,
    /// Headline line → its `:ID:`, which an `id:` link from another file
    /// reaches as `#ID-<id>`. Taken before the filter, which may drop the
    /// properties.
    org_ids: HashMap<u32, String>,
    body_only: bool,
    /// Footnote keys in the order first referenced; a key is the label, or a
    /// made-up one for an anonymous inline footnote.
    footnote_order: Vec<String>,
    footnote_refs: HashMap<String, usize>,
    definitions: HashMap<String, Definition>,
    anonymous: usize,
    figures: usize,
    tables: usize,
    listings: usize,
    mathjax: bool,
    /// Where `</head>` is in the output, for the MathJax script, which is
    /// only known to be needed once the body is written.
    head_end: Option<usize>,
}

/// Text, escaped for HTML text and attribute values alike.
pub(crate) fn esc(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

fn escaped(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    esc(s, &mut out);
    out
}

/// Whether `path` names an image a browser shows inline.
fn is_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".bmp", ".avif",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// `#+ATTR_HTML: :key value :key2 value…` as attributes, each ` key="value"`.
fn attributes(element: &Element, skip: &[&str]) -> String {
    let mut out = String::new();
    for (backend, value) in &element.affiliated.attr {
        if backend != "html" {
            continue;
        }
        let mut rest = value.trim();
        while let Some(after) = rest.strip_prefix(':') {
            let key_end = after.find(char::is_whitespace).unwrap_or(after.len());
            let key = &after[..key_end];
            let tail = after[key_end..].trim_start();
            let value_end = tail.find(" :").unwrap_or(tail.len());
            let value = tail[..value_end].trim();
            rest = tail[value_end..].trim_start();
            if key.is_empty() || skip.contains(&key) {
                continue;
            }
            let _ = write!(out, " {}=\"{}\"", escaped(key), escaped(value));
        }
    }
    out
}

impl Html {
    /// Number the headlines, write everything before the first headline, and
    /// keep what the rest of the walk needs.
    pub(crate) fn start(
        doc: &Doc,
        selection: &Selection,
        view: &mut View,
        settings: Settings,
        out: &mut String,
    ) -> Html {
        let Settings {
            done,
            source,
            body_only,
            stylesheet,
        } = settings;
        let dir = std::path::Path::new(&source)
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let offset = doc
            .headlines
            .iter()
            .map(|h| h.level)
            .min()
            .map_or(0, |min| 1 - min as i64);
        let mut html = Html {
            open: Vec::new(),
            numbers: HashMap::new(),
            offset,
            labels: HashMap::new(),
            done,
            dir,
            source,
            org_ids: HashMap::new(),
            body_only,
            footnote_order: Vec::new(),
            footnote_refs: HashMap::new(),
            definitions: HashMap::new(),
            anonymous: 0,
            figures: 0,
            tables: 0,
            listings: 0,
            mathjax: false,
            head_end: None,
        };
        let toc = html.number(doc, selection, view);
        if !body_only {
            html.head(view, &stylesheet, out);
        }
        out.push_str(&toc);
        let preamble = filter::elements(&doc.preamble, view);
        html.elements(&preamble, view, out);
        html
    }

    fn level(&self, h: &Headline) -> usize {
        (h.level as i64 + self.offset).max(1) as usize
    }

    /// Number the exported headlines and give back the table of contents.
    /// One pass over the headlines, not their sections.
    fn number(&mut self, doc: &Doc, selection: &Selection, view: &mut View) -> String {
        let o = &view.options;
        let deepest = o.headline_levels;
        // `num:` reaches below `H:`, as in ox: a numbered headline there is
        // an item in an ordered list.
        let numbered_to = match o.num {
            Depth::Off => 0,
            Depth::On => usize::MAX,
            Depth::To(n) => n,
        };
        let toc_to = match o.toc {
            Depth::Off => 0,
            Depth::On => deepest,
            Depth::To(n) => n.min(deepest),
        };
        let toc_tags = o.tags == Tags::On;
        let mut counters: Vec<usize> = Vec::new();
        // (headline, inherited UNNUMBERED: 0 none, 1 t, 2 notoc)
        let mut stack: Vec<(&Headline, u8)> = doc.headlines.iter().rev().map(|h| (h, 0)).collect();
        let mut entries: Vec<(usize, String, String)> = Vec::new();
        while let Some((h, inherited)) = stack.pop() {
            if !filter::visits(h, selection, &view.options) {
                continue;
            }
            let own = h
                .properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("UNNUMBERED"))
                .map(|(_, v)| v.trim());
            let unnumbered = match own {
                Some("notoc") => 2,
                Some("nil") => 0,
                Some(_) => 1,
                None => inherited,
            };
            stack.extend(h.children.iter().rev().map(|c| (c, unnumbered)));
            if let Some((_, id)) = h
                .properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("ID"))
            {
                self.org_ids.insert(h.line, id.trim().to_string());
            }
            let level = self.level(h);
            let anchor = view.targets().headline_anchor(h.line);
            let number = if unnumbered == 0 && level <= numbered_to {
                counters.truncate(level);
                while counters.len() < level {
                    counters.push(0);
                }
                counters[level - 1] += 1;
                self.numbers.insert(h.line, counters.clone());
                Some(dotted(&counters))
            } else {
                None
            };
            self.labels.insert(
                anchor.clone(),
                number.clone().unwrap_or_else(|| h.raw_title.clone()),
            );
            if level <= toc_to && unnumbered != 2 && !is_footnote_section(h) {
                let mut entry = String::new();
                if let Some(n) = &number {
                    let _ = write!(entry, "{n}. ");
                }
                let title = filter::objects(&h.title, view);
                self.toc_objects(&title, view, &mut entry);
                if toc_tags && !h.tags.is_empty() {
                    self.tags(&h.tags, &mut entry);
                }
                entries.push((level, anchor, entry));
            }
        }
        if entries.is_empty() {
            return String::new();
        }
        let mut toc = String::from(
            "<div id=\"table-of-contents\" role=\"doc-toc\">\n<h2>Table of Contents</h2>\n\
             <div id=\"text-table-of-contents\" role=\"doc-toc\">\n",
        );
        let mut depth = 0;
        for (i, (level, anchor, entry)) in entries.iter().enumerate() {
            if i == 0 || *level > depth {
                while depth < *level {
                    toc.push_str("<ul>\n");
                    depth += 1;
                    if depth < *level {
                        toc.push_str("<li>");
                    }
                }
            } else {
                toc.push_str("</li>\n");
                while depth > *level {
                    toc.push_str("</ul>\n</li>\n");
                    depth -= 1;
                }
            }
            let _ = write!(toc, "<li><a href=\"#{}\">{entry}</a>", escaped(anchor));
        }
        toc.push_str("</li>\n");
        while depth > 1 {
            toc.push_str("</ul>\n</li>\n");
            depth -= 1;
        }
        toc.push_str("</ul>\n</div>\n</div>\n");
        toc
    }

    fn head(&mut self, view: &mut View, stylesheet: &Stylesheet, out: &mut String) {
        let title = view.options.title.clone();
        let title = filter::objects(&title, view);
        let o = &view.options;
        let _ = write!(
            out,
            "<!DOCTYPE html>\n<html lang=\"{}\">\n<head>\n<meta charset=\"utf-8\" />\n\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n\
             <title>{}</title>\n<meta name=\"generator\" content=\"lattice\" />\n",
            escaped(&o.language),
            escaped(&filter::plain(&title)),
        );
        if o.with_author && !o.author.is_empty() {
            let _ = writeln!(
                out,
                "<meta name=\"author\" content=\"{}\" />",
                escaped(&filter::plain(&o.author))
            );
        }
        match stylesheet {
            Stylesheet::Default => {
                out.push_str("<style>\n");
                out.push_str(DEFAULT_STYLESHEET);
                out.push_str("</style>\n");
            }
            Stylesheet::None => {}
            Stylesheet::Link(href) => {
                let _ = writeln!(
                    out,
                    "<link rel=\"stylesheet\" type=\"text/css\" href=\"{}\" />",
                    escaped(href)
                );
            }
        }
        for line in o.html_head.iter().chain(&o.html_head_extra) {
            out.push_str(line);
            out.push('\n');
        }
        self.head_end = Some(out.len());
        out.push_str("</head>\n<body>\n<div id=\"content\" class=\"content\">\n");
        let with_title = o.with_title;
        let subtitle = o.subtitle.clone();
        if with_title {
            out.push_str("<h1 class=\"title\">");
            self.objects(&title, view, out);
            out.push_str("</h1>\n");
            if !subtitle.is_empty() {
                let subtitle = filter::objects(&subtitle, view);
                out.push_str("<p class=\"subtitle\" role=\"doc-subtitle\">");
                self.objects(&subtitle, view, out);
                out.push_str("</p>\n");
            }
        }
    }

    /// Close what is open down to `level`: everything at it or deeper.
    fn close_to(&mut self, level: usize, out: &mut String) {
        while let Some(&top) = self.open.last() {
            let at = match top {
                Open::Section(l) | Open::List(l, _) | Open::Item(l) => l,
            };
            if at < level {
                break;
            }
            self.close_one(out);
        }
    }

    fn close_one(&mut self, out: &mut String) {
        match self.open.pop() {
            Some(Open::Section(_)) => out.push_str("</div>\n"),
            Some(Open::List(_, true)) => out.push_str("</ol>\n"),
            Some(Open::List(_, false)) => out.push_str("</ul>\n"),
            Some(Open::Item(_)) => out.push_str("</li>\n"),
            None => {}
        }
    }

    /// One visited headline's own parts, already filtered (`filter::own`).
    pub(crate) fn headline(&mut self, h: &Headline, view: &mut View, out: &mut String) {
        if is_footnote_section(h) {
            // The footnotes are collected; the heading that holds them is not
            // exported, as in ox.
            self.collect_definitions(&h.section);
            return;
        }
        let level = self.level(h);
        let anchor = view.targets().headline_anchor(h.line);
        let number = self.numbers.get(&h.line).map(|n| dotted(n));
        if level > view.options.headline_levels {
            self.deep_headline(h, level, &anchor, number.is_some(), view, out);
            return;
        }
        self.close_to(level, out);
        let hn = (level + 1).min(6);
        let _ = writeln!(
            out,
            "<div id=\"outline-container-{a}\" class=\"outline-{n}\">",
            a = escaped(&anchor),
            n = level + 1
        );
        self.open.push(Open::Section(level));
        let _ = write!(out, "<h{hn} id=\"{}\">", escaped(&anchor));
        self.id_anchor(h, &anchor, out);
        if let Some(n) = &number {
            let _ = write!(
                out,
                "<span class=\"section-number-{}\">{n}.</span> ",
                level + 1
            );
        }
        self.headline_text(h, view, out);
        let _ = writeln!(out, "</h{hn}>");
        let has_body = !h.planning.is_empty() || !h.properties.is_empty() || !h.section.is_empty();
        if has_body {
            let _ = writeln!(
                out,
                "<div class=\"outline-text-{}\" id=\"text-{}\">",
                level + 1,
                escaped(&number.unwrap_or(anchor))
            );
            self.meta(h, out);
            self.elements(&h.section, view, out);
            out.push_str("</div>\n");
        }
    }

    /// A headline deeper than `H:`: an item in a list of its siblings.
    fn deep_headline(
        &mut self,
        h: &Headline,
        level: usize,
        anchor: &str,
        numbered: bool,
        view: &mut View,
        out: &mut String,
    ) {
        self.close_to(level + 1, out);
        if self.open.last() == Some(&Open::Item(level)) {
            self.close_one(out);
        }
        if !matches!(self.open.last(), Some(Open::List(l, _)) if *l == level) {
            out.push_str(if numbered {
                "<ol class=\"org-ol\">\n"
            } else {
                "<ul class=\"org-ul\">\n"
            });
            self.open.push(Open::List(level, numbered));
        }
        let _ = write!(out, "<li><a id=\"{}\"></a>", escaped(anchor));
        self.id_anchor(h, anchor, out);
        self.open.push(Open::Item(level));
        self.headline_text(h, view, out);
        if !h.section.is_empty() || !h.planning.is_empty() || !h.properties.is_empty() {
            out.push_str("<br />\n");
            self.meta(h, out);
            self.elements(&h.section, view, out);
        } else {
            out.push('\n');
        }
    }

    /// `<a id="ID-…">` for a headline with an `:ID:`, so `id:` links from
    /// other files land on it.
    fn id_anchor(&self, h: &Headline, anchor: &str, out: &mut String) {
        if let Some(id) = self.org_ids.get(&h.line) {
            let id = format!("ID-{id}");
            if id != anchor {
                let _ = write!(out, "<a id=\"{}\"></a>", escaped(&id));
            }
        }
    }

    fn headline_text(&mut self, h: &Headline, view: &mut View, out: &mut String) {
        let o = &view.options;
        if o.todo {
            if let Some(kw) = &h.todo {
                let class = if self.done.contains(kw) {
                    "done"
                } else {
                    "todo"
                };
                let _ = write!(
                    out,
                    "<span class=\"{class} {k}\">{k}</span> ",
                    k = escaped(kw)
                );
            }
        }
        if o.priority {
            if let Some(p) = h.priority {
                let _ = write!(out, "<span class=\"priority\">[{p}]</span> ");
            }
        }
        let tags = o.tags != Tags::Off;
        self.objects(&h.title, view, out);
        if tags && !h.tags.is_empty() {
            self.tags(&h.tags, out);
        }
    }

    fn tags(&self, tags: &[String], out: &mut String) {
        out.push_str("&#xa0;&#xa0;&#xa0;<span class=\"tag\">");
        for (i, t) in tags.iter().enumerate() {
            if i > 0 {
                out.push_str("&#xa0;");
            }
            let _ = write!(out, "<span class=\"{t}\">{t}</span>", t = escaped(t));
        }
        out.push_str("</span>");
    }

    /// Planning and properties, when the options kept them.
    fn meta(&mut self, h: &Headline, out: &mut String) {
        let p = &h.planning;
        if !p.is_empty() {
            out.push_str("<p>");
            let mut first = true;
            for (kw, stamp) in [
                ("SCHEDULED:", &p.scheduled),
                ("DEADLINE:", &p.deadline),
                ("CLOSED:", &p.closed),
            ] {
                if let Some(stamp) = stamp {
                    if !first {
                        out.push(' ');
                    }
                    first = false;
                    let _ = write!(
                        out,
                        "<span class=\"timestamp-wrapper\"><span class=\"timestamp-kwd\">{kw}</span> \
                         <span class=\"timestamp\">{}</span></span>",
                        escaped(stamp)
                    );
                }
            }
            out.push_str("</p>\n");
        }
        if !h.properties.is_empty() {
            out.push_str("<pre class=\"example\">\n");
            for (k, v) in &h.properties {
                let _ = writeln!(out, "{}: {}", escaped(k), escaped(v));
            }
            out.push_str("</pre>\n");
        }
    }

    fn collect_definitions(&mut self, elements: &[Element]) {
        for e in elements {
            if let ElementKind::FootnoteDefinition { label, contents } = &e.kind {
                self.definitions
                    .entry(label.clone())
                    .or_insert_with(|| Definition::Elements(contents.clone()));
            }
        }
    }

    /// Elements, as HTML.
    pub(crate) fn elements(&mut self, list: &[Element], view: &mut View, out: &mut String) {
        for e in list {
            self.element(e, false, view, out);
        }
    }

    /// One element. `bare` is a list item's first paragraph, which ox writes
    /// without `<p>`.
    fn element(&mut self, e: &Element, bare: bool, view: &mut View, out: &mut String) {
        let id = e
            .affiliated
            .name
            .as_ref()
            .map(|n| format!(" id=\"{}\"", escaped(&view.targets().name_anchor(n))))
            .unwrap_or_default();
        match &e.kind {
            ElementKind::Paragraph(objects) => {
                if let Some(img) = standalone_image(objects) {
                    self.figure(e, img, &id, view, out);
                } else if bare {
                    self.objects(objects, view, out);
                    out.push('\n');
                } else {
                    out.push_str("<p>\n");
                    self.objects(objects, view, out);
                    out.push_str("\n</p>\n");
                }
            }
            ElementKind::PlainList(list) => {
                let (open, close) = match list.kind {
                    ListKind::Unordered => ("<ul class=\"org-ul\">", "</ul>"),
                    ListKind::Ordered => ("<ol class=\"org-ol\">", "</ol>"),
                    ListKind::Descriptive => ("<dl class=\"org-dl\">", "</dl>"),
                };
                out.push_str(open);
                out.push('\n');
                for item in &list.items {
                    let checkbox = item.checkbox.map(|c| match c {
                        Checkbox::On => ("on", "[X]"),
                        Checkbox::Off => ("off", "[&#xa0;]"),
                        Checkbox::Partial => ("trans", "[-]"),
                    });
                    if list.kind == ListKind::Descriptive {
                        out.push_str("<dt>");
                        if let Some(tag) = &item.tag {
                            self.objects(tag, view, out);
                        }
                        out.push_str("</dt>\n<dd>");
                    } else {
                        out.push_str("<li");
                        if let Some((class, _)) = checkbox {
                            let _ = write!(out, " class=\"{class}\"");
                        }
                        if let Some(n) = item.counter {
                            let _ = write!(out, " value=\"{n}\"");
                        }
                        out.push('>');
                    }
                    if let Some((_, mark)) = checkbox {
                        let _ = write!(out, "<code>{mark}</code> ");
                    }
                    for (i, c) in item.contents.iter().enumerate() {
                        self.element(c, i == 0, view, out);
                    }
                    out.push_str(if list.kind == ListKind::Descriptive {
                        "</dd>\n"
                    } else {
                        "</li>\n"
                    });
                }
                out.push_str(close);
                out.push('\n');
            }
            ElementKind::Table(t) => self.table(e, t, &id, view, out),
            ElementKind::TableEl(lines) => {
                out.push_str("<pre class=\"example\">\n");
                for l in lines {
                    esc(l, out);
                    out.push('\n');
                }
                out.push_str("</pre>\n");
            }
            ElementKind::Block(b) => self.block(e, b, &id, view, out),
            ElementKind::Drawer { contents, .. } => self.elements(contents, view, out),
            ElementKind::Keyword { key, value } => {
                if key == "HTML" {
                    out.push_str(value);
                    out.push('\n');
                } else if key == "TOC" {
                    view.options.ignore("#+TOC".to_string());
                }
            }
            ElementKind::FootnoteDefinition { .. } => {
                self.collect_definitions(std::slice::from_ref(e));
            }
            ElementKind::FixedWidth(lines) => {
                out.push_str("<pre class=\"example\">\n");
                for l in lines {
                    esc(l, out);
                    out.push('\n');
                }
                out.push_str("</pre>\n");
            }
            ElementKind::Comment(_) => {}
            ElementKind::HorizontalRule => out.push_str("<hr />\n"),
            ElementKind::LatexEnvironment(text) => match view.options.tex {
                Tex::On => {
                    self.mathjax = true;
                    esc(text, out);
                    out.push('\n');
                }
                Tex::Verbatim => {
                    out.push_str("<pre class=\"example\">\n");
                    esc(text, out);
                    out.push_str("\n</pre>\n");
                }
                Tex::Off => {}
            },
            ElementKind::Clock(value) => {
                let _ = writeln!(
                    out,
                    "<p>\n<span class=\"timestamp-wrapper\"><span class=\"timestamp-kwd\">CLOCK:</span> \
                     <span class=\"timestamp\">{}</span></span>\n</p>",
                    escaped(value)
                );
            }
        }
    }

    fn figure(
        &mut self,
        e: &Element,
        img: (&str, ()),
        id: &str,
        view: &mut View,
        out: &mut String,
    ) {
        let (path, ()) = img;
        let _ = writeln!(out, "<div{id} class=\"figure\">");
        out.push_str("<p>");
        self.image(path, &attributes(e, &[]), out);
        out.push_str("\n</p>\n");
        if !e.affiliated.caption.is_empty() {
            self.figures += 1;
            let _ = write!(
                out,
                "<p><span class=\"figure-number\">Figure {}: </span>",
                self.figures
            );
            self.caption(e, view, out);
            out.push_str("</p>\n");
        }
        out.push_str("</div>\n");
    }

    /// An element's caption: its `#+CAPTION:` lines, joined as one.
    fn caption(&mut self, e: &Element, view: &mut View, out: &mut String) {
        for (i, c) in e.affiliated.caption.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            self.objects(c, view, out);
        }
    }

    fn image(&self, path: &str, attrs: &str, out: &mut String) {
        let src = self.href(path);
        let alt = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        let alt = if attrs.contains(" alt=") {
            String::new()
        } else {
            format!(" alt=\"{}\"", escaped(&alt))
        };
        let _ = write!(out, "<img src=\"{}\"{alt}{attrs} />", escaped(&src));
    }

    fn table(&mut self, e: &Element, t: &Table, id: &str, view: &mut View, out: &mut String) {
        let columns = t
            .rows
            .iter()
            .map(|r| match r {
                Row::Cells(c) => c.len(),
                Row::Rule => 0,
            })
            .max()
            .unwrap_or(0);
        let align: Vec<&str> = (0..columns)
            .map(|i| match t.align.get(i).copied().flatten() {
                Some(Align::Left) => "org-left",
                Some(Align::Center) => "org-center",
                Some(Align::Right) => "org-right",
                None if numeric_column(t, i) => "org-right",
                None => "org-left",
            })
            .collect();
        let _ = writeln!(
            out,
            "<table{id} border=\"2\" cellspacing=\"0\" cellpadding=\"6\" rules=\"groups\" frame=\"hsides\"{}>",
            attributes(e, &[])
        );
        if !e.affiliated.caption.is_empty() {
            self.tables += 1;
            let _ = write!(
                out,
                "<caption class=\"t-above\"><span class=\"table-number\">Table {}:</span> ",
                self.tables
            );
            self.caption(e, view, out);
            out.push_str("</caption>\n");
        }
        out.push_str("\n<colgroup>\n");
        for class in &align {
            let _ = writeln!(out, "<col  class=\"{class}\" />");
        }
        out.push_str("</colgroup>\n");
        // Groups of rows between rules; the first is a header when a rule
        // follows it and rows follow the rule.
        let mut groups: Vec<Vec<&Vec<Vec<Object>>>> = vec![Vec::new()];
        for r in &t.rows {
            match r {
                Row::Rule => groups.push(Vec::new()),
                Row::Cells(c) => {
                    if let Some(g) = groups.last_mut() {
                        g.push(c)
                    }
                }
            }
        }
        groups.retain(|g| !g.is_empty());
        let header = groups.len() > 1
            && t.rows
                .iter()
                .position(|r| matches!(r, Row::Rule))
                .is_some_and(|i| i > 0);
        for (gi, group) in groups.iter().enumerate() {
            let head = header && gi == 0;
            out.push_str(if head { "<thead>\n" } else { "<tbody>\n" });
            for cells in group {
                out.push_str("<tr>\n");
                for (i, class) in align.iter().enumerate() {
                    if head {
                        let _ = write!(out, "<th scope=\"col\" class=\"{class}\">");
                    } else {
                        let _ = write!(out, "<td class=\"{class}\">");
                    }
                    if let Some(cell) = cells.get(i) {
                        self.objects(cell, view, out);
                    }
                    out.push_str(if head { "</th>\n" } else { "</td>\n" });
                }
                out.push_str("</tr>\n");
            }
            out.push_str(if head { "</thead>\n" } else { "</tbody>\n" });
        }
        out.push_str("</table>\n");
    }

    fn block(&mut self, e: &Element, b: &Block, id: &str, view: &mut View, out: &mut String) {
        match &b.kind {
            BlockKind::Src { language } => {
                out.push_str("<div class=\"org-src-container\">\n");
                if !e.affiliated.caption.is_empty() {
                    self.listings += 1;
                    let _ = write!(
                        out,
                        "<label class=\"org-src-name\"><span class=\"listing-number\">Listing {}: </span>",
                        self.listings
                    );
                    self.caption(e, view, out);
                    out.push_str("</label>");
                }
                let lang = if language.is_empty() {
                    "text"
                } else {
                    language
                };
                let _ = write!(out, "<pre class=\"src src-{}\"{id}>", escaped(lang));
                esc(&b.body, out);
                out.push_str("</pre>\n</div>\n");
            }
            BlockKind::Example => {
                let _ = writeln!(out, "<pre class=\"example\"{id}>");
                esc(&b.body, out);
                if !b.body.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("</pre>\n");
            }
            BlockKind::Export { backend } => {
                if backend.eq_ignore_ascii_case("html") {
                    out.push_str(&b.body);
                    if !b.body.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
            BlockKind::Verse => {
                let objects = super::parse::objects::parse(b.body.trim_end_matches('\n'));
                let objects = filter::objects(&objects, view);
                out.push_str("<p class=\"verse\">\n");
                let mut text = String::new();
                self.objects(&objects, view, &mut text);
                for (i, line) in text.split('\n').enumerate() {
                    if i > 0 {
                        out.push_str("<br />\n");
                    }
                    let indent = line.len() - line.trim_start_matches(' ').len();
                    out.push_str(&"&#xa0;".repeat(indent));
                    out.push_str(&line[indent..]);
                }
                out.push_str("\n</p>\n");
            }
            BlockKind::Comment => {}
            BlockKind::Quote => {
                let _ = writeln!(out, "<blockquote{id}>");
                self.elements(&b.contents, view, out);
                out.push_str("</blockquote>\n");
            }
            BlockKind::Center => {
                let _ = writeln!(out, "<div class=\"org-center\"{id}>");
                self.elements(&b.contents, view, out);
                out.push_str("</div>\n");
            }
            BlockKind::Special(name) => {
                let _ = writeln!(out, "<div class=\"{}\"{id}>", escaped(name));
                self.elements(&b.contents, view, out);
                out.push_str("</div>\n");
            }
        }
    }

    /// Objects in a table-of-contents entry: no links (they would nest in
    /// the entry's own), footnote references or targets.
    fn toc_objects(&mut self, list: &[Object], view: &mut View, out: &mut String) {
        for o in list {
            match o {
                Object::Link {
                    path, description, ..
                } => {
                    if description.is_empty() {
                        esc(path, out);
                    } else {
                        self.toc_objects(description, view, out);
                    }
                }
                Object::FootnoteReference { .. } | Object::Target(_) => {}
                Object::RadioTarget(c) => self.toc_objects(c, view, out),
                other => self.object(other, view, out),
            }
        }
    }

    /// Objects, as HTML.
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
                    Emphasis::Bold => ("<b>", "</b>"),
                    Emphasis::Italic => ("<i>", "</i>"),
                    Emphasis::Underline => ("<span class=\"underline\">", "</span>"),
                    Emphasis::Strike => ("<del>", "</del>"),
                };
                out.push_str(open);
                self.objects(contents, view, out);
                out.push_str(close);
            }
            Object::Verbatim(t) | Object::Code(t) => {
                out.push_str("<code>");
                esc(t, out);
                out.push_str("</code>");
            }
            Object::Link {
                path,
                description,
                target,
            } => self.link(path, description, target.as_deref(), view, out),
            Object::FootnoteReference { label, definition } => {
                let key = match label {
                    Some(l) => l.clone(),
                    None => {
                        self.anonymous += 1;
                        format!(" anonymous {}", self.anonymous)
                    }
                };
                if !definition.is_empty() {
                    self.definitions
                        .entry(key.clone())
                        .or_insert_with(|| Definition::Inline(definition.clone()));
                }
                let n = match self.footnote_order.iter().position(|k| *k == key) {
                    Some(i) => i + 1,
                    None => {
                        self.footnote_order.push(key.clone());
                        self.footnote_order.len()
                    }
                };
                let count = self.footnote_refs.entry(key).or_insert(0);
                *count += 1;
                let id = if *count == 1 {
                    format!("fnr.{n}")
                } else {
                    format!("fnr.{n}.{count}")
                };
                let _ = write!(
                    out,
                    "<sup><a id=\"{id}\" class=\"footref\" href=\"#fn.{n}\" role=\"doc-backlink\">{n}</a></sup>"
                );
            }
            Object::Entity { utf8, .. } => esc(utf8, out),
            Object::LatexFragment(t) => match view.options.tex {
                Tex::On => {
                    self.mathjax = true;
                    let math = if let Some(inner) =
                        t.strip_prefix("$$").and_then(|r| r.strip_suffix("$$"))
                    {
                        format!("\\[{inner}\\]")
                    } else if let Some(inner) =
                        t.strip_prefix('$').and_then(|r| r.strip_suffix('$'))
                    {
                        format!("\\({inner}\\)")
                    } else {
                        t.clone()
                    };
                    esc(&math, out);
                }
                Tex::Verbatim => esc(t, out),
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
            // The newline after it is the text's own.
            Object::LineBreak => out.push_str("<br />"),
            Object::Timestamp(t) => {
                out.push_str("<span class=\"timestamp-wrapper\"><span class=\"timestamp\">");
                esc(t, out);
                out.push_str("</span></span>");
            }
            Object::Target(t) => {
                let _ = write!(
                    out,
                    "<a id=\"{}\"></a>",
                    escaped(&view.targets().target_anchor(t))
                );
            }
            Object::RadioTarget(contents) => {
                let name = filter::plain(contents);
                let _ = write!(
                    out,
                    "<a id=\"{}\"></a>",
                    escaped(&view.targets().target_anchor(&name))
                );
                self.objects(contents, view, out);
            }
            Object::StatisticsCookie(c) => {
                out.push_str("<code>");
                esc(c, out);
                out.push_str("</code>");
            }
            // The filter expands every macro before this sees the tree.
            Object::Macro { .. } => {}
            Object::ExportSnippet { backend, value } => {
                if backend.eq_ignore_ascii_case("html") {
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
            let _ = write!(out, "<a href=\"#{}\">", escaped(anchor));
            if description.is_empty() {
                let label = self
                    .labels
                    .get(anchor)
                    .cloned()
                    .unwrap_or_else(|| path.trim_start_matches(['#', '*']).to_string());
                esc(&label, out);
            } else {
                self.objects(description, view, out);
            }
            out.push_str("</a>");
            return;
        }
        let file = path.strip_prefix("file:").unwrap_or(path);
        let is_file = path.starts_with("file:")
            || ["/", "./", "../", "~/"].iter().any(|p| path.starts_with(p));
        if description.is_empty()
            && (is_file || path.starts_with("http://") || path.starts_with("https://"))
            && is_image(file)
        {
            self.image(file, "", out);
            return;
        }
        let href = if let Some(id) = path.strip_prefix("id:") {
            match view.host().id_file(id) {
                IdFile::In(file) if file == self.source => format!("#ID-{id}"),
                IdFile::In(file) => format!("{}#ID-{id}", self.html_of(&file)),
                IdFile::NoIndex | IdFile::Unknown => format!("#ID-{id}"),
            }
        } else if is_file {
            let (file, _search) = file.split_once("::").unwrap_or((file, ""));
            self.href(file)
        } else {
            path.to_string()
        };
        let _ = write!(out, "<a href=\"{}\">", escaped(&href));
        if description.is_empty() {
            esc(path, out);
        } else {
            self.objects(description, view, out);
        }
        out.push_str("</a>");
    }

    /// A file path as a link target: an org file becomes its HTML export, an
    /// absolute path a `file://` URL.
    fn href(&self, file: &str) -> String {
        let file = file
            .strip_suffix(".org")
            .map_or_else(|| file.to_string(), |stem| format!("{stem}.html"));
        if file.starts_with('/') {
            format!("file://{file}")
        } else {
            file
        }
    }

    /// Another org file's export, relative to this one when it is beside or
    /// under it.
    fn html_of(&self, file: &str) -> String {
        let prefix = format!("{}/", self.dir);
        let relative = file.strip_prefix(&prefix).unwrap_or(file);
        self.href(relative)
    }

    /// Everything after the last headline.
    pub(crate) fn finish(&mut self, view: &mut View, out: &mut String) {
        self.close_to(0, out);
        self.footnotes(view, out);
        if self.body_only {
            return;
        }
        out.push_str("</div>\n");
        let o = &view.options;
        let author = o.with_author && !o.author.is_empty();
        let date = o.with_date && !o.date.is_empty();
        let email = o.with_email && !o.email.is_empty();
        if author || date || email {
            let (a, d) = (o.author.clone(), o.date.clone());
            let mail = o.email.clone();
            out.push_str("<div id=\"postamble\" class=\"status\">\n");
            if author {
                out.push_str("<p class=\"author\">Author: ");
                let a = filter::objects(&a, view);
                self.objects(&a, view, out);
                out.push_str("</p>\n");
            }
            if date {
                out.push_str("<p class=\"date\">Date: ");
                let d = filter::objects(&d, view);
                self.objects(&d, view, out);
                out.push_str("</p>\n");
            }
            if email {
                let _ = writeln!(
                    out,
                    "<p class=\"email\">Email: <a href=\"mailto:{m}\">{m}</a></p>",
                    m = escaped(&mail)
                );
            }
            out.push_str("</div>\n");
        }
        out.push_str("</body>\n</html>\n");
        if self.mathjax {
            if let Some(at) = self.head_end {
                out.insert_str(at, MATHJAX);
            }
        }
    }

    /// The footnotes section: each referenced footnote's definition, in the
    /// order first referenced. A definition may reference another, which is
    /// numbered and written in turn.
    fn footnotes(&mut self, view: &mut View, out: &mut String) {
        if self.footnote_order.is_empty() {
            return;
        }
        out.push_str(
            "<div id=\"footnotes\">\n<h2 class=\"footnotes\">Footnotes: </h2>\n<div id=\"text-footnotes\">\n\n",
        );
        let mut i = 0;
        while i < self.footnote_order.len() {
            let key = self.footnote_order[i].clone();
            let n = i + 1;
            i += 1;
            let _ = write!(
                out,
                "<div class=\"footdef\"><sup><a id=\"fn.{n}\" class=\"footnum\" href=\"#fnr.{n}\" \
                 role=\"doc-backlink\">{n}</a></sup> <div class=\"footpara\" role=\"doc-footnote\">"
            );
            match self.definitions.remove(&key) {
                Some(Definition::Elements(elements)) => {
                    let elements = filter::elements(&elements, view);
                    for e in &elements {
                        if let ElementKind::Paragraph(objects) = &e.kind {
                            out.push_str("<p class=\"footpara\">");
                            self.objects(objects, view, out);
                            out.push_str("</p>");
                        } else {
                            self.element(e, false, view, out);
                        }
                    }
                }
                Some(Definition::Inline(objects)) => {
                    out.push_str("<p class=\"footpara\">");
                    self.objects(&objects, view, out);
                    out.push_str("</p>");
                }
                None => {}
            }
            out.push_str("</div></div>\n\n");
        }
        out.push_str("\n</div>\n</div>\n");
    }
}

/// `1.2.3`.
fn dotted(numbers: &[usize]) -> String {
    numbers
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// The heading org keeps footnote definitions under, which export leaves out.
fn is_footnote_section(h: &Headline) -> bool {
    h.raw_title == "Footnotes"
}

/// A paragraph that is one image link and nothing else.
fn standalone_image(objects: &[Object]) -> Option<(&str, ())> {
    let mut found = None;
    for o in objects {
        match o {
            Object::Text(t) if t.trim().is_empty() => {}
            Object::Link {
                path,
                description,
                target: None,
            } if description.is_empty() && found.is_none() => {
                let file = path.strip_prefix("file:").unwrap_or(path);
                let local = path.starts_with("file:")
                    || ["/", "./", "../", "~/"].iter().any(|p| path.starts_with(p))
                    || path.starts_with("http://")
                    || path.starts_with("https://");
                if !(local && is_image(file)) {
                    return None;
                }
                found = Some((file, ()));
            }
            _ => return None,
        }
    }
    found
}

/// Whether most of a column's non-empty cells are numbers: such a column is
/// right-aligned, as org's `org-table-number-fraction` has it.
fn numeric_column(t: &Table, column: usize) -> bool {
    let (mut numbers, mut filled) = (0, 0);
    for r in &t.rows {
        let Row::Cells(cells) = r else { continue };
        let Some(cell) = cells.get(column) else {
            continue;
        };
        let text = filter::plain(cell);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        filled += 1;
        let number = text
            .trim_start_matches(['+', '-'])
            .replace([',', '.'], "")
            .trim_end_matches('%')
            .chars()
            .all(|c| c.is_ascii_digit())
            && text.chars().any(|c| c.is_ascii_digit());
        if number {
            numbers += 1;
        }
    }
    filled > 0 && numbers * 2 >= filled
}

/// Text: escaped, `--`/`---`/`...`/`\-` as typography when `-:` is on, and
/// line breaks kept when `\n:` is.
fn text(t: &str, o: &Options, out: &mut String) {
    let mut rest = t;
    while let Some(c) = rest.chars().next() {
        if o.special_strings {
            let (with, len) = if rest.starts_with("\\-") {
                ("&#x00ad;", 2)
            } else if rest.starts_with("---") {
                ("&#x2014;", 3)
            } else if rest.starts_with("--") {
                ("&#x2013;", 2)
            } else if rest.starts_with("...") {
                ("&#x2026;", 3)
            } else {
                ("", 0)
            };
            if len > 0 {
                out.push_str(with);
                rest = &rest[len..];
                continue;
            }
        }
        match c {
            '\n' if o.preserve_breaks => out.push_str("<br />\n"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
        rest = &rest[c.len_utf8()..];
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::rc::Rc;

    use super::super::pipeline::{Backend, Request, Run, Step};
    use super::super::{Host, IdFile};
    use super::*;
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

    /// `text` exported to HTML from `/notes/plan.org`, or the echo that
    /// stopped it.
    fn export_with(text: &str, stylesheet: Stylesheet, body_only: bool) -> Result<String, String> {
        let request = Request {
            buffer: 1,
            source: "/notes/plan.org".to_string(),
            target: "/notes/plan.html".to_string(),
            backend: Backend::Html,
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
            stylesheet,
        )
        .map_err(|e| format!("{e:?}"))?;
        loop {
            if let Step::Done(effects) = run.step() {
                return effects
                    .iter()
                    .find_map(|e| match e {
                        Effect::WriteToFile(w) => Some(w.text.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| format!("{effects:?}"));
            }
        }
    }

    fn export(text: &str) -> String {
        export_with(text, Stylesheet::None, false).unwrap_or_else(|e| panic!("{e}"))
    }

    fn body(text: &str) -> String {
        export_with(text, Stylesheet::None, true).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Read `html` as strict XML: every tag closed in order, every entity
    /// known. Gives back the ids it defines and the `#fragment`s it links to.
    fn well_formed(html: &str) -> (HashSet<String>, Vec<String>) {
        use quick_xml::events::Event;
        let mut reader = quick_xml::Reader::from_str(html);
        let (mut ids, mut links) = (HashSet::new(), Vec::new());
        loop {
            let event = match reader.read_event() {
                Ok(e) => e,
                Err(e) => panic!(
                    "not well-formed at byte {}: {e}\n{html}",
                    reader.buffer_position()
                ),
            };
            match event {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) => {
                    for a in e.attributes() {
                        let a = a.unwrap_or_else(|err| panic!("bad attribute: {err}\n{html}"));
                        let value = a
                            .unescape_value()
                            .unwrap_or_else(|err| panic!("bad attribute value: {err}"))
                            .to_string();
                        match a.key.as_ref() {
                            b"id" => {
                                assert!(ids.insert(value.clone()), "id {value} twice\n{html}");
                            }
                            b"href" => {
                                if let Some(fragment) = value.strip_prefix('#') {
                                    links.push(fragment.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Event::GeneralRef(r) => {
                    let numeric = r
                        .resolve_char_ref()
                        .unwrap_or_else(|err| panic!("bad reference: {err}\n{html}"));
                    let named = matches!(&*r, b"amp" | b"lt" | b"gt" | b"quot" | b"apos");
                    assert!(numeric.is_some() || named, "an entity XML lacks\n{html}");
                }
                _ => {}
            }
        }
        (ids, links)
    }

    /// Compare the export of `testdata/NAME.org` with `testdata/NAME.html`;
    /// `UPDATE_GOLDEN=1` rewrites the `.html` instead. Every golden is also
    /// well-formed, and each of its internal links lands on an id.
    fn golden(name: &str) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/export/parse/testdata");
        let org = std::fs::read_to_string(dir.join(format!("{name}.org"))).expect("corpus file");
        let got = export(&org);
        let (ids, links) = well_formed(&got);
        for link in &links {
            assert!(ids.contains(link), "{name}: #{link} goes nowhere\n{got}");
        }
        let path = dir.join(format!("{name}.html"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &got).expect("write golden");
            return;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            got == want,
            "{name}: the HTML changed; UPDATE_GOLDEN=1 rewrites it after review.\n--- got\n{got}\n--- want\n{want}"
        );
    }

    #[test]
    fn golden_headlines() {
        golden("headlines");
    }

    #[test]
    fn golden_lists() {
        golden("lists");
    }

    #[test]
    fn golden_blocks() {
        golden("blocks");
    }

    #[test]
    fn golden_misc() {
        golden("misc");
    }

    #[test]
    fn golden_objects() {
        golden("objects");
    }

    /// Numbers count per level from the export's top headline; the contents
    /// nest as the outline does and skip what is not numbered for it.
    #[test]
    fn numbers_and_the_table_of_contents_follow_the_outline() {
        let html = body(
            "** One\n*** One.a\n*** One.b\n** Two\n:PROPERTIES:\n:UNNUMBERED: t\n:END:\n\
             *** Under two\n** Three\n:PROPERTIES:\n:UNNUMBERED: notoc\n:END:\n** Four\n",
        );
        well_formed(&html);
        let toc = &html[..html.find("</div>\n</div>\n").expect("a toc")];
        for entry in [
            "1. One",
            "1.1. One.a",
            "1.2. One.b",
            ">Two<",
            ">Under two<",
            "2. Four",
        ] {
            assert!(toc.contains(entry), "{entry} in\n{toc}");
        }
        assert!(!toc.contains("Three"), "{toc}");
        assert!(html.contains("<h2 id=\"org"), "{html}");
        assert!(html.contains("<span class=\"section-number-2\">2.</span> Four</h2>"));
        assert!(html.contains("<h3"), "a second level is h3:\n{html}");
    }

    #[test]
    fn the_options_turn_numbers_and_contents_off() {
        let html = body("#+OPTIONS: num:nil toc:nil\n* One\n");
        assert!(!html.contains("table-of-contents"), "{html}");
        assert!(!html.contains("section-number"), "{html}");
    }

    /// Below `H:` a headline is an item in a list, numbered or not as its
    /// level is.
    #[test]
    fn headlines_below_h_are_list_items() {
        let html = body("#+OPTIONS: H:1 toc:nil\n* Top\n** Deep\ntext\n** Deeper\n* Next\n");
        well_formed(&html);
        assert!(
            html.contains("<ol class=\"org-ol\">\n<li><a id=\"org"),
            "{html}"
        );
        assert_eq!(html.matches("<li>").count(), 2, "{html}");
        let next = html.find(" Next</h2>").expect("Next");
        assert!(
            html[..next].contains("</ol>"),
            "the list closes before Next"
        );
    }

    #[test]
    fn keywords_are_classed_done_or_todo() {
        let html = body("#+OPTIONS: toc:nil\n* TODO a\n* DONE b\n");
        assert!(
            html.contains("<span class=\"todo TODO\">TODO</span> a"),
            "{html}"
        );
        assert!(
            html.contains("<span class=\"done DONE\">DONE</span> b"),
            "{html}"
        );
    }

    /// Footnotes are numbered as first referenced, wherever their
    /// definitions are, and the heading that holds them is not exported.
    #[test]
    fn footnotes_are_numbered_by_first_reference() {
        let html = body(
            "#+OPTIONS: toc:nil\n* Text\nSecond[fn:b], first[fn:a] and an inline[fn:: here].\n\
             Again[fn:b].\n* Footnotes\n[fn:a] Alpha.\n[fn:b] Beta.\n",
        );
        let (ids, links) = well_formed(&html);
        for link in &links {
            assert!(ids.contains(link), "#{link}");
        }
        assert!(!html.contains(">Footnotes</h2>"), "{html}");
        let notes = &html[html.find("id=\"footnotes\"").expect("footnotes")..];
        let at = |s: &str| notes.find(s).unwrap_or_else(|| panic!("{s} in {notes}"));
        assert!(at("Beta.") < at("Alpha.") && at("Alpha.") < at("here"));
        assert!(
            html.contains("id=\"fnr.1.2\""),
            "a second reference has its own id"
        );
    }

    #[test]
    fn math_brings_mathjax_and_only_then() {
        let with = export("* M\n$x^2$ and \\[y\\]\n");
        assert!(
            with.contains("MathJax-script") && with.contains("\\(x^2\\)"),
            "{with}"
        );
        well_formed(&with);
        assert!(!export("* M\nno math\n").contains("MathJax"));
    }

    #[test]
    fn links_to_files_ids_and_images() {
        let html = body(
            "#+OPTIONS: toc:nil\n* L\n[[file:other.org][o]] [[id:n1][n]] [[./a.png]] \
             [[https://x.y/z][w]]\n",
        );
        assert!(html.contains("<a href=\"other.html\">o</a>"), "{html}");
        assert!(
            html.contains("<a href=\"sub/n1.html#ID-n1\">n</a>"),
            "{html}"
        );
        assert!(
            html.contains("<img src=\"./a.png\" alt=\"a.png\" />"),
            "{html}"
        );
        assert!(html.contains("<a href=\"https://x.y/z\">w</a>"), "{html}");
    }

    /// A headline with an `:ID:` carries `ID-<id>`, where an `id:` link
    /// from another file goes, whether or not its properties are exported.
    #[test]
    fn org_ids_are_anchors_even_without_properties() {
        struct Here;
        impl Host for Here {
            fn read_file(&self, _: &str) -> Result<String, String> {
                Err(String::new())
            }
            fn id_file(&self, _: &str) -> IdFile {
                IdFile::In("/notes/plan.org".to_string())
            }
            fn output_buffer(&self, _: Backend) -> Option<(u32, String)> {
                None
            }
        }
        let request = Request {
            buffer: 1,
            source: "/notes/plan.org".to_string(),
            target: "/notes/plan.html".to_string(),
            backend: Backend::Html,
            subtree: false,
            body_only: true,
            line: 0,
            sink: crate::export::sink::Sink::File,
        };
        let text = "* A
:PROPERTIES:
:ID: abc
:END:
* B
[[id:abc][back]]
";
        let Ok(mut run) = Run::new(
            1,
            request,
            Some(text.to_string()),
            Vec::new(),
            When { local_secs: 0 },
            Rc::new(Here),
            Stylesheet::None,
        ) else {
            panic!("no run");
        };
        let html = loop {
            if let Step::Done(effects) = run.step() {
                break effects
                    .iter()
                    .find_map(|e| match e {
                        Effect::WriteToFile(w) => Some(w.text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
            }
        };
        let (ids, links) = well_formed(&html);
        assert!(ids.contains("ID-abc"), "{html}");
        // The filter knows the headline, so the link takes its anchor.
        assert!(html.contains("<a href=\"#org2f124dc1\">back</a>"), "{html}");
        assert!(links.iter().all(|l| ids.contains(l)), "{html}");
    }

    #[test]
    fn the_stylesheet_option_inlines_links_or_leaves_it_out() {
        let text = "* A\n";
        let inline = export_with(text, Stylesheet::Default, false).unwrap_or_default();
        assert!(inline.contains("<style>\n:root"), "{inline}");
        well_formed(&inline);
        let linked =
            export_with(text, Stylesheet::from_option("site.css"), false).unwrap_or_default();
        assert!(linked.contains("<link rel=\"stylesheet\" type=\"text/css\" href=\"site.css\" />"));
        assert!(!export(text).contains("<style>"));
    }

    #[test]
    fn body_only_is_the_content_alone() {
        let html = body("#+TITLE: T\n#+AUTHOR: A\n* One\n");
        assert!(
            !html.contains("<html") && !html.contains("postamble"),
            "{html}"
        );
        assert!(html.starts_with("<div id=\"table-of-contents\""), "{html}");
    }

    #[test]
    fn the_postamble_names_the_author_and_date() {
        let html = export("#+AUTHOR: Ada\n#+DATE: 2026-01-05\n* One\n");
        assert!(
            html.contains("<p class=\"author\">Author: Ada</p>"),
            "{html}"
        );
        assert!(
            html.contains("<p class=\"date\">Date: 2026-01-05</p>"),
            "{html}"
        );
        let html = export("#+OPTIONS: author:nil date:nil\n#+AUTHOR: Ada\n* One\n");
        assert!(!html.contains("postamble"), "{html}");
    }

    /// Text that looks like markup is escaped, and raw HTML from an export
    /// block or snippet is not.
    #[test]
    fn text_is_escaped_and_raw_html_is_not() {
        let html = body(
            "#+OPTIONS: toc:nil\n* <b>\na < b & \"c\" -- d\n\
             #+begin_export html\n<hr/>\n#+end_export\n@@html:<br/>@@\n",
        );
        assert!(html.contains("&lt;b&gt;</h2>"), "{html}");
        assert!(
            html.contains("a &lt; b &amp; &quot;c&quot; &#x2013; d"),
            "{html}"
        );
        assert!(html.contains("<hr/>\n") && html.contains("<br/>"), "{html}");
    }

    /// A table's header is the rows above its first rule, and a column of
    /// numbers is right-aligned without a cookie.
    #[test]
    fn tables_have_a_head_and_align_numbers() {
        let html = body("#+OPTIONS: toc:nil\n* T\n| a | n |\n|---+---|\n| x | 1 |\n| y | 22 |\n");
        well_formed(&html);
        assert!(html.contains("<thead>\n<tr>\n<th scope=\"col\" class=\"org-left\">a</th>"));
        assert!(html.contains("<td class=\"org-right\">22</td>"), "{html}");
    }

    #[test]
    fn source_blocks_carry_their_language_class() {
        let html = body("#+OPTIONS: toc:nil\n* S\n#+begin_src rust\nfn a() {}\n#+end_src\n");
        assert!(
            html.contains(
                "<div class=\"org-src-container\">\n<pre class=\"src src-rust\">fn a() {}"
            ),
            "{html}"
        );
    }
}

//! Export options, resolved in ox's order of precedence (lattice's
//! `org-export.md` §3.3), later winning: the plugin's `org.export.*` options
//! (ox's defaults until a user sets them), then the file's keywords and
//! `#+OPTIONS:` lines in file order, then, for a subtree export, the
//! headline's `EXPORT_*` properties.
//!
//! Every option here has ox's name and default, so a file written for emacs
//! exports the same. What a file asks for and this does not implement is
//! collected in [`Options::ignored`], for the export's echo to name once,
//! rather than dropped without a word.

use std::collections::BTreeMap;

use lattice_plugin_sdk::ConfigShape as ConfigShapeDerive;

use super::parse::objects::{self, Object};

/// Which `_` / `^` are sub- and superscripts: `^:`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SubSuperscripts {
    /// `^:t`: `a_b`, `a^{bc}`.
    #[default]
    All,
    /// `^:{}`: only the braced forms.
    Braced,
    /// `^:nil`: none; `_` and `^` are characters.
    None,
}

/// A switch that may instead carry a depth: `toc:2`, `num:3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Depth {
    Off,
    /// To the headline depth `H`.
    On,
    /// To this level.
    To(usize),
}

/// `tags:` — `t`, `nil`, or `not-in-toc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tags {
    On,
    Off,
    NotInToc,
}

/// `<:` — which timestamps are exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Timestamps {
    All,
    None,
    Active,
    Inactive,
}

/// `d:` — which drawers are exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Drawers {
    All,
    None,
    /// Only these, by name.
    Only(Vec<String>),
    /// All but these: ox's default is all but `LOGBOOK`.
    Except(Vec<String>),
}

impl Drawers {
    pub(crate) fn keeps(&self, name: &str) -> bool {
        let named = |list: &[String]| list.iter().any(|n| n.eq_ignore_ascii_case(name));
        match self {
            Drawers::All => true,
            Drawers::None => false,
            Drawers::Only(list) => named(list),
            Drawers::Except(list) => !named(list),
        }
    }
}

/// `tex:` — LaTeX fragments and environments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tex {
    /// Processed: MathJax in HTML, native in LaTeX.
    On,
    /// Dropped.
    Off,
    /// Kept as verbatim text.
    Verbatim,
}

/// `broken-links:` — what an internal link that resolves to nothing does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokenLinks {
    /// The export fails, naming the link.
    Error,
    /// The link is dropped, description and all, as in ox.
    Ignore,
    /// The link is replaced by a `[BROKEN LINK: …]` marker.
    Mark,
}

/// How a LaTeX export writes source blocks: emacs's
/// `org-latex-src-block-backend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SrcBackend {
    Verbatim,
    Listings,
    Minted,
}

impl SrcBackend {
    /// `verbatim`, `listings` or `minted`; `None` for anything else.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "verbatim" => Some(SrcBackend::Verbatim),
            "listings" => Some(SrcBackend::Listings),
            "minted" => Some(SrcBackend::Minted),
            _ => None,
        }
    }
}

/// One `[[org.export.latex-classes]]` entry as the user writes it.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShapeDerive)]
pub(crate) struct DeclaredClass {
    /// What `#+LATEX_CLASS:` names it by.
    pub(crate) name: String,
    /// The `\documentclass` line and anything after it the class needs;
    /// absent is `\documentclass[11pt]{<name>}`.
    pub(crate) header: Option<String>,
    /// The sectioning commands, a headline level each: `["chapter",
    /// "section"]`.
    pub(crate) sections: Vec<String>,
}

impl From<DeclaredClass> for LatexClass {
    fn from(d: DeclaredClass) -> Self {
        let name = d.name.trim().to_string();
        LatexClass {
            header: d
                .header
                .filter(|h| !h.trim().is_empty())
                .unwrap_or_else(|| format!("\\documentclass[11pt]{{{name}}}")),
            sections: d
                .sections
                .into_iter()
                .map(|s| s.trim().trim_start_matches('\\').to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            name,
        }
    }
}

/// A LaTeX class: its name, its `\documentclass` line (and anything after
/// it the class needs), and its sectioning commands a level each, as an
/// entry of emacs's `org-latex-classes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LatexClass {
    pub(crate) name: String,
    pub(crate) header: String,
    pub(crate) sections: Vec<String>,
}

/// The classes every LaTeX export knows, as ox's defaults.
pub(crate) fn builtin_classes() -> Vec<LatexClass> {
    let class = |name: &str, sections: &[&str]| LatexClass {
        name: name.to_string(),
        header: format!("\\documentclass[11pt]{{{name}}}"),
        sections: sections.iter().map(|s| s.to_string()).collect(),
    };
    let book = ["part", "chapter", "section", "subsection", "subsubsection"];
    vec![
        class(
            "article",
            &[
                "section",
                "subsection",
                "subsubsection",
                "paragraph",
                "subparagraph",
            ],
        ),
        class("report", &book),
        class("book", &book),
    ]
}

impl Options {
    /// Add `classes` to the known ones, each replacing a known one of its
    /// name.
    pub(crate) fn add_classes(&mut self, classes: impl IntoIterator<Item = LatexClass>) {
        for class in classes {
            match self.latex_classes.iter_mut().find(|c| c.name == class.name) {
                Some(known) => *known = class,
                None => self.latex_classes.push(class),
            }
        }
    }
}

/// Everything an export reads from options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) toc: Depth,
    pub(crate) num: Depth,
    /// `H:` — headlines deeper than this are list items.
    pub(crate) headline_levels: usize,
    pub(crate) subsup: SubSuperscripts,
    pub(crate) with_author: bool,
    pub(crate) with_date: bool,
    pub(crate) with_title: bool,
    pub(crate) with_email: bool,
    pub(crate) tags: Tags,
    pub(crate) todo: bool,
    pub(crate) priority: bool,
    pub(crate) timestamps: Timestamps,
    /// `p:` — planning lines.
    pub(crate) planning: bool,
    /// `c:` — clock lines.
    pub(crate) clocks: bool,
    /// `prop:` — property drawers.
    pub(crate) properties: bool,
    pub(crate) drawers: Drawers,
    /// `f:` — footnotes.
    pub(crate) footnotes: bool,
    /// `stat:` — statistics cookies.
    pub(crate) statistics_cookies: bool,
    pub(crate) tex: Tex,
    pub(crate) broken_links: BrokenLinks,
    /// `*:` — emphasis; off leaves the markers as characters.
    pub(crate) emphasis: bool,
    /// `-:` — `--`, `---`, `...` and `\-` as typography.
    pub(crate) special_strings: bool,
    /// `::` — fixed-width areas.
    pub(crate) fixed_width: bool,
    /// `|:` — tables.
    pub(crate) tables: bool,
    /// `e:` — entities; off leaves `\name` as written.
    pub(crate) entities: bool,
    /// `\n:` — keep the source's line breaks.
    pub(crate) preserve_breaks: bool,

    pub(crate) title: Vec<Object>,
    pub(crate) subtitle: Vec<Object>,
    pub(crate) author: Vec<Object>,
    pub(crate) email: String,
    pub(crate) date: Vec<Object>,
    pub(crate) language: String,
    pub(crate) select_tags: Vec<String>,
    pub(crate) exclude_tags: Vec<String>,
    /// `#+FILETAGS:`, inherited by every headline.
    pub(crate) file_tags: Vec<String>,
    pub(crate) html_head: Vec<String>,
    pub(crate) html_head_extra: Vec<String>,
    pub(crate) latex_class: String,
    pub(crate) latex_class_options: String,
    /// `org.export.latex-header`'s lines, then the file's `#+LATEX_HEADER`s.
    pub(crate) latex_header: Vec<String>,
    pub(crate) latex_header_extra: Vec<String>,
    /// `pdflatex`, `xelatex` or `lualatex`; anything else stops a LaTeX
    /// export, naming it.
    pub(crate) latex_compiler: String,
    /// The built-in classes, then `org.export.latex-classes`, a later one of
    /// a name replacing an earlier.
    pub(crate) latex_classes: Vec<LatexClass>,
    /// `org.export.latex-src-block-backend`, as written: a LaTeX export
    /// parses it, and refuses a value that is not a [`SrcBackend`].
    pub(crate) latex_src_backend: String,
    /// The width an image with none of its own is drawn at.
    pub(crate) latex_image_width: String,
    /// Macro templates by name: the keyword built-ins, then `#+MACRO:` lines,
    /// which win.
    pub(crate) macros: BTreeMap<String, String>,

    /// What the file asked for that this does not do: `#+KEY` and
    /// `OPTIONS key:`, each once, in the order first seen.
    pub(crate) ignored: Vec<String>,
}

impl Default for Options {
    /// ox's defaults.
    fn default() -> Self {
        Options {
            toc: Depth::On,
            num: Depth::On,
            headline_levels: 3,
            subsup: SubSuperscripts::All,
            with_author: true,
            with_date: true,
            with_title: true,
            with_email: false,
            tags: Tags::On,
            todo: true,
            priority: false,
            timestamps: Timestamps::All,
            planning: false,
            clocks: false,
            properties: false,
            drawers: Drawers::Except(vec!["LOGBOOK".to_string()]),
            footnotes: true,
            statistics_cookies: true,
            tex: Tex::On,
            broken_links: BrokenLinks::Error,
            emphasis: true,
            special_strings: true,
            fixed_width: true,
            tables: true,
            entities: true,
            preserve_breaks: false,
            title: Vec::new(),
            subtitle: Vec::new(),
            author: Vec::new(),
            email: String::new(),
            date: Vec::new(),
            language: "en".to_string(),
            select_tags: vec!["export".to_string()],
            exclude_tags: vec!["noexport".to_string()],
            file_tags: Vec::new(),
            html_head: Vec::new(),
            html_head_extra: Vec::new(),
            latex_class: "article".to_string(),
            latex_class_options: String::new(),
            latex_header: Vec::new(),
            latex_header_extra: Vec::new(),
            latex_compiler: "pdflatex".to_string(),
            latex_classes: builtin_classes(),
            latex_src_backend: "verbatim".to_string(),
            latex_image_width: ".9\\linewidth".to_string(),
            macros: BTreeMap::new(),
            ignored: Vec::new(),
        }
    }
}

/// Keywords ox's export reads that this does not, named in the echo when a
/// file uses one. Keywords export never reads (`#+STARTUP`, `#+TODO`,
/// `#+PROPERTY`…) are not listed: a file using them loses nothing.
const UNIMPLEMENTED_KEYWORDS: &[&str] = &[
    "BIND",
    "CREATOR",
    "DESCRIPTION",
    "EXPORT_FILE_NAME",
    "HTML_CONTAINER",
    "HTML_DOCTYPE",
    "HTML_LINK_HOME",
    "HTML_LINK_UP",
    "HTML_MATHJAX",
    "KEYWORDS",
    "SETUPFILE",
];

impl Options {
    /// Options for a file whose keywords, in file order, are `keywords`, and,
    /// for a subtree export, whose headline is `subtree` (its raw title and
    /// properties), over `base`: the plugin's options.
    pub(crate) fn resolve(
        base: &Options,
        keywords: &[(String, String)],
        subtree: Option<(&str, &[(String, String)])>,
        file_name: &str,
    ) -> Self {
        let mut o = base.clone();
        let mut raw = Raw::default();
        for (key, value) in keywords {
            o.keyword(key, value, &mut raw);
        }
        if let Some((headline, properties)) = subtree {
            // A subtree's title is its headline unless it names another.
            raw.title = Some(headline.to_string());
            for (key, value) in properties {
                let key = key.to_ascii_uppercase();
                if let Some(k) = key.strip_prefix("EXPORT_") {
                    if k == "TITLE" {
                        raw.title = Some(value.to_string());
                    } else {
                        o.keyword(k, value, &mut raw);
                    }
                }
            }
        }
        let stem = file_name.strip_suffix(".org").unwrap_or(file_name);
        let title = raw.title.unwrap_or_else(|| stem.to_string());
        o.title = objects::parse(&title);
        o.macros = [
            ("title", title),
            ("author", raw.author.unwrap_or_default()),
            ("date", raw.date.unwrap_or_default()),
            ("email", o.email.clone()),
            ("input-file", file_name.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .chain(raw.macros)
        .collect();
        o
    }

    /// One keyword. Keywords that may repeat accumulate; the rest take the
    /// last value.
    fn keyword(&mut self, key: &str, value: &str, raw: &mut Raw) {
        let words = |v: &str| v.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        match key {
            "OPTIONS" => self.options_line(value),
            "TITLE" => match &mut raw.title {
                // `#+TITLE:` lines concatenate, as in ox.
                Some(t) => {
                    t.push(' ');
                    t.push_str(value);
                }
                None => raw.title = Some(value.to_string()),
            },
            "SUBTITLE" => self.subtitle = objects::parse(value),
            "AUTHOR" => {
                self.author = objects::parse(value);
                raw.author = Some(value.to_string());
            }
            "EMAIL" => self.email = value.to_string(),
            "DATE" => {
                self.date = objects::parse(value);
                raw.date = Some(value.to_string());
            }
            "MACRO" => {
                let (name, body) = value
                    .trim()
                    .split_once(char::is_whitespace)
                    .unwrap_or((value.trim(), ""));
                if !name.is_empty() {
                    raw.macros
                        .insert(name.to_ascii_lowercase(), body.trim().to_string());
                }
            }
            "LANGUAGE" => self.language = value.to_string(),
            "SELECT_TAGS" => self.select_tags = words(value),
            "EXCLUDE_TAGS" => self.exclude_tags = words(value),
            "FILETAGS" => self.file_tags.extend(
                value
                    .split([':', ' '])
                    .filter(|t| !t.is_empty())
                    .map(str::to_string),
            ),
            "HTML_HEAD" => self.html_head.push(value.to_string()),
            "HTML_HEAD_EXTRA" => self.html_head_extra.push(value.to_string()),
            "LATEX_CLASS" => self.latex_class = value.to_string(),
            "LATEX_CLASS_OPTIONS" => self.latex_class_options = value.to_string(),
            "LATEX_HEADER" => self.latex_header.push(value.to_string()),
            "LATEX_HEADER_EXTRA" => self.latex_header_extra.push(value.to_string()),
            "LATEX_COMPILER" => self.latex_compiler = value.trim().to_string(),
            k if UNIMPLEMENTED_KEYWORDS.contains(&k) => self.ignore(format!("#+{k}")),
            _ => {}
        }
    }

    /// Name `what` in the export's closing echo, once.
    pub(crate) fn ignore(&mut self, what: String) {
        if !self.ignored.contains(&what) {
            self.ignored.push(what);
        }
    }

    /// One `#+OPTIONS:` line: `key:value` items, values in ox's lisp
    /// spelling (`t`, `nil`, a number, a symbol, a `("list" "of" "strings")`).
    fn options_line(&mut self, line: &str) {
        for (key, value) in option_items(line) {
            if !self.option(&key, &value) {
                self.ignore(format!("{key}:"));
            }
        }
    }

    /// Apply one option; `false` when the key is not one this implements or
    /// the value is not one it takes.
    fn option(&mut self, key: &str, value: &Value) -> bool {
        use Value::{List, Nil, Num, Sym, T};
        let flag = |v: &Value| match v {
            T => Some(true),
            Nil => Some(false),
            _ => None,
        };
        let set = |slot: &mut bool, v: &Value| flag(v).map(|b| *slot = b).is_some();
        let depth = |v: &Value| match v {
            T => Some(Depth::On),
            Nil => Some(Depth::Off),
            Num(n) => Some(Depth::To(*n)),
            _ => None,
        };
        match key {
            "toc" => depth(value).map(|d| self.toc = d).is_some(),
            "num" => depth(value).map(|d| self.num = d).is_some(),
            "H" => match value {
                Num(n) => {
                    self.headline_levels = *n;
                    true
                }
                _ => false,
            },
            "^" => {
                self.subsup = match value {
                    T => SubSuperscripts::All,
                    Nil => SubSuperscripts::None,
                    Sym(s) if s == "{}" => SubSuperscripts::Braced,
                    _ => return false,
                };
                true
            }
            "author" => set(&mut self.with_author, value),
            "date" => set(&mut self.with_date, value),
            "title" => set(&mut self.with_title, value),
            "email" => set(&mut self.with_email, value),
            "tags" => {
                self.tags = match value {
                    T => Tags::On,
                    Nil => Tags::Off,
                    Sym(s) if s == "not-in-toc" => Tags::NotInToc,
                    _ => return false,
                };
                true
            }
            "todo" => set(&mut self.todo, value),
            "pri" => set(&mut self.priority, value),
            "<" => {
                self.timestamps = match value {
                    T => Timestamps::All,
                    Nil => Timestamps::None,
                    Sym(s) if s == "active" => Timestamps::Active,
                    Sym(s) if s == "inactive" => Timestamps::Inactive,
                    _ => return false,
                };
                true
            }
            "p" => set(&mut self.planning, value),
            "c" => set(&mut self.clocks, value),
            "prop" => set(&mut self.properties, value),
            "d" => {
                self.drawers = match value {
                    T => Drawers::All,
                    Nil => Drawers::None,
                    List(names) => match names.first().map(String::as_str) {
                        Some("not") => Drawers::Except(names[1..].to_vec()),
                        _ => Drawers::Only(names.clone()),
                    },
                    _ => return false,
                };
                true
            }
            "f" => set(&mut self.footnotes, value),
            "stat" => set(&mut self.statistics_cookies, value),
            "tex" => {
                self.tex = match value {
                    T => Tex::On,
                    Nil => Tex::Off,
                    Sym(s) if s == "verbatim" => Tex::Verbatim,
                    _ => return false,
                };
                true
            }
            "broken-links" => {
                self.broken_links = match value {
                    Nil => BrokenLinks::Error,
                    T => BrokenLinks::Ignore,
                    Sym(s) if s == "mark" => BrokenLinks::Mark,
                    _ => return false,
                };
                true
            }
            "*" => set(&mut self.emphasis, value),
            "-" => set(&mut self.special_strings, value),
            ":" => set(&mut self.fixed_width, value),
            "|" => set(&mut self.tables, value),
            "e" => set(&mut self.entities, value),
            "\\n" => set(&mut self.preserve_breaks, value),
            _ => false,
        }
    }
}

/// Keyword values kept as written while the keywords are read: the
/// built-in macros expand to them.
#[derive(Default)]
struct Raw {
    title: Option<String>,
    author: Option<String>,
    date: Option<String>,
    macros: BTreeMap<String, String>,
}

/// An `#+OPTIONS:` value, as ox reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    T,
    Nil,
    Num(usize),
    Sym(String),
    /// `("a" "b")` or `(not "a")`: the words, quotes taken off.
    List(Vec<String>),
}

/// The `key:value` items of an options line. A key runs to the first `:`
/// (so `\n:t` and `<:nil` are keys `\n` and `<`, and `::t` is the key `:`);
/// a value runs to whitespace, or to the closing `)` of a list.
fn option_items(line: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut rest = line.trim_start();
    while !rest.is_empty() {
        let colon = if rest.starts_with("::") {
            1
        } else {
            match rest.find(':').filter(|&c| c > 0) {
                Some(c) => c,
                None => break,
            }
        };
        let key = &rest[..colon];
        if key.contains(char::is_whitespace) {
            // Not a key: skip the word.
            rest = rest
                .find(char::is_whitespace)
                .map_or("", |w| rest[w..].trim_start());
            continue;
        }
        let after = &rest[colon + 1..];
        let (raw, next) = if after.starts_with('(') {
            match after.find(')') {
                Some(close) => (&after[..=close], &after[close + 1..]),
                None => (after, ""),
            }
        } else {
            match after.find(char::is_whitespace) {
                Some(w) => (&after[..w], &after[w..]),
                None => (after, ""),
            }
        };
        out.push((key.to_string(), value(raw)));
        rest = next.trim_start();
    }
    out
}

fn value(raw: &str) -> Value {
    if let Some(inner) = raw.strip_prefix('(') {
        return Value::List(
            inner
                .trim_end_matches(')')
                .split_whitespace()
                .map(|w| w.trim_matches('"').to_string())
                .collect(),
        );
    }
    match raw {
        "t" => Value::T,
        "nil" => Value::Nil,
        _ => raw
            .parse::<usize>()
            .map(Value::Num)
            .unwrap_or_else(|_| Value::Sym(raw.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kw(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_defaults_are_oxs() {
        let o = Options::resolve(&Options::default(), &[], None, "notes");
        assert_eq!((o.toc, o.num, o.headline_levels), (Depth::On, Depth::On, 3));
        assert!(!o.planning && !o.clocks && !o.properties && !o.priority);
        assert!(o.drawers.keeps("NOTES") && !o.drawers.keeps("logbook"));
        assert_eq!(o.exclude_tags, ["noexport"]);
        assert_eq!(o.title, objects::parse("notes"));
    }

    #[test]
    fn an_options_line_reads_every_spelling() {
        let o = Options::resolve(
            &Options::default(),
            &kw(&[(
                "OPTIONS",
                r#"toc:2 num:nil H:4 ^:{} \n:t <:active ::nil |:nil d:("NOTES" "X") tex:verbatim broken-links:mark *:nil"#,
            )]),
            None,
            "x",
        );
        assert_eq!(
            (o.toc, o.num, o.headline_levels),
            (Depth::To(2), Depth::Off, 4)
        );
        assert_eq!(o.subsup, SubSuperscripts::Braced);
        assert!(o.preserve_breaks && !o.fixed_width && !o.tables && !o.emphasis);
        assert_eq!(o.timestamps, Timestamps::Active);
        assert_eq!(o.drawers, Drawers::Only(vec!["NOTES".into(), "X".into()]));
        assert_eq!((o.tex, o.broken_links), (Tex::Verbatim, BrokenLinks::Mark));
        assert!(o.ignored.is_empty(), "{:?}", o.ignored);
        let not = Options::resolve(
            &Options::default(),
            &kw(&[("OPTIONS", r#"d:(not "LOGBOOK" "X")"#)]),
            None,
            "x",
        );
        assert!(not.drawers.keeps("NOTES") && !not.drawers.keeps("x"));
    }

    /// Later wins: a second `#+OPTIONS:` overrides the first, and a subtree's
    /// `EXPORT_OPTIONS` and `EXPORT_TITLE` override both.
    #[test]
    fn precedence_is_oxs() {
        let keywords = kw(&[
            ("OPTIONS", "toc:nil num:nil"),
            ("TITLE", "The"),
            ("TITLE", "File"),
            ("OPTIONS", "toc:t"),
        ]);
        let o = Options::resolve(&Options::default(), &keywords, None, "x");
        assert_eq!((o.toc, o.num), (Depth::On, Depth::Off));
        assert_eq!(o.title, objects::parse("The File"));

        let props = kw(&[("EXPORT_OPTIONS", "num:t"), ("CUSTOM_ID", "x")]);
        let sub = Options::resolve(
            &Options::default(),
            &keywords,
            Some(("Heading", &props)),
            "x",
        );
        assert_eq!(sub.num, Depth::On);
        assert_eq!(
            sub.title,
            objects::parse("Heading"),
            "the headline titles a subtree"
        );
        let named = kw(&[("EXPORT_TITLE", "Named")]);
        let sub = Options::resolve(
            &Options::default(),
            &keywords,
            Some(("Heading", &named)),
            "x",
        );
        assert_eq!(sub.title, objects::parse("Named"));
    }

    /// What is asked for and not done is named once each, keywords and
    /// option keys alike.
    #[test]
    fn what_is_not_implemented_is_named_once() {
        let o = Options::resolve(
            &Options::default(),
            &kw(&[
                ("SETUPFILE", "x.org"),
                ("OPTIONS", "html-postamble:nil arch:nil toc:maybe"),
                ("SETUPFILE", "y.org"),
                ("STARTUP", "overview"),
            ]),
            None,
            "x",
        );
        assert_eq!(
            o.ignored,
            ["#+SETUPFILE", "html-postamble:", "arch:", "toc:"]
        );
    }

    /// The keyword built-ins follow the keywords' own precedence, and a
    /// `#+MACRO:` line beats a built-in of the same name.
    #[test]
    fn macros_collect_the_built_ins_and_the_files() {
        let o = Options::resolve(
            &Options::default(),
            &kw(&[
                ("TITLE", "Plan"),
                ("AUTHOR", "Ada"),
                ("MACRO", "greet Hello, $1"),
                ("MACRO", "Email someone@example.invalid"),
                ("MACRO", "greet  Hi, $1 "),
            ]),
            Some(("Sub", &kw(&[("EXPORT_AUTHOR", "Grace")]))),
            "notes.org",
        );
        let get = |k: &str| o.macros.get(k).map(String::as_str);
        assert_eq!(get("title"), Some("Sub"));
        assert_eq!(get("author"), Some("Grace"));
        assert_eq!(get("greet"), Some("Hi, $1"));
        assert_eq!(get("email"), Some("someone@example.invalid"));
        assert_eq!(get("input-file"), Some("notes.org"));
        assert_eq!(o.title, objects::parse("Sub"));
        assert_eq!(
            Options::resolve(&Options::default(), &[], None, "notes.org").title,
            objects::parse("notes")
        );
    }

    #[test]
    fn file_tags_accumulate() {
        let o = Options::resolve(
            &Options::default(),
            &kw(&[("FILETAGS", ":a:b:"), ("FILETAGS", "c")]),
            None,
            "x",
        );
        assert_eq!(o.file_tags, ["a", "b", "c"]);
    }
}

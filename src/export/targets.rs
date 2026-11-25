//! Where internal links go: ox's link resolution.
//!
//! The index is built as the parse steps run, a section at a time, so no
//! call walks the whole tree for it. Once the render's selection is known it
//! learns which headlines are exported: a target inside a subtree the export
//! leaves out is not a target, as in ox, where links resolve against the
//! pruned tree.
//!
//! Resolution follows ox: `#id` is a `CUSTOM_ID`, `*Title` a headline, `id:`
//! an `ID` property (here, or elsewhere through the roam index), and a plain
//! path a `<<target>>` or radio target, then a `#+NAME:`, then a headline
//! title. Plain paths and titles match with whitespace collapsed and ASCII
//! case folded.
//!
//! Anchors are a headline's `CUSTOM_ID` when it has one, else `org` and a
//! hash of what they name: the same text names the same anchor in every
//! export, so a link into an exported page survives a re-export.

use std::collections::{HashMap, HashSet};

use super::parse::objects::Object;
use super::parse::{Element, ElementKind, FlatSection, Row};
use super::{Host, IdFile};

/// What a link path names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// A place in this export, by anchor.
    Internal(String),
    /// Somewhere else: a URL, a file, a node in another file.
    External,
    /// Nothing.
    Broken,
    /// A kind of link this does not resolve, named for the echo; it is
    /// exported as written.
    Unsupported(String),
}

/// The link targets of one file.
#[derive(Default)]
pub(crate) struct Targets {
    /// `CUSTOM_ID` → headline line.
    custom_ids: HashMap<String, u32>,
    /// Headline line → its `CUSTOM_ID`, for its anchor.
    anchors: HashMap<u32, String>,
    /// `ID` (lowercased, as org-id compares them) → headline line.
    ids: HashMap<String, u32>,
    /// Normalised title → the first headline with it.
    titles: HashMap<String, u32>,
    /// Normalised `<<target>>` / `<<<radio>>>` text → its headline.
    targets: HashMap<String, Option<u32>>,
    /// Normalised `#+NAME:` → its headline.
    names: HashMap<String, Option<u32>>,
    /// Radio targets: the text as matched (normalised) and its anchor.
    radios: Vec<(String, String)>,
    /// Every link path in the file, as written, for which anchors something
    /// links to.
    links: HashSet<String>,
    /// The headlines the export shows; `None` until the selection is known.
    live: Option<HashSet<u32>>,
    /// Whether the text before the first headline is exported.
    preamble_live: bool,
}

/// `s` with runs of whitespace one space and ASCII case folded.
fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// `org` and a 32-bit FNV-1a hash of `kind` and `name`.
fn hashed(kind: &str, name: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in kind.bytes().chain([0]).chain(name.bytes()) {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("org{h:08x}")
}

impl Targets {
    /// Take in one parsed section.
    pub(crate) fn collect(&mut self, section: &FlatSection) {
        let at = section.headline.as_ref().map(|h| h.line);
        if let Some(h) = &section.headline {
            for (key, value) in &h.properties {
                let value = value.trim();
                if key.eq_ignore_ascii_case("CUSTOM_ID") && !value.is_empty() {
                    self.custom_ids.entry(value.to_string()).or_insert(h.line);
                    self.anchors.insert(h.line, value.to_string());
                } else if key.eq_ignore_ascii_case("ID") && !value.is_empty() {
                    self.ids.entry(value.to_ascii_lowercase()).or_insert(h.line);
                }
            }
            self.titles.entry(norm(&h.raw_title)).or_insert(h.line);
            self.objects(&h.title, at);
        }
        self.elements(&section.elements, at);
    }

    fn elements(&mut self, list: &[Element], at: Option<u32>) {
        for e in list {
            if let Some(name) = &e.affiliated.name {
                self.names.entry(norm(name)).or_insert(at);
            }
            for caption in &e.affiliated.caption {
                self.objects(caption, at);
            }
            match &e.kind {
                ElementKind::Paragraph(objs) => self.objects(objs, at),
                ElementKind::PlainList(list) => {
                    for item in &list.items {
                        if let Some(tag) = &item.tag {
                            self.objects(tag, at);
                        }
                        self.elements(&item.contents, at);
                    }
                }
                ElementKind::Table(t) => {
                    for row in &t.rows {
                        if let Row::Cells(cells) = row {
                            for cell in cells {
                                self.objects(cell, at);
                            }
                        }
                    }
                }
                ElementKind::Block(b) => self.elements(&b.contents, at),
                ElementKind::Drawer { contents, .. }
                | ElementKind::FootnoteDefinition { contents, .. } => self.elements(contents, at),
                _ => {}
            }
        }
    }

    fn objects(&mut self, list: &[Object], at: Option<u32>) {
        for o in list {
            match o {
                Object::Target(t) => {
                    self.targets.entry(norm(t)).or_insert(at);
                }
                Object::RadioTarget(contents) => {
                    let name = norm(&super::filter::plain(contents));
                    if !name.is_empty() && !self.targets.contains_key(&name) {
                        self.targets.insert(name.clone(), at);
                        let anchor = hashed("target", &name);
                        self.radios.push((name, anchor));
                    }
                }
                Object::Emphasis { contents, .. }
                | Object::Subscript { contents, .. }
                | Object::Superscript { contents, .. } => self.objects(contents, at),
                Object::FootnoteReference { definition, .. } => self.objects(definition, at),
                Object::Link {
                    path, description, ..
                } => {
                    if !self.links.contains(path) {
                        self.links.insert(path.clone());
                    }
                    self.objects(description, at);
                }
                _ => {}
            }
        }
    }

    /// The headlines the export shows, and whether the preamble is among
    /// what it shows.
    pub(crate) fn set_live(&mut self, live: HashSet<u32>, preamble: bool) {
        self.live = Some(live);
        self.preamble_live = preamble;
    }

    fn is_live(&self, at: Option<u32>) -> bool {
        match (at, &self.live) {
            (_, None) => true,
            (None, Some(_)) => self.preamble_live,
            (Some(line), Some(live)) => live.contains(&line),
        }
    }

    /// The anchor of the headline at `line`.
    pub(crate) fn headline_anchor(&self, line: u32) -> String {
        self.anchors
            .get(&line)
            .cloned()
            .unwrap_or_else(|| hashed("headline", &line.to_string()))
    }

    /// The anchor of a `<<target>>` with this text.
    pub(crate) fn target_anchor(&self, text: &str) -> String {
        hashed("target", &norm(text))
    }

    /// The anchor of an element with this `#+NAME:`.
    pub(crate) fn name_anchor(&self, name: &str) -> String {
        hashed("name", &norm(name))
    }

    /// The anchors some link in the file goes to: a backend that writes an
    /// anchor only where one is needed (Markdown's headings) asks this
    /// before the walk, since a link may come before what it names. One
    /// resolution per distinct path.
    pub(crate) fn referred(&self, host: &dyn Host) -> HashSet<String> {
        self.links
            .iter()
            .filter_map(|path| match self.resolve(path, host) {
                Resolution::Internal(anchor) => Some(anchor),
                _ => None,
            })
            .collect()
    }

    /// The live radio targets, as `(normalised text, anchor)`.
    pub(crate) fn radios(&self) -> Vec<(String, String)> {
        self.radios
            .iter()
            .filter(|(name, _)| self.is_live(self.targets.get(name).copied().flatten()))
            .cloned()
            .collect()
    }

    /// Where `path` goes.
    pub(crate) fn resolve(&self, path: &str, host: &dyn Host) -> Resolution {
        let headline = |line: Option<&u32>| match line {
            Some(&l) if self.is_live(Some(l)) => Resolution::Internal(self.headline_anchor(l)),
            _ => Resolution::Broken,
        };
        if let Some(id) = path.strip_prefix('#') {
            return headline(self.custom_ids.get(id));
        }
        if let Some(title) = path.strip_prefix('*') {
            return headline(self.titles.get(&norm(title)));
        }
        if path.starts_with('(') && path.ends_with(')') {
            return Resolution::Unsupported("code references".to_string());
        }
        if ["/", "./", "../", "~/"].iter().any(|p| path.starts_with(p)) {
            return Resolution::External;
        }
        if let Some((scheme, rest)) = path.split_once(':') {
            if scheme.eq_ignore_ascii_case("id") {
                if let Some(&line) = self.ids.get(&rest.to_ascii_lowercase()) {
                    return headline(Some(&line));
                }
                // Another file's node is external; one no index knows is
                // broken. With no index to ask there is no telling, and the
                // link is let through.
                return match host.id_file(rest) {
                    IdFile::Unknown => Resolution::Broken,
                    IdFile::In(_) | IdFile::NoIndex => Resolution::External,
                };
            }
            if super::parse::objects::is_link_scheme(scheme) {
                return Resolution::External;
            }
        }
        let n = norm(path);
        if let Some(&at) = self.targets.get(&n) {
            if self.is_live(at) {
                return Resolution::Internal(self.target_anchor(&n));
            }
        }
        if let Some(&at) = self.names.get(&n) {
            if self.is_live(at) {
                return Resolution::Internal(self.name_anchor(&n));
            }
        }
        headline(self.titles.get(&n))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::parse::{self, Context};
    use super::*;

    /// A host that reads no file and knows the ids given; `None` is a host
    /// with no index.
    pub(crate) struct Ids(pub(crate) Option<Vec<&'static str>>);

    impl Host for Ids {
        fn read_file(&self, path: &str) -> Result<String, String> {
            Err(format!("no {path}"))
        }
        fn id_file(&self, id: &str) -> IdFile {
            match &self.0 {
                None => IdFile::NoIndex,
                Some(ids) if ids.contains(&id) => IdFile::In(format!("/notes/{id}.org")),
                Some(_) => IdFile::Unknown,
            }
        }
        fn output_buffer(&self, _: super::super::Backend) -> Option<(u32, String)> {
            None
        }
    }

    fn targets(text: &str) -> Targets {
        let lines: Vec<&str> = text.lines().collect();
        let mut t = Targets::default();
        for section in parse::parse_chunk(&lines, 0, &Context::for_file(text, Vec::new())) {
            t.collect(&section);
        }
        t
    }

    const FILE: &str =
        "Intro <<top>>.\n* Install\n:PROPERTIES:\n:CUSTOM_ID: install\n:ID: ABC-1\n:END:\n\
                        #+NAME: tbl\n| a |\n* Second   Part\nA <<<radio>>> target.\n";

    #[test]
    fn each_kind_of_path_resolves_as_in_ox() {
        let t = targets(FILE);
        let host = Ids(Some(vec!["elsewhere"]));
        let r = |p: &str| t.resolve(p, &host);
        assert_eq!(r("#install"), Resolution::Internal("install".into()));
        assert_eq!(r("*Install"), Resolution::Internal("install".into()));
        assert_eq!(r("id:abc-1"), Resolution::Internal("install".into()));
        assert_eq!(
            r("*second part"),
            Resolution::Internal(t.headline_anchor(8))
        );
        assert_eq!(r("top"), Resolution::Internal(t.target_anchor("top")));
        assert_eq!(r("Radio"), Resolution::Internal(t.target_anchor("radio")));
        assert_eq!(r("tbl"), Resolution::Internal(t.name_anchor("tbl")));
        assert_eq!(r("Second Part"), Resolution::Internal(t.headline_anchor(8)));
        assert_eq!(r("id:elsewhere"), Resolution::External);
        assert_eq!(r("https://example.com"), Resolution::External);
        assert_eq!(r("file:other.org"), Resolution::External);
        assert_eq!(r("./other.org"), Resolution::External);
        assert_eq!(
            r("(ref)"),
            Resolution::Unsupported("code references".into())
        );
        for broken in ["#nope", "*Nope", "nope", "id:nope"] {
            assert_eq!(r(broken), Resolution::Broken, "{broken}");
        }
        assert_eq!(t.resolve("id:nope", &Ids(None)), Resolution::External);
        assert_eq!(
            t.radios(),
            [("radio".to_string(), t.target_anchor("radio"))]
        );
    }

    /// A target the export leaves out is no target.
    #[test]
    fn targets_in_what_is_not_exported_do_not_resolve() {
        let mut t = targets(FILE);
        t.set_live([8].into_iter().collect(), false);
        let host = Ids(None);
        assert_eq!(t.resolve("#install", &host), Resolution::Broken);
        assert_eq!(t.resolve("tbl", &host), Resolution::Broken);
        assert_eq!(t.resolve("top", &host), Resolution::Broken, "the preamble");
        assert_eq!(
            t.resolve("radio", &host),
            Resolution::Internal(t.target_anchor("radio"))
        );
        t.set_live([1].into_iter().collect(), true);
        assert!(t.radios().is_empty(), "a radio left out links nothing");
    }

    #[test]
    fn anchors_are_stable_and_distinct() {
        let t = targets(FILE);
        assert_eq!(t.target_anchor("A  B"), t.target_anchor("a b"));
        assert_ne!(t.target_anchor("x"), t.name_anchor("x"));
        assert_ne!(t.headline_anchor(1), t.headline_anchor(2));
        assert_eq!(t.headline_anchor(1), targets(FILE).headline_anchor(1));
    }
}

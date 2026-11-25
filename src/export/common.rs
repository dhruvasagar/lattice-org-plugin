//! What every backend computes the same way: the outline's numbers and
//! table of contents, what a description-less internal link says, the
//! footnotes in the order they are referenced, and which links are images.
//! The backends differ only in how they write these.

use std::collections::HashMap;

use super::filter::{self, Selection, View};
use super::options::{Depth, Tags};
use super::parse::objects::Object;
use super::parse::{Doc, Element, ElementKind, Headline};

/// One table-of-contents entry.
pub(crate) struct TocEntry {
    /// The headline's level in the export.
    pub(crate) level: usize,
    pub(crate) anchor: String,
    /// `1.2`, for a numbered headline.
    pub(crate) number: Option<String>,
    /// The title, filtered.
    pub(crate) title: Vec<Object>,
    /// The tags, when `tags:t` shows them in the contents too.
    pub(crate) tags: Vec<String>,
}

/// The exported outline: numbers, contents, labels and `:ID:`s, from one
/// pass over the headlines (not their sections) before the walk.
pub(crate) struct Outline {
    /// Added to a headline's level to give its level in the export: a
    /// subtree export's top headline is level 1.
    offset: i64,
    /// Headline line → its section number, for the numbered ones.
    numbers: HashMap<u32, Vec<usize>>,
    /// Anchor → an internal link's text when it has no description: the
    /// number of a numbered headline, else its title.
    labels: HashMap<String, String>,
    /// Headline line → its `:ID:`, taken before the filter, which may drop
    /// the properties.
    org_ids: HashMap<u32, String>,
    pub(crate) toc: Vec<TocEntry>,
}

impl Outline {
    pub(crate) fn new(doc: &Doc, selection: &Selection, view: &mut View) -> Outline {
        let offset = doc
            .headlines
            .iter()
            .map(|h| h.level)
            .min()
            .map_or(0, |min| 1 - min as i64);
        let mut outline = Outline {
            offset,
            numbers: HashMap::new(),
            labels: HashMap::new(),
            org_ids: HashMap::new(),
            toc: Vec::new(),
        };
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
        while let Some((h, inherited)) = stack.pop() {
            if !filter::visits(h, selection, &view.options) {
                continue;
            }
            let unnumbered = match property(h, "UNNUMBERED") {
                Some("notoc") => 2,
                Some("nil") => 0,
                Some(_) => 1,
                None => inherited,
            };
            stack.extend(h.children.iter().rev().map(|c| (c, unnumbered)));
            if let Some(id) = property(h, "ID") {
                outline.org_ids.insert(h.line, id.to_string());
            }
            let level = outline.level(h);
            let anchor = view.targets().headline_anchor(h.line);
            let number = if unnumbered == 0 && level <= numbered_to {
                counters.truncate(level);
                while counters.len() < level {
                    counters.push(0);
                }
                counters[level - 1] += 1;
                outline.numbers.insert(h.line, counters.clone());
                Some(dotted(&counters))
            } else {
                None
            };
            outline.labels.insert(
                anchor.clone(),
                number.clone().unwrap_or_else(|| h.raw_title.clone()),
            );
            if level <= toc_to && unnumbered != 2 && !is_footnote_section(h) {
                outline.toc.push(TocEntry {
                    level,
                    anchor,
                    number,
                    title: filter::objects(&h.title, view),
                    tags: if toc_tags { h.tags.clone() } else { Vec::new() },
                });
            }
        }
        outline
    }

    /// `h`'s level in the export.
    pub(crate) fn level(&self, h: &Headline) -> usize {
        (h.level as i64 + self.offset).max(1) as usize
    }

    /// `h`'s section number, `1.2`, when it is numbered.
    pub(crate) fn number(&self, h: &Headline) -> Option<String> {
        self.numbers.get(&h.line).map(|n| dotted(n))
    }

    /// What a link to `anchor` with no description says.
    pub(crate) fn label(&self, anchor: &str) -> Option<&str> {
        self.labels.get(anchor).map(String::as_str)
    }

    /// `h`'s `:ID:`, whether or not its properties are exported.
    pub(crate) fn org_id(&self, h: &Headline) -> Option<&str> {
        self.org_ids.get(&h.line).map(String::as_str)
    }
}

fn property<'h>(h: &'h Headline, key: &str) -> Option<&'h str> {
    h.properties
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim())
}

/// `1.2.3`.
fn dotted(numbers: &[usize]) -> String {
    numbers
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// The heading org keeps footnote definitions under, which export leaves
/// out, as in ox.
pub(crate) fn is_footnote_section(h: &Headline) -> bool {
    h.raw_title == "Footnotes"
}

/// A footnote's definition, as it is found.
pub(crate) enum Definition {
    Elements(Vec<Element>),
    Inline(Vec<Object>),
}

/// The footnotes, numbered by first reference wherever they are defined.
#[derive(Default)]
pub(crate) struct Footnotes {
    /// Keys in the order first referenced; a key is the label, or a made-up
    /// one for an anonymous inline footnote.
    order: Vec<String>,
    refs: HashMap<String, usize>,
    definitions: HashMap<String, Definition>,
    anonymous: usize,
    /// How many of `order` have been written out.
    written: usize,
}

impl Footnotes {
    /// A reference: its number, and how many times it has been referenced
    /// counting this one.
    pub(crate) fn reference(
        &mut self,
        label: Option<&str>,
        definition: &[Object],
    ) -> (usize, usize) {
        let key = match label {
            Some(l) => l.to_string(),
            None => {
                self.anonymous += 1;
                format!(" anonymous {}", self.anonymous)
            }
        };
        if !definition.is_empty() {
            self.definitions
                .entry(key.clone())
                .or_insert_with(|| Definition::Inline(definition.to_vec()));
        }
        let n = match self.order.iter().position(|k| *k == key) {
            Some(i) => i + 1,
            None => {
                self.order.push(key.clone());
                self.order.len()
            }
        };
        let count = self.refs.entry(key).or_insert(0);
        *count += 1;
        (n, *count)
    }

    /// Keep the definitions among `elements`.
    pub(crate) fn collect(&mut self, elements: &[Element]) {
        for e in elements {
            if let ElementKind::FootnoteDefinition { label, contents } = &e.kind {
                self.definitions
                    .entry(label.clone())
                    .or_insert_with(|| Definition::Elements(contents.clone()));
            }
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The next footnote to write, by number, with its definition (`None`
    /// when it was referenced and never defined). Writing one may reference
    /// another, which is then next in turn.
    pub(crate) fn next(&mut self) -> Option<(usize, Option<Definition>)> {
        let key = self.order.get(self.written)?.clone();
        self.written += 1;
        Some((self.written, self.definitions.remove(&key)))
    }
}

/// Whether `path` names an image a browser shows inline.
pub(crate) fn is_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".bmp", ".avif",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// Whether a link path names a file, as opposed to a URL or something in
/// this document.
pub(crate) fn is_file_path(path: &str) -> bool {
    path.starts_with("file:") || ["/", "./", "../", "~/"].iter().any(|p| path.starts_with(p))
}

/// A link with no description to an image, which the backends inline: the
/// image's path.
pub(crate) fn inline_image<'p>(path: &'p str, description: &[Object]) -> Option<&'p str> {
    let file = path.strip_prefix("file:").unwrap_or(path);
    let web = path.starts_with("http://") || path.starts_with("https://");
    (description.is_empty() && (is_file_path(path) || web) && is_image(file)).then_some(file)
}

/// A paragraph that is one image link and nothing else: its path.
pub(crate) fn standalone_image(objects: &[Object]) -> Option<&str> {
    let mut found = None;
    for o in objects {
        match o {
            Object::Text(t) if t.trim().is_empty() => {}
            Object::Link {
                path,
                description,
                target: None,
            } if found.is_none() => found = Some(inline_image(path, description)?),
            _ => return None,
        }
    }
    found
}

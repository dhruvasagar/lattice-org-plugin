//! What every backend computes the same way: the outline's numbers and
//! table of contents, what a description-less internal link says, the
//! footnotes in the order they are referenced, and which links are images.
//! The backends differ only in how they write these.

use std::collections::{HashMap, HashSet};

use super::filter::{self, Selection, View};
use super::options::{Depth, Tags};
use super::parse::objects::Object;
use super::parse::{Doc, Element, ElementKind, Headline, Row, Table};

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

/// The exported outline: numbers, contents, labels and `:ID:`s, from a pass
/// over the headlines (not their sections) before the body is written. The
/// pass is a walk taken in steps ([`Outline::step`]), as the render walk is:
/// a file of thousands of headlines is not numbered in one call.
#[derive(Default)]
pub(crate) struct Outline {
    /// Added to a headline's level to give its level in the export: a
    /// subtree export's top headline is level 1.
    offset: i64,
    /// Headline line → its section number, for the numbered ones.
    numbers: HashMap<u32, Vec<usize>>,
    /// Anchor → an internal link's text when it has no description: the
    /// number of a numbered headline, else its title.
    labels: HashMap<String, String>,
    /// The anchors of the numbered headlines.
    numbered: HashSet<String>,
    /// Headline line → its `:ID:`, taken before the filter, which may drop
    /// the properties.
    org_ids: HashMap<u32, String>,
    pub(crate) toc: Vec<TocEntry>,
    /// The anchors the contents lists.
    listed: HashSet<String>,
    /// The lowest level in the contents, which its outermost list is at.
    pub(crate) toc_top: usize,
}

/// Where the outline's walk has got to, between steps.
pub(crate) struct OutlineWalk {
    /// The next headline, as [`advance`] moves it; empty at the end.
    next: Vec<usize>,
    /// The UNNUMBERED value each headline on the path to `next` hands its
    /// children: 0 none, 1 `t`, 2 `notoc`.
    unnumbered: Vec<u8>,
    counters: Vec<usize>,
    numbered_to: usize,
    toc_to: usize,
    toc_tags: bool,
}

impl Outline {
    /// An empty outline and the walk that fills it.
    pub(crate) fn begin(doc: &Doc, view: &View) -> (Outline, OutlineWalk) {
        let offset = doc
            .headlines
            .iter()
            .map(|h| h.level)
            .min()
            .map_or(0, |min| 1 - min as i64);
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
        let outline = Outline {
            offset,
            numbers: HashMap::new(),
            labels: HashMap::new(),
            numbered: HashSet::new(),
            org_ids: HashMap::new(),
            toc: Vec::new(),
            listed: HashSet::new(),
            toc_top: usize::MAX,
        };
        let walk = OutlineWalk {
            next: if doc.headlines.is_empty() {
                Vec::new()
            } else {
                vec![0]
            },
            unnumbered: Vec::new(),
            counters: Vec::new(),
            numbered_to,
            toc_to,
            toc_tags: o.tags == Tags::On,
        };
        (outline, walk)
    }

    /// Take the walk on by about `budget` bytes of headline: `true` once
    /// every headline is in.
    pub(crate) fn step(
        &mut self,
        walk: &mut OutlineWalk,
        doc: &Doc,
        selection: &Selection,
        view: &mut View,
        budget: usize,
    ) -> bool {
        let mut spent = 0usize;
        while !walk.next.is_empty() && spent < budget {
            let h = at(doc, &walk.next);
            walk.unnumbered.truncate(walk.next.len() - 1);
            let visit = filter::visits(h, selection, &view.options);
            if visit {
                let inherited = walk.unnumbered.last().copied().unwrap_or(0);
                let unnumbered = match property(h, "UNNUMBERED") {
                    Some("notoc") => 2,
                    Some("nil") => 0,
                    Some(_) => 1,
                    None => inherited,
                };
                walk.unnumbered.push(unnumbered);
                self.add(h, unnumbered, walk, view);
                // A headline costs its title and a little more for the
                // bookkeeping, so a run of empty titles still steps.
                spent += h.raw_title.len() + 64;
            }
            advance(doc, &mut walk.next, visit);
        }
        walk.next.is_empty()
    }

    fn add(&mut self, h: &Headline, unnumbered: u8, walk: &mut OutlineWalk, view: &mut View) {
        if let Some(id) = property(h, "ID") {
            self.org_ids.insert(h.line, id.to_string());
        }
        let level = self.level(h);
        let anchor = view.targets().headline_anchor(h.line);
        let number = if unnumbered == 0 && level <= walk.numbered_to {
            let counters = &mut walk.counters;
            counters.truncate(level);
            while counters.len() < level {
                counters.push(0);
            }
            counters[level - 1] += 1;
            self.numbers.insert(h.line, counters.clone());
            self.numbered.insert(anchor.clone());
            Some(dotted(counters))
        } else {
            None
        };
        self.labels.insert(
            anchor.clone(),
            number.clone().unwrap_or_else(|| h.raw_title.clone()),
        );
        if level <= walk.toc_to && unnumbered != 2 && !is_footnote_section(h) {
            self.toc_top = self.toc_top.min(level);
            self.listed.insert(anchor.clone());
            self.toc.push(TocEntry {
                level,
                anchor,
                number,
                title: filter::objects(&h.title, view),
                tags: if walk.toc_tags {
                    h.tags.clone()
                } else {
                    Vec::new()
                },
            });
        }
    }

    /// `h`'s level in the export.
    pub(crate) fn level(&self, h: &Headline) -> usize {
        (h.level as i64 + self.offset).max(1) as usize
    }

    /// `h`'s section number, `1.2`, when it is numbered.
    pub(crate) fn number(&self, h: &Headline) -> Option<String> {
        self.numbers.get(&h.line).map(|n| dotted(n))
    }

    /// Whether the contents lists `anchor`'s headline.
    pub(crate) fn is_listed(&self, anchor: &str) -> bool {
        self.listed.contains(anchor)
    }

    /// Whether `anchor` is a numbered headline's.
    pub(crate) fn is_numbered(&self, anchor: &str) -> bool {
        self.numbered.contains(anchor)
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
    /// Key → its number and how many times it has been referenced.
    refs: HashMap<String, (usize, usize)>,
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
        let next = self.order.len() + 1;
        let (n, count) = self.refs.entry(key.clone()).or_insert((next, 0));
        if *count == 0 {
            self.order.push(key);
        }
        *count += 1;
        (*n, *count)
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

    /// Footnote `n`'s definition, taken: a backend that writes each one at
    /// its first reference (LaTeX's `\footnote`) rather than at the end.
    pub(crate) fn take(&mut self, n: usize) -> Option<Definition> {
        let key = self.order.get(n.checked_sub(1)?)?;
        self.definitions.remove(key)
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

/// Whether most of a column's non-empty cells are numbers: such a column is
/// right-aligned, as org's `org-table-number-fraction` has it.
pub(crate) fn numeric_column(t: &Table, column: usize) -> bool {
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

/// The headline at `path`: indices from `doc.headlines` down through
/// `children`.
pub(crate) fn at<'d>(doc: &'d Doc, path: &[usize]) -> &'d Headline {
    let mut h = &doc.headlines[path[0]];
    for &i in &path[1..] {
        h = &h.children[i];
    }
    h
}

/// Move `path` to the next headline in document order: the first child when
/// `descend` and there is one, else the next sibling of the nearest ancestor
/// that has one. Empty at the end.
pub(crate) fn advance(doc: &Doc, path: &mut Vec<usize>, descend: bool) {
    if descend && !at(doc, path).children.is_empty() {
        path.push(0);
        return;
    }
    while let Some(i) = path.pop() {
        let siblings = if path.is_empty() {
            doc.headlines.len()
        } else {
            at(doc, path).children.len()
        };
        if i + 1 < siblings {
            path.push(i + 1);
            return;
        }
    }
}

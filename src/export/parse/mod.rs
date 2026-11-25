//! The export parser: org text to a backend-neutral tree, shaped like
//! emacs's org-element (lattice's `org-export.md` §3.1).
//!
//! **Elements, then objects.** The element pass finds sections and
//! headlines, paragraphs, lists, tables, blocks, drawers, keywords, footnote
//! definitions and the rest; the text inside them that holds objects (a
//! paragraph, a headline's title, a table cell, an item's tag, a caption) goes
//! through [`objects`], bounded per piece of text.
//!
//! **A headline ends everything.** Org's grammar lets no element cross a
//! headline: a block whose `#+end_` is below the next headline is not a
//! block. So a file splits at its headline lines into sections that parse
//! independently, which is what lets an export parse a large file a chunk at
//! a time ([`parse_chunk`]) and still build the same tree ([`assemble`]).
//!
//! **Bounded work.** Line-oriented, one pass per element run. The end of a
//! block, drawer or LaTeX environment is found through an index of the
//! section's end markers, so a file full of unterminated `#+begin_`s costs a
//! lookup per line, not a scan to the end of the section per line. Nesting
//! deeper than [`MAX_DEPTH`] is read as text rather than recursed into.

pub(crate) mod dump;
mod element;
pub(crate) mod objects;

use objects::Object;

/// How deep lists, drawers and greater blocks nest before their contents are
/// read as plain text. Org files nest a handful of levels; this bounds the
/// guest's stack against a file that nests thousands.
pub(crate) const MAX_DEPTH: usize = 32;

/// A parsed file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Doc {
    /// What comes before the first headline: the zeroth section.
    pub(crate) preamble: Vec<Element>,
    /// The top-level headlines, each holding its subtree.
    pub(crate) headlines: Vec<Headline>,
}

/// A headline and everything under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Headline {
    /// Its line in the file, 0-based.
    pub(crate) line: u32,
    pub(crate) level: usize,
    /// A TODO keyword, only if it is one of the file's set.
    pub(crate) todo: Option<String>,
    pub(crate) priority: Option<char>,
    pub(crate) title: Vec<Object>,
    /// The title as written: what an internal `[[*Title]]` link matches.
    pub(crate) raw_title: String,
    pub(crate) tags: Vec<String>,
    /// Titled `COMMENT`: the subtree is not exported.
    pub(crate) commented: bool,
    /// The planning line under it, if any.
    pub(crate) planning: Planning,
    /// Its property drawer, in order.
    pub(crate) properties: Vec<(String, String)>,
    /// What is between it and the next headline.
    pub(crate) section: Vec<Element>,
    pub(crate) children: Vec<Headline>,
}

/// A headline's planning line: each timestamp as written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Planning {
    pub(crate) scheduled: Option<String>,
    pub(crate) deadline: Option<String>,
    pub(crate) closed: Option<String>,
}

impl Planning {
    pub(crate) fn is_empty(&self) -> bool {
        self.scheduled.is_none() && self.deadline.is_none() && self.closed.is_none()
    }
}

/// One element, with the affiliated keywords written above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Element {
    /// Its first line in the file, 0-based (its affiliated keywords' first
    /// line when it has any).
    pub(crate) line: u32,
    pub(crate) affiliated: Affiliated,
    pub(crate) kind: ElementKind,
}

/// `#+NAME:`, `#+CAPTION:`, `#+ATTR_<backend>:`, `#+RESULTS:` and `#+HEADER:`
/// above an element, which belong to it rather than standing alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Affiliated {
    pub(crate) name: Option<String>,
    /// One entry per `#+CAPTION:` line, as org concatenates them.
    pub(crate) caption: Vec<Vec<Object>>,
    /// `(backend, value)` per `#+ATTR_<backend>:` line, backend lowercased.
    pub(crate) attr: Vec<(String, String)>,
    /// `#+RESULTS:`'s value, when the element is a block's result.
    pub(crate) results: Option<String>,
    pub(crate) header: Vec<String>,
}

impl Affiliated {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ElementKind {
    /// Lines of text, joined with `\n`.
    Paragraph(Vec<Object>),
    PlainList(List),
    Table(Table),
    /// A `table.el` table (`+---+`), kept as its lines.
    TableEl(Vec<String>),
    Block(Block),
    Drawer {
        name: String,
        contents: Vec<Element>,
    },
    /// `#+KEY: value` standing alone; the key uppercased.
    Keyword {
        key: String,
        value: String,
    },
    FootnoteDefinition {
        label: String,
        contents: Vec<Element>,
    },
    /// `: ` lines, without the marker.
    FixedWidth(Vec<String>),
    /// `# ` lines, without the marker.
    Comment(Vec<String>),
    HorizontalRule,
    /// `\begin{env}` … `\end{env}`, as written.
    LatexEnvironment(String),
    /// A `CLOCK:` line's value.
    Clock(String),
}

/// A `#+begin_X` … `#+end_X` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) kind: BlockKind,
    /// What follows the name on the `#+begin_` line, past the language or
    /// backend.
    pub(crate) parameters: String,
    /// The body as text, for the blocks whose body is not parsed (source,
    /// example, export, verse, comment): common indentation removed, and a
    /// comma escaping `*` or `#+` at a line's start taken off.
    pub(crate) body: String,
    /// The body as elements, for the greater blocks (quote, center, special).
    pub(crate) contents: Vec<Element>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockKind {
    Src {
        language: String,
    },
    Example,
    Export {
        backend: String,
    },
    Verse,
    Comment,
    Quote,
    Center,
    /// Any other name, lowercased: `#+begin_note` is `Special("note")`.
    Special(String),
}

impl BlockKind {
    /// Whether the body is elements rather than text.
    pub(crate) fn is_greater(&self) -> bool {
        matches!(
            self,
            BlockKind::Quote | BlockKind::Center | BlockKind::Special(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct List {
    pub(crate) kind: ListKind,
    pub(crate) items: Vec<Item>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListKind {
    Unordered,
    Ordered,
    /// Unordered items with `tag :: description`.
    Descriptive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) line: u32,
    /// As written: `-`, `+`, `*`, `1.`, `a)`.
    pub(crate) bullet: String,
    /// `[@3]`.
    pub(crate) counter: Option<u32>,
    pub(crate) checkbox: Option<Checkbox>,
    /// A descriptive item's term.
    pub(crate) tag: Option<Vec<Object>>,
    pub(crate) contents: Vec<Element>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Checkbox {
    /// `[X]`.
    On,
    /// `[ ]`.
    Off,
    /// `[-]`.
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Table {
    /// The rows, cookie rows taken out.
    pub(crate) rows: Vec<Row>,
    /// Each column's alignment from a `<l>` / `<c>` / `<r>` cookie.
    pub(crate) align: Vec<Option<Align>>,
    /// `#+TBLFM:` lines under it, which export ignores.
    pub(crate) formulas: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    /// `|---+---|`.
    Rule,
    /// Each cell, trimmed.
    Cells(Vec<Vec<Object>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

/// What a parse needs from outside the text: the TODO keywords a headline's
/// first word is checked against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub(crate) todo_keywords: Vec<String>,
}

impl Context {
    /// The keywords in effect for `lines`: the file's own `#+TODO:` /
    /// `#+SEQ_TODO:` / `#+TYP_TODO:` lines when it has any (they replace the
    /// configured set for that file, as in emacs), else `configured`.
    pub(crate) fn for_file(text: &str, configured: Vec<String>) -> Self {
        let mut own = Vec::new();
        for line in text.lines() {
            let Some((key, value)) = keyword_parts(line) else {
                continue;
            };
            if matches!(key.as_str(), "TODO" | "SEQ_TODO" | "TYP_TODO") {
                own.extend(crate::todo::parse_keywords(value));
            }
        }
        Self {
            todo_keywords: if own.is_empty() { configured } else { own },
        }
    }
}

/// A section as one chunk parses it: a headline's own parts and its section,
/// or the preamble (`headline: None`). [`assemble`] nests them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlatSection {
    pub(crate) headline: Option<Headline>,
    pub(crate) elements: Vec<Element>,
}

/// Parse a whole file.
#[cfg(test)]
pub(crate) fn parse(text: &str, ctx: &Context) -> Doc {
    let lines: Vec<&str> = text.lines().collect();
    assemble(parse_chunk(&lines, 0, ctx))
}

/// Parse `lines`, the file's lines from `first_line` on, which start at a
/// headline or at the top of the file, into its sections in order.
pub(crate) fn parse_chunk(lines: &[&str], first_line: u32, ctx: &Context) -> Vec<FlatSection> {
    let heads: Vec<usize> = (0..lines.len())
        .filter(|&i| crate::headline::headline_level(lines[i]).is_some())
        .collect();
    let mut sections = Vec::new();
    let first = heads.first().copied().unwrap_or(lines.len());
    if first > 0 {
        let preamble = section(&lines[..first], first_line, ctx);
        if !preamble.elements.is_empty() {
            sections.push(preamble);
        }
    }
    for (k, &start) in heads.iter().enumerate() {
        let end = heads.get(k + 1).copied().unwrap_or(lines.len());
        sections.push(section(&lines[start..end], first_line + start as u32, ctx));
    }
    sections
}

/// One section: a headline line and its body, or (when `lines[0]` is not a
/// headline) the preamble.
fn section(lines: &[&str], first_line: u32, ctx: &Context) -> FlatSection {
    let Some(head) = lines.first().and_then(|l| headline_of(l, first_line, ctx)) else {
        return FlatSection {
            headline: None,
            elements: element::parse(lines, first_line),
        };
    };
    let mut head = head;
    let mut body = 1;
    if let Some(planning) = lines.get(body).and_then(|l| planning_of(l)) {
        head.planning = planning;
        body += 1;
    }
    if let Some((properties, used)) = property_drawer(&lines[body..]) {
        head.properties = properties;
        body += used;
    }
    FlatSection {
        headline: Some(head),
        elements: element::parse(&lines[body..], first_line + body as u32),
    }
}

fn headline_of(line: &str, at: u32, ctx: &Context) -> Option<Headline> {
    let h = crate::todo::parse(line, &ctx.todo_keywords)?;
    let (commented, title) = match h.title.strip_prefix("COMMENT") {
        Some(rest) if rest.is_empty() || rest.starts_with(' ') => (true, rest.trim()),
        _ => (false, h.title),
    };
    Some(Headline {
        line: at,
        level: h.stars.len(),
        todo: h.keyword.map(str::to_string),
        priority: h.priority,
        title: objects::parse(title),
        raw_title: title.to_string(),
        tags: h.tags.iter().map(|t| t.to_string()).collect(),
        commented,
        planning: Planning::default(),
        properties: Vec::new(),
        section: Vec::new(),
        children: Vec::new(),
    })
}

/// A planning line: one or more of `SCHEDULED:`, `DEADLINE:`, `CLOSED:`, each
/// followed by its timestamp, and nothing else.
fn planning_of(line: &str) -> Option<Planning> {
    let mut rest = line.trim();
    let mut planning = Planning::default();
    while !rest.is_empty() {
        let (slot, after) = if let Some(r) = rest.strip_prefix("SCHEDULED:") {
            (&mut planning.scheduled, r)
        } else if let Some(r) = rest.strip_prefix("DEADLINE:") {
            (&mut planning.deadline, r)
        } else if let Some(r) = rest.strip_prefix("CLOSED:") {
            (&mut planning.closed, r)
        } else {
            return None;
        };
        let after = after.trim_start();
        let close = match after.chars().next()? {
            '<' => '>',
            '[' => ']',
            _ => return None,
        };
        let end = after.find(close)? + 1;
        // A range, `<a>--<b>`, is one timestamp.
        let end = match after[end..].strip_prefix("--") {
            Some(tail) if tail.starts_with(['<', '[']) => end + 2 + tail.find([close])? + 1,
            _ => end,
        };
        *slot = Some(after[..end].to_string());
        rest = after[end..].trim_start();
    }
    (!planning.is_empty()).then_some(planning)
}

/// A property drawer at the top of `lines`: its properties and how many lines
/// it took. `None` when the first line is not `:PROPERTIES:` or the drawer
/// never closes.
fn property_drawer(lines: &[&str]) -> Option<(Vec<(String, String)>, usize)> {
    if !lines.first()?.trim().eq_ignore_ascii_case(":PROPERTIES:") {
        return None;
    }
    let end = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(":END:"))?;
    let properties = lines[1..end]
        .iter()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix(':')?;
            let (key, value) = rest.split_once(':')?;
            (!key.is_empty() && !key.contains(char::is_whitespace))
                .then(|| (key.to_string(), value.trim().to_string()))
        })
        .collect();
    Some((properties, end + 1))
}

/// `#+KEY: value` as `(KEY uppercased, value trimmed)`.
pub(crate) fn keyword_parts(line: &str) -> Option<(String, &str)> {
    let rest = line.trim_start().strip_prefix("#+")?;
    let (key, value) = rest.split_once(':')?;
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    Some((key.to_ascii_uppercase(), value.trim()))
}

/// Nest `sections`, in file order, into a tree.
pub(crate) fn assemble(sections: Vec<FlatSection>) -> Doc {
    let mut doc = Doc::default();
    // The open headlines, outermost first.
    let mut stack: Vec<Headline> = Vec::new();
    fn close(stack: &mut Vec<Headline>, doc: &mut Doc) {
        if let Some(done) = stack.pop() {
            match stack.last_mut() {
                Some(parent) => parent.children.push(done),
                None => doc.headlines.push(done),
            }
        }
    }
    for s in sections {
        let Some(mut h) = s.headline else {
            doc.preamble.extend(s.elements);
            continue;
        };
        h.section = s.elements;
        while stack.last().is_some_and(|open| open.level >= h.level) {
            close(&mut stack, &mut doc);
        }
        stack.push(h);
    }
    while !stack.is_empty() {
        close(&mut stack, &mut doc);
    }
    doc
}

#[cfg(test)]
mod tests;

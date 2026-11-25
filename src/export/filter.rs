//! What an export leaves out, by its options: ox's pruning, applied as a
//! view during the stepped render walk rather than as a rewrite of the whole
//! tree, so no one call touches more than the step's headlines.
//!
//! - **Headlines.** A `COMMENT` headline and one tagged with an exclude tag
//!   go, with their subtrees. When any headline carries a select tag, only
//!   selected subtrees and their ancestors stay. `#+FILETAGS:` count as every
//!   headline's tags.
//! - **Elements.** Planning lines, clocks, property drawers, drawers,
//!   fixed-width areas, tables, footnote definitions and LaTeX environments
//!   go as `p:`, `c:`, `prop:`, `d:`, `::`, `|:`, `f:` and `tex:` say.
//!   Comments and comment blocks never export.
//! - **Objects.** Timestamps, footnote references, LaTeX fragments and
//!   statistics cookies go as `<:`, `f:`, `tex:` and `stat:` say. What
//!   `^:`, `*:` and `e:` turn off is put back as the text it was written as:
//!   the parse reads every script, emphasis and entity, because the
//!   `#+OPTIONS:` line that governs them may come after them.
//! - **Macros** expand here too, as the walk reaches them (see
//!   [`super::macros`]), so a [`View`] carries the macro state with the
//!   options.
//! - **Links** are resolved here (see [`super::targets`]): an internal one
//!   gets its anchor, a broken one fails the export or is dropped or marked
//!   as `broken-links:` says, and text naming a radio target becomes a link
//!   to it.

use std::collections::HashSet;
use std::rc::Rc;

use super::macros::{Expansion, Macros};
use super::options::{BrokenLinks, Options, SubSuperscripts, Tex, Timestamps};
use super::parse::objects::{self as parse_objects, Emphasis, Object};
use super::targets::{Resolution, Targets};
use super::Host;
use crate::time_format::When;

/// How deep a macro may expand into macros before it is taken to expand
/// into itself.
const MAX_MACRO_DEPTH: usize = 16;

/// The export as the render sees it: the options, and the macro state that
/// changes as the walk goes (`n`'s counters, the headline whose properties
/// `property` reads).
pub(crate) struct View {
    pub(crate) options: Options,
    macros: Macros,
    /// The properties of the headline being rendered.
    properties: Vec<(String, String)>,
    depth: usize,
    error: Option<String>,
    targets: Targets,
    /// The live radio targets, `(normalised text, anchor)`.
    radios: Vec<(String, String)>,
    /// Inside a link's description or a radio target, where text is not
    /// made a radio link.
    in_link: bool,
    host: Rc<dyn Host>,
}

impl View {
    /// A view of `options`, for a file whose keywords are `keywords`,
    /// exported at `now`.
    ///
    /// `targets` must know what is exported (`Targets::set_live`) before the
    /// view is made: the radio targets are taken from it here.
    pub(crate) fn new(
        options: Options,
        keywords: Vec<(String, String)>,
        now: When,
        targets: Targets,
        host: Rc<dyn Host>,
    ) -> Self {
        let macros = Macros::new(options.macros.clone(), keywords, now);
        let radios = targets.radios();
        View {
            options,
            macros,
            properties: Vec::new(),
            depth: 0,
            error: None,
            targets,
            radios,
            in_link: false,
            host,
        }
    }

    /// The link targets, for the anchors a backend writes.
    pub(crate) fn targets(&self) -> &Targets {
        &self.targets
    }

    /// What the export asks of the editor, for the backends' own questions
    /// (which file an id lives in).
    pub(crate) fn host(&self) -> &dyn Host {
        &*self.host
    }

    /// Why the export cannot go on: an undefined macro, or one that expands
    /// into itself. The first such, once set.
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Stop the export for `why`, unless something already has: the first
    /// reason is the one the user is told.
    pub(crate) fn fail(&mut self, why: String) {
        self.error.get_or_insert(why);
    }
}
use super::parse::{Block, BlockKind, Doc, Element, ElementKind, Headline, Item, List, Row, Table};

/// Which headlines a select tag keeps, by line; `None` when no headline
/// carries one, so every headline the other rules allow is kept.
pub(crate) struct Selection {
    kept: Option<HashSet<u32>>,
}

/// Walk `doc` for select tags. One pass over the headlines (not their
/// sections), iterative so a deep outline costs no stack.
pub(crate) fn selection(doc: &Doc, o: &Options) -> Selection {
    if o.file_tags.iter().any(|t| o.select_tags.contains(t)) {
        return Selection { kept: None };
    }
    let mut kept: HashSet<u32> = HashSet::new();
    // Each entry: a headline and the lines of its ancestors.
    let mut stack: Vec<(&Headline, Vec<u32>)> = doc
        .headlines
        .iter()
        .rev()
        .map(|h| (h, Vec::new()))
        .collect();
    while let Some((h, ancestors)) = stack.pop() {
        if kept.contains(&h.line) {
            continue;
        }
        if h.tags.iter().any(|t| o.select_tags.contains(t)) {
            kept.extend(ancestors.iter().copied());
            // The whole subtree, which inherits the tag.
            let mut below = vec![h];
            while let Some(d) = below.pop() {
                kept.insert(d.line);
                below.extend(d.children.iter());
            }
            continue;
        }
        let mut path = ancestors;
        path.push(h.line);
        stack.extend(h.children.iter().rev().map(|c| (c, path.clone())));
    }
    Selection {
        kept: (!kept.is_empty()).then_some(kept),
    }
}

/// Whether the walk renders `h` and goes into its subtree.
pub(crate) fn visits(h: &Headline, selection: &Selection, o: &Options) -> bool {
    let excluded = |t: &String| o.exclude_tags.contains(t);
    !h.commented
        && !h.tags.iter().any(excluded)
        && !o.file_tags.iter().any(excluded)
        && selection.kept.as_ref().is_none_or(|k| k.contains(&h.line))
}

/// The lines of every headline the walk renders: those it visits, under
/// ancestors it visits.
pub(crate) fn live(doc: &Doc, selection: &Selection, o: &Options) -> HashSet<u32> {
    let mut live = HashSet::new();
    let mut stack: Vec<&Headline> = doc.headlines.iter().collect();
    while let Some(h) = stack.pop() {
        if visits(h, selection, o) {
            live.insert(h.line);
            stack.extend(h.children.iter());
        }
    }
    live
}

/// `h`'s own parts as the export shows them: no children (the walk visits
/// those), and the planning, properties and section filtered.
pub(crate) fn own(h: &Headline, v: &mut View) -> Headline {
    v.properties.clone_from(&h.properties);
    let o = &v.options;
    let planning = if o.planning {
        h.planning.clone()
    } else {
        Default::default()
    };
    let properties = if o.properties {
        h.properties.clone()
    } else {
        Vec::new()
    };
    Headline {
        line: h.line,
        level: h.level,
        todo: h.todo.clone(),
        priority: h.priority,
        title: objects(&h.title, v),
        raw_title: h.raw_title.clone(),
        tags: h.tags.clone(),
        commented: h.commented,
        planning,
        properties,
        section: elements(&h.section, v),
        children: Vec::new(),
    }
}

/// `list` with what the options leave out taken out, inside nested
/// elements too.
pub(crate) fn elements(list: &[Element], v: &mut View) -> Vec<Element> {
    list.iter().filter_map(|e| element(e, v)).collect()
}

fn element(e: &Element, v: &mut View) -> Option<Element> {
    let o = &v.options;
    let dropped = match &e.kind {
        ElementKind::Comment(_) => true,
        ElementKind::Clock(_) if !o.clocks => true,
        ElementKind::FixedWidth(_) if !o.fixed_width => true,
        ElementKind::Table(_) | ElementKind::TableEl(_) if !o.tables => true,
        ElementKind::FootnoteDefinition { .. } if !o.footnotes => true,
        ElementKind::LatexEnvironment(_) if o.tex == Tex::Off => true,
        ElementKind::Drawer { name, .. } if !o.drawers.keeps(name) => true,
        ElementKind::Block(Block {
            kind: BlockKind::Comment,
            ..
        }) => true,
        _ => false,
    };
    if dropped {
        return None;
    }
    let kind = match &e.kind {
        ElementKind::Paragraph(objs) => ElementKind::Paragraph(objects(objs, v)),
        ElementKind::PlainList(list) => ElementKind::PlainList(List {
            kind: list.kind,
            items: list
                .items
                .iter()
                .map(|item| Item {
                    line: item.line,
                    bullet: item.bullet.clone(),
                    counter: item.counter,
                    checkbox: item.checkbox,
                    tag: item.tag.as_ref().map(|t| objects(t, v)),
                    contents: elements(&item.contents, v),
                })
                .collect(),
        }),
        ElementKind::Table(t) => ElementKind::Table(Table {
            rows: t
                .rows
                .iter()
                .map(|r| match r {
                    Row::Rule => Row::Rule,
                    Row::Cells(cells) => Row::Cells(cells.iter().map(|c| objects(c, v)).collect()),
                })
                .collect(),
            align: t.align.clone(),
            formulas: t.formulas.clone(),
        }),
        ElementKind::Block(b) => ElementKind::Block(Block {
            kind: b.kind.clone(),
            parameters: b.parameters.clone(),
            body: b.body.clone(),
            contents: elements(&b.contents, v),
        }),
        ElementKind::Drawer { name, contents } => ElementKind::Drawer {
            name: name.clone(),
            contents: elements(contents, v),
        },
        ElementKind::FootnoteDefinition { label, contents } => ElementKind::FootnoteDefinition {
            label: label.clone(),
            contents: elements(contents, v),
        },
        other => other.clone(),
    };
    let mut affiliated = e.affiliated.clone();
    affiliated.caption = affiliated.caption.iter().map(|c| objects(c, v)).collect();
    Some(Element {
        line: e.line,
        affiliated,
        kind,
    })
}

/// `list` with what the options leave out taken out, and what they turn off
/// put back as text. Adjacent text is joined.
pub(crate) fn objects(list: &[Object], v: &mut View) -> Vec<Object> {
    let mut out = Vec::new();
    for obj in list {
        object(obj, v, &mut out);
    }
    out
}

fn push_text(out: &mut Vec<Object>, text: &str) {
    if let Some(Object::Text(last)) = out.last_mut() {
        last.push_str(text);
    } else if !text.is_empty() {
        out.push(Object::Text(text.to_string()));
    }
}

fn object(obj: &Object, v: &mut View, out: &mut Vec<Object>) {
    let o = &v.options;
    let subsup = o.subsup;
    let keep_script = |braced: bool| match subsup {
        SubSuperscripts::All => true,
        SubSuperscripts::Braced => braced,
        SubSuperscripts::None => false,
    };
    match obj {
        Object::Text(t) if !v.in_link && !v.radios.is_empty() => radio_links(t, v, out),
        Object::Text(t) => push_text(out, t),
        Object::Timestamp(t) => {
            let active = t.starts_with('<');
            let keep = match o.timestamps {
                Timestamps::All => true,
                Timestamps::None => false,
                Timestamps::Active => active,
                Timestamps::Inactive => !active,
            };
            if keep {
                out.push(obj.clone());
            }
        }
        Object::FootnoteReference { .. } if !o.footnotes => {}
        Object::LatexFragment(_) if o.tex == Tex::Off => {}
        Object::StatisticsCookie(_) if !o.statistics_cookies => {}
        Object::Emphasis { kind, contents } if !o.emphasis => {
            let marker = match kind {
                Emphasis::Bold => "*",
                Emphasis::Italic => "/",
                Emphasis::Underline => "_",
                Emphasis::Strike => "+",
            };
            push_text(out, marker);
            for c in contents {
                object(c, v, out);
            }
            push_text(out, marker);
        }
        Object::Entity { name, braces, .. } if !o.entities => {
            push_text(out, &format!("\\{name}{}", if *braces { "{}" } else { "" }));
        }
        Object::Subscript { braced, contents } | Object::Superscript { braced, contents }
            if !keep_script(*braced) =>
        {
            let mark = if matches!(obj, Object::Subscript { .. }) {
                "_"
            } else {
                "^"
            };
            push_text(out, mark);
            if *braced {
                push_text(out, "{");
            }
            for c in contents {
                object(c, v, out);
            }
            if *braced {
                push_text(out, "}");
            }
        }
        Object::Emphasis { kind, contents } => out.push(Object::Emphasis {
            kind: *kind,
            contents: objects(contents, v),
        }),
        Object::Link {
            path, description, ..
        } => link(path, description, v, out),
        Object::FootnoteReference { label, definition } => out.push(Object::FootnoteReference {
            label: label.clone(),
            definition: objects(definition, v),
        }),
        Object::Subscript { braced, contents } => out.push(Object::Subscript {
            braced: *braced,
            contents: objects(contents, v),
        }),
        Object::Superscript { braced, contents } => out.push(Object::Superscript {
            braced: *braced,
            contents: objects(contents, v),
        }),
        Object::RadioTarget(contents) => {
            let outer = std::mem::replace(&mut v.in_link, true);
            let contents = objects(contents, v);
            v.in_link = outer;
            out.push(Object::RadioTarget(contents));
        }
        Object::Macro { name, args } => expand(name, args, v, out),
        other => out.push(other.clone()),
    }
}

/// A link, resolved: an internal one carries its anchor, an external one
/// none, and a broken one goes as `broken-links:` says.
fn link(path: &str, description: &[Object], v: &mut View, out: &mut Vec<Object>) {
    let target = match v.targets.resolve(path, &*v.host) {
        Resolution::Internal(anchor) => Some(anchor),
        Resolution::External => None,
        Resolution::Unsupported(what) => {
            v.options.ignore(what);
            None
        }
        Resolution::Broken => {
            match v.options.broken_links {
                BrokenLinks::Error => {
                    if v.error.is_none() {
                        v.error = Some(format!(
                            "org: [[{path}]] links to nothing in the export \
                             (#+OPTIONS: broken-links:mark exports it marked)"
                        ));
                    }
                }
                // ox drops it, description and all.
                BrokenLinks::Ignore => {}
                BrokenLinks::Mark => push_text(out, &format!("[BROKEN LINK: {path}]")),
            }
            return;
        }
    };
    let outer = std::mem::replace(&mut v.in_link, true);
    let description = objects(description, v);
    v.in_link = outer;
    out.push(Object::Link {
        path: path.to_string(),
        description,
        target,
    });
}

/// `text`, with each occurrence of a radio target's text (whole words,
/// ASCII case aside) made a link to it.
fn radio_links(text: &str, v: &mut View, out: &mut Vec<Object>) {
    let lower = text.to_ascii_lowercase();
    let bounded = |at: usize, len: usize| {
        let before = lower[..at].chars().next_back();
        let after = lower[at + len..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    };
    let mut from = 0;
    loop {
        // The earliest match of any radio target from `from`, the longest
        // on a tie.
        let mut best: Option<(usize, usize, String)> = None;
        for (name, anchor) in &v.radios {
            let mut search = from;
            while let Some(i) = lower[search..].find(name.as_str()) {
                let at = search + i;
                if bounded(at, name.len()) {
                    if best
                        .as_ref()
                        .is_none_or(|(b, l, _)| at < *b || (at == *b && name.len() > *l))
                    {
                        best = Some((at, name.len(), anchor.clone()));
                    }
                    break;
                }
                search = at + name.len().max(1);
            }
        }
        let Some((at, len, anchor)) = best else {
            push_text(out, &text[from..]);
            return;
        };
        push_text(out, &text[from..at]);
        out.push(Object::Link {
            path: text[at..at + len].to_string(),
            description: vec![Object::Text(text[at..at + len].to_string())],
            target: Some(anchor),
        });
        from = at + len;
    }
}

/// `{{{name(args)}}}`, expanded and parsed in place, with the options
/// applied to what it expands to.
fn expand(name: &str, args: &[String], v: &mut View, out: &mut Vec<Object>) {
    if v.error.is_some() {
        return;
    }
    if v.depth >= MAX_MACRO_DEPTH {
        v.error = Some(format!("org: {{{{{{{name}}}}}}} expands into itself"));
        return;
    }
    let properties = std::mem::take(&mut v.properties);
    let expansion = v.macros.expand(name, args, &properties);
    v.properties = properties;
    match expansion {
        Ok(Expansion::Text(text)) => {
            v.depth += 1;
            for obj in &parse_objects::parse(&text) {
                object(obj, v, out);
            }
            v.depth -= 1;
        }
        Ok(Expansion::Unsupported(what)) => v.options.ignore(what),
        Err(why) => v.error = Some(why),
    }
}

/// `list`'s text without its markup: what a `<title>` or a PDF's metadata
/// shows.
pub(crate) fn plain(list: &[Object]) -> String {
    let mut out = String::new();
    fn walk(list: &[Object], out: &mut String) {
        for o in list {
            match o {
                Object::Text(t) | Object::Verbatim(t) | Object::Code(t) | Object::Target(t) => {
                    out.push_str(t)
                }
                Object::Entity { utf8, .. } => out.push_str(utf8),
                Object::Emphasis { contents, .. }
                | Object::Subscript { contents, .. }
                | Object::Superscript { contents, .. }
                | Object::RadioTarget(contents) => walk(contents, out),
                Object::Link {
                    path, description, ..
                } => {
                    if description.is_empty() {
                        out.push_str(path);
                    } else {
                        walk(description, out);
                    }
                }
                Object::LineBreak => out.push(' '),
                Object::LatexFragment(t) | Object::Timestamp(t) => out.push_str(t),
                _ => {}
            }
        }
    }
    walk(list, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::super::parse::{self, dump, Context};
    use super::*;

    fn opts(options: &str) -> Options {
        Options::resolve(&[("OPTIONS".to_string(), options.to_string())], None, "x")
    }

    fn filtered(text: &str, o: &Options) -> String {
        let doc = parse::parse(text, &Context::for_file(text, Vec::new()));
        let sel = selection(&doc, o);
        let mut v = view(o.clone());
        let mut out = String::new();
        let preamble = elements(&doc.preamble, &mut v);
        dump::preamble(&mut out, &preamble);
        fn walk(out: &mut String, hs: &[Headline], depth: usize, sel: &Selection, v: &mut View) {
            for h in hs {
                if visits(h, sel, &v.options) {
                    let own = own(h, v);
                    dump::headline(out, &own, depth);
                    walk(out, &h.children, depth + 1, sel, v);
                }
            }
        }
        walk(&mut out, &doc.headlines, 0, &sel, &mut v);
        out
    }

    fn view(o: Options) -> View {
        View::new(
            o,
            Vec::new(),
            When { local_secs: 0 },
            Targets::default(),
            Rc::new(super::super::targets::tests::Ids(None)),
        )
    }

    fn objs(text: &str, options: &str) -> String {
        dump::objects(&objects(
            &parse::objects::parse(text),
            &mut view(opts(options)),
        ))
    }

    /// `text` in a file whose keywords are `keywords`, its macros expanded.
    fn expanded(text: &str, keywords: &[(&str, &str)]) -> (String, View) {
        let keywords: Vec<(String, String)> = keywords
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut v = view(Options::resolve(&keywords, None, "notes.org"));
        let got = dump::objects(&objects(&parse::objects::parse(text), &mut v));
        (got, v)
    }

    /// A macro's expansion is parsed, may hold macros, and is filtered like
    /// the text around it.
    #[test]
    fn macros_expand_in_place_and_their_expansion_is_markup() {
        let keywords = [
            ("MACRO", "strong *$1*"),
            ("MACRO", "twice {{{strong($1)}}} {{{strong($1)}}}"),
            ("OPTIONS", "*:nil"),
            ("TITLE", "Plan"),
        ];
        let (got, v) = expanded(
            "a {{{twice(x)}}} in {{{input-file}}}, {{{title}}}",
            &keywords,
        );
        assert_eq!(got, r#""a *x* *x* in notes.org, Plan""#);
        assert!(v.error().is_none());
        let (got, _) = expanded("{{{strong(y)}}}", &[("MACRO", "strong *$1*")]);
        assert_eq!(got, r#"bold["y"]"#);
    }

    #[test]
    fn a_property_macro_reads_the_headline_being_rendered() {
        let text =
            "#+MACRO: x unused\n* H\n:PROPERTIES:\n:OWNER: Ada\n:END:\nBy {{{property(OWNER)}}}.\n";
        assert!(filtered(text, &Options::default()).contains(r#""By Ada.""#));
    }

    #[test]
    fn a_macro_that_is_undefined_or_expands_into_itself_stops_the_export() {
        let (_, v) = expanded("{{{nope}}}", &[]);
        assert_eq!(v.error(), Some("org: {{{nope}}} is not defined"));
        let (_, v) = expanded("{{{me}}}", &[("MACRO", "me again {{{me}}}")]);
        assert_eq!(v.error(), Some("org: {{{me}}} expands into itself"));
        let (_, v) = expanded("{{{modification-time(%Y)}}}", &[]);
        assert_eq!(v.options.ignored, ["{{{modification-time}}}"]);
    }

    #[test]
    fn comment_and_noexport_subtrees_go() {
        let text = "* Keep\n** COMMENT Gone\n*** Gone too\n* Out :noexport:\n** Child\n* Also\n";
        let got = filtered(text, &Options::default());
        assert_eq!(got, "headline 1 @0 [\"Keep\"]\nheadline 1 @5 [\"Also\"]\n");
    }

    /// With a select tag anywhere, only selected subtrees and their
    /// ancestors stay; without one, select tags change nothing.
    #[test]
    fn select_tags_keep_their_subtrees_and_ancestors() {
        let text = "* A\n** B :export:\n*** C\n** D\n* E\n";
        let got = filtered(text, &Options::default());
        assert_eq!(
            got,
            "headline 1 @0 [\"A\"]\n  headline 2 @1 [\"B\"] :export:\n    headline 3 @2 [\"C\"]\n"
        );
        let none = "* A\n** D\n";
        assert_eq!(filtered(none, &Options::default()).lines().count(), 2);
    }

    #[test]
    fn file_tags_are_every_headlines() {
        let o = Options::resolve(
            &[("FILETAGS".to_string(), ":noexport:".to_string())],
            None,
            "x",
        );
        assert_eq!(filtered("* A\n* B\n", &o), "");
    }

    #[test]
    fn elements_go_as_the_options_say() {
        let text = "* H\nSCHEDULED: <2026-01-05 Mon>\n:PROPERTIES:\n:ID: x\n:END:\n\
                    :LOGBOOK:\nCLOCK: [2026-01-01]\n:END:\n:NOTES:\nkept\n:END:\n\
                    CLOCK: [2026-01-02]\n# comment\n#+begin_comment\nc\n#+end_comment\n\
                    : fixed\n| t |\n[fn:1] def\n";
        let defaults = filtered(text, &Options::default());
        assert_eq!(
            defaults,
            "headline 1 @0 [\"H\"]\n  drawer NOTES @8\n    paragraph @9 \"kept\"\n  \
             fixed-width @16 [\"fixed\"]\n  table @17\n    row [\"t\"]\n  footnote 1 @18\n    \
             paragraph @18 \"def\"\n"
        );
        let all = filtered(text, &opts("p:t c:t prop:t d:t"));
        for want in ["scheduled", "property ID", "drawer LOGBOOK", "clock @11"] {
            assert!(all.contains(want), "{want}: {all}");
        }
        let none = filtered(text, &opts("::nil |:nil f:nil d:nil"));
        assert_eq!(none, "headline 1 @0 [\"H\"]\n");
    }

    #[test]
    fn objects_go_or_revert_as_the_options_say() {
        let text = "a_b c_{d} *e* \\alpha{} <2026-01-05 Mon> [2026-01-06 Tue] [fn:1] $x$ [1/2]";
        assert_eq!(objs(text, ""), dump::objects(&parse::objects::parse(text)));
        assert_eq!(
            objs(text, "^:{}"),
            r#""a_b c" sub{}["d"] " " bold["e"] " " entity(alpha{}) " " timestamp("<2026-01-05 Mon>") " " timestamp("[2026-01-06 Tue]") " " fn("1") " " latex("$x$") " " cookie("[1/2]")"#
        );
        assert_eq!(
            objs(text, "^:nil *:nil e:nil <:active f:nil tex:nil stat:nil"),
            r#""a_b c_{d} *e* \\alpha{} " timestamp("<2026-01-05 Mon>") "    ""#
        );
    }

    /// `text` exported with its own targets: the filtered dump, links
    /// resolved, or the error that stops it.
    fn linked(text: &str) -> Result<String, String> {
        let lines: Vec<&str> = text.lines().collect();
        let sections = parse::parse_chunk(&lines, 0, &Context::for_file(text, Vec::new()));
        let mut targets = Targets::default();
        let mut keywords = Vec::new();
        for section in &sections {
            targets.collect(section);
            for e in &section.elements {
                if let ElementKind::Keyword { key, value } = &e.kind {
                    keywords.push((key.clone(), value.clone()));
                }
            }
        }
        let doc = parse::assemble(sections);
        let o = Options::resolve(&keywords, None, "x.org");
        let sel = selection(&doc, &o);
        targets.set_live(live(&doc, &sel, &o), true);
        let mut v = View::new(
            o,
            keywords,
            When { local_secs: 0 },
            targets,
            Rc::new(super::super::targets::tests::Ids(None)),
        );
        let mut out = String::new();
        let mut stack: Vec<&Headline> = doc.headlines.iter().rev().collect();
        while let Some(h) = stack.pop() {
            if visits(h, &sel, &v.options) {
                let own = own(h, &mut v);
                dump::headline(&mut out, &own, 0);
                stack.extend(h.children.iter().rev());
            }
        }
        match v.error() {
            Some(why) => Err(why.to_string()),
            None => Ok(out),
        }
    }

    #[test]
    fn links_resolve_and_broken_ones_go_as_the_option_says() {
        let file = "* A\n:PROPERTIES:\n:CUSTOM_ID: a\n:END:\nSee [[#a][here]] and [[nowhere]].\n";
        let err = linked(file).err().unwrap_or_default();
        assert!(
            err.contains("[[nowhere]]") && err.contains("broken-links:mark"),
            "{err}"
        );
        let marked = linked(&format!("#+OPTIONS: broken-links:mark\n{file}")).unwrap_or_else(|e| e);
        assert!(
            marked.contains(r##""See " link("#a" -> a)["here"] " and [BROKEN LINK: nowhere].""##),
            "{marked}"
        );
        let dropped = linked(&format!("#+OPTIONS: broken-links:t\n{file}")).unwrap_or_else(|e| e);
        assert!(dropped.contains(r#"["here"] " and .""#), "{dropped}");
    }

    /// A target in a subtree the export leaves out is no target.
    #[test]
    fn a_link_into_what_is_left_out_is_broken() {
        let err = linked("* A :noexport:\n<<t>>\n* B\n[[t]]\n").err();
        assert!(err.is_some_and(|e| e.contains("[[t]]")));
        assert!(linked("* A\n<<t>>\n* B\n[[t]]\n").is_ok_and(|o| o.contains("link(\"t\" -> org")));
    }

    /// Text naming a radio target, as a whole word, links to it; text in a
    /// link or in the radio target itself does not.
    #[test]
    fn radio_targets_link_their_text() {
        let got = linked("* A\nThe <<<Lattice>>> editor.\n* B\nUse LATTICE, not lattices, [[https://x][lattice]].\n")
            .unwrap_or_else(|e| e);
        let radio = r#"link("LATTICE" -> org"#;
        assert_eq!(got.matches(radio).count(), 1, "{got}");
        assert!(
            got.contains(r#"", not lattices, " link("https://x")["lattice"]"#),
            "{got}"
        );
        assert!(got.contains(r#"radio["Lattice"]"#), "{got}");
    }

    #[test]
    fn plain_text_drops_the_markup() {
        let o = parse::objects::parse("The *bold* [[x][link]] \\alpha =v=");
        assert_eq!(plain(&o), "The bold link α v");
    }
}

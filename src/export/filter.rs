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

use std::collections::HashSet;

use super::options::{Options, SubSuperscripts, Tex, Timestamps};
use super::parse::objects::{Emphasis, Object};
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

/// `h`'s own parts as the export shows them: no children (the walk visits
/// those), and the planning, properties and section filtered.
pub(crate) fn own(h: &Headline, o: &Options) -> Headline {
    Headline {
        line: h.line,
        level: h.level,
        todo: h.todo.clone(),
        priority: h.priority,
        title: objects(&h.title, o),
        raw_title: h.raw_title.clone(),
        tags: h.tags.clone(),
        commented: h.commented,
        planning: if o.planning {
            h.planning.clone()
        } else {
            Default::default()
        },
        properties: if o.properties {
            h.properties.clone()
        } else {
            Vec::new()
        },
        section: elements(&h.section, o),
        children: Vec::new(),
    }
}

/// `list` with what the options leave out taken out, inside nested
/// elements too.
pub(crate) fn elements(list: &[Element], o: &Options) -> Vec<Element> {
    list.iter().filter_map(|e| element(e, o)).collect()
}

fn element(e: &Element, o: &Options) -> Option<Element> {
    let kind = match &e.kind {
        ElementKind::Comment(_) => return None,
        ElementKind::Clock(_) if !o.clocks => return None,
        ElementKind::FixedWidth(_) if !o.fixed_width => return None,
        ElementKind::Table(_) | ElementKind::TableEl(_) if !o.tables => return None,
        ElementKind::FootnoteDefinition { .. } if !o.footnotes => return None,
        ElementKind::LatexEnvironment(_) if o.tex == Tex::Off => return None,
        ElementKind::Drawer { name, .. } if !o.drawers.keeps(name) => return None,
        ElementKind::Block(Block {
            kind: BlockKind::Comment,
            ..
        }) => return None,
        ElementKind::Paragraph(objs) => ElementKind::Paragraph(objects(objs, o)),
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
                    tag: item.tag.as_ref().map(|t| objects(t, o)),
                    contents: elements(&item.contents, o),
                })
                .collect(),
        }),
        ElementKind::Table(t) => ElementKind::Table(Table {
            rows: t
                .rows
                .iter()
                .map(|r| match r {
                    Row::Rule => Row::Rule,
                    Row::Cells(cells) => Row::Cells(cells.iter().map(|c| objects(c, o)).collect()),
                })
                .collect(),
            align: t.align.clone(),
            formulas: t.formulas.clone(),
        }),
        ElementKind::Block(b) => ElementKind::Block(Block {
            kind: b.kind.clone(),
            parameters: b.parameters.clone(),
            body: b.body.clone(),
            contents: elements(&b.contents, o),
        }),
        ElementKind::Drawer { name, contents } => ElementKind::Drawer {
            name: name.clone(),
            contents: elements(contents, o),
        },
        ElementKind::FootnoteDefinition { label, contents } => ElementKind::FootnoteDefinition {
            label: label.clone(),
            contents: elements(contents, o),
        },
        other => other.clone(),
    };
    let mut affiliated = e.affiliated.clone();
    affiliated.caption = affiliated.caption.iter().map(|c| objects(c, o)).collect();
    Some(Element {
        line: e.line,
        affiliated,
        kind,
    })
}

/// `list` with what the options leave out taken out, and what they turn off
/// put back as text. Adjacent text is joined.
pub(crate) fn objects(list: &[Object], o: &Options) -> Vec<Object> {
    let mut out = Vec::new();
    for obj in list {
        object(obj, o, &mut out);
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

fn object(obj: &Object, o: &Options, out: &mut Vec<Object>) {
    let keep_script = |braced: bool| match o.subsup {
        SubSuperscripts::All => true,
        SubSuperscripts::Braced => braced,
        SubSuperscripts::None => false,
    };
    match obj {
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
                object(c, o, out);
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
                object(c, o, out);
            }
            if *braced {
                push_text(out, "}");
            }
        }
        Object::Emphasis { kind, contents } => out.push(Object::Emphasis {
            kind: *kind,
            contents: objects(contents, o),
        }),
        Object::Link { path, description } => out.push(Object::Link {
            path: path.clone(),
            description: objects(description, o),
        }),
        Object::FootnoteReference { label, definition } => out.push(Object::FootnoteReference {
            label: label.clone(),
            definition: objects(definition, o),
        }),
        Object::Subscript { braced, contents } => out.push(Object::Subscript {
            braced: *braced,
            contents: objects(contents, o),
        }),
        Object::Superscript { braced, contents } => out.push(Object::Superscript {
            braced: *braced,
            contents: objects(contents, o),
        }),
        Object::RadioTarget(contents) => out.push(Object::RadioTarget(objects(contents, o))),
        other => out.push(other.clone()),
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
                Object::Link { path, description } => {
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
        let mut out = String::new();
        dump::preamble(&mut out, &elements(&doc.preamble, o));
        fn walk(out: &mut String, hs: &[Headline], depth: usize, sel: &Selection, o: &Options) {
            for h in hs {
                if visits(h, sel, o) {
                    dump::headline(out, &own(h, o), depth);
                    walk(out, &h.children, depth + 1, sel, o);
                }
            }
        }
        walk(&mut out, &doc.headlines, 0, &sel, o);
        out
    }

    fn objs(text: &str, options: &str) -> String {
        dump::objects(&objects(&parse::objects::parse(text), &opts(options)))
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

    #[test]
    fn plain_text_drops_the_markup() {
        let o = parse::objects::parse("The *bold* [[x][link]] \\alpha =v=");
        assert_eq!(plain(&o), "The bold link α v");
    }
}

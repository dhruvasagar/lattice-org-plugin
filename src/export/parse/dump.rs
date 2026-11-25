//! A readable dump of a [`Doc`](super::Doc): one node per line, indented by depth, `@N`
//! its file line, strings in Rust's quoting.
//!
//! What the parser's and the filter's golden tests compare.

use std::fmt::Write;

use super::objects::{Emphasis, Object};
use super::{Affiliated, Block, BlockKind, Element, ElementKind, Headline, Row};

/// The whole tree.
#[cfg(test)]
pub(crate) fn dump(doc: &super::Doc) -> String {
    let mut out = String::new();
    preamble(&mut out, &doc.preamble);
    fn subtree(out: &mut String, h: &Headline, depth: usize) {
        headline(out, h, depth);
        for c in &h.children {
            subtree(out, c, depth + 1);
        }
    }
    for h in &doc.headlines {
        subtree(&mut out, h, 0);
    }
    out
}

/// The zeroth section, when it has anything.
pub(crate) fn preamble(out: &mut String, elements: &[Element]) {
    if !elements.is_empty() {
        out.push_str("preamble\n");
        self::elements(out, elements, 1);
    }
}

fn pad(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// A headline's own parts, at `depth` in the outline: its line, planning,
/// properties and section, not its children.
pub(crate) fn headline(out: &mut String, h: &Headline, depth: usize) {
    pad(out, depth);
    let _ = write!(out, "headline {} @{}", h.level, h.line);
    if let Some(t) = &h.todo {
        let _ = write!(out, " {t}");
    }
    if let Some(p) = h.priority {
        let _ = write!(out, " [#{p}]");
    }
    if h.commented {
        out.push_str(" COMMENT");
    }
    let _ = write!(out, " [{}]", objects(&h.title));
    if !h.tags.is_empty() {
        let _ = write!(out, " :{}:", h.tags.join(":"));
    }
    out.push('\n');
    let p = &h.planning;
    for (name, value) in [
        ("scheduled", &p.scheduled),
        ("deadline", &p.deadline),
        ("closed", &p.closed),
    ] {
        if let Some(v) = value {
            pad(out, depth + 1);
            let _ = writeln!(out, "{name} {v:?}");
        }
    }
    for (k, v) in &h.properties {
        pad(out, depth + 1);
        let _ = writeln!(out, "property {k} {v:?}");
    }
    elements(out, &h.section, depth + 1);
}

fn elements(out: &mut String, list: &[Element], depth: usize) {
    for e in list {
        element(out, e, depth);
    }
}

fn affiliated(out: &mut String, a: &Affiliated, depth: usize) {
    if a.is_empty() {
        return;
    }
    pad(out, depth + 1);
    out.push_str("with");
    if let Some(n) = &a.name {
        let _ = write!(out, " name={n:?}");
    }
    for c in &a.caption {
        let _ = write!(out, " caption=[{}]", objects(c));
    }
    for (b, v) in &a.attr {
        let _ = write!(out, " attr_{b}={v:?}");
    }
    if let Some(r) = &a.results {
        let _ = write!(out, " results={r:?}");
    }
    for h in &a.header {
        let _ = write!(out, " header={h:?}");
    }
    out.push('\n');
}

fn element(out: &mut String, e: &Element, depth: usize) {
    pad(out, depth);
    match &e.kind {
        ElementKind::Paragraph(t) => {
            let _ = writeln!(out, "paragraph @{} {}", e.line, objects(t));
        }
        ElementKind::PlainList(l) => {
            let _ = writeln!(out, "list {:?} @{}", l.kind, e.line);
            affiliated(out, &e.affiliated, depth);
            for item in &l.items {
                pad(out, depth + 1);
                let _ = write!(out, "item {:?} @{}", item.bullet, item.line);
                if let Some(n) = item.counter {
                    let _ = write!(out, " [@{n}]");
                }
                if let Some(c) = item.checkbox {
                    let _ = write!(out, " {c:?}");
                }
                if let Some(t) = &item.tag {
                    let _ = write!(out, " tag=[{}]", objects(t));
                }
                out.push('\n');
                elements(out, &item.contents, depth + 2);
            }
            return;
        }
        ElementKind::Table(t) => {
            let _ = write!(out, "table @{}", e.line);
            if !t.align.is_empty() {
                let _ = write!(out, " align={:?}", t.align);
            }
            out.push('\n');
            affiliated(out, &e.affiliated, depth);
            for row in &t.rows {
                pad(out, depth + 1);
                match row {
                    Row::Rule => out.push_str("rule\n"),
                    Row::Cells(cells) => {
                        out.push_str("row");
                        for c in cells {
                            let _ = write!(out, " [{}]", objects(c));
                        }
                        out.push('\n');
                    }
                }
            }
            for f in &t.formulas {
                pad(out, depth + 1);
                let _ = writeln!(out, "tblfm {f:?}");
            }
            return;
        }
        ElementKind::TableEl(lines) => {
            let _ = writeln!(out, "table.el @{} {lines:?}", e.line);
        }
        ElementKind::Block(b) => {
            block(out, b, e, depth);
            return;
        }
        ElementKind::Drawer { name, contents } => {
            let _ = writeln!(out, "drawer {name} @{}", e.line);
            affiliated(out, &e.affiliated, depth);
            elements(out, contents, depth + 1);
            return;
        }
        ElementKind::Keyword { key, value } => {
            let _ = writeln!(out, "keyword {key} @{} {value:?}", e.line);
        }
        ElementKind::FootnoteDefinition { label, contents } => {
            let _ = writeln!(out, "footnote {label} @{}", e.line);
            elements(out, contents, depth + 1);
            return;
        }
        ElementKind::FixedWidth(lines) => {
            let _ = writeln!(out, "fixed-width @{} {lines:?}", e.line);
        }
        ElementKind::Comment(lines) => {
            let _ = writeln!(out, "comment @{} {lines:?}", e.line);
        }
        ElementKind::HorizontalRule => {
            let _ = writeln!(out, "rule @{}", e.line);
        }
        ElementKind::LatexEnvironment(t) => {
            let _ = writeln!(out, "latex @{} {t:?}", e.line);
        }
        ElementKind::Clock(t) => {
            let _ = writeln!(out, "clock @{} {t:?}", e.line);
        }
    }
    affiliated(out, &e.affiliated, depth);
}

fn block(out: &mut String, b: &Block, e: &Element, depth: usize) {
    let kind = match &b.kind {
        BlockKind::Src { language } => format!("src {language:?}"),
        BlockKind::Export { backend } => format!("export {backend:?}"),
        BlockKind::Example => "example".to_string(),
        BlockKind::Verse => "verse".to_string(),
        BlockKind::Comment => "comment".to_string(),
        BlockKind::Quote => "quote".to_string(),
        BlockKind::Center => "center".to_string(),
        BlockKind::Special(n) => format!("special {n:?}"),
    };
    let _ = write!(out, "block {kind} @{}", e.line);
    if !b.parameters.is_empty() {
        let _ = write!(out, " params={:?}", b.parameters);
    }
    if !b.kind.is_greater() {
        let _ = write!(out, " {:?}", b.body);
    }
    out.push('\n');
    affiliated(out, &e.affiliated, depth);
    elements(out, &b.contents, depth + 1);
}

/// Objects on one line: text quoted, the rest as `kind(args)[contents]`.
pub(crate) fn objects(list: &[Object]) -> String {
    let mut out = String::new();
    for (i, o) in list.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        object(&mut out, o);
    }
    out
}

fn object(out: &mut String, o: &Object) {
    let nested = |out: &mut String, name: &str, contents: &[Object]| {
        let _ = write!(out, "{name}[{}]", objects(contents));
    };
    match o {
        Object::Text(t) => {
            let _ = write!(out, "{t:?}");
        }
        Object::Emphasis { kind, contents } => {
            let name = match kind {
                Emphasis::Bold => "bold",
                Emphasis::Italic => "italic",
                Emphasis::Underline => "underline",
                Emphasis::Strike => "strike",
            };
            nested(out, name, contents);
        }
        Object::Verbatim(t) => {
            let _ = write!(out, "verbatim({t:?})");
        }
        Object::Code(t) => {
            let _ = write!(out, "code({t:?})");
        }
        Object::Link {
            path,
            description,
            target,
        } => {
            match target {
                Some(t) => {
                    let _ = write!(out, "link({path:?} -> {t})");
                }
                None => {
                    let _ = write!(out, "link({path:?})");
                }
            }
            if !description.is_empty() {
                nested(out, "", description);
            }
        }
        Object::FootnoteReference { label, definition } => {
            match label {
                Some(l) => {
                    let _ = write!(out, "fn({l:?})");
                }
                None => out.push_str("fn(None)"),
            }
            if !definition.is_empty() {
                nested(out, "", definition);
            }
        }
        Object::Entity { name, braces, .. } => {
            let _ = write!(out, "entity({name}{})", if *braces { "{}" } else { "" });
        }
        Object::LatexFragment(t) => {
            let _ = write!(out, "latex({t:?})");
        }
        Object::Subscript { braced, contents } => {
            nested(out, if *braced { "sub{}" } else { "sub" }, contents)
        }
        Object::Superscript { braced, contents } => {
            nested(out, if *braced { "sup{}" } else { "sup" }, contents)
        }
        Object::LineBreak => out.push_str("linebreak"),
        Object::Timestamp(t) => {
            let _ = write!(out, "timestamp({t:?})");
        }
        Object::Target(t) => {
            let _ = write!(out, "target({t:?})");
        }
        Object::RadioTarget(c) => nested(out, "radio", c),
        Object::StatisticsCookie(t) => {
            let _ = write!(out, "cookie({t:?})");
        }
        Object::Macro { name, args } => {
            let _ = write!(out, "macro({name} {args:?})");
        }
        Object::ExportSnippet { backend, value } => {
            let _ = write!(out, "snippet({backend} {value:?})");
        }
    }
}

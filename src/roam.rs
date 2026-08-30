//! OR.4 — what makes a file's contents into roam nodes.
//!
//! Design: `docs/dev/architecture/org-roam.md` §3–§4 in the lattice tree.
//!
//! A **node** is a title, an id, and its links to other nodes. This module
//! answers, for one file: which ids are in it, what each one is called, what
//! tags it carries, and which other ids it points at. It writes nothing and
//! reads no configuration — [`index`](crate::roam_index) does both.
//!
//! ## Two grains, because the corpus has two
//!
//! A **file node** is a file whose top-level property drawer carries an `:ID:`;
//! its title is `#+title:` and its tags are `#+filetags:`. A **headline node**
//! is any headline whose property drawer carries an `:ID:`. On the reference
//! corpus that is 475 file nodes and **110 headline nodes** — so a file-only
//! model would silently drop 19% of the notes and every link into them.
//!
//! ## Structure from the tree, characters from the text
//!
//! `agenda.rs` states this rule; org-roam needs it stated *sharply*, because a
//! real parse turns out not to say what `grammar.js` reads like. Both of these
//! were established by dumping a real tree before any of this was written, and
//! both would have been guessed wrong:
//!
//! **A file-level `:PROPERTIES:` drawer is not a `property_drawer`.** The
//! grammar attaches `property_drawer` to a `section`, so a drawer at the top of
//! a file — before any headline — parses as a generic `drawer` sitting in the
//! document `body`. Worse, its innards are not `property` nodes: a
//! `property_drawer` gives structured children with `name` / `value` fields,
//! while a `drawer` gives one `contents` node holding an undifferentiated run
//! of `expr`s, in which `:ID:` and its value are separate untyped siblings.
//!
//! So the two grains genuinely need different readers, and the file grain reads
//! its properties **from the lines** the drawer spans. Since 81% of the corpus
//! is file-level, a shared reader written from the grammar source would have
//! dropped four notes in five and said nothing.
//!
//! **`[[id:…]]` links are not reliably one node.** In the same dump, one link
//! survived whole as a single `expr` and the next was split across two
//! (`"[[id:CCCC-DDDD][another"` + `"link]]."`), because org's grammar breaks a
//! paragraph on whitespace and a link description contains spaces. Link
//! extraction is therefore textual. The tree's job is not to find the links; it
//! is to decide **which node owns** the line a link sits on, which is a
//! structural question and exactly the one a text scan cannot answer.
//!
//! ## The split in this file
//!
//! [`Outline`] is the handful of structural facts the tree supplies.
//! [`extract`] is a **pure function** of `(path, text, outline)` — no tree, no
//! host calls — so every rule below is unit-testable on the host, where a
//! `tree-snapshot` cannot be constructed at all. `outline_from_tree` (in
//! [`crate::roam_tree`]) is the thin layer that harvests those facts, and it is
//! covered by the integration tests that run a real editor.

use serde::{Deserialize, Serialize};

/// One roam node, as it lands in the store.
///
/// Tags are **resolved** — inheritance is already applied — because the
/// alternative is a tree walk inside the picker's per-keystroke filter loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// The `:ID:` verbatim, as written.
    pub id: String,
    pub title: String,
    /// `:ROAM_ALIASES:` — 71 nodes in the reference corpus are findable under a
    /// name that is not their title, so this is real surface rather than
    /// completeness.
    pub aliases: Vec<String>,
    /// Own tags plus every ancestor's plus the file's, flattened.
    pub tags: Vec<String>,
    /// `:ROAM_REFS:`. One node in the reference corpus has one; it is indexed
    /// because it costs a column beside aliases, not because it is used.
    pub refs: Vec<String>,
    pub file: String,
    /// 0 for a file node, else the headline's line.
    pub line: u32,
    /// 0 for a file node, else headline depth.
    pub level: u32,
}

/// One node plus what it points at — kept beside [`Node`] rather than on it
/// because the store holds them in different key families (`n/<id>` vs
/// `b/<id>`), and a node record that carried its outgoing links would be
/// rewritten every time an unrelated file linked to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub node: Node,
    /// Ids this node links to, deduplicated, in first-appearance order.
    pub links: Vec<String>,
}

/// The structural facts [`extract`] needs from the parse tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outline {
    /// Line range (inclusive) of the file-level `:PROPERTIES:` drawer's
    /// contents, if the file opens with one.
    pub file_drawer: Option<(u32, u32)>,
    /// `(name, value)` for every top-level `#+keyword:` line, name lowercased.
    pub directives: Vec<(String, String)>,
    /// Every headline section, in document order (parents before children).
    pub sections: Vec<Section>,
}

/// One headline's structural facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    /// The headline's own line.
    pub headline_line: u32,
    /// The last line this section covers, subsections included.
    pub end_line: u32,
    /// Headline depth — the width of the `stars` node.
    pub level: u32,
    /// The headline text as written, TODO keyword / priority / tags still on
    /// it. [`extract`] strips them, because what counts as a keyword is
    /// configuration and the tree does not know.
    pub raw_title: String,
    /// Tags written on this headline itself.
    pub own_tags: Vec<String>,
    /// Line range (inclusive) of this headline's property drawer, if any.
    pub drawer: Option<(u32, u32)>,
    /// Index into [`Outline::sections`] of the enclosing section, if any.
    pub parent: Option<usize>,
}

/// Extract every node in `text`, with the links each one owns.
///
/// Pure: no tree, no host calls, no configuration beyond `todo_keywords` (which
/// decides what a title's leading word means). `path` is recorded verbatim on
/// each node.
pub fn extract(
    path: &str,
    text: &str,
    outline: &Outline,
    todo_keywords: &[String],
) -> Vec<Extracted> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Extracted> = Vec::new();

    // --- the file node -----------------------------------------------------
    let file_props = outline
        .file_drawer
        .map(|(a, b)| properties_in_lines(&lines, a, b))
        .unwrap_or_default();
    let file_tags = directive(outline, "filetags")
        .map(|v| split_tags(&v))
        .unwrap_or_default();

    let mut file_node_index: Option<usize> = None;
    if let Some(id) = property(&file_props, "ID") {
        let title = directive(outline, "title").unwrap_or_else(|| file_stem(path).to_string());
        file_node_index = Some(out.len());
        out.push(Extracted {
            node: Node {
                id,
                title,
                aliases: property_list(&file_props, "ROAM_ALIASES"),
                tags: file_tags.clone(),
                refs: property_list(&file_props, "ROAM_REFS"),
                file: path.to_string(),
                line: 0,
                level: 0,
            },
            links: Vec::new(),
        });
    }

    // --- headline nodes ----------------------------------------------------
    //
    // `section_node[i]` is the index in `out` of the node section `i` produced,
    // if it produced one. A section with no `:ID:` is not a node but is still
    // walked, because its tags are inherited by any descendant that IS one.
    let mut section_node: Vec<Option<usize>> = vec![None; outline.sections.len()];
    for (i, section) in outline.sections.iter().enumerate() {
        let props = section
            .drawer
            .map(|(a, b)| properties_in_lines(&lines, a, b))
            .unwrap_or_default();
        let Some(id) = property(&props, "ID") else {
            continue;
        };
        section_node[i] = Some(out.len());
        out.push(Extracted {
            node: Node {
                id,
                title: strip_headline(&section.raw_title, todo_keywords),
                aliases: property_list(&props, "ROAM_ALIASES"),
                // Resolved HERE, not at query time: deferring it would put a
                // tree walk inside the picker's per-keystroke filter loop.
                tags: inherited_tags(outline, i, &file_tags),
                refs: property_list(&props, "ROAM_REFS"),
                file: path.to_string(),
                line: section.headline_line,
                level: section.level,
            },
            links: Vec::new(),
        });
    }

    // --- links, attributed to the node that owns their line ----------------
    for (line_no, line) in lines.iter().enumerate() {
        let line_no = line_no as u32;
        let owner = owning_node(outline, &section_node, file_node_index, line_no);
        let Some(owner) = owner else {
            continue;
        };
        for target in id_links_in(line) {
            // Deduplicated per node: a note that links to the same target three
            // times is one backlink, not three.
            if !out[owner]
                .links
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&target))
            {
                out[owner].links.push(target);
            }
        }
    }
    out
}

/// Which node owns `line` — the innermost ID-bearing section containing it, or
/// the file node.
///
/// Innermost rather than nearest-preceding, because sections nest: a link in a
/// deep subsection belongs to that subsection when it has an id, and to the
/// closest ancestor that does when it does not. That walk is why the tree is
/// needed at all for links whose *text* a scan already found.
fn owning_node(
    outline: &Outline,
    section_node: &[Option<usize>],
    file_node: Option<usize>,
    line: u32,
) -> Option<usize> {
    let mut best: Option<(u32, usize)> = None; // (level, node index)
    for (i, section) in outline.sections.iter().enumerate() {
        if line < section.headline_line || line > section.end_line {
            continue;
        }
        let Some(node) = section_node[i] else {
            continue;
        };
        if best.is_none_or(|(level, _)| section.level > level) {
            best = Some((section.level, node));
        }
    }
    best.map(|(_, n)| n).or(file_node)
}

/// The value of the first `#+<name>:` directive, if any. Case-insensitive:
/// the corpus contains `#+TITLE:`, `#+title:`, `#+Filetags:` and `#+filetags:`,
/// all written by org itself at different times, and a case-sensitive reader
/// would index half of it.
fn directive(outline: &Outline, name: &str) -> Option<String> {
    outline
        .directives
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Read `:KEY: value` pairs out of the lines a drawer spans.
///
/// **From the lines, not from the tree**, and that is the file grain's whole
/// story: a file-level drawer parses as a generic `drawer` whose contents are
/// an undifferentiated run of `expr`s, so there is no `name`/`value` pair to
/// read. Doing it the same way for both grains means one reader with one set of
/// rules rather than two that drift.
fn properties_in_lines(lines: &[&str], first: u32, last: u32) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for i in first..=last {
        let Some(line) = lines.get(i as usize) else {
            break;
        };
        let trimmed = line.trim();
        // `:PROPERTIES:` / `:END:` are the fence, not properties.
        if trimmed.eq_ignore_ascii_case(":properties:") || trimmed.eq_ignore_ascii_case(":end:") {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix(':') else {
            continue;
        };
        let Some((name, value)) = rest.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        out.push((name.to_string(), value.trim().to_string()));
    }
    out
}

/// A single property's value, case-insensitively by name.
fn property(props: &[(String, String)], name: &str) -> Option<String> {
    props
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// A property whose value is a list — `:ROAM_ALIASES:` and `:ROAM_REFS:`.
///
/// Org quotes an alias containing spaces: `:ROAM_ALIASES: "Honey Garlic"
/// Chicken` is two aliases, not three words. Splitting on whitespace alone
/// would make *Honey Garlic Chicken Breast* findable under `"Honey` and
/// `Garlic"`, which is worse than not supporting aliases at all.
fn property_list(props: &[(String, String)], name: &str) -> Vec<String> {
    let Some(raw) = property(props, name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in raw.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                // A closing quote ends the item even when the next char is not
                // whitespace, so `"a""b"` is two items rather than one.
                if !quoted && !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// `:a:b:c:` → `["a", "b", "c"]`. Also accepts a bare space-separated list,
/// which org tolerates in `#+filetags:`.
fn split_tags(raw: &str) -> Vec<String> {
    raw.split([':', ' ', '\t'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// A headline node's tags: its own, plus every ancestor's, plus the file's.
///
/// Deduplicated case-sensitively (org tags are case-sensitive) and ordered
/// innermost-first, because that is the order a reader expects to see them in.
fn inherited_tags(outline: &Outline, index: usize, file_tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cursor = Some(index);
    while let Some(i) = cursor {
        let section = &outline.sections[i];
        for tag in &section.own_tags {
            if !out.contains(tag) {
                out.push(tag.clone());
            }
        }
        cursor = section.parent;
    }
    for tag in file_tags {
        if !out.contains(tag) {
            out.push(tag.clone());
        }
    }
    out
}

/// A headline's title: the text with its TODO keyword, priority cookie, any
/// trailing tag list and any statistics cookie removed.
///
/// The keyword list is configuration, which is why it is a parameter rather
/// than a constant — the same reason TK.4 generates the highlight rules from
/// the option instead of guessing a word list.
fn strip_headline(raw: &str, todo_keywords: &[String]) -> String {
    let mut s = raw.trim();
    // The TODO keyword, if the first word is one.
    if let Some((first, rest)) = s.split_once(char::is_whitespace) {
        if todo_keywords.iter().any(|k| k == first) {
            s = rest.trim_start();
        }
    }
    // A priority cookie, `[#A]`.
    if let Some(rest) = s.strip_prefix("[#") {
        if let Some((_, after)) = rest.split_once(']') {
            s = after.trim_start();
        }
    }
    // A trailing tag list, `:tag:tag:`. Only when it is genuinely trailing and
    // genuinely a tag list — a title ending in a colon must survive.
    let mut s = s.trim_end();
    if s.ends_with(':') {
        if let Some(start) = s.rfind(':') {
            let candidate = &s[..=start];
            if let Some(open) = candidate.rfind(|c: char| c.is_whitespace()) {
                let tail = &candidate[open + 1..];
                if tail.len() > 2 && tail.starts_with(':') && tail.ends_with(':') {
                    s = candidate[..open].trim_end();
                }
            }
        }
    }
    // A statistics cookie, `[2/5]` or `[40%]`.
    if s.ends_with(']') {
        if let Some(open) = s.rfind('[') {
            let inner = &s[open + 1..s.len() - 1];
            if !inner.is_empty()
                && inner
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '/' || c == '%')
            {
                s = s[..open].trim_end();
            }
        }
    }
    s.to_string()
}

/// Every `[[id:<ID>]` target on one line, in order.
///
/// **Textual, and deliberately so** — see the module note: org's grammar splits
/// a paragraph on whitespace, so a link with a description is not one node and
/// cannot be read as one. The scan ends at `]` so both link shapes work:
/// `[[id:X]]` and `[[id:X][description]]`.
fn id_links_in(line: &str) -> Vec<String> {
    const OPEN: &str = "[[id:";
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(at) = find_ignore_ascii_case(rest, OPEN) {
        let after = &rest[at + OPEN.len()..];
        let end = after.find(']').unwrap_or(after.len());
        let id = after[..end].trim();
        if !id.is_empty() {
            out.push(id.to_string());
        }
        rest = &after[end.min(after.len())..];
        if rest.is_empty() {
            break;
        }
        rest = &rest[1.min(rest.len())..];
    }
    out
}

/// `str::find`, case-insensitively, for an ASCII needle.
///
/// `[[ID:` and `[[Id:` are both org, and a link that fails to resolve over
/// letter case looks exactly like a missing note — the worst way for this to
/// fail, and the same reason ids themselves compare case-insensitively.
fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .char_indices()
        .filter(|(i, _)| haystack.is_char_boundary(*i))
        .find(|(i, _)| {
            haystack
                .get(*i..*i + needle.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(needle))
        })
        .map(|(i, _)| i)
}

/// A path's filename without its extension — the fallback title for a file node
/// with an `:ID:` but no `#+title:`. Honest rather than pretty: a node with no
/// title is findable under the slug the user would otherwise never see.
fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keywords() -> Vec<String> {
        ["TODO", "NEXT", "DONE"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// The file grain: a drawer at the top of the file, read from its lines.
    fn file_outline(drawer: (u32, u32), directives: &[(&str, &str)]) -> Outline {
        Outline {
            file_drawer: Some(drawer),
            directives: directives
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            sections: Vec::new(),
        }
    }

    const FILE_NOTE: &str = "\
:PROPERTIES:
:ID:       6F398E54-1111-2222-3333-444455556666
:ROAM_ALIASES: \"Honey Garlic\" Chicken
:END:
#+title: Honey Garlic Chicken Breast
#+filetags: :food:recipe:

Prose with a [[id:AAAA-BBBB][link]] in it.
";

    #[test]
    fn a_file_node_is_read_from_the_lines_its_drawer_spans() {
        let outline = file_outline(
            (0, 3),
            &[
                ("title", "Honey Garlic Chicken Breast"),
                ("filetags", ":food:recipe:"),
            ],
        );
        let got = extract("/roam/chicken.org", FILE_NOTE, &outline, &keywords());
        assert_eq!(got.len(), 1, "one file node: {got:?}");
        let n = &got[0].node;
        assert_eq!(n.id, "6F398E54-1111-2222-3333-444455556666");
        assert_eq!(n.title, "Honey Garlic Chicken Breast");
        assert_eq!(n.tags, vec!["food", "recipe"]);
        assert_eq!(n.line, 0, "a file node has no headline line");
        assert_eq!(n.level, 0);
        assert_eq!(got[0].links, vec!["AAAA-BBBB"]);
    }

    /// Org quotes an alias containing spaces. Splitting on whitespace alone
    /// would make this note findable under `"Honey` and `Garlic"`, which is
    /// worse than having no aliases at all.
    #[test]
    fn a_quoted_alias_is_one_alias() {
        let outline = file_outline((0, 3), &[("title", "Honey Garlic Chicken Breast")]);
        let got = extract("/roam/chicken.org", FILE_NOTE, &outline, &keywords());
        assert_eq!(got[0].node.aliases, vec!["Honey Garlic", "Chicken"]);
    }

    /// The corpus contains `#+TITLE:`, `#+title:` and `#+Filetags:`, all
    /// written by org itself. A case-sensitive reader indexes half of it.
    #[test]
    fn keyword_case_does_not_matter() {
        let text = ":PROPERTIES:\n:ID: ABC\n:END:\n#+TITLE: Shouty\n#+Filetags: :one:\n";
        let outline = file_outline((0, 2), &[("TITLE", "Shouty"), ("Filetags", ":one:")]);
        let got = extract("/n.org", text, &outline, &keywords());
        assert_eq!(got[0].node.title, "Shouty");
        assert_eq!(got[0].node.tags, vec!["one"]);
    }

    /// A file with no `#+title:` is still findable — under its slug, which is
    /// honest rather than pretty.
    #[test]
    fn a_file_node_with_no_title_falls_back_to_its_slug() {
        let text = ":PROPERTIES:\n:ID: ABC\n:END:\n";
        let outline = file_outline((0, 2), &[]);
        let got = extract(
            "/roam/20250603103551-chicken.org",
            text,
            &outline,
            &keywords(),
        );
        assert_eq!(got[0].node.title, "20250603103551-chicken");
    }

    fn section(headline_line: u32, end_line: u32, level: u32, title: &str) -> Section {
        Section {
            headline_line,
            end_line,
            level,
            raw_title: title.to_string(),
            own_tags: Vec::new(),
            drawer: None,
            parent: None,
        }
    }

    const BOTH_GRAINS: &str = "\
:PROPERTIES:
:ID: FILE-ID
:END:
#+title: The File
#+filetags: :filetag:

* A headline node
:PROPERTIES:
:ID: HEAD-ID
:END:

Body with [[id:TARGET][a link]].
";

    /// Both grains in one file — the case a file-only model drops 19% of the
    /// corpus by getting wrong.
    #[test]
    fn a_file_node_and_a_headline_node_coexist() {
        let mut head = section(6, 11, 1, "A headline node");
        head.drawer = Some((7, 9));
        let outline = Outline {
            file_drawer: Some((0, 2)),
            directives: vec![
                ("title".into(), "The File".into()),
                ("filetags".into(), ":filetag:".into()),
            ],
            sections: vec![head],
        };
        let got = extract("/n.org", BOTH_GRAINS, &outline, &keywords());
        assert_eq!(got.len(), 2, "both grains: {got:?}");
        assert_eq!(got[0].node.id, "FILE-ID");
        assert_eq!(got[1].node.id, "HEAD-ID");
        assert_eq!(got[1].node.line, 6, "a headline node lands on its headline");
        assert_eq!(got[1].node.level, 1);
        // The link is inside the headline's span, so the HEADLINE owns it —
        // this is the structural question a text scan cannot answer.
        assert_eq!(got[0].links, Vec::<String>::new());
        assert_eq!(got[1].links, vec!["TARGET"]);
    }

    /// Tag inheritance through two headline levels, resolved at index time.
    #[test]
    fn a_headline_node_inherits_its_ancestors_tags_and_the_files() {
        let text = ":PROPERTIES:\n:ID: F\n:END:\n#+filetags: :file:\n\n* Parent\n\n** Child\n:PROPERTIES:\n:ID: C\n:END:\n";
        let mut parent = section(5, 10, 1, "Parent");
        parent.own_tags = vec!["parent".into()];
        let mut child = section(7, 10, 2, "Child");
        child.own_tags = vec!["child".into()];
        child.parent = Some(0);
        child.drawer = Some((8, 10));
        let outline = Outline {
            file_drawer: Some((0, 2)),
            directives: vec![("filetags".into(), ":file:".into())],
            sections: vec![parent, child],
        };
        let got = extract("/n.org", text, &outline, &keywords());
        let child = got.iter().find(|e| e.node.id == "C").expect("the child");
        assert_eq!(
            child.node.tags,
            vec!["child", "parent", "file"],
            "own, then ancestors', then the file's"
        );
    }

    /// A section with no `:ID:` is not a node, but its tags still reach a
    /// descendant that is one. Skipping it entirely would lose them.
    #[test]
    fn an_untagged_intermediate_section_still_passes_its_tags_down() {
        let text = "* Middle\n** Leaf\n:PROPERTIES:\n:ID: L\n:END:\n";
        let mut middle = section(0, 4, 1, "Middle");
        middle.own_tags = vec!["middle".into()];
        let mut leaf = section(1, 4, 2, "Leaf");
        leaf.parent = Some(0);
        leaf.drawer = Some((2, 4));
        let outline = Outline {
            file_drawer: None,
            directives: Vec::new(),
            sections: vec![middle, leaf],
        };
        let got = extract("/n.org", text, &outline, &keywords());
        assert_eq!(got.len(), 1, "the middle section is not a node");
        assert_eq!(got[0].node.tags, vec!["middle"]);
    }

    /// The innermost ID-bearing section owns a link, not the nearest preceding
    /// headline and not the file.
    #[test]
    fn the_innermost_node_owns_a_link() {
        let text = "* Outer\n:PROPERTIES:\n:ID: O\n:END:\n** Inner\n:PROPERTIES:\n:ID: I\n:END:\nlink [[id:T]]\n";
        let mut outer = section(0, 8, 1, "Outer");
        outer.drawer = Some((1, 3));
        let mut inner = section(4, 8, 2, "Inner");
        inner.drawer = Some((5, 7));
        inner.parent = Some(0);
        let outline = Outline {
            file_drawer: None,
            directives: Vec::new(),
            sections: vec![outer, inner],
        };
        let got = extract("/n.org", text, &outline, &keywords());
        let outer = got.iter().find(|e| e.node.id == "O").unwrap();
        let inner = got.iter().find(|e| e.node.id == "I").unwrap();
        assert!(outer.links.is_empty(), "the outer node does not own it");
        assert_eq!(inner.links, vec!["T"]);
    }

    /// Both link shapes, and a link whose description contains spaces — the
    /// case the parse tree splits into two nodes and a tree-native reader would
    /// miss.
    #[test]
    fn both_link_shapes_are_found_including_across_a_split_description() {
        assert_eq!(id_links_in("see [[id:ABC]] here"), vec!["ABC"]);
        assert_eq!(
            id_links_in("see [[id:ABC][a long description]] here"),
            vec!["ABC"]
        );
        assert_eq!(
            id_links_in("[[id:A][one]] and [[id:B][two]]"),
            vec!["A", "B"]
        );
        assert_eq!(id_links_in("no links here"), Vec::<String>::new());
    }

    /// `[[ID:` and `[[Id:` are both org, and a link that fails to resolve over
    /// letter case looks exactly like a missing note.
    #[test]
    fn link_scanning_is_case_insensitive() {
        assert_eq!(id_links_in("[[ID:ABC][x]]"), vec!["ABC"]);
        assert_eq!(id_links_in("[[Id:ABC]]"), vec!["ABC"]);
    }

    /// A note that links to the same target three times is one backlink.
    #[test]
    fn repeated_links_to_one_target_are_deduplicated() {
        let text = ":PROPERTIES:\n:ID: F\n:END:\n[[id:T]] [[id:T]]\n[[id:t]]\n";
        let outline = file_outline((0, 2), &[]);
        let got = extract("/n.org", text, &outline, &keywords());
        assert_eq!(
            got[0].links,
            vec!["T"],
            "including across letter case, which is how ids compare"
        );
    }

    #[test]
    fn a_headline_title_loses_its_keyword_priority_tags_and_cookie() {
        let k = keywords();
        assert_eq!(
            strip_headline("TODO Write the thing", &k),
            "Write the thing"
        );
        assert_eq!(strip_headline("DONE [#A] Ship it", &k), "Ship it");
        assert_eq!(
            strip_headline("Plain title  :tag:other:", &k),
            "Plain title"
        );
        assert_eq!(strip_headline("Reading list [2/5]", &k), "Reading list");
        assert_eq!(strip_headline("NEXT [#B] Do it [40%]", &k), "Do it");
    }

    /// A title that merely ends in a colon is not a tag list.
    #[test]
    fn a_title_ending_in_a_colon_survives() {
        assert_eq!(
            strip_headline("A note about C++:", &keywords()),
            "A note about C++:"
        );
    }

    /// A word that is not a configured keyword is part of the title. Which
    /// words are keywords is configuration, which is why it is a parameter.
    #[test]
    fn a_non_keyword_first_word_stays_in_the_title() {
        assert_eq!(strip_headline("MAYBE do it", &keywords()), "MAYBE do it");
        let custom = vec!["MAYBE".to_string()];
        assert_eq!(strip_headline("MAYBE do it", &custom), "do it");
    }

    /// A file with no `:ID:` anywhere produces no nodes, and its links are
    /// owned by nobody rather than by a phantom.
    #[test]
    fn a_file_with_no_ids_produces_nothing() {
        let text = "#+title: Just a file\n\nWith a [[id:T]] link.\n";
        let outline = Outline {
            file_drawer: None,
            directives: vec![("title".into(), "Just a file".into())],
            sections: Vec::new(),
        };
        assert!(extract("/n.org", text, &outline, &keywords()).is_empty());
    }

    /// A drawer whose lines carry junk does not derail the read — one
    /// malformed property must not cost the node its id.
    #[test]
    fn a_malformed_property_line_is_skipped_not_fatal() {
        let text = ":PROPERTIES:\nnot a property\n:ID: ABC\n:  : empty name\n:END:\n";
        let outline = file_outline((0, 4), &[]);
        let got = extract("/n.org", text, &outline, &keywords());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].node.id, "ABC");
    }
}

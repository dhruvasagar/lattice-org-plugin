//! `#+INCLUDE:` expansion: ox's, textual and before the parse.
//!
//! Textual because an included file's headlines nest into the including
//! file's outline, its keywords (`#+MACRO:`, `#+OPTIONS:`) apply, and its
//! includes include in turn: all things the parse of the whole sees and a
//! splice into the tree would have to redo by hand.
//!
//! The text is held as pieces: runs of text, and includes not yet read. A
//! step reads one include and replaces it with its own pieces, so no call
//! reads more than one file, and the text is joined once at the end. The
//! include lines are found by one pass over each text that skips blocks,
//! where an `#+INCLUDE:` is a block's content, not a keyword.
//!
//! What an include reads goes through the host's `read-file`, so it can
//! include only what the plugin may read; one it may not read fails the
//! export, naming the path, rather than leaving a silent gap.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// How deep includes may nest.
const MAX_DEPTH: usize = 16;

/// A text being expanded.
pub(crate) struct Includes {
    pieces: Vec<Piece>,
    /// Every piece before this one is text.
    at: usize,
    todo_keywords: Vec<String>,
    /// Each top-level include's line, and the lines it has become so far.
    map: BTreeMap<u32, u32>,
}

enum Piece {
    /// Text, and the line of the including file's top-level include it came
    /// from (`None` for the including file's own text).
    Text {
        text: String,
        origin: Option<u32>,
    },
    Include(Pending),
}

/// An include not yet read.
struct Pending {
    /// The keyword's value.
    value: String,
    /// The directory a relative path is read from: the including file's.
    dir: PathBuf,
    /// The files that include this one, outermost first, for cycles.
    chain: Vec<PathBuf>,
    /// The level of the headline the include sits under; 0 above the first.
    level: usize,
    /// The line of the top-level include this one descends from.
    origin: u32,
}

/// The expanded text, and where the including file's lines went.
pub(crate) struct Expanded {
    pub(crate) text: String,
    /// `(line of a top-level include, lines it became)`, in line order.
    map: BTreeMap<u32, u32>,
}

impl Expanded {
    /// Where line `line` of the including file is in the expanded text. An
    /// include line maps to the first line it became.
    pub(crate) fn line(&self, line: u32) -> u32 {
        let shift: i64 = self.map.range(..line).map(|(_, n)| i64::from(*n) - 1).sum();
        (i64::from(line) + shift).max(0) as u32
    }
}

impl Includes {
    /// `text`, the file `source`, with its includes found but not read.
    pub(crate) fn new(text: &str, source: &str, todo_keywords: Vec<String>) -> Self {
        let source = normalize(Path::new(source));
        let dir = source.parent().map(Path::to_path_buf).unwrap_or_default();
        let pieces = scan(text, &dir, &[source], 0, None);
        let map = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Include(i) => Some((i.origin, 0)),
                Piece::Text { .. } => None,
            })
            .collect();
        Includes {
            pieces,
            at: 0,
            todo_keywords,
            map,
        }
    }

    /// Whether every include is read.
    pub(crate) fn done(&self) -> bool {
        self.pieces[self.at..]
            .iter()
            .all(|p| matches!(p, Piece::Text { .. }))
    }

    /// Read the next include with `read`. `Err` fails the export.
    pub(crate) fn step(
        &mut self,
        read: &dyn Fn(&str) -> Result<String, String>,
    ) -> Result<(), String> {
        while matches!(self.pieces.get(self.at), Some(Piece::Text { .. })) {
            self.at += 1;
        }
        let Some(Piece::Include(pending)) = self.pieces.get(self.at) else {
            return Ok(());
        };
        let replacement = resolve(pending, read, &self.todo_keywords)?;
        self.pieces.splice(self.at..=self.at, replacement);
        Ok(())
    }

    /// The text, joined. Call once [`done`](Self::done).
    pub(crate) fn finish(self) -> Expanded {
        let mut text = String::new();
        let mut map = self.map;
        for piece in self.pieces {
            let Piece::Text { text: t, origin } = piece else {
                continue;
            };
            if let Some(line) = origin {
                *map.entry(line).or_insert(0) += t.bytes().filter(|&b| b == b'\n').count() as u32;
            }
            text.push_str(&t);
        }
        Expanded { text, map }
    }
}

/// Whether `s` starts with `prefix`, ASCII case aside.
fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
}

/// `text`'s pieces: its runs of text and its includes. `level` is the
/// headline level in force at the top of `text`; `origin` the top-level
/// include it came from, `None` for the including file itself (whose
/// includes are each their own origin).
fn scan(
    text: &str,
    dir: &Path,
    chain: &[PathBuf],
    mut level: usize,
    origin: Option<u32>,
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut run = String::new();
    let mut block: Option<String> = None;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let trimmed = line.trim_start();
        if let Some(name) = &block {
            let end = trimmed.trim_end();
            if starts_with_ci(end, "#+end_") && end[6..].eq_ignore_ascii_case(name) {
                block = None;
            }
            run.push_str(line);
            continue;
        }
        if let Some(l) = crate::headline::headline_level(line) {
            level = l;
        } else if starts_with_ci(trimmed, "#+begin_") {
            let name = trimmed[8..].split(char::is_whitespace).next().unwrap_or("");
            block = Some(name.to_string());
        } else if starts_with_ci(trimmed, "#+include:") {
            if !run.is_empty() {
                pieces.push(Piece::Text {
                    text: std::mem::take(&mut run),
                    origin,
                });
            }
            pieces.push(Piece::Include(Pending {
                value: trimmed[10..].trim().to_string(),
                dir: dir.to_path_buf(),
                chain: chain.to_vec(),
                level,
                origin: origin.unwrap_or(i as u32),
            }));
            continue;
        }
        run.push_str(line);
    }
    if !run.is_empty() {
        pieces.push(Piece::Text { text: run, origin });
    }
    pieces
}

/// One `#+INCLUDE:` keyword's value, read.
#[derive(Debug, Default, PartialEq, Eq)]
struct Directive {
    path: String,
    /// `::*headline` or `::#custom-id`.
    search: Option<String>,
    /// `src rust`, `example`, `export html`, `quote`…: the block kind and
    /// its parameters.
    block: Option<(String, String)>,
    /// `:lines "5-10"`: 1-based, inclusive, either end open.
    lines: Option<(Option<usize>, Option<usize>)>,
    minlevel: Option<usize>,
    only_contents: bool,
}

fn directive(value: &str) -> Result<Directive, String> {
    let bad = || format!("org: cannot read #+INCLUDE: {value}");
    let mut words = words(value).into_iter();
    let target = words.next().ok_or_else(bad)?;
    let (path, search) = match target.split_once("::") {
        Some((p, s)) => (p.to_string(), Some(s.to_string())),
        None => (target, None),
    };
    let mut d = Directive {
        path,
        search,
        ..Directive::default()
    };
    let mut positional = Vec::new();
    while let Some(w) = words.next() {
        match w.as_str() {
            ":lines" => {
                let range = words.next().ok_or_else(bad)?;
                let (from, to) = range.split_once('-').ok_or_else(bad)?;
                let num = |s: &str| -> Result<Option<usize>, String> {
                    if s.trim().is_empty() {
                        Ok(None)
                    } else {
                        s.trim().parse().map(Some).map_err(|_| bad())
                    }
                };
                d.lines = Some((num(from)?, num(to)?));
            }
            ":minlevel" => {
                d.minlevel = Some(
                    words
                        .next()
                        .and_then(|n| n.parse().ok())
                        .filter(|&n| n > 0)
                        .ok_or_else(bad)?,
                );
            }
            ":only-contents" => d.only_contents = words.next().is_some_and(|v| v != "nil"),
            w if w.starts_with(':') => {
                // A keyword this does not read, and its value.
                words.next();
            }
            _ => positional.push(w),
        }
    }
    if let Some((kind, params)) = positional.split_first() {
        d.block = Some((kind.to_ascii_lowercase(), params.join(" ")));
    }
    Ok(d)
}

/// `s` split at whitespace, a `"…"` one word without its quotes.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s.trim_start();
    while !rest.is_empty() {
        if let Some(q) = rest.strip_prefix('"') {
            let end = q.find('"').unwrap_or(q.len());
            out.push(q[..end].to_string());
            rest = q.get(end + 1..).unwrap_or("").trim_start();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push(rest[..end].to_string());
            rest = rest[end..].trim_start();
        }
    }
    out
}

/// Read `p` and give the pieces it becomes.
fn resolve(
    p: &Pending,
    read: &dyn Fn(&str) -> Result<String, String>,
    todo_keywords: &[String],
) -> Result<Vec<Piece>, String> {
    let d = directive(&p.value)?;
    let path = normalize(&p.dir.join(crate::roam_scan::expand_tilde(&d.path)));
    let shown = path.display().to_string();
    if p.chain.contains(&path) {
        return Err(format!("org: {shown} includes itself"));
    }
    if p.chain.len() > MAX_DEPTH {
        return Err(format!(
            "org: includes nest deeper than {MAX_DEPTH} at {shown}"
        ));
    }
    let text = read(&shown).map_err(|why| format!("org: cannot include {shown}: {why}"))?;
    let mut lines: Vec<&str> = text.lines().collect();
    if let Some(search) = &d.search {
        let (mut first, end) = find(&lines, search, todo_keywords)
            .ok_or_else(|| format!("org: {shown} has no {search}"))?;
        if d.only_contents {
            first = after_meta(&lines, first).min(end);
        }
        lines = lines[first..end].to_vec();
    }
    if let Some((from, to)) = d.lines {
        let from = from.unwrap_or(1).saturating_sub(1).min(lines.len());
        let to = to.unwrap_or(lines.len()).min(lines.len()).max(from);
        lines = lines[from..to].to_vec();
    }
    let origin = Some(p.origin);
    match &d.block {
        Some((kind, params)) => {
            let escape = kind == "src" || kind == "example";
            let mut out = format!("#+begin_{kind}");
            if !params.is_empty() {
                out.push(' ');
                out.push_str(params);
            }
            out.push('\n');
            for line in &lines {
                if escape && needs_comma(line) {
                    let indent = line.len() - line.trim_start().len();
                    out.push_str(&line[..indent]);
                    out.push(',');
                    out.push_str(&line[indent..]);
                } else {
                    out.push_str(line);
                }
                out.push('\n');
            }
            out.push_str(&format!("#+end_{kind}\n"));
            Ok(vec![Piece::Text { text: out, origin }])
        }
        None => {
            // ox's default: an include under a headline nests below it.
            let minlevel = d.minlevel.or((p.level > 0).then_some(p.level + 1));
            let text = shift(&lines, minlevel);
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let mut chain = p.chain.clone();
            chain.push(path);
            Ok(scan(&text, &dir, &chain, p.level, origin))
        }
    }
}

/// The subtree `search` names: `(headline line, end)`.
fn find(lines: &[&str], search: &str, todo_keywords: &[String]) -> Option<(usize, usize)> {
    let at = if let Some(id) = search.strip_prefix('#') {
        lines.iter().position(|l| {
            let t = l.trim();
            starts_with_ci(t, ":custom_id:") && t[11..].trim() == id
        })?
    } else {
        let title = search.strip_prefix('*').unwrap_or(search).trim();
        lines.iter().position(|l| {
            crate::todo::parse(l, todo_keywords).is_some_and(|h| {
                let t = h.title.strip_prefix("COMMENT ").unwrap_or(h.title);
                t.trim() == title
            })
        })?
    };
    let (first, end) = crate::babel::block::subtree_at(lines, at as u32)?;
    Some((first as usize, end as usize))
}

/// The first line after headline `h`'s planning and property drawer.
fn after_meta(lines: &[&str], h: usize) -> usize {
    let mut i = h + 1;
    let planning = ["SCHEDULED:", "DEADLINE:", "CLOSED:"];
    while lines
        .get(i)
        .is_some_and(|l| planning.iter().any(|p| l.trim_start().starts_with(p)))
    {
        i += 1;
    }
    if lines
        .get(i)
        .is_some_and(|l| l.trim().eq_ignore_ascii_case(":PROPERTIES:"))
    {
        while lines
            .get(i)
            .is_some_and(|l| !l.trim().eq_ignore_ascii_case(":END:"))
        {
            i += 1;
        }
        i += 1;
    }
    i.min(lines.len())
}

/// Whether a line of code would read as org inside a block: `*` or `#+`,
/// after any commas already escaping it.
fn needs_comma(line: &str) -> bool {
    let t = line.trim_start().trim_start_matches(',');
    t.starts_with('*') || t.starts_with("#+")
}

/// `lines` as text, headlines shifted so the shallowest is at `minlevel`.
fn shift(lines: &[&str], minlevel: Option<usize>) -> String {
    let level = |l: &str| crate::headline::headline_level(l);
    let delta = minlevel.and_then(|min| {
        let shallowest = lines.iter().filter_map(|l| level(l)).min()?;
        Some(min as i64 - shallowest as i64)
    });
    let mut out = String::new();
    for line in lines {
        match (delta, level(line)) {
            (Some(d), Some(l)) if d != 0 => {
                let to = (l as i64 + d).max(1) as usize;
                out.push_str(&"*".repeat(to));
                out.push_str(&line[l..]);
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// `path` with `.` and `..` taken out, so one file has one name in a chain.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn expand(main: &str, files: &[(&str, &str)]) -> Result<Expanded, String> {
        let files: HashMap<String, String> = files
            .iter()
            .map(|(p, t)| (p.to_string(), t.to_string()))
            .collect();
        let read = move |p: &str| files.get(p).cloned().ok_or_else(|| "missing".to_string());
        let keywords = vec!["TODO".to_string(), "DONE".to_string()];
        let mut inc = Includes::new(main, "/notes/main.org", keywords);
        let mut steps = 0;
        while !inc.done() {
            inc.step(&read)?;
            steps += 1;
            assert!(steps < 100, "runaway");
        }
        Ok(inc.finish())
    }

    fn text(main: &str, files: &[(&str, &str)]) -> String {
        expand(main, files).unwrap_or_else(|e| panic!("{e}")).text
    }

    #[test]
    fn an_include_is_spliced_and_nests_below_its_headline() {
        let got = text(
            "* Top\n#+INCLUDE: \"part.org\"\nafter\n",
            &[("/notes/part.org", "intro\n* A\n** B\n")],
        );
        assert_eq!(got, "* Top\nintro\n** A\n*** B\nafter\n");
        // Above every headline, no shift; `:minlevel` sets it.
        let top = text("#+include: part.org\n", &[("/notes/part.org", "** A\n")]);
        assert_eq!(top, "** A\n");
        let set = text(
            "* T\n#+INCLUDE: \"part.org\" :minlevel 1\n",
            &[("/notes/part.org", "*** A\n**** B\n")],
        );
        assert_eq!(set, "* T\n* A\n** B\n");
    }

    #[test]
    fn includes_include_relative_to_themselves() {
        let got = text(
            "#+INCLUDE: \"sub/a.org\"\n",
            &[
                ("/notes/sub/a.org", "a\n#+INCLUDE: \"../b.org\"\n"),
                ("/notes/b.org", "b\n"),
            ],
        );
        assert_eq!(got, "a\nb\n");
    }

    #[test]
    fn blocks_wrap_and_escape_code() {
        let got = text(
            "#+INCLUDE: \"x.sh\" src sh :lines \"2-3\"\n#+INCLUDE: \"x.sh\" export html\n",
            &[("/notes/x.sh", "one\n* star\n#+keyword\nfour")],
        );
        assert_eq!(
            got,
            "#+begin_src sh\n,* star\n,#+keyword\n#+end_src\n\
             #+begin_export html\none\n* star\n#+keyword\nfour\n#+end_export\n"
        );
    }

    #[test]
    fn a_search_takes_one_subtree() {
        let file = "* One\nx\n* TODO Two :t:\nSCHEDULED: <2026-01-05 Mon>\n:PROPERTIES:\n\
                    :CUSTOM_ID: two\n:END:\nbody\n** Deep\n* Three\n";
        let by_title = text("#+INCLUDE: \"f.org::*Two\"\n", &[("/notes/f.org", file)]);
        assert!(
            by_title.starts_with("* TODO Two :t:\n") && by_title.ends_with("** Deep\n"),
            "{by_title}"
        );
        let contents = text(
            "#+INCLUDE: \"f.org::#two\" :only-contents t\n",
            &[("/notes/f.org", file)],
        );
        assert_eq!(contents, "body\n** Deep\n");
        assert_eq!(
            expand("#+INCLUDE: \"f.org::*Four\"\n", &[("/notes/f.org", file)]).err(),
            Some("org: /notes/f.org has no *Four".to_string())
        );
    }

    #[test]
    fn an_include_in_a_block_is_text() {
        let main = "#+begin_example\n#+INCLUDE: \"x.org\"\n#+end_example\n";
        assert_eq!(text(main, &[]), main);
    }

    #[test]
    fn failures_name_the_path() {
        assert_eq!(
            expand("#+INCLUDE: \"gone.org\"\n", &[]).err(),
            Some("org: cannot include /notes/gone.org: missing".to_string())
        );
        assert_eq!(
            expand(
                "#+INCLUDE: \"a.org\"\n",
                &[("/notes/a.org", "#+INCLUDE: \"./main.org\"\n")]
            )
            .err(),
            Some("org: /notes/main.org includes itself".to_string())
        );
        assert_eq!(
            expand("#+INCLUDE: \"x\" :lines \"a-b\"\n", &[]).err(),
            Some("org: cannot read #+INCLUDE: \"x\" :lines \"a-b\"".to_string())
        );
    }

    /// The including file's lines map past what its includes became, so a
    /// cursor line still finds its headline; one that became nothing still
    /// took its own line away.
    #[test]
    fn the_line_map_follows_the_splices() {
        let e = expand(
            "a\n#+INCLUDE: \"three.org\"\nb\n#+INCLUDE: \"none.org\"\nc\n",
            &[
                ("/notes/three.org", "1\n#+INCLUDE: \"two.org\"\n"),
                ("/notes/two.org", "2\n3\n"),
                ("/notes/none.org", ""),
            ],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(e.text, "a\n1\n2\n3\nb\nc\n");
        let lines: Vec<u32> = (0..5).map(|l| e.line(l)).collect();
        assert_eq!(lines, [0, 1, 4, 5, 5]);
    }
}

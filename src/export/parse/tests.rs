use super::dump::dump;
use super::*;

fn ctx() -> Context {
    Context {
        todo_keywords: ["TODO", "NEXT", "DONE"].map(String::from).to_vec(),
    }
}

/// Compare the dump of `testdata/NAME.org` with `testdata/NAME.ir`;
/// `UPDATE_GOLDEN=1` rewrites the `.ir` instead.
fn golden(name: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/export/parse/testdata");
    let org = std::fs::read_to_string(dir.join(format!("{name}.org"))).expect("corpus file");
    let got = dump(&parse(&org, &Context::for_file(&org, ctx().todo_keywords)));
    let ir = dir.join(format!("{name}.ir"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&ir, &got).expect("write golden");
        return;
    }
    let want = std::fs::read_to_string(&ir).unwrap_or_default();
    assert!(
        got == want,
        "{name}: the parse changed; UPDATE_GOLDEN=1 rewrites it after review.\n--- got\n{got}\n--- want\n{want}"
    );
}

#[test]
fn golden_headlines() {
    golden("headlines");
}

#[test]
fn golden_lists() {
    golden("lists");
}

#[test]
fn golden_blocks() {
    golden("blocks");
}

#[test]
fn golden_misc() {
    golden("misc");
}

/// The file's own `#+TODO:` replaces the configured set, as in emacs.
#[test]
fn the_files_todo_line_replaces_the_configured_keywords() {
    let text = "#+TODO: PROPOSED | ACCEPTED\n* PROPOSED a\n* TODO b\n";
    let doc = parse(text, &Context::for_file(text, ctx().todo_keywords));
    assert_eq!(doc.headlines[0].todo.as_deref(), Some("PROPOSED"));
    assert_eq!(doc.headlines[1].todo, None);
    assert_eq!(doc.headlines[1].title, "TODO b");
}

/// Parsing a file a chunk at a time, cut at headlines, builds the same tree
/// as parsing it whole: what a stepped export depends on.
fn chunked_equals_whole(text: &str, every: usize) {
    let lines: Vec<&str> = text.lines().collect();
    let heads: Vec<usize> = (0..lines.len())
        .filter(|&i| crate::headline::headline_level(lines[i]).is_some())
        .collect();
    let mut cuts: Vec<usize> = heads.iter().copied().step_by(every.max(1)).collect();
    if cuts.first() != Some(&0) {
        cuts.insert(0, 0);
    }
    cuts.push(lines.len());
    cuts.dedup();
    let mut sections = Vec::new();
    for w in cuts.windows(2) {
        sections.extend(parse_chunk(&lines[w[0]..w[1]], w[0] as u32, &ctx()));
    }
    assert_eq!(assemble(sections), parse(text, &ctx()), "{text}");
}

#[test]
fn a_file_parsed_in_chunks_is_the_file_parsed_whole() {
    for name in ["headlines", "lists", "blocks", "misc"] {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/export/parse/testdata");
        let org = std::fs::read_to_string(dir.join(format!("{name}.org"))).unwrap();
        for every in 1..4 {
            chunked_equals_whole(&org, every);
        }
    }
}

/// A tiny deterministic generator: no dependency, and a failing seed is
/// reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[(self.next() % from.len() as u64) as usize]
    }
}

/// Lines built from the fragments org's grammar turns on, so the generator
/// reaches the edge cases rather than producing paragraphs.
fn random_org(rng: &mut Rng) -> String {
    const PIECES: &[&str] = &[
        "* ",
        "** ",
        "*** TODO [#A] ",
        "COMMENT ",
        ":tag:x:",
        "- ",
        "+ ",
        "1. ",
        "2) ",
        "  - ",
        "    ",
        "[ ] ",
        "[X] ",
        "[@3] ",
        " :: ",
        "#+begin_src sh",
        "#+end_src",
        "#+BEGIN_QUOTE",
        "#+end_quote",
        "#+begin_foo",
        "#+end_foo",
        ":PROPERTIES:",
        ":ID: x",
        ":END:",
        ":LOGBOOK:",
        "| a | b |",
        "|---+---|",
        "| <l> | <r5> |",
        "#+TBLFM: $1=1",
        "+--+--+",
        "[fn:1] ",
        "[fn:x]",
        ": fixed",
        ":",
        "# note",
        "#",
        "-----",
        "\\begin{eq}",
        "\\end{eq}",
        "CLOCK: [2026-01-01]",
        "SCHEDULED: <2026-01-01 Thu>",
        "DEADLINE: [x]",
        "#+NAME: n",
        "#+CAPTION: c",
        "#+ATTR_HTML: :width 1",
        "#+RESULTS:",
        "#+TITLE: t",
        ",* x",
        ",#+y",
        "text",
        "more words",
        "é ü ✓",
        "\t",
        "",
        "",
    ];
    let lines = 1 + rng.next() % 60;
    let mut out = String::new();
    for _ in 0..lines {
        for _ in 0..(1 + rng.next() % 3) {
            out.push_str(rng.pick(PIECES));
        }
        out.push('\n');
    }
    out
}

/// No input panics the parser, and every input parses the same in chunks
/// as whole.
#[test]
fn arbitrary_text_parses_without_panicking() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..3000 {
        let text = random_org(&mut rng);
        let _ = parse(&text, &ctx());
        chunked_equals_whole(&text, 2);
    }
}

/// Nesting far past [`MAX_DEPTH`] is read, not recursed into without bound.
/// Distinct block names, because a block closes at the first `#+end_` of
/// its name: same-name blocks do not nest in org.
#[test]
fn deep_nesting_is_bounded() {
    let n = 10_000;
    let opens: String = (0..n).map(|i| format!("#+begin_b{i}\n")).collect();
    let closes: String = (0..n).rev().map(|i| format!("#+end_b{i}\n")).collect();
    let doc = parse(&format!("{opens}x\n{closes}"), &ctx());
    assert_eq!(doc.preamble.len(), 1);
    let mut depth = 0;
    let mut at = &doc.preamble[0];
    while let ElementKind::Block(b) = &at.kind {
        depth += 1;
        match b.contents.first() {
            Some(inner) => at = inner,
            None => break,
        }
    }
    assert!(depth <= MAX_DEPTH, "{depth}");
    assert!(
        matches!(&at.kind, ElementKind::Paragraph(t) if t.contains("#+begin_b") && !t.ends_with('x')),
        "past the bound the rest is text: {:?}",
        at.kind
    );

    // A list a thousand deep: each line one space further in, so the text is
    // quadratic in the depth and a deeper one only measures the copy.
    let lists: String = (0..1_000)
        .map(|i| format!("{}- x\n", " ".repeat(i)))
        .collect();
    let _ = parse(&lists, &ctx());
}

/// An unterminated block, drawer or environment costs a lookup per line,
/// not a scan: thousands of them parse in linear time.
#[test]
fn unterminated_openers_do_not_rescan_the_section() {
    let text = "#+begin_src sh\n:DRAWER:\n\\begin{x}\n".repeat(20_000);
    let start = std::time::Instant::now();
    let doc = parse(&text, &ctx());
    assert!(!doc.preamble.is_empty());
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

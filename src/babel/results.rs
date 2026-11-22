//! What a finished run writes, and where (OB.6, OB.8, design §5.2–5.3).
//!
//! Pure: lines in, an edit out. The events seam reads the buffer, finds the
//! block again by its key, and hands both here; nothing in this module knows a
//! job happened.

use serde::{Deserialize, Serialize};

use super::block::SrcBlock;

/// `:results`' collection axis: what the program printed, or the value of
/// its last expression (through a value wrapper).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Collection {
    Output,
    Value,
}

/// `:results`' handling axis: where the result goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Insertion {
    Replace,
    Append,
    Prepend,
    /// The echo area only; the file is not touched.
    Silent,
    /// Nowhere (`none`, `discard`).
    None,
}

/// `:results`' type axis: how the lines are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Kind {
    /// `verbatim`, `scalar`.
    Verbatim,
    /// `table`, `vector`: rows split on tabs, else on runs of whitespace.
    Table,
}

/// `:results`' format axis: what the lines are wrapped in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Wrap {
    /// Babel's default for the type: `: ` lines or an example block for
    /// verbatim, the table itself for a table.
    Plain,
    Raw,
    Drawer,
    /// `#+begin_src <the block's language>`.
    Code,
    /// `#+begin_src org`.
    Org,
}

/// The effective `:results`, parsed once when the run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResultsSpec {
    pub(crate) collection: Collection,
    pub(crate) insertion: Insertion,
    pub(crate) kind: Kind,
    pub(crate) wrap: Wrap,
}

impl ResultsSpec {
    /// Parse the merged `:results` words. `value_by_default` is whether the
    /// language has a value wrapper: babel's default collection is `value`,
    /// and lattice's is too exactly when it can honour it (design §5.2).
    ///
    /// A word babel has and this does not (`file`, `list`, `html`, …) is an
    /// `Err` naming it: refused visibly, never written some other way.
    pub(crate) fn parse(words: &[String], value_by_default: bool) -> Result<Self, String> {
        let mut spec = ResultsSpec {
            collection: if value_by_default {
                Collection::Value
            } else {
                Collection::Output
            },
            insertion: Insertion::Replace,
            kind: Kind::Verbatim,
            wrap: Wrap::Plain,
        };
        for w in words {
            match w.to_ascii_lowercase().as_str() {
                "output" => spec.collection = Collection::Output,
                "value" => spec.collection = Collection::Value,
                "replace" => spec.insertion = Insertion::Replace,
                "append" => spec.insertion = Insertion::Append,
                "prepend" => spec.insertion = Insertion::Prepend,
                "silent" => spec.insertion = Insertion::Silent,
                "none" | "discard" => spec.insertion = Insertion::None,
                "verbatim" | "scalar" => spec.kind = Kind::Verbatim,
                "table" | "vector" => spec.kind = Kind::Table,
                "raw" => spec.wrap = Wrap::Raw,
                "drawer" => spec.wrap = Wrap::Drawer,
                "code" => spec.wrap = Wrap::Code,
                "org" => spec.wrap = Wrap::Org,
                other => {
                    return Err(format!(
                        "org: `:results {other}` is not supported — use output or value; \
                         verbatim or table; raw, drawer, code or org; replace, append, \
                         prepend, silent or none"
                    ))
                }
            }
        }
        Ok(spec)
    }
}

/// The lines under `#+RESULTS:` for `output` as `spec` asks. `lang` is the
/// block's language, for `code`. Nothing at all for no output, in every
/// format, as babel writes it.
pub(crate) fn format(
    spec: &ResultsSpec,
    output: &[String],
    min_block: usize,
    lang: &str,
) -> Vec<String> {
    if output.is_empty() {
        return Vec::new();
    }
    let lines = || match spec.kind {
        Kind::Table => table(output),
        Kind::Verbatim => output.to_vec(),
    };
    match spec.wrap {
        Wrap::Plain => match spec.kind {
            Kind::Table => table(output),
            Kind::Verbatim => verbatim(output, min_block),
        },
        // Raw is raw: it may become structure, which is what it is for.
        Wrap::Raw => lines(),
        Wrap::Drawer => {
            let mut out = vec![":RESULTS:".to_string()];
            out.extend(lines());
            out.push(":END:".to_string());
            out
        }
        Wrap::Code | Wrap::Org => {
            let lang = if spec.wrap == Wrap::Org { "org" } else { lang };
            let mut out = vec![format!("#+begin_src {lang}")];
            out.extend(output.iter().map(|l| escape(l)));
            out.push("#+end_src".to_string());
            out
        }
    }
}

/// Whether `cell` reads as a number to org's table aligner: digits with an
/// optional sign, point and exponent.
fn is_number(cell: &str) -> bool {
    let c = cell.trim_end_matches('%');
    c.chars().any(|ch| ch.is_ascii_digit())
        && c.chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+' | 'e' | 'E'))
        && c.parse::<f64>().is_ok()
}

/// `output` as an org table, aligned as org aligns one (`org-table-align`):
/// each column as wide as its widest cell, a column right-aligned when more
/// than half its non-empty cells are numbers (`org-table-number-fraction`).
/// A row is split on tabs when it has any, else on runs of whitespace; a
/// `|` in a cell is written `\vert`, so it cannot split the cell again.
pub(crate) fn table(output: &[String]) -> Vec<String> {
    let rows: Vec<Vec<String>> = output
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let cells: Vec<&str> = if l.contains('\t') {
                l.split('\t').map(str::trim).collect()
            } else {
                l.split_whitespace().collect()
            };
            cells.iter().map(|c| c.replace('|', "\\vert")).collect()
        })
        .collect();
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let width = |i: usize| {
        rows.iter()
            .filter_map(|r| r.get(i))
            .map(|c| c.chars().count())
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<usize> = (0..cols).map(width).collect();
    let numeric: Vec<bool> = (0..cols)
        .map(|i| {
            let cells: Vec<&String> = rows
                .iter()
                .filter_map(|r| r.get(i))
                .filter(|c| !c.is_empty())
                .collect();
            let n = cells.iter().filter(|c| is_number(c)).count();
            !cells.is_empty() && n * 2 > cells.len()
        })
        .collect();
    rows.iter()
        .map(|r| {
            let cells: Vec<String> = (0..cols)
                .map(|i| {
                    let c = r.get(i).map_or("", String::as_str);
                    let pad = widths[i] - c.chars().count();
                    if numeric[i] {
                        format!("{}{c}", " ".repeat(pad))
                    } else {
                        format!("{c}{}", " ".repeat(pad))
                    }
                })
                .collect();
            format!("| {} |", cells.join(" | "))
        })
        .collect()
}

/// Org's escape for a line that would otherwise READ as org — a headline
/// (`* x`) or a keyword (`#+x`) — inside a block: one comma in front, after
/// the indentation, and a line already escaped gets one more
/// (`org-escape-code-in-string`). This is what keeps program output from
/// becoming structure in the file (design review focus #1).
pub(crate) fn escape(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let bare = rest.trim_start_matches(',');
    if bare.starts_with('*') || bare.starts_with("#+") {
        format!("{},{rest}", &line[..indent])
    } else {
        line.to_string()
    }
}

/// The lines under `#+RESULTS:` for `output`, emacs's verbatim format:
/// `: ` lines below `min_block` lines, else an example block, whose contents
/// are escaped. A `: ` line cannot become structure — it starts with a colon
/// — so only the block escapes. Nothing at all for no output.
pub(crate) fn verbatim(output: &[String], min_block: usize) -> Vec<String> {
    if output.is_empty() {
        return Vec::new();
    }
    if output.len() < min_block {
        return output
            .iter()
            .map(|l| {
                if l.is_empty() {
                    ":".to_string()
                } else {
                    format!(": {l}")
                }
            })
            .collect();
    }
    let mut out = Vec::with_capacity(output.len() + 2);
    out.push("#+begin_example".to_string());
    out.extend(output.iter().map(|l| escape(l)));
    out.push("#+end_example".to_string());
    out
}

/// The edit that puts `body` as `block`'s result the way `insertion` asks,
/// or `None` for `silent` / `none`. `append` and `prepend` add to an existing
/// result, under its one `#+RESULTS:`; with no result yet they write one,
/// as `replace` does.
pub(crate) fn insertion_edit(
    lines: &[&str],
    ends_with_newline: bool,
    block: &SrcBlock,
    body: &[String],
    insertion: Insertion,
) -> Option<ResultEdit> {
    let at_line = match (insertion, &block.results) {
        (Insertion::Silent | Insertion::None, _) => return None,
        (Insertion::Replace, _) | (_, None) => {
            return Some(result_edit(lines, ends_with_newline, block, body))
        }
        (Insertion::Append, Some(r)) => r.end_line,
        (Insertion::Prepend, Some(r)) => r.keyword_line,
    };
    if body.is_empty() {
        return None;
    }
    let joined = body.join("\n");
    let count = lines.len() as u32;
    Some(if at_line + 1 < count || ends_with_newline {
        ResultEdit {
            start: (at_line + 1, 0),
            end: (at_line + 1, 0),
            text: format!("{joined}\n"),
        }
    } else {
        // The last line of a file with no final newline: add after its end,
        // and add no newline the file did not have.
        let len = lines.get(at_line as usize).map_or(0, |l| l.len()) as u32;
        ResultEdit {
            start: (at_line, len),
            end: (at_line, len),
            text: format!("\n{joined}"),
        }
    })
}

/// One replacement: `(start, end)` as `(line, byte)` pairs, and the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResultEdit {
    pub(crate) start: (u32, u32),
    pub(crate) end: (u32, u32),
    pub(crate) text: String,
}

/// The edit that puts `body` (already formatted) as `block`'s result: over its
/// existing `#+RESULTS:` when it has one — replaced, never appended to — else
/// after `#+end_src` with one blank line between, as emacs writes it.
///
/// `lines` is the buffer, `ends_with_newline` whether its text does: the line
/// after the last one exists only then, and an edit ending there must not
/// add a newline the file did not have.
pub(crate) fn result_edit(
    lines: &[&str],
    ends_with_newline: bool,
    block: &SrcBlock,
    body: &[String],
) -> ResultEdit {
    let mut text = match &block.name {
        Some(n) => format!("#+RESULTS: {n}"),
        None => "#+RESULTS:".to_string(),
    };
    for l in body {
        text.push('\n');
        text.push_str(l);
    }
    let count = lines.len() as u32;
    // The start of the line after `last`, if there is one, or `last`'s end.
    let after = |last: u32| -> ((u32, u32), bool) {
        if last + 1 < count || ends_with_newline {
            ((last + 1, 0), true)
        } else {
            let len = lines.get(last as usize).map_or(0, |l| l.len()) as u32;
            ((last, len), false)
        }
    };
    match &block.results {
        Some(r) => {
            let (end, at_line_start) = after(r.end_line);
            if at_line_start {
                text.push('\n');
            }
            ResultEdit {
                start: (r.keyword_line, 0),
                end,
                text,
            }
        }
        None => {
            let (at, at_line_start) = after(block.end_line);
            let text = if at_line_start {
                format!("\n{text}\n")
            } else {
                format!("\n\n{text}")
            };
            ResultEdit {
                start: at,
                end: at,
                text,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::block::src_blocks;
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn few_lines_are_fixed_width_and_many_are_an_example_block() {
        assert_eq!(verbatim(&s(&["a", "", "b"]), 10), s(&[": a", ":", ": b"]));
        let ten: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let block = verbatim(&ten, 10);
        assert_eq!(block.first().map(String::as_str), Some("#+begin_example"));
        assert_eq!(block.last().map(String::as_str), Some("#+end_example"));
        assert_eq!(block.len(), 12);
        assert!(verbatim(&[], 10).is_empty());
    }

    #[test]
    fn output_that_reads_as_org_is_escaped_inside_a_block() {
        assert_eq!(escape("* Not a headline"), ",* Not a headline");
        assert_eq!(escape("  #+end_example"), "  ,#+end_example");
        assert_eq!(escape(",* already"), ",,* already");
        assert_eq!(escape("plain *star"), "plain *star");
        let out = verbatim(&s(&["* x", "#+end_example"]), 1);
        assert_eq!(
            out,
            s(&["#+begin_example", ",* x", ",#+end_example", "#+end_example"])
        );
    }

    #[test]
    fn a_first_result_goes_after_the_block_with_a_blank_line() {
        let text = "#+begin_src sh\necho hi\n#+end_src\nafter\n";
        let l: Vec<&str> = text.lines().collect();
        let b = &src_blocks(&l)[0];
        let e = result_edit(&l, true, b, &s(&[": hi"]));
        assert_eq!((e.start, e.end), ((3, 0), (3, 0)));
        assert_eq!(e.text, "\n#+RESULTS:\n: hi\n");
    }

    #[test]
    fn a_block_at_the_end_of_a_file_without_a_newline_gets_none_added() {
        let text = "#+begin_src sh\necho hi\n#+end_src";
        let l: Vec<&str> = text.lines().collect();
        let b = &src_blocks(&l)[0];
        let e = result_edit(&l, false, b, &s(&[": hi"]));
        assert_eq!((e.start, e.end), ((2, 9), (2, 9)));
        assert_eq!(e.text, "\n\n#+RESULTS:\n: hi");
    }

    #[test]
    fn an_existing_result_is_replaced_whole_and_keeps_its_name() {
        let text =
            "#+NAME: n\n#+begin_src sh\nx\n#+end_src\n\n#+RESULTS[ab]: n\n: old\n: old\nafter\n";
        let l: Vec<&str> = text.lines().collect();
        let b = &src_blocks(&l)[0];
        let e = result_edit(&l, true, b, &s(&[": new"]));
        assert_eq!((e.start, e.end), ((5, 0), (8, 0)));
        assert_eq!(e.text, "#+RESULTS: n\n: new\n");
    }

    fn words(w: &str) -> Vec<String> {
        w.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn results_parse_by_axis_and_refuse_what_is_not_built() {
        let d = ResultsSpec::parse(&words("replace"), false).unwrap();
        assert_eq!(
            (d.collection, d.insertion, d.kind, d.wrap),
            (
                Collection::Output,
                Insertion::Replace,
                Kind::Verbatim,
                Wrap::Plain
            )
        );
        assert_eq!(
            ResultsSpec::parse(&[], true).unwrap().collection,
            Collection::Value,
            "a language with a wrapper defaults to value, as babel does"
        );
        let s = ResultsSpec::parse(&words("output table drawer append"), true).unwrap();
        assert_eq!(
            (s.collection, s.insertion, s.kind, s.wrap),
            (
                Collection::Output,
                Insertion::Append,
                Kind::Table,
                Wrap::Drawer
            )
        );
        assert_eq!(
            ResultsSpec::parse(&words("discard"), false)
                .unwrap()
                .insertion,
            Insertion::None
        );
        let e = ResultsSpec::parse(&words("file"), false).unwrap_err();
        assert!(e.contains("`:results file` is not supported"), "{e}");
    }

    #[test]
    fn a_table_is_split_aligned_and_numbers_go_right() {
        let t = table(&s(&["name\tsize", "a b\t10", "", "ccc\t7"]));
        assert_eq!(
            t,
            s(&["| name | size |", "| a b  |   10 |", "| ccc  |    7 |"])
        );
        let ws = table(&s(&["x  1", "yy 22 extra"]));
        assert_eq!(ws, s(&["| x  |  1 |       |", "| yy | 22 | extra |"]));
        assert_eq!(table(&s(&["a|b"])), s(&["| a\\vertb |"]));
    }

    #[test]
    fn each_format_wraps_as_babel_writes_it() {
        let spec = |w: &str| ResultsSpec::parse(&words(w), false).unwrap();
        let out = s(&["* not a headline", "x"]);
        assert_eq!(format(&spec("raw"), &out, 10, "sh"), out);
        assert_eq!(
            format(&spec("drawer"), &out, 10, "sh"),
            s(&[":RESULTS:", "* not a headline", "x", ":END:"])
        );
        assert_eq!(
            format(&spec("code"), &out, 10, "sh"),
            s(&["#+begin_src sh", ",* not a headline", "x", "#+end_src"])
        );
        assert_eq!(
            format(&spec("org"), &out, 10, "sh")
                .first()
                .map(String::as_str),
            Some("#+begin_src org")
        );
        assert_eq!(
            format(&spec("table drawer"), &s(&["a 1"]), 10, "sh"),
            s(&[":RESULTS:", "| a | 1 |", ":END:"])
        );
        assert!(format(&spec("drawer"), &[], 10, "sh").is_empty());
    }

    #[test]
    fn append_and_prepend_add_under_the_one_keyword() {
        let text = "#+begin_src sh\nx\n#+end_src\n\n#+RESULTS:\n: old\nafter\n";
        let l: Vec<&str> = text.lines().collect();
        let b = &src_blocks(&l)[0];
        let new = s(&[": new"]);
        let a = insertion_edit(&l, true, b, &new, Insertion::Append).unwrap();
        assert_eq!(
            (a.start, a.end, a.text.as_str()),
            ((6, 0), (6, 0), ": new\n")
        );
        let p = insertion_edit(&l, true, b, &new, Insertion::Prepend).unwrap();
        assert_eq!((p.start, p.text.as_str()), ((5, 0), ": new\n"));
        assert!(insertion_edit(&l, true, b, &new, Insertion::Silent).is_none());
        assert!(insertion_edit(&l, true, b, &new, Insertion::None).is_none());
        // At the end of a file with no newline.
        let eof = "#+begin_src sh\nx\n#+end_src\n\n#+RESULTS:\n: old";
        let l: Vec<&str> = eof.lines().collect();
        let b = &src_blocks(&l)[0];
        let a = insertion_edit(&l, false, b, &new, Insertion::Append).unwrap();
        assert_eq!((a.start, a.text.as_str()), ((5, 5), "\n: new"));
    }
}

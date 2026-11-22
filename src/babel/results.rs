//! What a finished run writes, and where (OB.6, design §5.3).
//!
//! Pure: lines in, an edit out. The events seam reads the buffer, finds the
//! block again by its key, and hands both here; nothing in this module knows a
//! job happened.

use super::block::SrcBlock;

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
}

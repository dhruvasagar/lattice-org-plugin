//! Where a source block is (OB.4).
//!
//! Read from the LINES, not the syntax tree. The tree is a cache the host
//! fills off the keystroke path, so straight after an edit an action may be
//! handed none, or one a parse behind — and `C-c C-c` deciding whether it is
//! babel's from parse timing would mean the same key on the same text runs a
//! block once and falls through to org the next. The lines are always the
//! text the user sees, and a block's bounds are two marker lines: there is
//! nothing the tree knows here that the text does not.

/// A `#+begin_src` … `#+end_src` pair around a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SrcBounds {
    /// The `#+begin_src` line.
    pub(crate) begin_line: u32,
    /// The `#+end_src` line.
    pub(crate) end_line: u32,
    /// The word after `#+begin_src`, as written; empty when there is none.
    pub(crate) lang: String,
}

/// The rest of `line` after a case-insensitive `#+<marker>` that starts it
/// (after indentation), when the marker is followed by a space or the end.
fn after_marker<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let trimmed = line.trim_start();
    let head = trimmed.get(..marker.len())?;
    if !head.eq_ignore_ascii_case(marker) {
        return None;
    }
    let rest = &trimmed[marker.len()..];
    (rest.is_empty() || rest.starts_with([' ', '\t'])).then_some(rest)
}

/// `#+begin_src`'s header: everything after the marker, or `None` for any
/// other line.
pub(crate) fn begin_src_header(line: &str) -> Option<&str> {
    after_marker(line, "#+begin_src")
}

/// Whether `line` is a block's `#+end_src`.
pub(crate) fn is_end_src(line: &str) -> bool {
    after_marker(line, "#+end_src").is_some_and(|rest| rest.trim().is_empty())
}

/// A headline: one or more `*` at column 0, then a space. A block cannot
/// span one, so the search for a marker stops there.
fn is_headline(line: &str) -> bool {
    let stars = line.bytes().take_while(|b| *b == b'*').count();
    stars > 0 && line[stars..].starts_with(' ')
}

/// The source block `at` is in — on its `#+begin_src` line, its body, or its
/// `#+end_src` line — or `None`.
///
/// Bounded by the nearest headline either way, so the cost is the size of
/// the section, not of the file. An unterminated `#+begin_src` is not a block
/// (org does not run one either).
pub(crate) fn src_bounds_at(
    line: &dyn Fn(u32) -> Option<String>,
    line_count: u32,
    at: u32,
) -> Option<SrcBounds> {
    let mut begin = None;
    for i in (0..=at.min(line_count.saturating_sub(1))).rev() {
        let text = line(i)?;
        if let Some(header) = begin_src_header(&text) {
            let lang = header.split_whitespace().next().unwrap_or("").to_string();
            begin = Some((i, lang));
            break;
        }
        // An `#+end_src` above the cursor closes some other block — unless it
        // is the cursor's own line, which belongs to the block above it.
        if (is_end_src(&text) && i != at) || is_headline(&text) {
            return None;
        }
    }
    let (begin_line, lang) = begin?;
    for j in begin_line + 1..line_count {
        let text = line(j)?;
        if is_end_src(&text) {
            return (at <= j).then_some(SrcBounds {
                begin_line,
                end_line: j,
                lang,
            });
        }
        if is_headline(&text) {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(text: &str, at: u32) -> Option<SrcBounds> {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let line = |n: u32| lines.get(n as usize).cloned();
        src_bounds_at(&line, lines.len() as u32, at)
    }

    const DOC: &str = "* H\n\
                       text\n\
                       #+begin_src sh :results output\n\
                       echo hi\n\
                       #+end_src\n\
                       after\n\
                       #+BEGIN_SRC Python\n\
                       print(1)\n\
                       #+END_SRC\n";

    #[test]
    fn every_line_of_a_block_is_in_it_and_nothing_around_it_is() {
        for at in 2..=4 {
            assert_eq!(
                bounds(DOC, at),
                Some(SrcBounds {
                    begin_line: 2,
                    end_line: 4,
                    lang: "sh".to_string()
                }),
                "line {at}"
            );
        }
        for at in [0, 1, 5] {
            assert_eq!(bounds(DOC, at), None, "line {at}");
        }
    }

    #[test]
    fn markers_are_case_insensitive_and_the_language_is_kept_as_written() {
        assert_eq!(bounds(DOC, 7).map(|b| b.lang), Some("Python".to_string()));
    }

    #[test]
    fn other_blocks_and_near_misses_are_not_src_blocks() {
        let text = "#+begin_quote\nq\n#+end_quote\n#+begin_srcx\nx\n#+end_src\n";
        for at in 0..6 {
            assert_eq!(bounds(text, at), None, "line {at}");
        }
    }

    #[test]
    fn an_unterminated_block_or_one_cut_by_a_headline_is_none() {
        assert_eq!(bounds("#+begin_src sh\necho\n", 1), None);
        assert_eq!(bounds("#+begin_src sh\necho\n* H\n#+end_src\n", 1), None);
        assert_eq!(bounds("#+begin_src sh\n* H\necho\n#+end_src\n", 2), None);
    }

    #[test]
    fn a_block_with_no_language_has_an_empty_one() {
        assert_eq!(
            bounds("#+begin_src\nx\n#+end_src\n", 1).map(|b| b.lang),
            Some(String::new())
        );
    }
}

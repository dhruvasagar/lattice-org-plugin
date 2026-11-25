//! Source blocks in an export: what a block's `:exports` keeps, as emacs's
//! `org-babel-exp-do-export` decides it (lattice's `org-export.md` §4).
//!
//! | `:exports` | appears |
//! |---|---|
//! | `code` (babel's default) | the block |
//! | `results` | its result |
//! | `both` | block, then result |
//! | `none` | nothing |
//!
//! `code` leaving the result out is emacs's rule too: a block that exports
//! its code removes its result from the copy it exports.
//!
//! The header arguments are babel's, merged as a run merges them (defaults,
//! `#+PROPERTY:` and headline `header-args`, the block's own), so a block
//! exports as it runs.
//!
//! **It is read in steps.** A whole-file pass in one call would exceed what
//! one guest call may spend on a large file, so [`Scan`] reads each chunk as
//! the parse does, carrying the headlines open across chunks; a chunk starts
//! at a headline, and neither a block nor its result crosses one. A block
//! that names no `:exports` of its own waits for the file's `#+PROPERTY:`
//! lines, which may come after it, and is resolved once the parse is done.

use std::ops::RangeInclusive;
use std::rc::Rc;

use crate::babel::block::{drawer_properties, headline_level, inherit, src_blocks};
use crate::babel::header_args::HeaderArgs;

/// What a block's `:exports` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kept {
    Code,
    Results,
    Both,
    None,
}

impl Kept {
    /// `:exports` as written; a value org does not know keeps the code, the
    /// default, rather than dropping what the writer meant to show.
    pub(crate) fn parse(words: Option<&[String]>) -> Self {
        let word = words
            .and_then(|w| w.first())
            .map(|w| w.to_ascii_lowercase());
        match word.as_deref() {
            Some("results") => Kept::Results,
            Some("both") => Kept::Both,
            Some("none") => Kept::None,
            _ => Kept::Code,
        }
    }

    fn keeps_code(self) -> bool {
        matches!(self, Kept::Code | Kept::Both)
    }

    fn keeps_results(self) -> bool {
        matches!(self, Kept::Results | Kept::Both)
    }
}

/// The `header-args` properties above a point, outermost first.
type Above = Rc<[(String, String)]>;

/// One block found, with what decides its `:exports`.
struct Found {
    /// Its first line (its `#+NAME:` / `#+HEADER:` when it has them) to its
    /// `#+end_src`.
    block: RangeInclusive<u32>,
    results: Option<RangeInclusive<u32>>,
    /// `None` until resolved: a block naming its own `:exports` is resolved
    /// when found, since its own header arguments win.
    kept: Option<Kept>,
    lang: String,
    own: HeaderArgs,
    above: Above,
}

/// [`Exports`] being read, a chunk at a time.
#[derive(Default)]
pub(crate) struct Scan {
    /// The headlines open where the last chunk ended, outermost first: each
    /// one's level and its `header-args` properties.
    open: Vec<(usize, Vec<(String, String)>)>,
    /// `open`'s properties in one list, shared by the blocks under them.
    above: Above,
    found: Vec<Found>,
    /// The first block in `found` not yet resolved.
    next: usize,
}

impl Scan {
    /// Read the chunk `lines`, whose first line is `first_line` of the file.
    pub(crate) fn chunk(&mut self, lines: &[&str], first_line: u32) {
        let blocks = src_blocks(lines);
        let mut blocks = blocks.into_iter().peekable();
        for (i, line) in lines.iter().enumerate() {
            if let Some(level) = headline_level(line) {
                self.enter(level, lines, i);
            }
            while blocks.peek().is_some_and(|b| b.first_line as usize == i) {
                let Some(b) = blocks.next() else { break };
                let shift = |l: u32| l + first_line;
                let kept = b.header.get("exports").map(|w| Kept::parse(Some(w)));
                self.found.push(Found {
                    block: shift(b.first_line)..=shift(b.end_line),
                    results: b
                        .results
                        .as_ref()
                        .map(|r| shift(r.keyword_line)..=shift(r.end_line)),
                    kept,
                    lang: b.lang,
                    own: b.header,
                    above: Rc::clone(&self.above),
                });
            }
        }
    }

    /// A headline at `level`, line `at` of `lines`: it closes the headlines
    /// at its level and below, and opens itself.
    fn enter(&mut self, level: usize, lines: &[&str], at: usize) {
        let props: Vec<(String, String)> = drawer_properties(lines, at)
            .into_iter()
            .filter(|(k, _)| k.starts_with("header-args"))
            .map(|(k, v)| (k, v.to_string()))
            .collect();
        let before = self.open.iter().any(|(_, p)| !p.is_empty());
        self.open.retain(|(l, _)| *l < level);
        self.open.push((level, props));
        let after = self.open.iter().any(|(_, p)| !p.is_empty());
        // Most files set no `header-args` on a headline; they keep sharing
        // the one empty list.
        if before || after {
            self.above = self
                .open
                .iter()
                .flat_map(|(_, p)| p.iter().cloned())
                .collect::<Vec<_>>()
                .into();
        }
    }

    /// Resolve up to `count` of the blocks waiting on the file's properties,
    /// `file_props` (`#+PROPERTY:` values as written, one per line). `false`
    /// when none was waiting.
    pub(crate) fn resolve(&mut self, file_props: &[&str], count: usize) -> bool {
        let start = self.next;
        let props: Vec<(String, &str)> = file_props
            .iter()
            .map(|v| {
                let (k, rest) = v.split_once(char::is_whitespace).unwrap_or((v, ""));
                (k.to_ascii_lowercase(), rest.trim())
            })
            .collect();
        let mut done = 0;
        while let Some(found) = self.found.get_mut(self.next) {
            if done == count {
                break;
            }
            if found.kept.is_none() {
                let levels = props
                    .iter()
                    .map(|(k, v)| (k.as_str(), *v))
                    .chain(found.above.iter().map(|(k, v)| (k.as_str(), v.as_str())));
                let header = inherit(levels, &found.lang, &found.own);
                found.kept = Some(Kept::parse(header.get("exports")));
                done += 1;
            }
            self.next += 1;
        }
        self.next > start
    }

    /// What the scan found to leave out. Blocks not resolved keep `code`,
    /// the default.
    pub(crate) fn finish(self) -> Exports {
        let mut left_out = Vec::new();
        for found in self.found {
            let kept = found.kept.unwrap_or(Kept::Code);
            if !kept.keeps_code() {
                left_out.push(found.block);
            }
            if let Some(results) = found.results.filter(|_| !kept.keeps_results()) {
                left_out.push(results);
            }
        }
        Exports { left_out }
    }
}

/// What `:exports` leaves out of a file, as line spans of the text the
/// export parses: the includes are in it, so its lines are the parse's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Exports {
    /// Disjoint, in order.
    left_out: Vec<RangeInclusive<u32>>,
}

impl Exports {
    /// Whether the element starting on `line` is left out.
    pub(crate) fn leaves_out(&self, line: u32) -> bool {
        let after = self.left_out.partition_point(|r| *r.start() <= line);
        after > 0 && self.left_out[after - 1].contains(&line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` scanned a headline at a time, as the parse reads it in its
    /// smallest chunks, and resolved one block per step.
    fn exports(text: &str) -> Exports {
        let lines: Vec<&str> = text.lines().collect();
        let mut scan = Scan::default();
        let mut from = 0;
        for i in 1..=lines.len() {
            if i == lines.len() || headline_level(lines[i]).is_some() {
                scan.chunk(&lines[from..i], from as u32);
                from = i;
            }
        }
        let props: Vec<&str> = lines
            .iter()
            .filter_map(|l| l.strip_prefix("#+PROPERTY: "))
            .collect();
        while scan.resolve(&props, 1) {}
        scan.finish()
    }

    /// The 0-based lines of `text` that `:exports` leaves out.
    fn left_out(text: &str) -> Vec<u32> {
        let exports = exports(text);
        (0..text.lines().count() as u32)
            .filter(|&l| exports.leaves_out(l))
            .collect()
    }

    const BLOCK: &str = "#+begin_src sh{}\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\n\nAfter.\n";

    fn with(header: &str) -> String {
        BLOCK.replace("{}", header)
    }

    #[test]
    fn code_is_the_default_and_leaves_the_result_out() {
        assert_eq!(left_out(&with("")), [4, 5]);
        assert_eq!(left_out(&with(" :exports code")), [4, 5]);
    }

    #[test]
    fn results_leaves_the_block_out() {
        assert_eq!(left_out(&with(" :exports results")), [0, 1, 2]);
    }

    #[test]
    fn both_keeps_everything_and_none_nothing() {
        assert!(left_out(&with(" :exports both")).is_empty());
        assert_eq!(left_out(&with(" :exports none")), [0, 1, 2, 4, 5]);
    }

    /// A multi-line result is left out whole, and the block's `#+NAME:` with
    /// the block.
    #[test]
    fn the_spans_cover_names_and_whole_results() {
        let text = "#+NAME: hi\n#+begin_src sh :exports none\necho hi\n#+end_src\n\n\
                    #+RESULTS: hi\n#+begin_example\nhi\n#+end_example\n\nAfter.\n";
        assert_eq!(left_out(text), [0, 1, 2, 3, 5, 6, 7, 8]);
    }

    /// `:exports` is inherited as babel inherits it — the file's
    /// `header-args`, then each headline's, then the block's own — across
    /// the chunks the parse reads, with the file's `#+PROPERTY:` counting
    /// even below the block.
    #[test]
    fn exports_is_inherited_as_a_run_inherits_it() {
        let text = "* A\n#+begin_src sh\necho a\n#+end_src\n#+RESULTS:\n: a\n\
                    * B\n:PROPERTIES:\n:header-args:sh: :exports results\n:END:\n\
                    ** B1\n#+begin_src sh\necho b\n#+end_src\n#+RESULTS:\n: b\n\
                    #+begin_src sh :exports none\necho c\n#+end_src\n\
                    * C\n#+begin_src sh\necho d\n#+end_src\n#+RESULTS:\n: d\n\
                    #+PROPERTY: header-args :exports both\n";
        assert_eq!(left_out(text), [11, 12, 13, 16, 17, 18]);
    }

    /// A headline's `header-args` covers the headlines under it and ends at
    /// the next one at its level.
    #[test]
    fn a_headlines_header_args_end_with_its_subtree() {
        let text = "* A\n:PROPERTIES:\n:header-args: :exports none\n:END:\n\
                    ** A1\n#+begin_src sh\necho a\n#+end_src\n\
                    * B\n#+begin_src sh\necho b\n#+end_src\n";
        assert_eq!(left_out(text), [5, 6, 7]);
    }

    #[test]
    fn a_value_org_does_not_know_keeps_the_code() {
        assert_eq!(left_out(&with(" :exports sometimes")), [4, 5]);
    }

    #[test]
    fn a_file_without_source_blocks_leaves_nothing_out() {
        assert_eq!(
            exports("* A\ntext\n#+begin_example\nx\n#+end_example\n"),
            Exports::default()
        );
    }
}

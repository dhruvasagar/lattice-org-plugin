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
//! **A block exporting its result runs**, as `org-export-use-babel` runs it:
//! through babel's own runner, so only a configured language runs, with the
//! same header-argument merge, `:var` chains, noweb and sessions. It runs on
//! the export's copy of the file — its result goes into that copy and the
//! file's `#+RESULTS:` is never touched. `:eval no-export` / `never-export`
//! (and `no` / `never`) keep a block from running during an export, which
//! then uses the result already in the file.
//!
//! **It is read in steps.** A whole-file pass in one call would exceed what
//! one guest call may spend on a large file, so [`Scan`] reads each chunk as
//! the parse does, carrying the headlines open across chunks; a chunk starts
//! at a headline, and neither a block nor its result crosses one. A block
//! that names no `:exports` of its own waits for the file's `#+PROPERTY:`
//! lines, which may come after it, and is resolved once the parse is done.

use std::collections::{HashSet, VecDeque};
use std::ops::RangeInclusive;
use std::rc::Rc;

use crate::babel::block::{
    drawer_properties, effective_header_args, headline_level, inherit, src_blocks, SrcBlock,
};
use crate::babel::header_args::HeaderArgs;
use crate::babel::results::Insertion;
use crate::babel::run::RunRequest;
use crate::babel::Language;
use crate::lattice::plugin_host::types::{EchoLevel, EchoPayload, Effect};

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

/// Whether a block whose header arguments are `header` may run during an
/// export: not when its `:eval` is `no` / `never`, or `no-export` /
/// `never-export`, which say so for exports alone.
pub(crate) fn evaluates_in_export(header: &HeaderArgs) -> bool {
    let word = header
        .get("eval")
        .and_then(|w| w.first())
        .map(|w| w.to_ascii_lowercase());
    !matches!(
        word.as_deref(),
        Some("no" | "never" | "no-export" | "never-export")
    )
}

/// One block found, with what decides its `:exports`.
struct Found {
    /// The block, its lines the file's.
    src: SrcBlock,
    /// What `:exports` keeps and whether it runs; `None` until resolved.
    kept: Option<(Kept, bool)>,
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
                let Some(mut src) = blocks.next() else { break };
                src.first_line += first_line;
                src.begin_line += first_line;
                src.end_line += first_line;
                if let Some(r) = src.results.as_mut() {
                    r.keyword_line += first_line;
                    r.end_line += first_line;
                }
                self.found.push(Found {
                    src,
                    kept: None,
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
            let levels = props
                .iter()
                .map(|(k, v)| (k.as_str(), *v))
                .chain(found.above.iter().map(|(k, v)| (k.as_str(), v.as_str())));
            let header = inherit(levels, &found.src.lang, &found.src.header);
            let kept = Kept::parse(header.get("exports"));
            found.kept = Some((kept, kept.keeps_results() && evaluates_in_export(&header)));
            done += 1;
            self.next += 1;
        }
        self.next > start
    }

    /// The blocks whose result the export shows and that may run for it,
    /// in order; within `lines` (a subtree's) when given.
    pub(crate) fn runnable(&self, lines: Option<(u32, u32)>) -> Vec<SrcBlock> {
        self.found
            .iter()
            .filter(|f| f.kept.is_some_and(|(_, runs)| runs))
            .filter(|f| lines.is_none_or(|(from, to)| (from..to).contains(&f.src.begin_line)))
            .map(|f| f.src.clone())
            .collect()
    }

    /// What the scan found to leave out. Blocks not resolved keep `code`,
    /// the default.
    pub(crate) fn finish(self) -> Exports {
        let mut left_out = Vec::new();
        for found in self.found {
            let kept = found.kept.map_or(Kept::Code, |(k, _)| k);
            if !kept.keeps_code() {
                left_out.push(found.src.first_line..=found.src.end_line);
            }
            if let Some(r) = found.src.results.filter(|_| !kept.keeps_results()) {
                left_out.push(r.keyword_line..=r.end_line);
            }
        }
        Exports { left_out }
    }
}

/// How one of an export's blocks ended, as babel's jobs tell the export.
#[derive(Debug)]
pub(crate) enum Settled {
    /// It ran: its result, formatted as `#+RESULTS:` would hold it, and how
    /// that goes in.
    Ran {
        key: String,
        body: Vec<String>,
        insertion: Insertion,
    },
    /// It will not run: it could not start, what it needed failed, or its
    /// batch was stopped. The copy keeps the result it had.
    Not,
    /// It was cancelled, and the export with it.
    Cancelled,
}

/// The runs an export plans, a few blocks a step: each plan reads the
/// whole file more than once (header arguments, `:var`, noweb), so a
/// file's every block in one call could exceed a call's budget.
pub(crate) struct Planning {
    /// The blocks the export shows the results of, still to plan.
    queue: VecDeque<SrcBlock>,
    languages: Vec<Language>,
    /// The blocks planned, by `#+begin_src` line, each once.
    planned: HashSet<u32>,
    /// The names of the blocks planned, for the `:var`s of later ones.
    scheduled: Vec<String>,
    pub(crate) runs: Vec<RunRequest>,
    /// What runs, for the question.
    chosen: Vec<(SrcBlock, Language)>,
    /// The languages with no `org.babel.languages` entry, once each.
    unconfigured: Vec<String>,
    pub(crate) notes: Vec<Effect>,
}

impl Planning {
    /// The plan for `blocks`, the ones a scan found runnable, in `languages`.
    pub(crate) fn new(blocks: Vec<SrcBlock>, languages: Vec<Language>) -> Self {
        Self {
            queue: blocks.into(),
            languages,
            planned: HashSet::new(),
            scheduled: Vec::new(),
            runs: Vec::new(),
            chosen: Vec::new(),
            unconfigured: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Plan up to `count` more of the blocks, and what each needs run first
    /// (babel's `:var` chain). `Ok(true)` while blocks are left; `Err` is
    /// why the export cannot run them, as babel refuses such a run.
    pub(crate) fn step(
        &mut self,
        lines: &[&str],
        buffer: u32,
        export: u64,
        base: Option<&str>,
        count: usize,
    ) -> Result<bool, String> {
        for _ in 0..count {
            let Some(root) = self.queue.pop_front() else {
                break;
            };
            let root_line = root.begin_line;
            for block in crate::babel::run::expand(lines, std::slice::from_ref(&root))? {
                if !self.planned.insert(block.begin_line) {
                    continue;
                }
                // What a chained `:var` needs runs only as the root would:
                // its own `:eval` may keep it from running for an export.
                if block.begin_line != root_line
                    && !evaluates_in_export(&effective_header_args(lines, &block))
                {
                    continue;
                }
                let Some(lang) = self.languages.iter().find(|l| l.name == block.lang) else {
                    if !self.unconfigured.contains(&block.lang) {
                        self.unconfigured.push(block.lang.clone());
                    }
                    continue;
                };
                let mut run = crate::babel::run::request(
                    buffer,
                    lines,
                    &block,
                    lang,
                    base,
                    &mut self.scheduled,
                    &mut self.notes,
                )?;
                run.export = Some(export);
                self.runs.push(run);
                self.chosen.push((block, lang.clone()));
            }
        }
        if !self.queue.is_empty() {
            return Ok(true);
        }
        if !self.unconfigured.is_empty() {
            self.notes.push(Effect::Echo(EchoPayload {
                level: EchoLevel::Info,
                text: format!(
                    "org: the export uses the results in the file for {} — no \
                     org.babel.languages entry runs {}",
                    self.unconfigured.join(", "),
                    if self.unconfigured.len() == 1 {
                        "it"
                    } else {
                        "them"
                    }
                ),
            }));
        }
        Ok(false)
    }

    /// The question asked before the runs start.
    pub(crate) fn prompt(&self) -> String {
        let chosen: Vec<(&SrcBlock, Language)> =
            self.chosen.iter().map(|(b, l)| (b, l.clone())).collect();
        format!("Export runs {}? (y/n)", crate::babel::run::tally(&chosen))
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

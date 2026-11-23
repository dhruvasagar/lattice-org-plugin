//! Starting a run, from `C-c C-c` on the grammar seam (OB.6, design §2–§3).
//!
//! The action does no I/O. It reads the block, claims it in the store (so a
//! second `C-c C-c` is refused and the running sign can find it), and hands
//! org's own events seam the run request — where the script is written and
//! the process spawned. Not here: a grammar action runs on the host's
//! SYNCHRONOUS linker, where a WASI filesystem call panics inside
//! wasmtime-wasi, and a file write on the keystroke path is the I/O paramount
//! goal #1 keeps off it anyway.

use serde::{Deserialize, Serialize};

use super::block::{BlockKey, SrcBlock};
use super::results::{Collection, ResultsSpec};
use super::vars::Binding;
use super::wrappers::Wrapper;
use super::Language;
use crate::lattice::plugin_host::buffer::Document;
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::types::{
    ArgValue, Args, ConfirmPayload, EchoLevel, Effect, Position, Range,
};

/// The plugin event carrying a [`RunRequest`] from the grammar seam to the
/// events seam. Both ends are org; the host moves the bytes unread.
pub(crate) const EV_RUN: &str = "org/babel-run";

/// Where a running block's claim lives: `babel/run/<buffer>/<key>`.
pub(crate) const RUN_PREFIX: &str = "babel/run/";

/// The claim prefix of one buffer.
pub(crate) fn buffer_prefix(buffer: u32) -> String {
    format!("{RUN_PREFIX}{buffer}/")
}

/// The claim of one block.
pub(crate) fn claim_key(buffer: u32, key: &str) -> String {
    format!("{}{key}", buffer_prefix(buffer))
}

/// Everything the events seam needs to run a block and put its result back
/// — a run request is a LIST of these, run in order (`execute-buffer`),
/// so it never has to re-read the configuration the user may be editing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunRequest {
    pub(crate) buffer: u32,
    /// [`BlockKey::encode`].
    pub(crate) key: String,
    /// How the block is named in messages: `sh block "disk"` or `sh block`.
    pub(crate) label: String,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) ext: String,
    /// The body as written; the `:var` prelude goes ahead of it at start,
    /// once every binding is known.
    pub(crate) body: String,
    /// `#+NAME:`, which later blocks in the chain wait on (OB.12).
    pub(crate) name: Option<String>,
    /// `:var`, in order: values, or the earlier block each waits on.
    pub(crate) vars: Vec<(String, Binding)>,
    /// The block's language, for `:results code`.
    pub(crate) lang: String,
    /// The effective `:results`, parsed.
    pub(crate) results: ResultsSpec,
    /// The language's value wrapper, when `results` collects `value`.
    pub(crate) wrapper: Option<Wrapper>,
    /// `:dir` as written, quotes stripped; resolved in the events seam,
    /// where the environment (`HOME`) can be read.
    pub(crate) dir: Option<String>,
    /// The directory of the buffer's file: where a block runs without
    /// `:dir`, and what a relative `:dir` is relative to, as in emacs.
    pub(crate) base: Option<String>,
    /// `org.babel.min-lines-for-block-output`.
    pub(crate) min_block: usize,
    /// `org.babel.max-output-lines`.
    pub(crate) max_lines: usize,
}

/// The whole buffer, in one crossing.
pub(crate) fn whole_text(doc: &Document) -> String {
    let count = doc.line_count();
    if count == 0 {
        return String::new();
    }
    let last = count - 1;
    let end = doc.line(last).map_or(0, |l| l.len() as u32);
    doc.get_text_range(Range {
        start: Position { line: 0, byte: 0 },
        end: Position {
            line: last,
            byte: end,
        },
    })
    .unwrap_or_default()
}

/// How a block is named to the user.
pub(crate) fn label(block: &SrcBlock) -> String {
    match &block.name {
        Some(n) => format!("{} block \"{n}\"", block.lang),
        None => format!("{} block", block.lang),
    }
}

/// Whether `block` already has a run in flight. Claims are matched by
/// finding their block again, not by comparing keys: an unnamed block's key
/// records the line it was on, and the user may have typed above it since.
pub(crate) fn is_running(buffer: u32, lines: &[&str], block: &SrcBlock) -> bool {
    let prefix = buffer_prefix(buffer);
    host_services::store_keys(&prefix).iter().any(|k| {
        k.strip_prefix(&prefix)
            .and_then(BlockKey::decode)
            .and_then(|key| key.find(lines))
            .is_some_and(|b| b.begin_line == block.begin_line)
    })
}

fn number_option(name: &str, default: usize) -> usize {
    crate::option_or(name, &default.to_string())
        .trim()
        .parse()
        .unwrap_or(default)
}

/// The plugin event that asks the events seam to stop this buffer's runs;
/// the payload is the buffer id, as text.
pub(crate) const EV_CANCEL: &str = "org/babel-cancel";

thread_local! {
    /// Whether this session has said that a language without a value
    /// wrapper runs `:results value` as output — said once (design §5.2).
    static VALUE_NOTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// What a block's header and language make of a run.
pub(crate) struct Plan {
    pub(crate) results: ResultsSpec,
    pub(crate) wrapper: Option<Wrapper>,
    /// `value` was asked for and cannot be had, so it runs as output.
    pub(crate) fell_back: bool,
    /// `:var` (OB.11): values, or blocks earlier in the chain (OB.12).
    pub(crate) vars: Vec<(String, Binding)>,
    /// The body as it runs: noweb-expanded when `:noweb` says so (OB.13).
    pub(crate) body: String,
}

/// What a block's `:results`, `:var` and language make of a run, or why it
/// cannot run. `scheduled` names the blocks running before it in the same
/// chain, whose fresh results its `:var`s take.
pub(crate) fn plan(
    lines: &[&str],
    block: &SrcBlock,
    lang: &Language,
    scheduled: &[String],
) -> Result<Plan, String> {
    let wrapper = match &lang.value {
        None => None,
        Some(name) => Some(Wrapper::named(name).ok_or_else(|| {
            format!(
                "org: no value wrapper \"{name}\" for {} — python, ruby, node or sh",
                lang.name
            )
        })?),
    };
    let header = super::block::effective_header_args(lines, block);
    let mut spec = ResultsSpec::parse(&header.results(), wrapper.is_some())?;
    let fell_back = spec.collection == Collection::Value && wrapper.is_none();
    if fell_back {
        spec.collection = Collection::Output;
    }
    // Resolved now, on the grammar seam, from the text the user sees: a bad
    // reference refuses before the confirm, and nothing runs.
    let vars = super::vars::resolve(lines, &header, scheduled)?;
    if !vars.is_empty() && wrapper.is_none() {
        return Err(format!(
            "org: {} cannot take `:var` — name its language in its org.babel.languages \
             entry: `value = \"sh\"` (or python, ruby, node)",
            lang.name
        ));
    }
    let body = super::noweb::expand(lines, block, super::noweb::Context::Run)?;
    Ok(Plan {
        results: spec,
        wrapper,
        fell_back,
        vars,
        body,
    })
}

/// The directory of `doc`'s file, if it has one.
pub(crate) fn base_dir(doc: &Document) -> Option<String> {
    let path = doc.path()?;
    let (dir, _) = path.rsplit_once('/')?;
    Some(if dir.is_empty() {
        "/".to_string()
    } else {
        dir.to_string()
    })
}

/// The blocks to run, checked: each in a configured language, none already
/// running. `Err` is the echo saying why not.
pub(crate) fn check<'a>(
    buffer: u32,
    lines: &[&str],
    blocks: &'a [SrcBlock],
) -> Result<Vec<(&'a SrcBlock, Language)>, Vec<Effect>> {
    if blocks.is_empty() {
        return Err(super::echo(EchoLevel::Info, "org: no source blocks here"));
    }
    let languages = super::languages().map_err(|e| super::echo(EchoLevel::Error, e))?;
    let mut out = Vec::with_capacity(blocks.len());
    let mut scheduled = Vec::new();
    for block in blocks {
        // Emacs refuses a buffer run that meets a language it cannot run,
        // rather than skipping it; so does this.
        let Some(lang) = languages.iter().find(|l| l.name == block.lang) else {
            return Err(super::unconfigured(&block.lang));
        };
        if let Err(e) = plan(lines, block, lang, &scheduled) {
            return Err(super::echo(EchoLevel::Error, e));
        }
        scheduled.extend(block.name.clone());
        if is_running(buffer, lines, block) {
            return Err(super::echo(
                EchoLevel::Info,
                format!(
                    "org: the {} is already running — :org-babel-cancel stops it",
                    label(block)
                ),
            ));
        }
        out.push((block, lang.clone()));
    }
    Ok(out)
}

/// The confirm prompt for `blocks`: `Run sh block "disk"? (y/n)` for one,
/// `Run 4 blocks (sh ×3, python)? (y/n)` for several — a count per language,
/// in the order they first appear.
pub(crate) fn prompt(blocks: &[(&SrcBlock, Language)]) -> String {
    if let [(block, _)] = blocks {
        return format!("Run {}? (y/n)", label(block));
    }
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for (b, _) in blocks {
        match counts.iter_mut().find(|(l, _)| *l == b.lang) {
            Some(c) => c.1 += 1,
            None => counts.push((&b.lang, 1)),
        }
    }
    let langs: Vec<String> = counts
        .iter()
        .map(|(l, n)| {
            if *n == 1 {
                l.to_string()
            } else {
                format!("{l} ×{n}")
            }
        })
        .collect();
    format!("Run {} blocks ({})? (y/n)", blocks.len(), langs.join(", "))
}

/// The keys of `blocks`, packed into the confirm's ONE argument: a confirm's
/// arguments are cut to the yes-action's declared arity, and a block name
/// cannot hold a newline, so one string with one key per line is lossless.
pub(crate) fn pack(lines: &[&str], blocks: &[(&SrcBlock, Language)]) -> String {
    blocks
        .iter()
        .map(|(b, _)| BlockKey::of(lines, b).encode())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The blocks `packed` names, found again in `lines` as they are now — the
/// user may have edited while the prompt was open — or `None` if any is gone.
pub(crate) fn unpack(lines: &[&str], packed: &str) -> Option<Vec<SrcBlock>> {
    packed
        .lines()
        .map(|k| BlockKey::decode(k)?.find(lines))
        .collect()
}

/// `blocks` with what they need run first (OB.12): a block a `:var` names
/// that has no `#+RESULTS:`, or that is in this run anyway, runs before the
/// block naming it, and each block runs once. A cycle over names refuses the
/// whole run before anything starts.
pub(crate) fn expand(lines: &[&str], blocks: &[SrcBlock]) -> Result<Vec<SrcBlock>, String> {
    fn visit(
        lines: &[&str],
        all: &[SrcBlock],
        asked: &[SrcBlock],
        block: &SrcBlock,
        order: &mut Vec<SrcBlock>,
        path: &mut Vec<SrcBlock>,
    ) -> Result<(), String> {
        let same = |b: &SrcBlock| b.begin_line == block.begin_line;
        if order.iter().any(same) {
            return Ok(());
        }
        if let Some(at) = path.iter().position(same) {
            let names: Vec<String> = path[at..]
                .iter()
                .chain(std::iter::once(block))
                .map(|b| b.name.clone().unwrap_or_else(|| label(b)))
                .collect();
            return Err(format!(
                "org: these blocks need each other's results ({}); nothing ran",
                names.join(" → ")
            ));
        }
        path.push(block.clone());
        let header = super::block::effective_header_args(lines, block);
        for name in super::vars::references(&header) {
            let Some(dep) = all.iter().find(|b| b.name.as_deref() == Some(&name)) else {
                continue;
            };
            let in_run = asked.iter().any(|b| b.begin_line == dep.begin_line);
            if dep.results.is_none() || in_run {
                visit(lines, all, asked, dep, order, path)?;
            }
        }
        path.pop();
        order.push(block.clone());
        Ok(())
    }
    let all = super::block::src_blocks(lines);
    let mut order = Vec::new();
    for b in blocks {
        visit(lines, &all, blocks, b, &mut order, &mut Vec::new())?;
    }
    Ok(order)
}

/// Ask before running `blocks` — or, with `org.babel.confirm` off, run them.
/// What they need is run first, and asked about in the same question.
pub(crate) fn confirm_or_start(
    buffer: u32,
    lines: &[&str],
    blocks: &[SrcBlock],
    base: Option<&str>,
) -> Vec<Effect> {
    let blocks = match expand(lines, blocks) {
        Ok(b) => b,
        Err(e) => return super::echo(EchoLevel::Error, e),
    };
    let checked = match check(buffer, lines, &blocks) {
        Ok(c) => c,
        Err(echo) => return echo,
    };
    if !crate::option_or("babel.confirm", "true").eq_ignore_ascii_case("true") {
        return start(buffer, lines, &checked, base);
    }
    vec![Effect::Confirm(ConfirmPayload {
        prompt: prompt(&checked),
        yes_action: super::RUN_CONFIRMED_NAME.to_string(),
        args: Args::List(vec![ArgValue::String(pack(lines, &checked))]),
    })]
}

/// Claim each block and ask the events seam to run them, in order, one
/// after another.
pub(crate) fn start(
    buffer: u32,
    lines: &[&str],
    blocks: &[(&SrcBlock, Language)],
    base: Option<&str>,
) -> Vec<Effect> {
    let min_block = number_option("babel.min-lines-for-block-output", 10);
    let max_lines = number_option("babel.max-output-lines", 10_000);
    let mut batch = Vec::with_capacity(blocks.len());
    let mut notes = Vec::new();
    let mut scheduled = Vec::new();
    for (block, lang) in blocks {
        let key = BlockKey::of(lines, block).encode();
        let Plan {
            results,
            wrapper,
            fell_back,
            vars,
            body,
        } = match plan(lines, block, lang, &scheduled) {
            Ok(p) => p,
            Err(e) => return super::echo(EchoLevel::Error, e),
        };
        scheduled.extend(block.name.clone());
        if fell_back && !VALUE_NOTED.with(|n| n.replace(true)) {
            notes.extend(super::echo(
                EchoLevel::Info,
                format!(
                    "org: {} has no value wrapper, so `:results value` runs as output \
                     — name one in its org.babel.languages entry",
                    lang.name
                ),
            ));
        }
        let header = super::block::effective_header_args(lines, block);
        let dir = header
            .get("dir")
            .map(|w| w.join(" ").trim_matches('"').to_string());
        batch.push(RunRequest {
            buffer,
            key,
            label: label(block),
            command: lang.command.clone(),
            args: lang
                .args
                .clone()
                .unwrap_or_else(|| vec!["{file}".to_string()]),
            ext: lang.ext.clone().unwrap_or_else(|| lang.name.clone()),
            body,
            name: block.name.clone(),
            vars,
            lang: block.lang.clone(),
            results,
            wrapper,
            dir,
            base: base.map(str::to_string),
            min_block,
            max_lines,
        });
    }
    let bytes = match rmp_serde::to_vec(&batch) {
        Ok(b) => b,
        Err(e) => return super::echo(EchoLevel::Error, format!("org: cannot start: {e}")),
    };
    for request in &batch {
        if let Err(e) = host_services::store_put(&claim_key(buffer, &request.key), &[]) {
            for r in &batch {
                let _ = host_services::store_delete(&claim_key(buffer, &r.key));
            }
            return super::echo(
                EchoLevel::Error,
                format!("org: cannot start the {}: {e}", request.label),
            );
        }
    }
    host_services::emit_event(EV_RUN, &bytes);
    // The claims are what the sign producer reads, so the signs are up
    // before the first process is.
    host_services::refresh_decorations();
    if notes.is_empty() {
        vec![Effect::None]
    } else {
        notes
    }
}

/// `:org-babel-cancel`: the events seam owns the jobs, so ask it.
pub(crate) fn cancel(buffer: u32) -> Vec<Effect> {
    let running = host_services::store_keys(&buffer_prefix(buffer)).len();
    if running == 0 {
        return super::echo(EchoLevel::Info, "org: no source block is running here");
    }
    host_services::emit_event(EV_CANCEL, buffer.to_string().as_bytes());
    let what = if running == 1 {
        "1 block".to_string()
    } else {
        format!("{running} blocks")
    };
    super::echo(EchoLevel::Info, format!("org: cancelling {what}"))
}

#[cfg(test)]
mod tests {
    use super::super::block::src_blocks;
    use super::*;

    fn names(blocks: &[SrcBlock]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| b.name.clone().unwrap_or_default())
            .collect()
    }

    fn named(lines: &[&str], name: &str) -> SrcBlock {
        src_blocks(lines)
            .into_iter()
            .find(|b| b.name.as_deref() == Some(name))
            .unwrap()
    }

    #[test]
    fn what_a_block_needs_runs_first_and_once() {
        let doc = "#+NAME: a\n#+begin_src sh\nx\n#+end_src\n\
                   #+NAME: b\n#+begin_src sh :var x=a\nx\n#+end_src\n\
                   #+NAME: c\n#+begin_src sh :var y=b z=a\nx\n#+end_src\n";
        let lines: Vec<&str> = doc.lines().collect();
        let order = expand(&lines, &[named(&lines, "c")]).unwrap();
        assert_eq!(names(&order), ["a", "b", "c"]);
        // The whole buffer: `a` and `b` are in the run already, and run once.
        let order = expand(&lines, &src_blocks(&lines)).unwrap();
        assert_eq!(names(&order), ["a", "b", "c"]);
    }

    #[test]
    fn a_result_in_the_file_is_read_not_rerun() {
        let doc = "#+NAME: a\n#+begin_src sh\nx\n#+end_src\n\n#+RESULTS: a\n: 1\n\n\
                   #+NAME: b\n#+begin_src sh :var x=a\nx\n#+end_src\n";
        let lines: Vec<&str> = doc.lines().collect();
        assert_eq!(
            names(&expand(&lines, &[named(&lines, "b")]).unwrap()),
            ["b"]
        );
    }

    #[test]
    fn a_later_block_in_the_run_moves_before_what_needs_it() {
        let doc = "#+NAME: b\n#+begin_src sh :var x=a\nx\n#+end_src\n\
                   #+NAME: a\n#+begin_src sh\nx\n#+end_src\n\n#+RESULTS: a\n: old\n";
        let lines: Vec<&str> = doc.lines().collect();
        assert_eq!(
            names(&expand(&lines, &src_blocks(&lines)).unwrap()),
            ["a", "b"],
            "`a` runs in this run anyway, so `b` takes its fresh result"
        );
    }

    #[test]
    fn a_cycle_names_itself() {
        let doc = "#+NAME: a\n#+begin_src sh :var x=b\nx\n#+end_src\n\
                   #+NAME: b\n#+begin_src sh :var y=a\nx\n#+end_src\n";
        let lines: Vec<&str> = doc.lines().collect();
        let e = expand(&lines, &[named(&lines, "a")]).unwrap_err();
        assert!(e.contains("a → b → a"), "{e}");
        assert!(e.contains("nothing ran"), "{e}");
    }

    fn lang(name: &str) -> Language {
        Language {
            name: name.to_string(),
            command: name.to_string(),
            args: None,
            ext: None,
            value: None,
        }
    }

    #[test]
    fn the_prompt_names_one_block_or_counts_several_by_language() {
        let text = "#+NAME: disk\n#+begin_src sh\nx\n#+end_src\n#+begin_src sh\ny\n#+end_src\n\
                    #+begin_src python\nz\n#+end_src\n#+begin_src sh\nw\n#+end_src\n";
        let l: Vec<&str> = text.lines().collect();
        let blocks = src_blocks(&l);
        let one = [(&blocks[0], lang("sh"))];
        assert_eq!(prompt(&one), "Run sh block \"disk\"? (y/n)");
        let all: Vec<_> = blocks.iter().map(|b| (b, lang(&b.lang))).collect();
        assert_eq!(prompt(&all), "Run 4 blocks (sh ×3, python)? (y/n)");
    }

    #[test]
    fn packed_keys_find_their_blocks_after_an_edit_above() {
        let text = "#+begin_src sh\nx\n#+end_src\n#+begin_src sh\nx\n#+end_src\n";
        let l: Vec<&str> = text.lines().collect();
        let blocks = src_blocks(&l);
        let checked: Vec<_> = blocks.iter().map(|b| (b, lang("sh"))).collect();
        let packed = pack(&l, &checked);
        let moved = format!("new\n{text}");
        let l2: Vec<&str> = moved.lines().collect();
        let found = unpack(&l2, &packed).unwrap();
        assert_eq!(
            found.iter().map(|b| b.begin_line).collect::<Vec<_>>(),
            [1, 4]
        );
        assert!(unpack(&l2, "n:gone").is_none());
    }
}

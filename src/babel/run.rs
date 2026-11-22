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
use super::header_args::HeaderArgs;
use super::Language;
use crate::lattice::plugin_host::buffer::Document;
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::types::{EchoLevel, Effect, Position, Range};

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

/// Everything the events seam needs to run a block and put its result back,
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
    pub(crate) body: String,
    /// The effective `:results` words, for OB.8's formats.
    pub(crate) results: Vec<String>,
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

/// Claim `block` and ask the events seam to run it.
pub(crate) fn start(
    buffer: u32,
    lines: &[&str],
    block: &SrcBlock,
    lang: &Language,
    header: &HeaderArgs,
) -> Vec<Effect> {
    let key = BlockKey::of(lines, block).encode();
    let claim = claim_key(buffer, &key);
    if let Err(e) = host_services::store_put(&claim, &[]) {
        return super::echo(
            EchoLevel::Error,
            format!("org: cannot start the {}: {e}", label(block)),
        );
    }
    let request = RunRequest {
        buffer,
        key,
        label: label(block),
        command: lang.command.clone(),
        args: lang
            .args
            .clone()
            .unwrap_or_else(|| vec!["{file}".to_string()]),
        ext: lang.ext.clone().unwrap_or_else(|| lang.name.clone()),
        body: block.body.clone(),
        results: header.results(),
        min_block: number_option("babel.min-lines-for-block-output", 10),
        max_lines: number_option("babel.max-output-lines", 10_000),
    };
    let bytes = match rmp_serde::to_vec(&request) {
        Ok(b) => b,
        Err(e) => {
            let _ = host_services::store_delete(&claim);
            return super::echo(
                EchoLevel::Error,
                format!("org: cannot start the block: {e}"),
            );
        }
    };
    host_services::emit_event(EV_RUN, &bytes);
    // The claim is what the sign producer reads, so the sign is up before
    // the process is.
    host_services::refresh_decorations();
    vec![Effect::None]
}

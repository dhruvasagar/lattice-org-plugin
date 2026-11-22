//! The events seam's half of a run (OB.6, design §2–§5.3): write the script,
//! spawn it, gather its output, and put the result back where the block is
//! NOW — or, when it is not there any more, in `*org-babel*`.
//!
//! The job table is this instance's memory. Every job this plugin starts is
//! started here, and every `job-output` / `job-finished` for it arrives here,
//! so nothing about a live job needs the store; the store holds only the
//! claim, which the grammar seam and the sign producer read.

use std::cell::RefCell;
use std::collections::HashMap;

use super::block::BlockKey;
use super::results::{result_edit, verbatim};
use super::run::{claim_key, RunRequest, RUN_PREFIX};
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::types::{
    ApplyEditPayload, EchoLevel, EchoPayload, Edit, EditKind, Effect, GutterDecoration, GutterSign,
    Position, Range,
};

/// The output buffer a result goes to when its block cannot be found.
pub(crate) const OUTPUT_BUFFER: &str = "*org-babel*";

/// The sign on a running block's `#+begin_src`, before the host's `org.`
/// namespacing — and its namespaced name, which a placement must use.
pub(crate) const RUNNING_SIGN: &str = "babel-running";
pub(crate) const RUNNING_SIGN_NS: &str = "org.babel-running";

/// Where scripts are written, as the guest sees it.
const SCRIPT_DIR: &str = "/data/babel";

struct Job {
    request: RunRequest,
    /// The guest path of the script, removed at finish.
    script: String,
    output: Vec<String>,
    /// Lines past `max_lines`, counted and dropped.
    dropped: usize,
}

thread_local! {
    static JOBS: RefCell<HashMap<u64, Job>> = RefCell::new(HashMap::new());
}

fn echo(level: EchoLevel, text: String) -> Effect {
    Effect::Echo(EchoPayload { level, text })
}

fn post(effects: Vec<Effect>) {
    // `err` only when the editor wired nowhere to post to — then there is
    // no one to tell either.
    let _ = host_services::post_effects(&effects);
}

/// `register-events`: no job survives a load, so neither does its script or
/// its claim. A crash, an unload or a reload (editing the languages table
/// reloads the plugin) would otherwise leave a block that reads as running
/// forever.
pub(crate) fn sweep() {
    if let Ok(entries) = std::fs::read_dir(SCRIPT_DIR) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    for key in host_services::store_keys(RUN_PREFIX) {
        let _ = host_services::store_delete(&key);
    }
}

/// A run request from `C-c C-c`: write the script and spawn it.
pub(crate) fn on_run(payload: &[u8]) {
    let Ok(request) = rmp_serde::from_slice::<RunRequest>(payload) else {
        return;
    };
    let claim = claim_key(request.buffer, &request.key);
    let fail = |text: String| {
        let _ = host_services::store_delete(&claim);
        host_services::refresh_decorations();
        post(vec![echo(EchoLevel::Error, text)]);
    };
    let Some(host_dir) = host_services::data_dir() else {
        return fail("org: babel needs this plugin's data directory, and it has none".to_string());
    };
    let uuid = match host_services::new_uuid() {
        Ok(u) => u,
        Err(e) => return fail(format!("org: cannot name the script: {e}")),
    };
    let file = format!("{uuid}.{}", request.ext);
    let script = format!("{SCRIPT_DIR}/{file}");
    let mut body = request.body.clone();
    body.push('\n');
    if let Err(e) = std::fs::create_dir_all(SCRIPT_DIR).and_then(|_| std::fs::write(&script, body))
    {
        return fail(format!(
            "org: cannot write the {} script: {e}",
            request.label
        ));
    }
    let host_file = format!("{}/babel/{file}", host_dir.trim_end_matches('/'));
    let args: Vec<String> = request
        .args
        .iter()
        .map(|a| a.replace("{file}", &host_file))
        .collect();
    match host_services::spawn_process(&request.command, &args, "") {
        Ok(id) => JOBS.with(|j| {
            j.borrow_mut().insert(
                id,
                Job {
                    request,
                    script,
                    output: Vec::new(),
                    dropped: 0,
                },
            );
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&script);
            fail(format!("org: the {} did not start: {e}", request.label));
        }
    }
}

/// `job-output`: keep the lines, up to the cap.
pub(crate) fn on_output(id: u64, lines: Vec<String>) {
    JOBS.with(|j| {
        if let Some(job) = j.borrow_mut().get_mut(&id) {
            for line in lines {
                if job.output.len() < job.request.max_lines {
                    job.output.push(line);
                } else {
                    job.dropped += 1;
                }
            }
        }
    });
}

/// `job-finished`: put the result back, or say where it went.
pub(crate) fn on_finished(id: u64, outcome: Result<(), String>) {
    let Some(job) = JOBS.with(|j| j.borrow_mut().remove(&id)) else {
        return;
    };
    let _ = std::fs::remove_file(&job.script);
    let _ = host_services::store_delete(&claim_key(job.request.buffer, &job.request.key));
    host_services::refresh_decorations();

    let label = job.request.label.clone();
    let mut output = job.output;
    if job.dropped > 0 {
        output.push(format!("… {} lines truncated", job.dropped));
    }
    let mut effects = Vec::new();
    if let Err(e) = &outcome {
        // A program that never started has written nothing; its reason is
        // the result, so the block shows why (design §3).
        if output.is_empty() {
            output.push(e.clone());
        }
        effects.push(echo(
            EchoLevel::Warn,
            format!("org: the {label} failed: {e}"),
        ));
    }
    match place(&job.request, &output) {
        Ok(edit) => effects.insert(0, edit),
        Err(why) => {
            let mut lines = vec![format!("── {label}: {why} ──")];
            lines.extend(output);
            let _ = host_services::output_append(OUTPUT_BUFFER, &lines);
            effects.push(echo(
                EchoLevel::Warn,
                format!("org: {why} — the {label}'s output is in {OUTPUT_BUFFER}"),
            ));
        }
    }
    post(effects);
}

/// The edit that writes `output` as the block's result, or why there is none.
fn place(request: &RunRequest, output: &[String]) -> Result<Effect, String> {
    let text = host_services::buffer_text(request.buffer)
        .ok_or_else(|| "its buffer closed while it ran".to_string())?;
    let lines: Vec<&str> = text.lines().collect();
    let block = BlockKey::decode(&request.key)
        .and_then(|k| k.find(&lines))
        .ok_or_else(|| "the block changed while it ran".to_string())?;
    let body = verbatim(output, request.min_block);
    let e = result_edit(&lines, text.ends_with('\n'), &block, &body);
    Ok(Effect::ApplyEdit(ApplyEditPayload {
        target: request.buffer,
        edit: Edit {
            range: Range {
                start: Position {
                    line: e.start.0,
                    byte: e.start.1,
                },
                end: Position {
                    line: e.end.0,
                    byte: e.end.1,
                },
            },
            kind: EditKind::Replace(e.text),
        },
        cursor: None,
    }))
}

/// The running signs for `buffer`: one per claimed block that is still there.
/// Free when nothing in this buffer runs — one store listing — which is the
/// case on almost every call, since the producer is asked about every buffer
/// on every edit.
pub(crate) fn running_signs(buffer: u64) -> Vec<GutterDecoration> {
    let Ok(buffer) = u32::try_from(buffer) else {
        return Vec::new();
    };
    let prefix = super::run::buffer_prefix(buffer);
    let keys = host_services::store_keys(&prefix);
    if keys.is_empty() {
        return Vec::new();
    }
    let Some(text) = host_services::buffer_text(buffer) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    keys.iter()
        .filter_map(|k| BlockKey::decode(k.strip_prefix(&prefix)?)?.find(&lines))
        .map(|b| {
            GutterDecoration::Sign(GutterSign {
                line: b.begin_line,
                name: RUNNING_SIGN_NS.to_string(),
            })
        })
        .collect()
}

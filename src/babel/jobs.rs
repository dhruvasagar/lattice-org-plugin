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
use super::results::{format, insertion_edit, Collection, Insertion};
use super::run::{claim_key, RunRequest, RUN_PREFIX};
use super::vars::{prelude, value_of_result, Binding, Value};
use super::wrappers::after_sentinel;
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
    /// The rest of its batch, started when this one finishes.
    rest: Vec<RunRequest>,
    /// The line a value wrapper prints before the value, if it was wrapped.
    sentinel: Option<String>,
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

/// A run request from the grammar seam: a batch, run in order.
pub(crate) fn on_run(payload: &[u8]) {
    let Ok(batch) = rmp_serde::from_slice::<Vec<RunRequest>>(payload) else {
        return;
    };
    start_next(batch);
}

/// Forget the claims of runs that will not start now.
fn release(runs: &[RunRequest]) {
    for r in runs {
        let _ = host_services::store_delete(&claim_key(r.buffer, &r.key));
    }
}

/// The values of `request`'s `:var`s, or the block one still waits on —
/// which can only mean that block did not finish well, since every block a
/// chain waits on runs before the blocks waiting.
fn ready_vars(request: &RunRequest) -> Result<Vec<(String, Value)>, String> {
    request
        .vars
        .iter()
        .map(|(name, b)| match b {
            Binding::Ready(v) => Ok((name.clone(), v.clone())),
            Binding::After(block) => Err(block.clone()),
        })
        .collect()
}

/// Start the first of `runs`; the others wait for it to finish. A run that
/// cannot start stops the batch — what follows it was meant to follow it.
/// A run whose `:var` waits on a block that failed is skipped, and says so;
/// the rest go on, and any waiting on IT are skipped in turn (OB.12).
fn start_next(mut runs: Vec<RunRequest>) {
    let mut skipped = Vec::new();
    let (request, vars) = loop {
        if runs.is_empty() {
            if !skipped.is_empty() {
                host_services::refresh_decorations();
                post(skipped);
            }
            return;
        }
        let request = runs.remove(0);
        match ready_vars(&request) {
            Ok(vars) => break (request, vars),
            Err(needed) => {
                release(std::slice::from_ref(&request));
                skipped.push(echo(
                    EchoLevel::Warn,
                    format!(
                        "org: the {} did not run — it needs `{needed}`'s result, and `{needed}` \
                         did not finish",
                        request.label
                    ),
                ));
            }
        }
    };
    if !skipped.is_empty() {
        host_services::refresh_decorations();
        post(skipped);
    }
    let rest = runs;
    let fail = |text: String| {
        release(std::slice::from_ref(&request));
        release(&rest);
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
    // `:results value`: the body runs inside its wrapper, which prints the
    // value after a line no program prints by accident — this run's uuid.
    // `:var` (OB.11): assignments ahead of the body, inside any wrapper, so
    // `:results value` sees them. `plan` refused `:var` without a wrapper.
    let source = match request.wrapper {
        Some(w) => format!("{}{}", prelude(w, &vars), request.body),
        None => request.body.clone(),
    };
    let (mut body, sentinel) = match (request.results.collection, request.wrapper) {
        (Collection::Value, Some(w)) => {
            let sentinel = format!("org-babel-value-{uuid}");
            let (script, cut) = w.wrap(&source, &sentinel);
            (script, cut.then_some(sentinel))
        }
        _ => (source, None),
    };
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
    let cwd = working_dir(request.dir.as_deref(), request.base.as_deref());
    match host_services::spawn_process(&request.command, &args, &cwd) {
        Ok(id) => JOBS.with(|j| {
            j.borrow_mut().insert(
                id,
                Job {
                    request,
                    rest,
                    sentinel,
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

/// Where a block runs: `:dir`, tilde-expanded, against the file's directory
/// when relative; without `:dir`, the file's directory; with neither, the
/// editor's (an empty `cwd`). A `:dir` that is not a directory is
/// `spawn-process`'s refusal, reported as the block not starting.
fn working_dir(dir: Option<&str>, base: Option<&str>) -> String {
    match (dir.map(crate::roam_scan::expand_tilde), base) {
        (Some(d), Some(b)) if !d.starts_with('/') => format!("{}/{d}", b.trim_end_matches('/')),
        (Some(d), _) => d,
        (None, Some(b)) => b.to_string(),
        (None, None) => String::new(),
    }
}

/// `:org-babel-cancel` reached us: kill this buffer's jobs and drop what
/// was queued behind them. Each killed job still reports `job-finished`
/// (`err("cancelled")`), which clears its claim and leaves its result alone.
pub(crate) fn on_cancel(payload: &[u8]) {
    let Ok(buffer) = String::from_utf8_lossy(payload).parse::<u32>() else {
        return;
    };
    let ids: Vec<u64> = JOBS.with(|j| {
        let mut jobs = j.borrow_mut();
        jobs.iter_mut()
            .filter(|(_, job)| job.request.buffer == buffer)
            .map(|(id, job)| {
                release(&job.rest);
                job.rest.clear();
                *id
            })
            .collect()
    });
    for id in ids {
        host_services::cancel_job(id);
    }
    host_services::refresh_decorations();
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
    // Cancelled: the user asked for nothing to happen, so nothing is
    // written — the block keeps the result it had.
    if outcome.as_ref().is_err_and(|e| e == "cancelled") {
        post(vec![echo(
            EchoLevel::Info,
            format!("org: the {label} was cancelled"),
        )]);
        return;
    }
    let mut rest = job.rest;
    let mut output = match &job.sentinel {
        // No sentinel means the body failed before its value: the whole
        // output is what says why.
        Some(s) => after_sentinel(&job.output, s).unwrap_or(job.output),
        None => job.output,
    };
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
    // OB.12: a block later in the chain waits on this one's result. It gets
    // what `#+RESULTS:` would now say, read the way a result in the file is.
    // A failed run fills nothing, and what waited on it is skipped.
    if let (Ok(()), Some(name)) = (&outcome, &job.request.name) {
        let r = &job.request;
        let placed = format(&r.results, &output, r.min_block, &r.lang);
        let lines: Vec<&str> = placed.iter().map(String::as_str).collect();
        let value = value_of_result(&lines);
        for waiting in &mut rest {
            for (_, b) in &mut waiting.vars {
                if *b == Binding::After(name.clone()) {
                    *b = Binding::Ready(value.clone());
                }
            }
        }
    }
    let insertion = job.request.results.insertion;
    if matches!(insertion, Insertion::Silent | Insertion::None) {
        // `silent` shows the result in the echo area and `none` nowhere;
        // neither touches the file. A failure's echo, above, stands.
        if insertion == Insertion::Silent && outcome.is_ok() {
            effects.push(echo(
                EchoLevel::Info,
                if output.is_empty() {
                    format!("org: the {label} printed nothing")
                } else {
                    output.join("\n")
                },
            ));
        }
        post(effects);
        start_next(rest);
        return;
    }
    match place(&job.request, &output) {
        Ok(Some(edit)) => effects.insert(0, edit),
        Ok(None) => {}
        Err(why) => {
            let mut lines = vec![format!("── {label}: {why} ──")];
            lines.extend(output);
            // Never pointed at an empty buffer: if the lines did not go in,
            // the echo says the output is lost, and why.
            effects.push(match host_services::output_append(OUTPUT_BUFFER, &lines) {
                Ok(()) => echo(
                    EchoLevel::Warn,
                    format!("org: {why} — :org-babel-output shows the {label}'s output"),
                ),
                Err(e) => echo(
                    EchoLevel::Error,
                    format!("org: {why}, and the {label}'s output was lost: {e}"),
                ),
            });
        }
    }
    post(effects);
    // The next of its batch — after this one's result is posted, so the
    // results land in document order. A failure does not stop the batch,
    // as in emacs: each block's own result says how it went.
    start_next(rest);
}

/// The edit that writes `output` as the block's result, or why there is none.
fn place(request: &RunRequest, output: &[String]) -> Result<Option<Effect>, String> {
    let text = host_services::buffer_text(request.buffer)
        .ok_or_else(|| "its buffer closed while it ran".to_string())?;
    let lines: Vec<&str> = text.lines().collect();
    let block = BlockKey::decode(&request.key)
        .and_then(|k| k.find(&lines))
        .ok_or_else(|| "the block changed while it ran".to_string())?;
    let body = format(&request.results, output, request.min_block, &request.lang);
    let Some(e) = insertion_edit(
        &lines,
        text.ends_with('\n'),
        &block,
        &body,
        request.results.insertion,
    ) else {
        return Ok(None);
    };
    Ok(Some(Effect::ApplyEdit(ApplyEditPayload {
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
    })))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_runs_in_its_dir_or_its_files_dir() {
        let base = Some("/notes");
        assert_eq!(
            working_dir(None, base),
            "/notes",
            "the file's directory, as in emacs"
        );
        assert_eq!(working_dir(Some("sub"), base), "/notes/sub");
        assert_eq!(working_dir(Some("/abs"), base), "/abs");
        assert_eq!(working_dir(Some("sub"), None), "sub");
        assert_eq!(
            working_dir(None, None),
            "",
            "no file: the editor's directory"
        );
    }
}

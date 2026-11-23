//! `:session` — a long-lived interpreter that successive blocks share
//! (OB.15, design §6.3).
//!
//! One `spawn-interactive` job per `(buffer, language, session name)`,
//! started by the first block that names it, in that block's directory. Each
//! block is one line on the job's stdin ([`Wrapper::session_line`]), which
//! the wrapper's driver runs where the last block left its state. The line
//! prints the block's end sentinel on stdout and on stderr. When both have
//! come, everything the block wrote has, and it is reported exactly as a
//! script's exit is ([`jobs::complete`]). A session runs one block at a
//! time; a block for a busy session waits in its queue.
//!
//! A session ends when its process does (an `exit` in a block, a crash), on
//! `:org-babel-cancel`, when its buffer closes, and on unload, when the host
//! drops the plugin's jobs. Each session also has a record in the store
//! (`babel/session/<buffer>/…`), so the grammar seam's `:org-babel-cancel`
//! can see an idle one, and each start and end is logged in `*org-babel*`.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use super::jobs::{self, echo, post, Finished, OUTPUT_BUFFER};
use super::results::Collection;
use super::run::RunRequest;
use super::vars::{prelude, Value};
use super::wrappers::Wrapper;
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::types::EchoLevel;

/// Where a live session's record lives: `babel/session/<buffer>/<lang>/<name>`.
pub(crate) const SESSION_PREFIX: &str = "babel/session/";

/// The session records of one buffer.
pub(crate) fn buffer_prefix(buffer: u32) -> String {
    format!("{SESSION_PREFIX}{buffer}/")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    buffer: u32,
    lang: String,
    /// `""` for a bare `:session`: the language's default session.
    name: String,
}

impl Key {
    fn record(&self) -> String {
        format!("{}{}/{}", buffer_prefix(self.buffer), self.lang, self.name)
    }

    fn label(&self) -> String {
        if self.name.is_empty() {
            format!("{} session", self.lang)
        } else {
            format!("{} session \"{}\"", self.lang, self.name)
        }
    }
}

/// A block on its way to a session.
struct Block {
    request: RunRequest,
    rest: Vec<RunRequest>,
    /// What is written to stdin, newline added on writing.
    line: String,
    end: String,
    /// The value sentinel, for `:results value`.
    value: Option<String>,
}

/// The block a session is running.
struct Running {
    block: Block,
    output: Vec<String>,
    dropped: usize,
    /// How many of the block's two end sentinels have come.
    ends: u8,
}

/// Why this plugin is ending a session, which decides what its running
/// block's output becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ending {
    /// `:org-babel-cancel`: the block reports cancelled and writes nothing.
    Cancelled,
    /// Its buffer closed: the block's output so far goes to `*org-babel*`,
    /// as a script's does when its buffer closes.
    BufferClosed,
}

struct Session {
    key: Key,
    current: Option<Running>,
    queue: VecDeque<Block>,
    ending: Option<Ending>,
}

thread_local! {
    /// The live sessions, by job id.
    static SESSIONS: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());
}

fn log(line: String) {
    // `*org-babel*` keeps the lines until it is opened; failing to keep a
    // log line costs the log, not the session.
    let _ = host_services::output_append(OUTPUT_BUFFER, &[line]);
}

/// Send `request` to its session, starting the session first if there is
/// none; `rest` starts once it is done. A session that cannot start is
/// reported as any run that did not start.
pub(crate) fn submit(request: RunRequest, rest: Vec<RunRequest>, vars: &[(String, Value)]) {
    let Some(name) = request.session.clone() else {
        let why = format!("org: the {} is not a session block", request.label);
        return jobs::not_started(&request, &rest, why);
    };
    let key = Key {
        buffer: request.buffer,
        lang: request.lang.clone(),
        name,
    };
    // `plan` refused a session without a wrapper; this is that rule held.
    let Some(wrapper) = request.wrapper else {
        let why = format!("org: the {} has no value wrapper to drive it", key.label());
        return jobs::not_started(&request, &rest, why);
    };
    let uuid = match host_services::new_uuid() {
        Ok(u) => u,
        Err(e) => {
            let why = format!("org: cannot start the {}: {e}", request.label);
            return jobs::not_started(&request, &rest, why);
        }
    };
    let end = format!("org-babel-end-{uuid}");
    // A shell's value is its output; the other wrappers print a value after
    // a sentinel of its own.
    let value = (request.results.collection == Collection::Value && wrapper != Wrapper::Sh)
        .then(|| format!("org-babel-value-{uuid}"));
    let source = format!("{}{}", prelude(wrapper, vars), request.body);
    let line = wrapper.session_line(&source, value.as_deref(), &end);
    let job = match live(&key) {
        Some(job) => job,
        None => match start(&key, &request) {
            Ok(job) => job,
            Err(why) => return jobs::not_started(&request, &rest, why),
        },
    };
    SESSIONS.with(|s| {
        if let Some(session) = s.borrow_mut().get_mut(&job) {
            session.queue.push_back(Block {
                request,
                rest,
                line,
                end,
                value,
            });
        }
    });
    pump(job);
}

/// The job of `key`'s session, when it is live and not ending.
fn live(key: &Key) -> Option<u64> {
    SESSIONS.with(|s| {
        s.borrow()
            .iter()
            .find(|(_, session)| session.key == *key && session.ending.is_none())
            .map(|(id, _)| *id)
    })
}

/// Start `key`'s interpreter, in `request`'s directory.
fn start(key: &Key, request: &RunRequest) -> Result<u64, String> {
    let cwd = jobs::working_dir(request.dir.as_deref(), request.base.as_deref());
    let job = host_services::spawn_interactive(&request.command, &request.session_args, &cwd)
        .map_err(|e| format!("org: the {} did not start: {e}", key.label()))?;
    let _ = host_services::store_put(&key.record(), &[]);
    log(format!("── {} started ──", key.label()));
    SESSIONS.with(|s| {
        s.borrow_mut().insert(
            job,
            Session {
                key: key.clone(),
                current: None,
                queue: VecDeque::new(),
                ending: None,
            },
        );
    });
    Ok(job)
}

/// Write the next queued block, if the session is idle.
fn pump(job: u64) {
    let line = SESSIONS.with(|s| {
        let mut s = s.borrow_mut();
        let session = s.get_mut(&job)?;
        if session.current.is_some() || session.ending.is_some() {
            return None;
        }
        let block = session.queue.pop_front()?;
        let line = format!("{}\n", block.line);
        session.current = Some(Running {
            block,
            output: Vec::new(),
            dropped: 0,
            ends: 0,
        });
        Some(line)
    });
    if let Some(line) = line {
        // `err` only when the process is gone, and then its `job-finished`
        // is on the way and reports the block.
        let _ = host_services::write_job_stdin(job, line.as_bytes());
    }
}

/// `job-output` for a session: the running block's lines, up to the cap,
/// until its end has come on both streams. `Some` hands back lines that are
/// not a session's, for the caller.
pub(crate) fn on_output(job: u64, lines: Vec<String>) -> Option<Vec<String>> {
    if !SESSIONS.with(|s| s.borrow().contains_key(&job)) {
        return Some(lines);
    }
    let done = SESSIONS.with(|s| {
        let mut s = s.borrow_mut();
        let session = s.get_mut(&job)?;
        let running = session.current.as_mut()?;
        for line in lines {
            if line == running.block.end {
                running.ends += 1;
            } else if running.ends < 2 {
                if running.output.len() < running.block.request.max_lines {
                    running.output.push(line);
                } else {
                    running.dropped += 1;
                }
            }
        }
        if running.ends >= 2 {
            session.current.take()
        } else {
            None
        }
    });
    if let Some(r) = done {
        jobs::complete(finished(r), Ok(()));
        pump(job);
    }
    None
}

fn finished(r: Running) -> Finished {
    Finished {
        request: r.block.request,
        rest: r.block.rest,
        output: r.output,
        dropped: r.dropped,
        sentinel: r.block.value,
    }
}

/// `job-finished` for a session: it is over. Its running block is reported
/// by why it ended, and its queued blocks did not run. `Some` hands back an
/// outcome that is not a session's, for the caller.
pub(crate) fn on_finished(job: u64, outcome: Result<(), String>) -> Option<Result<(), String>> {
    let Some(session) = SESSIONS.with(|s| s.borrow_mut().remove(&job)) else {
        return Some(outcome);
    };
    let _ = host_services::store_delete(&session.key.record());
    let label = session.key.label();
    let why = match (&session.ending, &outcome) {
        (Some(Ending::Cancelled), _) => "cancelled".to_string(),
        (Some(Ending::BufferClosed), _) => format!("its buffer closed, which ended its {label}"),
        (None, Ok(())) => format!("its {label} ended"),
        (None, Err(e)) => format!("its {label} ended: {e}"),
    };
    log(format!(
        "── {label} ended{} ──",
        match session.ending {
            Some(Ending::Cancelled) => ": cancelled",
            Some(Ending::BufferClosed) => ": its buffer closed",
            None => "",
        }
    ));
    let mut effects = Vec::new();
    for b in &session.queue {
        jobs::release(std::slice::from_ref(&b.request));
        jobs::release(&b.rest);
        effects.push(echo(
            EchoLevel::Warn,
            format!("org: the {} did not run — {why}", b.request.label),
        ));
    }
    if !effects.is_empty() {
        host_services::refresh_decorations();
        post(effects);
    }
    match session.current {
        Some(r) => jobs::complete(finished(r), Err(why)),
        // Nothing was running, and only an end nobody asked for is news.
        None if session.ending.is_none() => post(vec![echo(
            EchoLevel::Warn,
            format!("org: the {label} ended"),
        )]),
        None => {}
    }
    None
}

/// End `buffer`'s sessions, for `why`: what is queued does not run, and what
/// was to follow the running block does not either. Each reports through its
/// `job-finished`.
pub(crate) fn end_all(buffer: u32, why: Ending) {
    let ids: Vec<u64> = SESSIONS.with(|s| {
        let mut s = s.borrow_mut();
        s.iter_mut()
            .filter(|(_, session)| session.key.buffer == buffer && session.ending.is_none())
            .map(|(id, session)| {
                session.ending = Some(why);
                for b in session.queue.drain(..) {
                    jobs::release(std::slice::from_ref(&b.request));
                    jobs::release(&b.rest);
                }
                if let Some(r) = session.current.as_mut() {
                    jobs::release(&r.block.rest);
                    r.block.rest.clear();
                }
                *id
            })
            .collect()
    });
    for id in ids {
        host_services::cancel_job(id);
    }
}

/// `document-closed`: a session belongs to its buffer.
pub(crate) fn on_closed(buffer: u64) {
    if let Ok(buffer) = u32::try_from(buffer) {
        end_all(buffer, Ending::BufferClosed);
    }
}

//! A PDF export: the LaTeX written to disk, then the user's
//! `org.export.latex-pdf-process` run over it a step at a time, each after
//! the last exits zero, as emacs's `org-latex-pdf-process` (lattice's
//! `org-export.md` §5).
//!
//! **The `.tex` is on disk before the compiler starts.** It is written with
//! `write-file`, whose `ok` means the bytes are there, not with the
//! `write-to-file` effect, which the editor applies later and would race the
//! process.
//!
//! **No compiler runs until the user names one.** The option is empty by
//! default, and the process grant follows only a value the user wrote, so a
//! PDF export with nothing configured writes the `.tex` and says how to
//! name a compiler.
//!
//! Every step's output goes to `*org-export*`, which starts each export on a
//! clean page; its headerline says what is running and how it ended. The
//! jobs are this instance's memory, as babel's are: every `job-output` and
//! `job-finished` for them arrives here.

use std::cell::RefCell;
use std::collections::HashMap;

use lattice_plugin_sdk::ConfigShape as ConfigShapeDerive;

use super::pipeline::file_name;
use crate::lattice::plugin_host::types::{EchoLevel, EchoPayload, Effect, OpenBufferPayload};

/// The buffer every step's output goes to.
pub(crate) const LOG_BUFFER: &str = "*org-export*";

/// One `[[org.export.latex-pdf-process]]` entry: a program and its
/// arguments, in which `%f` is the `.tex` file, `%b` its name without the
/// extension, `%o` its directory and `%latex` the compiler.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShapeDerive)]
pub(crate) struct ProcessStep {
    /// A program on `PATH`, or its path. No shell is involved.
    pub(crate) command: String,
    pub(crate) args: Option<Vec<String>>,
}

/// How a run's log headerline reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogState {
    Running,
    Succeeded,
    Failed,
}

/// What a PDF export does outside the plugin, so a test can stand in.
pub(crate) trait Runner {
    /// Write `text` to `path`; `ok` once it is on disk.
    fn write_file(&self, path: &str, text: &str) -> Result<(), String>;
    /// Start `command` in `cwd`: the job's id.
    fn spawn(&self, command: &str, args: &[String], cwd: &str) -> Result<u64, String>;
    fn cancel_job(&self, id: u64);
    /// Empty the log.
    fn log_reset(&self);
    fn log(&self, lines: &[String]);
    fn log_status(&self, state: LogState, text: &str);
}

/// A PDF export, from its LaTeX to its PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Compile {
    /// The `.tex` path, and its text.
    pub(crate) tex: String,
    pub(crate) output: String,
    /// The process, as the user wrote it.
    pub(crate) steps: Vec<ProcessStep>,
    /// The compiler the file was written for, which `%latex` names: one of
    /// pdflatex, xelatex and lualatex, which the LaTeX export checked.
    pub(crate) compiler: String,
    /// Then open the PDF: in lattice, or by the system when `true`.
    pub(crate) open: Option<bool>,
    /// What the file asked for that the export does not do.
    pub(crate) left_out: Vec<String>,
}

/// A compile in flight: the step running and the ones after it.
struct Job {
    pdf: String,
    /// The running step as a line, for the log and the echo.
    running: String,
    program: String,
    rest: Vec<(String, Vec<String>)>,
    cwd: String,
    open: Option<bool>,
}

thread_local! {
    static JOBS: RefCell<HashMap<u64, Job>> = RefCell::new(HashMap::new());
}

/// `tex` with `.pdf` for its extension.
fn pdf_path(tex: &str) -> String {
    match tex.strip_suffix(".tex") {
        Some(stem) => format!("{stem}.pdf"),
        None => format!("{tex}.pdf"),
    }
}

/// `arg` with the placeholders filled in.
fn expand(arg: &str, tex: &str, compiler: &str) -> String {
    let (dir, name) = match tex.rsplit_once('/') {
        Some((dir, name)) => (if dir.is_empty() { "/" } else { dir }, name),
        None => (".", tex),
    };
    let base = name.strip_suffix(".tex").unwrap_or(name);
    // `%latex` first, so its `%l` is not left behind by a shorter one.
    arg.replace("%latex", compiler)
        .replace("%f", tex)
        .replace("%b", base)
        .replace("%o", dir)
}

/// Write the `.tex` and start the first step: what to post now.
pub(crate) fn begin(compile: Compile, runner: &dyn Runner) -> Vec<Effect> {
    let tex_name = file_name(&compile.tex).to_string();
    if let Err(why) = runner.write_file(&compile.tex, &compile.output) {
        return vec![echo(
            EchoLevel::Error,
            format!("org: {tex_name} was not written: {why}"),
        )];
    }
    let pdf = pdf_path(&compile.tex);
    // A second export of the same file wins, as a second export's write
    // does: the first would only make the PDF the second replaces.
    let replaced: Vec<u64> = JOBS.with(|j| {
        let mut jobs = j.borrow_mut();
        let ids: Vec<u64> = jobs
            .iter()
            .filter(|(_, job)| job.pdf == pdf)
            .map(|(id, _)| *id)
            .collect();
        for id in &ids {
            jobs.remove(id);
        }
        ids
    });
    for id in replaced {
        runner.cancel_job(id);
    }
    let steps: Vec<(String, Vec<String>)> = compile
        .steps
        .iter()
        .filter(|s| !s.command.trim().is_empty())
        .map(|s| {
            let args = s.args.as_deref().unwrap_or_default();
            (
                s.command.trim().to_string(),
                args.iter()
                    .map(|a| expand(a, &compile.tex, &compile.compiler))
                    .collect(),
            )
        })
        .collect();
    if steps.is_empty() {
        return vec![echo(
            EchoLevel::Warn,
            format!(
                "Exported {tex_name}; no PDF, since org.export.latex-pdf-process names no \
                 compiler — set it in your config, e.g. command = \"latexmk\", args = \
                 [\"-pdf\", \"-interaction=nonstopmode\", \"-output-directory=%o\", \"%f\"]"
            ),
        )];
    }
    let cwd = match compile.tex.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((dir, _)) => dir.to_string(),
        None => ".".to_string(),
    };
    runner.log_reset();
    let mut steps = steps.into_iter();
    let Some((program, args)) = steps.next() else {
        return Vec::new();
    };
    let job = Job {
        pdf: pdf.clone(),
        running: String::new(),
        program,
        rest: steps.collect(),
        cwd,
        open: compile.open,
    };
    let mut effects = start(job, args, runner);
    if effects.is_empty() {
        let mut text = format!("Compiling {}…", file_name(&pdf));
        if !compile.left_out.is_empty() {
            text.push_str(&format!(
                " (not supported, so left out: {})",
                compile.left_out.join(", ")
            ));
        }
        effects.push(echo(EchoLevel::Info, text));
    }
    effects
}

/// Spawn `job`'s program with `args`. Nothing to post when it started; the
/// echo that says why when it did not.
fn start(mut job: Job, args: Vec<String>, runner: &dyn Runner) -> Vec<Effect> {
    let mut line = job.program.clone();
    for a in &args {
        line.push(' ');
        line.push_str(a);
    }
    runner.log(&[format!("── {line} (in {}) ──", job.cwd)]);
    runner.log_status(LogState::Running, &line);
    match runner.spawn(&job.program, &args, &job.cwd) {
        Ok(id) => {
            job.running = line;
            JOBS.with(|j| j.borrow_mut().insert(id, job));
            Vec::new()
        }
        Err(why) => {
            let text = format!("could not start `{}`: {why}", job.program);
            runner.log(std::slice::from_ref(&text));
            runner.log_status(LogState::Failed, &text);
            vec![echo(
                EchoLevel::Error,
                format!(
                    "org: {text}. A compiler runs only from org.export.latex-pdf-process \
                     as your config file sets it"
                ),
            )]
        }
    }
}

/// A job's output, when the job is a compile's.
pub(crate) fn on_output(id: u64, lines: &[String], runner: &dyn Runner) {
    if JOBS.with(|j| j.borrow().contains_key(&id)) {
        runner.log(lines);
    }
}

/// A job ended: start the compile's next step, or say how it went. `None`
/// when the job is not a compile's.
pub(crate) fn on_finished(
    id: u64,
    outcome: Result<(), String>,
    runner: &dyn Runner,
) -> Option<Vec<Effect>> {
    let mut job = JOBS.with(|j| j.borrow_mut().remove(&id))?;
    let pdf_name = file_name(&job.pdf).to_string();
    match outcome {
        Err(why) if why == "cancelled" => {
            runner.log_status(LogState::Failed, "cancelled");
            Some(vec![echo(
                EchoLevel::Info,
                format!("org: the export to {pdf_name} was cancelled"),
            )])
        }
        Err(why) => {
            let text = format!("`{}` failed: {why}", job.running);
            runner.log_status(LogState::Failed, &text);
            Some(vec![echo(
                EchoLevel::Error,
                format!(
                    "org: {pdf_name} was not made: `{}` failed ({why}) — \
                     :org-export-log shows its output",
                    job.program
                ),
            )])
        }
        Ok(()) => {
            if !job.rest.is_empty() {
                let (program, args) = job.rest.remove(0);
                job.program = program;
                return Some(start(job, args, runner));
            }
            runner.log_status(LogState::Succeeded, &format!("made {pdf_name}"));
            let mut effects = Vec::new();
            match job.open {
                Some(true) => effects.push(Effect::OpenExternalUri(job.pdf.clone())),
                Some(false) => effects.push(Effect::OpenBuffer(OpenBufferPayload {
                    path: Some(job.pdf.clone()),
                    force: false,
                })),
                None => {}
            }
            effects.push(echo(EchoLevel::Info, format!("Exported {pdf_name}")));
            Some(effects)
        }
    }
}

/// Stop every compile in flight: how many there were.
pub(crate) fn cancel(runner: &dyn Runner) -> usize {
    let ids: Vec<u64> = JOBS.with(|j| j.borrow().keys().copied().collect());
    // Each stays in the table until its `job-finished` says it was
    // cancelled, which is what tells the user.
    for id in &ids {
        runner.cancel_job(*id);
    }
    ids.len()
}

fn echo(level: EchoLevel, text: impl Into<String>) -> Effect {
    Effect::Echo(EchoPayload {
        text: text.into(),
        level,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// A runner that records what it was asked to do.
    #[derive(Default)]
    struct Fake {
        written: RefCell<Vec<(String, String)>>,
        spawned: RefCell<Vec<(String, Vec<String>, String)>>,
        cancelled: RefCell<Vec<u64>>,
        log: RefCell<Vec<String>>,
        status: RefCell<Vec<(LogState, String)>>,
        next: Cell<u64>,
        refuse_write: bool,
        refuse_spawn: bool,
    }

    impl Runner for Fake {
        fn write_file(&self, path: &str, text: &str) -> Result<(), String> {
            if self.refuse_write {
                return Err("denied".to_string());
            }
            self.written
                .borrow_mut()
                .push((path.to_string(), text.to_string()));
            Ok(())
        }
        fn spawn(&self, command: &str, args: &[String], cwd: &str) -> Result<u64, String> {
            if self.refuse_spawn {
                return Err("no grant".to_string());
            }
            self.spawned
                .borrow_mut()
                .push((command.to_string(), args.to_vec(), cwd.to_string()));
            self.next.set(self.next.get() + 1);
            Ok(1000 + self.next.get())
        }
        fn cancel_job(&self, id: u64) {
            self.cancelled.borrow_mut().push(id);
        }
        fn log_reset(&self) {
            self.log.borrow_mut().clear();
        }
        fn log(&self, lines: &[String]) {
            self.log.borrow_mut().extend(lines.iter().cloned());
        }
        fn log_status(&self, state: LogState, text: &str) {
            self.status.borrow_mut().push((state, text.to_string()));
        }
    }

    fn step(command: &str, args: &[&str]) -> ProcessStep {
        ProcessStep {
            command: command.to_string(),
            args: Some(args.iter().map(|a| a.to_string()).collect()),
        }
    }

    fn compile(steps: Vec<ProcessStep>, open: Option<bool>) -> Compile {
        Compile {
            tex: "/notes/plan.tex".to_string(),
            output: "\\documentclass{article}".to_string(),
            steps,
            compiler: "xelatex".to_string(),
            open,
            left_out: Vec::new(),
        }
    }

    fn texts(effects: &[Effect]) -> Vec<String> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Echo(p) => Some(p.text.clone()),
                _ => None,
            })
            .collect()
    }

    fn reset() {
        JOBS.with(|j| j.borrow_mut().clear());
    }

    #[test]
    fn the_placeholders_name_the_file() {
        assert_eq!(
            expand("-outdir=%o", "/a/b/plan.tex", "pdflatex"),
            "-outdir=/a/b"
        );
        assert_eq!(expand("%b.pdf", "/a/b/plan.tex", "pdflatex"), "plan.pdf");
        assert_eq!(expand("%f", "/a/plan.tex", "pdflatex"), "/a/plan.tex");
        assert_eq!(
            expand("-pdflatex=%latex", "/plan.tex", "lualatex"),
            "-pdflatex=lualatex"
        );
        assert_eq!(expand("%o", "/plan.tex", "pdflatex"), "/");
    }

    /// The `.tex` is written before anything runs, and the steps run in
    /// order, each after the last succeeds, in the file's directory; the
    /// PDF opens after the last.
    #[test]
    fn the_steps_run_in_order_after_the_write() {
        reset();
        let fake = Fake::default();
        let steps = vec![
            step("%latex", &[]),
            step("latexmk", &["-pdf", "%f"]),
            step("bibtex", &["%b"]),
        ];
        let effects = begin(compile(steps, Some(false)), &fake);
        assert_eq!(texts(&effects), vec!["Compiling plan.pdf…".to_string()]);
        assert_eq!(fake.written.borrow().len(), 1);
        // A program is not a placeholder: only arguments are filled in.
        assert_eq!(
            fake.spawned.borrow()[0],
            ("%latex".to_string(), vec![], "/notes".to_string())
        );
        assert!(on_finished(1001, Ok(()), &fake).is_some_and(|e| e.is_empty()));
        assert_eq!(
            fake.spawned.borrow()[1],
            (
                "latexmk".to_string(),
                vec!["-pdf".to_string(), "/notes/plan.tex".to_string()],
                "/notes".to_string()
            )
        );
        assert!(on_finished(1002, Ok(()), &fake).is_some_and(|e| e.is_empty()));
        assert_eq!(fake.spawned.borrow()[2].1, vec!["plan".to_string()]);
        let done = on_finished(1003, Ok(()), &fake).unwrap_or_default();
        assert!(
            matches!(&done[0], Effect::OpenBuffer(p) if p.path.as_deref() == Some("/notes/plan.pdf"))
        );
        assert_eq!(texts(&done), vec!["Exported plan.pdf".to_string()]);
        assert_eq!(
            fake.status.borrow().last(),
            Some(&(LogState::Succeeded, "made plan.pdf".to_string()))
        );
        assert!(
            on_finished(1003, Ok(()), &fake).is_none(),
            "the job is gone"
        );
    }

    /// A failing step stops the list and names the log.
    #[test]
    fn a_failing_step_stops_the_list_and_names_the_log() {
        reset();
        let fake = Fake::default();
        begin(
            compile(
                vec![step("latexmk", &["%f"]), step("never", &[])],
                Some(false),
            ),
            &fake,
        );
        let effects =
            on_finished(1001, Err("exit status 12".to_string()), &fake).unwrap_or_default();
        assert_eq!(
            texts(&effects),
            vec![
                "org: plan.pdf was not made: `latexmk` failed (exit status 12) — \
                 :org-export-log shows its output"
                    .to_string()
            ]
        );
        assert_eq!(fake.spawned.borrow().len(), 1);
        assert!(!effects.iter().any(|e| matches!(e, Effect::OpenBuffer(_))));
        assert_eq!(
            fake.status.borrow().last().map(|s| s.0),
            Some(LogState::Failed)
        );
    }

    /// The output lands in the log, which starts each export clean and
    /// says what runs.
    #[test]
    fn the_output_goes_to_the_log() {
        reset();
        let fake = Fake::default();
        fake.log(&["stale".to_string()]);
        begin(compile(vec![step("latexmk", &["%f"])], None), &fake);
        on_output(1001, &["Latexmk: done".to_string()], &fake);
        on_output(77, &["someone else's".to_string()], &fake);
        assert_eq!(
            *fake.log.borrow(),
            vec![
                "── latexmk /notes/plan.tex (in /notes) ──".to_string(),
                "Latexmk: done".to_string()
            ]
        );
        let done = on_finished(1001, Ok(()), &fake).unwrap_or_default();
        assert_eq!(done.len(), 1, "no open was asked for: {done:?}");
    }

    /// Nothing configured: the `.tex` is written and the echo says how to
    /// name a compiler; nothing runs.
    #[test]
    fn with_no_process_nothing_runs() {
        reset();
        let fake = Fake::default();
        let effects = begin(compile(vec![step("  ", &[])], Some(false)), &fake);
        assert!(fake.spawned.borrow().is_empty());
        assert_eq!(fake.written.borrow().len(), 1);
        assert!(texts(&effects)[0]
            .starts_with("Exported plan.tex; no PDF, since org.export.latex-pdf-process"));
    }

    /// A write or a start that fails stops the export, saying why.
    #[test]
    fn a_refused_write_or_start_is_said() {
        reset();
        let fake = Fake {
            refuse_write: true,
            ..Fake::default()
        };
        let effects = begin(compile(vec![step("latexmk", &[])], None), &fake);
        assert_eq!(
            texts(&effects),
            vec!["org: plan.tex was not written: denied".to_string()]
        );
        assert!(fake.spawned.borrow().is_empty());
        let fake = Fake {
            refuse_spawn: true,
            ..Fake::default()
        };
        let effects = begin(compile(vec![step("latexmk", &[])], None), &fake);
        assert!(
            texts(&effects)[0].starts_with("org: could not start `latexmk`: no grant."),
            "{effects:?}"
        );
    }

    /// A second export of the file cancels the first's compile, and cancel
    /// stops every compile, whose end then says it was cancelled.
    #[test]
    fn a_newer_export_or_cancel_stops_a_compile() {
        reset();
        let fake = Fake::default();
        begin(compile(vec![step("latexmk", &[])], None), &fake);
        begin(compile(vec![step("latexmk", &[])], None), &fake);
        assert_eq!(*fake.cancelled.borrow(), vec![1001]);
        assert!(
            on_finished(1001, Err("cancelled".to_string()), &fake).is_none(),
            "replaced, so quiet"
        );
        assert_eq!(cancel(&fake), 1);
        assert_eq!(*fake.cancelled.borrow(), vec![1001, 1002]);
        let effects = on_finished(1002, Err("cancelled".to_string()), &fake).unwrap_or_default();
        assert_eq!(
            texts(&effects),
            vec!["org: the export to plan.pdf was cancelled".to_string()]
        );
    }
}

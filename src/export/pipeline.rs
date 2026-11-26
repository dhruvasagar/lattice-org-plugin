//! The export itself, on the events seam: read the buffer, parse it, render
//! it, write the file.
//!
//! The grammar seam only checks the target and emits [`EV_REQUESTED`]; this
//! side does the work and posts its result, so no export runs on the
//! keystroke. The buffer is read once, with `buffer-text`, when the request
//! arrives: an export sees the buffer as it was then, unsaved edits
//! included, as emacs's does, and edits made while it runs do not reach it.
//!
//! **Stepped.** One events-seam call has a fuel budget, and nothing done
//! over the whole of a large file fits in it (lattice's `org-export.md`
//! §7). So a [`Run`] does about [`STEP_BYTES`] of work per call and emits
//! [`EV_STEP`] to itself for the next, the way the roam scan walks a corpus:
//! the includes, the parse a chunk of source per step, then three walks over
//! the headlines a run at a time ([`Phase`]): numbering the outline, the
//! writer's pass before the body, and the body. Chunks are cut at headlines,
//! where no org element can continue, so the chunked parse is the whole-file
//! parse. Runs live in guest memory between steps: a new export of the same
//! target replaces the one in flight, and an export of another runs beside
//! it.
//!
//! The writers are [`super::html`], [`super::markdown`] and
//! [`super::latex`], each told the outline and then each headline in turn.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ops::Range;

use super::babel::{Planning, Settled};
use super::common::{advance, at, Outline, OutlineWalk};
use super::filter::{self, Selection, View};
use super::html::{self, Html, Stylesheet};
use super::include::Includes;
use super::latex::{self, Latex};
use super::markdown::{self, Markdown};
use super::options::Options;
use super::parse::{self, Context, Doc, ElementKind, FlatSection, Headline};
use super::sink::{self, Sink};
use super::targets::Targets;
use super::Host;
use crate::babel::block::BlockKey;
use crate::babel::results::{insertion_edit, Insertion};
use crate::babel::run::RunRequest;
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::types::{
    ArgValue, Args, ConfirmPayload, EchoLevel, EchoPayload, Effect,
};
use crate::time_format::When;
use std::rc::Rc;

/// The plugin event carrying a [`Request`] from the command to the events
/// seam.
pub(crate) const EV_REQUESTED: &str = "org/export-requested";

/// The plugin event a [`Run`] emits to itself to take its next step; the
/// payload is the run's id.
pub(crate) const EV_STEP: &str = "org/export-step";

/// `:org-export-cancel`, from the grammar seam, where the runs are not.
pub(crate) const EV_CANCEL: &str = "org/export-cancel";

/// The `y` to an export's question about running its blocks, from the
/// grammar seam: the run's id.
pub(crate) const EV_RUN_BLOCKS: &str = "org/export-run-blocks";

/// The `n` to it: export with the results already in the file.
pub(crate) const EV_KEEP_RESULTS: &str = "org/export-keep-results";

/// How much one step does: the source a parse step reads, and the output a
/// render step writes. A parse chunk ends at the first headline past this,
/// and a render step at the first headline past it, so one section larger
/// than it is one step: within a section there is no line an element cannot
/// cross.
///
/// The spike measured about 7,000 fuel per line of real org for parse,
/// objects and render together; 64 KiB is a little over a tenth of the event
/// budget at that rate, leaving room for denser text.
const STEP_BYTES: usize = 64 * 1024;

/// How many source blocks a step resolves the `:exports` of, once the
/// file's properties are known: each is a header-argument merge of a few
/// short strings, so this many is a small part of a step's budget.
const BLOCKS_PER_STEP: usize = 512;

/// About how many lines of reading a step's planning may do: a block's plan
/// reads the whole file a few times, so a step plans this many lines' worth
/// of blocks, and always one.
const PLAN_LINES: usize = 100_000;

/// An output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Backend {
    Html,
    Markdown,
    Latex,
}

impl Backend {
    /// The output file's extension.
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Backend::Html => "html",
            Backend::Markdown => "md",
            Backend::Latex => "tex",
        }
    }
}

/// One export, as the command hands it across.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Request {
    /// The org buffer, read when the export runs.
    pub(crate) buffer: u32,
    /// Its file, which names the output.
    pub(crate) source: String,
    /// Where the output goes, already checked with `can-write-file`.
    pub(crate) target: String,
    pub(crate) backend: Backend,
    /// Export the subtree around [`line`](Self::line) only.
    pub(crate) subtree: bool,
    /// Leave out the preamble or `<head>`.
    pub(crate) body_only: bool,
    /// The cursor's line when the command ran.
    pub(crate) line: u32,
    /// Where the output goes.
    pub(crate) sink: Sink,
}

/// The output's path: the source's, with the backend's extension in place of
/// `.org` (or after a name that has none).
pub(crate) fn target_path(source: &str, backend: Backend) -> String {
    let stem = source
        .rsplit_once('/')
        .map_or(source, |(_, name)| name)
        .rsplit_once('.')
        // A leading dot is a hidden file's name, not its extension: emacs
        // exports `.notes` to `.notes.html`.
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(stem, _)| stem.len());
    let base = match stem {
        Some(len) => {
            let name_start = source.rfind('/').map_or(0, |i| i + 1);
            &source[..name_start + len]
        }
        None => source,
    };
    format!("{base}.{}", backend.extension())
}

/// What an export takes from the plugin's options: the lowest layer of its
/// options, under the file's keywords, and HTML's stylesheet.
#[derive(Debug, Clone, Default)]
pub(crate) struct Configured {
    pub(crate) options: Options,
    pub(crate) stylesheet: Stylesheet,
    /// `org.export.latex-pdf-process`, for a PDF export.
    pub(crate) pdf_process: Vec<super::pdf::ProcessStep>,
    /// Babel's settings, for the blocks an export runs.
    pub(crate) babel: BabelConfig,
}

/// The `org.babel.*` settings an export runs the file's blocks with.
#[derive(Debug, Clone)]
pub(crate) struct BabelConfig {
    /// `org.babel.enabled`: off, an export uses the results in the file.
    pub(crate) enabled: bool,
    /// `org.babel.confirm`: whether to ask before running.
    pub(crate) confirm: bool,
    /// `org.babel.languages`, or why it cannot be read.
    pub(crate) languages: Result<Vec<crate::babel::Language>, String>,
}

impl Default for BabelConfig {
    /// Off, as a run with no editor behind it has no languages to run.
    fn default() -> Self {
        Self {
            enabled: false,
            confirm: true,
            languages: Ok(Vec::new()),
        }
    }
}

/// The `org.export.*` options as they are now; `Err` names one whose value
/// it does not take. A LaTeX value a LaTeX export cannot use (an unknown
/// compiler) is kept as written, for that export to refuse, naming it.
fn configured() -> Result<Configured, String> {
    use super::options::{DeclaredClass, LatexClass};
    let mut options = Options::default();
    let string = |name: &str, field: &mut String| *field = crate::option_or(name, field);
    string("export.latex-compiler", &mut options.latex_compiler);
    string("export.latex-class", &mut options.latex_class);
    string(
        "export.latex-class-options",
        &mut options.latex_class_options,
    );
    string("export.latex-image-width", &mut options.latex_image_width);
    string(
        "export.latex-src-block-backend",
        &mut options.latex_src_backend,
    );
    options.latex_header = crate::string_list("export.latex-header");
    // The host validated the value against the shape, so an `Err` is a bug
    // between the two, and the built-in classes are what is left.
    if let Some(Ok(classes)) =
        crate::config_shape::read_option::<Vec<DeclaredClass>>("export.latex-classes")
    {
        options.add_classes(classes.into_iter().map(LatexClass::from));
    }
    for item in super::options::ITEMS {
        let name = format!("export.{}", item.name);
        options.configure(item, &crate::option_or(&name, item.default))?;
    }
    let author = crate::option_or("export.author", "");
    if !author.trim().is_empty() {
        options.author = super::parse::objects::parse(&author);
        options.author_text = author;
    }
    string("export.email", &mut options.email);
    string("export.language", &mut options.language);
    options.select_tags = crate::string_list("export.select-tags");
    options.exclude_tags = crate::string_list("export.exclude-tags");
    options.html_head = crate::string_list("export.html-head");
    options.html_head_extra = crate::string_list("export.html-head-extra");
    let pdf_process = crate::config_shape::read_option::<Vec<super::pdf::ProcessStep>>(
        "export.latex-pdf-process",
    )
    .and_then(Result::ok)
    .unwrap_or_default();
    let on = |name: &str| crate::option_or(name, "true").eq_ignore_ascii_case("true");
    Ok(Configured {
        options,
        stylesheet: Stylesheet::from_option(&crate::option_or("export.html-stylesheet", "default")),
        pdf_process,
        babel: BabelConfig {
            enabled: on("babel.enabled"),
            confirm: on("babel.confirm"),
            languages: crate::babel::languages(),
        },
    })
}

/// The last component of `path`, for the echo area.
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// Hand `request` to the events seam. Returns at once; `Err` is why it was
/// not sent, for the command to echo.
pub(crate) fn request(request: &Request) -> Result<(), String> {
    let bytes = rmp_serde::to_vec(request)
        .map_err(|e| format!("org: the export request did not encode: {e}"))?;
    host_services::emit_event(EV_REQUESTED, &bytes);
    Ok(())
}

thread_local! {
    /// The exports in flight, oldest first.
    static RUNS: RefCell<Vec<Run>> = const { RefCell::new(Vec::new()) };
    static NEXT_RUN: Cell<u64> = const { Cell::new(1) };
}

/// `on-event` for [`EV_REQUESTED`]: read the buffer and start a run.
pub(crate) fn on_requested(payload: &[u8]) {
    let Ok(request) = rmp_serde::from_slice::<Request>(payload) else {
        // The plugin's own event, so this is a bug; the guest has no log, so
        // the user is told.
        post(vec![echo(
            EchoLevel::Error,
            "org: an export request was unreadable",
        )]);
        return;
    };
    let configured = match configured() {
        Ok(c) => c,
        Err(why) => {
            post(vec![echo(EchoLevel::Error, why)]);
            return;
        }
    };
    let text = host_services::buffer_text(request.buffer);
    let id = NEXT_RUN.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    let now = When {
        local_secs: crate::local_now_secs(),
    };
    match Run::new(
        id,
        request,
        text,
        crate::todo_keyword_lines(),
        now,
        Rc::new(super::Guest),
        configured,
    ) {
        Ok(run) => {
            let replaced: Vec<u64> = RUNS.with(|runs| {
                let mut runs = runs.borrow_mut();
                // A second export to the same file wins: the first would only
                // write what the second overwrites.
                let replaced = runs
                    .iter()
                    .filter(|r| r.request.target == run.request.target)
                    .map(|r| r.id)
                    .collect();
                runs.retain(|r| r.request.target != run.request.target);
                runs.push(run);
                replaced
            });
            for old in replaced {
                crate::babel::jobs::cancel_export(old);
            }
            host_services::emit_event(EV_STEP, id.to_string().as_bytes());
        }
        Err(effects) => post(effects),
    }
}

/// `on-event` for [`EV_STEP`]: one step of the run it names.
pub(crate) fn on_step(payload: &[u8]) {
    let Some(id) = std::str::from_utf8(payload)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    else {
        post(vec![echo(
            EchoLevel::Error,
            "org: an export step was unreadable",
        )]);
        return;
    };
    let step = RUNS.with(|runs| {
        let mut runs = runs.borrow_mut();
        // Absent when a newer export of its target replaced it.
        let at = runs.iter().position(|r| r.id == id)?;
        let step = runs[at].step();
        let notes = if matches!(step, Step::Done(_) | Step::Compile(_)) {
            runs.remove(at).notes
        } else {
            Vec::new()
        };
        Some((step, notes))
    });
    let step = step.map(|(step, notes)| {
        if !notes.is_empty() {
            post(notes);
        }
        step
    });
    // Outside the borrow: starting a run can settle a block at once (one
    // that cannot start), and settling reads the runs.
    match step {
        Some(Step::More) => host_services::emit_event(EV_STEP, payload),
        Some(Step::Done(effects)) => post(effects),
        Some(Step::Compile(compile)) => post(super::pdf::begin(compile, &super::Guest)),
        Some(Step::Wait(effects)) => {
            if !effects.is_empty() {
                post(effects);
            }
        }
        Some(Step::Run(runs)) => crate::babel::jobs::start_next(runs),
        None => {}
    }
}

/// Ask the events seam to run the blocks export `id` asked about, for the
/// `y` to its question.
pub(crate) fn request_run_blocks(id: u64) {
    host_services::emit_event(EV_RUN_BLOCKS, id.to_string().as_bytes());
}

/// Ask the events seam to go on with export `id` without running its
/// blocks, for the `n` to its question.
pub(crate) fn request_keep_results(id: u64) {
    host_services::emit_event(EV_KEEP_RESULTS, id.to_string().as_bytes());
}

/// `on-event` for [`EV_KEEP_RESULTS`]: go on with the run named, over the
/// file as it is, if it is still waiting for an answer.
pub(crate) fn on_keep_results(payload: &[u8]) {
    let Some(id) = std::str::from_utf8(payload)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return;
    };
    let waiting = RUNS.with(|runs| {
        let mut runs = runs.borrow_mut();
        runs.iter_mut()
            .find(|r| r.id == id)
            .is_some_and(Run::declined)
    });
    if waiting {
        host_services::emit_event(EV_STEP, payload);
    }
}

/// `on-event` for [`EV_RUN_BLOCKS`]: start the blocks of the run named,
/// if it is still waiting for them.
pub(crate) fn on_run_blocks(payload: &[u8]) {
    let Some(id) = std::str::from_utf8(payload)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return;
    };
    let runs = RUNS.with(|runs| {
        let mut runs = runs.borrow_mut();
        let run = runs.iter_mut().find(|r| r.id == id)?;
        run.consented()
    });
    match runs {
        Some(runs) => crate::babel::jobs::start_next(runs),
        // Replaced by a newer export, or cancelled, while the question was
        // open: what it would have run is not wanted.
        None => post(vec![echo(
            EchoLevel::Info,
            "org: that export is no longer waiting — nothing ran",
        )]),
    }
}

/// One of export `id`'s blocks settled (babel's jobs call this). The last
/// one resumes the export, over the copy with the results in.
pub(crate) fn on_block(id: u64, settled: Settled) {
    let cancelled = matches!(settled, Settled::Cancelled);
    let resume = RUNS.with(|runs| {
        let mut runs = runs.borrow_mut();
        let at = runs.iter().position(|r| r.id == id)?;
        if cancelled {
            runs.remove(at);
            return Some(false);
        }
        Some(runs[at].settle(settled))
    });
    match resume {
        Some(true) => host_services::emit_event(EV_STEP, id.to_string().as_bytes()),
        Some(false) if cancelled => {
            crate::babel::jobs::cancel_export(id);
            post(vec![echo(EchoLevel::Info, "org: the export was cancelled")]);
        }
        _ => {}
    }
}

/// Ask the events seam to stop every export, for `:org-export-cancel`.
pub(crate) fn request_cancel() {
    host_services::emit_event(EV_CANCEL, &[]);
}

/// `on-event` for [`EV_CANCEL`]: drop the runs, stop the compiles, and say
/// so. A compile says it was cancelled when its job ends.
pub(crate) fn on_cancel() {
    let ids: Vec<u64> = RUNS.with(|runs| {
        std::mem::take(&mut *runs.borrow_mut())
            .iter()
            .map(|r| r.id)
            .collect()
    });
    let runs = ids.len();
    for id in ids {
        crate::babel::jobs::cancel_export(id);
    }
    let compiles = super::pdf::cancel(&super::Guest);
    if runs > 0 {
        post(vec![echo(EchoLevel::Info, "org: the export was cancelled")]);
    } else if compiles == 0 {
        post(vec![echo(EchoLevel::Info, "org: no export is running")]);
    }
}

/// What a step leaves to do.
#[derive(Debug)]
pub(crate) enum Step {
    /// Another step is needed.
    More,
    /// The run is over; post these.
    Done(Vec<Effect>),
    /// The LaTeX is written; compile it to PDF.
    Compile(super::pdf::Compile),
    /// Post these and wait: for the user's answer, or the blocks' results,
    /// whose arrival resumes the run.
    Wait(Vec<Effect>),
    /// Start these runs of the file's blocks and wait for their results.
    Run(Vec<RunRequest>),
}

/// Where a run is in running the file's blocks.
enum Eval {
    /// Not looked at yet: the parse decides what runs.
    Unread,
    Planning(Box<Planning>),
    /// Asked; waiting for `y`.
    Asking(Vec<RunRequest>),
    /// Running; this many blocks have yet to settle.
    Running(usize),
    /// Nothing more to run.
    Done,
}

/// One export in flight.
pub(crate) struct Run {
    id: u64,
    request: Request,
    /// The buffer's text, as it was when the export was asked for.
    text: String,
    ctx: Context,
    /// The file's `#+INCLUDE:`s while they are being read; `text` is the
    /// expanded text once this is `None`.
    includes: Option<Includes>,
    /// The cursor's line in `text`: the request's, moved past what the
    /// includes above it became.
    line: u32,
    /// What each parse step reads: a byte range of `text`, starting at a
    /// headline or at the top of the file, and its first line. Planned once
    /// the includes are in.
    chunks: Vec<(Range<usize>, u32)>,
    /// The next chunk to parse.
    next: usize,
    sections: Vec<FlatSection>,
    /// The file's keywords, in order, gathered as the chunks are parsed.
    keywords: Vec<(String, String)>,
    /// When the export was asked for: `{{{time}}}` reads it.
    now: When,
    /// The link targets, gathered as the chunks are parsed.
    targets: Targets,
    host: Rc<dyn Host>,
    /// The `org.export.*` options, read when the export was asked for.
    configured: Configured,
    /// About how many bytes a step parses or writes: [`STEP_BYTES`], or
    /// another size in tests, which check that the steps change nothing.
    budget: usize,
    /// What the source blocks' `:exports` leave out, read with the chunks.
    scan: super::babel::Scan,
    eval: Eval,
    /// What the user is told when the run ends: why blocks did not run.
    notes: Vec<Effect>,
    /// Set once the parse is done.
    render: Option<Render>,
}

/// The render phase: the tree, its options, where the walk is, and the
/// output so far.
struct Render {
    doc: Doc,
    view: View,
    selection: Selection,
    phase: Phase,
    /// The next headline to visit, as indices from `doc.headlines` down
    /// through `children`; empty once the walk is over.
    next: Vec<usize>,
    out: String,
}

/// Where a render is. Each phase is a walk taken a budget at a time, so no
/// call does work in proportion to the whole file.
enum Phase {
    /// Numbering the headlines. The writer is made from `settings` when the
    /// outline is whole, since every backend reads it from the first byte.
    Outline {
        outline: Box<Outline>,
        walk: OutlineWalk,
        settings: Option<Settings>,
    },
    /// Writing. While `index` is `Some` it is the writer's pass before the
    /// body (its table of contents, LaTeX's footnote definitions), and what
    /// that has written so far; then the body, the headlines from `next` on.
    Write {
        writer: Writer,
        index: Option<String>,
    },
}

/// What a writer is made from, besides the outline.
enum Settings {
    Html(html::Settings),
    Markdown(markdown::Settings),
    Latex(latex::Settings),
}

/// What writes the output.
enum Writer {
    Html(Box<Html>),
    Markdown(Box<Markdown>),
    Latex(Box<Latex>),
}

impl Writer {
    fn new(settings: Settings, outline: Outline, view: &mut View) -> Writer {
        match settings {
            Settings::Html(s) => Writer::Html(Box::new(Html::new(outline, s))),
            Settings::Markdown(s) => Writer::Markdown(Box::new(Markdown::new(outline, s))),
            Settings::Latex(s) => Writer::Latex(Box::new(Latex::new(outline, s, view))),
        }
    }
}

impl Run {
    /// A run over `text` (`None` when the buffer closed before the export
    /// read it), or the effects that say why there is none.
    ///
    /// The whole file is parsed even for a subtree export: the file's
    /// keywords (`#+OPTIONS:`, `#+TITLE:`, `#+EXCLUDE_TAGS:`…) apply to a
    /// subtree too, and only the parse tells a keyword from a line inside an
    /// example block that looks like one.
    pub(crate) fn new(
        id: u64,
        request: Request,
        text: Option<String>,
        configured_keywords: Vec<String>,
        now: When,
        host: Rc<dyn Host>,
        configured: Configured,
    ) -> Result<Self, Vec<Effect>> {
        let Some(text) = text else {
            return Err(vec![echo(
                EchoLevel::Warn,
                format!(
                    "org: {} closed before it was exported",
                    file_name(&request.source)
                ),
            )]);
        };
        // The file's own `#+TODO:` lines decide its keywords, as in emacs,
        // where the buffer's settings are read before anything is included.
        let ctx = Context::for_file(&text, configured_keywords);
        let includes = Includes::new(&text, &request.source, ctx.todo_keywords.clone());
        let (includes, chunks) = if includes.done() {
            (None, chunks(&text, STEP_BYTES))
        } else {
            (Some(includes), Vec::new())
        };
        let line = request.line;
        Ok(Run {
            id,
            request,
            ctx,
            includes,
            line,
            chunks,
            next: 0,
            sections: Vec::new(),
            keywords: Vec::new(),
            now,
            targets: Targets::default(),
            host,
            configured,
            scan: Default::default(),
            eval: Eval::Unread,
            notes: Vec::new(),
            render: None,
            budget: STEP_BYTES,
            text,
        })
    }

    /// The same run in steps of about `budget` bytes.
    #[cfg(test)]
    pub(crate) fn with_budget(mut self, budget: usize) -> Self {
        self.budget = budget;
        if self.includes.is_none() {
            self.chunks = chunks(&self.text, budget);
        }
        self
    }

    /// One step: parse the next chunk, or prepare the render, or render the
    /// next headlines, or finish the file and write it.
    pub(crate) fn step(&mut self) -> Step {
        if let Some(includes) = self.includes.as_mut() {
            if !includes.done() {
                let host = Rc::clone(&self.host);
                if let Err(why) = includes.step(&|path| host.read_file(path)) {
                    return Step::Done(vec![echo(EchoLevel::Error, why)]);
                }
                return Step::More;
            }
            if let Some(includes) = self.includes.take() {
                let expanded = includes.finish();
                self.line = expanded.line(self.request.line);
                self.text = expanded.text;
                self.chunks = chunks(&self.text, self.budget);
            }
            return Step::More;
        }
        if let Some((bytes, first_line)) = self.chunks.get(self.next).cloned() {
            let lines: Vec<&str> = self.text[bytes].lines().collect();
            let parsed = parse::parse_chunk(&lines, first_line, &self.ctx);
            self.scan.chunk(&lines, first_line);
            for section in &parsed {
                self.targets.collect(section);
                self.keywords
                    .extend(section.elements.iter().filter_map(|e| match &e.kind {
                        ElementKind::Keyword { key, value } => Some((key.clone(), value.clone())),
                        _ => None,
                    }));
            }
            self.sections.extend(parsed);
            self.next += 1;
            return Step::More;
        }
        let backend = self.request.backend;
        if self.render.is_none() {
            let props: Vec<&str> = self
                .keywords
                .iter()
                .filter(|(k, _)| k == "PROPERTY")
                .map(|(_, v)| v.as_str())
                .collect();
            if self.scan.resolve(&props, BLOCKS_PER_STEP) {
                return Step::More;
            }
            if let Some(step) = self.evaluate() {
                return step;
            }
        }
        let Some(render) = self.render.as_mut() else {
            return match self.prepare() {
                Ok(render) => {
                    self.render = Some(render);
                    Step::More
                }
                Err(effects) => Step::Done(effects),
            };
        };
        let (doc, view, out) = (&render.doc, &mut render.view, &mut render.out);
        match &mut render.phase {
            Phase::Outline {
                outline,
                walk,
                settings,
            } => {
                if outline.step(walk, doc, &render.selection, view, self.budget) {
                    let Some(settings) = settings.take() else {
                        return Step::Done(vec![echo(
                            EchoLevel::Error,
                            "org: the export lost its settings",
                        )]);
                    };
                    let writer = Writer::new(settings, std::mem::take(&mut **outline), view);
                    render.phase = Phase::Write {
                        writer,
                        index: Some(String::new()),
                    };
                }
                return match view.error() {
                    Some(why) => Step::Done(vec![echo(EchoLevel::Error, why.to_string())]),
                    None => Step::More,
                };
            }
            Phase::Write {
                writer,
                index: index @ Some(_),
            } => {
                let Some(toc) = index.as_mut() else {
                    return Step::More;
                };
                let done = match writer {
                    Writer::Html(w) => w.index(view, toc, self.budget),
                    Writer::Markdown(w) => w.index(view, toc, self.budget),
                    Writer::Latex(w) => w.index(doc, self.budget),
                };
                if done {
                    let toc = index.take().unwrap_or_default();
                    match writer {
                        Writer::Html(w) => w.begin(doc, view, toc, out),
                        Writer::Markdown(w) => w.begin(doc, view, toc, out),
                        Writer::Latex(w) => w.begin(doc, view, out),
                    }
                }
                return match view.error() {
                    Some(why) => Step::Done(vec![echo(EchoLevel::Error, why.to_string())]),
                    None => Step::More,
                };
            }
            Phase::Write { index: None, .. } => {}
        }
        let Phase::Write { writer, .. } = &mut render.phase else {
            return Step::More;
        };
        if !render.next.is_empty() {
            let from = render.out.len();
            while !render.next.is_empty() && render.out.len() - from < self.budget {
                let h = at(&render.doc, &render.next);
                let visit = filter::visits(h, &render.selection, &render.view.options);
                if visit {
                    let own = filter::own(h, &mut render.view);
                    let (view, out) = (&mut render.view, &mut render.out);
                    match writer {
                        Writer::Html(w) => w.headline(&own, view, out),
                        Writer::Markdown(w) => w.headline(&own, view, out),
                        Writer::Latex(w) => w.headline(&own, view, out),
                    }
                }
                advance(&render.doc, &mut render.next, visit);
            }
            if let Some(why) = render.view.error() {
                return Step::Done(vec![echo(EchoLevel::Error, why.to_string())]);
            }
            if !render.next.is_empty() {
                return Step::More;
            }
        }
        match writer {
            Writer::Html(w) => w.finish(&mut render.view, &mut render.out),
            Writer::Markdown(w) => w.finish(&mut render.view, &mut render.out),
            Writer::Latex(w) => w.finish(&mut render.out),
        }
        if let Some(why) = render.view.error() {
            return Step::Done(vec![echo(EchoLevel::Error, why.to_string())]);
        }
        let output = std::mem::take(&mut render.out);
        if let Sink::Pdf { open } = self.request.sink {
            return Step::Compile(super::pdf::Compile {
                tex: self.request.target.clone(),
                output,
                steps: self.configured.pdf_process.clone(),
                compiler: render.view.options.latex_compiler.trim().to_string(),
                open,
                left_out: render.view.options.ignored.clone(),
            });
        }
        let target = sink::target_label(self.request.sink, backend, &self.request.target);
        let ignored = &render.view.options.ignored;
        let done = if ignored.is_empty() {
            echo(EchoLevel::Info, format!("Exported {target}"))
        } else {
            echo(
                EchoLevel::Warn,
                format!(
                    "Exported {target}; not supported, so left out: {}",
                    ignored.join(", ")
                ),
            )
        };
        Step::Done(sink::effects(
            self.request.sink,
            backend,
            &self.request.target,
            output,
            done,
            &*self.host,
        ))
    }

    /// The blocks the file runs for this export, before its render: `None`
    /// when the export goes on to render now.
    fn evaluate(&mut self) -> Option<Step> {
        match &mut self.eval {
            Eval::Done => None,
            Eval::Asking(_) | Eval::Running(_) => Some(Step::Wait(Vec::new())),
            Eval::Unread => {
                let lines: Vec<&str> = self.text.lines().collect();
                let within = if self.request.subtree {
                    crate::babel::block::subtree_at(&lines, self.line)
                } else {
                    None
                };
                let runnable = self.scan.runnable(within);
                if runnable.is_empty() {
                    self.eval = Eval::Done;
                    return None;
                }
                if !self.configured.babel.enabled {
                    self.eval = Eval::Done;
                    self.notes.push(echo(
                        EchoLevel::Info,
                        "org: babel is off, so the export uses the results in the file",
                    ));
                    return None;
                }
                let languages = match &self.configured.babel.languages {
                    Ok(l) => l.clone(),
                    Err(why) => return Some(Step::Done(vec![echo(EchoLevel::Error, why.clone())])),
                };
                self.eval = Eval::Planning(Box::new(Planning::new(runnable, languages)));
                Some(Step::More)
            }
            Eval::Planning(planning) => {
                let lines: Vec<&str> = self.text.lines().collect();
                // Each block's plan reads the file a few times over.
                let count = (PLAN_LINES / lines.len().max(1)).max(1);
                let base = self.request.source.rsplit_once('/').map(|(dir, _)| {
                    if dir.is_empty() {
                        "/"
                    } else {
                        dir
                    }
                });
                match planning.step(&lines, self.request.buffer, self.id, base, count) {
                    Err(why) => Some(Step::Done(vec![echo(EchoLevel::Error, why)])),
                    Ok(true) => Some(Step::More),
                    Ok(false) => {
                        self.notes.append(&mut planning.notes);
                        let runs = std::mem::take(&mut planning.runs);
                        if runs.is_empty() {
                            self.eval = Eval::Done;
                            return Some(Step::More);
                        }
                        if self.configured.babel.confirm {
                            let effects = vec![Effect::Confirm(ConfirmPayload {
                                prompt: planning.prompt(),
                                yes_action: super::RUN_BLOCKS_NAME.to_string(),
                                // `n` still exports, with the results the
                                // file already has, as in emacs.
                                no_action: Some(super::KEEP_RESULTS_NAME.to_string()),
                                args: Args::List(vec![ArgValue::String(self.id.to_string())]),
                            })];
                            self.eval = Eval::Asking(runs);
                            return Some(Step::Wait(effects));
                        }
                        self.eval = Eval::Running(runs.len());
                        Some(Step::Run(runs))
                    }
                }
            }
        }
    }

    /// The `n`: render over the file as it is. `false` when the run was not
    /// waiting for an answer.
    fn declined(&mut self) -> bool {
        if !matches!(self.eval, Eval::Asking(_)) {
            return false;
        }
        self.eval = Eval::Done;
        true
    }

    /// The `y`: the runs to start, if the run is waiting for it.
    fn consented(&mut self) -> Option<Vec<RunRequest>> {
        let Eval::Asking(runs) = &mut self.eval else {
            return None;
        };
        let runs = std::mem::take(runs);
        self.eval = Eval::Running(runs.len());
        Some(runs)
    }

    /// One of the run's blocks settled; `true` when it was the last, and the
    /// run should go on.
    fn settle(&mut self, settled: Settled) -> bool {
        let Eval::Running(left) = &mut self.eval else {
            return false;
        };
        *left = left.saturating_sub(1);
        let last = *left == 0;
        if let Settled::Ran {
            key,
            body,
            insertion,
        } = settled
        {
            self.splice(&key, &body, insertion);
        }
        if last {
            self.eval = Eval::Done;
            self.reparse();
        }
        last
    }

    /// Put a block's fresh result into the copy, where babel would put it in
    /// the file.
    fn splice(&mut self, key: &str, body: &[String], insertion: Insertion) {
        let lines: Vec<&str> = self.text.lines().collect();
        let Some(block) = BlockKey::decode(key).and_then(|k| k.find(&lines)) else {
            return;
        };
        let Some(edit) = insertion_edit(&lines, self.text.ends_with('\n'), &block, body, insertion)
        else {
            return;
        };
        let start = byte_at(&self.text, edit.start);
        let end = byte_at(&self.text, edit.end);
        // The cursor's line, for a subtree export, moves with what changed
        // above it.
        if edit.start.0 < self.line {
            let added = edit.text.matches('\n').count() as i64;
            let removed = i64::from(edit.end.0 - edit.start.0);
            self.line = (i64::from(self.line) + added - removed).max(0) as u32;
        }
        self.text.replace_range(start..end, &edit.text);
    }

    /// Parse the copy again, now that it holds the blocks' results.
    fn reparse(&mut self) {
        self.chunks = chunks(&self.text, self.budget);
        self.next = 0;
        self.sections.clear();
        self.keywords.clear();
        self.targets = Targets::default();
        self.scan = Default::default();
    }

    /// The step between the parse and the render: build the tree, take the
    /// subtree when that is what was asked for, resolve the options, and
    /// start the output. `Err` when a subtree was asked for and the cursor is
    /// under no headline.
    fn prepare(&mut self) -> Result<Render, Vec<Effect>> {
        let mut doc = parse::assemble(std::mem::take(&mut self.sections));
        let name = file_name(&self.request.source);
        let options = if self.request.subtree {
            let Some(headline) = take_subtree(&mut doc, self.line) else {
                return Err(vec![echo(
                    EchoLevel::Warn,
                    "org: a subtree export needs the cursor under a headline",
                )]);
            };
            let options = Options::resolve(
                &self.configured.options,
                &self.keywords,
                Some((&headline.raw_title, &headline.properties)),
                name,
            );
            doc = Doc {
                preamble: Vec::new(),
                headlines: vec![headline],
            };
            options
        } else {
            Options::resolve(&self.configured.options, &self.keywords, None, name)
        };
        let selection = filter::selection(&doc, &options);
        let mut targets = std::mem::take(&mut self.targets);
        targets.set_live(
            filter::live(&doc, &selection, &options),
            !self.request.subtree,
        );
        let referred = if self.request.backend == Backend::Markdown {
            targets.referred(&*self.host)
        } else {
            HashSet::new()
        };
        let view = View::new(
            options,
            self.keywords.clone(),
            self.now,
            targets,
            Rc::clone(&self.host),
        )
        .leaving_out(std::mem::take(&mut self.scan).finish());
        let settings = match self.request.backend {
            Backend::Html => Settings::Html(html::Settings {
                done: self.ctx.done_keywords.clone(),
                source: self.request.source.clone(),
                body_only: self.request.body_only,
                stylesheet: self.configured.stylesheet.clone(),
            }),
            Backend::Markdown => Settings::Markdown(markdown::Settings {
                source: self.request.source.clone(),
                body_only: self.request.body_only,
                referred,
            }),
            Backend::Latex => Settings::Latex(latex::Settings {
                body_only: self.request.body_only,
            }),
        };
        let (outline, walk) = Outline::begin(&doc, &view);
        let next = if doc.headlines.is_empty() {
            Vec::new()
        } else {
            vec![0]
        };
        Ok(Render {
            doc,
            view,
            selection,
            phase: Phase::Outline {
                outline: Box::new(outline),
                walk,
                settings: Some(settings),
            },
            next,
            out: String::new(),
        })
    }
}

/// Take out of `doc` the innermost headline whose subtree holds `line`.
fn take_subtree(doc: &mut Doc, line: u32) -> Option<Headline> {
    // The last of a run of siblings that starts at or before `line` is the
    // one whose subtree holds it.
    let pick = |list: &[Headline]| list.iter().rposition(|h| h.line <= line);
    let mut path = vec![pick(&doc.headlines)?];
    while let Some(i) = pick(&at(doc, &path).children) {
        path.push(i);
    }
    let last = path.pop()?;
    let siblings = if path.is_empty() {
        &mut doc.headlines
    } else {
        let mut h = &mut doc.headlines[path[0]];
        for &i in &path[1..] {
            h = &mut h.children[i];
        }
        &mut h.children
    };
    Some(siblings.remove(last))
}

/// Cut `text` into chunks of about `budget` bytes, each starting at a
/// headline (or at the top of the file), as `(bytes, first line)`. One pass;
/// an empty text is one empty chunk, so a run always takes a step.
fn chunks(text: &str, budget: usize) -> Vec<(Range<usize>, u32)> {
    let mut out = Vec::new();
    let mut from = 0;
    let mut first = 0;
    let mut at = 0;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        if at - from >= budget && crate::headline::headline_level(line).is_some() {
            out.push((from..at, first));
            from = at;
            first = i as u32;
        }
        at += line.len();
    }
    out.push((from..at, first));
    out
}

/// The byte offset in `text` of `(line, byte)`; the line past the last is
/// the end of the text.
fn byte_at(text: &str, (line, byte): (u32, u32)) -> usize {
    let start = text
        .split_inclusive('\n')
        .take(line as usize)
        .map(str::len)
        .sum::<usize>();
    (start + byte as usize).min(text.len())
}

fn echo(level: EchoLevel, text: impl Into<String>) -> Effect {
    Effect::Echo(EchoPayload {
        level,
        text: text.into(),
    })
}

pub(crate) fn post(effects: Vec<Effect>) {
    // `err` only when the editor wired nowhere to post to; then there is no
    // one to tell, and the guest has no log of its own to write it to.
    let _ = host_services::post_effects(&effects);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lattice::plugin_host::types::FileAnchor;

    fn request(backend: Backend) -> Request {
        Request {
            buffer: 1,
            source: "/notes/plan.org".to_string(),
            target: target_path("/notes/plan.org", backend),
            backend,
            subtree: false,
            body_only: false,
            line: 0,
            sink: Sink::File,
        }
    }

    /// A host over an in-memory file system, with no roam index.
    struct Files(std::collections::HashMap<String, String>);

    impl Host for Files {
        fn read_file(&self, path: &str) -> Result<String, String> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| "missing".to_string())
        }
        fn id_file(&self, _: &str) -> super::super::IdFile {
            super::super::IdFile::NoIndex
        }
        fn output_buffer(&self, _: Backend) -> Option<(u32, String)> {
            None
        }
    }

    /// Run `request` over `text` to the end, and the steps it took.
    fn run_counting(request: &Request, text: Option<String>) -> (Vec<Effect>, usize) {
        run_with_files(request, text, &[], STEP_BYTES)
    }

    /// [`run_counting`] where an include may read `files`, in steps of about
    /// `budget` bytes.
    fn run_with_files(
        request: &Request,
        text: Option<String>,
        files: &[(&str, &str)],
        budget: usize,
    ) -> (Vec<Effect>, usize) {
        let host = Rc::new(Files(
            files
                .iter()
                .map(|(p, t)| (p.to_string(), t.to_string()))
                .collect(),
        ));
        let keywords = vec!["TODO DONE".to_string()];
        let mut run = match Run::new(
            1,
            request.clone(),
            text,
            keywords,
            When { local_secs: 0 },
            host,
            Configured {
                stylesheet: Stylesheet::None,
                ..Configured::default()
            },
        ) {
            Ok(run) => run.with_budget(budget),
            Err(effects) => return (effects, 0),
        };
        let mut steps = 0;
        loop {
            steps += 1;
            if let Step::Done(effects) = run.step() {
                return (effects, steps);
            }
        }
    }

    fn run(request: &Request, text: Option<String>) -> Vec<Effect> {
        run_counting(request, text).0
    }

    /// An org file of about `lines` lines shaped like real notes: headlines
    /// three deep with TODO keywords and tags, prose with emphasis, code,
    /// links and footnotes, lists with checkboxes, a table and a source block
    /// with its results in each section.
    fn representative(lines: usize) -> String {
        let mut text = String::from("#+TITLE: Notes\n#+AUTHOR: Someone\n#+OPTIONS: toc:2\n\n");
        let mut n = 0;
        while text.lines().count() < lines {
            n += 1;
            let stars = "*".repeat(1 + n % 3);
            let todo = ["", "TODO ", "DONE "][n % 3];
            text.push_str(&format!("{stars} {todo}Section {n} :work:notes:\n"));
            text.push_str(&format!(
                "Prose with *bold*, /italic/, =verbatim= and ~code~, a [[https://example.com/{n}][link]] \
                 and a footnote[fn:{n}]. It runs on long enough to wrap, as notes do, and\n\
                 carries [[file:other.org::*Section {n}][a link to another file]] and a <<target-{n}>>.\n\n\
                 - [ ] an open item with ~inline code~\n\
                 - [X] a done item\n  - nested, with a *bold* word\n\
                 1. first\n2. second\n\n\
                 | name | value | note |\n|------+-------+------|\n| a | {n} | /x/ |\n| b | 2 | =y= |\n\n\
                 #+begin_src rust\nfn f() -> u32 {{\n    {n}\n}}\n#+end_src\n\n\
                 #+RESULTS:\n: {n}\n\n\
                 [fn:{n}] The footnote's text.\n\n"
            ));
        }
        text
    }

    /// What an export costs: parse alone, then each backend end to end, over
    /// the 2,300-line size the editor's latency is measured on and the
    /// 20,000 lines the step budget is sized for. A step is what one events
    /// call pays, so the slowest step is the number that matters to the
    /// editor; the total is how long the user waits for the file. The crate
    /// is a cdylib, which `benches/` cannot link, so it is a timing test:
    /// `cargo test --release export_timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing; run with --release --ignored --nocapture"]
    fn export_timing() {
        use std::time::{Duration, Instant};
        for lines in [2_300, 20_000] {
            let text = representative(lines);
            let ctx = parse::Context::for_file(&text, vec!["TODO | DONE".to_string()]);
            let n = 20;
            let start = Instant::now();
            for _ in 0..n {
                std::hint::black_box(parse::parse(&text, &ctx));
            }
            println!(
                "{lines} lines ({} KiB): parse {:?}",
                text.len() / 1024,
                start.elapsed() / n
            );
            for backend in [Backend::Html, Backend::Markdown, Backend::Latex] {
                let (mut total, mut slowest, mut steps) = (Duration::ZERO, Duration::ZERO, 0);
                for _ in 0..n {
                    let host = Rc::new(Files(Default::default()));
                    let Ok(mut run) = Run::new(
                        1,
                        request(backend),
                        Some(text.clone()),
                        vec!["TODO | DONE".to_string()],
                        When { local_secs: 0 },
                        host,
                        Configured {
                            stylesheet: Stylesheet::None,
                            ..Configured::default()
                        },
                    ) else {
                        panic!("the run starts");
                    };
                    steps = 0;
                    loop {
                        steps += 1;
                        let at = Instant::now();
                        let done = matches!(run.step(), Step::Done(_));
                        let took = at.elapsed();
                        total += took;
                        slowest = slowest.max(took);
                        if done {
                            break;
                        }
                    }
                }
                println!(
                    "  {backend:?}: {:?} in {steps} steps, slowest step {slowest:?}",
                    total / n
                );
            }
        }
    }

    fn written(effects: &[Effect]) -> (&str, &str) {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::WriteToFile(w) => Some((w.path.as_str(), w.text.as_str())),
                _ => None,
            })
            .expect("a write")
    }

    /// A block's `:exports` decides what of it reaches the output, the
    /// default keeping the code and leaving the result out.
    #[test]
    fn exports_decides_what_of_a_block_is_exported() {
        let doc = |header: &str| {
            format!(
                "* A\n#+begin_src sh{header}\necho the-code\n#+end_src\n\n#+RESULTS:\n: the-result\n"
            )
        };
        let html = |header: &str| {
            let effects = run(&request(Backend::Html), Some(doc(header)));
            written(&effects).1.to_string()
        };
        let shown = |out: &str| (out.contains("the-code"), out.contains("the-result"));
        assert_eq!(shown(&html("")), (true, false));
        assert_eq!(shown(&html(" :exports results")), (false, true));
        assert_eq!(shown(&html(" :exports both")), (true, true));
        assert_eq!(shown(&html(" :exports none")), (false, false));
    }

    /// A run waiting on `n` blocks, over `text`: the parse done, the
    /// blocks' runs started.
    fn running(text: &str, n: usize) -> Run {
        let mut run = Run::new(
            1,
            request(Backend::Html),
            Some(text.to_string()),
            Vec::new(),
            When { local_secs: 0 },
            Rc::new(Files(Default::default())),
            Configured {
                stylesheet: Stylesheet::None,
                ..Configured::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        run.eval = Eval::Running(n);
        run
    }

    /// Step `run` to its end, and give what it wrote.
    fn finish(mut run: Run) -> String {
        loop {
            match run.step() {
                Step::More => {}
                Step::Done(effects) => return written(&effects).1.to_string(),
                other => panic!("{other:?}"),
            }
        }
    }

    /// `sh`'s fresh result for the block in `text` named `name`.
    fn ran(text: &str, name: &str, output: &str) -> Settled {
        let lines: Vec<&str> = text.lines().collect();
        let block = crate::babel::block::src_blocks(&lines)
            .into_iter()
            .find(|b| b.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no block {name}"));
        let spec =
            crate::babel::results::ResultsSpec::parse(&[], false).unwrap_or_else(|e| panic!("{e}"));
        Settled::Ran {
            key: BlockKey::of(&lines, &block).encode(),
            body: crate::babel::results::format(&spec, &[output.to_string()], 10, "sh"),
            insertion: Insertion::Replace,
        }
    }

    const RAN: &str = "* A\n#+NAME: one\n#+begin_src sh :exports results\necho x\n#+end_src\n\n\
                       #+RESULTS: one\n: stale-one\n\n\
                       #+NAME: two\n#+begin_src sh :exports both\necho y\n#+end_src\n";

    /// A block's fresh result replaces its old one in the export — not in
    /// the text the run was handed, a copy — and a block that gained its
    /// first result exports it. The export waits for the last.
    #[test]
    fn fresh_results_go_into_the_export_once_all_are_in() {
        let mut run = running(RAN, 2);
        assert!(!run.settle(ran(RAN, "one", "fresh-one")));
        let waits = (0..10).any(|_| matches!(run.step(), Step::Wait(_)));
        assert!(waits, "waits for the second, rather than writing");
        assert!(run.settle(ran(RAN, "two", "fresh-two")));
        let out = finish(run);
        assert!(
            out.contains("fresh-one") && out.contains("fresh-two"),
            "{out}"
        );
        assert!(!out.contains("stale-one"), "{out}");
        assert!(
            !out.contains("echo x"),
            "`:exports results` keeps only the result: {out}"
        );
        assert!(out.contains("echo y"), "{out}");
    }

    /// A block that did not run keeps the result in the file.
    #[test]
    fn a_block_that_did_not_run_keeps_its_old_result() {
        let mut run = running(RAN, 2);
        assert!(!run.settle(Settled::Not));
        assert!(run.settle(ran(RAN, "two", "fresh-two")));
        let out = finish(run);
        assert!(
            out.contains("stale-one") && out.contains("fresh-two"),
            "{out}"
        );
    }

    /// A result added above the cursor moves it, so a subtree export still
    /// takes the headline it was asked from.
    #[test]
    fn a_result_above_the_cursor_moves_it() {
        let text = "* A\n#+NAME: one\n#+begin_src sh :exports results\necho x\n#+end_src\n\
                    * B\nbody of b\n";
        let mut run = running(text, 1);
        run.request.subtree = true;
        run.line = 6;
        assert!(run.settle(ran(text, "one", "fresh")));
        assert_eq!(
            run.line, 9,
            "a blank line, `#+RESULTS:` and one line went in above"
        );
        let out = finish(run);
        assert!(out.contains("body of b") && !out.contains("fresh"), "{out}");
    }

    /// A PDF export ends in its compile, not a write: the `.tex` path and
    /// text, the configured process, and the file's own compiler.
    #[test]
    fn a_pdf_export_ends_in_its_compile() {
        let request = Request {
            sink: Sink::Pdf {
                open: Some(super::super::sink::Viewer::Editor(
                    super::super::sink::Place::Vsplit,
                )),
            },
            ..request(Backend::Latex)
        };
        let process = vec![super::super::pdf::ProcessStep {
            command: "latexmk".to_string(),
            args: Some(vec!["%f".to_string()]),
        }];
        let mut run = Run::new(
            1,
            request,
            Some("#+LATEX_COMPILER: lualatex\n#+BIND: x 1\n* A\n".to_string()),
            Vec::new(),
            When { local_secs: 0 },
            Rc::new(Files(Default::default())),
            Configured {
                pdf_process: process.clone(),
                ..Configured::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let compile = loop {
            match run.step() {
                Step::More => {}
                Step::Compile(c) => break c,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(compile.tex, "/notes/plan.tex");
        assert!(compile
            .output
            .starts_with("% Intended LaTeX compiler: lualatex\n"));
        assert!(compile.output.contains("\\section{A}"));
        assert_eq!(compile.steps, process);
        assert_eq!(compile.compiler, "lualatex");
        assert_eq!(
            compile.open,
            Some(super::super::sink::Viewer::Editor(
                super::super::sink::Place::Vsplit
            ))
        );
        assert_eq!(compile.left_out, vec!["#+BIND".to_string()]);
    }

    #[test]
    fn the_output_goes_beside_the_source_with_the_backends_extension() {
        assert_eq!(target_path("/a/plan.org", Backend::Html), "/a/plan.html");
        assert_eq!(target_path("/a/plan.org", Backend::Markdown), "/a/plan.md");
        assert_eq!(
            target_path("/a/v1.2/README", Backend::Latex),
            "/a/v1.2/README.tex"
        );
        assert_eq!(target_path("/a/x.y.org", Backend::Html), "/a/x.y.html");
        assert_eq!(target_path("/a/.notes", Backend::Html), "/a/.notes.html");
        assert_eq!(
            target_path("/a/.notes.org", Backend::Html),
            "/a/.notes.html"
        );
    }

    /// The write replaces the target whole and saves it: the file is for
    /// another reader.
    #[test]
    fn an_export_writes_the_whole_file_and_saves_it() {
        let effects = run(&request(Backend::Html), Some("* One <&>\n".to_string()));
        let Some(Effect::WriteToFile(w)) = effects.first() else {
            panic!("the write comes first: {effects:?}");
        };
        assert!(matches!(w.anchor, FileAnchor::Whole) && w.save);
        assert_eq!(w.path, "/notes/plan.html");
        assert!(w.text.contains("One &lt;&amp;&gt;</h2>"), "{}", w.text);
        assert!(w.text.contains("<title>plan</title>"), "{}", w.text);
    }

    #[test]
    fn the_title_keyword_names_the_document() {
        let effects = run(
            &request(Backend::Latex),
            Some("#+title: The Plan\n* One\n".to_string()),
        );
        assert!(written(&effects).1.contains("\\title{The Plan}\n"));
    }

    #[test]
    fn body_only_leaves_out_the_shell() {
        let mut r = request(Backend::Latex);
        r.body_only = true;
        let effects = run(&r, Some("* One\n".to_string()));
        let text = written(&effects).1;
        assert!(text.starts_with("\\section{One}\n"), "{text}");
        assert!(
            !text.contains("documentclass") && !text.contains("\\end{document}"),
            "{text}"
        );
    }

    /// The body of `text`'s export to Markdown, the plainest view of which
    /// headlines it holds; for a subtree export when `line` is given.
    fn markdown_body(text: &str, line: Option<u32>, files: &[(&str, &str)]) -> String {
        let mut r = request(Backend::Markdown);
        r.body_only = true;
        if let Some(line) = line {
            r.subtree = true;
            r.line = line;
        }
        let (effects, _) = run_with_files(&r, Some(text.to_string()), files, STEP_BYTES);
        written(&effects).1.to_string()
    }

    #[test]
    fn a_subtree_export_takes_the_headline_around_the_cursor() {
        let text = "* One\nx\n* Two\ny\n** Deep\n* Three\n";
        assert_eq!(
            markdown_body(text, Some(2), &[]),
            "# 1. Two\n\ny\n\n## 1.1. Deep\n"
        );
        let mut r = request(Backend::Latex);
        r.subtree = true;
        r.line = 2;
        let effects = run(&r, Some(text.to_string()));
        assert!(
            written(&effects).1.contains("\\title{Two}\n"),
            "the headline titles it"
        );
        // In the section of a nested headline, the innermost one.
        assert_eq!(
            markdown_body("* One\nx\n* Two\ny\n** Deep\nz\n", Some(5), &[]),
            "# 1. Deep\n\nz\n"
        );
        r.line = 0;
        let outside = run(&r, Some("text\n* One\n".to_string()));
        assert!(matches!(&outside[..], [Effect::Echo(_)]), "{outside:?}");
    }

    #[test]
    fn chunks_start_at_headlines_past_the_budget() {
        let text = "pre\n* a\nxxxxxxxx\n* b\n* c\nyy\n";
        // Line starts: pre 0, * a 4, xxxxxxxx 8, * b 17, * c 21, yy 25, end 28.
        assert_eq!(
            chunks(text, 10),
            vec![(0..17, 0), (17..28, 3)],
            "a chunk runs to the first headline at least the budget in"
        );
        assert_eq!(
            chunks(text, 1),
            vec![(0..4, 0), (4..17, 1), (17..21, 3), (21..28, 4)]
        );
        assert_eq!(chunks(text, 1000), vec![(0..28, 0)]);
        assert_eq!(chunks("", 10), vec![(0..0, 0)]);
        assert_eq!(chunks("a\r\n* b\r\n", 1), vec![(0..3, 0), (3..8, 1)]);
    }

    /// Export `text` in steps of about `budget` bytes and in one step, check
    /// the two are the same, and give the steps the first took.
    fn steps_change_nothing(r: &Request, text: &str, budget: usize) -> usize {
        let (stepped, steps) = run_with_files(r, Some(text.to_string()), &[], budget);
        let (whole, one) = run_with_files(r, Some(text.to_string()), &[], usize::MAX);
        assert!(
            one <= 6,
            "parse, resolve the blocks' `:exports`, prepare, outline, index, and the body \
             with its finish: {one}"
        );
        assert!(
            written(&stepped).1 == written(&whole).1,
            "{:?} in steps of {budget} differs from the export in one",
            r.backend
        );
        steps
    }

    /// A file larger than a step is parsed and written over several, and the
    /// output is the one exporting it whole gives, for every backend.
    #[test]
    fn a_large_file_is_exported_in_steps_into_the_whole_files_output() {
        let section = format!(
            "* TODO Heading\n{}\nSee [[*Heading]].\n",
            "- item\n  text\n".repeat(200)
        );
        let text = section.repeat(1 + 3 * STEP_BYTES / section.len());
        for backend in [Backend::Html, Backend::Markdown, Backend::Latex] {
            let mut r = request(backend);
            r.body_only = true;
            let steps = steps_change_nothing(&r, &text, STEP_BYTES);
            assert!(
                steps > 6,
                "{backend:?}: parse and render both step: {steps}"
            );
        }
    }

    /// The walk is the same however it is cut: each corpus file, exported a
    /// headline a step, is the export done in one, for every backend.
    #[test]
    fn steps_of_any_size_give_the_same_export() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/export/parse/testdata");
        for name in ["headlines", "lists", "blocks", "misc", "objects"] {
            let text = std::fs::read_to_string(dir.join(format!("{name}.org"))).expect("corpus");
            for backend in [Backend::Html, Backend::Markdown, Backend::Latex] {
                steps_change_nothing(&request(backend), &text, 1);
            }
        }
    }

    /// A macro that is not defined fails the export, in the title or in the
    /// body, and nothing is written.
    #[test]
    fn an_undefined_macro_fails_the_export() {
        for text in ["#+TITLE: {{{nope}}}\n* A\n", "* A\n** B\n{{{nope}}}\n"] {
            let effects = run(&request(Backend::Html), Some(text.to_string()));
            assert!(
                matches!(&effects[..], [Effect::Echo(e)] if e.text == "org: {{{nope}}} is not defined"),
                "{text}: {effects:?}"
            );
        }
        let effects = run(
            &request(Backend::Latex),
            Some("#+MACRO: who Ada\n#+TITLE: By {{{who}}}\n".to_string()),
        );
        assert!(written(&effects).1.contains("\\title{By Ada}\n"));
    }

    /// Includes are read a step each before the parse, their keywords
    /// apply, and a subtree export under them still finds the cursor's
    /// headline in the expanded text.
    #[test]
    fn includes_are_read_before_the_parse_and_the_cursor_follows() {
        let text = "#+INCLUDE: \"setup.org\"\n* One\n#+INCLUDE: \"part.org\"\n* Two\ny\n";
        let files = [
            (
                "/notes/setup.org",
                "#+MACRO: who Ada\n#+TITLE: By {{{who}}}\n",
            ),
            ("/notes/part.org", "* A\n* B\n"),
        ];
        let r = request(Backend::Latex);
        let (effects, steps) = run_with_files(&r, Some(text.to_string()), &files, STEP_BYTES);
        assert!(written(&effects).1.contains("\\title{By Ada}\n"));
        assert!(steps >= 4, "an include a step: {steps}");
        assert_eq!(
            markdown_body(text, None, &files),
            "# 1. One\n\n## 1.1. A\n\n## 1.2. B\n\n# 2. Two\n\ny\n"
        );
        // `y`, under Two, past both includes.
        assert_eq!(markdown_body(text, Some(4), &files), "# 1. Two\n\ny\n");

        let (effects, _) = run_with_files(
            &request(Backend::Html),
            Some(text.to_string()),
            &[],
            STEP_BYTES,
        );
        assert!(
            matches!(&effects[..], [Effect::Echo(e)] if e.text == "org: cannot include /notes/setup.org: missing"),
            "{effects:?}"
        );
    }

    #[test]
    fn a_buffer_closed_before_the_export_says_so() {
        let effects = run(&request(Backend::Html), None);
        assert!(matches!(&effects[..], [Effect::Echo(e)] if e.text.contains("plan.org")));
    }

    #[test]
    fn a_request_survives_the_crossing() {
        let r = request(Backend::Latex);
        let bytes = rmp_serde::to_vec(&r).unwrap();
        assert_eq!(rmp_serde::from_slice::<Request>(&bytes).unwrap(), r);
    }
}

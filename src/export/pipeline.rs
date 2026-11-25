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
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::types::{EchoLevel, EchoPayload, Effect};
use crate::time_format::When;
use std::rc::Rc;

/// The plugin event carrying a [`Request`] from the command to the events
/// seam.
pub(crate) const EV_REQUESTED: &str = "org/export-requested";

/// The plugin event a [`Run`] emits to itself to take its next step; the
/// payload is the run's id.
pub(crate) const EV_STEP: &str = "org/export-step";

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
}

/// The `org.export.*` options as they are now. A value an export cannot use
/// (an unknown compiler) is kept as written, for the export it would change
/// to refuse, naming it.
fn configured() -> Configured {
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
    Configured {
        options,
        stylesheet: Stylesheet::from_option(&crate::option_or("export.html-stylesheet", "default")),
    }
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
        configured(),
    ) {
        Ok(run) => {
            RUNS.with(|runs| {
                let mut runs = runs.borrow_mut();
                // A second export to the same file wins: the first would only
                // write what the second overwrites.
                runs.retain(|r| r.request.target != run.request.target);
                runs.push(run);
            });
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
        match runs[at].step() {
            Step::More => Some(Step::More),
            Step::Done(effects) => {
                runs.remove(at);
                Some(Step::Done(effects))
            }
        }
    });
    match step {
        Some(Step::More) => host_services::emit_event(EV_STEP, payload),
        Some(Step::Done(effects)) => post(effects),
        None => {}
    }
}

/// What a step leaves to do.
#[derive(Debug)]
pub(crate) enum Step {
    /// Another step is needed.
    More,
    /// The run is over; post these.
    Done(Vec<Effect>),
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
        );
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

fn echo(level: EchoLevel, text: impl Into<String>) -> Effect {
    Effect::Echo(EchoPayload {
        level,
        text: text.into(),
    })
}

fn post(effects: Vec<Effect>) {
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

    fn written(effects: &[Effect]) -> (&str, &str) {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::WriteToFile(w) => Some((w.path.as_str(), w.text.as_str())),
                _ => None,
            })
            .expect("a write")
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
            one <= 5,
            "parse, prepare, outline, index, and the body with its finish: {one}"
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

//! `org-export-mode` — HTML, Markdown and LaTeX from an org file (design:
//! lattice's `docs/dev/architecture/org-export.md`).
//!
//! Everything export is lives here: the mode, its `C-c C-e` layer, its
//! options, its commands and the dispatcher menu, and the pipeline that turns
//! a buffer into a file. `lib.rs` only calls in, from the seams that register
//! and from one arm in each dispatch `match`, as it does for babel.
//!
//! **Nothing heavy runs on the keystroke.** A command checks the target, emits
//! `org/export-requested` and returns; the export itself happens on the events
//! seam ([`pipeline`]), and its result is posted back.

pub(crate) mod common;
pub(crate) mod filter;
pub(crate) mod html;
pub(crate) mod include;
pub(crate) mod latex;
pub(crate) mod macros;
pub(crate) mod markdown;
pub(crate) mod options;
pub(crate) mod parse;
pub(crate) mod pipeline;
pub(crate) mod sink;
pub(crate) mod targets;

use crate::lattice::plugin_host::buffer::Document;
use crate::lattice::plugin_host::config::{register_option, OptionType};
use crate::lattice::plugin_host::events;
use crate::lattice::plugin_host::grammar::{register_action, register_ex_command};
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::modes::{
    register_mode, ActivationPolicy, BindingMode, ModeCapabilities, ModeDeclaration,
    ModeKeymapBinding, ModeKind,
};
use crate::lattice::plugin_host::tree_sitter::TreeSnapshot;
use crate::lattice::plugin_host::types::{
    ActionContext, ActionSpec, ArgDefault, ArgKind, ArgSpec, ArgValue, Args, EchoLevel,
    EchoPayload, Effect, Event, EventFilter, EventKind, ExCommandContext, ExCommandSpec,
    LatencyClass, OpenTransientPayload, SurfaceForm, TransientAction, TransientFlag,
    TransientGroup, TransientItem, TransientItemKind, TransientSpec,
};

/// What an export asks the host for while it runs, behind a trait so the
/// pipeline's tests can stand in for it.
pub(crate) trait Host {
    /// Read a file, as `#+INCLUDE:` does: the host's `read-file`, which only
    /// reads what the plugin's grant covers.
    fn read_file(&self, path: &str) -> Result<String, String>;
    /// The file that holds the node with this org id.
    fn id_file(&self, id: &str) -> IdFile;
    /// The buffer the last export to a buffer for `backend` made, and its
    /// text, while it is open.
    fn output_buffer(&self, backend: Backend) -> Option<(u32, String)>;
}

/// Where an org id lives, as the roam index knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdFile {
    /// In this file.
    In(String),
    /// Nowhere the index knows.
    Unknown,
    /// There is no index to ask.
    NoIndex,
}

/// The host the guest runs in.
pub(crate) struct Guest;

impl Host for Guest {
    fn read_file(&self, path: &str) -> Result<String, String> {
        crate::lattice::plugin_host::host_services::read_file(path)
    }

    fn id_file(&self, id: &str) -> IdFile {
        if crate::roam_index::is_empty() {
            return IdFile::NoIndex;
        }
        match crate::roam_index::node(id) {
            Some(node) => IdFile::In(node.file),
            None => IdFile::Unknown,
        }
    }

    fn output_buffer(&self, backend: Backend) -> Option<(u32, String)> {
        sink::guest_output_buffer(backend)
    }
}

pub(crate) use pipeline::Backend;
use sink::Sink;

/// The mode's id, also the name `mode_gates` ties to `org.export.enabled`.
pub(crate) const MODE: &str = "org-export-mode";

// Callback ids, 300 and up in every seam's space, so an export id is
// recognisable at a glance and none of org's or babel's moves. Never reuse
// one.

/// `C-c C-e`: open the dispatcher.
const DISPATCH: u32 = 300;
/// `:org-export-html`.
const TO_HTML: u32 = 301;
/// `:org-export-markdown`.
const TO_MARKDOWN: u32 = 302;
/// `:org-export-latex`.
const TO_LATEX: u32 = 303;
/// `:org-export-html-buffer`.
const TO_HTML_BUFFER: u32 = 304;
/// `:org-export-html-open`.
const TO_HTML_OPEN: u32 = 305;
/// `:org-export-markdown-buffer`.
const TO_MARKDOWN_BUFFER: u32 = 306;
/// `:org-export-markdown-open`.
const TO_MARKDOWN_OPEN: u32 = 307;
/// `:org-export-latex-buffer`.
const TO_LATEX_BUFFER: u32 = 308;

/// The events handler for an output buffer's minor going on.
pub(crate) const ON_OUTPUT_MODE: u32 = 300;

/// The `:` line parse every export command shares: `--subtree`, `--body-only`.
const SWITCHES_PARSE: u32 = 320;

/// What the dispatcher is opened FOR, on org's one transient source.
pub(crate) const TRANSIENT_KEY: &str = "export";

/// The dispatcher's switches as the last export left them, so they stay set
/// from one export to the next (emacs's do). Cleared on load, so a session
/// starts with both off, as emacs's does.
const SWITCHES_KEY: &str = "export/switches";

/// The two switches every export command takes, in its schema's order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Switches {
    /// Export the subtree at point, not the whole file (`C-s`).
    pub(crate) subtree: bool,
    /// Leave out the preamble or `<head>` (`C-b`).
    pub(crate) body_only: bool,
}

impl Switches {
    /// Read from a command's arguments: the schema's two slots, by position,
    /// which is how a menu's flags and a parsed `:` line both arrive.
    fn from_args(args: &Args) -> Self {
        let bool_at = |v: &[ArgValue], i: usize| matches!(v.get(i), Some(ArgValue::Bool(true)));
        match args {
            Args::List(v) => Self {
                subtree: bool_at(v, 0),
                body_only: bool_at(v, 1),
            },
            _ => Self::default(),
        }
    }

    fn to_args(self) -> Args {
        Args::List(vec![
            ArgValue::Bool(self.subtree),
            ArgValue::Bool(self.body_only),
        ])
    }

    fn remembered() -> Self {
        match host_services::store_get(SWITCHES_KEY).as_deref() {
            Some([subtree, body_only]) => Self {
                subtree: *subtree != 0,
                body_only: *body_only != 0,
            },
            _ => Self::default(),
        }
    }

    fn remember(self) {
        let _ = host_services::store_put(
            SWITCHES_KEY,
            &[u8::from(self.subtree), u8::from(self.body_only)],
        );
    }
}

/// Whether `callback` is one of export's action ids.
pub(crate) fn is_action(callback: u32) -> bool {
    callback == DISPATCH
}

/// Whether `callback` is one of export's ex-command ids.
pub(crate) fn is_ex_command(callback: u32) -> bool {
    command(callback).is_some()
}

/// What an ex-command id exports, and where to.
fn command(callback: u32) -> Option<(Backend, SinkKind)> {
    Some(match callback {
        TO_HTML => (Backend::Html, SinkKind::File),
        TO_MARKDOWN => (Backend::Markdown, SinkKind::File),
        TO_LATEX => (Backend::Latex, SinkKind::File),
        TO_HTML_BUFFER => (Backend::Html, SinkKind::Buffer),
        TO_MARKDOWN_BUFFER => (Backend::Markdown, SinkKind::Buffer),
        TO_LATEX_BUFFER => (Backend::Latex, SinkKind::Buffer),
        TO_HTML_OPEN => (Backend::Html, SinkKind::Open),
        TO_MARKDOWN_OPEN => (Backend::Markdown, SinkKind::Open),
        _ => return None,
    })
}

/// A command's sink before `org.export.open-with` is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SinkKind {
    File,
    Buffer,
    Open,
}

/// Whether `callback` is one of export's `:` line parse ids.
pub(crate) fn is_ex_parse(callback: u32) -> bool {
    callback == SWITCHES_PARSE
}

/// Every export option, `org.export.*`. Each is registered by the code that
/// reads it, so none is listed that does nothing.
pub(crate) fn register_options() {
    // Registered here so `:describe-option` has the words for it; the host
    // registers the same switch from the manifest's `mode_gates` and acts on
    // it.
    let _ = register_option(
        "export.enabled",
        OptionType::Boolean,
        "true",
        "Export org files (`org-export-mode`). Off removes the `C-c C-e` \
         dispatcher and the export commands' effect, and leaves the rest of \
         org on.",
    );
    let _ = crate::config_shape::register_option::<Vec<String>>(
        "export.directories",
        &Vec::new(),
        "Directories an export may write into, besides `org.directory` and \
         `org.roam-directory`. Empty by default, so a project's README.org \
         exports once its directory is listed here.",
    );
    let _ = register_option(
        "export.open-with",
        OptionType::String,
        "editor",
        "How `h o` / `m o` open the file they write: `editor` in lattice \
         (a PDF in pdf-mode in the GUI), `system` with the system's handler \
         for the file, a browser for HTML. In the TUI, where a PDF page is \
         alt text, `system` is the better choice for PDF.",
    );
    let _ = register_option(
        "export.html-stylesheet",
        OptionType::String,
        "default",
        "The stylesheet an HTML export uses: `default` inlines org's own, \
         `none` leaves style to the reader, and anything else is a URL or \
         path the page links to. ox-html's class names are kept, so a \
         stylesheet written for emacs's output styles this one.",
    );
    // The contents defaults, each under the file's `#+OPTIONS:` item.
    for item in options::ITEMS {
        let kind = match item.kind {
            options::Kind::Flag => OptionType::Boolean,
            options::Kind::Text => OptionType::String,
            options::Kind::Number => OptionType::Integer,
        };
        let _ = register_option(
            &format!("export.{}", item.name),
            kind,
            item.default,
            item.doc,
        );
    }
    for (name, default, doc) in [
        (
            "export.author",
            "",
            "The author an export names when the file has no `#+AUTHOR:`.",
        ),
        (
            "export.email",
            "",
            "The email an export names, with `org.export.with-email`, when \
             the file has no `#+EMAIL:`.",
        ),
        (
            "export.language",
            "en",
            "The language an export declares (`<html lang>`, babel's), when \
             the file has no `#+LANGUAGE:`.",
        ),
    ] {
        let _ = register_option(name, OptionType::String, default, doc);
    }
    for (name, default, doc) in [
        (
            "export.select-tags",
            vec!["export".to_string()],
            "Tags that select what is exported: when a headline carries one, \
             only those subtrees are. `#+SELECT_TAGS:` overrides it.",
        ),
        (
            "export.exclude-tags",
            vec!["noexport".to_string()],
            "Tags whose subtrees are left out. `#+EXCLUDE_TAGS:` overrides it.",
        ),
        (
            "export.html-head",
            Vec::new(),
            "Lines an HTML export puts in its `<head>`. A file's \
             `#+HTML_HEAD:` lines replace them.",
        ),
        (
            "export.html-head-extra",
            Vec::new(),
            "More `<head>` lines, after `org.export.html-head`'s. A file's \
             `#+HTML_HEAD_EXTRA:` lines replace them.",
        ),
    ] {
        let _ = crate::config_shape::register_option::<Vec<String>>(name, &default, doc);
    }
    // The LaTeX defaults, each under the file's keyword of the same name.
    for (name, default, doc) in [
        (
            "export.latex-compiler",
            "pdflatex",
            "The compiler a LaTeX export is written for, and a PDF export \
             runs: `pdflatex`, `xelatex` or `lualatex`. It picks the default \
             packages: `inputenc`/`fontenc` for pdflatex, `fontspec` (Unicode \
             text, system fonts) for the other two. `#+LATEX_COMPILER:` \
             overrides it.",
        ),
        (
            "export.latex-class",
            "article",
            "The LaTeX class an export uses: `article`, `report`, `book`, or one \
             of `org.export.latex-classes`. `#+LATEX_CLASS:` overrides it.",
        ),
        (
            "export.latex-class-options",
            "",
            "Options for the class's `\\documentclass`, brackets included \
             (`[a4paper,12pt]`); empty keeps the class's own. \
             `#+LATEX_CLASS_OPTIONS:` overrides it.",
        ),
        (
            "export.latex-src-block-backend",
            "verbatim",
            "How a LaTeX export writes source blocks: `verbatim`; `listings`, \
             highlighted by the listings package; or `minted`, highlighted by \
             Pygments, which needs the compiler run with `-shell-escape`. The \
             package is added to the preamble.",
        ),
        (
            "export.latex-image-width",
            ".9\\linewidth",
            "The width a LaTeX export draws an image at when its \
             `#+ATTR_LATEX:` gives no `:width` or `:height`. Empty draws it at \
             its own size.",
        ),
    ] {
        let _ = register_option(name, OptionType::String, default, doc);
    }
    let _ = crate::config_shape::register_option::<Vec<String>>(
        "export.latex-header",
        &Vec::new(),
        "Preamble lines every LaTeX export carries, after the default \
         packages and before the file's `#+LATEX_HEADER:` lines.",
    );
    let _ = crate::config_shape::register_option::<Vec<options::DeclaredClass>>(
        "export.latex-classes",
        &Vec::new(),
        "More LaTeX classes, emacs's `org-latex-classes`: one \
         `[[org.export.latex-classes]]` per class with a `name`, its \
         `sections` a headline level each (`[\"chapter\", \"section\"]`), and \
         optionally a `header`, the `\\documentclass` line and whatever the \
         class needs (default `\\documentclass[11pt]{<name>}`). A header \
         may place the preamble with `[DEFAULT-PACKAGES]`, `[PACKAGES]` and \
         `[EXTRA]`, or leave a part out with `[NO-DEFAULT-PACKAGES]` and its \
         peers. A class named `article`, `report` or `book` replaces the \
         built-in one.",
    );
}

/// The mode: a minor on org's major, whose layer holds export's prefix alone.
pub(crate) fn register() {
    register_mode(&ModeDeclaration {
        id: MODE.to_string(),
        kind: ModeKind::Minor,
        activation_policy: ActivationPolicy::Majors(vec!["org-mode".to_string()]),
        capabilities: ModeCapabilities::empty(),
        // Emacs's `org-export-dispatch` key.
        keymap: vec![ModeKeymapBinding {
            binding_mode: BindingMode::Normal,
            chord: "<C-c><C-e>".to_string(),
            command: "org-export-dispatch".to_string(),
        }],
        target_language: None,
        options: vec![],
    });
    // One per backend, on the buffer an export writes into, so its
    // activation hands the plugin that buffer's id (see `sink`). Only an
    // export turns one on.
    for backend in sink::BACKENDS {
        register_mode(&ModeDeclaration {
            id: sink::output_mode(backend).to_string(),
            kind: ModeKind::Minor,
            activation_policy: ActivationPolicy::Manual,
            capabilities: ModeCapabilities::empty(),
            keymap: vec![],
            target_language: None,
            options: vec![],
        });
    }
}

fn switch_arg(name: &str, doc: &str) -> ArgSpec {
    ArgSpec {
        name: name.to_string(),
        kind: ArgKind::Bool,
        doc: doc.to_string(),
        prompt: String::new(),
        default: ArgDefault::None,
        completion: None,
        picker: None,
    }
}

/// Export's commands.
pub(crate) fn register_grammar() {
    register_action(
        "org-export-dispatch",
        "Open the export dispatcher (emacs's `org-export-dispatch`)",
        &ActionSpec {
            args_schema: Vec::new(),
        },
        DISPATCH,
    );
    // Named `subtree` and `body-only`: the dispatcher's flag rows fill the
    // slots by these names.
    let spec = || ExCommandSpec {
        latency_class: LatencyClass::Reflex,
        accepts_bang: false,
        accepts_range: false,
        args_schema: vec![
            switch_arg("subtree", "export the subtree at point only"),
            switch_arg("body-only", "leave out the preamble / <head>"),
        ],
        surface_form: SurfaceForm::Keyword,
    };
    for (name, doc, id) in [
        (
            "org-export-html",
            "Export the buffer to HTML, beside the file. `--subtree` exports \
             the subtree at point, `--body-only` leaves out `<head>`.",
            TO_HTML,
        ),
        (
            "org-export-markdown",
            "Export the buffer to Markdown (GitHub-flavoured), beside the file.",
            TO_MARKDOWN,
        ),
        (
            "org-export-latex",
            "Export the buffer to LaTeX, beside the file.",
            TO_LATEX,
        ),
        (
            "org-export-html-buffer",
            "Export the buffer to HTML into `*Org HTML Export*`; exporting \
             again replaces its text.",
            TO_HTML_BUFFER,
        ),
        (
            "org-export-markdown-buffer",
            "Export the buffer to Markdown into `*Org MD Export*`.",
            TO_MARKDOWN_BUFFER,
        ),
        (
            "org-export-latex-buffer",
            "Export the buffer to LaTeX into `*Org LATEX Export*`.",
            TO_LATEX_BUFFER,
        ),
        (
            "org-export-html-open",
            "Export the buffer to HTML beside the file, then open it as \
             `org.export.open-with` says.",
            TO_HTML_OPEN,
        ),
        (
            "org-export-markdown-open",
            "Export the buffer to Markdown beside the file, then open it as \
             `org.export.open-with` says.",
            TO_MARKDOWN_OPEN,
        ),
    ] {
        register_ex_command(name, doc, &spec(), SWITCHES_PARSE, id);
    }
}

/// `register-events`: a session starts with the switches off, and the
/// output buffers' activations are heard.
pub(crate) fn register_events() {
    let _ = host_services::store_delete(SWITCHES_KEY);
    // Every minor's lifecycle is delivered (the filter has no mode field);
    // `on_event` keeps the output buffers' minors.
    events::subscribe(
        &EventFilter {
            kinds: Some(vec![EventKind::MinorActivated]),
            path_globs: None,
            major_modes: None,
            minor_modes: None,
        },
        ON_OUTPUT_MODE,
    );
}

/// `on-event` for [`ON_OUTPUT_MODE`].
pub(crate) fn on_event(ev: &Event) {
    if let Event::MinorActivated(m) = ev {
        if let Some(backend) = sink::output_backend(&m.mode) {
            sink::on_output_mode(backend, m.buffer as u32);
        }
    }
}

fn echo(level: EchoLevel, text: impl Into<String>) -> Vec<Effect> {
    vec![Effect::Echo(EchoPayload {
        level,
        text: text.into(),
    })]
}

/// `apply-action` for export's ids.
pub(crate) fn apply_action(
    callback: u32,
    _ctx: &ActionContext,
    _doc: &Document,
    _tree: Option<&TreeSnapshot>,
) -> Result<Vec<Effect>, String> {
    match callback {
        DISPATCH => Ok(vec![Effect::OpenTransient(OpenTransientPayload {
            source: crate::CAPTURE_TRANSIENT.to_string(),
            args: Args::String(TRANSIENT_KEY.to_string()),
        })]),
        other => Err(format!("org: unknown export action callback {other}")),
    }
}

/// `parse-ex-args` for export's commands: `--subtree` and `--body-only`, in
/// any order.
pub(crate) fn parse_ex_args(callback: u32, rest: &str) -> Result<Args, String> {
    if callback != SWITCHES_PARSE {
        return Err(format!("org: unknown export parse callback {callback}"));
    }
    let mut switches = Switches::default();
    for word in rest.split_whitespace() {
        match word {
            "--subtree" => switches.subtree = true,
            "--body-only" => switches.body_only = true,
            other => {
                return Err(format!(
                    "org: unknown export switch `{other}` — use --subtree or --body-only"
                ))
            }
        }
    }
    Ok(switches.to_args())
}

/// `apply-ex-command` for export's commands: check where the output goes,
/// then hand the export to the events seam.
pub(crate) fn apply_ex_command(
    callback: u32,
    ctx: &ExCommandContext,
    doc: &Document,
    _tree: Option<&TreeSnapshot>,
) -> Result<Vec<Effect>, String> {
    // A command is registered whatever the mode's state, so `:` reaches it
    // with export switched off; say so rather than export.
    if !crate::option_or("export.enabled", "true").eq_ignore_ascii_case("true") {
        return Ok(echo(
            EchoLevel::Warn,
            "org: export is off — set org.export.enabled to use it",
        ));
    }
    let Some((backend, kind)) = command(callback) else {
        return Err(format!(
            "org: unknown export ex-command callback {callback}"
        ));
    };
    let sink = match kind {
        SinkKind::File => Sink::File,
        SinkKind::Buffer => Sink::Buffer,
        SinkKind::Open => match sink::open_sink(&crate::option_or("export.open-with", "editor")) {
            Ok(sink) => sink,
            Err(why) => return Ok(echo(EchoLevel::Error, why)),
        },
    };
    let switches = Switches::from_args(&ctx.args);
    switches.remember();
    // A buffer export needs the file too: its name is the title's fallback,
    // and includes and links are relative to it.
    let Some(source) = doc.path() else {
        return Ok(echo(
            EchoLevel::Warn,
            "org: export works from the buffer's file — save the buffer first",
        ));
    };
    let target = match sink {
        Sink::Buffer => sink::buffer_name(backend).to_string(),
        Sink::File | Sink::Open { .. } => pipeline::target_path(&source, backend),
    };
    // Before anything runs, as capture checks its target before a word is
    // typed: a refusal found after a parse (or a babel run) wastes the work
    // and reads as a failed export rather than a missing grant.
    if sink != Sink::Buffer && host_services::can_write_file(&target).is_err() {
        return Ok(echo(
            EchoLevel::Error,
            format!(
                "org: org may not write {} — add its directory to org.export.directories",
                pipeline::file_name(&target)
            ),
        ));
    }
    if let Err(e) = pipeline::request(&pipeline::Request {
        buffer: ctx.buffer_id,
        source: source.clone(),
        target: target.clone(),
        backend,
        subtree: switches.subtree,
        body_only: switches.body_only,
        line: ctx.cursor.line,
        sink,
    }) {
        return Ok(echo(EchoLevel::Error, e));
    }
    Ok(echo(
        EchoLevel::Info,
        format!(
            "Exporting {} → {}…",
            pipeline::file_name(&source),
            sink::target_label(sink, backend, &target)
        ),
    ))
}

/// The dispatcher, emacs's `org-export-dispatch`: a backend group per format
/// and the two switches. Built per open, so the switches show what the last
/// export left them at.
pub(crate) fn dispatcher_menu() -> Result<TransientSpec, String> {
    let row = |key: &str, label: &str, kind| TransientItem {
        key: vec![key.to_string()],
        label: label.to_string(),
        description: String::new(),
        kind,
    };
    // Args `none`: the host fills the command's `subtree` / `body-only` slots
    // from the flags' state when the row fires.
    let to = |command: &str| {
        TransientItemKind::Action(TransientAction {
            command: command.to_string(),
            args: Args::None,
        })
    };
    let flag = |name: &str, default| {
        TransientItemKind::Flag(TransientFlag {
            name: name.to_string(),
            default,
        })
    };
    let switches = Switches::remembered();
    Ok(TransientSpec {
        title: "Export".to_string(),
        groups: vec![
            TransientGroup {
                label: "Options".to_string(),
                items: vec![
                    row("<C-s>", "subtree only", flag("subtree", switches.subtree)),
                    row("<C-b>", "body only", flag("body-only", switches.body_only)),
                ],
            },
            TransientGroup {
                label: "HTML".to_string(),
                items: vec![
                    row("hh", "to file", to("org-export-html")),
                    row("hH", "to buffer", to("org-export-html-buffer")),
                    row("ho", "to file and open", to("org-export-html-open")),
                ],
            },
            TransientGroup {
                label: "Markdown".to_string(),
                items: vec![
                    row("mm", "to file", to("org-export-markdown")),
                    row("mM", "to buffer", to("org-export-markdown-buffer")),
                    row("mo", "to file and open", to("org-export-markdown-open")),
                ],
            },
            TransientGroup {
                label: "LaTeX".to_string(),
                items: vec![
                    row("ll", "to file", to("org-export-latex")),
                    row("lL", "to buffer", to("org-export-latex-buffer")),
                ],
            },
            TransientGroup {
                label: String::new(),
                items: vec![row("q", "quit", TransientItemKind::Dismiss)],
            },
        ],
        footer: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switches_parse_in_any_order_and_nothing_else_does() {
        let parse = |s: &str| parse_ex_args(SWITCHES_PARSE, s).map(|a| Switches::from_args(&a));
        assert_eq!(parse("").unwrap(), Switches::default());
        let both = Switches {
            subtree: true,
            body_only: true,
        };
        assert_eq!(parse("--body-only --subtree").unwrap(), both);
        assert_eq!(Switches::from_args(&both.to_args()), both, "round-trips");
        assert_eq!(
            parse("--subtree").unwrap(),
            Switches {
                subtree: true,
                body_only: false
            }
        );
        assert!(parse("--toc").unwrap_err().contains("--toc"));
    }
}

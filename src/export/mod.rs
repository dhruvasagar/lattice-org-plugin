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

pub(crate) mod filter;
pub(crate) mod include;
pub(crate) mod macros;
pub(crate) mod options;
pub(crate) mod parse;
pub(crate) mod pipeline;
pub(crate) mod targets;

use crate::lattice::plugin_host::buffer::Document;
use crate::lattice::plugin_host::config::{register_option, OptionType};
use crate::lattice::plugin_host::grammar::{register_action, register_ex_command};
use crate::lattice::plugin_host::host_services;
use crate::lattice::plugin_host::modes::{
    register_mode, ActivationPolicy, BindingMode, ModeCapabilities, ModeDeclaration,
    ModeKeymapBinding, ModeKind,
};
use crate::lattice::plugin_host::tree_sitter::TreeSnapshot;
use crate::lattice::plugin_host::types::{
    ActionContext, ActionSpec, ArgDefault, ArgKind, ArgSpec, ArgValue, Args, EchoLevel,
    EchoPayload, Effect, ExCommandContext, ExCommandSpec, LatencyClass, OpenTransientPayload,
    SurfaceForm, TransientAction, TransientFlag, TransientGroup, TransientItem, TransientItemKind,
    TransientSpec,
};

/// What an export asks the host for while it runs, behind a trait so the
/// pipeline's tests can stand in for it.
pub(crate) trait Host {
    /// Read a file, as `#+INCLUDE:` does: the host's `read-file`, which only
    /// reads what the plugin's grant covers.
    fn read_file(&self, path: &str) -> Result<String, String>;
    /// Whether some file has a node with this org id; `None` when there is
    /// no index to ask.
    fn id_exists(&self, id: &str) -> Option<bool>;
}

/// The host the guest runs in.
pub(crate) struct Guest;

impl Host for Guest {
    fn read_file(&self, path: &str) -> Result<String, String> {
        crate::lattice::plugin_host::host_services::read_file(path)
    }

    fn id_exists(&self, id: &str) -> Option<bool> {
        (!crate::roam_index::is_empty()).then(|| crate::roam_index::node(id).is_some())
    }
}

pub(crate) use pipeline::Backend;

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
    matches!(callback, TO_HTML | TO_MARKDOWN | TO_LATEX)
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
    ] {
        register_ex_command(name, doc, &spec(), SWITCHES_PARSE, id);
    }
}

/// `register-events`: a session starts with the switches off.
pub(crate) fn register_events() {
    let _ = host_services::store_delete(SWITCHES_KEY);
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
    let backend = match callback {
        TO_HTML => Backend::Html,
        TO_MARKDOWN => Backend::Markdown,
        TO_LATEX => Backend::Latex,
        other => return Err(format!("org: unknown export ex-command callback {other}")),
    };
    let switches = Switches::from_args(&ctx.args);
    switches.remember();
    let Some(source) = doc.path() else {
        return Ok(echo(
            EchoLevel::Warn,
            "org: export writes beside the file — save the buffer first",
        ));
    };
    let target = pipeline::target_path(&source, backend);
    // Before anything runs, as capture checks its target before a word is
    // typed: a refusal found after a parse (or a babel run) wastes the work
    // and reads as a failed export rather than a missing grant.
    if host_services::can_write_file(&target).is_err() {
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
    }) {
        return Ok(echo(EchoLevel::Error, e));
    }
    Ok(echo(
        EchoLevel::Info,
        format!(
            "Exporting {} → {}…",
            pipeline::file_name(&source),
            pipeline::file_name(&target)
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
                items: vec![row("hh", "to file", to("org-export-html"))],
            },
            TransientGroup {
                label: "Markdown".to_string(),
                items: vec![row("mm", "to file", to("org-export-markdown"))],
            },
            TransientGroup {
                label: "LaTeX".to_string(),
                items: vec![row("ll", "to file", to("org-export-latex"))],
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

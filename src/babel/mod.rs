//! `org-babel-mode` — running source blocks (design: lattice's
//! `docs/dev/architecture/org-babel.md`).
//!
//! Everything babel is lives here: the mode, its keys, its options, its
//! command bodies. `lib.rs` only calls in — `register_*` from the seams that
//! register, and one arm in each dispatch `match` that hands babel's callback
//! ids back to this module. That is the design's §0 boundary drawn inside the
//! plugin too: babel is a feature set, so it is one mode with one owner, and
//! org's own `C-c C-c` does not grow a src-block branch (§5.4).

pub(crate) mod block;
pub(crate) mod header_args;
pub(crate) mod jobs;
pub(crate) mod results;
pub(crate) mod run;

use crate::lattice::plugin_host::buffer::Document;
use crate::lattice::plugin_host::config::{register_option, OptionType};
use crate::lattice::plugin_host::grammar::{register_action, register_ex_command};
use crate::lattice::plugin_host::modes::{
    register_mode, ActivationPolicy, BindingMode, ModeCapabilities, ModeDeclaration,
    ModeKeymapBinding, ModeKind,
};
use crate::lattice::plugin_host::tree_sitter::TreeSnapshot;
use crate::lattice::plugin_host::types::{
    ActionContext, ActionSpec, ApplyEditPayload, ArgDefault, ArgKind, ArgSpec, ArgValue, Args,
    EchoLevel, EchoPayload, Edit, EditKind, Effect, Event, EventFilter, EventKind,
    ExCommandContext, ExCommandSpec, LatencyClass, Position, Range, SurfaceForm,
};
use lattice_plugin_sdk::ConfigShape as ConfigShapeDerive;

/// The mode's id — also the name `mode_gates` ties to `org.babel.enabled`.
pub(crate) const MODE: &str = "org-babel-mode";

// Callback ids. Each seam dispatches its own export by its own number, so the
// spaces are separate; babel still takes one block, 200 and up, so a babel id
// is recognisable at a glance in every one of them and none of org's is
// shifted. Never reuse one — see the note on org's own ids in `lib.rs`.

/// `C-c C-c` in a src block (emacs `org-babel-execute-src-block`). An ACTION,
/// because only an action may decline: outside a src block the key belongs to
/// org's `C-c C-c` below it.
const EXECUTE_SRC_BLOCK: u32 = 200;

/// `:org-babel-execute-buffer` (`C-c C-v b`).
const EXECUTE_BUFFER: u32 = 201;
/// `:org-babel-execute-subtree` (`C-c C-v s`).
const EXECUTE_SUBTREE: u32 = 202;
/// `:org-babel-remove-result` (`C-c C-v k`).
const REMOVE_RESULT: u32 = 203;
/// `:org-babel-cancel`.
const CANCEL: u32 = 204;
/// The confirm's yes-half: run the blocks whose keys it carries. An ACTION,
/// because a confirm dispatches an action by name with typed arguments.
const RUN_CONFIRMED: u32 = 205;
/// [`RUN_CONFIRMED`]'s name — what the confirm names as its `yes-action`.
pub(crate) const RUN_CONFIRMED_NAME: &str = "org-babel-run-confirmed";

/// The `:` line parse for every babel command: none takes arguments.
const NO_ARGS_PARSE: u32 = 220;

/// The events handler for a babel job's `job-output` and `job-finished`.
pub(crate) const ON_JOB: u32 = 200;

/// Whether `callback` is one of babel's action ids.
pub(crate) fn is_action(callback: u32) -> bool {
    matches!(callback, EXECUTE_SRC_BLOCK | RUN_CONFIRMED)
}

/// Whether `callback` is one of babel's ex-command ids.
pub(crate) fn is_ex_command(callback: u32) -> bool {
    matches!(
        callback,
        EXECUTE_BUFFER | EXECUTE_SUBTREE | REMOVE_RESULT | CANCEL
    )
}

/// Whether `callback` is one of babel's `:` line parse ids.
pub(crate) fn is_ex_parse(callback: u32) -> bool {
    callback == NO_ARGS_PARSE
}

/// One entry of `org.babel.languages` — what a block in that language runs.
///
/// The table is also the permission: org's manifest asks for
/// `proc:spawn:{org.babel.languages:command}`, so `command` is exactly what
/// the host lets this plugin run, and only once the USER has written it
/// (design §3–4). Nothing here is a default for that reason.
#[derive(Debug, Clone, PartialEq, Eq, ConfigShapeDerive)]
pub(crate) struct Language {
    /// The word after `#+begin_src`.
    pub(crate) name: String,
    /// The program, on `PATH` or by path, compared as written.
    pub(crate) command: String,
    /// Its arguments; `{file}` is the script the body is written to.
    /// Absent means `["{file}"]`.
    pub(crate) args: Option<Vec<String>>,
    /// The script's extension. Absent means the language's name.
    pub(crate) ext: Option<String>,
    /// The built-in wrapper `:results value` uses (`python`, `ruby`,
    /// `node`, `sh`). Absent means the language has none, and `value`
    /// falls back to `output` (design §5.2).
    pub(crate) value: Option<String>,
}

/// Every babel option, `org.babel.*` (design §0).
pub(crate) fn register_options() {
    // Registered here so `:describe-option` has the words for it; the host
    // would register the same switch from the manifest's `mode_gates` if
    // this did not, and it is the host, not this plugin, that acts on it.
    let _ = register_option(
        "babel.enabled",
        OptionType::Boolean,
        "true",
        "Run source blocks (`org-babel-mode`). Off removes babel's keys and \
         commands and leaves the rest of org on. Inert until a language is \
         configured in `org.babel.languages`.",
    );
    let _ = register_option(
        "babel.confirm",
        OptionType::Boolean,
        "true",
        "Ask before running a block (emacs' `org-confirm-babel-evaluate`).",
    );
    let _ = crate::config_shape::register_option::<Vec<Language>>(
        "babel.languages",
        &Vec::new(),
        "The languages source blocks may run, one `[[org.babel.languages]]` \
         per language: `name` (the word after `#+begin_src`), `command` (the \
         program), `args` (default `[\"{file}\"]`, the block written to a \
         script), `ext` (the script's extension) and `value` (a built-in \
         `:results value` wrapper). Empty by default, and this list is also \
         what org may run: a program is run only if it is a `command` here.",
    );
    let _ = crate::config_shape::register_option::<Vec<String>>(
        "babel.tangle-directories",
        &Vec::new(),
        "Directories `:org-babel-tangle` may write into, besides \
         `org.directory`. Empty by default.",
    );
    let _ = register_option(
        "babel.min-lines-for-block-output",
        OptionType::Integer,
        "10",
        "Results this long or longer are written as an `#+begin_example` \
         block rather than `: ` lines (emacs' \
         `org-babel-min-lines-for-block-output`).",
    );
    let _ = register_option(
        "babel.max-output-lines",
        OptionType::Integer,
        "10000",
        "The most output lines a block's result keeps; the rest are counted \
         in a final `… N lines truncated` line.",
    );
}

/// The configured languages, or why they could not be read.
pub(crate) fn languages() -> Result<Vec<Language>, String> {
    match crate::config_shape::read_option::<Vec<Language>>("babel.languages") {
        Some(Ok(list)) => Ok(list),
        Some(Err(e)) => Err(format!("org: org.babel.languages is malformed: {e}")),
        None => Ok(Vec::new()),
    }
}

/// The mode. A minor on org's major, so its layer sits above org's and its
/// `C-c C-c` sees the key first (design §5.4).
pub(crate) fn register() {
    let bind = |chord: &str, command: &str| ModeKeymapBinding {
        binding_mode: BindingMode::Normal,
        chord: chord.to_string(),
        command: command.to_string(),
    };
    register_mode(&ModeDeclaration {
        id: MODE.to_string(),
        kind: ModeKind::Minor,
        activation_policy: ActivationPolicy::Majors(vec!["org-mode".to_string()]),
        capabilities: ModeCapabilities::empty(),
        // The `C-c C-v` prefix and its letters are emacs's
        // (`org-babel-key-prefix`). Tangle (`t`) arrives with tangling.
        keymap: vec![
            bind("<C-c><C-c>", "org-babel-execute-src-block"),
            bind("<C-c><C-v>b", "org-babel-execute-buffer"),
            bind("<C-c><C-v>s", "org-babel-execute-subtree"),
            bind("<C-c><C-v>k", "org-babel-remove-result"),
        ],
        target_language: None,
        options: vec![],
    });
}

/// Babel's commands.
pub(crate) fn register_grammar() {
    register_action(
        "org-babel-execute-src-block",
        "Run the source block at the cursor and write its output to `#+RESULTS:`; \
         anywhere else, `C-c C-c` is org's",
        &ActionSpec {
            args_schema: Vec::new(),
        },
        EXECUTE_SRC_BLOCK,
    );
    register_action(
        RUN_CONFIRMED_NAME,
        "Run the source blocks a babel confirm named (its `y`)",
        &ActionSpec {
            // ONE slot: a confirm's arguments are cut to the action's
            // declared arity, so every block's key travels packed in it.
            args_schema: vec![ArgSpec {
                name: "blocks".to_string(),
                kind: ArgKind::String,
                doc: "the blocks' keys, one per line".to_string(),
                prompt: String::new(),
                default: ArgDefault::None,
                completion: None,
                picker: None,
            }],
        },
        RUN_CONFIRMED,
    );
    let spec = || ExCommandSpec {
        latency_class: LatencyClass::Reflex,
        accepts_bang: false,
        accepts_range: false,
        args_schema: Vec::new(),
        surface_form: SurfaceForm::Keyword,
    };
    for (name, doc, id) in [
        (
            "org-babel-execute-buffer",
            "Run every source block in the buffer, in order.",
            EXECUTE_BUFFER,
        ),
        (
            "org-babel-execute-subtree",
            "Run every source block under the current headline, in order.",
            EXECUTE_SUBTREE,
        ),
        (
            "org-babel-remove-result",
            "Delete the `#+RESULTS:` of the source block at the cursor.",
            REMOVE_RESULT,
        ),
        (
            "org-babel-cancel",
            "Stop the source blocks running in this buffer.",
            CANCEL,
        ),
    ] {
        register_ex_command(name, doc, &spec(), NO_ARGS_PARSE, id);
    }
}

/// `register-events`: clear what a previous load left, and hear our jobs.
pub(crate) fn register_events() {
    jobs::sweep();
    crate::lattice::plugin_host::events::subscribe(
        &EventFilter {
            kinds: Some(vec![EventKind::JobOutput, EventKind::JobFinished]),
            path_globs: None,
            major_modes: None,
            minor_modes: None,
        },
        ON_JOB,
    );
}

/// `on-event` for [`ON_JOB`].
pub(crate) fn on_job_event(ev: Event) {
    match ev {
        Event::JobOutput(o) => jobs::on_output(o.id, o.lines),
        Event::JobFinished(f) => jobs::on_finished(f.id, f.outcome),
        _ => {}
    }
}

/// `register-signs`: the running sign. One glyph in both palettes, `▶`
/// (a BMP shape in the fallback set), so turning nerd fonts on or off does
/// not move the column — the agenda mark's reasoning.
pub(crate) fn define_signs() {
    let _ = crate::lattice::plugin_host::signs::define_sign(
        jobs::RUNNING_SIGN,
        &crate::lattice::plugin_host::signs::SignSpec {
            text: "▶".to_string(),
            fallback: "▶".to_string(),
            theme_element: jobs::RUNNING_SIGN_NS.to_string(),
            priority: 10,
            column: "mark".to_string(),
        },
    );
}

fn echo(level: EchoLevel, text: impl Into<String>) -> Vec<Effect> {
    vec![Effect::Echo(EchoPayload {
        level,
        text: text.into(),
    })]
}

/// The echo for a block whose language is not in `org.babel.languages`.
fn unconfigured(lang: &str) -> Vec<Effect> {
    if lang.is_empty() {
        return echo(
            EchoLevel::Warn,
            "org: this source block names no language — write it after #+begin_src",
        );
    }
    echo(
        EchoLevel::Warn,
        format!("org: no babel language \"{lang}\" — add it to org.babel.languages"),
    )
}

/// `apply-action` for babel's ids.
pub(crate) fn apply_action(
    callback: u32,
    ctx: &ActionContext,
    doc: &Document,
    _tree: Option<&TreeSnapshot>,
) -> Result<Vec<Effect>, String> {
    match callback {
        EXECUTE_SRC_BLOCK => {
            // One crossing for the whole buffer: the header merge reads
            // `#+PROPERTY:` lines and drawers anywhere above the block.
            let text = run::whole_text(doc);
            let lines: Vec<&str> = text.lines().collect();
            // Outside a src block this key is org's (a checkbox, a
            // headline's tags, a table): decline, and the layer below
            // answers as if babel's were not there.
            let Some(block) = block::src_block_at(&lines, ctx.cursor.line) else {
                return Ok(vec![Effect::Declined]);
            };
            Ok(run::confirm_or_start(ctx.buffer_id, &lines, &[block]))
        }
        RUN_CONFIRMED => {
            let packed = match &ctx.args {
                Args::List(v) => match v.first() {
                    Some(ArgValue::String(s)) => s.clone(),
                    _ => String::new(),
                },
                Args::String(s) => s.clone(),
                _ => String::new(),
            };
            let text = run::whole_text(doc);
            let lines: Vec<&str> = text.lines().collect();
            // Found again by key: the user may have typed while the prompt
            // was open, and what runs must be what the prompt named.
            let Some(blocks) = run::unpack(&lines, &packed).filter(|b| !b.is_empty()) else {
                return Ok(echo(
                    EchoLevel::Warn,
                    "org: the blocks changed while the prompt was open — nothing ran",
                ));
            };
            match run::check(ctx.buffer_id, &lines, &blocks) {
                Ok(checked) => Ok(run::start(ctx.buffer_id, &lines, &checked)),
                Err(e) => Ok(e),
            }
        }
        other => Err(format!("org: unknown babel action callback {other}")),
    }
}

/// `parse-ex-args` for babel's commands: none takes arguments.
pub(crate) fn parse_ex_args(callback: u32, rest: &str) -> Result<Args, String> {
    match callback {
        NO_ARGS_PARSE if rest.trim().is_empty() => Ok(Args::None),
        NO_ARGS_PARSE => Err("org: this command takes no arguments".to_string()),
        other => Err(format!("org: unknown babel parse callback {other}")),
    }
}

/// `apply-ex-command` for babel's commands.
pub(crate) fn apply_ex_command(
    callback: u32,
    ctx: &ExCommandContext,
    doc: &Document,
    _tree: Option<&TreeSnapshot>,
) -> Result<Vec<Effect>, String> {
    // A command is registered whatever the mode's state, so `:` reaches it
    // with babel switched off — say so rather than run (design §0: off
    // removes "the commands' effect").
    if !crate::option_or("babel.enabled", "true").eq_ignore_ascii_case("true") {
        return Ok(echo(
            EchoLevel::Warn,
            "org: babel is off — set org.babel.enabled to use it",
        ));
    }
    if callback == CANCEL {
        return Ok(run::cancel(ctx.buffer_id));
    }
    let text = run::whole_text(doc);
    let lines: Vec<&str> = text.lines().collect();
    match callback {
        EXECUTE_BUFFER => Ok(run::confirm_or_start(
            ctx.buffer_id,
            &lines,
            &block::src_blocks(&lines),
        )),
        EXECUTE_SUBTREE => {
            let Some((first, end)) = block::subtree_at(&lines, ctx.cursor.line) else {
                return Ok(echo(EchoLevel::Info, "org: not under a headline"));
            };
            let blocks: Vec<_> = block::src_blocks(&lines)
                .into_iter()
                .filter(|b| b.begin_line > first && b.end_line < end)
                .collect();
            Ok(run::confirm_or_start(ctx.buffer_id, &lines, &blocks))
        }
        REMOVE_RESULT => Ok(remove_result(
            ctx.buffer_id,
            &lines,
            text.ends_with('\n'),
            ctx.cursor.line,
        )),
        other => Err(format!("org: unknown babel ex-command callback {other}")),
    }
}

/// `:org-babel-remove-result`: delete the block's `#+RESULTS:` and the blank
/// lines between it and the block, so running a block and removing its
/// result leaves the file as it was.
fn remove_result(buffer: u32, lines: &[&str], ends_with_newline: bool, at: u32) -> Vec<Effect> {
    let Some(block) = block::src_block_at(lines, at) else {
        return echo(EchoLevel::Info, "org: not in a source block");
    };
    let Some(r) = block.results else {
        return echo(EchoLevel::Info, "org: this block has no result");
    };
    let gap_is_blank = r.keyword_line > block.end_line
        && (block.end_line + 1..r.keyword_line).all(|i| lines[i as usize].trim().is_empty());
    let first = if gap_is_blank {
        block.end_line + 1
    } else {
        r.keyword_line
    };
    let count = lines.len() as u32;
    let line_len = |n: u32| lines.get(n as usize).map_or(0, |l| l.len()) as u32;
    let (start, end) = if r.end_line + 1 < count || ends_with_newline {
        ((first, 0), (r.end_line + 1, 0))
    } else {
        // The result is the file's last line and has no newline: take the
        // one before it instead, so no newline is left dangling.
        (
            (first - 1, line_len(first - 1)),
            (r.end_line, line_len(r.end_line)),
        )
    };
    vec![Effect::ApplyEdit(ApplyEditPayload {
        target: buffer,
        edit: Edit {
            range: Range {
                start: Position {
                    line: start.0,
                    byte: start.1,
                },
                end: Position {
                    line: end.0,
                    byte: end.1,
                },
            },
            kind: EditKind::Replace(String::new()),
        },
        cursor: None,
    })]
}

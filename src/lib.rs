//! The reference org language plugin.
//!
//! Implements the `language-plugin` world: imports `register-language`,
//! exports `register-languages`. The grammar is compiled to wasm by this
//! crate's build.rs and baked in with `include_bytes!`; the queries ship as
//! source and are compiled host-side at registration, so a malformed one
//! fails at load naming the file.
//!
//! OM.2 adds the third seam: `modes`, declaring `org-mode` as the MAJOR for
//! the language registered above. Until majors crossed the seam a `.org` file
//! opened in `text-mode` however good the grammar was, because
//! `major_mode_id_for_lang` is a hand-written match over the `Lang` enum and
//! has no arm for a language the host has never heard of.
//!
//! OM.3 adds the fourth: `grammar`, contributing the promote/demote actions
//! and binding them in `org-mode`'s own keymap layer. This is where the plugin
//! starts EDITING — the actions read the buffer through the `borrow<document>`
//! handle `apply-action` receives and return `Effect::ApplyEdit`.

// A plugin that provides TWO seams needs a world that imports both, and a
// component implements exactly one world. Bundled plugins get theirs written
// into lattice's own `wit/` (`auto-pair-plugin` imports six interfaces) — but
// an EXTERNAL plugin cannot add a world to someone else's package.
//
// It does not need to. WIT `include` composes worlds, and `wit-bindgen`
// resolves an `inline` package against the interfaces found at `path`, so the
// plugin declares its own world locally and gets ONE `Guest` trait carrying
// both exports. Nothing in lattice changes to allow it.
//
// Three details, each of which is a build error if missed:
//   * `include` needs the VERSION (`@0.1.0`) — the resolver knows
//     `lattice:plugin-host@0.1.0`, not `lattice:plugin-host`.
//   * `generate_all` — without it wit-bindgen demands a `with` mapping for
//     every interface reached through the include.
//   * the inline package needs its own name, distinct from lattice's.
wit_bindgen::generate!({
    inline: r#"
        package lattice:org-plugin@0.1.0;
        world org-plugin {
            include lattice:plugin-host/language-plugin@0.1.0;
            include lattice:plugin-host/help-plugin@0.1.0;
            include lattice:plugin-host/modes-plugin@0.1.0;
            include lattice:plugin-host/grammar-plugin@0.1.0;
            include lattice:plugin-host/config-plugin@0.1.0;
            include lattice:plugin-host/media-plugin@0.1.0;
            // OM.A1: the agenda seam. Its three exports (`extensions` /
            // `begin` / `scan`) sit at WORLD level, so they land on the same
            // `Guest` trait as `register-languages` rather than behind an
            // interface — which is why they read differently below.
            include lattice:plugin-host/agenda-source-plugin@0.1.0;
            // OM.11: refile's target list. NOT `include
            // picker-source-plugin` — that world also imports `logging`, and
            // a component's imports must ALL be satisfiable on EVERY seam's
            // linker it is instantiated against, including the grammar seam's
            // SYNC one. `logging` is deliberately absent there, so that the
            // "no logging reachable from the grammar hot path" invariant is
            // structural rather than a matter of discipline. Declaring only
            // what the source actually uses keeps it that way.
            import lattice:plugin-host/host-services@0.1.0;
            // TK.3: one theme element per TODO state. NOT `include
            // theme-plugin` — that world imports `logging` too, and this
            // component is instantiated against the grammar seam's sync
            // linker where `logging` is absent. Same scar as the three
            // seams below; the export sits at world level, so it lands on
            // the same `Guest` trait as `register-languages`.
            import lattice:plugin-host/theme@0.1.0;
            export register-theme-elements: func();
            // OR.5b: the registry a picker plugin declares its sources
            // through. NOT `include picker-source-plugin` — that world also
            // imports `logging`, and this component is instantiated against
            // the grammar seam's sync linker where `logging` is absent. Same
            // scar as the seams below.
            import lattice:plugin-host/picker-registry@0.1.0;
            export lattice:plugin-host/picker-source@0.1.0;
            export register-picker-sources: func();
            // OR.7: org-roam nodes inside an `[[…]]` link. NOT `include
            // completion-source-plugin` — that world imports `logging` too,
            // and every import a component declares must be satisfiable on
            // EVERY linker it is instantiated against, including the grammar
            // seam's sync one where `logging` is deliberately absent. Same
            // scar as `picker-source` and `transient-source` above.
            export lattice:plugin-host/completion-source@0.1.0;
            // OC.3: the capture menu. Exported bare for the SAME reason
            // `picker-source` is — `transient-source-plugin` imports
            // `logging` and `project`, and an import a component declares
            // must be satisfiable on EVERY linker it is instantiated
            // against, including the grammar seam's sync one where
            // `logging` is deliberately absent. That is not a theoretical
            // constraint: OC.2 added one `logging::log` call, the component
            // started importing `logging`, and the WHOLE plugin stopped
            // instantiating.
            export lattice:plugin-host/transient-source@0.1.0;
            // OC.6: the clock's async side. NOT `include events-plugin` — that
            // world also imports `logging` and `project`, and every import a
            // component declares must be satisfiable on EVERY linker it is
            // instantiated against, including the grammar seam's sync one where
            // `logging` is deliberately absent. Same reasoning, and the same
            // scar, as `picker-source` and `transient-source` above.
            //
            // `events` gives the clock `wake-every` (the minute tick) and
            // `subscribe`; `ui` gives it the modeline segment. Both are on the
            // sync grammar linker too, for the reason just stated — and both
            // REFUSE there, because the store the grammar seam runs in carries
            // neither a wake nor a modeline context. That is where "the clock's
            // session is off the keystroke path" (design D6) is actually
            // enforced.
            import lattice:plugin-host/events@0.1.0;
            import lattice:plugin-host/ui@0.1.0;
            use lattice:plugin-host/types@0.1.0.{event};
            use lattice:plugin-host/events@0.1.0.{wake-id};
            export register-events: func();
            export on-event: func(handler: u32, ev: event);
            export on-wake: func(id: wake-id);
        }
    "#,
    path: "wit",
    world: "org-plugin",
    generate_all,
});

mod agenda;
mod archive;
mod capture;
mod capture_flow;
mod capture_target;
mod capture_templates;
mod checkbox;
mod clock;
mod headline;
mod links;
mod refile;
// OR.4: what makes a file's contents into roam nodes — the pure half.
mod roam;
// OR.4: the thin tree half — where the headlines and drawers are.
mod roam_complete;
mod roam_find;
mod roam_index;
mod roam_scan;
mod roam_tree;
mod table;
mod timestamp;
mod todo;
mod tree;

use exports::lattice::plugin_host::grammar_callbacks::Guest as GrammarCallbacks;
use exports::lattice::plugin_host::media::Guest as MediaProducer;
use exports::lattice::plugin_host::picker_source::Guest as PickerSource;
use lattice::plugin_host::buffer::Document;
use lattice::plugin_host::config::{get_option, register_option, OptionType};
use lattice::plugin_host::theme::{
    register_element, set_element_override, ColorRef, ModifierSet, StyleSpec as ThemeStyleSpec,
};
// OC.6: the clock's async side — `events` for the minute wake, `ui` for the
// modeline segment. Both are also on the sync grammar linker (a component's
// imports must resolve on every linker it instantiates against) and both refuse
// there, which is where "off the keystroke path" is actually enforced.
use lattice::plugin_host::events;
use lattice::plugin_host::grammar::{register_action, register_motion, register_text_object};
use lattice::plugin_host::help::register_topic;
use lattice::plugin_host::host_services;
use lattice::plugin_host::language::{register_language, ConcealRule, LanguageSpec};
use lattice::plugin_host::modes::{
    register_mode, ActivationPolicy, BindingMode, ModeCapabilities, ModeDeclaration,
    ModeKeymapBinding, ModeKind, ModeOptionOverride, OverridePriority,
};
use lattice::plugin_host::types::{
    ActionContext, ActionSpec, AppEffect, ArgDefault, ArgKind, ArgSpec, Args, DecorationContext,
    EchoLevel, EchoPayload, Edit, EditKind, Effect, EventFilter, EventKind, ExCommandContext,
    ExCommandSpec, FileAnchor, LatencyClass, MediaBlock, MediaFit, MotionContext, MotionResult,
    MotionSpec, OpenProviderViewPayload, OperatorContext, Position, Range, SurfaceForm,
    TextObjectContext, TextObjectSpec, UiZone, WriteToFilePayload,
};
use lattice::plugin_host::ui;

/// Callback ids for `apply-action`. The guest chooses these; the host only
/// hands them back (§6 — a plugin cannot forge a `CommandId`).
const PROMOTE_HEADLINE: u32 = 1;
const DEMOTE_HEADLINE: u32 = 2;
const PROMOTE_SUBTREE: u32 = 3;
const DEMOTE_SUBTREE: u32 = 4;

/// Callback ids for `apply-motion` — a separate space from the action ids
/// above, since the host dispatches each export by its own callback number.
const NEXT_HEADLINE: u32 = 1;
const PREV_HEADLINE: u32 = 2;
const PARENT_HEADLINE: u32 = 3;

/// Callback ids for `apply-text-object`.
const INNER_HEADLINE: u32 = 1;
const AROUND_HEADLINE: u32 = 2;
const INNER_SUBTREE: u32 = 3;
const AROUND_SUBTREE: u32 = 4;

/// `<Tab>` / `<S-Tab>` (OM.5).
const CYCLE: u32 = 5;
const CYCLE_GLOBAL: u32 = 6;

/// Structural editing (OM.6).
const MOVE_SUBTREE_UP: u32 = 7;
const MOVE_SUBTREE_DOWN: u32 = 8;
const META_RETURN: u32 = 9;
const TOGGLE_HEADING: u32 = 10;

/// `org-todo-mode` (OM.7).
const TODO_CYCLE: u32 = 11;
const TODO_CYCLE_BACK: u32 = 12;
const PRIORITY_CYCLE: u32 = 13;
const SET_TAGS: u32 = 14;
/// The second half of `<leader>o:` — the host dispatches this with the
/// prompt's text once the user submits.
const SET_TAGS_SUBMIT: u32 = 15;

/// `<leader>oI` (IM.7).
const TOGGLE_INLINE_IMAGES: u32 = 16;

/// `<C-Space>` (OM.8).
const TOGGLE_CHECKBOX: u32 = 17;

/// `<C-a>` / `<C-x>` (OM.9).
const TIMESTAMP_UP: u32 = 18;
const TIMESTAMP_DOWN: u32 = 19;

/// `<leader>oo` (OM.10).
const OPEN_LINK: u32 = 20;

/// `<CR>` (OL.3) — the same body as [`OPEN_LINK`], different miss.
///
/// A second action rather than a flag on the first, because the two
/// chords need OPPOSITE answers when the cursor is not on a link and
/// the effect vocabulary has no way to say "it depends".
///
/// `<leader>oo` must answer `Effect::None`: it is a multi-key chord
/// behind a plugin-owned prefix, and `Declined` re-runs a chord's
/// TRAILING key alone — so declining would fire whatever bare `o`
/// means, which in Normal mode is "open a line below and enter
/// Insert". A miss would start editing the buffer.
///
/// `<CR>` must answer `Effect::Declined`: it is a single key, there is
/// no trailing key to re-run, and declining is what lets the builtin
/// first-non-blank-of-next-line motion still work everywhere a link
/// is not.
const FOLLOW_LINK: u32 = 47;

/// TK.6 — fast select. `TODO_SELECT` opens the menu; `TODO_SET` is what a
/// row runs, carrying the chosen state in its own args.
///
/// Two actions rather than one for the reason TR.2a's per-row args exist:
/// one registered command, N rows, and the key you pressed decides which
/// state. The alternative is one command per keyword, and keywords are an
/// option read at menu-build time rather than at load — so they could not
/// be registered as commands at all.
const TODO_SELECT: u32 = 48;
const TODO_SET: u32 = 49;

/// The `transient-source` id. One per guest, so org's menus share it and
/// branch on what the open was FOR — the shape OC.4 established for
/// capture's two menus.
const ORG_TRANSIENT_TODO: &str = "todo";

/// `org-table-mode` (OM.12).
const TABLE_NEXT_CELL: u32 = 21;
const TABLE_PREV_CELL: u32 = 22;
const TABLE_ALIGN: u32 = 23;

/// Row / column structure (OM.13).
const TABLE_ROW_UP: u32 = 24;
const TABLE_ROW_DOWN: u32 = 25;
const TABLE_COL_LEFT: u32 = 26;
const TABLE_COL_RIGHT: u32 = 27;
const TABLE_INSERT_ROW: u32 = 28;
const TABLE_INSERT_COL: u32 = 29;
const TABLE_DELETE_ROW: u32 = 30;
const TABLE_DELETE_COL: u32 = 31;

/// `<leader>o$` (OM.6b) — the first action in this plugin that writes to a
/// file other than the one it fired in.
const ARCHIVE_SUBTREE: u32 = 32;

/// `<leader>or` (OM.11) — the two halves of refile. The first opens the
/// target picker; the second is what the picker's accept invokes, with the
/// chosen target as its args.
const REFILE: u32 = 33;
const REFILE_TO: u32 = 34;

/// The picker source refile opens. Named once — the guest registers it under
/// this id and the action names it in `Effect::OpenPicker`, and a typo between
/// the two would be a chord that opens nothing.
const REFILE_PICKER: &str = "org-refile";

/// `org-refile-targets`' `:maxlevel`. Three is deep enough to reach the
/// headings people actually file under and shallow enough that the list stays
/// scannable; `:picker org-refile 5` overrides it per invocation.
const DEFAULT_REFILE_MAX_LEVEL: usize = 3;

/// `<C-x>oc` (OM.11, moved at OC.1) — capture's two hops: the prompt, and what
/// the host dispatches with the submitted text.
const CAPTURE: u32 = 35;
const CAPTURE_SUBMIT: u32 = 36;

/// OC.3 — the chord's action now OPENS THE MENU rather than the prompt. It is
/// its own action rather than a mode of `org-capture` because the menu's rows
/// fire `org-capture` with a key, and one action that sometimes opens a menu
/// and sometimes captures would make the row's own args ambiguous.
const CAPTURE_MENU: u32 = 37;

/// AG.1 — `:org-agenda`, this plugin's first ex-command.
///
/// TWO ids because `register-ex-command` takes two callbacks: the `:` line's
/// rest is parsed by one and the effect produced by the other.
const AGENDA_PARSE: u32 = 39;
const AGENDA_APPLY: u32 = 40;

/// OC.6 — the clock's four chords. `<leader>oi` / `oO` / `oq` / `oj`, org's own
/// `C-c C-x C-i` / `C-o` / `C-q` / `C-j` spelled the way nvim-orgmode spells
/// them.
///
/// All four run in the GRAMMAR store, which is the only one that can edit a
/// buffer. The session, the minute wake and the modeline segment live in the
/// EVENTS store (design D6) and are reached from here by emitting a plugin
/// event — the two are separate `Store`s of this same component, with separate
/// linear memories, so the bus is the only bridge between them.
/// Shared parse callback: these four take no arguments, so one is enough.
const CLOCK_PARSE: u32 = 45;
/// OC.9 — `:org-clock-resume`, on the last-clocked entry.
const CLOCK_RESUME: u32 = 46;
const CLOCK_IN: u32 = 41;
const CLOCK_OUT: u32 = 42;
const CLOCK_CANCEL: u32 = 43;
const CLOCK_GOTO: u32 = 44;
/// OR.4 — `:org-roam-sync`, the escape hatch when the watcher missed something.
const ROAM_SYNC: u32 = 47;
/// OR.6 — `:org-roam-find-node`, and the create row's landing place.
const ROAM_FIND_NODE: u32 = 48;
const ROAM_CREATE_NODE: u32 = 49;
/// The create command takes the new note's title as its one argument, so it
/// needs its own parse callback rather than the clock's no-arg one.
const ROAM_CREATE_PARSE: u32 = 50;
/// OR.8 — `:org-roam-id-create`, making the headline at point a node.
const ROAM_ID_CREATE: u32 = 51;

/// OC.6 — the events handler ids. The guest picks these; the host hands them
/// back to `on-event`.
const ON_CLOCK_EVENT: u32 = 1;
/// OR.4: the watcher told us files under the roam directory changed.
const ON_ROAM_FILES_CHANGED: u32 = 2;

/// OR.6: `YYYYMMDDHHMMSS` in LOCAL time, for a new note's filename.
///
/// Local rather than UTC, because the stamp is what the user sees in a
/// directory listing and a note filed under yesterday's date because they live
/// east of Greenwich is the midnight bug in a different costume. The offset
/// comes from `local-utc-offset-seconds` (OC.4) — `wasi:clocks` is UTC and the
/// guest has no `TZ`.
fn roam_file_stamp() -> String {
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let local = utc + host_services::local_utc_offset_seconds() as i64;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, m, d) = agenda::civil_from_epoch_day(days);
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// The plugin events the grammar store emits and the events store consumes.
/// Private to this plugin: both ends are org, so the payload is org's own
/// business and the host moves the bytes without reading them.
const EV_CLOCK_STARTED: &str = "org/clock-started";
const EV_CLOCK_STOPPED: &str = "org/clock-stopped";
/// OR.4: `:org-roam-sync`, rung from the grammar store so the EVENT store does
/// the walk.
///
/// The bridge is not ceremony. A cold pass reads 706 files and parses the ones
/// that moved; running that inside the ex-command would do it on the dispatch
/// thread and freeze the editor for the duration. Ringing the doorbell costs a
/// bus publish and puts the work where every other expensive org thing already
/// lives (design D6, and the OC.1 pattern the clock uses).
const EV_ROAM_SYNC: &str = "org/roam-sync";

/// OC.4 — the fields menu's own submit, distinct from the prompt's.
///
/// Two actions rather than one that guesses: the prompt hop hands its action
/// `[text, buffer-name]` and the fields hop hands its action `[key, answers…]`,
/// and a single action would have to sniff which shape it got. Naming them
/// separately is what makes each one's arguments a fact rather than an
/// inference.
const CAPTURE_FIELDS_SUBMIT: u32 = 38;

/// `org-default-notes-file`, with no default. A key that silently creates
/// `capture.org` in whichever directory the editor happened to start in would
/// scatter notes across the filesystem; being told to set it once is better
/// than finding them later.
const DEFAULT_CAPTURE_FILE: &str = "";

/// `org-capture-templates`, reduced to one. See `capture.rs` for the
/// placeholders. The single-template path OM.11 shipped; superseded by
/// `capture-templates` and kept as the fallback while OC.3–OC.5 build the
/// multi-template engine on top of it.
const DEFAULT_CAPTURE_TEMPLATE: &str = "* TODO %?\n  %U";

/// OC.2: the template SET. Empty by default and deliberately so — the same
/// reasoning as `DEFAULT_CAPTURE_FILE`. A default set would name files the
/// user never chose, and capture would scatter notes into them.
const DEFAULT_CAPTURE_TEMPLATES: &str = "";

/// AF.3: `org-agenda-files`, and empty by default for the reason
/// `DEFAULT_CAPTURE_FILE` gives — a default would name files the user never
/// chose. Empty means "no opinion", and the host then scans the project root
/// exactly as it did before this option existed.
const DEFAULT_AGENDA_FILES: &str = "";

const GRAMMAR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/grammar.wasm"));

struct Component;

/// Fallbacks for the two options, used when the config seam is not wired (the
/// manifest may not declare `config`) or the option was somehow not registered.
/// `get_option` returning `none` must degrade to working defaults rather than
/// disabling the keys.
const DEFAULT_TODO_KEYWORDS: &str = "TODO | DONE";
const DEFAULT_HIGHEST_PRIORITY: &str = "C";

/// IM.7: images are OFF by default.
///
/// Two reasons, and neither is timidity. A buffer that silently reads and
/// decodes every referenced file the moment it opens is a surprise — an org
/// file can reference anything. And the TUI cannot draw them, so on by default
/// would mean every terminal user paying reserved rows for boxes of alt text
/// they did not ask for.
const DEFAULT_INLINE_IMAGES: &str = "false";

/// Whether inline images are enabled, read fresh so `:set` lands on the next
/// producer trigger.
fn inline_images_enabled() -> bool {
    get_option("inline-images")
        .unwrap_or_else(|| DEFAULT_INLINE_IMAGES.to_string())
        .eq_ignore_ascii_case("true")
}

/// The configured keyword sequence, read fresh on each use.
///
/// Read per keystroke rather than cached at load: `:set org.todo-keywords=…`
/// must take effect on the next press, and caching would need an
/// `OptionChanged` subscription to stay honest. The read is one host call
/// against an in-memory registry, which is far below the budget a chord has —
/// and cheaper than the buffer reads the same action already does.
/// An option's value, falling back to `default` when the config seam is
/// unwired or the option was never registered. `get_option` answering `none`
/// must degrade to something that works rather than to a dead key.
fn option_or(name: &str, default: &str) -> String {
    get_option(name).unwrap_or_else(|| default.to_string())
}

/// Split `org.agenda-files` into paths.
///
/// One per line. Blank lines and `#` comments are dropped so the option can be
/// annotated — a list of file paths is the kind of configuration people explain
/// to themselves in six months.
///
/// Free of the WIT and of any host type, so the parsing is unit-testable
/// without a running editor.
fn agenda_files(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn todo_keywords() -> Vec<String> {
    let spec = get_option("todo-keywords").unwrap_or_else(|| DEFAULT_TODO_KEYWORDS.to_string());
    let parsed = todo::parse_keywords(&spec);
    // A user who sets the option to nothing gets the default rather than a
    // dead key: an empty sequence would make `<leader>ot` cycle between one
    // state and itself.
    if parsed.is_empty() {
        todo::parse_keywords(DEFAULT_TODO_KEYWORDS)
    } else {
        parsed
    }
}

/// The lowest priority letter in the cycle — `C` gives `A`/`B`/`C`.
fn highest_priority() -> char {
    get_option("highest-priority")
        .unwrap_or_else(|| DEFAULT_HIGHEST_PRIORITY.to_string())
        .chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .unwrap_or('C')
}

/// OL.2 — what org hides when it renders.
///
/// Two rules and nothing else: the mechanism, the coordinate maths and
/// the mode scoping are all the host's (`conceal.md`). Org contributes
/// patterns; it does not learn how concealment works, and the host does
/// not learn what an org link is.
///
/// ## Why patterns and not the tree
///
/// Everything else in this plugin was migrated ONTO the tree in OT.x,
/// so this looks like a regression and is not. `tree-sitter-org` has no
/// `link` rule at all — `[[id:X][Title]]` is undifferentiated `expr`
/// tokens inside `item` or `paragraph`, so there is nothing to capture.
/// And the tree is absent during a reparse, so tree-driven conceal
/// would flicker between concealed and raw while the user types: a
/// pixel change to content they did not edit. `links.rs` already
/// recorded that second reason for its own text scanning.
///
/// This is "structure from the tree, characters from the text" applied
/// honestly rather than abandoned: a link has no structure in this
/// grammar, so there is none to read.
///
/// ## The bare rule keeps its target visible
///
/// `[[https://example.com]]` renders as `https://example.com`, not as
/// nothing. Emacs draws the same line, and the reason is not deference:
/// a link whose only text IS its target has nothing left to show once
/// the target is hidden, and an invisible activatable region is worse
/// than visible markup.
///
/// ## Declaration order does not matter
///
/// An earlier revision of the design said the described rule had to be
/// declared first or a described link would be matched as a bare one.
/// It was wrong twice: the host UNIONS every rule's hidden spans, so no
/// rule consumes text before another sees it; and independently the
/// bare pattern's `[^]]+` stops at the first `]`, so it never reaches a
/// described link's closing `]]` and cannot match it at all. The two
/// patterns are disjoint by construction — a property of how they are
/// written, which is why the host-side tests assert it.
fn conceal_rules() -> Vec<ConcealRule> {
    vec![
        // `[[target][description]]` → `description`.
        ConcealRule {
            pattern: r"(\[\[[^]]+\]\[)[^]]+(\]\])".to_string(),
            hide: vec![1, 2],
        },
        // `[[target]]` → `target`.
        ConcealRule {
            pattern: r"(\[\[)([^]]+)(\]\])".to_string(),
            hide: vec![1, 3],
        },
    ]
}

/// TK.3 — one theme element per TODO state.
///
/// `org-todo-keyword-faces` needs no new mechanism: an element override in the
/// theme scope IS that feature. What this function supplies is the *default*
/// each override sits on top of, and the inherit chain that keeps a keyword
/// the user adds later from rendering unstyled:
///
/// ```text
/// org.todo.<KEYWORD>  →  org.todo.active | org.todo.done  →  org.todo
/// ```
///
/// Every colour is a **palette key**, never a literal, so a colourscheme swap
/// recolours the whole set and a palette missing a key falls through the chain
/// rather than failing.
mod todo_theme {
    use super::*;

    /// `(fg, bold, italic, dim, done)` for org's conventional vocabulary.
    ///
    /// A keyword not listed here inherits `active` / `done` and is styled
    /// sensibly rather than not at all — which is what makes the load-time
    /// resolution of the keyword set tolerable instead of a cliff.
    fn conventional(name: &str) -> Option<(&'static str, bool, bool, bool)> {
        Some(match name {
            // Not done.
            "TODO" => ("red", true, false, false),
            "NEXT" => ("blue", true, false, false),
            "STARTED" | "IN-PROGRESS" | "READING" | "WATCHING" | "DOING" => {
                ("orange", false, false, false)
            }
            "WAITING" | "HOLD" | "BLOCKED" => ("yellow", false, true, false),
            "PROJECT" | "PROJ" => ("blue", false, false, false),
            "TO-READ" | "TO-WATCH" | "SOMEDAY" => ("yellow", false, false, false),
            // Done.
            "DONE" => ("green", true, false, false),
            // Abandoned, deliberately NOT `DONE`'s green. Emacs' own default
            // config paints both green; achieved versus abandoned is the
            // distinction someone scanning an agenda actually wants.
            "CANCELLED" | "CANCELED" | "KILL" | "ABANDONED" => ("overlay", false, false, true),
            _ => return None,
        })
    }

    /// Every modifier left unspecified, so the inherit chain decides.
    fn unset() -> ModifierSet {
        ModifierSet {
            bold: None,
            italic: None,
            underline: None,
            dim: None,
            reverse: None,
        }
    }

    fn spec(
        inherit: &str,
        fg: Option<&str>,
        bold: bool,
        italic: bool,
        dim: bool,
    ) -> ThemeStyleSpec {
        ThemeStyleSpec {
            inherit: Some(inherit.to_string()),
            fg: fg.map(|k| ColorRef::Palette(k.to_string())),
            bg: None,
            modifiers: ModifierSet {
                // `Some(false)` CLEARS an inherited modifier, which is why
                // `DONE` can be bold-and-not-dim under a dim parent.
                bold: Some(bold),
                italic: Some(italic),
                underline: None,
                dim: Some(dim),
                reverse: None,
            },
            scale: None,
        }
    }

    /// Register the base three plus one element per configured keyword.
    ///
    /// Names are auto-namespaced by the host (`todo.TODO` → `org.todo.TODO`),
    /// but `inherit` is NOT — it is resolved against the whole registry at
    /// theme-build time, so it names the full `org.todo.active`.
    pub fn register(kws: &crate::todo::Keywords) {
        let base = ThemeStyleSpec {
            inherit: None,
            fg: Some(ColorRef::Palette("text".to_string())),
            bg: None,
            modifiers: unset(),
            scale: None,
        };
        let _ = register_element("todo", "Base style for every TODO state.", &base);
        let _ = register_element(
            "todo.active",
            "Default for a not-done TODO state with no style of its own.",
            &spec("org.todo", Some("yellow"), true, false, false),
        );
        let _ = register_element(
            "todo.done",
            "Default for a done TODO state with no style of its own.",
            &spec("org.todo", Some("overlay"), false, false, true),
        );

        for k in &kws.all {
            let parent = if k.done {
                "org.todo.done"
            } else {
                "org.todo.active"
            };
            let s = match conventional(&k.name) {
                Some((fg, bold, italic, dim)) => spec(parent, Some(fg), bold, italic, dim),
                // Not a conventional name: inherit and set nothing, so the
                // chain decides and a user override still has somewhere to
                // land.
                None => ThemeStyleSpec {
                    inherit: Some(parent.to_string()),
                    fg: None,
                    bg: None,
                    modifiers: unset(),
                    scale: None,
                },
            };
            // A failure costs that keyword its own colour and nothing
            // else: it still resolves through the chain. Deliberately not
            // logged — this component must not import `logging`, or it
            // stops instantiating against the grammar seam's sync linker
            // (see the world declaration).
            let _ = register_element(&format!("todo.{}", k.name), "A TODO state.", &s);
        }
    }

    /// TK.5 — the user's own per-keyword styles, as OVERRIDES.
    ///
    /// Deliberately not element defaults. A default sits BELOW the active
    /// theme in the resolution stack, so a theme that styled
    /// `org.todo.WAITING` would beat the user's configuration — backwards
    /// from what `org-todo-keyword-faces` means. An override sits above it.
    ///
    /// A style naming a keyword that is not configured is refused by the
    /// host (the element does not exist) and skipped here: the alternative
    /// is an override that lands nowhere and reads as the feature not
    /// working.
    pub fn apply_overrides(styles: &[(String, crate::todo::KeywordStyle)]) {
        for (name, st) in styles {
            let spec = ThemeStyleSpec {
                inherit: None,
                fg: st.fg.as_deref().map(colour),
                bg: st.bg.as_deref().map(colour),
                modifiers: ModifierSet {
                    bold: st.bold,
                    italic: st.italic,
                    underline: st.underline,
                    dim: st.dim,
                    reverse: None,
                },
                scale: None,
            };
            let _ = set_element_override(&format!("todo.{name}"), &spec);
        }
    }

    /// A palette key, or a literal `#rrggbb`.
    ///
    /// A palette key is the path that survives a colourscheme swap, so it is
    /// the default reading; `#rrggbb` is the escape hatch for a colour no
    /// key expresses. An unparseable `#…` falls back to being treated as a
    /// key, where an unknown key resolves through the inherit chain — the
    /// forgiving resolution the seam documents, so a typo is "looks
    /// inherited" rather than a crash.
    fn colour(v: &str) -> ColorRef {
        if let Some(hex) = v.strip_prefix('#') {
            if hex.len() == 6 {
                if let Ok(rgb) = u32::from_str_radix(hex, 16) {
                    return ColorRef::LiteralRgb(rgb);
                }
            }
        }
        ColorRef::Palette(v.to_string())
    }
}

/// TK.4 — the TODO-keyword highlight rules, generated from the option.
///
/// The static query cannot carry these: which words are keywords is
/// configuration, and a `#any-of?` list compiled into a file is a guess at it.
/// What the query CAN carry — and does — is the structure, which is the part
/// the grammar actually knows.
mod todo_query {
    use super::todo::Keywords;

    /// One rule per keyword, each capturing to that keyword's own element.
    ///
    /// ```scheme
    /// (headline (item . (expr) @org.todo.WAITING)
    ///   (#eq? @org.todo.WAITING "WAITING"))
    /// ```
    ///
    /// `.` anchors the expr to the start of the item, so a `TODO` in the
    /// middle of a title stays prose and a headline written inside a
    /// `#+BEGIN_SRC` block is not a headline — the case a regex over lines
    /// cannot get right, which is most of why this stayed on the tree.
    ///
    /// The capture name IS the element name, because that is exactly what
    /// TK.1 made resolvable: a capture the host does not recognise as a
    /// builtin category, but which names a registered theme element, becomes
    /// `Style::Element`.
    pub fn rules(kws: &Keywords) -> String {
        let mut out = String::new();
        for k in &kws.all {
            // `parse_keyword` refuses anything that is not letters, digits,
            // `_` or `-`, so nothing here needs escaping. That refusal is the
            // only thing standing between a config value and a query
            // compiler, which is why it is a hard error there rather than a
            // normalisation.
            debug_assert!(
                k.name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-'),
                "TK.2 must have refused this keyword"
            );
            out.push_str(&format!(
                "(headline (item . (expr) @org.todo.{kw})\n  (#eq? @org.todo.{kw} \"{kw}\"))\n",
                kw = k.name
            ));
        }
        out
    }
}

impl Guest for Component {
    /// OM.7's options. Auto-namespaced by the host to `org.*`, so these are
    /// `org.todo-keywords` and `org.highest-priority` to the user.
    /// TK.3: one theme element per TODO state, so `org-todo-keyword-faces`
    /// is an ordinary element override rather than a new mechanism.
    ///
    /// Runs before `register_languages` — the loader drains `theme` first —
    /// which is what lets TK.4's generated query name these elements.
    fn register_theme_elements() {
        todo_theme::register(&todo::parse_todo_keywords(
            &get_option("todo-keywords").unwrap_or_else(|| DEFAULT_TODO_KEYWORDS.to_string()),
        ));
        // TK.5: the user's own per-keyword styles, applied as overrides so
        // they sit ABOVE the theme. Applied here rather than anywhere else
        // because this is the one export where the `theme` import is live —
        // the seam's store is dropped when this returns.
        let (styles, _problems) =
            todo::parse_keyword_styles(&get_option("todo-keyword-styles").unwrap_or_default());
        todo_theme::apply_overrides(&styles);
    }

    /// OR.5b: declare org's picker sources. One today (refile); roam's
    /// find-node and insert-node join it at OR.6/OR.7, which is the reason the
    /// seam had to stop being "one component IS one source".
    fn register_picker_sources() {
        lattice::plugin_host::picker_registry::register_picker_source(
            &lattice::plugin_host::types::PickerSourceSpec {
                id: REFILE_PICKER.to_string(),
                doc: "Org headlines a subtree can be refiled under".to_string(),
                args_schema: Vec::new(),
                args_hint: "[max-level]".to_string(),
                // Not live: the target set is the files on disk, and re-walking
                // them on every keystroke of the query would put a filesystem
                // walk on the typing path for a list that does not change while
                // the picker is open.
                live: false,
                // OR.5: refile moves a subtree UNDER an existing headline, so
                // there is nothing here to create — a "create" row would have
                // to invent a parent, which is not what the user asked for.
                // Roam's own pickers are where the label earns its keep.
                create_label: None,
            },
        );
        // OR.6: roam's find-node. The SECOND source from this component, which
        // is what OR.5b existed to make possible.
        lattice::plugin_host::picker_registry::register_picker_source(&roam_find::spec());
    }

    fn register_options() {
        let _ = register_option(
            "todo-keywords",
            OptionType::String,
            DEFAULT_TODO_KEYWORDS,
            "TODO keywords, one sequence per line, in emacs' \
             `org-todo-keywords` syntax:\n\n\
             \x20 sequence: TODO(t) NEXT(n) | DONE(d)\n\
             \x20 type: PROJECT TO-READ READING(!/!)\n\n\
             `|` separates not-done from done. `(t)` is a fast-select key. \
             `(@)` / `(!)` / `(@/!)` are logging specs \u{2014} parsed, not yet \
             acted on. A bare list with no `sequence:` / `type:` prefix is a \
             sequence, so the old flat spelling still means what it meant. \
             Cycling follows this option live; the per-keyword COLOURS resolve \
             at load, so a change needs a reload to recolour (emacs is the \
             same).",
        );
        let _ = register_option(
            "todo-keyword-styles",
            OptionType::String,
            "",
            "Per-keyword colours, one per line \u{2014} the org-shaped spelling of \
             `org-todo-keyword-faces`:\n\n\
             \x20 TODO: fg=red bold\n\
             \x20 WAITING: fg=orange italic\n\
             \x20 CANCELLED: fg=overlay dim\n\n\
             `fg=` / `bg=` take a palette key (which follows a colourscheme \
             swap) or a literal `#rrggbb`. Modifiers are `bold`, `italic`, \
             `underline`, `dim`, and `no-bold` and friends to CLEAR one \
             inherited from the state's default. These are applied as theme \
             OVERRIDES, so they beat the active colourscheme \u{2014} but \
             `:colorscheme` replaces the override set, so a swap drops them \
             until the next reload.",
        );
        let _ = register_option(
            "highest-priority",
            OptionType::String,
            DEFAULT_HIGHEST_PRIORITY,
            "The last priority letter `<leader>o,` cycles to. `C` gives A, B, C.",
        );
        // OC.2: the template SET, a string whose value is TOML. Forced, not
        // preferred — an option is `boolean | integer | string` and a template
        // is a record, so an array-of-tables cannot reach an option at all.
        // The cost is stated in the design fragment: `:describe-option` shows a
        // blob and `:set` cannot meaningfully edit it. If structured options
        // ever land the declaration migrates and the template language does
        // not change, which is why the language is defined by the parser here
        // rather than by the option's shape.
        let _ = register_option(
            "capture-templates",
            OptionType::String,
            DEFAULT_CAPTURE_TEMPLATES,
            "Your capture templates, as TOML: one `[[template]]` per entry with \
             `key`, `description`, `target = { file = \"…\", headline = \"…\" }` \
             and a `body`. Unset means `<C-x>oc` says so rather than guessing.",
        );
        let _ = register_option(
            "capture-file",
            OptionType::String,
            DEFAULT_CAPTURE_FILE,
            "Where `<C-x>oc` files a capture when `capture-templates` is unset. \
             Absolute, or relative to the editor's working directory.",
        );
        let _ = register_option(
            "capture-template",
            OptionType::String,
            DEFAULT_CAPTURE_TEMPLATE,
            "The single template a capture expands when `capture-templates` is \
             unset. `%?` is what you typed, `%U` / `%T` today's date inactive / \
             active, `%%` a literal percent.",
        );
        // AF.3: `org-agenda-files`, one path per line.
        //
        // Newline-separated and not `:`- or `,`-separated because a path may
        // contain either; not TOML-inside-a-string (`capture-templates`' shape)
        // because a list of paths is not a record and does not need one. The
        // cost is `capture-templates`' cost: options are
        // `boolean | integer | string` and there is no list kind, so
        // `:describe-option` shows a blob and `:set` cannot edit a multi-line
        // value meaningfully. If a list kind ever lands, this declaration
        // migrates and the meaning does not change.
        let _ = register_option(
            "agenda-files",
            OptionType::String,
            DEFAULT_AGENDA_FILES,
            "Files and directories the agenda scans, one path per line. A \
             directory is walked; a file is scanned whatever its extension. \
             `~` is expanded. Blank lines and `#` comments are ignored. Unset \
             scans the project root, as before.",
        );
        // OR.4: the corpus root. UNSET by default, and that default is the
        // feature's contract — see `roam_scan::roam_directory`.
        let _ = register_option(
            "roam-directory",
            OptionType::String,
            "",
            "Where your org-roam notes live. Unset means roam is inert: no \
             walk, no watcher, no index, and `<CR>` on an `[[id:\u{2026}]]` link \
             says the directory is not configured rather than blaming the \
             filesystem. The directory must also be in this plugin's \
             `fs:read` grant, or the walk reaches nothing.",
        );
        let _ = register_option(
            "roam-dailies-directory",
            OptionType::String,
            "daily",
            "Where `:org-roam-dailies-*` files live. Relative to \
             `org.roam-directory` unless it starts with `/`.",
        );
        let _ = register_option(
            "inline-images",
            OptionType::Boolean,
            DEFAULT_INLINE_IMAGES,
            "Draw `[[file:…]]` image links inline. Off by default: an org file \
             can reference anything, and only the GPUI peer can draw them.",
        );
    }

    fn register_languages() {
        let _ = register_language(&LanguageSpec {
            name: "org".to_string(),
            // The grammar's export is `tree_sitter_org`, which matches the
            // language name — so this is the common case and the field is
            // absent. It exists for grammars whose upstream name differs
            // (lattice's own `sql` rides `sequel`).
            grammar_name: None,
            extensions: vec!["org".to_string(), "org_archive".to_string()],
            grammar: GRAMMAR.to_vec(),
            // TK.4: the static structure, plus one generated rule per
            // configured keyword. Generated rather than compiled in because
            // which words are keywords is configuration — the hardcoded
            // `#any-of?` list this replaces could only ever guess, and
            // against a real config it missed nine of thirteen.
            highlights: Some(format!(
                "{}\n{}",
                include_str!("../queries/highlights.scm"),
                todo_query::rules(&todo::parse_todo_keywords(
                    &get_option("todo-keywords")
                        .unwrap_or_else(|| DEFAULT_TODO_KEYWORDS.to_string())
                ))
            )),
            folds: Some(include_str!("../queries/folds.scm").to_string()),
            // The language a `#+begin_src` block names, so its body is
            // highlighted by that language's own grammar (the markdown
            // fenced-block path, reused verbatim host-side).
            injections: Some(include_str!("../queries/injections.scm").to_string()),
            indents: None,
            textobjects: None,
            conceal_rules: conceal_rules(),
        });
    }

    /// `org-mode`, the major for the language declared above (OM.2).
    ///
    /// `target_language` is the whole point: it names the language by the same
    /// canonical string `register_language` used, the host indexes it, and a
    /// `.org` document resolves onto this mode through the ordinary
    /// `resolve_major_mode` path — the same one `rust-mode` takes. There is no
    /// org branch anywhere in the host.
    ///
    /// `Manual` activation is not a contradiction: the policy governs
    /// *explicit* activation, while a major bound to a language is activated by
    /// language resolution. It is the minors (org-todo, org-table) that will
    /// carry `Majors(["org-mode"])`.
    ///
    /// OM.3: the mode now carries its chords. Each binds to an action THIS
    /// plugin registered through the `grammar` seam, resolved by name against
    /// the command registry at bind time — which is why the loader must drain
    /// `grammar` before `modes` (OM.0 made that structural rather than a
    /// comment in this file's manifest).
    ///
    /// `<leader>oh` / `ol` and their capitals are the adapted nvim-orgmode set.
    /// nvim binds `<<` / `>>` / `<s` / `>s`, none of which are reachable here:
    /// `<` and `>` are TERMINAL operator bindings, so the trie resolves them on
    /// the first key and the second never arrives. Shadowing the operators
    /// inside org buffers would have bought the literal chords at the price of
    /// `>ap` and `ciw`, which is a bad trade for one filetype (org-mode.md
    /// §5.1). The letters are evil-org's directional `h`/`l`, so the mnemonic
    /// survives the move.
    // ── OC.6: the clock's async side ──────────────────────────────────────
    //
    // These three run in the EVENTS store — a different `Store`, with a
    // different linear memory, from the grammar store the chords run in. That
    // separation is the design (D6): the buffer edit must be synchronous, and
    // the session, the minute wake and the modeline must not be.

    /// Subscribe to org's own clock events, and declare the modeline element
    /// this plugin owns.
    ///
    /// The descriptor is registered here rather than on first use because
    /// `modeline.md` §6 says an owner registers on load — and because
    /// registering it lazily would mean the very first `emit-segment` lands in
    /// the content store keyed by an id no descriptor names, and renders
    /// nothing. Priority 8 puts it right of `lsp` (5) and `claude-code` (6) and
    /// left of `core.position` (10).
    fn register_events() {
        events::subscribe(
            &EventFilter {
                kinds: Some(vec![EventKind::Plugin]),
                path_globs: None,
                major_modes: None,
            },
            ON_CLOCK_EVENT,
        );
        ui::register_segment(CLOCK_SEGMENT, UiZone::Right, 8);
        // OR.4: the roam index. Everything below is a no-op when
        // `org.roam-directory` is unset — no walk, no watcher, no store write —
        // so an org user who keeps no zettelkasten pays nothing for it.
        //
        // The subscription is UNCONDITIONAL, and the walk is not. A plugin
        // loads before the user's `init.rs` has necessarily set
        // `org.roam-directory` (that is the documented `plugin-loaded`
        // pattern), so a subscription armed only when the option is already set
        // would never arm for the user who configures roam the documented way.
        // A subscription with no watch behind it receives nothing and costs
        // nothing; `sync_all` arms the watch when there is a directory to
        // watch.
        events::subscribe(
            &EventFilter {
                kinds: Some(vec![EventKind::FilesChanged]),
                path_globs: None,
                major_modes: None,
            },
            ON_ROAM_FILES_CHANGED,
        );
        // Boot does a full walk, because lattice was not running while the
        // corpus changed and a watcher cannot report what it did not see.
        // Unchanged files cost a read and a hash; only moved ones parse. A
        // no-op when roam is unconfigured.
        roam_scan::sync_all();
    }

    /// The grammar store told us a clock started or stopped.
    ///
    /// The payload is org's own — both ends are this plugin, and the host moves
    /// the bytes without reading them — so it is a tab-separated string rather
    /// than MessagePack, which would have cost a dependency for two fields.
    fn on_event(handler: u32, ev: Event) {
        // OR.4: a batch of changed paths. Re-indexing happens HERE, on the
        // event actor's own task, so it reaches the store with no keystroke
        // involved — which is the property the whole watcher exists for.
        if handler == ON_ROAM_FILES_CHANGED {
            if let Event::FilesChanged(paths) = ev {
                roam_scan::reindex(&paths);
            }
            return;
        }
        if handler != ON_CLOCK_EVENT {
            return;
        }
        let Event::Plugin(p) = ev else { return };
        match p.name.as_str() {
            EV_CLOCK_STARTED => {
                let payload = String::from_utf8_lossy(&p.payload).into_owned();
                let (started, title) = match payload.split_once('\t') {
                    Some((s, t)) => (s.parse::<i64>().unwrap_or(0), t.to_string()),
                    None => return,
                };
                // Replace rather than stack: a second start cancels the first
                // wake, so a clock-in that somehow raced a clock-out cannot
                // leave an orphan timer ticking for the rest of the session.
                SESSION.with(|s| {
                    if let Some(old) = s.borrow_mut().take() {
                        events::cancel_wake(old.wake);
                    }
                    // Every 60s. `on-wake` is a full guest call, so this is the
                    // rate the segment can change at — a clock reads in whole
                    // minutes, so a faster tick would buy nothing visible.
                    let wake = events::wake_every(60_000);
                    *s.borrow_mut() = Some(ClockSession {
                        started,
                        title,
                        wake,
                    });
                });
                // Immediately, not on the first wake — otherwise `◷ 0:00` takes
                // up to a minute to appear and the chord looks like it failed.
                publish_clock_segment();
            }
            EV_ROAM_SYNC => {
                roam_scan::sync_all();
            }
            EV_CLOCK_STOPPED => {
                SESSION.with(|s| {
                    if let Some(old) = s.borrow_mut().take() {
                        events::cancel_wake(old.wake);
                    }
                });
                publish_clock_segment();
            }
            _ => {}
        }
    }

    /// A minute passed. Re-render the segment.
    ///
    /// Deliberately re-reads the clock rather than counting wakes: a wake fires
    /// no *sooner* than its interval and may be late under load, so a counter
    /// would drift behind the real elapsed time and never catch up.
    fn on_wake(_id: u32) {
        publish_clock_segment();
    }

    fn register_modes() {
        let bind = |chord: &str, command: &str| ModeKeymapBinding {
            binding_mode: BindingMode::Normal,
            chord: chord.to_string(),
            command: command.to_string(),
        };
        register_mode(&ModeDeclaration {
            id: "org-mode".to_string(),
            kind: ModeKind::Major,
            activation_policy: ActivationPolicy::Manual,
            capabilities: ModeCapabilities::empty(),
            keymap: vec![
                bind("<leader>oh", "org-promote-headline"),
                bind("<leader>ol", "org-demote-headline"),
                bind("<leader>oH", "org-promote-subtree"),
                bind("<leader>oL", "org-demote-subtree"),
                // OM.6. `oK` / `oJ` are nvim-orgmode's verbatim — vim's own
                // `K` / `J` are not shadowed because these sit behind the
                // `<leader>o` prefix.
                bind("<leader>oK", "org-move-subtree-up"),
                bind("<leader>oJ", "org-move-subtree-down"),
                bind("<leader><CR>", "org-meta-return"),
                bind("<leader>o*", "org-toggle-heading"),
                // OM.6b: org's own `C-c C-x C-a`, spelled the way
                // nvim-orgmode spells it.
                bind("<leader>o$", "org-archive-subtree"),
                // OC.6 — org's `C-c C-x C-i` / `C-o` / `C-q` / `C-j`, in
                // nvim-orgmode's spelling. `i`, `O`, `q` and `j` were all free
                // under the `<leader>o` prefix; `J` is taken (move subtree
                // down) and is a different key from `j`.
                bind("<leader>oi", "org-clock-in"),
                bind("<leader>oO", "org-clock-out"),
                bind("<leader>oq", "org-clock-cancel"),
                bind("<leader>oj", "org-clock-goto"),
                // OC.9: org's `C-c C-x C-x`, in nvim-orgmode's spelling.
                bind("<leader>oR", "org-clock-resume"),
                // OM.11: opens the target picker; `org-refile-to` is the
                // second hop and is NOT bound — it is invoked by the picker's
                // accept, never typed.
                bind("<leader>or", "org-refile"),
                // IM.7: images are off by default, so the toggle is how most
                // users will ever turn them on.
                bind("<leader>oI", "org-toggle-inline-images"),
                // OM.8: org's own binding. `<C-Space>` is unbound in vim's
                // Normal mode, so nothing is shadowed.
                bind("<C-Space>", "org-toggle-checkbox"),
                // OM.9: `<C-a>` / `<C-x>` are vim's increment / decrement.
                // These SHADOW them inside org buffers and DECLINE off a
                // timestamp, so the builtin still works on ordinary numbers —
                // the one place in this plugin where declining is right.
                bind("<leader>oo", "org-open-link"),
                // OL.3: `<CR>` follows a link and declines everywhere
                // else, so the builtin motion still works. `<leader>oo`
                // stays — it is the explicit form, it works when the
                // cursor is outside the link's span, and removing a
                // working chord to make room costs muscle memory for
                // nothing.
                bind("<CR>", "org-follow-link"),
                bind("<C-a>", "org-timestamp-up"),
                bind("<C-x>", "org-timestamp-down"),
                // Motions, kept verbatim from nvim-orgmode — `]` and `[` are
                // prefixes rather than terminal bindings, so unlike `>>` / `<<`
                // these transplant unchanged. `g{` is emacs's
                // `outline-up-heading`; lattice's own `zp` walks the FOLD
                // hierarchy, which coincides with the headline hierarchy in org
                // but is a different thing, so both earn their place.
                bind("]]", "org-next-headline"),
                bind("[[", "org-prev-headline"),
                bind("g{", "org-parent-headline"),
                // Text objects (OM.4b). The host sees that these name TEXT
                // OBJECTS rather than actions and expands each into
                // `<operator><chord>` rows in this mode's own layer, plus a
                // Visual binding — so `dar` deletes a subtree through the
                // ORDINARY delete operator and no org-specific chord is
                // involved.
                //
                // `ir` / `ar` for the subtree, not `is` / `as`: `s` is already
                // vim's SENTENCE object, and nvim-orgmode uses `r` (subtRee)
                // for exactly that reason.
                bind("ih", "org-inner-headline"),
                bind("ah", "org-around-headline"),
                bind("ir", "org-inner-subtree"),
                bind("ar", "org-around-subtree"),
                // Visibility cycling. The behaviour is already native —
                // `AppEffect::CycleFoldAtCursor` was written for org and its
                // doc comment says so — so these actions route to it rather
                // than reimplementing anything. `z<Space>` / `z<Tab>` keep
                // working; this adds org's own keys on top.
                bind("<Tab>", "org-cycle"),
                bind("<S-Tab>", "org-global-cycle"),
            ],
            target_language: Some("org".to_string()),
            // MO.3 — the reason mode option overrides exist.
            //
            // Org's folding is structural: headline nesting IS the fold tree,
            // and `foldmethod=syntax` is what produces it. Until the seam
            // carried options, org could only *hope* the user had set that
            // globally — so `<Tab>` cycling worked on the author's machine and
            // did nothing on anyone else's, with no error to explain it. A
            // native major would simply have declared it; now this one can.
            //
            // A LAYER, not a write: it changes what `foldmethod` resolves to in
            // org buffers and leaves the user's global setting alone, so a
            // `foldmethod=indent` user still gets indent folds everywhere else.
            // A `:setlocal` in an org buffer still wins over it, which is the
            // right way round — the user gets the last word in their own buffer.
            options: vec![ModeOptionOverride {
                name: "foldmethod".to_string(),
                value: "syntax".to_string(),
                priority: OverridePriority::Normal,
            }],
        });

        // OM.7 — `org-todo-mode`, a MINOR riding the major above.
        //
        // A second mode rather than four more chords on `org-mode`, because
        // the two answer different questions. `org-mode` is what an org file
        // IS — structure, folding, motions — and is not optional. Keyword
        // cycling is a workflow: plenty of org files are outlines with no TODO
        // in them, and a user who wants the outliner without the task tracker
        // can `:org-todo-mode` off and keep everything else. Splitting it also
        // means `org.todo-keywords` has an obvious owner.
        //
        // `Majors(["org-mode"])` is the activation policy the WIT provides for
        // exactly this: the host activates the minor on buffers whose major is
        // named, and nowhere else. No `target_language` — a minor must not be
        // indexed as a language's major (the seam warns and ignores it).
        register_mode(&ModeDeclaration {
            id: "org-todo-mode".to_string(),
            kind: ModeKind::Minor,
            activation_policy: ActivationPolicy::Majors(vec!["org-mode".to_string()]),
            capabilities: ModeCapabilities::empty(),
            keymap: vec![
                bind("<leader>ot", "org-todo-cycle"),
                bind("<leader>oT", "org-todo-cycle-back"),
                // TK.6: fast select, ALONGSIDE cycling rather than instead
                // of it — emacs keeps both under
                // `org-use-fast-todo-selection`, and cycling is the faster
                // move when the next state is the one you want.
                bind("<leader>os", "org-todo-select"),
                bind("<leader>o,", "org-priority-cycle"),
                bind("<leader>o:", "org-set-tags"),
            ],
            target_language: None,
            // MO.1: this mode sets no options for its buffers.
            options: vec![],
        });

        // OC.1 — `org-global-mode`, the mode that is not about org FILES.
        //
        // `Universal`, and that is the whole point rather than a convenience.
        // Capture and the agenda are the two org verbs whose value is that
        // they work from wherever you happen to be: a capture chord that
        // only fires inside an org buffer is backwards, because the thought
        // you are trying not to lose arrives while you are reading code.
        // Every other org mode above is `Majors(["org-mode"])` or `Manual`
        // precisely because those verbs act ON an org file.
        //
        // **The prefix is `<leader>o`, not `<C-x>o`.** The design fragment
        // proposed `<C-x>o` and it cannot work: org's MAJOR keymap already
        // binds a TERMINAL `<C-x>` (timestamp decrement, OM.9), so inside an
        // org buffer `<C-x>` fires that and never waits for the second key.
        // A prefix in one layer and a terminal binding in another is the
        // ambiguity vim resolves with `timeoutlen`, which this editor does
        // not have.
        //
        // `<leader>o` costs nothing and breaks no muscle memory: it is where
        // capture already lived, and the ONLY change is that these two now
        // work outside an org file. It also drops the emacs `other-window`
        // shadowing the fragment had accepted as a price. Layered prefixes
        // compose — the major's `<leader>oh` and this minor's `<leader>oc`
        // both resolve, which the tests pin.
        //
        // AG.1: `oa` reaches THIS plugin's `:org-agenda`. The agenda VIEW is
        // still the multibuffer provider's and this plugin still only supplies
        // rows through the `agenda-source` seam — what changed is who owns the
        // trigger. A host-registered `:agenda` meant a feature every user calls
        // `org-agenda` shipped under a generic name that the plugin had no way
        // to correct from its own side.
        register_mode(&ModeDeclaration {
            id: "org-global-mode".to_string(),
            kind: ModeKind::Minor,
            activation_policy: ActivationPolicy::Universal,
            capabilities: ModeCapabilities::empty(),
            keymap: vec![
                bind("<leader>oa", "org-agenda"),
                // OM.11 bound this at `<leader>oc` on the org MAJOR, where it
                // could only fire inside an org file. `org-capture-submit` is
                // the second hop and is NOT bound — the host dispatches it on
                // submit.
                bind("<leader>oc", "org-capture-menu"),
                // OR.6 — finding a note belongs on the UNIVERSAL mode with
                // capture and the agenda, for their reason: the note you want
                // is rarely the file you are in. A `Majors(["org-mode"])`
                // binding would mean you can only reach your notes from a note.
                //
                // `<leader>on…` mirrors emacs org-roam's `C-c n …` prefix, so
                // the f keeps its meaning; users carry that muscle memory in.
                // NOT `<C-x>n…` — org's MAJOR binds a terminal `<C-x>`
                // (timestamp decrement, OM.9), and a prefix in one layer against
                // a terminal binding in another is the ambiguity vim settles
                // with `timeoutlen`, which this editor does not have.
                bind("<leader>onf", "org-roam-find-node"),
            ],
            target_language: None,
            // MO.1: this mode sets no options for its buffers.
            options: vec![],
        });

        // OM.A3 — `org-agenda-mode`, the fourth mode.
        //
        // MANUAL activation, and that is the whole reason it is a separate
        // mode rather than a wider policy on `org-todo-mode`. The agenda view
        // is a multibuffer: its major is `multibuffer-mode`, so
        // `Majors(["org-mode"])` never fires there — and
        // `Majors(["multibuffer-mode"])` would fire in project-search results
        // and magit diffs too, where `<leader>ot` means nothing.
        //
        // No activation policy can say "the buffer the agenda provider just
        // built", so the HOST activates it, on the strength of this plugin's
        // `view-mode` export naming it. The keymap and every handler body
        // stay here.
        //
        // The chords are the SAME actions `org-todo-mode` binds, deliberately
        // — cycling a TODO state means one thing, and the agenda is a place
        // you do it FROM. What differs is only which buffer the edit lands
        // in, and that is the multibuffer substrate's job: an edit in the
        // view is translated to source coordinates and written to the file
        // the row came from.
        //
        // `<leader>o:` (tags) is NOT here. It is a two-hop prompt flow whose
        // submit action re-reads the buffer, and the composed→source
        // translation of that second hop is untested; binding it would ship a
        // chord that might write to the wrong file.
        register_mode(&ModeDeclaration {
            id: "org-agenda-mode".to_string(),
            kind: ModeKind::Minor,
            activation_policy: ActivationPolicy::Manual,
            capabilities: ModeCapabilities::empty(),
            keymap: vec![
                bind("<leader>ot", "org-todo-cycle"),
                bind("<leader>oT", "org-todo-cycle-back"),
                bind("<leader>o,", "org-priority-cycle"),
            ],
            target_language: None,
            // MO.1: this mode sets no options for its buffers.
            options: vec![],
        });

        // OM.12 — `org-table-mode`, the third mode.
        //
        // Its `<Tab>` sits ABOVE `org-mode`'s in the layer order, and declines
        // when the cursor is not in a table. That makes the chain two hops:
        // table → headline cycle → whatever `<Tab>` natively means. The chain
        // only actually works because the dispatcher was fixed to peel ONE
        // keymap layer per decline (lattice `b9f6e3f6`); before that a decline
        // dropped every mode layer at once and skipped org-mode entirely.
        register_mode(&ModeDeclaration {
            id: "org-table-mode".to_string(),
            kind: ModeKind::Minor,
            activation_policy: ActivationPolicy::Majors(vec!["org-mode".to_string()]),
            capabilities: ModeCapabilities::empty(),
            keymap: vec![
                bind("<Tab>", "org-table-next-cell"),
                bind("<S-Tab>", "org-table-prev-cell"),
                bind("<leader>o|", "org-table-align"),
                // OM.13. Deliberately the outliner's directional letters
                // (`K`/`J` move, `H`/`L` for columns) so one mnemonic covers
                // subtrees and table rows alike.
                bind("<leader>tK", "org-table-row-up"),
                bind("<leader>tJ", "org-table-row-down"),
                bind("<leader>tH", "org-table-column-left"),
                bind("<leader>tL", "org-table-column-right"),
                bind("<leader>tr", "org-table-insert-row"),
                bind("<leader>tc", "org-table-insert-column"),
                bind("<leader>tdr", "org-table-delete-row"),
                bind("<leader>tdc", "org-table-delete-column"),
            ],
            target_language: None,
            // MO.1: this mode sets no options for its buffers.
            options: vec![],
        });
    }

    /// The promote/demote actions (OM.3).
    ///
    /// Registered as ACTIONS rather than operators: promote takes no motion and
    /// composes with nothing, which is what an action is for. The structural
    /// text objects that DO compose with operators (`ih`/`ah`/`is`/`as`) are a
    /// separate contribution, OM.4.
    fn register_grammar() {
        let spec = || ActionSpec {
            args_schema: Vec::new(),
        };
        // Motions, not actions: a motion composes with an operator, so `d]]`
        // deletes to the next headline and `3]]` takes a count — which is the
        // whole reason for the distinction (paramount goal #3). `jump: true`
        // records a position-history entry, matching `}` / `G` / `]]` in a
        // source file: a headline jump is somewhere you want `<C-o>` to bring
        // you back from.
        let motion = || MotionSpec {
            jump: true,
            // Exclusive: `d]]` deletes up to but not including the next
            // headline, which is what "delete this section" means.
            exclusive: true,
            args_schema: Vec::new(),
        };
        register_motion(
            "org-next-headline",
            "Move to the next headline, at any level",
            &motion(),
            NEXT_HEADLINE,
        );
        register_motion(
            "org-prev-headline",
            "Move to the previous headline, at any level",
            &motion(),
            PREV_HEADLINE,
        );
        register_motion(
            "org-parent-headline",
            "Move to the parent of the headline at the cursor",
            &motion(),
            PARENT_HEADLINE,
        );
        let tobj = || TextObjectSpec {
            args_schema: Vec::new(),
        };
        register_text_object(
            "org-inner-headline",
            "The headline's title, without its stars",
            &tobj(),
            INNER_HEADLINE,
        );
        register_text_object(
            "org-around-headline",
            "The whole headline line, stars included",
            &tobj(),
            AROUND_HEADLINE,
        );
        register_text_object(
            "org-inner-subtree",
            "A subtree's body — everything under the headline, not the headline",
            &tobj(),
            INNER_SUBTREE,
        );
        register_text_object(
            "org-around-subtree",
            "A whole subtree: the headline and everything under it",
            &tobj(),
            AROUND_SUBTREE,
        );
        // AG.1: `:org-agenda`. Every command this plugin ships is `org-`
        // prefixed; this is the one that was not, because it was the host's.
        //
        // The name is chosen here rather than derived by the host from the
        // plugin id, so the convention is the plugin's to keep. That is the
        // looser of the two arrangements and it is the deliberate one: a
        // source's users have a name for this in whatever ecosystem the source
        // came from, and only the source knows it.
        lattice::plugin_host::grammar::register_ex_command(
            "org-agenda",
            "Open the agenda: every dated row an agenda-source finds under the \
             configured files, grouped and ordered by the source, as editable \
             excerpts. Pass a path to scan somewhere else instead.",
            &ExCommandSpec {
                latency_class: LatencyClass::Reflex,
                accepts_bang: false,
                accepts_range: false,
                args_schema: vec![ArgSpec {
                    name: "root".to_string(),
                    kind: ArgKind::String,
                    doc: "directory or file to scan; defaults to the configured \
                          agenda files"
                        .to_string(),
                    prompt: "Agenda root: ".to_string(),
                    // Optional: a bare `:org-agenda` is the common call and
                    // must not prompt for a path the configuration already has.
                    default: ArgDefault::None,
                    completion: None,
                    picker: None,
                }],
                surface_form: SurfaceForm::Keyword,
            },
            AGENDA_PARSE,
            AGENDA_APPLY,
        );
        register_action(
            "org-cycle",
            "Cycle the headline at the cursor: folded, children, subtree",
            &spec(),
            CYCLE,
        );
        register_action(
            "org-global-cycle",
            "Cycle the whole buffer: overview, contents, show-all",
            &spec(),
            CYCLE_GLOBAL,
        );
        register_action(
            "org-promote-headline",
            "Promote the headline at the cursor one level",
            &spec(),
            PROMOTE_HEADLINE,
        );
        register_action(
            "org-demote-headline",
            "Demote the headline at the cursor one level",
            &spec(),
            DEMOTE_HEADLINE,
        );
        register_action(
            "org-promote-subtree",
            "Promote the headline at the cursor and its whole subtree",
            &spec(),
            PROMOTE_SUBTREE,
        );
        register_action(
            "org-demote-subtree",
            "Demote the headline at the cursor and its whole subtree",
            &spec(),
            DEMOTE_SUBTREE,
        );
        // OM.6 — structural editing.
        register_action(
            "org-move-subtree-up",
            "Swap the subtree at the cursor with the sibling above it",
            &spec(),
            MOVE_SUBTREE_UP,
        );
        register_action(
            "org-move-subtree-down",
            "Swap the subtree at the cursor with the sibling below it",
            &spec(),
            MOVE_SUBTREE_DOWN,
        );
        register_action(
            "org-meta-return",
            "Insert a new headline at the same level, after this subtree",
            &spec(),
            META_RETURN,
        );
        register_action(
            "org-table-next-cell",
            "Move to the next table cell, aligning the table",
            &spec(),
            TABLE_NEXT_CELL,
        );
        register_action(
            "org-table-prev-cell",
            "Move to the previous table cell, aligning the table",
            &spec(),
            TABLE_PREV_CELL,
        );
        for (name, doc_text, cb) in [
            ("org-table-row-up", "Move this table row up", TABLE_ROW_UP),
            (
                "org-table-row-down",
                "Move this table row down",
                TABLE_ROW_DOWN,
            ),
            (
                "org-table-column-left",
                "Move this column left",
                TABLE_COL_LEFT,
            ),
            (
                "org-table-column-right",
                "Move this column right",
                TABLE_COL_RIGHT,
            ),
            (
                "org-table-insert-row",
                "Insert a row below",
                TABLE_INSERT_ROW,
            ),
            (
                "org-table-insert-column",
                "Insert a column after",
                TABLE_INSERT_COL,
            ),
            ("org-table-delete-row", "Delete this row", TABLE_DELETE_ROW),
            (
                "org-table-delete-column",
                "Delete this column",
                TABLE_DELETE_COL,
            ),
        ] {
            register_action(name, doc_text, &spec(), cb);
        }
        register_action(
            "org-table-align",
            "Align the table under the cursor",
            &spec(),
            TABLE_ALIGN,
        );
        register_action(
            "org-open-link",
            "Open the link under the cursor: file, URL, or another headline",
            &spec(),
            OPEN_LINK,
        );
        register_action(
            "org-follow-link",
            "Follow the link under the cursor, else the ordinary <CR> motion",
            &spec(),
            FOLLOW_LINK,
        );
        register_action(
            "org-todo-select",
            "Choose a TODO state from a menu, keyed the way the option says",
            &spec(),
            TODO_SELECT,
        );
        register_action(
            "org-todo-set",
            "Set the headline's TODO state to the one named in args",
            &spec(),
            TODO_SET,
        );
        register_action(
            "org-timestamp-up",
            "Step the timestamp component under the cursor forward",
            &spec(),
            TIMESTAMP_UP,
        );
        register_action(
            "org-timestamp-down",
            "Step the timestamp component under the cursor back",
            &spec(),
            TIMESTAMP_DOWN,
        );
        register_action(
            "org-toggle-checkbox",
            "Toggle the checkbox on this line and update the parent's cookie",
            &spec(),
            TOGGLE_CHECKBOX,
        );
        register_action(
            "org-toggle-inline-images",
            "Show or hide inline images for org buffers",
            &spec(),
            TOGGLE_INLINE_IMAGES,
        );
        // OC.6 — the clock. Four chords, each doing exactly one buffer edit and
        // then telling org's own async side what happened.
        // OC.7: EX-COMMANDS, not actions — so `:org-clock-in` works as well as
        // `<leader>oi`. One registration serves both surfaces: a mode keymap
        // binding resolves a command by NAME and does not care about its kind,
        // while `:` resolves only ex-commands (`excommand.rs` answers `Unknown`
        // for an action, and there is no `action:` kind-prefix). Registering as
        // an action would have given the chord and nothing else.
        //
        // That is design §5.2.1's unification made real rather than restated:
        // the `:` line is a parser front-end onto the one dispatcher, so the
        // same entry is reachable both ways with one body behind it.
        //
        // Reachable at all only because of OC.10 — before it an ex-command got
        // no cursor and no buffer id, so three of these four could not have been
        // written in this form.
        let clock_ex = || ExCommandSpec {
            latency_class: LatencyClass::Reflex,
            accepts_bang: false,
            accepts_range: false,
            args_schema: Vec::new(),
            surface_form: SurfaceForm::Keyword,
        };
        lattice::plugin_host::grammar::register_ex_command(
            "org-clock-in",
            "Start a clock on the entry at the cursor.",
            &clock_ex(),
            CLOCK_PARSE,
            CLOCK_IN,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-clock-out",
            "Stop the running clock on the entry at the cursor, writing its end \
             stamp and elapsed time.",
            &clock_ex(),
            CLOCK_PARSE,
            CLOCK_OUT,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-clock-cancel",
            "Discard the running clock on the entry at the cursor, leaving no \
             trace of it.",
            &clock_ex(),
            CLOCK_PARSE,
            CLOCK_CANCEL,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-clock-resume",
            "Start a clock on the last entry that was clocked, wherever it is.",
            &clock_ex(),
            CLOCK_PARSE,
            CLOCK_RESUME,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-clock-goto",
            "Jump to the entry the running clock is on.",
            &clock_ex(),
            CLOCK_PARSE,
            CLOCK_GOTO,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-find-node",
            "Find an org-roam note by title or alias, and jump to it.",
            &clock_ex(),
            CLOCK_PARSE,
            ROAM_FIND_NODE,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-id-create",
            "Give the headline at point an `:ID:`, making it an org-roam node.",
            &clock_ex(),
            CLOCK_PARSE,
            ROAM_ID_CREATE,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-create-node",
            "Create an org-roam note titled by the argument, and open it. \
             Usually reached by picking the create row in `:org-roam-find-node` \
             rather than typed.",
            &lattice::plugin_host::types::ExCommandSpec {
                latency_class: LatencyClass::Reflex,
                accepts_bang: false,
                accepts_range: false,
                args_schema: Vec::new(),
                surface_form: SurfaceForm::Keyword,
            },
            ROAM_CREATE_PARSE,
            ROAM_CREATE_NODE,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-sync",
            "Re-scan `org.roam-directory` and rebuild the roam index. The \
             escape hatch when the watcher missed something.",
            &clock_ex(),
            CLOCK_PARSE,
            ROAM_SYNC,
        );
        register_action(
            "org-archive-subtree",
            "Move the subtree at the cursor into `<this file>_archive`",
            &spec(),
            ARCHIVE_SUBTREE,
        );
        register_action(
            "org-capture-menu",
            "Open the capture menu: one key per template in `org.capture-templates`",
            &spec(),
            CAPTURE_MENU,
        );
        register_action(
            "org-capture",
            "Capture a note through one template (its key is the argument)",
            &spec(),
            CAPTURE,
        );
        register_action(
            "org-capture-fields-submit",
            "File the capture the fields menu collected (fired by its own row)",
            &spec(),
            CAPTURE_FIELDS_SUBMIT,
        );
        register_action(
            "org-capture-submit",
            "File the captured note (dispatched by the prompt on submit)",
            &spec(),
            CAPTURE_SUBMIT,
        );
        register_action(
            "org-refile",
            "Refile the subtree at the cursor under a headline you pick",
            &spec(),
            REFILE,
        );
        register_action(
            "org-refile-to",
            "Refile the subtree at the cursor to an already-chosen target",
            &spec(),
            REFILE_TO,
        );
        register_action(
            "org-toggle-heading",
            "Toggle the line at the cursor between a headline and plain text",
            &spec(),
            TOGGLE_HEADING,
        );
        // OM.7 — `org-todo-mode`'s actions. Registered here, with the rest of
        // the plugin's grammar, because the `grammar` seam is drained once per
        // plugin and both modes resolve their bindings against it.
        register_action(
            "org-todo-cycle",
            "Cycle the TODO keyword on this headline forward",
            &spec(),
            TODO_CYCLE,
        );
        register_action(
            "org-todo-cycle-back",
            "Cycle the TODO keyword on this headline backward",
            &spec(),
            TODO_CYCLE_BACK,
        );
        register_action(
            "org-priority-cycle",
            "Cycle this headline's priority: none, A, B, C, none",
            &spec(),
            PRIORITY_CYCLE,
        );
        register_action(
            "org-set-tags",
            "Set this headline's tags, prompting with the current ones",
            &spec(),
            SET_TAGS,
        );
        register_action(
            "org-set-tags-submit",
            "Apply tags submitted from the org tag prompt (internal)",
            &spec(),
            SET_TAGS_SUBMIT,
        );
    }

    /// Org's manual, compiled into this component and handed over once at
    /// load — the `help` seam's premise: the docs travel with the thing they
    /// document, and unloading the plugin removes them.
    ///
    /// An empty name lands at the bare plugin id, so this is `:help org`
    /// rather than `:help org.org`.
    fn register_help_topics() {
        let _ = register_topic(
            "",
            "Org files: headlines, folding, and what this plugin does not do.",
            include_str!("../doc/org.md"),
            // `:describe-command` cross-links from any command whose name
            // contains these.
            &["fold".to_string()],
        );
    }

    // ── OM.A1 / OM.A2: the agenda seam ──────────────────────────────────
    //
    // The host walks files and builds excerpts; everything org about the
    // agenda lives in `agenda.rs`. See its module docs for what counts as a
    // row and why.

    /// The host offers this plugin `.org` and `.org_archive` files and no
    /// others. It is the same pair `register_languages` claims, and the
    /// duplication is real: an agenda source is not required to have a
    /// language, so it cannot read the answer off one.
    fn extensions() -> Vec<String> {
        vec!["org".to_string(), "org_archive".to_string()]
    }

    /// AF.3: `org.agenda-files`, one path per line.
    ///
    /// Read here rather than cached, because the host calls this per scan for
    /// exactly that reason: the answer is user configuration and has to follow
    /// a `:set` without a reload.
    ///
    /// Empty — unset, or nothing but blanks and comments — means "no opinion",
    /// and the host scans the project root as it did before this option
    /// existed. That is what keeps an org user who has configured nothing on
    /// precisely today's behaviour.
    ///
    /// `~` is NOT expanded here. The host expands, so one implementation serves
    /// every source and a guest cannot get it wrong per-plugin.
    fn roots() -> Vec<String> {
        agenda_files(&option_or("agenda-files", DEFAULT_AGENDA_FILES))
    }

    /// OM.A3: the mode the host activates on the agenda view, so org's TODO
    /// chords work on org's own rows there.
    ///
    /// The view's GENERIC behaviour — `gr`, jump-to-source — is not named
    /// here and is not this plugin's: refreshing the agenda re-runs the
    /// host's walk, which only the host can do. What this claims is the part
    /// that is actually org.
    fn view_mode() -> Option<String> {
        Some("org-agenda-mode".to_string())
    }

    /// Capture the two things that must be the SAME for every file of one
    /// scan, and would drift if they were read per file.
    ///
    /// *Today*, because a scan that crosses midnight must not label half its
    /// rows against one day and half against the next — a row moving from
    /// "today" to "overdue" partway down the view is worse than being a few
    /// hours stale.
    ///
    /// *The keyword set*, because `:set org.todo-keywords` landing mid-scan
    /// would change what counts as done halfway through the project, and an
    /// agenda that hides an entry in one file and shows its twin in another
    /// is not a stale answer, it is an incoherent one.
    fn begin() -> u64 {
        let today = today_epoch_day();
        let keywords = agenda::Keywords::from_spec(
            &get_option("todo-keywords").unwrap_or_else(|| DEFAULT_TODO_KEYWORDS.to_string()),
        );
        // Single-threaded guest, one actor, calls serialised by the host's
        // per-plugin channel — so a `thread_local` IS the whole of the
        // synchronisation story, and `begin`-then-`scan` ordering is a host
        // guarantee rather than something the guest has to defend.
        // OT.3b: the generation key the host caches results under. Derived from
        // everything scan-wide that changes what a row would SAY — the day the
        // scan is anchored to (every label is relative to it: "tomorrow",
        // "overdue by 2 day(s)") and the keyword set (which decides what counts
        // as done, and so which headlines are rows at all).
        //
        // Both are captured immediately above for the same reason they matter
        // here: a scan must be coherent against ONE anchor. Hashing them means
        // the host discards yesterday's cached rows the moment the day rolls,
        // without the host knowing that days or keywords exist.
        let generation = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            today.hash(&mut h);
            keywords.all.hash(&mut h);
            keywords.done.hash(&mut h);
            h.finish()
        };
        SCAN.set(Some(ScanState { today, keywords }));
        generation
    }

    fn scan(
        _path: String,
        text: String,
        tree: Option<&TreeSnapshot>,
    ) -> Result<Vec<Entry>, String> {
        // `begin` is contractually called first. Refusing rather than
        // defaulting makes a host that stops calling it fail loudly on the
        // first file instead of producing a silently mis-dated agenda.
        SCAN.with_borrow(|state| {
            let Some(state) = state.as_ref() else {
                return Err("org: scan before begin".to_string());
            };
            // OT.3: structure from the tree when the host had a grammar for
            // this file, characters from the text either way. The text path is
            // the fallback for a host with no org grammar loaded, and it still
            // carries the "planning line is the next line" assumption — which
            // is the bug the tree path exists to fix, so it is a fallback and
            // not a peer.
            let rows = match tree {
                Some(snapshot) => agenda::scan_tree(&snapshot.root(), &text, &state.keywords),
                None => agenda::scan_file(&text, &state.keywords),
            };
            Ok(rows
                .into_iter()
                .map(|row| Entry {
                    line: row.line,
                    end_line: row.end_line,
                    group: agenda::group_key(row.day),
                    label: agenda::group_label(row.day, state.today),
                    sort_key: agenda::sort_key(&row),
                })
                .collect())
        })
    }
}

/// Per-scan state, captured in `begin`.
struct ScanState {
    today: i64,
    keywords: agenda::Keywords,
}

thread_local! {
    static SCAN: std::cell::RefCell<Option<ScanState>> =
        const { std::cell::RefCell::new(None) };
}

thread_local! {
    /// OC.5b: where the capture in flight was fired from, for `%a`.
    ///
    /// **The origin is gone by the time the note is written.** Opening the
    /// prompt focuses a synthetic prompt buffer, so the `document` handed to
    /// `capture_submit` is the prompt — not the file the user was reading when
    /// they pressed the chord. The fields menu has the same shape. So the
    /// annotation is computed in `capture_open`, while the source buffer is
    /// still the current one, and read back at submit.
    ///
    /// Guest-side rather than smuggled across the seam because the alternatives
    /// are worse: the prompt hop's only spare slot is `buffer-name`, which is
    /// shown to the user and would grow a file path, and the fields hop's is
    /// `args`, already carrying the template key. Both would need an encoding
    /// for something neither the host nor the menu has any use for. This is
    /// state whose lifetime is exactly one capture flow, and it lives with the
    /// flow — the `SCAN` precedent directly above.
    ///
    /// Always written by `capture_open`, including to `None`, so an abandoned
    /// capture cannot leave an annotation for the next one to pick up.
    static CAPTURE_ORIGIN: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Format `%a`: an org link back to the file and line a capture fired from.
///
/// `[[file:PATH::LINE][NAME]]` — the shape org itself writes, so following it
/// works in emacs too and the description is the basename a user recognises
/// rather than a path that wraps.
///
/// `None` for a buffer with no path: a scratch buffer, or the capture menu
/// itself. `%a` then expands to nothing, which is deliberate — a link to
/// nowhere is worse than no link, because it looks followable and is not.
fn origin_annotation(doc: &Document, line: u32) -> Option<String> {
    let path = doc.path()?;
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    // 1-based: `%a` is read by humans and followed by editors, both of which
    // count a file's first line as line 1.
    Some(format!("[[file:{path}::{}][{name}]]", line + 1))
}

/// Today, as days since the Unix epoch, from the host clock.
///
/// `SystemTime` in a wasip2 component resolves through `wasi:clocks`, which
/// the host wires for every seam. A clock that refuses (a jumped-back system
/// time is the realistic case) yields day 0 — 1970 — so every real row reads
/// as overdue. That is deliberately conspicuous: an agenda quietly anchored
/// to the wrong day looks correct and is not.
fn today_epoch_day() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0)
}

/// The shared body of all four promote/demote actions.
///
/// `delta` is -1 to promote, +1 to demote; `whole_subtree` decides whether the
/// rewritten span stops at the headline or runs to the end of its subtree.
///
/// Lines are read through the `document` handle ONE AT A TIME, on demand.
/// Materialising the buffer first would be simpler and would cost one
/// guest→host call per line — 10,000 of them on a 10,000-line file, every time
/// this key is pressed, which is a missed frame rather than a slow key
/// (paramount goal #1). As written, a headline op reads the handful of lines
/// between the caret and its headline, and a subtree op reads its subtree,
/// which it must read anyway to rewrite it.
///
/// Does nothing (rather than erroring) whenever there is nothing to do: the
/// cursor is in a file's preamble with no headline above it, or the shift is
/// refused at level 1.
///
/// `Effect::None` and NOT `Effect::Declined` — see `move_subtree` for the full
/// argument. In short: a declined chord is re-resolved with org's layer removed,
/// and for a multi-key sequence that runs the trailing key alone. These four
/// chords end in `h` / `l` / `H` / `L`, so declining moved the CARET instead of
/// doing nothing — invisible in a text assertion, which is why it survived
/// OM.3. `<leader>oJ`'s trailing `J` joined two lines and was caught at OM.6;
/// this is the same shape with a quieter symptom.
fn shift(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    delta: isize,
    whole_subtree: bool,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let Some((start, _level)) = hl.enclosing(ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let end = if whole_subtree {
        hl.subtree_end(start)
    } else {
        start
    };
    let Some((text, end_len)) = headline::shift_headlines(line, start, end, delta) else {
        return vec![Effect::None];
    };

    // ONE edit over the whole span — see `shift_headlines`. The range runs from
    // column 0 of the root headline to the end of the last line in the span,
    // exclusive of its newline, so the replacement never disturbs the line
    // structure around it.
    vec![Effect::ApplyEdit(
        lattice::plugin_host::types::ApplyEditPayload {
            target: ctx.buffer_id,
            edit: Edit {
                range: Range {
                    start: Position {
                        line: start,
                        byte: 0,
                    },
                    end: Position {
                        line: end,
                        byte: end_len,
                    },
                },
                kind: EditKind::Replace(text),
            },
            // Keep the caret on its own line, and clamp its column: promoting
            // `*** Deep` to `** Deep` shortens the line, and a caret parked past
            // the new end would be a visible jump on a key that only restars.
            cursor: Some(clamped_cursor(ctx, line, start, end, delta)),
        },
    )]
}

/// Where the caret lands after a shift: same line, column moved by the same
/// number of stars the line gained or lost, floored at 0.
///
/// Only headline lines within the span move, so a caret in body text stays
/// exactly where it was.
fn clamped_cursor(
    ctx: &ActionContext,
    line: impl Fn(u32) -> Option<String>,
    start: u32,
    end: u32,
    delta: isize,
) -> Position {
    let at = ctx.cursor.line;
    // One extra line read, and only when the caret is inside the rewritten
    // span — a caret in body text costs nothing.
    let moved = at >= start
        && at <= end
        && line(at)
            .as_deref()
            .and_then(headline::headline_level)
            .is_some();
    let byte = if moved {
        (ctx.cursor.byte as isize + delta).max(0) as u32
    } else {
        ctx.cursor.byte
    };
    Position {
        line: ctx.cursor.line,
        byte,
    }
}

/// One `Replace` over `[from..=to]`, with the caret placed at `cursor`.
///
/// Every OM.6 action rewrites a contiguous run of lines, so they all funnel
/// through here. The range ends at the last line's end and NOT at its newline,
/// which is what keeps a whole-subtree rewrite from eating the blank line after
/// it — the same rule `shift` follows.
/// [`replace_lines`] against a buffer id rather than an action context.
///
/// OC.7 needed it: the clock's bodies are reached from the EX-COMMAND seam as
/// well as from a chord, and the two contexts are different types carrying the
/// same two fields. Taking the id keeps one body serving both.
fn replace_lines_at(
    buffer_id: u32,
    from: u32,
    to: u32,
    to_len: u32,
    text: String,
    cursor: Position,
) -> Vec<Effect> {
    vec![Effect::ApplyEdit(
        lattice::plugin_host::types::ApplyEditPayload {
            target: buffer_id,
            edit: Edit {
                range: Range {
                    start: Position {
                        line: from,
                        byte: 0,
                    },
                    end: Position {
                        line: to,
                        byte: to_len,
                    },
                },
                kind: EditKind::Replace(text),
            },
            cursor: Some(cursor),
        },
    )]
}

fn replace_lines(
    ctx: &ActionContext,
    from: u32,
    to: u32,
    to_len: u32,
    text: String,
    cursor: Position,
) -> Vec<Effect> {
    vec![Effect::ApplyEdit(
        lattice::plugin_host::types::ApplyEditPayload {
            target: ctx.buffer_id,
            edit: Edit {
                range: Range {
                    start: Position {
                        line: from,
                        byte: 0,
                    },
                    end: Position {
                        line: to,
                        byte: to_len,
                    },
                },
                kind: EditKind::Replace(text),
            },
            cursor: Some(cursor),
        },
    )]
}

/// Read `[from..=to]` as owned lines. One boundary crossing per line, which is
/// unavoidable for a span we are about to rewrite wholesale.
fn read_lines(doc: &Document, from: u32, to: u32) -> Option<Vec<String>> {
    (from..=to).map(|i| doc.line(i)).collect()
}

/// Swap the subtree at the cursor with its previous or next SIBLING.
///
/// `<leader>oK` / `<leader>oJ`. Two subtrees trade places as one edit, so `u`
/// puts them back in one step.
///
/// Sibling, not "the adjacent headline" — see `headline::prev_sibling`. Moving
/// a level-2 subtree "up" past a level-3 headline would splice it into another
/// parent's children, which is a silent reparent, not a move.
///
/// ## Why `None` and not `Declined` when there is no sibling
///
/// `Declined` is right for `<Tab>` (OM.5): `<Tab>` has a native meaning worth
/// falling through to, so declining composes. It is WRONG here, and not
/// harmlessly so.
///
/// A declined chord is re-resolved with org's layer removed, and for a
/// multi-key sequence that ends up executing the trailing key on its own. So a
/// declined `<leader>oJ` runs vim's `J` and JOINS TWO LINES — the buffer is
/// modified by a key that was supposed to have found nothing to do. `<leader>o*`
/// would run `*` (search word under cursor), `<leader><CR>` would run `<CR>`.
/// A test caught the join; the others are the same shape.
///
/// The distinction is whether the chord is org's alone. `<Tab>` is shared, so
/// it declines. Everything behind the `<leader>o` prefix is org's, has nothing
/// underneath it, and so CONSUMES the key and does nothing.
fn move_subtree(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    up: bool,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let count = doc.line_count();
    let hl = headline::Headlines::new(tree, &line, count);
    let Some((start, level)) = hl.enclosing(ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let end = hl.subtree_end(start);

    // `first` and `second` are the two spans in DOCUMENT order; the edit
    // rewrites them swapped. Naming them by position rather than by
    // "mine"/"theirs" is what lets one body serve both directions.
    let (first, first_end, second, second_end) = if up {
        let Some(prev) = hl.prev_sibling(start, level) else {
            return vec![Effect::None];
        };
        (prev, start - 1, start, end)
    } else {
        let Some(next) = hl.next_sibling(start, level) else {
            return vec![Effect::None];
        };
        (start, end, next, hl.subtree_end(next))
    };

    let (Some(head), Some(tail)) = (
        read_lines(doc, first, first_end),
        read_lines(doc, second, second_end),
    ) else {
        return vec![Effect::None];
    };
    let last_len = tail.last().map_or(0, |s| s.len()) as u32;
    let mut swapped = tail;
    swapped.extend(head);
    let text = swapped.join("\n");

    // The caret rides its own subtree. Moving up, the cursor line drops by the
    // length of the sibling that was jumped; moving down, it rises by it.
    let jumped = if up {
        start - first
    } else {
        second_end - first_end
    };
    let cursor = Position {
        line: if up {
            ctx.cursor.line - jumped
        } else {
            ctx.cursor.line + jumped
        },
        byte: ctx.cursor.byte,
    };
    replace_lines(ctx, first, second_end, last_len, text, cursor)
}

// ── OC.6: the clock ───────────────────────────────────────────────────────
//
// Two stores, and knowing which is which is the whole design.
//
// The GRAMMAR store (below) edits the buffer. It is synchronous, on the
// keystroke path, and stateless apart from one jump target. The EVENTS store
// (further down) holds the session, arms the minute wake and pushes the
// modeline segment — off the keystroke path by construction (design D6).
//
// They are separate `wasmtime::Store`s of this same component, so they have
// separate linear memories: a `thread_local` here is invisible there. The event
// bus is the only bridge, which is why `emit-event` had to start working from
// the grammar seam before any of this could be built (host slice OC.1).

thread_local! {
    /// The one piece of state the grammar store keeps: where to jump for
    /// `org-clock-goto`.
    ///
    /// It lives here rather than in the events store because `clock-goto` is a
    /// grammar action — it returns an `Effect`, and the events store cannot
    /// (`on-event` returns nothing). Design D6 puts the *session* on the events
    /// seam for a stated reason: the wake and the modeline must be off the
    /// keystroke path. A jump target is neither, so this does not contradict it.
    ///
    /// It does not survive a restart, exactly as the modeline does not (design D4).
    /// The buffer remains the durable record: after a restart `<leader>oj` says it
    /// has nowhere to go, and clocking out on the entry still works because that is
    /// re-derived from the file.
    static CLOCK_GOTO_TARGET: std::cell::RefCell<Option<(String, u32)>> =
        const { std::cell::RefCell::new(None) };
}

/// The current local wall-clock instant.
///
/// `wasi:clocks` is UTC and a plugin's environment carries no `TZ`, so the
/// offset has to come from the host (`local-utc-offset-seconds`, host slice
/// OC.4). Without it every clock line would be wrong by the user's offset and,
/// near midnight, wrong by a day.
fn clock_now() -> clock::Now {
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let offset = i64::from(host_services::local_utc_offset_seconds());
    clock::Now::from_local_secs(utc + offset)
}

/// The headline text, trimmed of its stars and TODO keyword, for the modeline.
fn clock_title(text: &str) -> String {
    let rest = text.trim_start_matches('*').trim();
    let keywords = todo_keywords();
    match rest.split_once(' ') {
        Some((first, tail)) if keywords.iter().any(|k| k == first) => tail.trim().to_string(),
        _ => rest.to_string(),
    }
}

/// `<leader>oi` — start a clock on the entry at the cursor.
/// `cursor` and `buffer_id` rather than a context type, because since OC.10 the
/// action and ex-command contexts both carry exactly these two and the body
/// cares about nothing else. Naming the fields keeps it usable from either
/// surface without a conversion in between.
fn clock_in(
    cursor: Position,
    buffer_id: u32,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let lb = clock::Logbook::new(&hl);

    // Refuse rather than stack a second running clock on the same entry: org
    // treats that as an error, and silently writing one would make the drawer's
    // first line stop being the running clock — which is what makes finding it
    // a single-line look rather than a scan.
    if lb.running(cursor.line).is_some() {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org: this entry already has a running clock".to_string(),
        })];
    }
    let now = clock_now();
    let Some(ins) = lb.clock_in(cursor.line, now) else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org: no entry here to clock into".to_string(),
        })];
    };
    let Some((headline_line, _)) = hl.enclosing(cursor.line) else {
        return vec![Effect::None];
    };
    let title = hl
        .text(headline_line)
        .map(|t| clock_title(&t))
        .unwrap_or_default();

    // Remember where to jump back to, and tell the async side to start ticking.
    if let Some(path) = doc.path() {
        CLOCK_GOTO_TARGET.with(|c| *c.borrow_mut() = Some((path, headline_line)));
    }
    host_services::emit_event(
        EV_CLOCK_STARTED,
        format!("{}\t{title}", now.epoch_minutes()).as_bytes(),
    );

    // A zero-width range at the insertion line's column 0 — an insert, not a
    // replace, so nothing that was there is touched.
    vec![Effect::ApplyEdit(
        lattice::plugin_host::types::ApplyEditPayload {
            target: buffer_id,
            edit: Edit {
                range: Range {
                    start: Position {
                        line: ins.line,
                        byte: 0,
                    },
                    end: Position {
                        line: ins.line,
                        byte: 0,
                    },
                },
                kind: EditKind::Replace(ins.text),
            },
            cursor: Some(cursor),
        },
    )]
}

/// `<leader>oO` (close it) and `<leader>oq` (discard it).
///
/// One body for both because they differ in exactly one way — whether the line
/// is rewritten with an end stamp or removed — and everything around that
/// (locate the entry, find the running clock, refuse when there is none, tell
/// the async side to stop) is identical. Two copies would be two chances for
/// the "tell the async side" half to be forgotten in one of them.
fn clock_stop(
    cursor: Position,
    buffer_id: u32,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    discard: bool,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let lb = clock::Logbook::new(&hl);

    // Re-derived from the buffer, never from the session (design D4) — which is
    // exactly why this works after a restart, when there is no session at all.
    let Some(running) = lb.running(cursor.line) else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org: no running clock on this entry".to_string(),
        })];
    };

    // OC.9: the target is KEPT, not cleared. It stops meaning "where the
    // running clock is" and starts meaning "the last entry clocked" — which is
    // what `:org-clock-resume` needs, and is also what org's own
    // `org-clock-goto` does (it jumps to the current OR last clocked entry).
    // Whether a clock is *running* is answered by the buffer, as it always was.
    host_services::emit_event(EV_CLOCK_STOPPED, &[]);

    if discard {
        let (from, to) = clock::cancel_span(&lb, running);
        // Delete whole lines: the range runs from the first line's column 0 to
        // the START of the line after the last, so the newline goes with them
        // and no blank line is left behind.
        return vec![Effect::ApplyEdit(
            lattice::plugin_host::types::ApplyEditPayload {
                target: buffer_id,
                edit: Edit {
                    range: Range {
                        start: Position {
                            line: from,
                            byte: 0,
                        },
                        end: Position {
                            line: to + 1,
                            byte: 0,
                        },
                    },
                    kind: EditKind::Replace(String::new()),
                },
                cursor: Some(Position {
                    line: from,
                    byte: 0,
                }),
            },
        )];
    }

    let Some(text) = hl.text(running.line) else {
        return vec![Effect::None];
    };
    let Some(closed) = clock::close(&text, clock_now()) else {
        return vec![Effect::None];
    };
    replace_lines_at(
        buffer_id,
        running.line,
        running.line,
        text.len() as u32,
        closed,
        cursor,
    )
}

/// `:org-clock-resume` — start a clock on the last entry that was clocked.
///
/// Reuses `org-clock-goto`'s target, which is the whole reason this is cheap:
/// the grammar store already records `(path, line)` at every clock-in, so
/// "the last clocked entry" needed no new state.
///
/// Two paths, and the second is why this is not limited to the buffer you are
/// in. When the target is the CURRENT buffer it is an ordinary clock-in at that
/// line, so the change shows immediately and unsaved edits are respected. When
/// it is elsewhere the entry is reached the way capture reaches its target —
/// `read-file` for the characters, `parse-file` for the structure, and ONE
/// `WriteToFile`. `clock::Logbook` needs neither a buffer nor a cursor, only a
/// line accessor, so the drawer primitive works over file text unchanged.
fn clock_resume(buffer_id: u32, doc: &Document, tree: Option<&TreeSnapshot>) -> Vec<Effect> {
    let Some((path, line)) = CLOCK_GOTO_TARGET.with(|c| c.borrow().clone()) else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Info,
            text: "org: nothing has been clocked this session".to_string(),
        })];
    };

    // The target is the buffer in front of you: the ordinary path, which also
    // means an unsaved buffer is not written behind its own back.
    if doc.path().as_deref() == Some(path.as_str()) {
        return clock_in(Position { line, byte: 0 }, buffer_id, doc, tree);
    }

    let Ok(on_disk) = host_services::read_file(&path) else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: format!("org: cannot read {path} to resume its clock"),
        })];
    };
    let file_lines: Vec<String> = on_disk.lines().map(str::to_string).collect();
    let count = file_lines.len() as u32;
    let accessor = |n: u32| file_lines.get(n as usize).cloned();
    let snapshot = lattice::plugin_host::tree_sitter::parse_file(&path);
    let hl = headline::Headlines::new(snapshot.as_ref(), &accessor, count);
    let lb = clock::Logbook::new(&hl);

    // The buffer is the record (D4), so "already running" is answered by the
    // file rather than by anything remembered.
    if lb.running(line).is_some() {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: format!("org: that entry in {path} is already clocked in"),
        })];
    }
    let now = clock_now();
    let Some(ins) = lb.clock_in(line, now) else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: format!("org: no entry left at that place in {path}"),
        })];
    };
    let title = hl
        .enclosing(line)
        .and_then(|(h, _)| hl.text(h))
        .map(|t| clock_title(&t))
        .unwrap_or_default();
    host_services::emit_event(
        EV_CLOCK_STARTED,
        format!("{}\t{title}", now.epoch_minutes()).as_bytes(),
    );
    vec![write_at(path, FileAnchor::Line(ins.line), ins.text)]
}

/// `<leader>oj` — jump to the entry the running clock is on.
///
/// Reads the grammar store's own target (see [`CLOCK_GOTO_TARGET`]), so it
/// crosses buffers and reopens a file that was closed. Nothing recorded means
/// nothing is running *in this session*; saying so beats jumping somewhere
/// plausible.
fn clock_goto() -> Vec<Effect> {
    let target = CLOCK_GOTO_TARGET.with(|c| c.borrow().clone());
    match target {
        Some((path, line)) => vec![Effect::OpenBufferAt(
            lattice::plugin_host::types::OpenBufferAtPayload {
                path: Some(path),
                position: Position { line, byte: 0 },
                force: false,
            },
        )],
        None => vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Info,
            text: "org: nothing has been clocked this session".to_string(),
        })],
    }
}

// ── OC.6: the events store — session, wake, modeline ──────────────────────

/// The modeline element this plugin owns. Namespaced by the host to
/// `org.clock`, so it can neither shadow a built-in nor another plugin.
const CLOCK_SEGMENT: &str = "clock";

/// What the events store remembers between wakes. `None` when no clock is
/// running, which is also the state after a restart (design D4 — the modeline
/// is empty, the file is still right).
struct ClockSession {
    /// Minutes since the epoch, local, when the clock started.
    started: i64,
    /// The entry's headline, for the segment.
    title: String,
    /// The armed minute wake, so stopping can cancel it.
    wake: u32,
}

thread_local! {
    static SESSION: std::cell::RefCell<Option<ClockSession>> =
        const { std::cell::RefCell::new(None) };
}

/// The running segment, e.g. `◷ 0:14 Write the clocking slice`.
///
/// `◷` is U+25F7, Geometric Shapes — the BMP fallback that renders in every
/// terminal font. A Nerd Font clock replaces it when `ui.nerd_fonts=on`, at the
/// same cell width so the modeline's geometry does not shift on toggle (the
/// icon-degradation rule).
fn clock_segment_text(session: &ClockSession, now_minutes: i64) -> String {
    let icon = match get_option("ui.nerd_fonts").as_deref() {
        Some("true") | Some("on") => "\u{f017}",
        _ => "\u{25f7}",
    };
    let elapsed = clock::duration(now_minutes - session.started);
    // Trimmed, because the duration is the part that must always be readable —
    // a long headline must not push it off a narrow modeline.
    let title: String = session.title.chars().take(24).collect();
    let ellipsis = if session.title.chars().count() > 24 {
        "…"
    } else {
        ""
    };
    format!("{icon}{} {title}{ellipsis}", elapsed.trim_start())
}

/// Push the segment for the running clock, or clear it when none is running.
fn publish_clock_segment() {
    SESSION.with(|s| match s.borrow().as_ref() {
        Some(session) => {
            let text = clock_segment_text(session, clock_now().epoch_minutes());
            ui::emit_segment(CLOCK_SEGMENT, &text);
        }
        None => ui::clear_segment(CLOCK_SEGMENT),
    });
}

/// Move the subtree at the cursor into `<this file>_archive`.
///
/// `<leader>o$`, org's `C-c C-x C-a`. One `Effect::WriteToFile`, not an insert
/// plus a delete: the host inserts first and cuts ONLY if the insert landed,
/// so a target that cannot be written leaves the subtree where it is. As two
/// effects the failure modes are "the subtree exists twice" and "the subtree
/// is gone", and an effect cannot report failure to the one that follows it
/// (`cross-file-writes.md` §5).
///
/// ## Three refusals
///
/// **No enclosing headline** — the cursor is in the file's preamble. Consumes
/// (`Effect::None`) rather than declining: `<leader>o$` sits behind a
/// plugin-owned prefix with nothing underneath it, and a decline would re-run
/// the trailing `$` on its own, which in vim is "go to end of line" — a
/// cursor jump from a key that meant "archive" (OM.6).
///
/// **No file behind this buffer** — a scratch org buffer has no `_archive` to
/// derive. Echoed rather than silently no-oped, because the user pressed a key
/// that normally moves text and nothing visibly happened.
///
/// **Not granted `fs:write`** — refused at the boundary, before the effect
/// reaches the editor, and the host does the echoing. Nothing to do here; the
/// manifest is where that is answered.
fn archive_subtree(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
) -> Vec<Effect> {
    let Some(source) = doc.path() else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org: this buffer has no file, so it has no archive".to_string(),
        })];
    };
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let Some((text, (sl, sb, el, eb))) = archive::extract_subtree(&hl, ctx.cursor.line) else {
        return vec![Effect::None];
    };

    vec![Effect::WriteToFile(WriteToFilePayload {
        path: archive::archive_path(&source),
        // Append. Org's archive file is a log, and the newest entry belonging
        // at the bottom is what makes it readable top-to-bottom later.
        anchor: lattice::plugin_host::types::FileAnchor::End,
        text,
        cut: Some(Range {
            start: Position { line: sl, byte: sb },
            end: Position { line: el, byte: eb },
        }),
    })]
}

/// The second hop of `<leader>oc`: expand the template around what was typed
/// and append it to `org.capture-file`.
///
/// Reads no buffer at all — capture is the one org verb whose input is the
/// prompt rather than the text you are sitting in, which is exactly why it can
/// be fired from anywhere.
///
/// An unset `org.capture-file` ECHOES rather than guessing. Creating
/// `capture.org` in whichever directory the editor started in would scatter
/// notes somewhere the user never named and would not think to look; being
/// told to set the option once is the better trade.
/// OC.2: which template a capture is using, read fresh from the options.
///
/// Parsed on read rather than cached: `:set org.capture-templates=…` must take
/// effect on the NEXT capture, and a cache would need an `OptionChanged`
/// subscription to stay honest (the `todo-keywords` precedent, OM.7). Capture
/// is an explicit user action, so the parse is nowhere near a typing path.
///
/// `key` is the template to use. `None` means "the user did not choose", which
/// is answerable only when there is exactly one to choose from — OC.3's menu
/// is what supplies the key for a real set.
///
/// Falls back to the single `capture-file` / `capture-template` pair when
/// `capture-templates` is unset, so an existing config keeps working unchanged.
fn selected_template(key: Option<&str>) -> Result<capture_templates::Template, Effect> {
    let source = option_or("capture-templates", DEFAULT_CAPTURE_TEMPLATES);
    if source.trim().is_empty() {
        // The OM.11 path. A capture file that was never set is the one thing
        // capture refuses over: creating `capture.org` in whichever directory
        // the editor started in scatters notes somewhere the user never named.
        let file = option_or("capture-file", DEFAULT_CAPTURE_FILE);
        if file.trim().is_empty() {
            return Err(Effect::Echo(EchoPayload {
                level: EchoLevel::Warn,
                text: "org: set org.capture-templates (or org.capture-file) before capturing"
                    .to_string(),
            }));
        }
        return Ok(capture_templates::Template {
            key: String::new(),
            description: "capture".to_string(),
            target: capture_templates::Target::File { file },
            body: option_or("capture-template", DEFAULT_CAPTURE_TEMPLATE),
            // The bare `org.capture-file` path has no template table to carry a
            // `clock-in` key, so it never clocks.
            clock_in: false,
        });
    }

    let set = capture_templates::parse(&source).map_err(|e| {
        Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: e.message(),
        })
    })?;
    // The skips are NOT surfaced here. They ride back on `ParsedSet` and
    // OC.3's menu echoes them once, at open, which is where a missing row is
    // actually noticeable; echoing on every capture would be noise.
    //
    // A guest `logging::log` was tried and reverted: calling it makes the
    // component IMPORT `logging`, and org's multi-seam linker does not wire
    // that import — the whole component then fails to instantiate. That is the
    // fifth repeat of the TC.6 multi-seam-linker rule and it is a host fix, not
    // something to work around here (noted in the slice plan).

    match key {
        Some(k) => set.by_key(k).cloned().ok_or_else(|| {
            Effect::Echo(EchoPayload {
                level: EchoLevel::Warn,
                text: format!("org: no capture template keyed `{k}`"),
            })
        }),
        // No key and one template: there is nothing to choose. No key and
        // several: say which keys exist. OC.3 replaces this echo with the
        // menu that offers them, and the key argument stays exactly as it is.
        None if set.templates.len() == 1 => Ok(set.templates[0].clone()),
        None => Err(Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: format!(
                "org: pick a template — {}",
                set.templates
                    .iter()
                    .map(|t| format!("{} {}", t.key, t.description))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })),
    }
}

/// The first hop of `<C-x>oc`: resolve the template, then open the prompt.
///
/// Resolving BEFORE the prompt is what makes an unset or broken configuration
/// say so at the keystroke rather than after the user has already typed a note
/// — the one moment capture must not waste.
///
/// The chosen template's key arrives in `ctx.args` — put there by the menu row
/// the user pressed (OC.3) — and rides back out on `buffer_name`, which is what
/// the WIT documents that field for. The host hands it to the submit action
/// alongside the typed text (OC.3a), so the second hop knows which template it
/// is finishing.
///
/// State on the payload rather than in guest memory, deliberately: `<Esc>`
/// dispatches nothing at all, so a guest-side "current template" would never be
/// told to clear and the next capture would inherit it.
///
/// A bare `org-capture` with no key still works when the set holds exactly one
/// template — there is nothing to choose — which is what keeps `:org-capture`
/// and a one-template config usable without going through the menu.
fn capture_open(ctx: &ActionContext, doc: &Document) -> Vec<Effect> {
    // OC.5b: recorded HERE, because this is the last moment the buffer the user
    // fired from is still the current one. Written unconditionally — including
    // the `None` — so an abandoned capture leaves nothing behind for the next.
    let origin = origin_annotation(doc, ctx.cursor.line);
    CAPTURE_ORIGIN.with(|c| *c.borrow_mut() = origin);

    let key = submitted_text(&ctx.args).filter(|k| !k.is_empty());
    let template = match selected_template(key.as_deref()) {
        Ok(t) => t,
        Err(effect) => return vec![effect],
    };

    // OC.4: a template that asks questions gets the FIELDS MENU — one row per
    // `%^{Question}` plus the body — because several named answers before one
    // write is not something a single prompt can express.
    //
    // A template with no questions keeps the direct prompt, and that is a UX
    // decision rather than an omission: the common template is one `%?`, and
    // routing it through a menu would cost three keystrokes (open, pick the
    // body field, fire) to collect the one value the prompt already asks for.
    if !capture_flow::questions(&template.body).is_empty() {
        return vec![Effect::OpenTransient(
            lattice::plugin_host::types::OpenTransientPayload {
                source: CAPTURE_TRANSIENT.to_string(),
                // What the menu is opened FOR (TR.3a). The builder reads this
                // to know which template's questions to offer — the reason the
                // ONE registered source can serve both shapes.
                args: Args::String(template.key.clone()),
            },
        )];
    }

    vec![Effect::OpenPrompt(
        lattice::plugin_host::types::OpenPromptPayload {
            prompt: if template.description.is_empty() {
                "Capture: ".to_string()
            } else {
                format!("Capture ({}): ", template.description)
            },
            initial: String::new(),
            on_submit_action: "org-capture-submit".to_string(),
            buffer_name: (!template.key.is_empty())
                .then(|| format!("*org-capture:{}*", template.key)),
        },
    )]
}

/// The name the host smuggles back with a prompt submit, minus its wrapping.
///
/// `None` for a prompt that carried no state — a legacy single-template capture
/// — which is distinct from an empty key and is why this is not just a trim.
fn capture_key_from_prompt_name(name: &str) -> Option<&str> {
    name.strip_prefix("*org-capture:")?.strip_suffix('*')
}

/// The smuggled state a prompt submit carries: `[typed-text, buffer-name]`
/// (OC.3a). A submit with no state is a plain `Args::String`, so the second
/// slot being absent is the ordinary case rather than an error.
fn prompt_smuggled_state(args: &Args) -> Option<String> {
    let Args::List(items) = args else {
        return None;
    };
    match items.get(1)? {
        lattice::plugin_host::types::ArgValue::String(s) => Some(s.clone()),
        lattice::plugin_host::types::ArgValue::Raw(s) => Some(s.clone()),
        _ => None,
    }
}

/// OC.4: the fields menu's submit — expand the template around the answers the
/// menu collected and write it.
///
/// `ctx.args` is `[key, answer…]`: the fire row's own argument first, then the
/// menu's `Argument` rows in declaration order (TR.3b). The LAST answer is the
/// body — the fields menu appends a body row after the questions, so `%?` is
/// collected the same way everything else is rather than being a special case
/// OC.5b: consume the origin recorded at `capture_open`.
///
/// Consuming rather than peeking: one origin belongs to one capture, and a note
/// filed later carrying the previous capture's `%a` would be a link that looks
/// right and points somewhere the user was not. Empty when the capture came from
/// a buffer with no path.
fn taken_origin() -> String {
    CAPTURE_ORIGIN
        .with(|c| c.borrow_mut().take())
        .unwrap_or_default()
}

/// OC.5a — turn a template's target into the effect that files the note.
///
/// A `file` target appends. A `file+headline` target reads the file and inserts
/// after that headline's whole subtree; a headline that is not there appends
/// **and echoes**, because the note has already been typed and losing it is the
/// one outcome capture must never produce.
///
/// The read is a plain `std::fs::read_to_string` inside the WASI sandbox — an
/// `fs:write` grant preopens the directory with `READ | MUTATE`, so the
/// capability the write already needs is the capability this read needs too. A
/// file that cannot be read (absent, or outside the grant) resolves to an
/// append: absent is the ordinary "first capture into a new file" case, and the
/// host rejects an ungranted path at the boundary anyway with its own message.
/// Wrap a captured entry's text in a running clock, and tell org's async side.
///
/// The drawer is built INTO the text rather than edited in afterwards, because
/// capture files into another file and an `apply-edit` names a buffer id an
/// unopened file does not have. One write, and the record is correct the moment
/// it lands (design D4) — which also means a capture-clock survives immediately,
/// with no window where the entry exists and its clock does not.
///
/// `line` is where the entry starts in the target file, so `org-clock-goto` can
/// come back to it. It is the one thing here that needs a session rather than
/// the buffer.
fn clock_captured_entry(text: String, path: &str, line: u32, title: &str) -> String {
    let now = clock_now();
    CLOCK_GOTO_TARGET.with(|c| *c.borrow_mut() = Some((path.to_string(), line)));
    host_services::emit_event(
        EV_CLOCK_STARTED,
        format!("{}\t{title}", now.epoch_minutes()).as_bytes(),
    );
    // After the headline, which is the entry's first line — the same place
    // `clock.rs` puts it for a fresh drawer.
    let mut lines = text.lines();
    let headline = lines.next().unwrap_or_default();
    let rest: Vec<&str> = lines.collect();
    let drawer = format!(
        ":{}:\n{}{}\n:END:",
        clock::LOGBOOK,
        clock::CLOCK,
        now.stamp()
    );
    let mut out = format!("{headline}\n{drawer}");
    if !rest.is_empty() {
        out.push('\n');
        out.push_str(&rest.join("\n"));
    }
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The headline of a captured entry, for the modeline segment.
fn captured_title(text: &str) -> String {
    clock_title(text.lines().next().unwrap_or_default())
}

fn capture_effects(template: &capture_templates::Template, text: String) -> Vec<Effect> {
    let path = template.target.file().to_string();
    let headline = match &template.target {
        capture_templates::Target::File { .. } => None,
        capture_templates::Target::FileHeadline { headline, .. } => Some(headline.clone()),
    };
    let Some(headline) = headline else {
        // Appended at the end, so the entry starts at the file's current line
        // count. Read only when a clock is actually wanted — an ordinary capture
        // must not pay for it.
        let text = if template.clock_in {
            let at = lattice::plugin_host::host_services::read_file(&path)
                .map(|s| s.lines().count() as u32)
                .unwrap_or(0);
            let title = captured_title(&text);
            clock_captured_entry(text, &path, at, &title)
        } else {
            text
        };
        return vec![write_at(path, FileAnchor::End, text)];
    };

    // OC.5a: the HOST reads it, not the guest.
    //
    // `std::fs::read_to_string` here does not read a file — it panics. A
    // grammar action runs on the host's synchronous linker so the trampoline can
    // call it on the dispatch thread, and `wasmtime-wasi`'s sync filesystem shim
    // blocks on a runtime internally, which is a panic on a thread already
    // inside one. The picker source above CAN use `std::fs` because it runs on
    // the async linker; this path cannot, and the difference is invisible until
    // it takes the plugin down.
    //
    // An `Err` is the ordinary first-capture case (the file does not exist yet)
    // as often as it is a real problem, and both resolve to the same answer
    // here: nothing to search, so append.
    let on_disk = lattice::plugin_host::host_services::read_file(&path).unwrap_or_default();
    // OT.8: structure from `parse-file`, characters from the read above.
    //
    // Both cross the same grant (`parse-file` makes the identical check
    // `read-file` does), so nothing becomes reachable that was not — what
    // changes is that a `* Vocabulary` line written as an example inside a
    // `#+BEGIN_SRC` block stops matching a template's `headline = "Vocabulary"`
    // and filing the note into the middle of a code block.
    //
    // `none` from `parse-file` is the ordinary first-capture case (no file yet)
    // as much as it is a missing grammar, and both mean the same thing here:
    // fall back to the text outline over whatever was read, which for an absent
    // file is empty and appends.
    let lines: Vec<&str> = on_disk.lines().collect();
    let outline = match lattice::plugin_host::tree_sitter::parse_file(&path) {
        Some(snapshot) => {
            let mut out = Vec::new();
            headline::outline(&snapshot.root(), &mut out);
            out
        }
        None => headline::outline_text(&lines),
    };
    // The file is already read above for the target search, so the line the
    // entry lands on is known without a second read on either arm.
    let clocked = |text: String, at: u32| {
        if template.clock_in {
            let title = captured_title(&text);
            clock_captured_entry(text, &path, at, &title)
        } else {
            text
        }
    };
    match capture_target::resolve_in(&lines, &outline, &headline) {
        capture_target::Insertion::AtLine(line) => {
            let text = clocked(text, line);
            vec![write_at(path, FileAnchor::Line(line), text)]
        }
        // Warn, not Info: the note did not go where the user's config says it
        // should, and a silent append is how someone loses track of where their
        // captures are landing.
        capture_target::Insertion::Append => vec![
            write_at(
                path.clone(),
                FileAnchor::End,
                clocked(text, lines.len() as u32),
            ),
            Effect::Echo(EchoPayload {
                level: EchoLevel::Warn,
                text: format!("org: no headline `{headline}` in {path}; appended at the end"),
            }),
        ],
    }
}

/// One capture write. Nothing is being MOVED, so nothing is cut — capture is
/// the one of org's three cross-file writes that only adds.
fn write_at(path: String, anchor: FileAnchor, text: String) -> Effect {
    Effect::WriteToFile(WriteToFilePayload {
        path,
        anchor,
        text,
        cut: None,
    })
}

/// the user reaches by another route.
fn capture_fields_submit(ctx: &ActionContext) -> Vec<Effect> {
    let Args::List(values) = &ctx.args else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org: the capture menu collected nothing".to_string(),
        })];
    };
    let mut collected = values.iter().map(|v| match v {
        lattice::plugin_host::types::ArgValue::String(s) => s.clone(),
        lattice::plugin_host::types::ArgValue::Raw(s) => s.clone(),
        lattice::plugin_host::types::ArgValue::Char(c) => c.to_string(),
        other => format!("{other:?}"),
    });
    let Some(key) = collected.next() else {
        return vec![Effect::None];
    };
    // Re-resolved rather than carried, like the prompt hop: a `:set` between
    // opening the menu and firing it takes effect, which is what the option
    // promises.
    let template = match selected_template(Some(&key)) {
        Ok(t) => t,
        Err(effect) => return vec![effect],
    };
    let mut answers: Vec<String> = collected.collect();
    // The body is the last row. A menu that somehow collected nothing still
    // writes the template — losing it would be worse than writing it bare.
    let entered = answers.pop().unwrap_or_default();

    let text = capture::expand_with(
        &template.body,
        &entered,
        &answers,
        today_epoch_day(),
        &taken_origin(),
    );
    capture_effects(&template, text)
}

fn capture_submit(ctx: &ActionContext) -> Vec<Effect> {
    let Some(entered) = submitted_text(&ctx.args) else {
        return vec![Effect::None];
    };
    // The key the first hop chose, carried out on `buffer-name` and handed
    // back by the host (OC.3a). Re-RESOLVED rather than carried whole, so a
    // `:set org.capture-templates=…` between the two hops takes effect — which
    // is the behaviour the option promises.
    let key = prompt_smuggled_state(&ctx.args);
    let key = key.as_deref().and_then(capture_key_from_prompt_name);
    let template = match selected_template(key) {
        Ok(t) => t,
        Err(effect) => return vec![effect],
    };
    let text = capture::expand(&template.body, &entered, today_epoch_day(), &taken_origin());
    capture_effects(&template, text)
}

/// The second hop of `<leader>or`: file the subtree at the cursor into the
/// target the picker chose.
///
/// The target arrives in `ctx.args` as the token `refile::encode` produced,
/// having crossed the boundary twice — out with the candidate, back with the
/// accept — so it is decoded as untrusted input rather than trusted because
/// this plugin emitted it. A malformed one refuses; refiling a subtree
/// somewhere nobody chose is worse than refiling it nowhere.
///
/// Same single `write-to-file` as archive, and for the same reason: the cut
/// runs only if the insert landed.
///
/// **The caret does not follow the subtree.** Org's refile leaves you where
/// you were, and the target file may not even be open. Jumping would turn a
/// filing action into a navigation one.
fn refile_to(ctx: &ActionContext, doc: &Document, tree: Option<&TreeSnapshot>) -> Vec<Effect> {
    let Some(token) = submitted_text(&ctx.args) else {
        return vec![Effect::None];
    };
    let Some((path, before_line)) = refile::decode(&token) else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org: refile target could not be read".to_string(),
        })];
    };
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let Some((text, (sl, sb, el, eb))) = archive::extract_subtree(&hl, ctx.cursor.line) else {
        return vec![Effect::None];
    };

    vec![Effect::WriteToFile(WriteToFilePayload {
        path,
        anchor: match before_line {
            Some(line) => FileAnchor::Line(line),
            None => FileAnchor::End,
        },
        text,
        cut: Some(Range {
            start: Position { line: sl, byte: sb },
            end: Position { line: el, byte: eb },
        }),
    })]
}

/// Insert a new sibling headline after the subtree at the cursor, and put the
/// caret on it ready to type.
///
/// `<leader><CR>`, org's `M-RET`.
///
/// **After the subtree, not after the headline line.** Both readings exist in
/// the wild (emacs's `org-insert-heading` vs `org-insert-heading-respect-content`
/// on `C-RET`), and the difference only shows on a headline that HAS children:
/// inserting immediately below the headline line puts the new sibling in front
/// of its own children, which reparents every one of them under it. That is a
/// silent restructure from a key that means "new heading", and this plugin
/// already refuses that class of surprise — `restar` declines a level-0 promote
/// for the same reason. Respect-content is the non-destructive reading.
///
/// Declines in a file's preamble: with no enclosing headline there is no level
/// to inherit, and guessing level 1 would make `<leader><CR>` mean something
/// different depending on where the cursor happens to be.
fn meta_return(ctx: &ActionContext, doc: &Document, tree: Option<&TreeSnapshot>) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let Some((start, level)) = hl.enclosing(ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let end = hl.subtree_end(start);
    let Some(last) = doc.line(end) else {
        return vec![Effect::None];
    };
    let stars = "*".repeat(level);
    // A zero-width range at the end of the subtree's last line: the newline is
    // part of the INSERTED text, so the line above keeps its own.
    let cursor = Position {
        line: end + 1,
        byte: stars.len() as u32 + 1,
    };
    replace_lines(
        ctx,
        end,
        end,
        last.len() as u32,
        format!("{last}\n{stars} "),
        cursor,
    )
}

/// Turn the line at the cursor into a headline, or a headline back into text.
///
/// `<leader>o*`, org's `org-toggle-heading`. The new headline takes the level of
/// the headline it lands under, so a body line under `** Two` becomes a `**`
/// sibling rather than a top-level heading — promoting a note out of its section
/// is not what the key means. With no enclosing headline it becomes level 1.
///
/// Declines on a blank line: there is nothing to promote and no stars to strip.
fn toggle_heading(ctx: &ActionContext, doc: &Document, tree: Option<&TreeSnapshot>) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let at = ctx.cursor.line;
    let Some(text) = doc.line(at) else {
        return vec![Effect::None];
    };
    let level = hl.enclosing(at).map_or(1, |(_, lvl)| lvl);
    let Some(new) = headline::toggle_heading(&text, level) else {
        return vec![Effect::None];
    };
    // Clamp: stripping stars shortens the line, and a caret parked past the new
    // end would jump visibly on a key that only re-marks one line.
    let cursor = Position {
        line: at,
        byte: ctx.cursor.byte.min(new.len() as u32),
    };
    replace_lines(ctx, at, at, text.len() as u32, new, cursor)
}

/// Rewrite the enclosing headline's line through `f`.
///
/// Every `org-todo-mode` action is "find the headline I am under, change one
/// field, put the line back", so they share this. Note it operates on the
/// ENCLOSING headline, not the cursor's line: `<leader>ot` from inside a
/// subtree's body marks that subtree's headline, which is what org does and
/// what makes the key usable without navigating first.
fn rewrite_headline(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    f: impl FnOnce(&str, &[String]) -> Option<String>,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let Some((start, _)) = hl.enclosing(ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let (Some(text), keywords) = (doc.line(start), todo_keywords()) else {
        return vec![Effect::None];
    };
    let Some(new) = f(&text, &keywords) else {
        return vec![Effect::None];
    };
    if new == text {
        // No change: skip the edit rather than push a no-op onto the undo
        // stack. `u` after a key that did nothing must not "undo" it.
        return vec![Effect::None];
    }
    // The caret keeps its line. Its column is clamped because the headline can
    // shorten (dropping `TODO ` moves everything left by five).
    let cursor = if ctx.cursor.line == start {
        Position {
            line: start,
            byte: ctx.cursor.byte.min(new.len() as u32),
        }
    } else {
        ctx.cursor
    };
    replace_lines(ctx, start, start, text.len() as u32, new, cursor)
}

/// `<leader>o:` — prompt for tags, pre-filled with the current ones.
///
/// Two hops, because collecting text from the user is asynchronous: this
/// returns `OpenPrompt` naming `org-set-tags-submit`, the host runs the
/// minibuffer, and on submit dispatches that action with the typed string in
/// `ctx.args`. Escape dispatches nothing, so dismissing leaves the line alone.
fn set_tags_prompt(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let Some((start, _)) = hl.enclosing(ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let keywords = todo_keywords();
    let initial = doc
        .line(start)
        .and_then(|t| todo::parse(&t, &keywords).map(|h| todo::tags_string(&h)))
        .unwrap_or_default();
    vec![Effect::OpenPrompt(
        lattice::plugin_host::types::OpenPromptPayload {
            prompt: "Tags: ".to_string(),
            initial,
            on_submit_action: "org-set-tags-submit".to_string(),
            buffer_name: None,
        },
    )]
}

/// The submitted text, whichever `args` shape the host used to carry it.
fn submitted_text(args: &Args) -> Option<String> {
    match args {
        Args::String(s) => Some(s.clone()),
        Args::Char(c) => Some(c.to_string()),
        Args::List(items) => items.first().map(|v| match v {
            lattice::plugin_host::types::ArgValue::String(s) => s.clone(),
            lattice::plugin_host::types::ArgValue::Raw(s) => s.clone(),
            lattice::plugin_host::types::ArgValue::Char(c) => c.to_string(),
            other => format!("{other:?}"),
        }),
        // `none` is a submitted EMPTY prompt, which means "clear the tags" —
        // distinct from Escape, which dispatches nothing at all and so never
        // reaches this function.
        Args::None => Some(String::new()),
        Args::Bytes(_) => None,
    }
}

impl GrammarCallbacks for Component {
    fn apply_action(
        callback: u32,
        ctx: ActionContext,
        doc: &Document,
        // OT.4: the headline actions locate through the tree now — see
        // `headline::Headlines`. Everything that asks "which subtree am I in"
        // takes it; the ones that only rewrite a line the caller already
        // located (`restar`, `toggle_heading`'s rewrite half) do not.
        tree: Option<&TreeSnapshot>,
    ) -> Result<Vec<Effect>, String> {
        match callback {
            PROMOTE_HEADLINE => Ok(shift(&ctx, doc, tree, -1, false)),
            DEMOTE_HEADLINE => Ok(shift(&ctx, doc, tree, 1, false)),
            PROMOTE_SUBTREE => Ok(shift(&ctx, doc, tree, -1, true)),
            DEMOTE_SUBTREE => Ok(shift(&ctx, doc, tree, 1, true)),
            // OM.5: `<Tab>` is org's most overloaded key, and `Declined` is
            // what makes it composable rather than a host special case. On a
            // headline it cycles; anywhere else it DECLINES, and the
            // dispatcher re-resolves as if org-mode's layer were not there —
            // falling through to whatever `<Tab>` natively means (jump-list
            // forward). When `org-table-mode` arrives it binds `<Tab>` above
            // this one and declines outside a table, making the chain two
            // hops with no change here.
            CYCLE => {
                let line = |n: u32| doc.line(n);
                // OT.4: "is a section headline here", not "does this line start
                // with stars" — so `<Tab>` inside a source block declines to
                // the native meaning instead of cycling a fold that is not one.
                let hl = headline::Headlines::new(tree, &line, doc.line_count());
                if hl.is_headline(ctx.cursor.line) {
                    Ok(vec![Effect::AppAction(AppEffect::CycleFoldAtCursor)])
                } else {
                    Ok(vec![Effect::Declined])
                }
            }
            // `<S-Tab>` is whole-buffer, so it does not decline: org's global
            // cycle is meaningful wherever the cursor is.
            CYCLE_GLOBAL => Ok(vec![Effect::AppAction(AppEffect::CycleFoldsGlobal)]),
            // OM.6.
            MOVE_SUBTREE_UP => Ok(move_subtree(&ctx, doc, tree, true)),
            MOVE_SUBTREE_DOWN => Ok(move_subtree(&ctx, doc, tree, false)),
            META_RETURN => Ok(meta_return(&ctx, doc, tree)),
            TOGGLE_HEADING => Ok(toggle_heading(&ctx, doc, tree)),
            ARCHIVE_SUBTREE => Ok(archive_subtree(&ctx, doc, tree)),
            REFILE => Ok(vec![Effect::OpenPicker(
                lattice::plugin_host::types::OpenPickerPayload {
                    source: REFILE_PICKER.to_string(),
                    args: Vec::new(),
                },
            )]),
            REFILE_TO => Ok(refile_to(&ctx, doc, tree)),
            // OC.3: names the menu the `transient-source` seam registered.
            // The host resolves the name against the registry and calls this
            // plugin's `build` for the place it was opened from.
            CAPTURE_MENU => Ok(vec![Effect::OpenTransient(
                lattice::plugin_host::types::OpenTransientPayload {
                    source: CAPTURE_TRANSIENT.to_string(),
                    // The template menu is opened for nothing in particular —
                    // it IS the choice. TR.3a's args carry a subject when a
                    // menu drills down, which is the fields menu (OC.4).
                    args: Args::None,
                },
            )]),
            CAPTURE => Ok(capture_open(&ctx, doc)),
            CAPTURE_SUBMIT => Ok(capture_submit(&ctx)),
            CAPTURE_FIELDS_SUBMIT => Ok(capture_fields_submit(&ctx)),
            TOGGLE_CHECKBOX => Ok(toggle_checkbox(&ctx, doc, tree)),
            TABLE_NEXT_CELL => Ok(table_move(&ctx, doc, tree, 1)),
            TABLE_PREV_CELL => Ok(table_move(&ctx, doc, tree, -1)),
            TABLE_ALIGN => Ok(table_move(&ctx, doc, tree, 0)),
            TABLE_ROW_UP..=TABLE_DELETE_COL => Ok(table_structure(&ctx, doc, tree, callback)),
            OPEN_LINK => Ok(open_link(&ctx, doc, Effect::None)),
            FOLLOW_LINK => Ok(open_link(&ctx, doc, Effect::Declined)),
            TIMESTAMP_UP => Ok(step_timestamp(&ctx, doc, 1)),
            TIMESTAMP_DOWN => Ok(step_timestamp(&ctx, doc, -1)),
            // IM.7: flips `org.inline-images`. The option is global rather
            // than per-buffer because the producer is per-plugin, and a
            // per-buffer answer would mean the guest tracking buffer state it
            // has no other reason to hold.
            TOGGLE_INLINE_IMAGES => {
                let on = !inline_images_enabled();
                // `set_option` publishes `OptionChanged`, which is what
                // re-triggers the producer — so the images appear or vanish
                // without the user touching anything else.
                if !lattice::plugin_host::config::set_option(
                    "inline-images",
                    if on { "true" } else { "false" },
                ) {
                    return Err("org: could not set org.inline-images".to_string());
                }
                Ok(vec![Effect::Echo(
                    lattice::plugin_host::types::EchoPayload {
                        level: lattice::plugin_host::types::EchoLevel::Info,
                        text: format!("org: inline images {}", if on { "on" } else { "off" }),
                    },
                )])
            }
            // OM.7.
            TODO_SELECT => Ok(vec![Effect::OpenTransient(
                lattice::plugin_host::types::OpenTransientPayload {
                    source: CAPTURE_TRANSIENT.to_string(),
                    // The seam gives a guest ONE `id()`, so org's menus
                    // share a source and the args say which. OC.4
                    // established the shape for capture's two.
                    args: Args::String(ORG_TRANSIENT_TODO.to_string()),
                },
            )]),
            // The chosen state rides the row's own args (TR.2a).
            TODO_SET => {
                // An empty string is a real choice — the menu's "(none)"
                // row clears the state — so a missing arg and a cleared
                // state are deliberately the same thing here.
                let want = match &ctx.args {
                    Args::String(s) => s.clone(),
                    _ => String::new(),
                };
                Ok(rewrite_headline(&ctx, doc, tree, |line, kw| {
                    todo::set_keyword(line, kw, &want)
                }))
            }
            TODO_CYCLE => Ok(rewrite_headline(&ctx, doc, tree, |line, kw| {
                todo::cycle_keyword(line, kw, true)
            })),
            TODO_CYCLE_BACK => Ok(rewrite_headline(&ctx, doc, tree, |line, kw| {
                todo::cycle_keyword(line, kw, false)
            })),
            PRIORITY_CYCLE => {
                let highest = highest_priority();
                Ok(rewrite_headline(&ctx, doc, tree, move |line, kw| {
                    todo::cycle_priority(line, kw, highest, true)
                }))
            }
            SET_TAGS => Ok(set_tags_prompt(&ctx, doc, tree)),
            SET_TAGS_SUBMIT => {
                let Some(tags) = submitted_text(&ctx.args) else {
                    return Ok(vec![Effect::None]);
                };
                Ok(rewrite_headline(&ctx, doc, tree, move |line, kw| {
                    todo::set_tags(line, kw, &tags)
                }))
            }
            other => Err(format!("org: unknown action callback {other}")),
        }
    }

    // Org contributes no motions, operators or ex-commands yet; the text
    // objects land at OM.4. An `err` here is logged and the contribution
    // no-ops, so an unreachable callback can never wedge the keystroke path.
    /// The headline motions (OM.4).
    ///
    /// A motion must return SOMEWHERE, so "no headline that way" resolves to
    /// the cursor's own line rather than an `err`: an `err` is logged and the
    /// contribution no-ops, which is right for a broken motion and wrong for
    /// `]]` at the last headline. Staying put is what `}` does at the end of a
    /// buffer.
    ///
    /// `count` repeats the step, so `3]]` walks three headlines — applied by
    /// stepping rather than by multiplying, since headlines are not evenly
    /// spaced. A count that runs off the end stops at the last one.
    fn apply_motion(
        callback: u32,
        ctx: MotionContext,
        doc: &Document,
        // OT.4: `]]` / `[[` / `g{` walk the tree's sections — so they step over
        // a `* TODO` line inside a source block rather than landing on it, and
        // `g{` finds the parent as a node's ancestor instead of re-deriving
        // "the nearest shallower headline" from star counts.
        tree: Option<&TreeSnapshot>,
    ) -> Result<MotionResult, String> {
        let line = |n: u32| doc.line(n);
        let hl = headline::Headlines::new(tree, &line, doc.line_count());
        let count = ctx.count.max(1);
        let mut at = ctx.from.line;
        for _ in 0..count {
            let next = match callback {
                NEXT_HEADLINE => hl.next(at),
                PREV_HEADLINE => hl.prev(at),
                PARENT_HEADLINE => hl.parent(at),
                other => return Err(format!("org: unknown motion callback {other}")),
            };
            match next {
                Some(target) => at = target,
                // Ran out part-way through a count: stop where we got to
                // rather than abandoning the whole motion.
                None => break,
            }
        }
        Ok(MotionResult {
            target: Position { line: at, byte: 0 },
            // Charwise: `d]]` should delete to the start of the next headline,
            // not swallow the headline's own line.
            linewise: false,
        })
    }
    fn apply_operator(_c: u32, _ctx: OperatorContext) -> Result<Vec<Effect>, String> {
        Err("org: no operators".into())
    }
    /// The headline / subtree text objects (OM.4b).
    ///
    /// `inner` vs `around` follows vim's own distinction rather than inventing
    /// one: *around* takes the structural marker with it, *inner* leaves it.
    /// So `ah` is the whole headline line and `ih` its title without the stars;
    /// `ar` is a subtree headline-and-all, `ir` its body with the headline left
    /// standing. `dir` empties a section, `dar` removes it.
    ///
    /// An `err` here is logged and the contribution no-ops, leaving the
    /// operator with nothing to act on — which is the right outcome for `dar`
    /// with the cursor in a file's preamble.
    fn apply_text_object(
        callback: u32,
        ctx: TextObjectContext,
        doc: &Document,
        tree: Option<&TreeSnapshot>,
    ) -> Result<Range, String> {
        let line = |n: u32| doc.line(n);
        // OT.4: ask the grammar which section the cursor is in. The text path
        // matches `*` at the start of a line, so a headline written as example
        // inside a `#+BEGIN_SRC` block resolves as real and `dar` deletes a span
        // that is not a subtree. Whether a line is inside a block is not on the
        // line, so no line matcher can fix that — the tree simply knows.
        let hl = headline::Headlines::new(tree, &line, doc.line_count());
        let (start, stars) = hl
            .enclosing(ctx.at.line)
            .ok_or("org: no headline at or above the cursor")?;
        // The headline's own text, needed for the end-of-line byte on several
        // of the objects below. Read once, after the locator has agreed which
        // line it is.
        let head = line(start).ok_or("org: headline vanished mid-read")?;

        let range = match callback {
            // The headline's title: after the stars and their space, to the
            // end of the line.
            INNER_HEADLINE => Range {
                start: Position {
                    line: start,
                    byte: (stars + 1) as u32,
                },
                end: Position {
                    line: start,
                    byte: head.len() as u32,
                },
            },
            // The whole headline line, stars included.
            AROUND_HEADLINE => Range {
                start: Position {
                    line: start,
                    byte: 0,
                },
                end: Position {
                    line: start,
                    byte: head.len() as u32,
                },
            },
            // The subtree's BODY — everything under the headline, the
            // headline itself left standing. `dir` empties a section.
            INNER_SUBTREE => {
                // OT.4: the section node's own extent IS the subtree, so the
                // tree answers directly where the text path reconstructs it by
                // scanning for the next same-or-shallower headline.
                let end = hl.subtree_end(start);
                if end == start {
                    // A childless headline has no inner subtree; an empty
                    // range at its end is the honest answer, and `dir` on it
                    // deletes nothing rather than eating the headline.
                    Range {
                        start: Position {
                            line: start,
                            byte: head.len() as u32,
                        },
                        end: Position {
                            line: start,
                            byte: head.len() as u32,
                        },
                    }
                } else {
                    Range {
                        start: Position {
                            line: start + 1,
                            byte: 0,
                        },
                        end: Position {
                            line: end,
                            byte: line(end).map(|l| l.len()).unwrap_or(0) as u32,
                        },
                    }
                }
            }
            // The whole subtree, headline and all. `dar` removes a section.
            AROUND_SUBTREE => {
                // OT.4: the section node's own extent IS the subtree, so the
                // tree answers directly where the text path reconstructs it by
                // scanning for the next same-or-shallower headline.
                let end = hl.subtree_end(start);
                Range {
                    start: Position {
                        line: start,
                        byte: 0,
                    },
                    end: Position {
                        line: end,
                        byte: line(end).map(|l| l.len()).unwrap_or(0) as u32,
                    },
                }
            }
            other => return Err(format!("org: unknown text-object callback {other}")),
        };
        Ok(range)
    }
    /// AG.1: `:org-agenda [root]`.
    ///
    /// The root rides as a plain string so the PROVIDER owns the parse — the
    /// ex-command must not learn the provider's vocabulary, which is the same
    /// split the host's `:agenda` kept before this moved.
    fn parse_ex_args(callback: u32, rest: String, _bang: bool) -> Result<Args, String> {
        match callback {
            // OC.7: the clock's four take nothing. Anything typed after the
            // command is a mistake worth naming rather than ignoring.
            CLOCK_PARSE => {
                if rest.trim().is_empty() {
                    Ok(Args::None)
                } else {
                    Err("org: this command takes no arguments".to_string())
                }
            }
            // OR.6: the whole rest of the line is the new note's title,
            // verbatim. Not split on whitespace: a title has spaces in it, and
            // the picker's create row passes what the user typed.
            ROAM_CREATE_PARSE => {
                let trimmed = rest.trim();
                if trimmed.is_empty() {
                    Err("org-roam: a new note needs a title".to_string())
                } else {
                    Ok(Args::String(trimmed.to_string()))
                }
            }
            AGENDA_PARSE => {
                let trimmed = rest.trim();
                Ok(if trimmed.is_empty() {
                    Args::None
                } else {
                    Args::String(trimmed.to_string())
                })
            }
            other => Err(format!("org: unknown ex-command parse callback {other}")),
        }
    }

    fn apply_ex_command(
        callback: u32,
        ctx: ExCommandContext,
        doc: &Document,
        tree: Option<&TreeSnapshot>,
    ) -> Result<Vec<Effect>, String> {
        match callback {
            // OC.7: the clock. `ctx` carries the cursor and the buffer id since
            // OC.10, and `doc` / `tree` arrived with them — which is the whole
            // reason these can be ex-commands at all.
            CLOCK_IN => Ok(clock_in(ctx.cursor, ctx.buffer_id, doc, tree)),
            CLOCK_OUT => Ok(clock_stop(ctx.cursor, ctx.buffer_id, doc, tree, false)),
            CLOCK_CANCEL => Ok(clock_stop(ctx.cursor, ctx.buffer_id, doc, tree, true)),
            CLOCK_RESUME => Ok(clock_resume(ctx.buffer_id, doc, tree)),
            CLOCK_GOTO => Ok(clock_goto()),
            // OR.4: ring the doorbell; the event store walks. See
            // `EV_ROAM_SYNC` for why this is not done here.
            // OR.6: open the picker. The host owns the picker; this names it.
            ROAM_FIND_NODE => Ok(vec![Effect::OpenPicker(
                lattice::plugin_host::types::OpenPickerPayload {
                    source: roam_find::FIND_NODE_PICKER.to_string(),
                    args: Vec::new(),
                },
            )]),
            // OR.6: mint an id, write the note, open it.
            //
            // This is the grammar seam, which is the whole reason `new-uuid` is
            // host-side (OR.3): a guest minting through `wasi:random` would work
            // on the picker path and take the plugin down here. And the write is
            // an `Effect`, not a `std::fs::write`, for the same structural
            // reason — the sync linker cannot serve WASI at all.
            // OR.8 — `:org-roam-id-create`. The grammar seam, and the second
            // consumer of OR.3's host-side `new-uuid` for its reason: a guest
            // minting through `wasi:random` would work on the picker path and
            // take the plugin down here, because the sync linker serves no WASI.
            ROAM_ID_CREATE => Ok(id_create(&ctx, doc, tree)),
            ROAM_CREATE_NODE => {
                let title = match &ctx.args {
                    Args::String(t) if !t.trim().is_empty() => t.trim().to_string(),
                    _ => {
                        return Ok(vec![Effect::Echo(EchoPayload {
                            level: EchoLevel::Warn,
                            text: "org-roam: a new note needs a title".to_string(),
                        })]);
                    }
                };
                let Some(dir) = roam_scan::roam_directory() else {
                    return Ok(vec![Effect::Echo(EchoPayload {
                        level: EchoLevel::Warn,
                        text: "org-roam: set `org.roam-directory` first".to_string(),
                    })]);
                };
                let id = match host_services::new_uuid() {
                    Ok(id) => id,
                    // The one place this seam refuses rather than degrades — an
                    // `:ID:` is written into the user's file and outlives the
                    // session, so an empty one is worse than no note.
                    Err(error) => {
                        return Ok(vec![Effect::Echo(EchoPayload {
                            level: EchoLevel::Error,
                            text: format!("org-roam: cannot mint an id: {error}"),
                        })]);
                    }
                };
                let slug = roam_find::slug(&title);
                let stamp = roam_file_stamp();
                let name = if slug.is_empty() {
                    // A title of pure punctuation still gets a file, named by
                    // its timestamp alone — refusing to create it would be the
                    // picker declining a title the user deliberately typed.
                    format!("{stamp}.org")
                } else {
                    format!("{stamp}-{slug}.org")
                };
                let path = format!("{}/{name}", dir.trim_end_matches('/'));
                // ONE effect, and deliberately no trailing echo.
                //
                // `WriteToFile` can fail — an unresolvable path, a denied
                // `fs:write` grant — and it reports that by setting the
                // message. An `Echo` applied after it would overwrite exactly
                // that, so a refused write would look like a successful one and
                // the user would go hunting for a note that was never made. The
                // new buffer IS the feedback when it works.
                //
                // `WriteToFile` RESOLVES the path to a buffer, so the note
                // becomes a live unsaved buffer rather than only a file on
                // disk. That is org-roam-capture's own model — a new note is a
                // draft you finalize — and it is also why an abandoned draft
                // never enters the index: the watcher sees it when it lands on
                // disk, which is when the user saves.
                Ok(vec![Effect::WriteToFile(WriteToFilePayload {
                    path,
                    anchor: lattice::plugin_host::types::FileAnchor::End,
                    text: roam_find::new_node_text(&id, &title),
                    cut: None,
                })])
            }
            ROAM_SYNC => {
                host_services::emit_event(EV_ROAM_SYNC, &[]);
                Ok(vec![Effect::Echo(EchoPayload {
                    level: EchoLevel::Info,
                    text: "org-roam: re-scanning\u{2026}".to_string(),
                })])
            }
            // The agenda view is generic host machinery: it builds the
            // multibuffer, walks the files and asks every registered
            // `agenda-source` for rows. This plugin does not open it — it rings
            // the doorbell by the provider's name and supplies its own rows
            // through the seam, exactly as before. Only the trigger moved.
            AGENDA_APPLY => Ok(vec![Effect::AppAction(AppEffect::OpenProviderView(
                OpenProviderViewPayload {
                    provider: "agenda".to_string(),
                    argument: match &ctx.args {
                        Args::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                        _ => None,
                    },
                },
            ))]),
            other => Err(format!("org: unknown ex-command callback {other}")),
        }
    }
}

export!(Component);

/// IM.7 — org's inline-image producer.
///
/// Scans the buffer for `[[file:…]]` links alone on their line and reports one
/// media block per image. The HOST resolves each path against the buffer's
/// directory, reads the file's intrinsic size, decides how many rows it
/// reserves and draws it — this guest only says *what* and *where*.
impl MediaProducer for Component {
    fn media_blocks(ctx: DecorationContext, text: String) -> Result<Vec<MediaBlock>, String> {
        if !inline_images_enabled() {
            // A typed `err`, not an empty list. Empty would mean "I scanned and
            // found none", which the host caches as the answer; this says "I
            // produced nothing this trigger" and the host keeps what it had.
            return Err("org: inline images disabled".to_string());
        }
        if ctx.line_count == 0 {
            return Err("org: empty buffer".to_string());
        }
        Ok(text
            .lines()
            .enumerate()
            .filter_map(|(i, line)| links::image_link(line, i as u32))
            .map(|link| MediaBlock {
                anchor_line: link.line,
                path: link.path,
                alt: link.description,
                // `Contain` — never upscale. An icon stretched across the pane
                // is worse than the icon.
                fit: MediaFit::Contain,
            })
            .collect())
    }
}

/// OM.11 — refile's target list, as a picker source.
///
/// A second seam for this plugin and a deliberately narrow one: it walks and
/// reads the filesystem and never touches the buffer, which is the exact
/// opposite of the grammar seam's shape. The two never meet — `init` runs off
/// the keystroke path in the plugin's own task, and the only thing that
/// crosses back into the editing path is an opaque token.
///
/// Why a source of its own rather than the native `files` picker: `files`
/// accepts by OPENING the chosen file. Refile needs the choice routed back
/// into org's own action, which is what `picker-accept-outcome::invoke-command`
/// is for.
impl PickerSource for Component {
    fn init(
        source: String,
        ctx: lattice::plugin_host::types::PickerContext,
        args: Vec<String>,
    ) -> Result<Vec<exports::lattice::plugin_host::picker_source::CandidatePair>, String> {
        // OR.6: two sources now share this body. Route first.
        if source == roam_find::FIND_NODE_PICKER {
            return Ok(roam_find::init()?
                .into_iter()
                .map(|(candidate, routing)| {
                    exports::lattice::plugin_host::picker_source::CandidatePair {
                        candidate,
                        routing,
                    }
                })
                .collect());
        }
        // `org-refile-targets`' `:maxlevel`, as a picker argument rather than
        // an option: the picker world has no `config` seam, and an argument is
        // per-invocation anyway — `:picker org-refile 5` when you know the
        // heading is deep.
        let max_level = args
            .first()
            .and_then(|a| a.parse::<usize>().ok())
            .unwrap_or(DEFAULT_REFILE_MAX_LEVEL);

        let files = lattice::plugin_host::host_services::walk(&ctx.workspace_root)?;
        let mut pairs = Vec::new();
        for path in files {
            if !path.ends_with(".org") {
                continue;
            }
            // One unreadable file must not fail the picker — the agenda's rule
            // (`scan` returns a `result` and an `err` skips that file), because
            // it is the same failure class. A granted prefix that WASI refused
            // to preopen lands here too, degraded rather than fatal.
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // OT.8: the outline comes from `parse-file`, so a headline written
            // as an example inside a `#+BEGIN_SRC` block is not offered as a
            // refile destination — and, worse than being offered, is not
            // offered with an insertion line pointing into a code block.
            //
            // The text is still read here. `parse-file` returns structure and
            // no node text, and the labels are headline titles — the same
            // "structure from the tree, characters from the text" split the
            // agenda seam settled with a benchmark (D3).
            //
            // One unparseable file must not fail the picker, so `none` falls
            // back to the text outline rather than skipping the file.
            let lines: Vec<&str> = text.lines().collect();
            let outline = match lattice::plugin_host::tree_sitter::parse_file(&path) {
                Some(snapshot) => {
                    let mut out = Vec::new();
                    headline::outline(&snapshot.root(), &mut out);
                    out
                }
                None => headline::outline_text(&lines),
            };
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            for target in refile::targets_from(&path, &name, &lines, &outline, max_level) {
                pairs.push(
                    exports::lattice::plugin_host::picker_source::CandidatePair {
                        candidate: lattice::plugin_host::types::RawCandidate {
                            insert_text: None,
                            text: target.label.clone(),
                            display: target.label.clone(),
                            source: Some(REFILE_PICKER.to_string()),
                            kind: lattice::plugin_host::types::CandidateKind::Plain,
                            data: lattice::plugin_host::types::CandidateData::Plain,
                            annotations: Vec::new(),
                        },
                        // The routing token IS the invocation. The picker never
                        // interprets it; it hands back whichever one the user
                        // chose, and `accept` below only has to unwrap it.
                        routing: lattice::plugin_host::types::RoutingPayload::InvokeCommand(
                            lattice::plugin_host::types::CommandRef {
                                id: "org-refile-to".to_string(),
                                args: Args::String(refile::encode(&target)),
                            },
                        ),
                    },
                );
            }
        }
        Ok(pairs)
    }

    fn accept(
        source: String,
        _ctx: lattice::plugin_host::types::PickerContext,
        routing: lattice::plugin_host::types::RoutingPayload,
    ) -> Result<lattice::plugin_host::types::PickerAcceptOutcome, String> {
        if source == roam_find::FIND_NODE_PICKER {
            return roam_find::accept(routing);
        }
        match routing {
            lattice::plugin_host::types::RoutingPayload::InvokeCommand(cmd) => {
                Ok(lattice::plugin_host::types::PickerAcceptOutcome::InvokeCommand(cmd))
            }
            // A routing token this source did not emit. Refusing is right:
            // acting on someone else's token would refile a subtree somewhere
            // nobody chose.
            _ => Err("org: refile got a routing token it did not emit".to_string()),
        }
    }
}

/// OM.8 — toggle the checkbox on the cursor's line, and bring every ancestor
/// cookie up to date.
///
/// **One edit spanning the whole affected range**, not one per line. Ticking
/// `milk` under `* Shopping [1/3]` changes two lines, and a single `u` must
/// put both back — a half-undone list showing `[2/3]` above one ticked box is
/// a worse state than either end.
///
/// Consumes the key rather than declining when there is no checkbox:
/// `<C-Space>` is org's here and has nothing to fall through to.
fn toggle_checkbox(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let at = ctx.cursor.line;
    // OT.6: the tree decides what is a list item. A `- [ ] example` line
    // between `#+BEGIN_SRC` and `#+END_SRC` is block text, and indentation
    // cannot say so — the text path both toggles it and counts it into the
    // enclosing cookie.
    let cb = checkbox::Checkboxes::new(tree, &line, doc.line_count());
    let Some(text) = doc.line(at) else {
        return vec![Effect::None];
    };
    let Some(item) = cb.item_at(at) else {
        return vec![Effect::None];
    };
    let Some(flipped) = checkbox::set_state(&text, checkbox::toggled(item.state)) else {
        return vec![Effect::None];
    };

    // Rewrite from the toggled line down to itself, then extend upward for
    // each ancestor whose cookie changes. The span is contiguous because an
    // ancestor is always above its children.
    let mut rewritten: Vec<(u32, String)> = vec![(at, flipped)];
    for parent in cb.ancestors(at) {
        let Some(above) = line(parent.line()) else {
            continue;
        };
        // Structure from the locator, state from the buffer AS IT WILL BE —
        // the toggled line included, or the cookie lags one keypress behind
        // the box it is counting.
        let after = |n: u32| -> Option<String> {
            rewritten
                .iter()
                .find(|(i, _)| *i == n)
                .map(|(_, t)| t.clone())
                .or_else(|| line(n))
        };
        let (done, total) = checkbox::tally_lines(&cb.child_item_lines(parent), after);
        if let Some(updated) = checkbox::update_cookie(&above, done, total) {
            if updated != above {
                rewritten.push((parent.line(), updated));
            }
        }
    }

    let top = rewritten.iter().map(|(i, _)| *i).min().unwrap_or(at);
    let Some(last_text) = line(at) else {
        return vec![Effect::None];
    };
    let body: Vec<String> = (top..=at)
        .map(|i| {
            rewritten
                .iter()
                .find(|(j, _)| *j == i)
                .map(|(_, t)| t.clone())
                .or_else(|| line(i))
                .unwrap_or_default()
        })
        .collect();

    replace_lines(
        ctx,
        top,
        at,
        last_text.len() as u32,
        body.join("\n"),
        ctx.cursor,
    )
}

/// OM.9 — step the timestamp component under the cursor.
///
/// **This one DECLINES**, and it is the only action in the plugin that
/// should. `<C-a>` / `<C-x>` are vim's increment / decrement: a genuinely
/// SHARED chord with a real meaning to fall through to. Off a timestamp the
/// user means the builtin, and `Effect::Declined` re-resolves to it —
/// so `<C-a>` on `count: 41` still gives `42` inside an org buffer.
///
/// Contrast every `<leader>o…` action, which consumes: those are org's alone
/// and have nothing beneath them, and declining would run their trailing key
/// as an unrelated command.
///
/// A single-key chord, so the fall-through resolves cleanly — the multi-key
/// hazard that made `<leader>oJ` run vim's `J` does not arise here.
fn step_timestamp(ctx: &ActionContext, doc: &Document, delta: i64) -> Vec<Effect> {
    let Some(text) = doc.line(ctx.cursor.line) else {
        return vec![Effect::Declined];
    };
    let Some(stamp) = timestamp::stamp_at(&text, ctx.cursor.byte as usize) else {
        return vec![Effect::Declined];
    };
    let part = timestamp::part_at(&text, &stamp, ctx.cursor.byte as usize);
    let updated = timestamp::step(&text, &stamp, part, delta);
    if updated == text {
        return vec![Effect::Declined];
    }
    replace_lines(
        ctx,
        ctx.cursor.line,
        ctx.cursor.line,
        text.len() as u32,
        updated,
        ctx.cursor,
    )
}

/// OM.10 — open the link under the cursor.
///
/// Four destinations: a file opens as a buffer, a URL goes to the system
/// handler, an internal `[[*Headline]]` moves the cursor, and an `id:`
/// reference reports that there is no index to resolve it against.
///
/// `on_miss` is what to answer when the cursor is not inside a link, and
/// it is a parameter because the two chords bound to this need opposite
/// answers — see [`FOLLOW_LINK`].
///
/// The internal case searches THIS buffer only, which is what org means by
/// `*Headline`, and matches the title exactly — a fuzzy match that jumped to
/// the wrong heading would be worse than not jumping. A reference that
/// resolves to nothing echoes rather than moving the cursor somewhere
/// arbitrary.
fn open_link(ctx: &ActionContext, doc: &Document, on_miss: Effect) -> Vec<Effect> {
    let Some(text) = doc.line(ctx.cursor.line) else {
        return vec![on_miss];
    };
    let Some(link) = links::link_at(&text, ctx.cursor.byte as usize) else {
        return vec![on_miss];
    };
    match link.target {
        links::Target::File(path) => vec![Effect::OpenBufferAt(
            lattice::plugin_host::types::OpenBufferAtPayload {
                path: Some(path),
                position: Position { line: 0, byte: 0 },
                force: false,
            },
        )],
        // The host decides what "open" means for a URI — this guest neither
        // spawns a process nor touches the network.
        links::Target::Uri(uri) => vec![Effect::OpenExternalUri(uri)],
        links::Target::Headline(title) => {
            let line = |n: u32| doc.line(n);
            let keywords = todo_keywords();
            match links::find_headline(line, doc.line_count(), &title, &keywords) {
                Some(target) => vec![Effect::CursorMove(Position {
                    line: target,
                    byte: 0,
                })],
                None => vec![Effect::Echo(lattice::plugin_host::types::EchoPayload {
                    level: lattice::plugin_host::types::EchoLevel::Warn,
                    text: format!("org: no headline named \"{title}\""),
                })],
            }
        }
        // OR.8: resolved through the index OL.1 was waiting for.
        links::Target::Id(id) => follow_id(&id),
    }
}

/// OR.8 — `:org-roam-id-create`: give the headline at point an `:ID:`.
///
/// **Already has one ⇒ a no-op with a message, not an error and not a second
/// drawer.** Running this twice is something a user does, and org cannot read a
/// headline carrying two `:PROPERTIES:` blocks — so the check is
/// [`roam_index::id_drawer_insert`]'s job and it answers `None` here.
///
/// The insert is ONE edit at the start of a line, which is what keeps it
/// composable with the existing drawer case: an entry that already has a
/// drawer without an `:ID:` gets the line added inside it rather than a second
/// drawer opened above it.
///
/// The new node does not appear in the index until the file is SAVED and the
/// watcher sees it — the same rule §5.2 sets for a created note, and the reason
/// an abandoned edit never enters the index.
fn id_create(ctx: &ExCommandContext, doc: &Document, tree: Option<&TreeSnapshot>) -> Vec<Effect> {
    let warn = |text: String| {
        vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text,
        })]
    };
    let read = |n: u32| doc.line(n);
    let view = headline::Headlines::new(tree, &read, doc.line_count());
    let Some((line, _)) = view.enclosing(ctx.cursor.line) else {
        return warn("org-roam: not inside a headline — `:ID:` goes on an entry".to_string());
    };
    let id = match host_services::new_uuid() {
        Ok(id) => id,
        // Refuses rather than degrades, for `:org-roam-create-node`'s reason:
        // an `:ID:` is written into the user's file and outlives the session,
        // so an empty one is worse than none.
        Err(error) => {
            return vec![Effect::Echo(EchoPayload {
                level: EchoLevel::Error,
                text: format!("org-roam: cannot mint an id: {error}"),
            })];
        }
    };
    let Some((at, drawer)) = roam_index::id_drawer_insert(&read, line, &id) else {
        return warn("org-roam: this entry already has an `:ID:`".to_string());
    };
    vec![Effect::ApplyEdit(
        lattice::plugin_host::types::ApplyEditPayload {
            target: ctx.buffer_id,
            edit: Edit {
                range: Range {
                    start: Position { line: at, byte: 0 },
                    end: Position { line: at, byte: 0 },
                },
                // `edit-kind` has one arm — an insert is a Replace over an
                // empty range, which is how every other insert here is written.
                kind: EditKind::Replace(drawer),
            },
            // The caret stays on the headline it just identified. Following the
            // drawer down would move the user off the thing they acted on.
            cursor: Some(Position { line, byte: 0 }),
        },
    )]
}

/// OR.8 — follow an `[[id:…]]` link through the roam index.
///
/// One exact-key `get` (`roam_index::node`), never a walk of `nodes` — this is
/// the keystroke path, and the separate `n/<id>` key exists precisely so that
/// `<CR>` costs a lookup rather than a 90 KB decode.
///
/// **Three failures, three different messages**, because they send the reader
/// to three different places: roam is not configured (set the option), the
/// index is empty (run `:org-roam-sync`), or the id is genuinely absent (the
/// link is broken). OL.1's single "no id index" message could not distinguish
/// a broken link from an absent feature, which is the reason it said so
/// explicitly rather than saying "cannot open".
///
/// Jumps to file AND line, so a headline node lands on its headline rather than
/// at the top of a file it shares with other nodes — 19% of the reference
/// corpus is headline nodes, and every one of them would otherwise arrive in
/// the wrong place.
fn follow_id(id: &str) -> Vec<Effect> {
    let warn = |text: String| {
        vec![Effect::Echo(lattice::plugin_host::types::EchoPayload {
            level: lattice::plugin_host::types::EchoLevel::Warn,
            text,
        })]
    };
    if roam_scan::roam_directory().is_none() {
        return warn(format!(
            "org: [[id:{id}]] needs a note directory — set `org.roam-directory`"
        ));
    }
    match roam_index::node(id) {
        Some(node) => vec![Effect::OpenBufferAt(
            lattice::plugin_host::types::OpenBufferAtPayload {
                path: Some(node.file),
                position: Position {
                    line: node.line,
                    byte: 0,
                },
                force: false,
            },
        )],
        None if roam_index::is_empty() => warn(format!(
            "org: the roam index is empty, so [[id:{id}]] cannot be resolved — run `:org-roam-sync`"
        )),
        None => warn(format!("org: no note with id {id}")),
    }
}

/// OM.12 — align the table under the cursor and step `delta` cells.
///
/// `delta == 0` aligns without moving (`<leader>o|`).
///
/// **Declines when the cursor is not in a table**, which is what makes
/// `<Tab>` compose: `org-table-mode` sits above `org-mode`, so a decline here
/// falls to org's headline cycle, and a decline there falls to whatever
/// `<Tab>` natively means. Three outcomes from one key, and none of them a
/// host special case.
///
/// Alignment is whole-table and lands as ONE edit: a column's width is the
/// widest cell in it, so touching one cell can change every row, and a
/// half-aligned table is a worse state than either end.
fn table_move(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    delta: i32,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let count = doc.line_count();
    // OT.7: a `| a | b |` line inside a `#+BEGIN_SRC` block is block content,
    // and `is_table_line` cannot tell. Declining there is what leaves `<Tab>`
    // to fall through to org's headline cycle and then to its native meaning.
    let tables = table::Tables::new(tree, &line, count);
    let Some((first, last)) = tables.bounds(ctx.cursor.line) else {
        return vec![Effect::Declined];
    };

    let rows: Vec<table::Row> = (first..=last)
        .filter_map(|i| line(i).and_then(|t| table::parse_row(&t)))
        .collect();
    if rows.is_empty() {
        return vec![Effect::Declined];
    }
    let aligned = table::align(&rows);

    // Where the caret lands. Stepping past the last cell of a row moves to
    // the next row's first cell, which is what makes `<Tab>` walk a table
    // rather than stalling at its right edge.
    let here = ctx.cursor.line;
    let row_index = (here - first) as usize;
    let cell = line(here)
        .map(|t| table::cell_at(&t, ctx.cursor.byte as usize))
        .unwrap_or(0);
    let (mut target_row, mut target_cell) = (row_index, cell as i32 + delta);
    if delta != 0 {
        let cells_here = match rows.get(row_index) {
            Some(table::Row::Cells(c)) => c.len() as i32,
            _ => 1,
        };
        if target_cell >= cells_here {
            target_row = (row_index + 1).min(rows.len().saturating_sub(1));
            target_cell = 0;
        } else if target_cell < 0 {
            target_row = row_index.saturating_sub(1);
            target_cell = match rows.get(target_row) {
                Some(table::Row::Cells(c)) => c.len() as i32 - 1,
                _ => 0,
            };
        }
    }
    let target_line = first + target_row as u32;
    let byte = aligned
        .get(target_row)
        .map(|t| table::cell_start(t, target_cell.max(0) as usize))
        .unwrap_or(0) as u32;

    let last_len = line(last).map(|t| t.len()).unwrap_or(0) as u32;
    replace_lines(
        ctx,
        first,
        last,
        last_len,
        aligned.join("\n"),
        Position {
            line: target_line,
            byte,
        },
    )
}

/// OM.13 — move, insert and delete table rows and columns.
///
/// Every one of these is whole-table: a structural change re-aligns, because
/// a moved column takes its width with it and leaving the rest ragged would
/// look broken. One edit, so `u` restores the table in a step.
///
/// Declines off a table, so the `<leader>t…` chords stay available to
/// anything else that wants them outside one.
fn table_structure(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    action: u32,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let count = doc.line_count();
    // OT.7, as on `table_move`.
    let tables = table::Tables::new(tree, &line, count);
    let Some((first, last)) = tables.bounds(ctx.cursor.line) else {
        return vec![Effect::Declined];
    };
    let mut rows: Vec<table::Row> = (first..=last)
        .filter_map(|i| line(i).and_then(|t| table::parse_row(&t)))
        .collect();
    if rows.is_empty() {
        return vec![Effect::Declined];
    }

    let row = (ctx.cursor.line - first) as usize;
    let col = line(ctx.cursor.line)
        .map(|t| table::cell_at(&t, ctx.cursor.byte as usize))
        .unwrap_or(0);

    // Where the caret should end up. A move follows its row or column —
    // losing the cursor after moving a row is what makes the key feel broken.
    let (mut new_row, mut new_col) = (row, col);
    let changed = match action {
        TABLE_ROW_UP => {
            let ok = row > 0 && table::swap_rows(&mut rows, row, row - 1);
            if ok {
                new_row = row - 1;
            }
            ok
        }
        TABLE_ROW_DOWN => {
            let ok = table::swap_rows(&mut rows, row, row + 1);
            if ok {
                new_row = row + 1;
            }
            ok
        }
        TABLE_COL_LEFT => {
            let ok = col > 0 && table::swap_columns(&mut rows, col, col - 1);
            if ok {
                new_col = col - 1;
            }
            ok
        }
        TABLE_COL_RIGHT => {
            let ok = table::swap_columns(&mut rows, col, col + 1);
            if ok {
                new_col = col + 1;
            }
            ok
        }
        TABLE_INSERT_ROW => {
            table::insert_row(&mut rows, row);
            new_row = row + 1;
            true
        }
        TABLE_INSERT_COL => {
            table::insert_column(&mut rows, col);
            new_col = col + 1;
            true
        }
        TABLE_DELETE_ROW => {
            let ok = table::delete_row(&mut rows, row);
            if ok {
                new_row = row.min(rows.len().saturating_sub(1));
            }
            ok
        }
        TABLE_DELETE_COL => {
            let ok = table::delete_column(&mut rows, col);
            if ok {
                new_col = col.min(table::column_count(&rows).saturating_sub(1));
            }
            ok
        }
        _ => false,
    };
    if !changed {
        // Refused (the last row, a separator, an edge) — consume rather than
        // decline: the user is in a table and meant a table command.
        return vec![Effect::None];
    }

    let aligned = table::align(&rows);
    let last_len = line(last).map(|t| t.len()).unwrap_or(0) as u32;
    let byte = aligned
        .get(new_row)
        .map(|t| table::cell_start(t, new_col))
        .unwrap_or(0) as u32;
    replace_lines(
        ctx,
        first,
        last,
        last_len,
        aligned.join("\n"),
        Position {
            line: first + new_row as u32,
            byte,
        },
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// OC.3 — the capture menu
// ─────────────────────────────────────────────────────────────────────────────

/// The name `Effect::OpenTransient` addresses this menu by, and what the host
/// registers it under (it comes from `id()` below, so the host never learns it
/// statically).
const CAPTURE_TRANSIENT: &str = "org-capture";

/// The capture menu: one row per template, keyed the way the template says.
///
/// This is what makes a nine-template set usable. Before it, capture could
/// offer exactly one template, because a chord carries no way to say WHICH —
/// the whole reason `plugin-transients.md` exists.
///
/// **Built per open, never cached.** A menu row's set comes from
/// `org.capture-templates`, and `:set` must take effect on the next `<C-x>oc`.
/// The seam calls `build` per open for exactly this reason.
///
/// **Each row carries the template's key in its own args** — the per-row slot
/// TR.2a added. One action, N rows, and the key you pressed is what decides
/// which template runs. Without it org would need one registered command per
/// template, and templates are an option read at capture time, not at load.
/// TK.6 — the TODO state menu, one row per configured keyword.
///
/// **Built per open, never cached**, for the same reason the capture menu is:
/// the rows come from `org.todo-keywords`, and a `:set` must take effect on
/// the next press. Unlike the per-keyword COLOURS — which resolve at load
/// because `register-element` drains once — this side of the option is live.
///
/// A keyword with no `(k)` still appears, reachable by motion. A menu that
/// cannot reach a state the file already contains is worse than a menu with a
/// gap in its shortcuts, and dropping it would make the menu disagree with the
/// buffer.
fn todo_menu() -> Result<lattice::plugin_host::types::TransientSpec, String> {
    use lattice::plugin_host::types::{
        Args as WitArgs, TransientAction, TransientGroup, TransientItem, TransientItemKind,
        TransientSpec,
    };

    let kws = todo::parse_todo_keywords(&option_or("todo-keywords", DEFAULT_TODO_KEYWORDS));
    if kws.all.is_empty() {
        // An `err` echoes with the plugin named and the menu stays shut,
        // which says more than an empty menu does.
        return Err("org: no TODO keywords configured".to_string());
    }

    // Keys the option did not give, filled from the keyword itself so every
    // row is reachable by a key rather than only by motion. Lower-cased
    // initial first, then any unused letter of the word — deterministic, so
    // the same configuration always produces the same menu.
    let mut taken: Vec<char> = kws.all.iter().filter_map(|k| k.key).collect();
    taken.push('q');

    let mut items: Vec<TransientItem> = Vec::with_capacity(kws.all.len() + 2);
    for k in &kws.all {
        let key = k.key.or_else(|| {
            k.name
                .chars()
                .filter(|c| c.is_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .find(|c| !taken.contains(c))
                .inspect(|c| taken.push(*c))
        });
        items.push(TransientItem {
            key: key.map(|c| vec![c.to_string()]).unwrap_or_default(),
            label: k.name.clone(),
            description: if k.done {
                "done".to_string()
            } else {
                String::new()
            },
            kind: TransientItemKind::Action(TransientAction {
                command: "org-todo-set".to_string(),
                args: WitArgs::String(k.name.clone()),
            }),
        });
    }

    // Clearing the state is a state. Emacs' fast-select spells it SPC, and a
    // menu that can set every keyword but not remove one is a one-way door.
    items.push(TransientItem {
        key: vec!["<Space>".to_string()],
        label: "(none)".to_string(),
        description: "clear the state".to_string(),
        kind: TransientItemKind::Action(TransientAction {
            command: "org-todo-set".to_string(),
            args: WitArgs::String(String::new()),
        }),
    });
    items.push(TransientItem {
        key: vec!["q".to_string()],
        label: "quit".to_string(),
        description: String::new(),
        kind: TransientItemKind::Dismiss,
    });

    Ok(TransientSpec {
        title: "TODO state".to_string(),
        groups: vec![TransientGroup {
            label: String::new(),
            items,
        }],
        footer: None,
    })
}

/// OR.7 — the completion seam. One source: org-roam nodes, offered only
/// inside an `[[…]]` link in an org buffer (`roam_complete` decides both,
/// from the `language` + `line-before-cursor` the context carries).
///
/// The host runs `generate` off the keystroke path and feeds the result
/// through its own native matcher / ranker / annotator — the seam is a
/// GENERATOR by design, because `matches` and `annotate` run per candidate on
/// the synchronous keystroke pipeline and crossing them would fire hundreds
/// of boundary calls per keystroke (paramount #1).
impl exports::lattice::plugin_host::completion_source::Guest for Component {
    fn spec() -> lattice::plugin_host::types::CompletionSourceSpec {
        roam_complete::spec()
    }

    fn generate(
        ctx: lattice::plugin_host::types::GenerateContext,
    ) -> Result<Vec<lattice::plugin_host::types::RawCandidate>, String> {
        // `Ok` with an empty list, never `Err`, when the source simply does
        // not apply: an `Err` is logged host-side as a broken source, and
        // "the cursor is not in a link" is the normal case, not a fault.
        Ok(roam_complete::generate(&ctx))
    }
}

impl exports::lattice::plugin_host::transient_source::Guest for Component {
    fn id() -> String {
        CAPTURE_TRANSIENT.to_string()
    }

    fn build(
        ctx: lattice::plugin_host::types::TransientContext,
    ) -> Result<lattice::plugin_host::types::TransientSpec, String> {
        use lattice::plugin_host::types::{
            Args as WitArgs, TransientAction, TransientGroup, TransientItem, TransientItemKind,
            TransientSpec,
        };

        // OC.4: ONE registered source, several shapes — which one is decided
        // by what the open was FOR (TR.3a). Opened for nothing, this is the
        // template chooser; opened for a template key, it is that template's
        // fields; opened for `todo`, it is TK.6's state menu. Separate names
        // would have needed separate `id()`s, and the seam gives a guest one.
        //
        // TK.6's branch comes BEFORE the capture-templates parse below, and
        // that ordering is load-bearing rather than tidy: the TODO menu has
        // nothing to do with capture, and parsing first meant a user with no
        // `org.capture-templates` set got "no capture templates" when they
        // pressed the TODO chord. The test said so in those words.
        if let Args::String(key) = &ctx.args {
            if key == ORG_TRANSIENT_TODO {
                return todo_menu();
            }
        }

        // An `err` echoes with the plugin named and the menu does not open —
        // which is right for every one of these: an unset option, a set whose
        // TOML does not parse, and a set with nothing usable in it are all
        // things the user must fix before a menu means anything. A menu that
        // opens empty says none of that.
        let source = option_or("capture-templates", DEFAULT_CAPTURE_TEMPLATES);
        let set = capture_templates::parse(&source).map_err(|e| e.message())?;

        if let Args::String(key) = &ctx.args {
            if !key.is_empty() {
                return fields_menu(&set, key.as_str());
            }
        }

        let mut items: Vec<TransientItem> = set
            .templates
            .iter()
            .map(|t| TransientItem {
                key: vec![t.key.clone()],
                label: t.description.clone(),
                description: format!("→ {}", t.target.file()),
                kind: TransientItemKind::Action(TransientAction {
                    command: "org-capture".to_string(),
                    args: WitArgs::String(t.key.clone()),
                }),
            })
            .collect();
        // A menu with no way out is a trap. `q` is the transient's own
        // convention and costs nothing.
        items.push(TransientItem {
            key: vec!["q".to_string()],
            label: "quit".to_string(),
            description: String::new(),
            kind: TransientItemKind::Dismiss,
        });

        // The templates the set could not use are named in the FOOTER rather
        // than dropped in silence. This is the one place a missing row is
        // noticeable — the user is looking at the menu and counting — and it
        // is once per open rather than once per capture.
        let footer =
            (!set.skipped.is_empty()).then(|| format!("skipped: {}", set.skipped.join("; ")));

        Ok(TransientSpec {
            title: "Capture".to_string(),
            groups: vec![TransientGroup {
                label: "Templates".to_string(),
                items,
            }],
            footer,
        })
    }
}

/// OC.4: the fields menu for one template — a row per `%^{Question}`, a row
/// for the body, and a row that captures.
///
/// A form rather than a run of prompts, deliberately. The menu stays the
/// surface throughout, so an answer can be re-edited before anything is
/// written — a questionnaire has already moved on by the time you notice the
/// typo. It is also the mechanism the editor already has
/// (`PendingTransientArgument` park/resume), rather than a second one org
/// would have had to invent.
///
/// Field names are POSITIONAL (`q0`, `q1`, …) rather than the question text: a
/// template may legitimately ask the same question twice (`%^{Line}` in a list
/// template plainly means two different lines), and two rows sharing a state
/// key would overwrite each other.
fn fields_menu(
    set: &capture_templates::ParsedSet,
    key: &str,
) -> Result<lattice::plugin_host::types::TransientSpec, String> {
    use lattice::plugin_host::types::{
        Args as WitArgs, TransientAction, TransientArgument, TransientGroup, TransientItem,
        TransientItemKind, TransientSpec,
    };

    let template = set
        .by_key(key)
        .ok_or_else(|| format!("no capture template keyed `{key}`"))?;

    let mut items: Vec<TransientItem> = Vec::new();
    for (i, question) in capture_flow::questions(&template.body).iter().enumerate() {
        items.push(TransientItem {
            // `1`..`9` then letters would run out; the index IS the key for
            // the first nine, which covers every real template.
            key: vec![(i + 1).to_string()],
            label: question.clone(),
            description: String::new(),
            kind: TransientItemKind::Argument(TransientArgument {
                name: format!("q{i}"),
                default: None,
                prompt: question.clone(),
            }),
        });
    }
    // The body last, so `%?` is collected the same way every other field is.
    // Its answer is the final one `capture-fields-submit` pops off.
    items.push(TransientItem {
        key: vec!["b".to_string()],
        label: "body".to_string(),
        description: "what `%?` becomes".to_string(),
        kind: TransientItemKind::Argument(TransientArgument {
            name: "body".to_string(),
            default: None,
            prompt: "Capture".to_string(),
        }),
    });
    items.push(TransientItem {
        key: vec!["c".to_string()],
        label: "capture".to_string(),
        description: format!("→ {}", template.target.file()),
        kind: TransientItemKind::Action(TransientAction {
            command: "org-capture-fields-submit".to_string(),
            // The template this menu is collecting for. It arrives at the
            // action ahead of the answers (TR.3b).
            args: WitArgs::String(template.key.clone()),
        }),
    });
    items.push(TransientItem {
        key: vec!["q".to_string()],
        label: "quit".to_string(),
        description: String::new(),
        kind: TransientItemKind::Dismiss,
    });

    Ok(TransientSpec {
        title: format!("Capture: {}", template.description),
        groups: vec![TransientGroup {
            label: "Fields".to_string(),
            items,
        }],
        footer: Some("c to capture, q to abandon".to_string()),
    })
}

#[cfg(test)]
mod conceal_rule_tests {
    use super::conceal_rules;
    use lattice_syntax::conceal::{compile_rules, conceal_spans};

    /// Compile org's declared rules through the HOST's own compiler.
    ///
    /// `lattice-syntax` is already a dev-dependency, so these tests run
    /// the real evaluator rather than a re-implementation of it. That
    /// matters more than convenience: a hand-rolled union in this crate
    /// could agree with itself forever while disagreeing with the thing
    /// that actually renders the buffer.
    fn compiled() -> Vec<lattice_syntax::conceal::ConcealRule> {
        let declared: Vec<(String, Vec<u32>)> = conceal_rules()
            .into_iter()
            .map(|r| (r.pattern, r.hide))
            .collect();
        let (ok, errs) = compile_rules(&declared);
        assert!(
            errs.is_empty(),
            "org must ship rules the host accepts: {errs:?}"
        );
        assert_eq!(ok.len(), declared.len());
        ok
    }

    /// What the user actually sees, via the host's span union.
    fn rendered(line: &str) -> String {
        let rules = compiled();
        let mut out = String::new();
        let mut at = 0usize;
        for (s, e) in conceal_spans(&rules, line) {
            out.push_str(&line[at..s as usize]);
            at = e as usize;
        }
        out.push_str(&line[at..]);
        out
    }

    /// The registration gate, asserted here rather than discovered as a
    /// `warn` in someone's editor: every rule org ships must compile,
    /// name only groups its pattern has, and hide at least one that is
    /// not group 0.
    #[test]
    fn every_rule_org_ships_is_one_the_host_accepts() {
        let _ = compiled();
        for r in conceal_rules() {
            assert!(!r.hide.is_empty(), "{}", r.pattern);
            assert!(!r.hide.contains(&0), "{}", r.pattern);
        }
    }

    #[test]
    fn a_described_link_collapses_to_its_description() {
        assert_eq!(
            rendered("* See [[id:6F398E54][Project Kickoff]] before Friday."),
            "* See Project Kickoff before Friday."
        );
    }

    /// A link whose only text IS its target has nothing left to show
    /// once the target is hidden, so the target stays. Emacs draws the
    /// same line.
    #[test]
    fn a_bare_link_keeps_its_target() {
        assert_eq!(
            rendered("see [[https://example.com]] ok"),
            "see https://example.com ok"
        );
    }

    #[test]
    fn two_links_on_one_line_both_collapse() {
        assert_eq!(rendered("[[id:A][one]] and [[id:B][two]]"), "one and two");
    }

    #[test]
    fn a_file_link_collapses_like_any_other() {
        assert_eq!(
            rendered("[[file:img/diagram.png][a wiring diagram]]"),
            "a wiring diagram"
        );
    }

    #[test]
    fn a_malformed_link_is_left_entirely_alone() {
        let line = "[[id:6F39][unterminated";
        assert_eq!(rendered(line), line);
    }

    /// The property the retracted "declaration order matters" claim was
    /// actually worried about, asserted as what is true: the two
    /// patterns cannot both match one link. `[^]]+` stops at the first
    /// `]`, so the bare pattern never reaches a described link's closing
    /// `]]`.
    ///
    /// Pinned in the crate that OWNS the patterns, because it is a
    /// property of how they are written — a future edit that made them
    /// overlap would reintroduce exactly the failure the old rule
    /// feared, and nothing else would catch it.
    #[test]
    fn the_two_patterns_are_disjoint_by_construction() {
        let rules = compiled();
        let with_desc = "[[id:6F39][Project Kickoff]]";
        assert!(rules[0].pattern().find(with_desc).is_some());
        assert!(
            rules[1].pattern().find(with_desc).is_none(),
            "the bare pattern must not match a described link"
        );

        let plain = "[[https://example.com]]";
        assert!(rules[1].pattern().find(plain).is_some());
        assert!(rules[0].pattern().find(plain).is_none());
    }

    /// Order-independence, from the side that declares the rules.
    #[test]
    fn declaring_them_the_other_way_round_renders_the_same() {
        let mut declared: Vec<(String, Vec<u32>)> = conceal_rules()
            .into_iter()
            .map(|r| (r.pattern, r.hide))
            .collect();
        declared.reverse();
        let (reversed, _) = compile_rules(&declared);
        for line in [
            "[[id:6F39][Project Kickoff]]",
            "see [[https://example.com]] ok",
            "[[id:A][one]] and [[id:B][two]]",
        ] {
            assert_eq!(
                conceal_spans(&compiled(), line),
                conceal_spans(&reversed, line),
                "{line}"
            );
        }
    }
}

#[cfg(test)]
mod agenda_files_tests {
    use super::agenda_files;

    /// The two shapes one list carries — a directory and a single file — plus
    /// the annotation people add to configuration they will re-read in six
    /// months. This is Dhruva's own emacs config, transcribed: `org-directory`
    /// as a directory, `anniversaries.org` as a file.
    #[test]
    fn one_path_per_line_with_comments_and_blanks_ignored() {
        let raw = "\n\
            # everything I keep\n\
            ~/src/dhruvasagar/org-files\n\
            \n\
            # and the one that lives elsewhere\n\
            ~/src/dhruvasagar/org-files/anniversaries.org\n";
        assert_eq!(
            agenda_files(raw),
            vec![
                "~/src/dhruvasagar/org-files".to_string(),
                "~/src/dhruvasagar/org-files/anniversaries.org".to_string(),
            ]
        );
    }

    /// Unset, or nothing but blanks and comments, is "no opinion" — NOT "scan
    /// nothing". The host falls back to the project root, which is what keeps
    /// a user who has configured nothing on exactly the old behaviour.
    #[test]
    fn an_empty_option_is_no_opinion() {
        assert!(agenda_files("").is_empty());
        assert!(agenda_files("   \n\n  # only a note\n").is_empty());
    }

    /// A path may contain a colon or a comma, which is why the separator is a
    /// newline. Splitting on either would have silently cut these in half.
    #[test]
    fn a_path_may_contain_separators_other_formats_would_have_used() {
        let raw = "/tmp/notes: drafts\n/tmp/a,b/notes.org\n";
        assert_eq!(
            agenda_files(raw),
            vec![
                "/tmp/notes: drafts".to_string(),
                "/tmp/a,b/notes.org".to_string()
            ]
        );
    }
}

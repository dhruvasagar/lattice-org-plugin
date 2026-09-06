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
            include lattice:plugin-host/scanned-excerpt-source-plugin@0.1.0;
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
            // MV.3: the agenda is org's view, so org declares it. NOT
            // `include multibuffer-view-plugin` — that world imports
            // `logging`, and every import a component declares must be
            // satisfiable on EVERY linker it is instantiated against,
            // including the grammar seam's sync one where `logging` is
            // deliberately absent. Same scar as the seams above: OC.2 added
            // one `logging::log` call and the WHOLE plugin stopped
            // instantiating.
            import lattice:plugin-host/multibuffer-view-registry@0.1.0;
            export lattice:plugin-host/multibuffer-view-source@0.1.0;
            export register-multibuffer-views: func();
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
mod agenda_args;
mod agenda_custom_commands;
mod agenda_log;
mod agenda_match;
mod agenda_sections;
mod archive;
mod capture;
mod capture_flow;
mod capture_target;
mod capture_templates;
mod checkbox;
mod clock;
mod clock_scan;
mod config_shape;
mod habit_graph;
mod habit_row;
mod habit_stats;
mod headline;
mod history;
mod links;
mod org_date;
// OA.25: the line below a headline, and the rules for writing one field of it
// without destroying the others.
mod complete;
mod planning;
// OE.1: an entry's `:PROPERTIES:` drawer — where it starts, and the edit that
// writes one key of it without disturbing the others.
mod properties;
mod refile;
mod repeat;
// OR.4: what makes a file's contents into roam nodes — the pure half.
mod roam;
// OR.4: the thin tree half — where the headlines and drawers are.
mod roam_backlinks;
// OR.11a: `${title}` / `${slug}` / `${id}`. Committed at OR.11a and never
// compiled — this `mod` line is the whole of the bug that left every
// template-made note carrying a literal `${title}`.
mod roam_capture;
mod roam_complete;
// OR.10: the journal — a date names exactly one file, and this is the
// arithmetic that turns one into the other.
mod roam_dailies;
mod roam_find;
// OR.11b: what a new roam note starts as.
mod roam_index;
mod roam_scan;
mod roam_templates;
mod roam_tree;
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

/// OA.26 — the two remaining consumers of OA.23's seam.
///
/// `org-agenda-goto` jumps from a row to the headline it came from;
/// `<` restricts the agenda to the file the row under the cursor lives in.
/// Both ask the seam the same question — "which file is this row from" — which
/// is why they land together.
const AGENDA_GOTO: u32 = 78;
/// OR.11b — the second hop of creating a note from a template: the menu row
/// dispatches this with the title and the chosen key.
const ROAM_CREATE_FROM_TEMPLATE: u32 = 80;

/// OR.11b — the THIRD hop, for a template that asks `%^{…}` questions: the
/// fields menu's fire row, carrying the title, key and minted id ahead of the
/// answers.
const ROAM_CAPTURE_FIELDS_SUBMIT: u32 = 82;
const AGENDA_FILTER_FILE: u32 = 79;

/// OA.25 — `<leader>os` / `<leader>od`, and their submit halves.
///
/// Four actions rather than two because collecting a date is asynchronous:
/// the chord returns `OpenPrompt` naming the submit action, the host runs the
/// minibuffer, and dispatches the submit with what was typed. The same
/// two-hop shape `org-set-tags` uses.
///
/// One pair per FIELD rather than one pair carrying a field argument, because
/// `on-submit-action` names an action and nothing else — there is nowhere to
/// carry "which field this prompt was for" across the hop.
const SCHEDULE: u32 = 74;
const SCHEDULE_SUBMIT: u32 = 75;
/// TK.9: the note a `(@)` state change asks for, submitted.
const TODO_NOTE_SUBMIT: u32 = 81;
const DEADLINE: u32 = 76;
const DEADLINE_SUBMIT: u32 = 77;

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

/// OR.10 — the second hop of `<leader>ondD`. The chord runs
/// `:org-roam-dailies-goto-date` with no date, which opens a prompt naming this
/// ACTION; the host runs the minibuffer and dispatches here with what was
/// typed. It is an action rather than an ex-command because `on-submit-action`
/// names one — the same two-hop shape `org-set-tags-submit` uses.
const ROAM_DAILIES_GOTO_DATE_SUBMIT: u32 = 50;

/// The `transient-source` id. One per guest, so org's menus share it and
/// branch on what the open was FOR — the shape OC.4 established for
/// capture's two menus.
const ORG_TRANSIENT_TODO: &str = "todo";

/// OR.11b — roam's template chooser, alongside `todo` and `agenda`. Carries
/// the node's TITLE beside the discriminator, because the menu is opened FOR a
/// node and each row has to hand that title on to the create action.
const ORG_TRANSIENT_ROAM: &str = "roam";

/// OR.11b — the roam FIELDS menu, opened for a node whose template asks
/// `%^{…}` questions. Distinct from [`ORG_TRANSIENT_ROAM`] because the two
/// carry different things: the chooser knows a title, the fields menu knows a
/// title, a chosen key and the id already minted for the note.
const ORG_TRANSIENT_ROAM_FIELDS: &str = "roam-fields";

/// OA.12 — the agenda dispatcher's discriminator, alongside `todo`. Same
/// mechanism: one `transient-source::id()` per guest, so org's menus branch on
/// what the open was FOR rather than registering a source each.
const ORG_TRANSIENT_AGENDA: &str = "agenda";

/// OA.18 — the VIEW dispatch, which is a different menu from the one above and
/// so needs its own discriminator. `agenda` chooses WHICH agenda to open;
/// `agenda-view` changes how the agenda already open is being shown.
const ORG_TRANSIENT_AGENDA_VIEW: &str = "agenda-view";

// TB.2: 21..=31 were `org-table-mode`'s eleven generic table callbacks.
// They are the host's now — `table-mode` in `lattice-mode` owns pipe-table
// editing for markdown and org alike (see that mode's docs for why the host
// is the only owner that can serve both). The ids are left as a GAP rather
// than reused: a callback id is a wire value between this guest's
// registration and the host's dispatch, and shifting the ones above it to
// close a hole would silently re-point every action after it.

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

/// `<leader>oc` (OM.11, moved at OC.1) — capture's two hops: the prompt, and what
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

/// OC.6 — the clock's four chords. `<leader>oxi` / `oxo` / `oxq` / `oxj`, org's own
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
/// OR.9 — `:org-roam-backlinks`, what points at the note you are in.
const ROAM_BACKLINKS: u32 = 52;
/// OR.10 — the journal. Four commands and one date, so four callbacks rather
/// than one taking a keyword: each is separately bindable, separately
/// completable and separately documented, which is what makes `<leader>ondy`
/// possible at all.
const ROAM_DAILIES_TODAY: u32 = 53;
const ROAM_DAILIES_YESTERDAY: u32 = 54;
const ROAM_DAILIES_TOMORROW: u32 = 55;
const ROAM_DAILIES_GOTO_DATE: u32 = 56;
/// `-goto-date` is the only one that takes an argument, and it takes it
/// OPTIONALLY: with a date it goes, without one it prompts. So it cannot share
/// `CLOCK_PARSE` (which refuses arguments) or `ROAM_CREATE_PARSE` (which
/// requires one).
const ROAM_DAILIES_DATE_PARSE: u32 = 57;

/// OC.6 — the events handler ids. The guest picks these; the host hands them
/// back to `on-event`.
const ON_CLOCK_EVENT: u32 = 1;
/// OR.4: the watcher told us files under the roam directory changed.
const ON_ROAM_FILES_CHANGED: u32 = 2;
/// OR.4a: `org.roam-directory` was set or changed.
///
/// Without this, roam never indexes for the user who configures it the
/// DOCUMENTED way. `register-events` runs org's boot walk at plugin-load time;
/// an `init.rs` sets the option from a `plugin-loaded` handler, which fires
/// after. The walk therefore reads an unset directory, indexes nothing, and —
/// with nothing watching the option — never runs again. The symptom is `0/0` in
/// the find-node picker, which is indistinguishable from a corpus with no notes
/// in it.
const ON_ROAM_OPTION_CHANGED: u32 = 3;
/// OA.15b: `org-agenda-log-mode` went on or off on some buffer.
///
/// **This handler is the mode's body.** A native minor supplies one itself —
/// `scan-view-clockreport-mode` registers its provider in `on_activate` and
/// drops it on deactivation, which is what makes the mode the switch rather
/// than a label beside one. A plugin mode is DATA: the host builds it into a
/// `PluginMode` whose `on_activate` is a no-op, so without this the mode could
/// be toggled and change nothing.
///
/// The mode is therefore the single source of truth for whether log rows are
/// on, and the `log=` scan argument is DERIVED from it here — never written by
/// the chord. Two writers is exactly the disagreement the mode-as-switch shape
/// exists to prevent.
const ON_AGENDA_LOG_MODE: u32 = 4;

/// The mode `l` toggles, named once. Spelled in four places — the declaration,
/// the toggle effect, the lifecycle filter and the tests — and three of them
/// fail SILENTLY on a typo: a `ToggleMode` naming nothing echoes an error the
/// user sees but no test does, and a lifecycle filter matching nothing simply
/// never fires.
const AGENDA_LOG_MODE_ID: &str = "org-agenda-log-mode";

/// OR.6: `YYYYMMDDHHMMSS` in LOCAL time, for a new note's filename.
///
/// Local rather than UTC, because the stamp is what the user sees in a
/// directory listing and a note filed under yesterday's date because they live
/// east of Greenwich is the midnight bug in a different costume. The offset
/// comes from `local-utc-offset-seconds` (OC.4) — `wasi:clocks` is UTC and the
/// guest has no `TZ`.
fn roam_file_stamp() -> String {
    let local = local_now_secs();
    let days = epoch_day_from_local_secs(local);
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
/// OR.4b: one batch of a cold scan. The guest publishes this to ITSELF to
/// carry the scan across calls — a whole corpus in one call blows the seam's
/// ~1s epoch deadline, traps, and quarantines the plugin. The payload is the
/// scan's generation, so a chain left over from a previous root stops instead
/// of interleaving with the current one.
const EV_ROAM_SCAN_STEP: &str = "org/roam-scan-step";

/// OC.4 — the fields menu's own submit, distinct from the prompt's.
///
/// Two actions rather than one that guesses: the prompt hop hands its action
/// `[text, buffer-name]` and the fields hop hands its action `[key, answers…]`,
/// and a single action would have to sniff which shape it got. Naming them
/// separately is what makes each one's arguments a fact rather than an
/// inference.
const CAPTURE_FIELDS_SUBMIT: u32 = 38;

/// OC.7b — `C-c C-c` in the capture buffer: file what is written there.
const CAPTURE_FINALIZE: u32 = 58;
/// OC.7b — `C-c C-k`: throw the capture away, writing nothing.
///
/// Emacs's aborted capture creates NOTHING, and that falls out of the buffer
/// model rather than needing to be undone: the file is written on finalize, so
/// an abort has nothing to clean up. It is the property OR.6's
/// `WriteToFile`-on-create does not have.
const CAPTURE_ABORT: u32 = 59;

/// OA.12 — the agenda dispatcher. `AGENDA_MENU` opens it; `AGENDA_COMMAND` is
/// what a row fires, carrying the command's key in its own args.
///
/// Two actions rather than one, for `TODO_SELECT` / `TODO_SET`'s reason: the
/// menu's rows fire the second with a key, and one action that sometimes opens
/// a menu and sometimes opens an agenda would make a row's own args ambiguous.
///
/// `AGENDA_COMMAND` is also NOT `:org-agenda` with the key as its argument,
/// and that is the OA.11a two-slot split showing up one layer higher: the
/// ex-command's argument is a ROOT, so passing `w` there would set the scan
/// root to a directory that does not exist and quietly scan nothing.
const AGENDA_MENU: u32 = 60;
const AGENDA_COMMAND: u32 = 61;

/// OA.20 — span walking. `f` / `b` move the view one span forward or back,
/// `.` returns it to today, and `gD d w m y` set the span itself.
///
/// Emacs' keys (`org-agenda-later` / `org-agenda-earlier` / `org-agenda-goto-today`
/// / `org-agenda-day-view` and friends), because this is muscle memory and the
/// UX-follows-convention rule applies to a surface people arrive at with habits.
///
/// **The span keys are `gD`-prefixed, not `v`-prefixed**, and the first attempt
/// shipped `vd` / `vw` / `vm` / `vy` — which could never fire. `v` is bound at
/// `KeymapLayer::Builtin` to enter Visual mode, and layers merge into ONE trie:
/// a node carrying both a terminal binding and children resolves to the
/// terminal, so `v` entered Visual and the second key was never read. Verified
/// against `KeymapTrie::lookup` directly, not inferred.
///
/// `gD` is where evil-org-agenda puts the same commands
/// (`org-agenda-view-mode-dispatch`), for exactly this reason — evil never
/// shadows `v` — so following it costs no muscle memory and gains the one that
/// org users already have. It is also where this repo's own plan put them
/// (OA.18). `g` is a Builtin PREFIX rather than a terminal, and `gD` is
/// terminal only in `lsp-mode`, which never activates on an agenda.
const AGENDA_LATER: u32 = 62;
const AGENDA_EARLIER: u32 = 63;
const AGENDA_TODAY: u32 = 64;
const AGENDA_SPAN_DAY: u32 = 65;
const AGENDA_SPAN_WEEK: u32 = 66;
const AGENDA_SPAN_MONTH: u32 = 67;
const AGENDA_SPAN_YEAR: u32 = 68;

/// OA.21 — filtering. `/` narrows by tag, `\` adds another term, `|` clears.
/// Emacs' keys again.
///
/// Emacs' `<` (restrict to the file at the cursor) is NOT here, and the reason
/// is a missing seam rather than a decision: the agenda is a multibuffer, so
/// `document.path()` answers for the VIEW, and which source file the excerpt
/// under the cursor came from is host-side knowledge with no guest-facing
/// seam. The `file:` filter term itself is live — it parses, it round-trips
/// and `scan` applies it — so `<` is one seam away rather than one feature
/// away. Registering a chord that silently did nothing was the alternative,
/// and it is the failure class this codebase keeps paying for.
const AGENDA_FILTER_TAG: u32 = 69;
const AGENDA_FILTER_TAG_ADD: u32 = 70;
const AGENDA_FILTER_TAG_SUBMIT: u32 = 71;
const AGENDA_FILTER_CLEAR: u32 = 73;

/// OA.15 — `l`, emacs' `org-agenda-log-mode`. What you DID, beside what you
/// plan to do.
///
/// **An argument, not a mode**, which is the slice's whole shape and worth
/// having in front of whoever reads this next. A log row is an ordinary
/// excerpt over the headline the event happened to — so `<CR>` jumps to it and
/// `<leader>ot` acts on it — which means turning log mode on is "re-open this
/// view asking for more rows", exactly what `f` / `gDw` / `/` already are. The
/// design fragment originally filed log entries as host-side virtual rows
/// beside the clock report; that would have made them display-only, and the
/// use of seeing "you closed Ship it at 14:32" is being able to go there.
///
/// Bare `l`, as emacs binds it. Safe here for OA.20's reason and nowhere else:
/// the agenda is read-only, so `l` is not shadowing a motion anybody can use
/// on it, and this mode activates on agenda views alone.
const AGENDA_LOG_MODE: u32 = 83;

/// OE.2 — `org-set-property`, emacs' `C-c C-x p`.
///
/// Three callbacks for one command, because collecting two values takes two
/// prompts and the host dispatches each submit as its own action.
///
/// **The key rides `buffer-name` between the hops, not a guest-side stash.**
/// `open-prompt-payload` has no argument slot, but the host hands a plugin's
/// submit action `[typed-text, buffer-name]` (OC.3a) — which is exactly what
/// capture already smuggles its template key through. A `thread_local` would
/// be the obvious alternative and is the wrong one for capture's stated
/// reason: `<Esc>` on a prompt dispatches NOTHING, so nothing would ever clear
/// it and the next `org-set-property` would inherit the abandoned key.
const SET_PROPERTY: u32 = 85;
/// The first submit: the name was typed, now ask for the value.
const SET_PROPERTY_KEY: u32 = 86;
/// The second submit: both halves in hand, write the drawer.
const SET_PROPERTY_VALUE: u32 = 87;

/// OA.18 — `gD`, emacs' `org-agenda-view-mode-dispatch`. The view toggles in
/// one menu instead of four chords and two loose letters.
///
/// **`gD` could not be a chord while `gDd` was one, and that is why the four
/// span binds go.** `KeymapTrie::lookup` answers `Bound` the moment the walk
/// reaches a node carrying a binding and never looks at its children, so a
/// `gD` binding would make `gDd` / `gDw` / `gDm` / `gDy` unreachable — and
/// unreachable *silently*, since the trailing `d` would then fall through to
/// the grammar in a read-only view and do nothing anybody could see. Which is
/// exactly the failure this menu exists to stop shipping.
///
/// **The keystrokes do not change.** `gD` then `d` was day view before and is
/// day view now; what is new is that the letters are on screen while you
/// choose, which is what emacs' dispatch does too.
///
/// No time-grid row: that mode is not built, and OA.17 records why. A row for
/// it would be a menu entry that does nothing — the class this codebase keeps
/// paying for.
const AGENDA_VIEW_MENU: u32 = 84;

/// OE.3 — `C-c C-c`, emacs' `org-ctrl-c-ctrl-c`. See [`ctrl_c_ctrl_c`].
const CTRL_C_CTRL_C: u32 = 88;

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
/// No templates by default, for `DEFAULT_CAPTURE_FILE`'s reason: a default
/// would name files the user never chose. An empty LIST rather than an empty
/// string now — the option's value is a list, and an empty one is a legal value
/// of `list<record>`, which is what lets it register before anyone configures
/// it (TC.5).
const DEFAULT_CAPTURE_TEMPLATES: [capture_templates::RawTemplate; 0] = [];

/// AS.1: how many days past today the dated section spans. A string because it
/// reaches the option seam alongside its peers and `option_or` is the one
/// accessor; parsed with a fallback so a typo'd `:set org.agenda-span=soon`
/// gives the default rather than an empty agenda.
///
/// `7` is emacs's `org-agenda-span` default in day units — a week counting
/// today. `0` gives the daily agenda. There is deliberately no "everything"
/// value: the unbounded view is exactly what AS.1 replaced, and it is the one
/// that buries today under a year of someone's recurring reminders.
const DEFAULT_AGENDA_SPAN: &str = "7";
/// OA.15: which log items `l` admits, emacs' `org-agenda-log-mode-items`.
///
/// Emacs' own default, and for its reason: what you finished and what you
/// spent time on are the two questions a log answers, where a state change is
/// the noisiest of the three — a task walked through four keywords logs four
/// lines, and most of them say nothing you did not already know from the
/// closure. `state` is one word away for anyone who wants it.
const DEFAULT_LOG_MODE_ITEMS: &str = "closed,clock";
/// AS.2 / OA.11: `org.agenda-sections` and `org.agenda-custom-commands` are
/// STRUCTURED options (TC.6), so their defaults are empty LISTS rather than
/// empty strings — and an empty list is a legal value of `list<record>`, which
/// is what lets each register before anyone configures it. Both are declared
/// inline at their registration; there is no constant to keep in sync.

const GRAMMAR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/grammar.wasm"));

struct Component;

/// Fallbacks for the two options, used when the config seam is not wired (the
/// manifest may not declare `config`) or the option was somehow not registered.
/// `get_option` returning `none` must degrade to working defaults rather than
/// disabling the keys.
/// TK.2: the compiled default, as the LIST the option now is (TC.7). One
/// sequence line, which is emacs' own default reduced to what org needs to
/// function before anyone configures anything.
const DEFAULT_TODO_KEYWORDS: [&str; 1] = ["TODO | DONE"];
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

/// TC.7: a `list<string>` option's value, or an empty list.
///
/// The three line-format options are declared lists now, so "one per line,
/// blank lines and `#` comments dropped" is not a rule anyone has to implement
/// — a list has elements, and an element that is not there is not an element.
/// What is left is trimming, which a config file will always want.
fn string_list(name: &str) -> Vec<String> {
    match crate::config_shape::read_option::<Vec<String>>(name) {
        Some(Ok(list)) => list
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        // The host validated the write, so an `Err` here means the schema and
        // `from_value` disagree — a bug, not a user's typo. Degrade to empty
        // rather than trapping; every caller already handles "nothing set".
        _ => Vec::new(),
    }
}

/// AF.3: `org-agenda-files`, one path per element.
fn agenda_files() -> Vec<String> {
    string_list("agenda-files")
}

/// TK.2: `org.todo-keywords`, one SEQUENCE LINE per element.
///
/// The lines keep emacs' own grammar — `sequence: TODO(t) NEXT(n) | DONE(d)`,
/// with its fast-select keys and logging specs — because that grammar is org's
/// public spelling rather than an encoding worked around, and an emacs config
/// pasted into `lattice.toml` has to keep meaning what it meant. What TC.7
/// changed is the container: a list of lines instead of one newline-joined
/// string, so `:describe-option` says `list<string>` and a TOML array is what
/// a user writes.
fn todo_keyword_lines() -> Vec<String> {
    let lines = string_list("todo-keywords");
    if lines.is_empty() {
        DEFAULT_TODO_KEYWORDS
            .iter()
            .map(|l| l.to_string())
            .collect()
    } else {
        lines
    }
}

fn todo_keywords() -> Vec<String> {
    let parsed = todo::parse_keywords(&todo_keyword_lines().join("\n"));
    // A user who sets the option to nothing gets the default rather than a
    // dead key: an empty sequence would make `<leader>ot` cycle between one
    // state and itself.
    if parsed.is_empty() {
        todo::parse_keywords(&DEFAULT_TODO_KEYWORDS.join("\n"))
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
        // `[[target][description]]` → `description`, STYLED.
        //
        // OL.1: concealing alone made links less visible rather than more.
        // The brackets and target disappear and what is left is prose, so
        // nothing says `<CR>` under the cursor would go anywhere.
        //
        // The grammar cannot supply this: the pinned tree-sitter-org has no
        // link node — links are undifferentiated `expr` tokens — and a
        // `highlights.scm` rule naming an absent node fails to compile and
        // takes the whole language registration with it. So the rule that
        // already knows where a link is carries the style too, which also
        // means the two cannot disagree about where one is.
        //
        // `text.reference` — "this stands for something else", which is what
        // a description is. A builtin capture name, so a colourscheme styles
        // it without org registering a theme element.
        ConcealRule {
            pattern: r"(\[\[[^]]+\]\[)[^]]+(\]\])".to_string(),
            hide: vec![1, 2],
            slot: Some("text.reference".to_string()),
        },
        // `[[target]]` → `target`, styled as the URL it is.
        ConcealRule {
            pattern: r"(\[\[)([^]]+)(\]\])".to_string(),
            hide: vec![1, 3],
            slot: Some("text.uri".to_string()),
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

        // OA.6: the other two things an agenda row says about itself. Both
        // are real org semantics that the org GRAMMAR does not carry — which
        // is why an agenda painted from the source file's tree-sitter parse
        // showed neither — so they get elements of their own to be styled and
        // overridden through, like the keywords above.
        let _ = register_element(
            "priority",
            "A headline's `[#A]` priority cookie.",
            &ThemeStyleSpec {
                inherit: None,
                fg: Some(ColorRef::Palette("red".to_string())),
                bg: None,
                modifiers: ModifierSet {
                    bold: Some(true),
                    ..unset()
                },
                scale: None,
            },
        );
        let _ = register_element(
            "tag",
            "A headline's trailing `:tag:` list.",
            &ThemeStyleSpec {
                inherit: None,
                fg: Some(ColorRef::Palette("overlay".to_string())),
                bg: None,
                modifiers: unset(),
                scale: None,
            },
        );

        // OA.15: the three words a log row's annotation leads with. Elements
        // rather than baked colours for the clock report's reason — a
        // hardcoded green survives `:colorscheme` and then sits in the
        // previous theme's palette — and separate from each other because
        // "finished", "spent time on" and "moved" are three different facts
        // and a reader scans the log for one of them.
        //
        // A state change's KEYWORDS are not styled here: they resolve through
        // `org.todo.<KEYWORD>`, registered above, so `WAITING` in a log row is
        // the yellow it is in every other view and a user's
        // `todo-keyword-styles` override reaches it with nothing added.
        for (name, doc, colour) in [
            (
                "log.closed",
                "The `Closed` label on an agenda log row.",
                "green",
            ),
            (
                "log.clocked",
                "The `Clocked` label on an agenda log row.",
                "blue",
            ),
            (
                "log.state",
                "The `State` label on an agenda log row.",
                "overlay",
            ),
        ] {
            let _ = register_element(
                name,
                doc,
                &ThemeStyleSpec {
                    inherit: None,
                    fg: Some(ColorRef::Palette(colour.to_string())),
                    bg: None,
                    modifiers: unset(),
                    scale: None,
                },
            );
        }

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

        habit_elements();
    }

    /// HB.4 — the consistency graph's eight elements.
    ///
    /// Eight, not four: each of org's states has a solid and a muted variant,
    /// and the muted axis is what stops three weeks of ordinary kept days
    /// shouting as loudly as a miss. See `habit_graph`'s module doc.
    ///
    /// Colours follow org's faces — blue, green, yellow, red — resolved from
    /// the theme's palette rather than written as hex, so `:colorscheme` moves
    /// them. That is OA.16's lesson, which shipped hardcoded hex and had to be
    /// undone.
    ///
    /// Org fills the whole cell with a BACKGROUND colour. A terminal cell here
    /// carries a glyph the user reads, and a filled background behind it is a
    /// solid block, so the colour goes on the foreground instead. The muted
    /// variant dims rather than naming a second palette slot, which keeps the
    /// pair recognisably one colour under every theme.
    fn habit_elements() {
        use crate::habit_graph::DayState;
        for (state, slot, doc) in [
            (
                DayState::Clear,
                "blue",
                "A graph day before the habit is due again.",
            ),
            (
                DayState::Ready,
                "green",
                "A graph day the habit may be done on.",
            ),
            (DayState::Alert, "yellow", "The habit's deadline day."),
            (
                DayState::Overdue,
                "red",
                "A graph day the habit was missed on.",
            ),
        ] {
            for muted in [false, true] {
                let _ = register_element(
                    state.element(muted),
                    doc,
                    &ThemeStyleSpec {
                        inherit: None,
                        fg: Some(ColorRef::Palette(slot.to_string())),
                        bg: None,
                        modifiers: ModifierSet {
                            dim: Some(muted),
                            ..unset()
                        },
                        scale: None,
                    },
                );
            }
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
        todo_theme::register(&todo::parse_todo_keywords(&todo_keyword_lines().join("\n")));
        // TK.5: the user's own per-keyword styles, applied as overrides so
        // they sit ABOVE the theme. Applied here rather than anywhere else
        // because this is the one export where the `theme` import is live —
        // the seam's store is dropped when this returns.
        let (styles, _problems) = todo::styles_from_declared(
            crate::config_shape::read_option::<todo::DeclaredStyles>("todo-keyword-styles")
                .and_then(Result::ok)
                .unwrap_or_default(),
        );
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
        // OR.9: what points at the note you are in. A picker rather than a
        // multibuffer because backlinks is navigation — you look at what links
        // here and go read it — and a picker is what navigation wants. The
        // read-in-place view is a separate question; see `roam_backlinks`.
        lattice::plugin_host::picker_registry::register_picker_source(&roam_backlinks::spec());
    }

    fn register_multibuffer_views() {
        lattice::plugin_host::multibuffer_view_registry::register_multibuffer_view(
            &lattice::plugin_host::types::MultibufferViewSpec {
                // The name `:org-agenda` already opens, unchanged — the trigger
                // does not move, only who declares what it opens.
                id: "agenda".to_string(),
                doc: "Every dated row an org file has, across your agenda files".to_string(),
                buffer_name: "*agenda*".to_string(),
                view_mode: None,
                // One agenda, re-scanned in place: a second `:org-agenda` must
                // not accumulate views.
                reuse: true,
                input: lattice::plugin_host::types::MultibufferViewInput::Scan,
            },
        );
    }

    fn register_options() {
        // TK.2 / TC.7: a LIST of sequence lines, not one newline-joined string.
        //
        // The lines keep emacs' own grammar — that grammar is org's public
        // spelling rather than an encoding worked around, and an emacs config
        // pasted here has to keep meaning what it meant. Decomposing it into a
        // record per keyword would have typed the parenthetical specs at the
        // cost of the one property this option was ported to have. What TC.7
        // changed is the container: `:describe-option` says `list<string>`, and
        // a TOML array is what a user writes.
        let _ = crate::config_shape::register_option::<Vec<String>>(
            "todo-keywords",
            &DEFAULT_TODO_KEYWORDS
                .iter()
                .map(|l| l.to_string())
                .collect(),
            "TODO keywords, one sequence per element, in emacs' \
             `org-todo-keywords` syntax:\n\n\
             \x20 \"sequence: TODO(t) NEXT(n) | DONE(d)\"\n\
             \x20 \"type: PROJECT TO-READ READING(!/!)\"\n\n\
             `|` separates not-done from done. `(t)` is a fast-select key. \
             `(@)` / `(!)` / `(@/!)` are logging specs \u{2014} parsed, not yet \
             acted on. A bare list with no `sequence:` / `type:` prefix is a \
             sequence, so the old flat spelling still means what it meant. \
             Cycling follows this option live; the per-keyword COLOURS resolve \
             at load, so a change needs a reload to recolour (emacs is the \
             same).",
        );
        // TK.5 / TC.7: per-keyword colours, a declared record list.
        //
        // The fit is exact rather than convenient: the modifiers were already
        // tri-state (`Some(true)` sets, `Some(false)` clears an inherited one,
        // `None` leaves it alone), which is precisely what an optional boolean
        // field in a schema means. `no-bold` — the spelling that existed only
        // because a line format has no way to write "false" — becomes
        // `bold = false`, which is what it always meant.
        let _ = crate::config_shape::register_option::<todo::DeclaredStyles>(
            "todo-keyword-styles",
            &Vec::new(),
            "Per-keyword colours \u{2014} the org-shaped spelling of \
             `org-todo-keyword-faces`. One `[[org.todo-keyword-styles]]` per \
             keyword with a `keyword` and any of `fg`, `bg`, `bold`, `italic`, \
             `underline`, `dim`.\n\n\
             `fg` / `bg` take a palette key (which follows a colourscheme swap) \
             or a literal `#rrggbb`. A modifier set to `false` CLEARS one \
             inherited from the state's default; omitted leaves it alone. These \
             are applied as theme OVERRIDES, so they beat the active \
             colourscheme \u{2014} but `:colorscheme` replaces the override set, \
             so a swap drops them until the next reload.",
        );
        let _ = register_option(
            "highest-priority",
            OptionType::String,
            DEFAULT_HIGHEST_PRIORITY,
            "The last priority letter `<leader>o,` cycles to. `C` gives A, B, C.",
        );
        // OC.2 / TC.5: the template SET, a STRUCTURED option — it declares its
        // schema and its value crosses as a tree.
        //
        // It used to be TOML inside a string, and that file's header said why:
        // an option was `boolean | integer | string`, a template is a record,
        // and an array-of-tables could not reach an option at all. It also said
        // what would happen if structured options ever landed — "the
        // declaration migrates and the template language does not change" —
        // and that is exactly this. The language is the same; what moved is who
        // parses it and who reports a bad field.
        let _ = crate::config_shape::register_option::<capture_templates::Declared>(
            "capture-templates",
            &DEFAULT_CAPTURE_TEMPLATES.to_vec(),
            "Your capture templates: one `[[org.capture-templates]]` per entry \
             with `key`, `description`, `target = { file = \"…\", headline = \"…\" }`, \
             a `body` and an optional `clock-in`. Unset means `<leader>oc` says \
             so rather than guessing. `:describe-option` shows the full shape.",
        );
        // OR.11b — roam's own template set. Separate from `capture-templates`
        // for the reason `roam_templates` opens with: a capture template says
        // WHERE its text lands, and a roam note's destination is a file that
        // does not exist yet, named after the node being made.
        //
        // Default EMPTY, and that is the feature rather than an omission:
        // unset means creating a note writes the built-in stub and does not
        // prompt, so note creation works for every user who has never heard of
        // templates.
        let _ = crate::config_shape::register_option::<roam_templates::Declared>(
            "roam-capture-templates",
            &Vec::new(),
            "What a new org-roam note starts as: one \
             `[[org.roam-capture-templates]]` per entry with a `key`, an \
             optional `description`, a `body`, and an optional `file` naming \
             the note's filename. Both `${title}` / `${slug}` / `${id}` and \
             the `%` capture placeholders expand. Unset means a new note is \
             the built-in stub and no menu is shown.",
        );
        let _ = register_option(
            "capture-file",
            OptionType::String,
            DEFAULT_CAPTURE_FILE,
            "Where `<leader>oc` files a capture when `capture-templates` is unset. \
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
        let _ = crate::config_shape::register_option::<Vec<String>>(
            "agenda-files",
            &Vec::new(),
            "Which files the agenda scans \u{2014} one path per element. A \
             directory means the org files directly in it, not its whole \
             subtree. Unset scans the project root, which is what nearly \
             everyone wants until they keep org files somewhere else.",
        );
        // AS.1: the dated section's window. Registered as a String rather than
        // an Integer to sit beside every other org option behind the one
        // `option_or` accessor — the seam has an Integer kind, but mixing
        // accessors for one subsystem's options buys nothing and costs a
        // second code path that can disagree about what "unset" means.
        let _ = register_option(
            "agenda-span",
            OptionType::String,
            DEFAULT_AGENDA_SPAN,
            "How many days past today the agenda's dated section covers. `7` \
             is a week counting today (emacs's default); `0` is the daily \
             agenda. Overdue items are shown by their own section regardless, \
             so a short span does not hide a missed deadline.",
        );
        // OA.15: what `l` shows. A String beside `agenda-span` for the reason
        // given there — one accessor per subsystem — and a comma/space list
        // rather than three booleans because it is emacs' own spelling and
        // because "which items" is one decision, not three.
        let _ = register_option(
            "agenda-log-mode-items",
            OptionType::String,
            DEFAULT_LOG_MODE_ITEMS,
            "Which log entries `l` admits into the agenda: any of `closed`, \
             `clock` and `state`, separated by commas or spaces, or `all`. \
             The default matches emacs \u{2014} what you finished and what you \
             spent time on. Log rows cover the span ENDING today, where the \
             plan covers the span starting today.",
        );
        // AS.2 / TC.6: the section set, a STRUCTURED option. It was TOML in a
        // string for `capture-templates`' reason and is a declared shape for
        // the same one — see `agenda_sections`. `when` is an `enum-of`, so a
        // typo is refused with the four valid spellings inline rather than
        // costing that block silently.
        let _ = crate::config_shape::register_option::<agenda_sections::Declared>(
            "agenda-sections",
            &Vec::new(),
            "The agenda's blocks: one `[[org.agenda-sections]]` per block with a \
             `title`, a `when` (`overdue`, `days`, `undated` or `any`), and \
             optionally `days`, `todo-only`, `min-priority` and `match`. A \
             `days` block with no `days` uses `org.agenda-span`. Unset gives \
             the built-in set: Overdue, Agenda, Unscheduled, Priority A.",
        );
        // OA.11 / TC.6: named agendas — `agenda-sections`' shape one level
        // deeper, and its `section` field IS `RawSection`, so a command's
        // blocks obey exactly the rules documented above. One declaration, so
        // there is no second place for `todo-only` to be spelled differently.
        let _ = crate::config_shape::register_option::<agenda_custom_commands::Declared>(
            "agenda-custom-commands",
            &Vec::new(),
            "Named agendas: one `[[org.agenda-custom-commands]]` per agenda \
             with a `key`, an optional `description`, and one or more `section` \
             blocks in `org.agenda-sections`' shape. The key is what selects \
             it. A command whose key names nothing, or a set that does not fit \
             the shape, falls back to the default agenda and says so in the \
             first header — a broken configuration costs you your layout, never \
             your rows. Unset means the default agenda, which is what nearly \
             everyone wants.",
        );
        // OR.4: the corpus root. UNSET by default, and that default is the
        // feature's contract — see `roam_scan::roam_directory`.
        let _ = register_option(
            "roam-directory",
            OptionType::String,
            "",
            "Where your org-roam notes live. `~` is expanded. Unset means \
             roam is inert: no walk, no watcher, no index, and `<CR>` on an \
             `[[id:\u{2026}]]` link says the directory is not configured rather \
             than blaming the filesystem. The directory must also be in this \
             plugin's `fs:read` grant, or the walk reaches nothing.",
        );
        let _ = register_option(
            "roam-dailies-directory",
            OptionType::String,
            "daily",
            "Where `:org-roam-dailies-*` files live. Relative to \
             `org.roam-directory` unless it starts with `/`.",
        );
        // HB.2: where a repeating task's state-change line goes.
        //
        // Defaults ON, which is NOT emacs' default (`org-log-into-drawer` is
        // nil there). The UX-follows-convention rule decides it: essentially
        // every real org configuration sets this — Dhruva's does — because
        // loose log lines under a habit are what people migrate away from once
        // a file has a few months of history in it. Org reads both spellings,
        // so the cost of the deviation is placement, not compatibility.
        // HB.6: the derived analytics beside the graph.
        //
        // Defaults ON because deriving them is the whole reason the design
        // refuses to store them (§1) — a number nobody sees is a number nobody
        // checks. Off is a real setting rather than a courtesy: the graph is
        // 29 cells and the suffix is a dozen more, which is a real cost on a
        // narrow terminal, and someone who wants org's picture exactly should
        // be able to have it.
        let _ = register_option(
            "habit-stats",
            OptionType::Boolean,
            "true",
            "Append the streak, completion rate and weakest weekday to a \
             habit's consistency graph in the agenda.",
        );
        let _ = register_option(
            "log-into-drawer",
            OptionType::Boolean,
            "true",
            "Write state-change log lines into a `:LOGBOOK:` drawer rather \
             than loose under the headline (emacs' `org-log-into-drawer`).",
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
                todo_query::rules(&todo::parse_todo_keywords(&todo_keyword_lines().join("\n")))
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
        // OR.4b: the scan's progress. Priority 9 puts it right of the
        // clock and left of `core.position`.
        ui::register_segment(ROAM_SEGMENT, UiZone::Right, 9);
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
        // OR.4a: and the option itself. The boot walk below runs BEFORE a
        // user's `init.rs` has set `org.roam-directory` — that is the
        // documented `plugin-loaded` config shape — so without this the walk
        // reads an unset option and roam is silently inert for exactly the
        // users who configured it correctly.
        //
        // Unconditional, like the watch subscription above and for the same
        // reason: an option that is never set delivers nothing and costs
        // nothing.
        events::subscribe(
            &EventFilter {
                kinds: Some(vec![EventKind::OptionChanged]),
                path_globs: None,
                major_modes: None,
            },
            ON_ROAM_OPTION_CHANGED,
        );
        // OA.15b: the log mode's BODY.
        //
        // A native minor supplies one itself; a plugin mode is data with a
        // no-op `on_activate`, so this subscription is what makes
        // `org-agenda-log-mode` a switch rather than a label. Both directions
        // are needed — without the deactivation half, `l` would turn log rows
        // on and never off, with the mode reporting itself inactive.
        events::subscribe(
            &EventFilter {
                kinds: Some(vec![EventKind::MinorActivated, EventKind::MinorDeactivated]),
                path_globs: None,
                major_modes: None,
            },
            ON_AGENDA_LOG_MODE,
        );
        // Boot does a full walk, because lattice was not running while the
        // corpus changed and a watcher cannot report what it did not see.
        // Unchanged files cost a read and a hash; only moved ones parse. A
        // no-op when roam is unconfigured.
        //
        // OR.4b: `begin` only QUEUES the walk. The indexing happens one batch
        // per call, driven by `EV_ROAM_SCAN_STEP` below — a whole corpus in
        // this one call is what used to trap and quarantine the plugin.
        arm_roam_scan();
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
        // OR.4a: the corpus root was named (or moved). Walk it.
        //
        // Filtered by NAME here rather than by the subscription, because the
        // event filter has no option-name field — every option change is
        // delivered and this arm decides. `roam-dailies-directory` is
        // deliberately not included: it changes where a journal FILE goes, not
        // which tree is indexed, and the walk covers it either way when it sits
        // under the roam directory.
        if handler == ON_ROAM_OPTION_CHANGED {
            if let Event::OptionChanged(c) = ev {
                if c.name != "org.roam-directory" {
                    return;
                }
                // Re-reads the option, arms the watch on the new root and
                // queues a fresh scan. The generation bump inside `begin` is
                // what stops a chain still draining the OLD root, rather than
                // letting two interleave their writes.
                //
                // A cleared value queues nothing, which is the honest answer
                // for "roam is now unconfigured" — the stale index stays until
                // something replaces it rather than the picker going empty
                // with no explanation.
                arm_roam_scan();
            }
            return;
        }
        // OA.15b: log mode went on or off. Derive the `log=` argument from the
        // mode's new state and re-open the view — the mode is the truth, the
        // argument is its shadow.
        if handler == ON_AGENDA_LOG_MODE {
            let (mode, on) = match &ev {
                Event::MinorActivated(m) => (m.mode.as_str(), true),
                Event::MinorDeactivated(m) => (m.mode.as_str(), false),
                _ => return,
            };
            // Filtered by NAME here rather than by the subscription, because
            // the event filter has no mode field — every minor's lifecycle is
            // delivered and this arm decides. Same shape as
            // `ON_ROAM_OPTION_CHANGED` two arms up.
            if mode != AGENDA_LOG_MODE_ID {
                return;
            }
            let mut view = VIEW_ARGS.with_borrow(Clone::clone);
            view.log = on.then(|| {
                // Read from the option every time the mode goes ON rather than
                // remembered from last time: `:set org.agenda-log-mode-items`
                // has to take effect on the next `l`, and a remembered set
                // would make the option appear not to work until the view was
                // closed.
                let (items, _problems) = agenda_log::LogItems::parse(&option_or(
                    "agenda-log-mode-items",
                    DEFAULT_LOG_MODE_ITEMS,
                ));
                // A set that parsed to nothing would make "on" produce a view
                // identical to "off" — indistinguishable from a dead key.
                // Emacs' default is what a misconfigured option falls back to.
                if items.any() {
                    items
                } else {
                    agenda_log::LogItems::DEFAULT
                }
            });
            // OA.15a's seam. An `Effect` return is not available here —
            // `on-event` answers `()` by construction — which is precisely the
            // gap that slice closed.
            lattice::plugin_host::multibuffer_view_registry::refresh_view(
                "agenda",
                &view.to_args(),
            );
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
                // OR.4b: queue the walk, then let the batch chain drain it.
                arm_roam_scan();
            }
            // OR.4b: one batch, then re-arm. The generation rides in the
            // payload so a chain from a previous root stops here rather than
            // interleaving its writes with the current scan's.
            EV_ROAM_SCAN_STEP => {
                let generation = String::from_utf8_lossy(&p.payload)
                    .parse::<u64>()
                    .unwrap_or(0);
                if roam_scan::step(generation) {
                    host_services::emit_event(EV_ROAM_SCAN_STEP, generation.to_string().as_bytes());
                }
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
                // OC.6 / OA.27 — the clock, under `<leader>ox…`.
                //
                // **`ox` because emacs is `C-c C-x`.** Clocking is the whole
                // reason that prefix exists in org, and the letters under it
                // are org's own: `i` in, `o` out, `q` cancel, `j` goto. A hand
                // that knows emacs already types the second and third keys
                // correctly; only the entry into the prefix differs.
                //
                // OA.27 moved them off the flat `<leader>oi` / `oO` / `oq` /
                // `oj` they landed on at OC.6. Those were chosen because the
                // letters were free, which is a reason to pick a key and not a
                // reason to keep one: they scattered a five-command group
                // across the top level, spent the scarce single letters `i`,
                // `q` and `j` on it, and left `i` — the natural prefix for
                // inserting things — meaning "clock in".
                //
                // `<leader>oi` is now free. Deliberately left free rather than
                // filled: this slice is a reorganisation, and inventing an
                // insert group to justify it would be the feature creep the
                // reorganisation is meant to make room for.
                bind("<leader>oxi", "org-clock-in"),
                bind("<leader>oxo", "org-clock-out"),
                bind("<leader>oxq", "org-clock-cancel"),
                bind("<leader>oxj", "org-clock-goto"),
                // OC.9: emacs spells resume `C-c C-x C-x`, which would be
                // `oxx` — a doubled letter that says nothing. `r` for resume
                // is the mnemonic the rest of this group already uses, and the
                // command has no muscle memory to protect: it is org-mode's
                // own `org-clock-in-last`, which few people bind at all.
                bind("<leader>oxr", "org-clock-resume"),
                // The emacs spelling too, letter for letter, on the same
                // `ActionId`s — the pattern `<leader>on…` / `<C-c>n…` already
                // set for roam. Safe for `<C-c>`'s reason: it is only ever a
                // PREFIX here, so `<C-c>` alone stays vim's interrupt.
                //
                // No conflict with org's terminal `<C-x>` (timestamp
                // decrement, OM.9): that is `<C-x>` as the FIRST key, and this
                // reaches it only after `<C-c>`, which the trie is already
                // waiting on.
                bind("<C-c><C-x><C-i>", "org-clock-in"),
                bind("<C-c><C-x><C-o>", "org-clock-out"),
                bind("<C-c><C-x><C-q>", "org-clock-cancel"),
                bind("<C-c><C-x><C-j>", "org-clock-goto"),
                bind("<C-c><C-x><C-x>", "org-clock-resume"),
                // OM.11: opens the target picker; `org-refile-to` is the
                // second hop and is NOT bound — it is invoked by the picker's
                // accept, never typed.
                bind("<leader>or", "org-refile"),
                // OM.13 — the emacs spellings, letter for letter.
                //
                // These commands existed with the vim spelling alone, and
                // `<C-c>` gained an emacs peer only for the slices that
                // happened to touch it (schedule/deadline at OA.25, the clock
                // at OA.27). A half-copied set is the failure
                // `prefer-minor-modes-over-duplication` names: the gap
                // announces itself to nobody, and `<C-c><C-t>` was reported as
                // broken rather than absent.
                //
                // Safe for `<C-c>`'s usual reason — only ever a PREFIX here, so
                // `<C-c>` alone stays vim's interrupt.
                bind("<C-c><C-w>", "org-refile"),
                bind("<C-c><C-o>", "org-open-link"),
                bind("<C-c><C-x><C-a>", "org-archive-subtree"),
                bind("<C-c><C-x><C-v>", "org-toggle-inline-images"),
                bind("<C-c>*", "org-toggle-heading"),
                // OE.3 — emacs' `C-c C-c`, on the MAJOR and deliberately not
                // on a minor.
                //
                // `org-capture-mode` binds the same chord to finalize, and a
                // MINOR's layer beats its major's — which is what guarantees
                // a capture buffer files instead of dispatching. Bound on
                // `org-todo-mode` (a minor) first, this competed with capture
                // minor-to-minor, the order is not either mode's to control,
                // and capture lost: `C-c C-c` opened a TAGS prompt on the
                // template's headline and the note was never written. The
                // test that presses the chord rather than dispatching the
                // action by name is what caught it.
                //
                // It is also org's first TERMINAL binding under `<C-c>`, and
                // must stay the only one: every other org chord here is a
                // PREFIX (`<C-c>a`, `<C-c>n…`, `<C-c><C-x>…`), and a terminal
                // node kills any longer chord grown beneath it —
                // `KeymapTrie::lookup` answers `Bound` at the first binding
                // and never consults children.
                bind("<C-c><C-c>", "org-ctrl-c-ctrl-c"),
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
            options: vec![
                ModeOptionOverride {
                    name: "foldmethod".to_string(),
                    value: "syntax".to_string(),
                    priority: OverridePriority::Normal,
                },
                // …and the fold tree starts CLOSED. `foldmethod=syntax` builds
                // the tree; without this it builds it and then opens all of it,
                // because `foldlevel` defaults to 99 (effective infinity) so
                // that search results, project diffs and agent transcripts —
                // whose folds come from always-registered overlay sources — do
                // not open collapsed to nothing.
                //
                // Org is the case that default is wrong for. An outline that
                // opens fully expanded is a wall of text; emacs org ships
                // `#+STARTUP: overview` as its default for exactly this reason,
                // and `foldlevel=0` is that in vim's vocabulary — level 1 is
                // the outermost fold, so 0 closes every one of them and leaves
                // the top-level headlines standing. `<Tab>` / `<S-Tab>` cycle
                // out from there, which is the gesture the collapsed state
                // exists to make meaningful.
                //
                // A layer like its neighbour: the user's global `foldlevel` is
                // untouched everywhere else, and a `:setlocal foldlevel=99` in
                // an org buffer still wins.
                ModeOptionOverride {
                    name: "foldlevel".to_string(),
                    value: "0".to_string(),
                    priority: OverridePriority::Normal,
                },
            ],
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
                // OA.25 took `<leader>os` for scheduling, which is emacs'
                // `C-c C-s` and the strongest muscle memory org has. Fast
                // select moves up a shift rather than away: it is our own
                // invention (emacs reaches it through `C-c C-t`), so it is
                // the one with no convention to break.
                bind("<leader>oS", "org-todo-select"),
                bind("<leader>o,", "org-priority-cycle"),
                bind("<leader>o:", "org-set-tags"),
                // OA.25 — planning, in both spellings for the reason the roam
                // keys carry both: `<leader>o…` is the vim-native one, and
                // `<C-c><C-s>` / `<C-c><C-d>` is what a hand arriving from
                // emacs already knows. Two spellings of one `ActionId`, so
                // there is no second handler to keep in step.
                //
                // `<C-c>` stays safe because it is only ever a PREFIX here —
                // see `org-global-mode`'s note. No collision with capture's
                // `<C-c><C-c>` / `<C-c><C-k>`: different second key.
                bind("<leader>os", "org-schedule"),
                bind("<leader>od", "org-deadline"),
                bind("<C-c><C-s>", "org-schedule"),
                bind("<C-c><C-d>", "org-deadline"),
                // OM.13 — the rest of emacs' headline set. `C-c C-t` is the
                // one people reach for first and the one that was missing.
                //
                // It opens the MENU, not the cycle, and that is emacs': with
                // `org-use-fast-todo-selection` on — which is what defining
                // `(t)` keys in `org-todo-keywords` turns on — `C-c C-t`
                // offers the states and you pick one. Cycling to a state four
                // presses away is the thing fast-select exists to replace.
                //
                // `<leader>ot` stays the cycle. The two are different verbs and
                // both are wanted: cycling is faster when the next state IS the
                // one you want, which is most of the time.
                bind("<C-c><C-t>", "org-todo-select"),
                bind("<C-c><C-q>", "org-set-tags"),
                // OE.2 — emacs' own `C-c C-x p`, which sits beside the clock
                // family already under `<C-c><C-x>`. `<leader>op` is the
                // vim-native spelling of the same `ActionId`; two spellings,
                // one handler.
                bind("<C-c><C-x>p", "org-set-property"),
                bind("<leader>op", "org-set-property"),
                bind("<C-c>,", "org-priority-cycle"),
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
        // rows through the `scanned-excerpt-source` seam — what changed is who owns the
        // trigger. A host-registered `:agenda` meant a feature every user calls
        // `org-agenda` shipped under a generic name that the plugin had no way
        // to correct from its own side.
        register_mode(&ModeDeclaration {
            id: "org-global-mode".to_string(),
            kind: ModeKind::Minor,
            activation_policy: ActivationPolicy::Universal,
            capabilities: ModeCapabilities::empty(),
            keymap: vec![
                // OA.13: `oa` opens the DISPATCHER, not the agenda directly.
                //
                // A deliberate behaviour change, and the emacs-faithful one:
                // `C-c a` has always meant "choose an agenda", with `C-c a a`
                // being the built-in one. That second `a` is why the menu puts
                // the built-in agenda on `a` — the muscle memory people arrive
                // with already spells it.
                //
                // `:org-agenda` still opens the default agenda directly, so
                // nothing lost a way to get there; what moved is only what the
                // CHORD does. Landed as its own slice so it can be reverted
                // alone if it turns out to annoy.
                bind("<leader>oa", "org-agenda-menu"),
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
                // OR.10 — the journal, under `<leader>ond…` because emacs
                // org-roam puts it under `C-c n d` and the letters are the
                // same: `d` today, `y` yesterday, `t` tomorrow, `D` a date you
                // are asked for. Universal for find-node's reason, and more
                // so: the whole point of "open today's journal" is that you
                // are somewhere else when you want it.
                bind("<leader>ondd", "org-roam-dailies-today"),
                bind("<leader>ondy", "org-roam-dailies-yesterday"),
                bind("<leader>ondt", "org-roam-dailies-tomorrow"),
                bind("<leader>ondD", "org-roam-dailies-goto-date"),
                // The SAME commands under `<C-c>n…`, emacs org-roam's own
                // prefix, letter for letter.
                //
                // Both, not one. `<leader>on…` is the vim-native spelling and
                // stays; `<C-c>n…` is what a hand coming from emacs already
                // knows, and it is three keystrokes rather than five. Two
                // spellings of one command is cheap — they resolve to the same
                // `ActionId`, so there is no second handler to keep in step.
                //
                // **`<C-c>` is safe here because it is a PREFIX, never a
                // terminal binding.** `<C-c>` alone stays vim's interrupt; only
                // `<C-c>n` continues into org. That is the same reason magit
                // binds `<C-c>g` / `<C-c>f` on its own global mode, which is
                // the precedent this follows — and it is why the `<C-x>o`
                // proposal could NOT work, since org's major binds a terminal
                // `<C-x>` (timestamp decrement, OM.9) and a prefix in one layer
                // against a terminal binding in another is the ambiguity vim
                // settles with `timeoutlen`, which this editor does not have.
                //
                // No collision with magit's buffer-local `<C-c><C-c>` /
                // `<C-c><C-k>`: those continue on a different second key.
                bind("<C-c>nf", "org-roam-find-node"),
                bind("<C-c>ndd", "org-roam-dailies-today"),
                bind("<C-c>ndy", "org-roam-dailies-yesterday"),
                bind("<C-c>ndt", "org-roam-dailies-tomorrow"),
                bind("<C-c>ndD", "org-roam-dailies-goto-date"),
                // And emacs org's own two global entry points, for the same
                // reason and under the same safety argument: `C-c a` and
                // `C-c c` are the first two lines of every org setup guide
                // ever written, so they are the bindings a hand arriving
                // from emacs reaches for before it has read anything of
                // ours. `<leader>oa` / `<leader>oc` above stay — two
                // spellings of one `ActionId`, no second handler.
                //
                // `a` and `c` are free beneath `<C-c>`: magit's are
                // `<C-c><C-c>` / `<C-c><C-k>`, which continue on a CONTROL
                // key, and `<C-c>g` / `<C-c>f` differ in the second letter.
                // OA.13: both spellings move together, because they are two
                // spellings of one thing. `C-c a` opening a chooser is what
                // emacs does, so this is the binding arriving hands already
                // expect — the divergence was the OLD behaviour.
                bind("<C-c>a", "org-agenda-menu"),
                bind("<C-c>c", "org-capture-menu"),
            ],
            target_language: None,
            // MO.1: this mode sets no options for its buffers.
            options: vec![],
        });

        // OC.7b — `org-capture-mode`, the capture buffer's minor.
        //
        // A MINOR on an `org-mode` major, not a major of its own. A capture
        // buffer IS an org buffer — the user wants org's grammar, motions,
        // folding and TODO cycling while writing the entry — so only the
        // finalize/abort pair is capture-specific.
        //
        // Which is also why the chords cannot go on `org-mode`: `C-c C-c`
        // there would file-and-close every org file the user touched.
        // Scoping them to a minor that is activated on exactly one buffer is
        // what makes `<C-c>` safe to bind at all — it is vim's interrupt, and
        // a global binding would shadow it.
        //
        // MANUAL activation: the buffer names the minor when it opens
        // (`open-synthetic-buffer.activate-minor`), which is the only place
        // that knows a capture is what this buffer is for. No
        // `ActivationPolicy` can express "the buffer the capture flow just
        // created" — the same reason `org-agenda-mode` is Manual.
        //
        // Reused verbatim by org-roam capture (OR.11): same buffer, same
        // chords, same handlers; only the order of what is asked before the
        // buffer opens differs.
        let _ = register_mode(&ModeDeclaration {
            id: "org-capture-mode".to_string(),
            kind: ModeKind::Minor,
            activation_policy: ActivationPolicy::Manual,
            capabilities: ModeCapabilities::empty(),
            keymap: vec![
                bind("<C-c><C-c>", "org-capture-finalize"),
                bind("<C-c><C-k>", "org-capture-abort"),
            ],
            target_language: None,
            // The capture buffer opens EXPANDED, overriding the `foldlevel=0`
            // its `org-mode` major declares.
            //
            // That default is right for a FILE — emacs ships
            // `#+STARTUP: overview` and a large outline opening as a wall of
            // text is unreadable — and wrong for a capture, which is a handful
            // of lines the user is about to type into. Opening it collapsed
            // hides the template you are filling in, which is the one thing on
            // screen you need to see.
            //
            // A minor layers above its major, so this wins where it activates
            // and nowhere else: org files keep opening collapsed. org-roam
            // capture rides the same mode and gets the same answer.
            options: vec![ModeOptionOverride {
                name: "foldlevel".to_string(),
                value: "99".to_string(),
                priority: OverridePriority::Normal,
            }],
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
                // OA.25 — scheduling from the agenda, the same four bindings
                // and the same four actions `org-todo-mode` declares.
                //
                // **Repeated here because the seam cannot express it once.**
                // `ActivationPolicy` is `Majors(["org-mode"])` OR `Manual`,
                // never both, and the agenda's major is `multibuffer-mode` —
                // shared with project search and magit diffs, where a
                // scheduling key means nothing. So one mode cannot cover both
                // surfaces, and the repetition is of the BIND lines only: the
                // handler bodies live once, behind these `ActionId`s. That is
                // the same trade `<leader>ot` already makes two lines up.
                //
                // Not bare `s` / `d`, unlike OA.20's `f` / `b`. Emacs' agenda
                // spells scheduling `C-c C-s` there too, so the prefixed form
                // IS the convention — and a bare `d` next to a read-only view
                // reads like a delete.
                bind("<leader>os", "org-schedule"),
                bind("<leader>od", "org-deadline"),
                bind("<C-c><C-s>", "org-schedule"),
                bind("<C-c><C-d>", "org-deadline"),
                // OM.13 — and the same headline set here, because the agenda
                // is a place you edit a headline FROM. Repeated bind lines,
                // one set of handlers: `org-todo-mode` cannot activate on a
                // `multibuffer-mode` major, which is the constraint recorded
                // at OA.25.
                bind("<C-c><C-t>", "org-todo-select"),
                bind("<leader>oS", "org-todo-select"),
                bind("<C-c>,", "org-priority-cycle"),
                // OE.2 — emacs binds `C-c C-x p` in the agenda too
                // (`org-agenda-set-property`), and it works here for OA.25's
                // reason: `PlanTarget` resolves a row to the headline in its
                // SOURCE file, so the drawer is written where the entry lives.
                // Repeated bind line, one handler — the constraint OA.25
                // recorded.
                bind("<C-c><C-x>p", "org-set-property"),
                bind("<leader>op", "org-set-property"),
                // OA.20: emacs' span-walking keys. Bare letters, which is safe
                // here and nowhere else — the agenda is read-only, so `f` and
                // `b` are not shadowing an edit, and this mode activates on
                // agenda views alone.
                bind("f", "org-agenda-later"),
                bind("b", "org-agenda-earlier"),
                bind(".", "org-agenda-today"),
                // OA.18: `gD` is emacs' view-mode dispatch (`v` there,
                // `gD` in evil-org-agenda), and it is a MENU rather than a
                // prefix now.
                //
                // OA.20 bound `gDd` / `gDw` / `gDm` / `gDy` directly, and
                // those four lines are gone rather than kept beside this one:
                // `KeymapTrie::lookup` answers `Bound` as soon as the walk
                // reaches a node with a binding and never consults its
                // children, so `gD` and `gDd` cannot both fire. Keeping both
                // would have left the four span chords dead — and dead
                // *quietly*, since the trailing letter falls through to the
                // grammar in a read-only view.
                //
                // The keystrokes are unchanged: `gD` then `d` is still the day
                // view. What the menu adds is the two rows a chord had nowhere
                // to advertise, `l` and `r`.
                bind("gD", "org-agenda-view-menu"),
                // OA.15: emacs' `l`. Bare, for `f` / `b`'s reason — the agenda
                // is read-only, so a letter here shadows nothing that could be
                // typed, and this mode activates on agenda views alone.
                bind("l", "org-agenda-log-mode"),
                // OA.21: emacs' filter keys. `/` replaces the tag filter,
                // `\` narrows it further, `|` clears everything.
                bind("/", "org-agenda-filter-by-tag"),
                bind("\\", "org-agenda-filter-add-tag"),
                bind("|", "org-agenda-filter-clear"),
                // OA.26: `<` restricts to the row's file, as emacs' `<` locks
                // the agenda to a file. `<CR>` goes to the headline — emacs
                // spells that `RET` (`org-agenda-switch-to`); its `TAB` peer
                // is not free here, the shared `foldable-view-mode` owns it.
                // `<lt>`, vim's spelling for a literal `<` — a bare one is an
                // unterminated special-key form and the binding never parses.
                bind("<lt>", "org-agenda-filter-by-file"),
                bind("<CR>", "org-agenda-goto"),
                // OA.4b: `<Tab>` / `<S-Tab>` are NOT bound here. They come
                // from the host's shared `foldable-view-mode`, which the
                // agenda's native view mode pulls in by declaring
                // `fold_toggle_action()` — the same chord magit, project
                // search, the references view, `*problems*` and
                // `*compilation*` get. This mode briefly carried its own
                // copy; that was the third one in the tree, and four
                // foldable views had none.
            ],
            target_language: None,
            // AF.2 REVERSED: the agenda opens EXPANDED.
            //
            // It opened collapsed, on the reasoning that a view whose entire
            // structure is blocks should show its blocks. That reasoning was
            // about the view; the agenda's job is task tracking, planning and
            // scheduling, and for those the rows ARE the content — a plan you
            // have to expand four folds to read is a plan you do not read.
            // Emacs's own agenda opens with every entry visible for the same
            // reason, and `<Tab>` / `<S-Tab>` still collapse from there when a
            // block is in the way.
            //
            // Still declared HERE rather than on the multibuffer major, and
            // the scoping is still the point: `multibuffer-mode` is also
            // project search, project diff and the references view.
            // `org-agenda-mode` activates on agenda views and nothing else, so
            // it is the narrowest mode that owns the question — and the
            // override stays rather than being deleted because the global
            // default it would fall back to is not the agenda's to depend on.
            //
            // A layer, so `:setlocal foldlevel=0` in the agenda still wins.
            options: vec![ModeOptionOverride {
                name: "foldlevel".to_string(),
                value: "99".to_string(),
                priority: OverridePriority::Normal,
            }],
        });

        // OA.15b — `org-agenda-log-mode`, layered on the agenda view.
        //
        // Emacs' `l`: what you DID, beside what you plan to do — closed,
        // clocked and state-changed headlines, filed under the day each
        // happened.
        //
        // **An empty keymap, and that is not an oversight.** `l` binds on
        // `org-agenda-mode` above, not here — the one place "modes own their
        // full surface" cannot be read literally, since a keymap layer is
        // gated to buffers where its mode is ACTIVE, so an `l` declared here
        // could only ever turn the mode off. The switch belongs to the surface
        // that offers it; everything the mode does stays with the mode. Same
        // split as `cr` on `scan-view-mode` (OA.16), one layer over.
        //
        // What the mode owns instead is its BODY, in the `minor-activated` /
        // `minor-deactivated` handler (`ON_AGENDA_LOG_MODE`) — which is the
        // whole reason this is a mode rather than a bare view argument. It
        // gives `:describe-mode` something to answer, the `gD` transient
        // (OA.18) something to name, `:org-agenda-log-mode` a live toggle, and
        // a later log-specific chord a home.
        //
        // Manual, for `org-agenda-mode`'s reason: no policy can say "the view
        // the agenda provider just built", and one keyed on `multibuffer-mode`
        // would put log rows in project search and magit diffs.
        //
        // No option overrides: log mode changes which ROWS the scan produces,
        // not how the buffer behaves.
        register_mode(&ModeDeclaration {
            id: "org-agenda-log-mode".to_string(),
            kind: ModeKind::Minor,
            activation_policy: ActivationPolicy::Manual,
            capabilities: ModeCapabilities::empty(),
            keymap: Vec::new(),
            target_language: None,
            options: Vec::new(),
        });

        // OM.12 / TB.2 — `org-table-mode`, the third mode.
        //
        // It used to own the whole pipe-table surface, and its `<Tab>` sat
        // above `org-mode`'s to make the chain two hops: table → headline
        // cycle → whatever `<Tab>` natively means. TB.2 moved that surface to
        // the host's `table-mode`, which sits in the same place in the layer
        // order and declines the same way — the chain is unchanged, one of
        // its links just lives somewhere else now.
        register_mode(&ModeDeclaration {
            id: "org-table-mode".to_string(),
            kind: ModeKind::Minor,
            activation_policy: ActivationPolicy::Majors(vec!["org-mode".to_string()]),
            capabilities: ModeCapabilities::empty(),
            // TB.2: EMPTY, and deliberately still declared.
            //
            // Every chord this carried was generic — `<Tab>` walks cells in a
            // markdown table exactly as it does in an org one — so all eleven
            // moved to the host's `table-mode`, which activates on both
            // majors. Org users keep the same keys; markdown users get them
            // without installing this plugin, which is the whole point.
            //
            // What is left is the mode's REASON: this is where table
            // behaviour that is genuinely org's goes, and `#+TBLFM:` formulas
            // are the large one waiting. Deleting the mode and re-adding it
            // then would rename an entry in the plugin's mode list to buy
            // nothing; keeping it is what makes the split legible to whoever
            // picks that up. An empty keymap pushes no layer, so it costs a
            // registry entry and a `:describe-mode` page saying so.
            keymap: vec![],
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
            "Open the agenda: every dated row a scanned-excerpt-source finds under the \
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
        // TB.2: the eleven `org-table-*` actions are gone. Pipe-table
        // editing is the host's `table-mode` now — `action:table-align`,
        // `action:table-next-cell` and the rest — because the surface is
        // markdown's as much as org's and only the host can serve both.
        // Registering ours too would be the duplication the move removed,
        // wearing the `:` line as a disguise.
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
            "org-agenda-menu",
            "Choose an agenda from a menu, keyed the way org.agenda-custom-commands says",
            &spec(),
            AGENDA_MENU,
        );
        // OE.2 — three actions for one command: the chord fires the first, the
        // host dispatches the other two as each prompt is submitted. All three
        // are registered because the host resolves a submit action BY NAME.
        register_action(
            "org-set-property",
            "Set a property on the entry at the cursor",
            &spec(),
            SET_PROPERTY,
        );
        register_action(
            "org-set-property-key",
            "Take the property name typed at the prompt and ask for its value",
            &spec(),
            SET_PROPERTY_KEY,
        );
        register_action(
            "org-set-property-value",
            "Write the property typed at the prompt into the entry's drawer",
            &spec(),
            SET_PROPERTY_VALUE,
        );
        register_action(
            "org-agenda-view-menu",
            "Change how the agenda is shown \u{2014} span, log mode, clock report",
            &spec(),
            AGENDA_VIEW_MENU,
        );
        register_action(
            "org-agenda-command",
            "Open the agenda named in args, from org.agenda-custom-commands",
            &spec(),
            AGENDA_COMMAND,
        );
        for (name, doc, id) in [
            (
                "org-agenda-later",
                "Move the agenda one span forward",
                AGENDA_LATER,
            ),
            (
                "org-agenda-earlier",
                "Move the agenda one span back",
                AGENDA_EARLIER,
            ),
            (
                "org-agenda-today",
                "Return the agenda to today",
                AGENDA_TODAY,
            ),
            (
                "org-agenda-filter-by-tag",
                "Narrow the agenda to a tag (replacing any tag filter)",
                AGENDA_FILTER_TAG,
            ),
            (
                "org-agenda-filter-add-tag",
                "Narrow the agenda by one more tag",
                AGENDA_FILTER_TAG_ADD,
            ),
            (
                "org-agenda-filter-submit",
                "Apply the tag typed at the filter prompt",
                AGENDA_FILTER_TAG_SUBMIT,
            ),
            (
                "org-agenda-filter-clear",
                "Drop every agenda filter",
                AGENDA_FILTER_CLEAR,
            ),
            // OA.26: the two remaining consumers of OA.23's seam.
            (
                "org-agenda-goto",
                "Jump to the headline this agenda row came from",
                AGENDA_GOTO,
            ),
            (
                "org-agenda-filter-by-file",
                "Restrict the agenda to the file the row under the cursor is in",
                AGENDA_FILTER_FILE,
            ),
            // OA.15: emacs' `l`.
            (
                "org-agenda-log-mode",
                "Show what was closed, clocked and changed \u{2014} the span's log, not its plan",
                AGENDA_LOG_MODE,
            ),
            ("org-agenda-day-view", "Show one day", AGENDA_SPAN_DAY),
            ("org-agenda-week-view", "Show one week", AGENDA_SPAN_WEEK),
            ("org-agenda-month-view", "Show one month", AGENDA_SPAN_MONTH),
            ("org-agenda-year-view", "Show one year", AGENDA_SPAN_YEAR),
        ] {
            register_action(name, doc, &spec(), id);
        }
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
            "org-ctrl-c-ctrl-c",
            "Act on the thing at the cursor \u{2014} toggle a checkbox, set a headline's tags",
            &spec(),
            CTRL_C_CTRL_C,
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
        // `<leader>oxi`. One registration serves both surfaces: a mode keymap
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
            "org-roam-backlinks",
            "List the notes that link to the org-roam node at point.",
            &clock_ex(),
            CLOCK_PARSE,
            ROAM_BACKLINKS,
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
        // OR.11b — the second hop of a templated create. Registered as an
        // ex-command because a transient row names a COMMAND, and reached only
        // that way: the menu is what knows both the title and the chosen key.
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-create-from-template",
            "Create an org-roam note from a chosen template. Dispatched by the \
             template menu `:org-roam-create-node` opens, not usually typed.",
            &lattice::plugin_host::types::ExCommandSpec {
                latency_class: LatencyClass::Reflex,
                accepts_bang: false,
                accepts_range: false,
                args_schema: Vec::new(),
                surface_form: SurfaceForm::Keyword,
            },
            ROAM_CREATE_PARSE,
            ROAM_CREATE_FROM_TEMPLATE,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-sync",
            "Re-scan `org.roam-directory` and rebuild the roam index. The \
             escape hatch when the watcher missed something.",
            &clock_ex(),
            CLOCK_PARSE,
            ROAM_SYNC,
        );
        // OR.10 — the journal. Three commands that take nothing and one that
        // takes a date optionally.
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-dailies-today",
            "Open today's journal entry, creating it if it does not exist yet.",
            &clock_ex(),
            CLOCK_PARSE,
            ROAM_DAILIES_TODAY,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-dailies-yesterday",
            "Open yesterday's journal entry, creating it if it does not exist yet.",
            &clock_ex(),
            CLOCK_PARSE,
            ROAM_DAILIES_YESTERDAY,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-dailies-tomorrow",
            "Open tomorrow's journal entry, creating it if it does not exist yet.",
            &clock_ex(),
            CLOCK_PARSE,
            ROAM_DAILIES_TOMORROW,
        );
        lattice::plugin_host::grammar::register_ex_command(
            "org-roam-dailies-goto-date",
            "Open the journal entry for a date, given as `YYYY-MM-DD`. With no \
             date, asks for one.",
            &lattice::plugin_host::types::ExCommandSpec {
                latency_class: LatencyClass::Reflex,
                accepts_bang: false,
                accepts_range: false,
                args_schema: Vec::new(),
                surface_form: SurfaceForm::Keyword,
            },
            ROAM_DAILIES_DATE_PARSE,
            ROAM_DAILIES_GOTO_DATE,
        );
        register_action(
            "org-roam-dailies-goto-date-submit",
            "Open the journal entry for the date typed into the dailies prompt",
            &spec(),
            ROAM_DAILIES_GOTO_DATE_SUBMIT,
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
            "org-roam-capture-fields-submit",
            "Open the roam note the template menu collected (fired by its own row)",
            &spec(),
            ROAM_CAPTURE_FIELDS_SUBMIT,
        );
        register_action(
            "org-capture-finalize",
            "File the capture buffer's contents (C-c C-c)",
            &spec(),
            CAPTURE_FINALIZE,
        );
        register_action(
            "org-capture-abort",
            "Discard the capture, writing nothing (C-c C-k)",
            &spec(),
            CAPTURE_ABORT,
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
        // OA.25 — planning. Registered beside the rest of `org-todo-mode`'s
        // actions because they are the same workflow: a headline you schedule
        // is a headline you track, and `:org-todo-mode` off should take
        // scheduling with it.
        register_action(
            "org-schedule",
            "Schedule this headline, or clear it with an empty answer",
            &spec(),
            SCHEDULE,
        );
        register_action(
            "org-todo-note-submit",
            "Record the note a state change asked for (internal)",
            &spec(),
            TODO_NOTE_SUBMIT,
        );
        register_action(
            "org-schedule-submit",
            "Apply a date submitted from the org schedule prompt (internal)",
            &spec(),
            SCHEDULE_SUBMIT,
        );
        register_action(
            "org-deadline",
            "Set this headline's deadline, or clear it with an empty answer",
            &spec(),
            DEADLINE,
        );
        register_action(
            "org-deadline-submit",
            "Apply a date submitted from the org deadline prompt (internal)",
            &spec(),
            DEADLINE_SUBMIT,
        );
    }

    /// Org's manual, compiled into this component and handed over once at
    /// load — the `help` seam's premise: the docs travel with the thing they
    /// document, and unloading the plugin removes them.
    ///
    /// An empty name lands at the bare plugin id, so the first is `:help org`
    /// rather than `:help org.org`; a named one is auto-namespaced, so `roam`
    /// lands at `:help org.roam`.
    ///
    /// **Roam is its own page rather than a section of `org.md`** (OR.12).
    /// The manual is already the longest thing in this repository and roam is
    /// a self-contained layer with its own model — nodes, an index, a
    /// watcher — that an org user who keeps no zettelkasten never touches.
    /// Splitting it means `:help org` stays about editing org files and
    /// `:help org.roam` is reachable by name from the picker, rather than
    /// being a heading two thirds of the way down someone else's page.
    ///
    /// **The files are `doc/org.md` and `doc/roam.md`, named for the topic
    /// each one lands at — not `doc/org-roam.md`.** The `org` prefix is the
    /// host's to add, from the manifest id; a file that carries it too claims
    /// a topic name that does not exist. That is not cosmetic: a reader who
    /// sees `doc/org-roam.md` types `:help org-roam`, gets `no help topic:
    /// org-roam`, and concludes the page was never shipped. Reported
    /// 2026-09-05, and the file name was the whole of the bug.
    fn register_help_topics() {
        let _ = register_topic(
            "",
            "Org files: headlines, folding, and what this plugin does not do.",
            include_str!("../doc/org.md"),
            // `:describe-command` cross-links from any command whose name
            // contains these.
            &["fold".to_string()],
        );
        let _ = register_topic(
            "roam",
            "Org-roam: id-addressed notes, backlinks, dailies and templates.",
            include_str!("../doc/roam.md"),
            // Every roam command is `org-roam-…`, so one pattern reaches all
            // of them and nothing else.
            &["roam".to_string()],
        );
    }

    // ── OM.A1 / OM.A2: the agenda seam ──────────────────────────────────
    //
    // The host walks files and builds excerpts; everything org about the
    // agenda lives in `agenda.rs`. See its module docs for what counts as a
    // row and why.

    /// The host offers this plugin `.org` files and no others.
    ///
    /// **Deliberately NOT the pair `register_languages` claims**, and this
    /// used to be that pair on the reasoning that the two lists were the same.
    /// They are not the same question. `register_languages` answers "which
    /// files are org-mode", and an `.org_archive` is org-mode — it should
    /// highlight and fold like any other. This answers "which files does the
    /// AGENDA scan", and archiving exists precisely to take an entry out of
    /// the agenda. Scanning archives puts every task you ever finished back
    /// into the view you archived it to escape.
    ///
    /// Emacs draws the same line: `org-agenda-files` never includes archives,
    /// and `org-agenda-archives-mode` (`v a`) is an explicit, default-off
    /// toggle for the times you want them.
    fn extensions() -> Vec<String> {
        vec!["org".to_string()]
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
        agenda_files()
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
    ///
    /// AS.1 adds *the section set*, for the same reason and more sharply: a
    /// section's rank is packed into every row's sort key, so a set that
    /// changed mid-scan would file file A's rows under section 2 and file B's
    /// identical rows under section 3, and the host — which sorts on that
    /// number and knows nothing else — would interleave them into a view with
    /// no coherent reading at all.
    ///
    /// OA.11a adds *the scan's arguments* — what this particular scan was
    /// opened for. The host passes them through without reading them, so they
    /// are org's own vocabulary (the agenda dispatcher names which custom
    /// command to run). They are captured here with everything else that must
    /// hold still for one scan, because `begin` is the one call guaranteed to
    /// precede `roots` and every `scan`.
    /// OA.22: what this view is, for its headerline.
    ///
    /// Re-parses rather than reading what `begin` stashed. The two calls are
    /// adjacent and parsing is cheap, and a `describe` that depended on `begin`
    /// having run would answer for the PREVIOUS scan if the host ever reordered
    /// them — a header naming a filter that is no longer on is worse than none.
    fn describe(args: Vec<String>) -> String {
        let view = agenda_args::ViewArgs::parse(&args);
        let default_span = option_or("agenda-span", DEFAULT_AGENDA_SPAN)
            .trim()
            .parse::<u32>()
            .unwrap_or_else(|_| {
                DEFAULT_AGENDA_SPAN
                    .parse()
                    .expect("the compiled-in default parses")
            });
        // The SAME anchor the scan uses — `today + offset * span` — because a
        // header that computed the window differently would eventually
        // disagree with the rows under it.
        let span = view.span.unwrap_or(default_span);
        let anchor = today_epoch_day() + i64::from(view.offset) * i64::from(span.max(1));
        agenda_args::describe(&view, default_span, anchor)
    }

    fn begin(args: Vec<String>) -> u64 {
        // OA.19: the view's own arguments, parsed once. A bare token is still
        // the custom-command key, so every existing caller is unaffected.
        //
        // `view.problems` is deliberately NOT logged here. A guest
        // `logging::log` call makes the component IMPORT `logging`, which
        // org's multi-seam linker does not wire — the whole component then
        // fails to instantiate, which has cost this plugin a debug cycle more
        // than once. The problems ride to OA.22's headerline instead, which is
        // where a user would look for "why is my agenda not what I asked for"
        // anyway.
        let view = agenda_args::ViewArgs::parse(&args);
        let keywords = agenda::Keywords::from_spec(&todo_keyword_lines().join("\n"));
        // OA.20: `org.agenda-span` is what a FRESH agenda opens at; a view the
        // user has walked carries its own. Navigation must never rewrite the
        // option — glancing at next week would otherwise change what every
        // future agenda means.
        let span = view.span.unwrap_or_else(|| {
            option_or("agenda-span", DEFAULT_AGENDA_SPAN)
                .trim()
                .parse::<u32>()
                .unwrap_or_else(|_| {
                    DEFAULT_AGENDA_SPAN
                        .parse()
                        .expect("the compiled-in default parses")
                })
        });
        // The day the scan is anchored to. Every label is relative to it
        // ("tomorrow", "overdue by 2 day(s)"), so shifting it IS what walking a
        // span means — the view keeps its shape and moves.
        //
        // `max(1)` because the daily agenda is `span = 0`: a step of zero days
        // would make `f` a no-op on exactly the view people walk most.
        let today = today_epoch_day() + i64::from(view.offset) * i64::from(span.max(1));
        // AS.2: the user's set if `org.agenda-sections` holds one, the
        // built-ins otherwise. Parsed on read and never cached, the
        // `capture-templates` precedent — `:set org.agenda-sections=…` must
        // land on the next scan, and a cache would need an `OptionChanged`
        // subscription to stay honest.
        let sections = agenda_sections::resolve(span);
        // OA.11: …unless the view was opened for a NAMED agenda, in which case
        // that command's sections are the ones this scan runs. `args` is what
        // OA.11a carries from the view; empty is the default agenda, which is
        // the overwhelmingly common case and stays exactly as it was.
        //
        // The default set is computed either way and handed in as the
        // fallback, rather than being resolved lazily inside the branch: every
        // way of failing to find a command — unset option, broken TOML, a key
        // naming nothing — has to land on a WORKING agenda, and threading one
        // fallback through one call is what makes that structural instead of
        // three branches that each have to remember.
        let sections = agenda_custom_commands::resolve(&view.command_args(), span, sections);
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
        //
        // AS.1: the section set joins them, and it must — the sections decide
        // which rows exist and under which header, so a cached scan from
        // before a `:set org.agenda-span` would be answering the old question.
        let generation = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            today.hash(&mut h);
            keywords.all.hash(&mut h);
            keywords.done.hash(&mut h);
            for s in &sections {
                s.title.hash(&mut h);
                s.filter.hash(&mut h);
            }
            // OA.11a: the args join them, and for the sharpest version of the
            // same reason. Two custom commands ask two different questions of
            // the same files; a scan that did not key on which one it was
            // answering would serve the first command's cached rows under the
            // second command's name, and every one of them would look
            // plausible.
            args.hash(&mut h);
            h.finish()
        };
        // OA.15: log mode's window, resolved here beside `today` because it is
        // derived from it — a scan crossing midnight must file every file's
        // log against ONE window, which is the whole reason `begin` exists.
        let log = view
            .log
            .map(|items| (items, agenda_args::ViewArgs::log_days(span, today)));
        // OA.20: what the next span/filter chord modifies. The agenda is
        // `reuse: true`, so there is one view and one slot is the accurate
        // model; a chord reads this, changes one argument and re-opens.
        VIEW_ARGS.replace(view);
        SCAN.set(Some(ScanState {
            today,
            keywords,
            sections,
            log,
            nerd_fonts: matches!(
                get_option("ui.nerd_fonts").as_deref(),
                Some("true") | Some("on")
            ),
            stats: option_or("habit-stats", "true").eq_ignore_ascii_case("true"),
        }));
        generation
    }

    fn scan(
        path: String,
        text: String,
        tree: Option<&TreeSnapshot>,
    ) -> Result<lattice::plugin_host::scanned_excerpt_source::ScanResult, String> {
        // OA.21: a `file:` filter is answerable here and nowhere else — this
        // is the only place that knows which file the rows came from. Answered
        // FIRST because it is the cheapest possible rejection: a filtered-out
        // file is not parsed, not walked and not clocked.
        let view = VIEW_ARGS.with_borrow(Clone::clone);
        if !view.admits_file(&path) {
            return Ok(lattice::plugin_host::scanned_excerpt_source::ScanResult {
                entries: Vec::new(),
                clock: Vec::new(),
            });
        }
        // OA.14b: the clock spans are computed from the TEXT and are
        // independent of the sections — a headline that no block admits still
        // logged its time, and a report that only totalled admitted rows would
        // under-report with nothing to show for it.
        //
        // Computed before the row walk so an early `Err` on the rows cannot
        // silently take the clock report with it… except that it does: an
        // `Err` skips the whole file, rows and clock alike. That is the right
        // coupling — a file org could not read is a file it cannot honestly
        // report time from either — and it is stated here because the two
        // halves are otherwise independent enough to look separable.
        let clock: Vec<lattice::plugin_host::scanned_excerpt_source::ClockSpan> =
            clock_scan::scan(&text)
                .into_iter()
                .map(
                    |s| lattice::plugin_host::scanned_excerpt_source::ClockSpan {
                        line: s.line,
                        outline: s.outline,
                        day: s.day,
                        minutes: s.minutes,
                    },
                )
                .collect();
        // `begin` is contractually called first. Refusing rather than
        // defaulting makes a host that stops calling it fail loudly on the
        // first file instead of producing a silently mis-dated agenda.
        let entries = SCAN.with_borrow(|state| {
            let Some(state) = state.as_ref() else {
                return Err("org: scan before begin".to_string());
            };
            // OT.3: structure from the tree when the host had a grammar for
            // this file, characters from the text either way. The text path is
            // the fallback for a host with no org grammar loaded, and it still
            // carries the "planning line is the next line" assumption — which
            // is the bug the tree path exists to fix, so it is a fallback and
            // not a peer.
            // Whether any section this scan runs is a TAGS SEARCH. When none
            // is, both walks produce exactly the rows they always produced —
            // the extra ones would be unreachable, and a corpus of them is not
            // free to build or to carry back across the seam.
            // OA.15 widens the same switch: a tag filter has to reach LOG rows
            // too, and a log row's headline is usually DONE — which is not a
            // row at all unless this walk is admitting tag-only ones. A filter
            // that silently stopped applying to half the view would be the
            // worse failure by some distance.
            let filtering_log = state.log.is_some() && view.is_filtered();
            let admit_tag_only =
                filtering_log || state.sections.iter().any(|s| s.filter.r#match.is_some());
            let mut rows = match tree {
                Some(snapshot) => {
                    agenda::scan_tree(&snapshot.root(), &text, &state.keywords, admit_tag_only)
                }
                None => agenda::scan_file(&text, &state.keywords, admit_tag_only),
            };
            // …captured BEFORE the retain below, because the map has to answer
            // for every headline in the file rather than for the ones that
            // survived. Tags here are the walk's own, inheritance included, so
            // a log row obeys `-CANCELLED` for the reason every other row does.
            let tags_by_line: Vec<(u32, Vec<String>)> = if filtering_log {
                rows.iter().map(|r| (r.line, r.tags.clone())).collect()
            } else {
                Vec::new()
            };
            // OA.21: a `tag:` filter narrows every block at once, which is
            // what makes it a FILTER rather than another section. Applied to
            // the rows rather than folded into each section's `match` because
            // the two compose differently: a section's match says what that
            // block is FOR, the filter says what you are looking at right now,
            // and `r` then `/work` has to mean refile AND work rather than one
            // of them silently winning.
            if view.is_filtered() {
                rows.retain(|row| view.admits_tags(&row.tags));
            }
            // OA.6 reads a row's own line to colour it; both scan paths report
            // 0-based line numbers into this same split.
            let lines: Vec<&str> = text.lines().collect();
            // AS.1: one row, zero or more entries. A row is a candidate and
            // each section decides whether it wants it, so an overdue `[#A]`
            // TODO is emitted three times and a headline nothing wants is
            // emitted none. `flat_map` rather than `map` is the entire shape
            // change at this seam — the ABI did not move.
            let mut entries: Vec<Entry> = rows
                .into_iter()
                .flat_map(|row| {
                    // OA.6: computed ONCE per row. `entries_for_row` fans a
                    // row out across every section that admits it — an
                    // overdue `[#A]` lands in three — and they all render the
                    // same source line, so recomputing per entry would parse
                    // the same headline three times for identical spans.
                    let spans: Vec<crate::lattice::plugin_host::types::DisplaySpan> = lines
                        .get(row.line as usize)
                        .map(|line| {
                            agenda::headline_spans(line, &state.keywords.all)
                                .into_iter()
                                .map(|(start, end, slot)| {
                                    crate::lattice::plugin_host::types::DisplaySpan {
                                        start,
                                        end,
                                        slot,
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    // HB.5c: and the consistency graph, computed ONCE per row
                    // for `spans`' reason — a row fanned across three sections
                    // renders the same graph in each, and rebuilding it would
                    // re-read the same subtree three times.
                    //
                    // `None` for every row that is not a `:STYLE: habit`
                    // headline, which is nearly all of them: the check is a
                    // properties-drawer scan over a subtree the walk has
                    // already reached.
                    let annotation = habit_row::annotation_for(
                        &lines,
                        row.line,
                        repeat::from_days(state.today),
                        &state.keywords.done,
                        state.nerd_fonts,
                        state.stats,
                    );
                    agenda::entries_for_row(&row, &state.sections, &state.keywords, state.today)
                        .into_iter()
                        .map(move |(group, label, sort_key)| Entry {
                            line: row.line,
                            end_line: row.end_line,
                            group,
                            label,
                            sort_key,
                            spans: spans.clone(),
                            annotation: annotation.clone().map(|a| {
                                lattice::plugin_host::scanned_excerpt_source::Annotation {
                                    text: a.text,
                                    spans: a
                                        .spans
                                        .into_iter()
                                        .map(|(start, end, slot)| {
                                            crate::lattice::plugin_host::types::DisplaySpan {
                                                start,
                                                end,
                                                slot,
                                            }
                                        })
                                        .collect(),
                                }
                            }),
                        })
                })
                .collect();

            // OA.15: and the log's own rows, when the view asked for them.
            //
            // Appended rather than fanned through `entries_for_row`, because a
            // log row is not a candidate any SECTION could want: the sections
            // are a user's configuration of what their plan looks like, and a
            // record of the past has no business landing in "Overdue" because
            // that block happens to match. It gets one block of its own,
            // ranked after every section — the plan comes first and the record
            // reads as the answer to a different question.
            if let Some((items, days)) = &state.log {
                let rank = state.sections.len() as i64;
                for event in agenda_log::scan(&text, *items, days) {
                    // The tag filter reaches log rows too. An untagged
                    // headline is absent from the map and is therefore
                    // refused, which is correct: it can satisfy no `tag:`
                    // term.
                    if filtering_log
                        && !tags_by_line
                            .iter()
                            .find(|(line, _)| *line == event.line)
                            .is_some_and(|(_, tags)| view.admits_tags(tags))
                    {
                        continue;
                    }
                    let (text, spans) = event.annotation();
                    entries.push(Entry {
                        // The HEADLINE, not the `CLOCK:` or LOGBOOK line the
                        // event was read from: the row is the thing you want
                        // to read and `<CR>` is the thing you want it to do,
                        // and neither wants the interior of a drawer.
                        line: event.line,
                        end_line: event.line,
                        group: format!("{rank}:{}", agenda::group_key(event.day)),
                        label: format!(
                            "Log \u{2014} {}",
                            agenda::group_label(event.day, state.today)
                        ),
                        sort_key: agenda::log_sort_key(rank, event.day, event.within_day()),
                        // The headline's own colouring, exactly as a plan row
                        // gets it — a log row IS a headline, and painting it
                        // differently would make one task look like two.
                        spans: lines
                            .get(event.line as usize)
                            .map(|line| {
                                agenda::headline_spans(line, &state.keywords.all)
                                    .into_iter()
                                    .map(|(start, end, slot)| {
                                        crate::lattice::plugin_host::types::DisplaySpan {
                                            start,
                                            end,
                                            slot,
                                        }
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        // What happened, under the row. An excerpt is verbatim
                        // source and there is no line in the file that says
                        // "Closed 14:32" — HB.5's annotation is the one place
                        // a scan source may draw content of its own.
                        annotation: Some(
                            lattice::plugin_host::scanned_excerpt_source::Annotation {
                                text,
                                spans: spans
                                    .into_iter()
                                    .map(|(start, end, slot)| {
                                        crate::lattice::plugin_host::types::DisplaySpan {
                                            start,
                                            end,
                                            slot,
                                        }
                                    })
                                    .collect(),
                            },
                        ),
                    });
                }
            }
            Ok(entries)
        })?;
        Ok(lattice::plugin_host::scanned_excerpt_source::ScanResult { entries, clock })
    }
}

/// Per-scan state, captured in `begin`.
struct ScanState {
    today: i64,
    keywords: agenda::Keywords,
    /// HB.5c: which glyph palette the consistency graph draws in.
    ///
    /// Read ONCE per scan rather than per row: it is a host call, it cannot
    /// change mid-scan, and a scan of a large corpus would otherwise make one
    /// per habit for an answer that is the same every time.
    nerd_fonts: bool,
    /// HB.6: whether the graph carries its derived stats. Read once per scan
    /// for `nerd_fonts`' reason.
    stats: bool,
    /// AS.1: the blocks this scan files rows into, resolved once so every
    /// file of one scan agrees on both the set and each section's RANK — the
    /// rank is packed into the sort key the host orders on.
    sections: Vec<agenda::Section>,
    /// OA.15: log mode's item set and the days it covers, or `None` when the
    /// mode is off — which costs the walk nothing, because `agenda_log::scan`
    /// returns immediately on an empty item set.
    ///
    /// The RANGE is resolved in `begin` beside `today` and for the same
    /// reason: a scan crossing midnight must file every file's log against one
    /// window, or two files scanned a second apart would disagree about what
    /// "today" was.
    log: Option<(agenda_log::LogItems, std::ops::RangeInclusive<i64>)>,
}

thread_local! {
    /// OA.21: whether the filter prompt now open REPLACES the tag filter or
    /// narrows it.
    ///
    /// `/` and `\` open the same prompt and submit through the same action —
    /// the host's prompt seam carries text back, not which key opened it — so
    /// the distinction has to be remembered here. One slot, because one prompt
    /// is open at a time.
    static FILTER_REPLACES: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

thread_local! {
    /// OA.20: the arguments the agenda view on screen was opened with.
    ///
    /// A span or filter chord is "re-open this view with one argument
    /// different", so it has to know what the others were. The agenda is
    /// `reuse: true` — one view, re-scanned in place — so a single slot is the
    /// accurate model rather than a convenient one.
    ///
    /// Written by `begin`, which is the only thing that knows a scan is
    /// starting and with what.
    static VIEW_ARGS: std::cell::RefCell<agenda_args::ViewArgs> =
        const { std::cell::RefCell::new(agenda_args::ViewArgs::new()) };
}

thread_local! {
    /// OC.7b: the capture the buffer on screen belongs to.
    ///
    /// `C-c C-c` must know WHERE to file what was typed, and the action context
    /// carries `buffer-id` and `cursor` but no buffer NAME — and
    /// `document.path()` is `none` for a synthetic buffer, by definition. So
    /// the target cannot be recovered from the buffer; it is remembered when
    /// the buffer is opened.
    ///
    /// A `thread_local` is the whole synchronisation story for the same reason
    /// the agenda's `SCAN` is: single-threaded guest, one actor, calls
    /// serialised by the host's per-plugin channel.
    ///
    /// **One capture in flight at a time**, which is a real limit and not an
    /// oversight. The buffer name is derived from the template key, so two
    /// captures of the same template would collide on the buffer anyway; and
    /// emacs's default is likewise one (`org-capture` in progress refuses a
    /// second). Cleared on finalize and on abort, so an abandoned capture
    /// cannot mis-file the next one.
    static PENDING_CAPTURE: std::cell::RefCell<Option<CaptureDestination>> =
        const { std::cell::RefCell::new(None) };
}

/// OC.7b / OR.11b: what `C-c C-c` needs that the buffer cannot tell it.
///
/// **The destination, not the template that produced it.** This held a whole
/// `capture_templates::Template` until OR.11b, and `capture_finalize` used it
/// for exactly one call — `capture_effects`, which reads `target` and
/// `clock_in` and nothing else. Narrowing the state to what the consumer
/// actually reads is what lets org-capture and org-roam share one buffer
/// surface: a roam create has no capture template, no key and no `:clock-in`.
/// What it has is a file to append to, which is precisely a `Target::File`.
///
/// The alternative — roam synthesising a `Template` with dead `key`,
/// `description` and `clock_in` fields to satisfy this — is a struct built to
/// fit a consumer rather than to describe anything, and it would leave the
/// shared state coupled to a type roam has no business constructing.
#[derive(Clone)]
struct CaptureDestination {
    /// Where the finalized text lands: a file to append to, or a file and the
    /// headline whose subtree it goes after.
    target: capture_templates::Target,
    /// OC.11 — org's `:clock-in`. Always false for roam, which has no
    /// equivalent: a new note is not an entry you are working on.
    clock_in: bool,
}

impl CaptureDestination {
    /// Where an org-capture template files.
    fn of(template: &capture_templates::Template) -> Self {
        Self {
            target: template.target.clone(),
            clock_in: template.clock_in,
        }
    }

    /// Append to one file — roam's whole destination, and the shape of every
    /// capture target that names no headline.
    fn appending_to(path: String) -> Self {
        Self {
            target: capture_templates::Target::File { file: path },
            clock_in: false,
        }
    }
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
/// LOCAL, not UTC — see [`local_now_secs`]. This divided the raw UTC seconds
/// until 2026-09-05, which anchored the agenda to the UTC day.
fn today_epoch_day() -> i64 {
    epoch_day_from_local_secs(local_now_secs())
}

/// The epoch DAY a local wall-clock instant falls in.
///
/// `div_euclid`, not `/`: the local instant is UTC plus a SIGNED offset, and
/// truncating division rounds toward zero, which is the wrong direction below
/// the epoch. Pure, so the midnight boundary is testable without a clock.
fn epoch_day_from_local_secs(local_secs: i64) -> i64 {
    local_secs.div_euclid(86_400)
}

/// Now, in LOCAL wall-clock seconds since the epoch.
///
/// **The single place this plugin asks what time it is.** `wasi:clocks` is UTC
/// and a component carries no `TZ`, so the offset comes from the host
/// (`local-utc-offset-seconds`, host slice OC.4).
///
/// This exists because the correction was open-coded four times and the fourth
/// copy did not have it: `today_epoch_day` divided raw UTC seconds, so the
/// agenda anchored to the UTC day. East of Greenwich that is YESTERDAY until
/// local morning catches up with UTC — reported at GMT+5:30 as the agenda
/// opening on the 4th on the 5th — and west of it, tomorrow after the evening.
/// The three correct siblings each carried a comment warning about exactly
/// this bug while the fourth quietly had it, which is the argument for one
/// function over four careful ones.
fn local_now_secs() -> i64 {
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    utc + i64::from(host_services::local_utc_offset_seconds())
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
    /// The buffer remains the durable record: after a restart `<leader>oxj` says it
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
    clock::Now::from_local_secs(local_now_secs())
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

/// `<leader>oxo` (close it) and `<leader>oxq` (discard it).
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
/// it is elsewhere the entry is reached with `read-file` for the characters and
/// ONE `WriteToFile` — but NOT `parse-file`, which this seam cannot afford; see
/// the note at the call site. `clock::Logbook` needs neither a buffer nor a
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
    // OC.9 fix: the TEXT scan, not a tree — this seam is synchronous.
    //
    // `apply-ex-command` runs on the grammar trampoline, which is the dispatch
    // thread, and `parse-file` reads the file and runs tree-sitter over it
    // host-side while the guest blocks. The epoch deadline is wall-clock, so it
    // fires, the guest traps, and the plugin is QUARANTINED — `:org-clock-resume`
    // did nothing at all, with no effect and no echo, which reads as a dead
    // command rather than a budget it blew.
    //
    // The three other `parse-file` callers (the scan seam, the roam index, the
    // find-node picker) are all async actors, where a parse is the right trade.
    // This one was the odd one out and always was.
    //
    // What it costs: `Headlines` falls back to matching `^\*+ `, so a headline
    // inside a `#+BEGIN_SRC` block would be taken for a real one. Not reachable
    // here — the target is a line the user themselves clocked into from a live
    // buffer, so a headline exists at it by construction. That is a much
    // narrower exposure than the agenda's, which is why the agenda keeps its
    // tree and this does not.
    let hl = headline::Headlines::new(None, &accessor, count);
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

/// `<leader>oxj` — jump to the entry the running clock is on.
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

/// OR.4b — the roam scan's progress, in the modeline.
///
/// Its own segment rather than sharing the clock's: the two are unrelated and
/// can be live at once, and a scan that overwrote a running clock would look
/// like the clock had stopped.
const ROAM_SEGMENT: &str = "roam-scan";

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
/// OR.4b — queue a cold scan and ring the first step.
///
/// Split from `roam_scan::begin` because the doorbell is the HOST's mechanism
/// and the queue is the guest's: `begin` walks and parks the file list, this
/// starts the chain that drains it. A scan with nothing queued rings nothing,
/// so an unconfigured roam costs one option read.
fn arm_roam_scan() {
    if roam_scan::begin() > 0 {
        host_services::emit_event(
            EV_ROAM_SCAN_STEP,
            roam_scan::generation().to_string().as_bytes(),
        );
    }
}

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
        // The archive file sits BESIDE the source (`<this file>_archive`),
        // so its directory is the one the buffer is already open in.
        create_parents: false,
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
    let set = match capture_templates::read() {
        Ok(set) => Some(set),
        // "Unset" is not a failure — it is the OM.11 fallback path below.
        Err(capture_templates::TemplateError::Unset) => None,
        Err(e) => {
            return Err(Effect::Echo(EchoPayload {
                level: EchoLevel::Warn,
                text: e.message(),
            }));
        }
    };
    let Some(set) = set else {
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
    };

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

/// The first hop of `<leader>oc`: resolve the template, then open the prompt.
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

    // OC.7b: the BUFFER, not a one-line prompt. Before this the template was
    // never shown — it was expanded only at submit — so the user typed into an
    // empty minibuffer and found out what the template did afterwards.
    open_capture_buffer(
        capture_buffer_name(&template.key),
        CaptureDestination::of(&template),
        &template.body,
        &[],
        "",
        &capture_origin(),
    )
}

/// OC.7b: open the capture BUFFER — the surface emacs has and the prompt was
/// standing in for.
///
/// Shared by both entry paths on purpose. A template with `%^{…}` questions
/// still collects them in the transient first (emacs's order: prompts, then
/// the buffer), and a template without them comes straight here; either way
/// what appears is the same buffer with the same chords, so there is one
/// capture surface rather than two that can drift.
///
/// The major is `org-mode`, because a capture buffer IS an org buffer — it
/// wants org's grammar, motions and folding. Only the finalize/abort pair is
/// capture-specific, so it rides `org-capture-mode` as a MINOR; putting those
/// chords on the major would make `C-c C-c` file every org file you touched.
///
/// OR.11b widened this from two entry paths to four — org-capture's direct and
/// fields routes, and org-roam's two peers. It takes a `body` and a `name`
/// rather than a template because roam has neither a `capture_templates::
/// Template` nor a key to derive a name from, and because the body it hands
/// over has already had `${…}` expanded over the node being created.
///
/// `annotation` is `%a`'s expansion, passed in rather than read from
/// [`CAPTURE_ORIGIN`] here. That is not tidiness: the origin is written by
/// `capture_open` and cleared by `taken_origin`, so reading it here would let
/// a roam note inherit the `%a` of whatever org-capture ran before it — a link
/// back to a file the user was not in when they made the note. Roam passes
/// `""`, which is what `%a` correctly means for a create that has no origin
/// buffer.
fn open_capture_buffer(
    name: String,
    dest: CaptureDestination,
    body: &str,
    answers: &[String],
    entered: &str,
    annotation: &str,
) -> Vec<Effect> {
    let (text, point) =
        capture::expand_for_buffer(body, entered, answers, today_epoch_day(), annotation);
    // Remembered BEFORE the effect is returned: the action context carries a
    // buffer id and a cursor but no buffer NAME, and a synthetic buffer's
    // `document.path()` is `none`, so `C-c C-c` could not otherwise work out
    // where to file what it is looking at.
    PENDING_CAPTURE.with(|c| *c.borrow_mut() = Some(dest));
    vec![Effect::OpenSyntheticBuffer(
        lattice::plugin_host::types::OpenSyntheticBufferPayload {
            name,
            mode_id: "org-mode".to_string(),
            content: Some(text),
            cursor: point.map(|(line, byte)| lattice::plugin_host::types::Position { line, byte }),
            activate_minor: Some("org-capture-mode".to_string()),
        },
    )]
}

/// The capture buffer's name. Keyed by template so two templates do not fight
/// over one buffer, and stable so re-firing the same template returns to the
/// note in progress rather than starting a second.
fn capture_buffer_name(key: &str) -> String {
    if key.is_empty() {
        "*org-capture*".to_string()
    } else {
        format!("*org-capture:{key}*")
    }
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

/// The origin without consuming it — what the BUFFER path expands `%a` to.
///
/// Peeking rather than taking, unlike [`taken_origin`], because the buffer path
/// expands `%a` when the buffer OPENS and the capture is not over until
/// `C-c C-c`: a fields menu that opens the buffer after collecting answers
/// would otherwise have consumed the origin on the way through, and the same
/// capture would lose its own `%a`. The origin is overwritten unconditionally
/// by the next `capture_open`, so nothing leaks forward to a capture that has
/// one of its own — and org-roam does not read this at all, passing `""`.
fn capture_origin() -> String {
    CAPTURE_ORIGIN
        .with(|c| c.borrow().clone())
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

fn capture_effects(dest: &CaptureDestination, text: String) -> Vec<Effect> {
    let path = dest.target.file().to_string();
    let headline = match &dest.target {
        capture_templates::Target::File { .. } => None,
        capture_templates::Target::FileHeadline { headline, .. } => Some(headline.clone()),
    };
    let Some(headline) = headline else {
        // Appended at the end, so the entry starts at the file's current line
        // count. Read only when a clock is actually wanted — an ordinary capture
        // must not pay for it.
        let text = if dest.clock_in {
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
        if dest.clock_in {
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
        // A capture target is a path the USER typed into a template, which is
        // exactly the case the host's refusal exists for: a typo must not
        // silently build a tree.
        create_parents: false,
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

    // OC.7b: the menu still collects the `%^{…}` answers first — emacs's
    // order, prompts then buffer — but what it opens now is the capture
    // buffer rather than a write. The body row it collected seeds the `%?`
    // point and the caret lands after it, so the answer is a draft the user
    // can keep editing rather than the final word.
    open_capture_buffer(
        capture_buffer_name(&template.key),
        CaptureDestination::of(&template),
        &template.body,
        &answers,
        &entered,
        &capture_origin(),
    )
}

/// OC.7b — `C-c C-c`: file what the capture buffer holds.
///
/// The buffer's WHOLE text is the entry. Not the template re-expanded, and not
/// what a prompt collected: the point of the buffer surface is that the user
/// may have rewritten any of it, so the only honest source is what is on
/// screen when they finalize.
fn capture_finalize(doc: &Document) -> Vec<Effect> {
    let Some(pending) = PENDING_CAPTURE.with(|c| c.borrow().clone()) else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org: no capture in progress".to_string(),
        })];
    };
    let text = document_text(doc);
    // Cleared BEFORE the write is even queued. If the write fails the capture
    // is still over — leaving it pending would make the next `C-c C-c` in an
    // unrelated buffer file this one's text.
    PENDING_CAPTURE.with(|c| *c.borrow_mut() = None);
    if text.trim().is_empty() {
        return vec![
            Effect::BufferDelete(true),
            Effect::Echo(EchoPayload {
                level: EchoLevel::Info,
                text: "org: nothing captured".to_string(),
            }),
        ];
    }
    let mut effects = capture_effects(&pending, text);
    // AFTER the write, so a failed write leaves the buffer on screen with the
    // text still in it rather than closing over the top of it.
    effects.push(Effect::BufferDelete(true));
    effects
}

/// OC.7b — `C-c C-k`: throw the capture away.
///
/// **Nothing is created, and that falls out rather than being cleaned up.**
/// The file is written on finalize, so an abort has nothing to undo — the
/// property OR.6's `WriteToFile`-on-create does not have, and the reason the
/// buffer surface is worth the ABI it took.
fn capture_abort() -> Vec<Effect> {
    let was_pending = PENDING_CAPTURE.with(|c| c.borrow_mut().take()).is_some();
    if !was_pending {
        return vec![Effect::Declined];
    }
    vec![
        Effect::BufferDelete(true),
        Effect::Echo(EchoPayload {
            level: EchoLevel::Info,
            text: "org: capture aborted".to_string(),
        }),
    ]
}

/// Every line of `doc`, rejoined, with trailing blank lines trimmed to one
/// newline.
///
/// The `document` resource deliberately has no "give me everything" call —
/// bulk text is not supposed to cross casually — but a capture buffer IS the
/// payload, and it is one short entry.
///
/// The trim is not tidiness. A template with no `%?` gets its caret placed on
/// a fresh line at the end (`expand_for_buffer`), so filing verbatim would
/// write that blank line into the user's org file, and every capture through
/// such a template would leave one behind. Leading structure is untouched —
/// only the tail, and only down to a single terminating newline.
fn document_text(doc: &Document) -> String {
    let n = doc.line_count();
    let mut out = String::new();
    for i in 0..n {
        if let Some(l) = doc.line(i) {
            out.push_str(&l);
        }
        out.push('\n');
    }
    let trimmed = out.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return String::new();
    }
    format!("{trimmed}\n")
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
    capture_effects(&CaptureDestination::of(&template), text)
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
        // A refile target is an existing headline in an existing file —
        // the picker only offers what it walked.
        create_parents: false,
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
/// HB.2 — cycle the keyword, routing a landing on a done state through the
/// repeat path.
///
/// `cycle_keyword` answers a LINE, not a keyword, so the target is read back
/// out of the line it produced. That is deliberate rather than lazy: the
/// cycling rules (sequence order, wrap, the `|` split) live in one place, and
/// re-deriving "what does NEXT cycle to" here would be a second copy to drift.
fn cycle_keyword_repeating(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    forward: bool,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    let Some((start, _)) = hl.enclosing(ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let keywords = todo_keywords();
    let Some(head) = doc.line(start) else {
        return vec![Effect::None];
    };
    let Some(cycled) = todo::cycle_keyword(&head, &keywords, forward) else {
        return vec![Effect::None];
    };
    let want = todo::parse(&cycled, &keywords)
        .and_then(|h| h.keyword.map(|k| k.to_string()))
        .unwrap_or_default();
    set_keyword_repeating(ctx, doc, tree, &want)
}

/// HB.2 — set the headline's keyword, repeating the task when the keyword is
/// a done state and the task carries a repeater.
///
/// The gate is deliberately narrow: `complete_repeating` answers `None` for
/// anything that does not repeat, and then this is exactly the old
/// single-line rewrite. So a plain TODO is untouched by this slice, which is
/// what keeps a change to *every* completion from riding in on a habit fix.
///
/// **One edit over the whole subtree**, not three. A completion that shifted
/// the stamp but did not log, or logged but did not reset, is a worse state
/// than either end — and `u` has to take the completion back as one action,
/// not unpick it a line at a time.
fn set_keyword_repeating(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    want: &str,
) -> Vec<Effect> {
    let keywords = todo_keywords();
    let (_, done) = todo::parse_todo_keywords(&todo_keyword_lines().join("\n")).split();
    // Only a DONE state repeats. Cycling TODO → NEXT must not shift a
    // timestamp, which is the bug an over-eager gate here would be.
    if !done.iter().any(|d| d == want) {
        return set_keyword_logged(ctx, doc, tree, want);
    }
    // HB.2b: through the target, not through `doc`.
    //
    // An agenda excerpt is ONE line — the headline — so reading the subtree
    // from the active buffer there yields a single line with no planning line
    // under it, `complete_repeating` correctly finds nothing to repeat, and
    // the fallback below writes a plain `DONE`. That is not a failure to
    // repeat: it *destroys the habit*, because the headline stops being
    // scheduled and the completion is never recorded. The whole subtree has
    // to be read from, and written back to, the document that actually holds
    // it — which for an agenda row is a multibuffer source the view owns and
    // no buffer store holds.
    let Some(target) = PlanTarget::resolve(ctx, doc, tree) else {
        return vec![Effect::None];
    };
    let Some(head) = target.line(target.headline, doc) else {
        return vec![Effect::None];
    };
    let from_keyword = todo::parse(&head, &keywords)
        .and_then(|h| h.keyword.map(|k| k.to_string()))
        .unwrap_or_default();
    let end = target.subtree_end(doc, tree);
    let lines: Vec<String> = (target.headline..=end)
        .filter_map(|n| target.line(n, doc))
        .collect();

    let now = clock_now();
    let cx = complete::Context {
        today: today_local(),
        now_stamp: &now.stamp(),
        keywords: &keywords,
        done_keywords: &done,
        log_into_drawer: option_or("log-into-drawer", "true").eq_ignore_ascii_case("true"),
    };
    let Some(done_with) = complete::complete_repeating(&lines, &from_keyword, &cx) else {
        // Not a repeating task: the ordinary completion, which still logs what
        // the keyword's flags ask for (TK.8).
        return set_keyword_logged(ctx, doc, tree, want);
    };

    let last_len = target.line(end, doc).map(|l| l.len()).unwrap_or(0) as u32;
    let cursor = if target.from_source {
        // The edit lands in the source; the caret is in the VIEW and stays
        // where it is. Naming a source line here would move the caret to a
        // composed row that happens to share the number — the two coordinate
        // spaces are unrelated, which is the whole reason `from_source`
        // exists. (`plan_submit` passes `ctx.cursor` for the same reason.)
        ctx.cursor
    } else {
        Position {
            line: target.headline,
            byte: ctx
                .cursor
                .byte
                .min(done_with.lines.first().map(|l| l.len()).unwrap_or(0) as u32),
        }
    };
    let mut effects = replace_lines_at(
        target.buffer,
        target.headline,
        end,
        last_len,
        done_with.lines.join("\n"),
        cursor,
    );
    // Say what happened. A keyword that goes back to NEXT after you pressed
    // DONE looks like the key failed; without this line the correct behaviour
    // is indistinguishable from a bug, which is how org's own newcomers meet
    // repeaters.
    let d = done_with.next;
    effects.push(Effect::Echo(EchoPayload {
        level: EchoLevel::Info,
        text: format!(
            "org: repeated — {} again, next {:04}-{:02}-{:02}",
            done_with.reset_to, d.year, d.month, d.day
        ),
    }));
    effects
}

thread_local! {
    /// TK.9: the transition a note is being collected for.
    ///
    /// The submit arrives as a fresh context with only the typed text, and by
    /// then the headline already says the NEW state — so "which state did this
    /// come from" is no longer readable from the buffer. It has to be carried,
    /// and a `thread_local` is the whole of the synchronisation story here: one
    /// guest, one actor, calls serialised by the host's per-plugin channel.
    ///
    /// Cleared on submit and overwritten on each new prompt, so a dismissed
    /// prompt cannot make the NEXT note claim the wrong transition.
    static PENDING_NOTE: std::cell::RefCell<Option<(String, String)>> =
        const { std::cell::RefCell::new(None) };
}

/// TK.9 — ask for the note a `(@)` state declared, having already changed it.
///
/// Org's order, and the reason for it: `org-todo` writes the new keyword and
/// then calls `org-add-log-setup`, so dismissing the note leaves the state
/// changed and unnoted. Holding the change until the note arrived would make a
/// keyword appear not to respond, which is the more surprising of the two.
fn note_prompt_for(from: &str, to: &str) -> Vec<Effect> {
    PENDING_NOTE.replace(Some((from.to_string(), to.to_string())));
    vec![Effect::OpenPrompt(
        lattice::plugin_host::types::OpenPromptPayload {
            prompt: format!("Note ({to}): "),
            initial: String::new(),
            on_submit_action: "org-todo-note-submit".to_string(),
            buffer_name: None,
        },
    )]
}

/// The second hop: the note, written as the log line the transition owed.
///
/// An empty answer records the line with no note — a timestamp is still a
/// record, and org does the same when you store an empty note.
fn todo_note_submit(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
) -> Vec<Effect> {
    let Some((from, to)) = PENDING_NOTE.take() else {
        // No prompt is outstanding. Nothing to attribute the note to, and
        // guessing a transition would file it under the wrong one.
        return vec![Effect::None];
    };
    let Some(text) = submitted_text(&ctx.args) else {
        return vec![Effect::None];
    };
    let Some(target) = PlanTarget::resolve(ctx, doc, tree) else {
        return vec![Effect::None];
    };
    let end = target.subtree_end(doc, tree);
    let mut lines: Vec<String> = (target.headline..=end)
        .filter_map(|n| target.line(n, doc))
        .collect();
    if lines.is_empty() {
        return vec![Effect::None];
    }
    let stamp = clock_now().stamp();
    let mut log = complete::state_log_line(&to, &from, &stamp);
    let note = text.trim();
    if !note.is_empty() {
        // Org's continuation shape: the state line ends with ` \\` and the note
        // follows on the next line, indented under it.
        log.push_str(" \\\\\n  ");
        log.push_str(note);
    }
    complete::insert_log(
        &mut lines,
        &log,
        option_or("log-into-drawer", "true").eq_ignore_ascii_case("true"),
    );
    let last_len = target.line(end, doc).map(|l| l.len()).unwrap_or(0) as u32;
    replace_lines_at(
        target.buffer,
        target.headline,
        end,
        last_len,
        lines.join("\n"),
        ctx.cursor,
    )
}

/// TK.8 — set the headline's keyword, recording what the keyword flags ask for.
///
/// `org-todo-keywords` lets a state declare `(@)`, `(!)` or `(@/!)`, and until
/// this those were parsed and inert — the design said so in as many words, and
/// deferred acting on them to "its own slice with its own tests". This is that
/// slice's not-a-note half.
///
/// **One edit**, headline and log line together, so `u` takes the transition
/// back in one step. The note case cannot do that — it needs an answer from the
/// user first — and takes two; see [`note_prompt_for`].
fn set_keyword_logged(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    want: &str,
) -> Vec<Effect> {
    let keywords = todo_keywords();
    let kws = todo::parse_todo_keywords(&todo_keyword_lines().join("\n"));
    let Some(target) = PlanTarget::resolve(ctx, doc, tree) else {
        return vec![Effect::None];
    };
    let Some(head) = target.line(target.headline, doc) else {
        return vec![Effect::None];
    };
    let from = todo::parse(&head, &keywords)
        .and_then(|h| h.keyword.map(|k| k.to_string()))
        .unwrap_or_default();
    let to = (!want.is_empty()).then_some(want);
    let action = todo::log_on_change(&kws, (!from.is_empty()).then_some(from.as_str()), to);

    let Some(new_head) = todo::set_keyword(&head, &keywords, want) else {
        return vec![Effect::None];
    };
    if new_head == head {
        return vec![Effect::None];
    }
    match action {
        // The overwhelmingly common case, and it must stay a single-line edit:
        // most keywords declare no flags at all, and rewriting a whole subtree
        // to change one word would make every `<leader>ot` a bigger undo step
        // than it is.
        todo::LogOnChange::Nothing => {
            let cursor = Position {
                line: target.headline,
                byte: ctx.cursor.byte.min(new_head.len() as u32),
            };
            replace_lines_at(
                target.buffer,
                target.headline,
                target.headline,
                head.len() as u32,
                new_head,
                cursor,
            )
        }
        todo::LogOnChange::Timestamp => {
            let end = target.subtree_end(doc, tree);
            let mut lines: Vec<String> = (target.headline..=end)
                .filter_map(|n| target.line(n, doc))
                .collect();
            lines[0] = new_head;
            let stamp = clock_now().stamp();
            complete::insert_log(
                &mut lines,
                &complete::state_log_line(want, &from, &stamp),
                option_or("log-into-drawer", "true").eq_ignore_ascii_case("true"),
            );
            let last_len = target.line(end, doc).map(|l| l.len()).unwrap_or(0) as u32;
            replace_lines_at(
                target.buffer,
                target.headline,
                end,
                last_len,
                lines.join("\n"),
                ctx.cursor,
            )
        }
        // TK.9: the state change lands NOW and the note follows, which is org's
        // order — escaping the prompt leaves the state changed and unnoted.
        todo::LogOnChange::Note => {
            let cursor = Position {
                line: target.headline,
                byte: ctx.cursor.byte.min(new_head.len() as u32),
            };
            let mut effects = replace_lines_at(
                target.buffer,
                target.headline,
                target.headline,
                head.len() as u32,
                new_head,
                cursor,
            );
            effects.extend(note_prompt_for(&from, want));
            effects
        }
    }
}

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

/// OA.23: where the row under the cursor came from, or `None`.
///
/// `Some` only in a MULTIBUFFER — the agenda. In an ordinary org file the
/// cursor is already in the source, so a caller wanting a path there has
/// `doc.path()`; this answering `none` is what tells the two apart, and the two
/// need telling apart because an edit in a file is a plain `ApplyEdit` on
/// `ctx.buffer_id` while an edit behind the agenda has to name a document the
/// view does not compose.
///
/// OA.23b: `buffer` is the id to EDIT — not the path. See
/// `source-location.buffer`: the path may also name the user's own open buffer,
/// which is a different document, and writing to the wrong one loses work.
fn row_source(ctx: &ActionContext) -> Option<host_services::SourceLocation> {
    host_services::excerpt_source(u64::from(ctx.buffer_id), ctx.cursor.line)
}

/// OA.25 — where `s` / `d` are about to write, whichever surface they fired on.
///
/// Both surfaces from one struct, which is the point: the two differ only in
/// which document holds the headline and how the line below it is read, and
/// resolving that difference ONCE here is what keeps the handlers from growing
/// an agenda branch each.
struct PlanTarget {
    /// The buffer the edit names.
    buffer: u32,
    /// The headline's line, in that buffer's coordinates.
    headline: u32,
    /// The line below it, or `None` at end of document.
    next: Option<String>,
    /// Whether `buffer` is a multibuffer SOURCE rather than the active buffer.
    /// Decides which document lines are read through — the two coordinate
    /// spaces are unrelated, so guessing from the id would be a coin flip.
    from_source: bool,
}

impl PlanTarget {
    /// `None` when there is no headline to plan against — the cursor is in a
    /// file's preamble, or on an agenda header row rather than an entry.
    fn resolve(ctx: &ActionContext, doc: &Document, tree: Option<&TreeSnapshot>) -> Option<Self> {
        // The agenda first: `row_source` answering `Some` is what says this is
        // a composed view, and there the headline is the row, already located.
        if let Some(loc) = row_source(ctx) {
            return Some(PlanTarget {
                buffer: loc.buffer,
                headline: loc.line,
                // Read through the SOURCE document, not the file: an `s`
                // pressed twice must see the first one's line. See
                // `host-services.source-line`.
                next: host_services::source_line(loc.buffer, loc.line + 1),
                from_source: true,
            });
        }
        let line = |n: u32| doc.line(n);
        let hl = headline::Headlines::new(tree, &line, doc.line_count());
        let (start, _) = hl.enclosing(ctx.cursor.line)?;
        Some(PlanTarget {
            buffer: ctx.buffer_id,
            headline: start,
            next: doc.line(start + 1),
            from_source: false,
        })
    }

    /// One line of whichever document holds the headline.
    ///
    /// `doc` is a handle on the ACTIVE buffer, which in the agenda is the view
    /// rather than the source — so `doc.line(n)` there reads a composed row
    /// that happens to share a number with a source line. The two coordinate
    /// spaces are unrelated; only `from_source` tells them apart.
    fn line(&self, n: u32, doc: &Document) -> Option<String> {
        if self.from_source {
            host_services::source_line(self.buffer, n)
        } else {
            doc.line(n)
        }
    }

    /// The last line of the headline's subtree, in the target's coordinates.
    ///
    /// The two branches are not the same walk. In a file the tree-sitter
    /// structure is available and authoritative; behind the agenda there is no
    /// parse of the source and no line count either, so the end is found by
    /// reading until the document runs out. Both stop at the next same-or-
    /// shallower headline.
    fn subtree_end(&self, doc: &Document, tree: Option<&TreeSnapshot>) -> u32 {
        if self.from_source {
            headline::subtree_end_unbounded(|n| self.line(n, doc), self.headline)
        } else {
            let line = |n: u32| doc.line(n);
            headline::Headlines::new(tree, &line, doc.line_count()).subtree_end(self.headline)
        }
    }

    /// What is already on the planning line for `field`, for the prompt to
    /// open pre-filled — emacs opens `org-schedule` showing the current date,
    /// and retyping one you can see is the difference between changing a date
    /// and re-entering one.
    fn existing(&self, field: planning::Field) -> String {
        self.next
            .as_deref()
            .and_then(planning::parse)
            .and_then(|p| p.get(field).cloned())
            // The stamp without its brackets: `<2026-09-03 Thu>` is what org
            // writes, and what the prompt takes is a date expression.
            .map(|s| {
                s.trim_matches(|c| c == '<' || c == '>' || c == '[' || c == ']')
                    .to_string()
            })
            .unwrap_or_default()
    }
}

/// Today, in LOCAL time, as `org_date` wants it.
///
/// Local rather than UTC for `roam_file_stamp`'s reason: `+1d` resolved against
/// the UTC day schedules a task for the wrong date for anyone far enough east
/// or west, and a scheduling key that is off by one after 6pm is worse than no
/// key.
fn today_local() -> org_date::Date {
    let (year, month, day) =
        agenda::civil_from_epoch_day(epoch_day_from_local_secs(local_now_secs()));
    org_date::Date { year, month, day }
}

/// `<leader>os` / `<leader>od` — ask for a date. The first of two hops; the
/// answer arrives at [`plan_submit`].
///
/// An EMPTY answer removes the line, which is emacs' behaviour and the only
/// spelling of "unschedule" that does not need a second key. That is why the
/// prompt opens pre-filled: emptying a field you can see is a deliberate act,
/// where submitting an empty box you were never shown is an accident.
fn plan_prompt(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    field: planning::Field,
    on_submit: &str,
) -> Vec<Effect> {
    let Some(target) = PlanTarget::resolve(ctx, doc, tree) else {
        return vec![Effect::None];
    };
    vec![Effect::OpenPrompt(
        lattice::plugin_host::types::OpenPromptPayload {
            prompt: match field {
                planning::Field::Scheduled => "Schedule: ".to_string(),
                planning::Field::Deadline => "Deadline: ".to_string(),
            },
            initial: target.existing(field),
            on_submit_action: on_submit.to_string(),
            buffer_name: None,
        },
    )]
}

/// The second hop: the typed date, turned into the one edit that rewrites the
/// whole planning line.
///
/// Re-resolves the target rather than carrying it across the prompt, because
/// there is nowhere to carry it — the host dispatches the submit with a fresh
/// context. That is also the honest thing: the buffer is what it is now.
fn plan_submit(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    field: planning::Field,
    args: &Args,
) -> Vec<Effect> {
    let Some(target) = PlanTarget::resolve(ctx, doc, tree) else {
        return vec![Effect::None];
    };
    let Some(typed) = submitted_text(args) else {
        return vec![Effect::None];
    };
    let value = if typed.trim().is_empty() {
        None
    } else {
        match org_date::parse(&typed, today_local()) {
            Ok(plan) => Some(plan.render()),
            // Echo rather than swallow: a date the parser did not understand
            // is a typo the user can fix, and a key that silently does nothing
            // teaches them the feature is broken.
            Err(message) => {
                return vec![Effect::Echo(lattice::plugin_host::types::EchoPayload {
                    level: lattice::plugin_host::types::EchoLevel::Error,
                    text: message,
                })];
            }
        }
    };

    match planning::plan_edit(target.headline, target.next.as_deref(), field, value) {
        planning::PlanEdit::Nothing => vec![Effect::None],
        planning::PlanEdit::Replace { line, len, text } => {
            replace_lines_at(target.buffer, line, line, len, text, ctx.cursor)
        }
        // Insert as a REPLACE of the headline line by itself plus the new
        // line. Anchoring on the headline rather than on `line` start-of-line
        // keeps it correct at end of document, where `line` does not exist yet
        // and a range naming it would be out of bounds.
        planning::PlanEdit::Insert { line: _, text } => {
            let Some(head) = target.line(target.headline, doc) else {
                return vec![Effect::None];
            };
            replace_lines_at(
                target.buffer,
                target.headline,
                target.headline,
                head.len() as u32,
                format!("{head}\n{text}"),
                ctx.cursor,
            )
        }
        // Delete the planning line by replacing the headline-plus-planning
        // span with the headline alone — one edit, and it cannot leave a blank
        // line behind the way deleting a line's content would.
        planning::PlanEdit::Delete { line, len } => {
            let Some(head) = target.line(target.headline, doc) else {
                return vec![Effect::None];
            };
            replace_lines_at(target.buffer, target.headline, line, len, head, ctx.cursor)
        }
    }
}

/// OE.2, hop 1 — `org-set-property`: ask for the name.
///
/// The entry is resolved BEFORE the prompt opens, so a cursor that is nowhere
/// near a headline says so at the keystroke rather than after the user has
/// typed two answers. `PlanTarget` is what resolves it, which is also what
/// gives the command the agenda for free: a row there is a headline in its
/// source file, and setting a property on it writes to that file.
fn set_property_prompt(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
) -> Vec<Effect> {
    if PlanTarget::resolve(ctx, doc, tree).is_none() {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org: not inside a headline — a property goes on an entry".to_string(),
        })];
    }
    vec![Effect::OpenPrompt(
        lattice::plugin_host::types::OpenPromptPayload {
            prompt: "Property: ".to_string(),
            initial: String::new(),
            on_submit_action: "org-set-property-key".to_string(),
            buffer_name: None,
        },
    )]
}

/// OE.2, hop 2 — the name was typed; ask for the value, carrying the name.
///
/// An empty name backs out silently: `<CR>` on an empty prompt is how a person
/// abandons this, and asking for the value of a property with no name would
/// make the escape hatch look like a second question.
fn set_property_value_prompt(args: &Args) -> Vec<Effect> {
    let key = submitted_text(args).unwrap_or_default().trim().to_string();
    if key.is_empty() {
        return vec![Effect::None];
    }
    vec![Effect::OpenPrompt(
        lattice::plugin_host::types::OpenPromptPayload {
            prompt: format!("{key}: "),
            initial: String::new(),
            on_submit_action: "org-set-property-value".to_string(),
            // The smuggle slot (OC.3a). The value hop reads it as the SECOND
            // argument, beside the text the user typed.
            buffer_name: Some(key),
        },
    )]
}

/// OE.2, hop 3 — write it.
///
/// `args` is `[value, key]`: the typed text first, the name the previous hop
/// smuggled through `buffer-name` second. Re-resolves the entry rather than
/// carrying it, for `plan_submit`'s reason — there is nowhere to carry it, and
/// re-reading is the honest answer because the buffer is what it is now.
fn set_property_write(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
    args: &Args,
) -> Vec<Effect> {
    let Some(key) = smuggled_name(args).filter(|k| !k.trim().is_empty()) else {
        // No key means the hop that carries it did not run — a bug rather than
        // a user action, and writing `::` into their drawer would be the worst
        // possible answer to it.
        return vec![Effect::None];
    };
    let Some(target) = PlanTarget::resolve(ctx, doc, tree) else {
        return vec![Effect::None];
    };
    let value = submitted_text(args).unwrap_or_default().trim().to_string();
    let line = |n: u32| target.line(n, doc);
    match properties::set_entry_property(&line, target.headline, key.trim(), &value) {
        properties::PropertyEdit::Replace { at, text } => {
            let len = line(at).map(|l| l.len() as u32).unwrap_or(0);
            replace_lines_at(target.buffer, at, at, len, text, ctx.cursor)
        }
        // Anchored on the line ABOVE, exactly as `plan_submit` anchors its
        // insert: at end of document the line being inserted before does not
        // exist, and a range naming it is out of bounds.
        properties::PropertyEdit::Insert { at, text } => {
            let above = at.saturating_sub(1);
            let Some(prev) = line(above) else {
                return vec![Effect::None];
            };
            replace_lines_at(
                target.buffer,
                above,
                above,
                prev.len() as u32,
                format!("{prev}\n{text}"),
                ctx.cursor,
            )
        }
    }
}

/// The name the previous hop smuggled through `buffer-name` (OC.3a) — the
/// SECOND argument, where [`submitted_text`] reads the first.
fn smuggled_name(args: &Args) -> Option<String> {
    match args {
        Args::List(items) => items.get(1).and_then(|v| match v {
            lattice::plugin_host::types::ArgValue::String(s) => Some(s.clone()),
            lattice::plugin_host::types::ArgValue::Raw(s) => Some(s.clone()),
            _ => None,
        }),
        _ => None,
    }
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
            // forward). The host's `table-mode` binds `<Tab>` above this one
            // and declines outside a table, making the chain two hops with no
            // change here — it was `org-table-mode`'s binding until TB.2, and
            // moving it changed which crate declines, not the chain.
            CYCLE => Ok(cycle_at_cursor(&ctx, doc, tree)),
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
            ROAM_CAPTURE_FIELDS_SUBMIT => Ok(roam_capture_fields_submit(&ctx)),
            CAPTURE_FINALIZE => Ok(capture_finalize(doc)),
            CAPTURE_ABORT => Ok(capture_abort()),
            TOGGLE_CHECKBOX => Ok(toggle_checkbox(&ctx, doc, tree)),
            // OE.3: the context dispatcher. Its arms call the two bodies
            // above rather than repeating them.
            CTRL_C_CTRL_C => Ok(ctrl_c_ctrl_c(&ctx, doc, tree)),
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
            // OA.12: the agenda dispatcher, the same shape one menu over.
            AGENDA_MENU => Ok(vec![Effect::OpenTransient(
                lattice::plugin_host::types::OpenTransientPayload {
                    source: CAPTURE_TRANSIENT.to_string(),
                    args: Args::String(ORG_TRANSIENT_AGENDA.to_string()),
                },
            )]),
            // OA.18: the same seam, a different discriminator. The menu's rows
            // name actions that already exist — this handler opens a chooser
            // and owns no behaviour of its own, which is what keeps `gDd` and
            // the menu's `d` one code path rather than two.
            AGENDA_VIEW_MENU => Ok(vec![Effect::OpenTransient(
                lattice::plugin_host::types::OpenTransientPayload {
                    source: CAPTURE_TRANSIENT.to_string(),
                    args: Args::String(ORG_TRANSIENT_AGENDA_VIEW.to_string()),
                },
            )]),
            // OA.12: the chosen agenda rides the row's own args, and lands in
            // the SCAN-ARG slot rather than the argument one. The argument is
            // the root the host interprets; a command key sent there would
            // become a directory that does not exist (OA.11a).
            // OA.20: every span chord is the same move — take the view's own
            // arguments, change one, re-open. `reuse: true` means the same
            // buffer re-scans in place, so this is the whole implementation.
            AGENDA_LATER | AGENDA_EARLIER | AGENDA_TODAY | AGENDA_SPAN_DAY | AGENDA_SPAN_WEEK
            | AGENDA_SPAN_MONTH | AGENDA_SPAN_YEAR => {
                let mut view = VIEW_ARGS.with_borrow(Clone::clone);
                match callback {
                    AGENDA_LATER => view.offset = view.offset.saturating_add(1),
                    AGENDA_EARLIER => view.offset = view.offset.saturating_sub(1),
                    // `.` resets the OFFSET, not the span: "take me back to
                    // today" is about where you are, not about how much you
                    // were looking at, and losing a week view to get home
                    // would make the key cost more than it gives.
                    AGENDA_TODAY => view.offset = 0,
                    // Changing the span re-anchors on today. A `offset=2` held
                    // across a day→month switch means two MONTHS out, which is
                    // never what the person pressing `v m` meant.
                    AGENDA_SPAN_DAY => {
                        view.span = Some(0);
                        view.offset = 0;
                    }
                    AGENDA_SPAN_WEEK => {
                        view.span = Some(7);
                        view.offset = 0;
                    }
                    AGENDA_SPAN_MONTH => {
                        view.span = Some(30);
                        view.offset = 0;
                    }
                    _ => {
                        view.span = Some(365);
                        view.offset = 0;
                    }
                }
                Ok(vec![Effect::AppAction(AppEffect::OpenProviderView(
                    OpenProviderViewPayload {
                        provider: "agenda".to_string(),
                        argument: None,
                        scan_args: view.to_args(),
                    },
                ))])
            }
            // OA.21: `/` and `\` prompt; the difference is whether the answer
            // REPLACES the tag filter or narrows it, which the submit handler
            // reads back off the prompt's own action name.
            AGENDA_FILTER_TAG | AGENDA_FILTER_TAG_ADD => {
                let replacing = callback == AGENDA_FILTER_TAG;
                FILTER_REPLACES.replace(replacing);
                Ok(vec![Effect::OpenPrompt(
                    lattice::plugin_host::types::OpenPromptPayload {
                        prompt: if replacing {
                            "Filter by tag: ".to_string()
                        } else {
                            "Also filter by tag: ".to_string()
                        },
                        initial: String::new(),
                        on_submit_action: "org-agenda-filter-submit".to_string(),
                        buffer_name: None,
                    },
                )])
            }
            AGENDA_FILTER_TAG_SUBMIT => {
                let tag = submitted_text(&ctx.args)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let mut view = VIEW_ARGS.with_borrow(Clone::clone);
                if FILTER_REPLACES.get() {
                    view.filters
                        .retain(|f| !matches!(f, agenda_args::FilterTerm::Tag(_)));
                }
                // An empty answer CLEARS rather than filtering by nothing:
                // `/` then `<CR>` is how a person backs out, and a filter on
                // the empty tag would match no row and read as a broken agenda.
                if !tag.is_empty() {
                    view.filters.push(agenda_args::FilterTerm::Tag(tag));
                }
                Ok(vec![Effect::AppAction(AppEffect::OpenProviderView(
                    OpenProviderViewPayload {
                        provider: "agenda".to_string(),
                        argument: None,
                        scan_args: view.to_args(),
                    },
                ))])
            }
            // OA.26 — jump from a row to the headline it came from.
            //
            // The most-used key in emacs' agenda, and the seam answers it
            // directly: `excerpt-source` gives the file and the LINE, which is
            // exactly `OpenBufferAt`'s payload. Nothing here needs the
            // composed coordinates the row is displayed at.
            AGENDA_GOTO => {
                let Some(loc) = row_source(&ctx) else {
                    // Not a row — a header, a separator, or a plain buffer
                    // someone bound this in. `Effect::None` rather than
                    // `Declined`: `<CR>` declining would re-run the builtin
                    // first-non-blank-of-next-line motion, which in a
                    // read-only view is a silent no-op that looks identical
                    // and in an editable one is a surprise.
                    return Ok(vec![Effect::None]);
                };
                Ok(vec![Effect::OpenBufferAt(
                    lattice::plugin_host::types::OpenBufferAtPayload {
                        path: Some(loc.path),
                        position: Position {
                            line: loc.line,
                            byte: 0,
                        },
                        force: false,
                    },
                )])
            }
            // OA.26 — `<`, restrict the agenda to the row's own file.
            //
            // OA.21 shipped the `file:` filter term and could not bind it:
            // the term needs a path, and a guest acting in a multibuffer had
            // no way to learn which file a row came from. That is the gap
            // OA.23 closed.
            //
            // REPLACES any existing file filter rather than adding to it.
            // `file:` terms are an OR, so accumulating them would widen the
            // view on a key whose whole name is "restrict" — press it twice on
            // two rows and you would be looking at more than when you started.
            AGENDA_FILTER_FILE => {
                let Some(loc) = row_source(&ctx) else {
                    return Ok(vec![Effect::None]);
                };
                // The NAME, not the path: `file:` matches on the file name
                // (see `agenda_args`), so a full path would match nothing and
                // read as a key that empties the agenda.
                let name = loc.path.rsplit('/').next().unwrap_or(&loc.path).to_string();
                let mut view = VIEW_ARGS.with_borrow(Clone::clone);
                view.filters
                    .retain(|f| !matches!(f, agenda_args::FilterTerm::File(_)));
                view.filters.push(agenda_args::FilterTerm::File(name));
                Ok(vec![Effect::AppAction(AppEffect::OpenProviderView(
                    OpenProviderViewPayload {
                        provider: "agenda".to_string(),
                        argument: None,
                        scan_args: view.to_args(),
                    },
                ))])
            }
            // OA.15: the same move every phase-6 chord makes — take the view's
            // own arguments, change one, re-open. The mode IS the argument, so
            // there is no second piece of state that could disagree with it,
            // and `gr` carries it forward for free.
            // OA.15b: `l` flips the MODE and nothing else.
            //
            // It does not touch `VIEW_ARGS`, and that is the slice's whole
            // shape. The mode is the single source of truth for whether log
            // rows are on; the `log=` scan argument is derived from it in the
            // lifecycle handler (`ON_AGENDA_LOG_MODE`). A chord that wrote the
            // argument *and* flipped the mode would be two writers of one
            // fact, which is the disagreement OA.16 chose the
            // mode-as-the-switch shape to prevent — and the drift would be
            // invisible, because both states look right in isolation.
            //
            // The same `Effect::ToggleMode` the auto-generated
            // `:org-agenda-log-mode` ex-command returns, so the chord and the
            // command are one switch rather than two paths that can differ.
            AGENDA_LOG_MODE => Ok(vec![Effect::ToggleMode(AGENDA_LOG_MODE_ID.to_string())]),
            AGENDA_FILTER_CLEAR => {
                let mut view = VIEW_ARGS.with_borrow(Clone::clone);
                view.filters.clear();
                Ok(vec![Effect::AppAction(AppEffect::OpenProviderView(
                    OpenProviderViewPayload {
                        provider: "agenda".to_string(),
                        argument: None,
                        scan_args: view.to_args(),
                    },
                ))])
            }
            AGENDA_COMMAND => {
                let key = match &ctx.args {
                    Args::String(s) => s.trim().to_string(),
                    _ => String::new(),
                };
                Ok(vec![Effect::AppAction(AppEffect::OpenProviderView(
                    OpenProviderViewPayload {
                        provider: "agenda".to_string(),
                        // No root override: a custom command says WHICH agenda,
                        // never WHERE. The files stay `org.agenda-files`, which
                        // is the only place a user has said where their org
                        // lives.
                        argument: None,
                        // An empty key is the default agenda rather than an
                        // error — it is what the built-in row sends, and
                        // `agenda_custom_commands::resolve` already treats a
                        // blank key as "no command named".
                        scan_args: if key.is_empty() {
                            Vec::new()
                        } else {
                            vec![key]
                        },
                    },
                ))])
            }
            // The chosen state rides the row's own args (TR.2a).
            TODO_SET => {
                // An empty string is a real choice — the menu's "(none)"
                // row clears the state — so a missing arg and a cleared
                // state are deliberately the same thing here.
                let want = match &ctx.args {
                    Args::String(s) => s.clone(),
                    _ => String::new(),
                };
                Ok(set_keyword_repeating(&ctx, doc, tree, &want))
            }
            // HB.2: cycling INTO a done state repeats a repeating task, the
            // same as selecting it. Routing only `TODO_SET` would have left
            // `<leader>ot` — the key people actually use on a habit —
            // destroying it, which is the half-migration this whole slice is
            // about closing.
            TODO_CYCLE => Ok(cycle_keyword_repeating(&ctx, doc, tree, true)),
            TODO_CYCLE_BACK => Ok(cycle_keyword_repeating(&ctx, doc, tree, false)),
            PRIORITY_CYCLE => {
                let highest = highest_priority();
                Ok(rewrite_headline(&ctx, doc, tree, move |line, kw| {
                    todo::cycle_priority(line, kw, highest, true)
                }))
            }
            SET_TAGS => Ok(set_tags_prompt(&ctx, doc, tree)),
            // OR.10 — the dailies prompt's second hop. Escape dispatches
            // nothing, so a dismissed prompt creates no file; an empty submit
            // is the same answer, said out loud.
            ROAM_DAILIES_GOTO_DATE_SUBMIT => {
                let Some(text) = submitted_text(&ctx.args) else {
                    return Ok(vec![Effect::None]);
                };
                Ok(match roam_dailies::parse(&text) {
                    Ok(date) => open_daily(date),
                    Err(error) => vec![Effect::Echo(EchoPayload {
                        level: EchoLevel::Warn,
                        text: error,
                    })],
                })
            }
            TODO_NOTE_SUBMIT => Ok(todo_note_submit(&ctx, doc, tree)),
            // OE.2: three hops, one command. See `set_property_prompt`.
            SET_PROPERTY => Ok(set_property_prompt(&ctx, doc, tree)),
            SET_PROPERTY_KEY => Ok(set_property_value_prompt(&ctx.args)),
            SET_PROPERTY_VALUE => Ok(set_property_write(&ctx, doc, tree, &ctx.args)),
            SET_TAGS_SUBMIT => {
                let Some(tags) = submitted_text(&ctx.args) else {
                    return Ok(vec![Effect::None]);
                };
                Ok(rewrite_headline(&ctx, doc, tree, move |line, kw| {
                    todo::set_tags(line, kw, &tags)
                }))
            }
            // OA.25. All four go through the same two functions; only the
            // field differs, which is the point of resolving the surface in
            // `PlanTarget` rather than in each arm.
            SCHEDULE => Ok(plan_prompt(
                &ctx,
                doc,
                tree,
                planning::Field::Scheduled,
                "org-schedule-submit",
            )),
            DEADLINE => Ok(plan_prompt(
                &ctx,
                doc,
                tree,
                planning::Field::Deadline,
                "org-deadline-submit",
            )),
            SCHEDULE_SUBMIT => Ok(plan_submit(
                &ctx,
                doc,
                tree,
                planning::Field::Scheduled,
                &ctx.args,
            )),
            DEADLINE_SUBMIT => Ok(plan_submit(
                &ctx,
                doc,
                tree,
                planning::Field::Deadline,
                &ctx.args,
            )),
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
            // OR.10: `-goto-date` takes a date OPTIONALLY. Absent means "ask
            // me", which is what makes the same command serve `<leader>ondD`.
            // The date is VALIDATED here rather than in the handler so a typo
            // is refused by the `:` line — where the text is still on screen
            // and editable — instead of by an echo after it has scrolled away.
            ROAM_DAILIES_DATE_PARSE => {
                let trimmed = rest.trim();
                if trimmed.is_empty() {
                    Ok(Args::None)
                } else {
                    roam_dailies::parse(trimmed).map(|_| Args::String(trimmed.to_string()))
                }
            }
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
            // OR.9 — resolve the node at point HERE, on the grammar seam, and
            // hand its id to the picker as an argument.
            //
            // The picker seam has no cursor and no document: `init` receives
            // args and a `picker-context`, not the buffer you were in. So the
            // ex-command is the only place that can answer "which node am I
            // in", and the id crosses as the argument it resolved.
            ROAM_BACKLINKS => Ok(backlinks_at_point(&ctx, doc)),
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
                // OR.11b: with templates configured, ASK which one — the second
                // hop writes the note. Without any, write the stub as before: a
                // user who has never configured a template must not meet a
                // one-row menu, and note creation predates this option.
                //
                // Before the id is minted, so the hop that discards this call's
                // work does not mint one first. An `:ID:` is cheap but it is
                // also the one value here that must be unique per NOTE, and
                // minting two per note invites the reading that either is the
                // note's.
                if !roam_templates::read().is_empty() {
                    return Ok(vec![Effect::OpenTransient(
                        lattice::plugin_host::types::OpenTransientPayload {
                            source: CAPTURE_TRANSIENT.to_string(),
                            args: Args::List(vec![
                                lattice::plugin_host::types::ArgValue::String(
                                    ORG_TRANSIENT_ROAM.to_string(),
                                ),
                                lattice::plugin_host::types::ArgValue::String(title),
                            ]),
                        },
                    )]);
                }
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
                // TWO effects, in order, and deliberately no trailing echo.
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
                //
                // OR.13: `WriteToFile` is deliberately non-focusing — archive,
                // refile and capture-relocation all rely on it NOT stealing
                // focus when they move text into a file the user isn't
                // looking at (see `cross-file-writes.md` §2). The stub-create
                // path is not one of those callers: it exists to open a new
                // note for the user to start typing into, so it must ALSO
                // focus what it just wrote. `OpenBufferAt` does that, named
                // explicitly by the same `path` `WriteToFile` resolves — the
                // host applies effects in order (`handle_effect`), so the
                // file exists on disk (and its buffer in the registry) before
                // `OpenBufferAt` looks it up by path. Nothing here is
                // inferred from "whatever was just opened", which is the
                // hazard `cross-file-writes.md` §1 actually warns about.
                Ok(vec![
                    Effect::WriteToFile(WriteToFilePayload {
                        path: path.clone(),
                        anchor: lattice::plugin_host::types::FileAnchor::End,
                        text: roam_find::new_node_text(&id, &title),
                        cut: None,
                        // The roam directory is a path the user configured; if it
                        // does not exist that is worth saying, not papering over.
                        create_parents: false,
                    }),
                    Effect::OpenBufferAt(lattice::plugin_host::types::OpenBufferAtPayload {
                        path: Some(path),
                        position: Position { line: 0, byte: 0 },
                        force: false,
                    }),
                ])
            }
            // OR.11a + OR.11b — the second hop: the chosen template becomes a
            // note.
            ROAM_CREATE_FROM_TEMPLATE => Ok(roam_create_from_template(&ctx)),
            ROAM_SYNC => {
                host_services::emit_event(EV_ROAM_SYNC, &[]);
                Ok(vec![Effect::Echo(EchoPayload {
                    level: EchoLevel::Info,
                    text: "org-roam: re-scanning\u{2026}".to_string(),
                })])
            }
            // OR.10 — the journal. Today, and the two days either side of it,
            // are the same call with a different shift.
            ROAM_DAILIES_TODAY => Ok(open_daily(roam_dailies::Date::from_now(&clock_now()))),
            ROAM_DAILIES_YESTERDAY => Ok(open_daily(
                roam_dailies::Date::from_now(&clock_now()).shifted(-1),
            )),
            ROAM_DAILIES_TOMORROW => Ok(open_daily(
                roam_dailies::Date::from_now(&clock_now()).shifted(1),
            )),
            // With a date, go. Without one, ask — which is the whole reason
            // `<leader>ondD` can bind this command rather than needing an
            // action of its own.
            ROAM_DAILIES_GOTO_DATE => Ok(match &ctx.args {
                Args::String(text) if !text.trim().is_empty() => match roam_dailies::parse(text) {
                    // Already validated at parse time; re-checked because the
                    // action seam (the prompt's second hop) reaches the same
                    // helper without going through `parse_ex_args`.
                    Ok(date) => open_daily(date),
                    Err(error) => vec![Effect::Echo(EchoPayload {
                        level: EchoLevel::Warn,
                        text: error,
                    })],
                },
                _ => vec![Effect::OpenPrompt(
                    lattice::plugin_host::types::OpenPromptPayload {
                        prompt: "Journal date (YYYY-MM-DD): ".to_string(),
                        // Pre-filled with today, so the common edit is two
                        // keystrokes on the day rather than typing a date out.
                        initial: roam_dailies::Date::from_now(&clock_now()).title(),
                        on_submit_action: "org-roam-dailies-goto-date-submit".to_string(),
                        buffer_name: None,
                    },
                )],
            }),
            // The agenda view is generic host machinery: it builds the
            // multibuffer, walks the files and asks every registered
            // `scanned-excerpt-source` for rows. This plugin does not open it — it rings
            // the doorbell by the provider's name and supplies its own rows
            // through the seam, exactly as before. Only the trigger moved.
            AGENDA_APPLY => Ok(vec![Effect::AppAction(AppEffect::OpenProviderView(
                OpenProviderViewPayload {
                    provider: "agenda".to_string(),
                    argument: match &ctx.args {
                        Args::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                        _ => None,
                    },
                    // OA.11a: `:org-agenda` runs the DEFAULT agenda, so it
                    // names no command. The dispatcher (OA.12) is what fills
                    // this in; keeping the ex-command's meaning unchanged is
                    // what makes that a separate, revertable slice.
                    scan_args: Vec::new(),
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
        // OR.6/OR.9: three sources now share this body. Route first.
        if source == roam_backlinks::BACKLINKS_PICKER {
            return Ok(roam_backlinks::init(&args)?
                .into_iter()
                .map(|(candidate, routing)| {
                    exports::lattice::plugin_host::picker_source::CandidatePair {
                        candidate,
                        routing,
                    }
                })
                .collect());
        }
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
                            // PS.1: unstyled deliberately. A refile target is a
                            // `parent/child/grandchild` PATH, not one headline's
                            // text, so painting it as a title would style the
                            // separators too and claim a structure the row does
                            // not have.
                            display_spans: Vec::new(),
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
        if source == roam_backlinks::BACKLINKS_PICKER {
            return roam_backlinks::accept(routing);
        }
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

/// OE.3 — `C-c C-c`, emacs' `org-ctrl-c-ctrl-c`: act on the thing at point.
///
/// The most-pressed key in org, and the one surface where a key's meaning is
/// the CONTEXT rather than the chord. Two arms here; the rest live on the
/// modes that own what they act on (`focused-surface.md`'s sibling argument:
/// a guest cannot invoke a registered command, so `table-mode` binds this
/// chord itself and declines outside a table — OE.4).
///
/// | Context | Does |
/// |---|---|
/// | Checkbox item | toggle it, updating every ancestor cookie |
/// | Headline | set tags |
/// | Anything else | say so |
///
/// **The arms CALL the bodies the chords call.** `toggle_checkbox` and
/// `set_tags_prompt` are the same functions `<C-Space>` and `<C-c><C-q>`
/// reach, not copies — two spellings of one verb that could drift is the
/// thing this file already avoids everywhere else.
///
/// **No statistics-cookie arm**, and the slice plan said there would be one.
/// Emacs spells that `C-c #` (`org-update-statistics-cookies`), and a cookie
/// lives on a headline or on a parent list item — both of which already have
/// an arm here, and the toggle already rewrites every ancestor's cookie as a
/// side effect. A third arm would have been inventing a binding org does not
/// have.
///
/// **The fallback is a message, not silence.** Emacs answers `C-c C-c can do
/// nothing useful at this location`; a key that does nothing is
/// indistinguishable from one that is unbound, which is the failure class
/// this codebase keeps paying for.
fn ctrl_c_ctrl_c(ctx: &ActionContext, doc: &Document, tree: Option<&TreeSnapshot>) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let at = ctx.cursor.line;

    // A checkbox first: `item_at` already requires a real `[ ]` box, so a
    // plain bullet falls through rather than toggling nothing.
    let cb = checkbox::Checkboxes::new(tree, &line, doc.line_count());
    if cb.item_at(at).is_some() {
        return toggle_checkbox(ctx, doc, tree);
    }

    // Then the headline — the line ITSELF, not the subtree it encloses.
    // `enclosing` answers for every line under a headline, and `C-c C-c` in
    // the body of an entry is not "set the entry's tags" in emacs either.
    let hl = headline::Headlines::new(tree, &line, doc.line_count());
    if hl.enclosing(at).is_some_and(|(start, _)| start == at) {
        return set_tags_prompt(ctx, doc, tree);
    }

    vec![Effect::Echo(EchoPayload {
        level: EchoLevel::Warn,
        text: "org: C-c C-c has nothing to do here".to_string(),
    })]
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

/// OR.10 — open one day's journal entry, creating it if it is not there.
///
/// ## Existence is answered by `read-file`, not guessed
///
/// The roam index would be the tempting source — it has walked the corpus and
/// knows every file it saw. It is the wrong one: the index lags the filesystem
/// by a watcher debounce, so a journal created seconds ago reads as absent, and
/// "absent" here means *append the header again* to a file that already has
/// one. `host-services.read-file` is the fresh answer and, per its own doc
/// comment, is reachable from this seam precisely because a guest's WASI view
/// is not.
///
/// A read that fails for a reason other than absence — a revoked `fs:read`
/// grant, a permission bit — lands on the create path. That is deliberate: the
/// create path APPENDS (`FileAnchor::End`), so the worst case is a duplicated
/// header at the end of a file the user can see and delete, rather than
/// anything lost. Guessing the other way and refusing to open would make an
/// unreadable journal unreachable.
///
/// ## A new daily is a node, because the corpus's are
///
/// All eight dailies in the reference corpus carry a file-level `:ID:`, which
/// emacs's *default* dailies template does not write — the user's own template
/// does. Writing one matters beyond matching: without an id a journal entry
/// cannot be linked to, and linking a day to what happened on it is most of
/// what a journal is for. So this reuses `new_node_text`, which is the same
/// body `:org-roam-create-node` writes and the same one OR.4's extraction reads
/// back.
fn open_daily(date: roam_dailies::Date) -> Vec<Effect> {
    let Some(path) = roam_dailies::path(&date) else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org-roam: set `org.roam-directory` to the folder your notes live in".to_string(),
        })];
    };
    if host_services::read_file(&path).is_ok() {
        return vec![Effect::OpenBuffer(
            lattice::plugin_host::types::OpenBufferPayload {
                path: Some(path),
                force: false,
            },
        )];
    }
    let id = match host_services::new_uuid() {
        Ok(id) => id,
        // Refuses rather than degrades, for `:org-roam-create-node`'s reason:
        // an `:ID:` is written into the user's file and outlives the session,
        // so an empty one is worse than no entry.
        Err(error) => {
            return vec![Effect::Echo(EchoPayload {
                level: EchoLevel::Error,
                text: format!("org-roam: cannot mint an id: {error}"),
            })];
        }
    };
    // ONE effect and no trailing echo, for `:org-roam-create-node`'s reason:
    // `WriteToFile` reports its own failure by setting the message, and an
    // `Echo` after it would overwrite exactly that — so a refused write would
    // look like a successful one. The opened buffer IS the feedback.
    vec![Effect::WriteToFile(WriteToFilePayload {
        path,
        anchor: lattice::plugin_host::types::FileAnchor::End,
        text: roam_find::new_node_text(&id, &date.title()),
        cut: None,
        // The ONE site that asks. `daily/` is named by an option with a
        // default and is never typed by the user, so it is part of the
        // layout org owns — and without it the very first
        // `:org-roam-dailies-today` on a fresh corpus fails, which is
        // the one use where the feature has to work.
        create_parents: true,
    })]
}

/// `<Tab>` — cycle whatever fold the cursor is ON.
///
/// ## Any fold, not only a headline
///
/// This used to fire only on a headline and decline everywhere else, so
/// `<Tab>` on a `:PROPERTIES:` drawer or a `#+BEGIN_SRC` line fell through to
/// the builtin and did nothing org-ish. That was a gap rather than a policy:
/// `queries/folds.scm` has always captured `(drawer)`, `(property_drawer)`,
/// `(block)`, `(dynamic_block)`, `(list)`, `(table)` and `(latex_env)`
/// alongside `(section)`, and the host's `do_cycle_fold_at_cursor` matches the
/// innermost fold with `|_| true` — it never cared what kind it was. Only this
/// gate did.
///
/// ## The rule is "starts here", not "is inside"
///
/// A fold cycles when the cursor is on the line that OPENS it, which is emacs's
/// rule and preserves the one OT.4 argued for: inside a source block the block
/// began on an earlier line, so this declines and `<Tab>` keeps its native
/// meaning where a user is editing code. On the `#+BEGIN_SRC` line itself it
/// now folds, which is what was missing.
///
/// ## Two conditions beyond the kind
///
/// **Multi-line**, because the fold pipeline drops single-line captures
/// ("nothing to hide"). Without this check the guest would claim a fold the
/// host cannot find, and `do_cycle_fold_at_cursor` falls back to the GLOBAL
/// cycle when it finds nothing — so a `<Tab>` on an unfoldable line would fold
/// the entire buffer. A wrong answer that big is worse than declining.
///
/// **Probed at the first non-blank column**, not column 0: a node that starts
/// after indentation does not contain column 0, so the walk would land in
/// whatever encloses it and miss the drawer entirely. That is OT.6's lesson,
/// which cost every indented checkbox list once.
fn cycle_at_cursor(
    ctx: &ActionContext,
    doc: &Document,
    tree: Option<&TreeSnapshot>,
) -> Vec<Effect> {
    let read = |n: u32| doc.line(n);
    let here = ctx.cursor.line;

    // Mirrors `queries/folds.scm`. The two lists must agree: a kind here that
    // the query does not capture claims a fold the host cannot find (and gets
    // the global cycle), and a kind the query captures but this omits is a fold
    // `<Tab>` silently refuses to touch — the bug being fixed.
    const FOLDABLE: [&str; 8] = [
        "section",
        "block",
        "dynamic_block",
        "drawer",
        "property_drawer",
        "list",
        "table",
        "latex_env",
    ];

    if let Some(snapshot) = tree {
        let text = read(here).unwrap_or_default();
        let col = (text.len() - text.trim_start().len()) as u32;
        let kinds: Vec<String> = FOLDABLE.iter().map(|k| (*k).to_string()).collect();
        if let Some(node) = snapshot.enclosing(
            Position {
                line: here,
                byte: col,
            },
            &kinds,
        ) {
            let range = node.byte_range();
            let opens_here = range.start.line == here;
            let multi_line = tree::last_content_line(&range) > range.start.line;
            if opens_here && multi_line {
                return vec![Effect::AppAction(AppEffect::CycleFoldAtCursor)];
            }
        }
        return vec![Effect::Declined];
    }

    // No tree — the text fallback can only recognise a headline, which is what
    // this did for every kind before. Degraded rather than wrong.
    let hl = headline::Headlines::new(tree, &read, doc.line_count());
    if hl.is_headline(here) {
        vec![Effect::AppAction(AppEffect::CycleFoldAtCursor)]
    } else {
        vec![Effect::Declined]
    }
}

/// OR.9 — open the backlinks picker for the node the cursor is in.
///
/// Resolving the node happens here rather than in the picker because the
/// picker seam has neither a cursor nor the document — `init` gets args and a
/// `picker-context`. So this reads the buffer, finds the innermost node
/// containing the cursor, and passes its id across as the picker's argument.
///
/// **Not in a node is a message, not an empty picker.** "Nothing links here"
/// and "you are not in a note" look identical in an empty list and have
/// entirely different fixes — `find_node`'s unconfigured case makes the same
/// distinction for the same reason.
fn backlinks_at_point(ctx: &ExCommandContext, doc: &Document) -> Vec<Effect> {
    let read = |n: u32| doc.line(n);
    let Some(id) = roam_backlinks::node_id_at(&read, doc.line_count(), ctx.cursor.line) else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org-roam: not inside a node — no `:ID:` on this entry or its file".to_string(),
        })];
    };
    vec![Effect::OpenPicker(
        lattice::plugin_host::types::OpenPickerPayload {
            source: roam_backlinks::BACKLINKS_PICKER.to_string(),
            args: vec![id],
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
/// `org.capture-templates`, and `:set` must take effect on the next `<leader>oc`.
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
/// OA.12 — the agenda dispatcher: one row per configured agenda, plus the
/// built-in one.
///
/// **Built per open, never cached**, for the capture and TODO menus' reason:
/// the rows come from `org.agenda-custom-commands`, and a `:set` must take
/// effect on the next press.
///
/// **The built-in agenda is always a row**, and always first. It is what
/// `<leader>oa` opened before OA.13 moved that chord to this menu, so a
/// dispatcher without it would take a working agenda away from every user who
/// has configured no commands — which is nearly all of them. It sends an empty
/// key, which `agenda_custom_commands::resolve` already reads as "no command
/// named".
///
/// **An unconfigured user still gets a menu**, rather than an error. This is
/// the one place org's menus differ from `todo_menu`, which errs when there are
/// no keywords, and the difference is not an oversight: a TODO menu with no
/// states can do nothing at all, whereas an agenda dispatcher with no custom
/// commands can still open the agenda. A menu that refuses to open when it has
/// a working row is worse than a short menu.
fn agenda_menu() -> Result<lattice::plugin_host::types::TransientSpec, String> {
    use lattice::plugin_host::types::{
        Args as WitArgs, TransientAction, TransientGroup, TransientItem, TransientItemKind,
        TransientSpec,
    };

    let span = option_or("agenda-span", DEFAULT_AGENDA_SPAN)
        .trim()
        .parse::<u32>()
        .unwrap_or_else(|_| {
            DEFAULT_AGENDA_SPAN
                .parse()
                .expect("the compiled-in default parses")
        });
    // An unusable set costs the ROWS it describes, never the menu: the built-in
    // agenda is still reachable, and the footer says what went wrong. Erring
    // here would mean a typo in one command's `when` left you unable to open
    // any agenda at all.
    let parsed = agenda_custom_commands::read(span);

    let mut items: Vec<TransientItem> = Vec::new();
    // `a` rather than `<Space>`: emacs's `C-c a a` is the built-in agenda, and
    // this menu is reached by the same `a`, so the second press repeating it is
    // the muscle memory people arrive with.
    items.push(TransientItem {
        key: vec!["a".to_string()],
        label: "Agenda".to_string(),
        description: "the built-in blocks".to_string(),
        kind: TransientItemKind::Action(TransientAction {
            command: "org-agenda-command".to_string(),
            args: WitArgs::String(String::new()),
        }),
    });

    let mut footer = None;
    match &parsed {
        Ok(set) => {
            for c in &set.commands {
                items.push(TransientItem {
                    key: vec![c.key.clone()],
                    label: c.description.clone(),
                    description: format!(
                        "{} block{}",
                        c.sections.len(),
                        if c.sections.len() == 1 { "" } else { "s" }
                    ),
                    kind: TransientItemKind::Action(TransientAction {
                        command: "org-agenda-command".to_string(),
                        args: WitArgs::String(c.key.clone()),
                    }),
                });
            }
            // What the set could not use is named in the FOOTER rather than
            // dropped in silence — the capture menu's rule, and this is the one
            // place a missing row is noticeable, because the user is looking at
            // the menu and counting.
            if !set.skipped.is_empty() {
                footer = Some(format!("skipped: {}", set.skipped.join("; ")));
            }
        }
        // `Unset` is the ordinary case and says nothing; a broken set says so
        // here, which is the one channel that reaches the user BEFORE they have
        // opened an agenda. The section-title notice only lands after.
        Err(e) => footer = e.notice(),
    }

    // A menu with no way out is a trap. `q` is the transient's own convention.
    items.push(TransientItem {
        key: vec!["q".to_string()],
        label: "quit".to_string(),
        description: String::new(),
        kind: TransientItemKind::Dismiss,
    });

    Ok(TransientSpec {
        title: "Agenda".to_string(),
        groups: vec![TransientGroup {
            label: String::new(),
            items,
        }],
        footer,
    })
}

/// OA.18 — the view dispatch: how the open agenda is being SHOWN.
///
/// Emacs' `org-agenda-view-mode-dispatch`, whose letters this reuses exactly —
/// `d`ay, `w`eek, `m`onth, `y`ear, `l`og, `r`eport. A hand that has typed
/// `v d` in emacs types `gD d` here and gets the same view.
///
/// **Fixed rows, and the only menu of org's that reads no option.** Every row
/// names an action that is already registered, so the menu adds no behaviour
/// and cannot drift from the chords: the `d` here and `f` / `b` / `.` next to
/// it are one code path. That is also why it cannot fail — there is no
/// `Err` arm, unlike the capture and TODO menus, which err when what they
/// enumerate is unconfigured.
///
/// **Two groups, because the rows answer different questions.** How much time
/// am I looking at, versus what extra content is on. Emacs runs them together
/// in one prompt line; a menu has room to say which is which.
///
/// **No row shows its own on/off state**, and that is deliberate rather than
/// unfinished. Log mode's state is derivable here (`VIEW_ARGS.log`), the clock
/// report's is not — it is a NATIVE mode and the guest has no query for mode
/// state at all. One row reporting state beside one that cannot is worse than
/// neither reporting it: the silent row reads as "off". Emacs shows no state
/// either.
///
/// **The clock-report row names a HOST action**, `scan-view-clockreport-mode`'s
/// own toggle. Not org's to re-declare: the report is generic over any scan
/// view that reports clock spans (OA.16), and a second org-side toggle would be
/// a second writer of one switch.
fn agenda_view_menu() -> Result<lattice::plugin_host::types::TransientSpec, String> {
    use lattice::plugin_host::types::{
        Args as WitArgs, TransientAction, TransientGroup, TransientItem, TransientItemKind,
        TransientSpec,
    };

    let row = |key: &str, label: &str, description: &str, command: &str| TransientItem {
        key: vec![key.to_string()],
        label: label.to_string(),
        description: description.to_string(),
        kind: TransientItemKind::Action(TransientAction {
            command: command.to_string(),
            // The rows carry no arguments: each names a distinct action, so
            // there is nothing for a parameter to choose between. The capture
            // menu's per-row args exist because its rows differ only by
            // template key.
            args: WitArgs::None,
        }),
    };

    Ok(TransientSpec {
        title: "Agenda view".to_string(),
        groups: vec![
            TransientGroup {
                label: "Span".to_string(),
                items: vec![
                    row("d", "day", "one day", "org-agenda-day-view"),
                    row("w", "week", "seven days", "org-agenda-week-view"),
                    row("m", "month", "thirty days", "org-agenda-month-view"),
                    row("y", "year", "a year", "org-agenda-year-view"),
                ],
            },
            TransientGroup {
                label: "Display".to_string(),
                items: vec![
                    row(
                        "l",
                        "log mode",
                        "what you did, not what you planned",
                        "org-agenda-log-mode",
                    ),
                    row(
                        "r",
                        "clock report",
                        "clocked time, totalled up the outline",
                        lattice_clock_report_toggle(),
                    ),
                    TransientItem {
                        // A menu with no way out is a trap. `q` is the
                        // transient's own convention.
                        key: vec!["q".to_string()],
                        label: "quit".to_string(),
                        description: String::new(),
                        kind: TransientItemKind::Dismiss,
                    },
                ],
            },
        ],
        footer: None,
    })
}

/// The host's clock-report toggle, named in one place.
///
/// A string literal that has to agree with
/// `lattice_multibuffer::providers::clock_report::TOGGLE_ACTION` — the guest
/// compiles to wasm and cannot depend on the host crate, so this is a wire
/// value like every callback id above it. Isolated in a function so the one
/// place it is written is the one place a test can pin it.
const fn lattice_clock_report_toggle() -> &'static str {
    "action:scan-view-clockreport-toggle"
}

/// OR.11b — the roam template chooser, opened for one node.
///
/// Each row carries the TITLE as well as its key, because the menu is the only
/// place that knows both and the create action needs both. Carrying the title
/// through the menu rather than stashing it guest-side is what makes two
/// concurrent creates impossible to confuse — the same reason TR.3a gave a
/// transient open its own arguments.
/// The note's FILENAME for one template and node.
///
/// Takes `${…}` and nothing else: `%U` in a FILENAME would put a timestamp with
/// spaces and brackets into a path, which is a different kind of mistake from
/// putting one in a note.
fn roam_note_filename(
    template: &roam_templates::RoamTemplate,
    node: &roam_capture::Node<'_>,
) -> String {
    match template.file.as_deref() {
        Some(pattern) => roam_capture::expand_fields(pattern, node),
        None => {
            let stamp = roam_file_stamp();
            if node.slug.is_empty() {
                format!("{stamp}.org")
            } else {
                format!("{stamp}-{}.org", node.slug)
            }
        }
    }
}

/// The capture buffer a roam note is drafted in.
///
/// Namespaced apart from `*org-capture:…*` so a roam template and a capture
/// template that share a key cannot land in the same buffer — they are two
/// different drafts filing to two different places, and one buffer holding both
/// would file whichever was typed last into whichever target was remembered.
fn roam_capture_buffer_name(key: &str) -> String {
    if key.is_empty() {
        "*org-roam-capture*".to_string()
    } else {
        format!("*org-roam-capture:{key}*")
    }
}

/// OR.11a + OR.11b — open the note the chosen template describes, as a draft.
///
/// The second hop of `:org-roam-create-node`, and a THIRD hop when the template
/// asks questions.
///
/// ## The buffer, not a write
///
/// This wrote the file directly until OR.11b, which is why `%?`, `%^{…}` and
/// `%a` all expanded to nothing: each of them needs a surface, and there was
/// none. It now opens the same capture buffer `<leader>oc` opens — `org-mode`
/// major, `org-capture-mode` minor, `C-c C-c` to file and `C-c C-k` to throw
/// away — with the destination remembered rather than a template.
///
/// **`C-c C-k` leaves nothing behind, and that falls out rather than being
/// cleaned up.** The file is written on finalize, so an abort has nothing to
/// undo — including the minted id, which is simply discarded. That is emacs's
/// behaviour for an aborted roam capture and the property the direct write
/// could not have.
///
/// ## Both placeholder syntaxes, in one order
///
/// `${…}` first — `roam_capture::expand_fields` interpolates the node being
/// made — then `capture::expand_for_buffer` interpolates the capture context
/// and reports where `%?` was. That order is the safe one: a title containing a
/// literal `%U` is inserted as TEXT rather than being re-read as a placeholder,
/// so user data never becomes template syntax.
fn roam_create_from_template(ctx: &ExCommandContext) -> Vec<Effect> {
    let (title, key) = match &ctx.args {
        Args::List(items) => match items.as_slice() {
            [lattice::plugin_host::types::ArgValue::String(t), lattice::plugin_host::types::ArgValue::String(k)] => {
                (t.trim().to_string(), k.clone())
            }
            _ => return roam_warn("org-roam: a template needs a title and a key"),
        },
        _ => return roam_warn("org-roam: a template needs a title and a key"),
    };
    let id = match host_services::new_uuid() {
        Ok(id) => id,
        // Refuses rather than degrades, for `:org-roam-create-node`'s reason:
        // an `:ID:` outlives the session, so an empty one is worse than no note.
        Err(error) => {
            return vec![Effect::Echo(EchoPayload {
                level: EchoLevel::Error,
                text: format!("org-roam: cannot mint an id: {error}"),
            })];
        }
    };
    // OC.4's order, which is emacs's: questions first, then the buffer. The
    // fields menu is opened only when there is something to ask — a template
    // with no `%^{…}` goes straight to the draft, because routing it through a
    // menu would cost three keystrokes to collect nothing.
    match roam_draft(&title, &key, &id, &[], "") {
        Err(effect) => effect,
        Ok(draft) if draft.asks_questions => {
            vec![Effect::OpenTransient(
                lattice::plugin_host::types::OpenTransientPayload {
                    source: CAPTURE_TRANSIENT.to_string(),
                    args: Args::List(vec![
                        lattice::plugin_host::types::ArgValue::String(
                            ORG_TRANSIENT_ROAM_FIELDS.to_string(),
                        ),
                        lattice::plugin_host::types::ArgValue::String(title),
                        lattice::plugin_host::types::ArgValue::String(key),
                        lattice::plugin_host::types::ArgValue::String(id),
                    ]),
                },
            )]
        }
        Ok(draft) => draft.open(),
    }
}

/// OR.11b — the fire row of the roam fields menu.
///
/// `ctx.args` is `[title, key, id, answer…]`: the row's own three arguments
/// first, then the menu's `Argument` rows in declaration order (TR.3b). The
/// LAST answer is the body, exactly as in `capture_fields_submit` — the fields
/// menu appends a body row after the questions so `%?` is collected the same
/// way everything else is.
fn roam_capture_fields_submit(ctx: &ActionContext) -> Vec<Effect> {
    let Args::List(values) = &ctx.args else {
        return roam_warn("org-roam: the template menu collected nothing");
    };
    let mut collected = values.iter().map(|v| match v {
        lattice::plugin_host::types::ArgValue::String(s) => s.clone(),
        lattice::plugin_host::types::ArgValue::Raw(s) => s.clone(),
        lattice::plugin_host::types::ArgValue::Char(c) => c.to_string(),
        other => format!("{other:?}"),
    });
    let (Some(title), Some(key), Some(id)) = (collected.next(), collected.next(), collected.next())
    else {
        return roam_warn("org-roam: the template menu lost the note it was making");
    };
    let mut answers: Vec<String> = collected.collect();
    // The body is the last row. A menu that somehow collected nothing still
    // opens the draft — losing it would be worse than opening it bare.
    let entered = answers.pop().unwrap_or_default();
    match roam_draft(&title, &key, &id, &answers, &entered) {
        Err(effect) => effect,
        Ok(draft) => draft.open(),
    }
}

/// A roam note resolved far enough to know where it goes and what it says.
struct RoamDraft {
    name: String,
    dest: CaptureDestination,
    body: String,
    answers: Vec<String>,
    entered: String,
    /// Whether the body still holds `%^{…}` the user has not been asked.
    asks_questions: bool,
}

impl RoamDraft {
    fn open(self) -> Vec<Effect> {
        open_capture_buffer(
            self.name,
            self.dest,
            &self.body,
            &self.answers,
            &self.entered,
            // No origin: a roam create is fired from the picker or the command
            // line, not from a buffer you want a link back to. Empty is what
            // `%a` means when there is nothing to point at — the same answer
            // capture gives for a capture fired from a pathless buffer.
            "",
        )
    }
}

/// Resolve a roam note: its directory, template, filename and expanded body.
///
/// Shared by both hops so the menu and the submit cannot disagree about any of
/// them. The template is re-READ here rather than carried, so a `:set` between
/// the hops takes effect; only the id is pinned by the caller, because only the
/// id cannot be re-derived from the title.
fn roam_draft(
    title: &str,
    key: &str,
    id: &str,
    answers: &[String],
    entered: &str,
) -> Result<RoamDraft, Vec<Effect>> {
    let title = title.trim();
    if title.is_empty() {
        return Err(roam_warn("org-roam: a new note needs a title"));
    }
    let Some(dir) = roam_scan::roam_directory() else {
        return Err(roam_warn("org-roam: set `org.roam-directory` first"));
    };
    let set = roam_templates::read();
    let Some(template) = set.get(key) else {
        // The menu built its rows from this same set, so a key that is not in
        // it means the option changed between the open and the pick. Saying so
        // beats writing a note from a template the user is no longer looking at.
        return Err(roam_warn(&format!("org-roam: no template `{key}`")));
    };
    let slug = roam_find::slug(title);
    let node = roam_capture::Node {
        title,
        slug: &slug,
        id,
    };
    let body = roam_capture::expand_fields(&template.body, &node);
    let name = roam_note_filename(template, &node);
    Ok(RoamDraft {
        asks_questions: answers.is_empty()
            && entered.is_empty()
            && !capture_flow::questions(&body).is_empty(),
        name: roam_capture_buffer_name(key),
        dest: CaptureDestination::appending_to(format!("{}/{name}", dir.trim_end_matches('/'))),
        body,
        answers: answers.to_vec(),
        entered: entered.to_string(),
    })
}

fn roam_warn(text: &str) -> Vec<Effect> {
    vec![Effect::Echo(EchoPayload {
        level: EchoLevel::Warn,
        text: text.to_string(),
    })]
}

fn roam_template_menu(title: &str) -> Result<lattice::plugin_host::types::TransientSpec, String> {
    use lattice::plugin_host::types::{
        ArgValue, Args as WitArgs, TransientAction, TransientGroup, TransientItem,
        TransientItemKind, TransientSpec,
    };
    let set = roam_templates::read();
    let mut items: Vec<TransientItem> = set
        .templates
        .iter()
        .map(|t| TransientItem {
            key: vec![t.key.clone()],
            label: t.description.clone(),
            description: String::new(),
            kind: TransientItemKind::Action(TransientAction {
                command: "org-roam-create-from-template".to_string(),
                args: WitArgs::List(vec![
                    ArgValue::String(title.to_string()),
                    ArgValue::String(t.key.clone()),
                ]),
            }),
        })
        .collect();
    // A menu with no way out is a trap, and here the escape matters more than
    // usual: dismissing must leave NO note, which it does — the write is the
    // second hop and dismissing never reaches it.
    items.push(TransientItem {
        key: vec!["q".to_string()],
        label: "quit".to_string(),
        description: String::new(),
        kind: TransientItemKind::Dismiss,
    });
    Ok(TransientSpec {
        title: format!("New note: {title}"),
        groups: vec![TransientGroup {
            label: String::new(),
            items,
        }],
        // The templates the set could not use are named here rather than
        // dropped in silence — the one place a missing row is noticeable is
        // the menu the user is looking at.
        footer: (!set.skipped.is_empty()).then(|| format!("skipped: {}", set.skipped.join("; "))),
    })
}

fn todo_menu() -> Result<lattice::plugin_host::types::TransientSpec, String> {
    use lattice::plugin_host::types::{
        Args as WitArgs, TransientAction, TransientGroup, TransientItem, TransientItemKind,
        TransientSpec,
    };

    let kws = todo::parse_todo_keywords(&todo_keyword_lines().join("\n"));
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
/// MV.3 — the agenda, declared by the plugin whose feature it is.
///
/// Before this, `*agenda*`, the provider name, the reuse policy and the view's
/// modes were constants in `lattice-multibuffer`'s own agenda provider: org
/// supplied the ROWS and the host owned everything about the view. Now org owns
/// its identity and the host owns the machinery — the bounded walk, the
/// read-and-parse-once handoff, the stable sort across files, the group-run
/// computation and the progress headerline, none of which is org-specific and
/// all of which is measured.
///
/// `input: scan` because the agenda's answer genuinely requires reading every
/// candidate file's contents. Its rows still arrive through
/// `scanned-excerpt-source`; the host reads each file once and hands over text
/// AND tree, so this guest needs no filesystem capability for the agenda at all.
/// A `pull` view would have to discover and read the files itself.
///
/// `view_mode: None` deliberately — `org-agenda-mode` reaches the view through
/// the SOURCE's `view-mode` export, which is where it has always come from and
/// still works. Naming it here as well would activate it twice.
impl exports::lattice::plugin_host::multibuffer_view_source::Guest for Component {
    /// Never called for the agenda: a `scan` view's rows come from the host's
    /// walk through `scanned-excerpt-source`, not from here. An error rather
    /// than an empty result, because reaching this would mean the host drove a
    /// scan view down the pull path — a wiring fault worth seeing, not an empty
    /// agenda worth shrugging at.
    fn build(
        view: String,
        _args: Vec<String>,
    ) -> Result<lattice::plugin_host::types::MultibufferViewResult, String> {
        Err(format!(
            "org: `{view}` is a scan view; its rows come from the scan seam, not `build`"
        ))
    }
}

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
            // OA.12: and the agenda dispatcher, for TK.6's reason — it has
            // nothing to do with capture, so parsing capture-templates first
            // would answer "no capture templates" to a user who pressed the
            // agenda chord and has never configured a capture.
            if key == ORG_TRANSIENT_AGENDA {
                return agenda_menu();
            }
            // OA.18: and the VIEW dispatch, before the same parse and for the
            // same reason. It is the one menu in this list that reads no
            // option at all — its rows are fixed, because every one of them
            // names an action this plugin (or the host) already registered.
            if key == ORG_TRANSIENT_AGENDA_VIEW {
                return agenda_view_menu();
            }
        }

        // OR.11b: roam's chooser, opened FOR a node — so the discriminator and
        // the title arrive together as a list. Before the capture-templates
        // parse below, for TK.6's reason: a user with no `org.capture-templates`
        // must not be told "no capture templates" when they asked to make a
        // roam note.
        if let Args::List(items) = &ctx.args {
            if let [lattice::plugin_host::types::ArgValue::String(kind), lattice::plugin_host::types::ArgValue::String(title)] =
                items.as_slice()
            {
                if kind == ORG_TRANSIENT_ROAM {
                    return roam_template_menu(title);
                }
            }
            // OR.11b: and the roam FIELDS menu, which needs the id already
            // minted for this note as well as the title and key — carried
            // rather than re-minted, so the `${id}` the user sees in the draft
            // is the one written into the file's `:ID:`.
            if let [lattice::plugin_host::types::ArgValue::String(kind), lattice::plugin_host::types::ArgValue::String(title), lattice::plugin_host::types::ArgValue::String(key), lattice::plugin_host::types::ArgValue::String(id)] =
                items.as_slice()
            {
                if kind == ORG_TRANSIENT_ROAM_FIELDS {
                    return roam_fields_menu(title, key, id);
                }
            }
        }

        // An `err` echoes with the plugin named and the menu does not open —
        // which is right for every one of these: an unset option, a value that
        // does not fit the declared shape, and a set with nothing usable in it
        // are all things the user must fix before a menu means anything. A menu
        // that opens empty says none of that.
        let set = capture_templates::read().map_err(|e| e.message())?;

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
    use lattice::plugin_host::types::Args as WitArgs;

    let template = set
        .by_key(key)
        .ok_or_else(|| format!("no capture template keyed `{key}`"))?;

    Ok(fields_menu_spec(
        format!("Capture: {}", template.description),
        &template.body,
        format!("→ {}", template.target.file()),
        "org-capture-fields-submit",
        // The template this menu is collecting for. It arrives at the action
        // ahead of the answers (TR.3b).
        WitArgs::String(template.key.clone()),
    ))
}

/// OR.11b: the same form, for a roam note being created.
///
/// **The minted id is carried, not re-minted.** `title` and `key` alone would
/// be enough to rebuild the template — and the id would differ between the menu
/// opening and the answers arriving, so the `${id}` the user is about to see in
/// the buffer would not be the one written into the file's `:ID:`. Carrying it
/// is the same reasoning that puts the title on the roam template menu's rows
/// rather than in a guest-side slot (TR.3a): what the second hop needs travels
/// with the row that fires it.
///
/// The roam template is re-READ from the option, like capture's is, so a `:set`
/// between the two hops takes effect. Only the id is pinned, because only the
/// id cannot be re-derived.
fn roam_fields_menu(
    title: &str,
    key: &str,
    id: &str,
) -> Result<lattice::plugin_host::types::TransientSpec, String> {
    use lattice::plugin_host::types::{ArgValue, Args as WitArgs};

    let set = roam_templates::read();
    let template = set
        .get(key)
        .ok_or_else(|| format!("no roam template keyed `{key}`"))?;
    let slug = roam_find::slug(title);
    let node = roam_capture::Node {
        title,
        slug: &slug,
        id,
    };
    // `${…}` FIRST, so the questions this menu offers are the ones left in the
    // body after the node has been interpolated — and so a title that contains
    // a literal `%^{…}` cannot conjure a question out of user data.
    let body = roam_capture::expand_fields(&template.body, &node);
    Ok(fields_menu_spec(
        format!("Roam: {title}"),
        &body,
        format!("→ {}", roam_note_filename(template, &node)),
        "org-roam-capture-fields-submit",
        WitArgs::List(vec![
            ArgValue::String(title.to_string()),
            ArgValue::String(key.to_string()),
            ArgValue::String(id.to_string()),
        ]),
    ))
}

/// The rows both fields menus are: one per `%^{Question}`, one for the body,
/// one that fires and one that quits.
///
/// `carried` is prepended to the collected answers by the host (TR.3b), so the
/// submit action reads `[carried…, q0, q1, …, body]`. Capture carries one
/// string; roam carries three.
fn fields_menu_spec(
    title: String,
    body: &str,
    fire_description: String,
    command: &str,
    carried: lattice::plugin_host::types::Args,
) -> lattice::plugin_host::types::TransientSpec {
    use lattice::plugin_host::types::{
        TransientAction, TransientArgument, TransientGroup, TransientItem, TransientItemKind,
        TransientSpec,
    };

    let mut items: Vec<TransientItem> = Vec::new();
    for (i, question) in capture_flow::questions(body).iter().enumerate() {
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
    // Its answer is the final one the submit action pops off.
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
        description: fire_description,
        kind: TransientItemKind::Action(TransientAction {
            command: command.to_string(),
            args: carried,
        }),
    });
    items.push(TransientItem {
        key: vec!["q".to_string()],
        label: "quit".to_string(),
        description: String::new(),
        kind: TransientItemKind::Dismiss,
    });

    TransientSpec {
        title,
        groups: vec![TransientGroup {
            label: "Fields".to_string(),
            items,
        }],
        footer: Some("c to capture, q to abandon".to_string()),
    }
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
        let declared: Vec<(String, Vec<u32>, Option<String>)> = conceal_rules()
            .into_iter()
            .map(|r| (r.pattern, r.hide, r.slot))
            .collect();
        let (ok, errs) = compile_rules(&declared, None);
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
        let mut declared: Vec<(String, Vec<u32>, Option<String>)> = conceal_rules()
            .into_iter()
            .map(|r| (r.pattern, r.hide, r.slot))
            .collect();
        declared.reverse();
        let (reversed, _) = compile_rules(&declared, None);
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

    /// **OL.1: the part of a link that stays on screen is styled.**
    ///
    /// Reported as links rendering plain. Conceal made that worse rather than
    /// milder — it hides the brackets and the target, so what remains is bare
    /// prose with nothing saying `<CR>` under the cursor would go anywhere.
    ///
    /// Asserted DISJOINT from the concealed ranges, which is the whole point:
    /// a style covering only the hidden bytes would look correct in a span
    /// dump and paint nothing at all.
    #[test]
    fn ol1_the_visible_part_of_a_link_is_styled() {
        use lattice_syntax::conceal::conceal_style_spans;
        let rules = compiled();

        let line = "see [[id:6F39][Project Kickoff]] ok";
        let hidden = conceal_spans(&rules, line);
        let styled = conceal_style_spans(&rules, line);
        assert_eq!(styled.len(), 1, "one styled run: {styled:?}");
        let (s, e, style) = styled[0];
        assert_eq!(
            &line[s as usize..e as usize],
            "Project Kickoff",
            "the DESCRIPTION survives conceal, so it is what gets styled"
        );
        assert_eq!(style, lattice_syntax::Style::Link);
        for (hs, he) in &hidden {
            assert!(
                e <= *hs || s >= *he,
                "styled {s}..{e} overlaps hidden {hs}..{he} — it would paint \
                 bytes that are about to disappear"
            );
        }

        // The bare form styles the target, which is what it shows.
        let line = "see [[https://example.com]] ok";
        let styled = conceal_style_spans(&rules, line);
        assert_eq!(styled.len(), 1, "{styled:?}");
        let (s, e, style) = styled[0];
        assert_eq!(&line[s as usize..e as usize], "https://example.com");
        assert_eq!(style, lattice_syntax::Style::Url);

        // Prose is untouched — the rules match links, not bracket-ish text.
        assert!(conceal_style_spans(&rules, "no links on this line").is_empty());
    }
}

#[cfg(test)]
mod agenda_files_tests {
    use super::agenda_files;

    /// The two shapes one list carries — a directory and a single file — plus
    /// the annotation people add to configuration they will re-read in six
    /// TC.7 made `org.agenda-files` a `list<string>`, and most of what these
    /// tests covered went with the line format.
    ///
    /// "One path per line, blanks and `#` comments dropped" was a rule this
    /// module implemented because a newline-joined string is the only container
    /// a string option has. A list has elements; an element that is not there
    /// is not an element, and a comment lives beside the array in the TOML
    /// where it is a comment rather than something to parse around. What is
    /// left is trimming, which a config file will always want.
    ///
    /// The separator test went too, and its disappearance is the point: it
    /// existed because a path may contain a colon or a comma, so splitting on
    /// either would silently cut one in half. A list cannot have that bug.
    #[test]
    fn blank_elements_are_dropped_and_the_rest_are_trimmed() {
        assert_eq!(
            string_list_of(&["  ~/org  ", "", "   ", "~/org/anniversaries.org"]),
            vec!["~/org".to_string(), "~/org/anniversaries.org".to_string(),]
        );
    }

    /// Unset, or nothing but blanks, is "no opinion" — NOT "scan nothing". The
    /// host falls back to the project root, which is what keeps a user who has
    /// configured nothing on exactly the old behaviour.
    #[test]
    fn an_empty_option_is_no_opinion() {
        assert!(string_list_of(&[]).is_empty());
        assert!(string_list_of(&["", "   "]).is_empty());
    }

    /// A path containing a colon or a comma survives, because a list element
    /// is never split. The old line format needed a test for this; a list
    /// cannot get it wrong, and the assertion is kept as the record of why the
    /// separator question is closed.
    #[test]
    fn a_path_may_contain_what_a_separator_based_format_would_have_cut() {
        assert_eq!(
            string_list_of(&["/tmp/notes: drafts", "/tmp/a,b/notes.org"]),
            vec![
                "/tmp/notes: drafts".to_string(),
                "/tmp/a,b/notes.org".to_string()
            ]
        );
    }

    /// `string_list`'s resolution without a host to read an option from.
    fn string_list_of(items: &[&str]) -> Vec<String> {
        items
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod local_clock_tests {
    use super::epoch_day_from_local_secs;

    /// The reported bug, as arithmetic.
    ///
    /// 2026-09-05 00:30 at GMT+5:30 is 2026-09-04 19:00 UTC. Dividing the UTC
    /// seconds — which `today_epoch_day` did — anchors the agenda to the 4th
    /// while the user's wall clock says the 5th. Feeding the LOCAL instant is
    /// the whole fix, so this pins the boundary rather than the offset
    /// plumbing (which is a host call the guest cannot stub).
    #[test]
    fn the_local_day_wins_east_of_greenwich() {
        let utc_evening = 1_788_548_400_i64; // 2026-09-04 19:00:00 UTC
        let offset = 5 * 3600 + 30 * 60; // +05:30
        let utc_day = utc_evening.div_euclid(86_400);
        let local_day = epoch_day_from_local_secs(utc_evening + offset);
        assert_eq!(
            local_day,
            utc_day + 1,
            "past local midnight the agenda must have rolled to the next day; \
             equal days here is the bug — the agenda opening on yesterday"
        );
    }

    /// And the mirror case, which the same fix has to not break: west of
    /// Greenwich the local day is BEHIND UTC in the evening.
    #[test]
    fn the_local_day_also_wins_west_of_greenwich() {
        let utc_early = 1_788_570_600_i64; // 2026-09-05 01:10:00 UTC
        let offset = -(8 * 3600); // -08:00
        let utc_day = utc_early.div_euclid(86_400);
        let local_day = epoch_day_from_local_secs(utc_early + offset);
        assert_eq!(
            local_day,
            utc_day - 1,
            "it is still the previous evening in California"
        );
    }

    /// `div_euclid`, not `/`. A negative local instant is only reachable
    /// pre-1970, but truncating division rounds toward zero there and would
    /// put 1969-12-31 in 1970.
    #[test]
    fn days_below_the_epoch_round_downward() {
        assert_eq!(epoch_day_from_local_secs(-1), -1);
        assert_eq!(epoch_day_from_local_secs(-86_400), -1);
        assert_eq!(epoch_day_from_local_secs(-86_401), -2);
        assert_eq!((-1_i64) / 86_400, 0, "the truncating division this avoids");
    }
}

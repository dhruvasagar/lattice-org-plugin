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
        }
    "#,
    path: "wit",
    world: "org-plugin",
    generate_all,
});

mod agenda;
mod archive;
mod checkbox;
mod headline;
mod links;
mod table;
mod timestamp;
mod todo;

use exports::lattice::plugin_host::grammar_callbacks::Guest as GrammarCallbacks;
use exports::lattice::plugin_host::media::Guest as MediaProducer;
use lattice::plugin_host::buffer::Document;
use lattice::plugin_host::config::{get_option, register_option, OptionType};
use lattice::plugin_host::grammar::{register_action, register_motion, register_text_object};
use lattice::plugin_host::help::register_topic;
use lattice::plugin_host::language::{register_language, LanguageSpec};
use lattice::plugin_host::modes::{
    register_mode, ActivationPolicy, BindingMode, ModeCapabilities, ModeDeclaration,
    ModeKeymapBinding, ModeKind,
};
use lattice::plugin_host::tree_sitter::TreeSnapshot;
use lattice::plugin_host::types::{
    ActionContext, ActionSpec, AppEffect, Args, DecorationContext, EchoLevel, EchoPayload, Edit,
    EditKind, Effect, ExCommandContext, FileAnchor, MediaBlock, MediaFit, MotionContext,
    MotionResult, MotionSpec, OperatorContext, Position, Range, TextObjectContext, TextObjectSpec,
    WriteToFilePayload,
};

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

impl Guest for Component {
    /// OM.7's options. Auto-namespaced by the host to `org.*`, so these are
    /// `org.todo-keywords` and `org.highest-priority` to the user.
    fn register_options() {
        let _ = register_option(
            "todo-keywords",
            OptionType::String,
            DEFAULT_TODO_KEYWORDS,
            "TODO keywords `<leader>ot` cycles through, in order. `|` separates \
             not-done from done states and is ignored when cycling.",
        );
        let _ = register_option(
            "highest-priority",
            OptionType::String,
            DEFAULT_HIGHEST_PRIORITY,
            "The last priority letter `<leader>o,` cycles to. `C` gives A, B, C.",
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
            highlights: Some(include_str!("../queries/highlights.scm").to_string()),
            folds: Some(include_str!("../queries/folds.scm").to_string()),
            injections: None,
            indents: None,
            textobjects: None,
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
                bind("<leader>o,", "org-priority-cycle"),
                bind("<leader>o:", "org-set-tags"),
            ],
            target_language: None,
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
        register_action(
            "org-archive-subtree",
            "Move the subtree at the cursor into `<this file>_archive`",
            &spec(),
            ARCHIVE_SUBTREE,
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
    fn begin() {
        let today = today_epoch_day();
        let keywords = agenda::Keywords::from_spec(
            &get_option("todo-keywords").unwrap_or_else(|| DEFAULT_TODO_KEYWORDS.to_string()),
        );
        // Single-threaded guest, one actor, calls serialised by the host's
        // per-plugin channel — so a `thread_local` IS the whole of the
        // synchronisation story, and `begin`-then-`scan` ordering is a host
        // guarantee rather than something the guest has to defend.
        SCAN.set(Some(ScanState { today, keywords }));
    }

    fn scan(_path: String, text: String) -> Result<Vec<Entry>, String> {
        // `begin` is contractually called first. Refusing rather than
        // defaulting makes a host that stops calling it fail loudly on the
        // first file instead of producing a silently mis-dated agenda.
        SCAN.with_borrow(|state| {
            let Some(state) = state.as_ref() else {
                return Err("org: scan before begin".to_string());
            };
            Ok(agenda::scan_file(&text, &state.keywords)
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
fn shift(ctx: &ActionContext, doc: &Document, delta: isize, whole_subtree: bool) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let Some((start, _level)) = headline::enclosing_headline(line, ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let end = if whole_subtree {
        headline::subtree_end(line, start, doc.line_count())
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
fn move_subtree(ctx: &ActionContext, doc: &Document, up: bool) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let count = doc.line_count();
    let Some((start, level)) = headline::enclosing_headline(line, ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let end = headline::subtree_end(line, start, count);

    // `first` and `second` are the two spans in DOCUMENT order; the edit
    // rewrites them swapped. Naming them by position rather than by
    // "mine"/"theirs" is what lets one body serve both directions.
    let (first, first_end, second, second_end) = if up {
        let Some(prev) = headline::prev_sibling(line, start, level) else {
            return vec![Effect::None];
        };
        (prev, start - 1, start, end)
    } else {
        let Some(next) = headline::next_sibling(line, start, level, count) else {
            return vec![Effect::None];
        };
        (start, end, next, headline::subtree_end(line, next, count))
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
fn archive_subtree(ctx: &ActionContext, doc: &Document) -> Vec<Effect> {
    let Some(source) = doc.path() else {
        return vec![Effect::Echo(EchoPayload {
            level: EchoLevel::Warn,
            text: "org: this buffer has no file, so it has no archive".to_string(),
        })];
    };
    let line = |n: u32| doc.line(n);
    let Some((text, (sl, sb, el, eb))) =
        archive::extract_subtree(line, ctx.cursor.line, doc.line_count())
    else {
        return vec![Effect::None];
    };

    vec![Effect::WriteToFile(WriteToFilePayload {
        path: archive::archive_path(&source),
        // Append. Org's archive file is a log, and the newest entry belonging
        // at the bottom is what makes it readable top-to-bottom later.
        anchor: FileAnchor::End,
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
fn meta_return(ctx: &ActionContext, doc: &Document) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let Some((start, level)) = headline::enclosing_headline(line, ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let end = headline::subtree_end(line, start, doc.line_count());
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
fn toggle_heading(ctx: &ActionContext, doc: &Document) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let at = ctx.cursor.line;
    let Some(text) = doc.line(at) else {
        return vec![Effect::None];
    };
    let level = headline::enclosing_headline(line, at).map_or(1, |(_, lvl)| lvl);
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
    f: impl FnOnce(&str, &[String]) -> Option<String>,
) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let Some((start, _)) = headline::enclosing_headline(line, ctx.cursor.line) else {
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
fn set_tags_prompt(ctx: &ActionContext, doc: &Document) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let Some((start, _)) = headline::enclosing_headline(line, ctx.cursor.line) else {
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
        _tree: Option<&TreeSnapshot>,
    ) -> Result<Vec<Effect>, String> {
        match callback {
            PROMOTE_HEADLINE => Ok(shift(&ctx, doc, -1, false)),
            DEMOTE_HEADLINE => Ok(shift(&ctx, doc, 1, false)),
            PROMOTE_SUBTREE => Ok(shift(&ctx, doc, -1, true)),
            DEMOTE_SUBTREE => Ok(shift(&ctx, doc, 1, true)),
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
                let on_headline = line(ctx.cursor.line)
                    .as_deref()
                    .and_then(headline::headline_level)
                    .is_some();
                if on_headline {
                    Ok(vec![Effect::AppAction(AppEffect::CycleFoldAtCursor)])
                } else {
                    Ok(vec![Effect::Declined])
                }
            }
            // `<S-Tab>` is whole-buffer, so it does not decline: org's global
            // cycle is meaningful wherever the cursor is.
            CYCLE_GLOBAL => Ok(vec![Effect::AppAction(AppEffect::CycleFoldsGlobal)]),
            // OM.6.
            MOVE_SUBTREE_UP => Ok(move_subtree(&ctx, doc, true)),
            MOVE_SUBTREE_DOWN => Ok(move_subtree(&ctx, doc, false)),
            META_RETURN => Ok(meta_return(&ctx, doc)),
            TOGGLE_HEADING => Ok(toggle_heading(&ctx, doc)),
            ARCHIVE_SUBTREE => Ok(archive_subtree(&ctx, doc)),
            TOGGLE_CHECKBOX => Ok(toggle_checkbox(&ctx, doc)),
            TABLE_NEXT_CELL => Ok(table_move(&ctx, doc, 1)),
            TABLE_PREV_CELL => Ok(table_move(&ctx, doc, -1)),
            TABLE_ALIGN => Ok(table_move(&ctx, doc, 0)),
            TABLE_ROW_UP..=TABLE_DELETE_COL => Ok(table_structure(&ctx, doc, callback)),
            OPEN_LINK => Ok(open_link(&ctx, doc)),
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
            TODO_CYCLE => Ok(rewrite_headline(&ctx, doc, |line, kw| {
                todo::cycle_keyword(line, kw, true)
            })),
            TODO_CYCLE_BACK => Ok(rewrite_headline(&ctx, doc, |line, kw| {
                todo::cycle_keyword(line, kw, false)
            })),
            PRIORITY_CYCLE => {
                let highest = highest_priority();
                Ok(rewrite_headline(&ctx, doc, move |line, kw| {
                    todo::cycle_priority(line, kw, highest, true)
                }))
            }
            SET_TAGS => Ok(set_tags_prompt(&ctx, doc)),
            SET_TAGS_SUBMIT => {
                let Some(tags) = submitted_text(&ctx.args) else {
                    return Ok(vec![Effect::None]);
                };
                Ok(rewrite_headline(&ctx, doc, move |line, kw| {
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
    ) -> Result<MotionResult, String> {
        let line = |n: u32| doc.line(n);
        let count = ctx.count.max(1);
        let mut at = ctx.from.line;
        for _ in 0..count {
            let next = match callback {
                NEXT_HEADLINE => headline::next_headline(line, at, doc.line_count()),
                PREV_HEADLINE => headline::prev_headline(line, at),
                PARENT_HEADLINE => headline::parent_headline(line, at),
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
    ) -> Result<Range, String> {
        let line = |n: u32| doc.line(n);
        let (start, _level) = headline::enclosing_headline(line, ctx.at.line)
            .ok_or("org: no headline at or above the cursor")?;
        let head = line(start).ok_or("org: headline vanished mid-read")?;
        let stars = headline::headline_level(&head).ok_or("org: not a headline")?;

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
                let end = headline::subtree_end(line, start, doc.line_count());
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
                let end = headline::subtree_end(line, start, doc.line_count());
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
    fn parse_ex_args(_c: u32, _rest: String, _bang: bool) -> Result<Args, String> {
        Err("org: no ex-commands".into())
    }
    fn apply_ex_command(_c: u32, _ctx: ExCommandContext) -> Result<Vec<Effect>, String> {
        Err("org: no ex-commands".into())
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
fn toggle_checkbox(ctx: &ActionContext, doc: &Document) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let at = ctx.cursor.line;
    let Some(text) = doc.line(at) else {
        return vec![Effect::None];
    };
    let Some(item) = checkbox::parse_item(&text) else {
        return vec![Effect::None];
    };
    let Some(flipped) = checkbox::set_state(&text, checkbox::toggled(item.state)) else {
        return vec![Effect::None];
    };

    let count = doc.line_count();
    // Rewrite from the toggled line down to itself, then extend upward for
    // each ancestor whose cookie changes. The span is contiguous because an
    // ancestor is always above its children.
    let mut rewritten: Vec<(u32, String)> = vec![(at, flipped)];
    let mut current_indent = item.indent;
    let mut scan = at;
    while scan > 0 {
        scan -= 1;
        let Some(above) = line(scan) else { break };
        if above.trim().is_empty() {
            continue;
        }
        let indent = above.len() - above.trim_start().len();
        let is_headline = headline::headline_level(&above).is_some();
        // An ancestor is a shallower item, or a headline (which owns the
        // whole list beneath it whatever its indent).
        if !is_headline && indent >= current_indent {
            continue;
        }
        let parent_indent = if is_headline { 0 } else { indent };
        // Tally against the buffer AS IT WILL BE — the toggled line included —
        // or the cookie would lag one keypress behind the box.
        let after = |n: u32| -> Option<String> {
            rewritten
                .iter()
                .find(|(i, _)| *i == n)
                .map(|(_, t)| t.clone())
                .or_else(|| line(n))
        };
        let (done, total) = checkbox::tally(after, scan, parent_indent, count);
        if let Some(updated) = checkbox::update_cookie(&above, done, total) {
            if updated != above {
                rewritten.push((scan, updated));
            }
        }
        // A parent item may itself be a child of something shallower; a
        // headline is the top, so stop there.
        if is_headline {
            break;
        }
        current_indent = indent;
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
/// Three destinations, three effects: a file opens as a buffer, a URL goes to
/// the system handler, and an internal `[[*Headline]]` moves the cursor.
///
/// The internal case searches THIS buffer only, which is what org means by
/// `*Headline`, and matches the title exactly — a fuzzy match that jumped to
/// the wrong heading would be worse than not jumping. A reference that
/// resolves to nothing echoes rather than moving the cursor somewhere
/// arbitrary.
fn open_link(ctx: &ActionContext, doc: &Document) -> Vec<Effect> {
    let Some(text) = doc.line(ctx.cursor.line) else {
        return vec![Effect::None];
    };
    let Some(link) = links::link_at(&text, ctx.cursor.byte as usize) else {
        return vec![Effect::None];
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
fn table_move(ctx: &ActionContext, doc: &Document, delta: i32) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let count = doc.line_count();
    let Some((first, last)) = table::table_bounds(line, ctx.cursor.line, count) else {
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
fn table_structure(ctx: &ActionContext, doc: &Document, action: u32) -> Vec<Effect> {
    let line = |n: u32| doc.line(n);
    let count = doc.line_count();
    let Some((first, last)) = table::table_bounds(line, ctx.cursor.line, count) else {
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

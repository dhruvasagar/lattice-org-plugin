//! OM.3 — org promotes and demotes headlines, from inside a plugin.
//!
//! The first slice where the org plugin EDITS. Each chord runs the whole
//! route: `<leader>oh` → the org-mode major's own keymap layer → `Action::Invoke`
//! → the sync grammar trampoline → the guest's `apply-action`, which reads the
//! buffer through its `borrow<document>` handle and returns
//! `Effect::ApplyEdit` → the host's ordinary edit path.
//!
//! Nothing in `lattice-host` knows what a headline is.
//!
//! ## What is asserted, and why through dispatch
//!
//! The unit tests in `src/headline.rs` cover the line logic on the host
//! target. These are the better test: they exercise the seam, the keymap
//! layer, the leader expansion and the edit path, not just the arithmetic.
//!
//! Skips when the component was not built (the `org_folds.rs` precedent) —
//! `cargo test` builds the crate for the HOST, and the component these tests
//! load is a separate `--target wasm32-wasip2 --release` artefact.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_keymap::LookupResult;
use lattice_mode::ModeId;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::{parse_chord_sequence, KeyChord};

/// Boot an editor sealed off from the developer's real `~/.config/lattice`.
///
/// `Editor::boot` starts plugin auto-discovery on a spawned task, which scans
/// `~/.config/lattice/plugins/`. Once org is installed there for real use, that
/// task loads a SECOND copy of this plugin into the same registries the test
/// loads its own copy into — a duplicate-registration race that only makes
/// progress at an `.await`. See the longer note in `org_major_mode.rs`.
fn boot_sealed_editor() -> Editor {
    lattice_plugin_loader::disable_autoload();
    Editor::boot(CoreDocument::from_text("scratch\n"))
}

fn org_plugin_wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    );
    std::fs::read(path).ok()
}

fn loader_over_editor(editor: &Editor, base: &std::path::Path) -> PluginLoader {
    let host = Arc::new(
        PluginHost::with_dirs(base.join("cache"), base.join("data")).expect("host builds"),
    );
    PluginLoader::with_services(
        host,
        LoaderServices {
            runtime: Some(tokio::runtime::Handle::current()),
            bus: Some(editor.event_bus.clone()),
            command_registry: Some(editor.registry.clone()),
            mode_registry: Some(editor.mode_registry.clone()),
            keymap: Some(editor.keymap.clone()),
            help_topics: Some(editor.help_topics.clone()),
            config_registry: Some(editor.config.clone()),
            // IM.6b: the media seam fails the whole load when unwired, by
            // design — a plugin declaring `media` whose images can never be
            // drawn should say so rather than load silently half-working.
            media_registry: Some(std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                lattice_mode::MediaSourceRegistry::new(),
            ))),
            // OM.11: refile's target list is a picker source, so the loader
            // needs somewhere to register it.
            picker_registry: Some(editor.picker_registry.clone()),
            // OC.3: the capture menu registers into the editor's OWN transient
            // registry — the one TR.1 moved to `editor_boot` so a plugin menu
            // does not depend on whether magit happened to load.
            transient_registry: editor
                .services
                .get::<lattice_picker::TransientSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            // MV.3: the agenda is a plugin-owned view now, so the loader
            // needs somewhere to register its opener and somewhere to put its
            // excerpts. Both come off the editor's own service registry —
            // absent, the seam is `NotWired` and the WHOLE plugin fails to
            // load, which is how this surfaced.
            provider_view_registry: editor
                .services
                .get::<lattice_mode::ProviderViewRegistryHandle>()
                .map(|h| (*h).clone()),
            multibuffer_registry: editor
                .services
                .get::<lattice_multibuffer::registry::MultibufferRegistryHandle>()
                .map(|h| (*h).clone()),
            ..Default::default()
        },
    )
}

/// Boot an editor, load the org plugin into its live registries, and open a
/// `.org` file holding `text`. Returns the editor and the file path.
async fn org_editor(base: &std::path::Path, text: &str) -> Editor {
    org_editor_with_caps(base, text, &[]).await
}

/// [`org_editor`], plus the OS capabilities the manifest declares.
///
/// OM.6b is the first slice that needs any: a cross-file write is refused at
/// the boundary unless the plugin holds `fs:write` over the target, and the
/// refusal is silent to the guest (the effect is replaced with an `Echo`
/// before it reaches the editor). A test that forgot the grant would look
/// exactly like a broken archive.
async fn org_editor_with_caps(
    base: &std::path::Path,
    text: &str,
    capabilities: &[String],
) -> Editor {
    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    let caps = capabilities
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            // OT.4: `editor_capabilities` mirrors the shipped manifest. Without
            // the `tree-sitter` grant the trampoline hands every seam a `none`
            // tree and the guest falls back to line matching — so a harness
            // that omitted it would test the text path while claiming to test
            // the tree one.
            "id = \"org\"\nprovides = [\"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\", \"picker-source\", \"transient-source\", \"events\"]\ndefault_modes = [\"org-todo-mode\", \"org-global-mode\", \"org-table-mode\"]\neditor_capabilities = [\"tree-sitter\"]\ncapabilities = [{caps}]\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("component.wasm"),
        org_plugin_wasm().expect("caller checked"),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base)
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    // OM.4b: boot subscribes to `PluginLoaded` and expands a plugin mode's
    // motion / text-object bindings into operator rows on a spawned task. That
    // is right in production and a race in a test that dispatches immediately,
    // so drive the same function directly here. The event-driven wiring has its
    // own test below rather than every test racing it.
    {
        let commands = editor.registry.load();
        for (mode_id, kind) in editor.mode_registry.load().iter_meta() {
            let layer = match kind {
                lattice_mode::ModeKind::Major => lattice_keymap::KeymapLayer::MajorMode(mode_id),
                lattice_mode::ModeKind::Minor => lattice_keymap::KeymapLayer::MinorMode(mode_id),
            };
            lattice_host::keymap_normal::expand_plugin_mode_grammar_rows(
                &editor.keymap,
                &commands,
                &editor.builtins,
                layer,
            );
        }
    }

    // OM.7: `org-todo-mode` needs TWO per-tick drains, in this order, and both
    // are races against a test that dispatches immediately.
    //
    //   1. The manifest's `default_mode` makes the loader publish
    //      `ModeEnablementRequested`, and a plugin minor is INERT until that
    //      lands — `auto_activatable_minors` filters on enablement (CI.3).
    //   2. Opening the file publishes `MajorEntered`, and the minor is
    //      activated from THAT, against the buffer's now-org major.
    //
    // Enablement must precede the open or step 2 finds the mode still
    // disabled; activation must follow it or the policy sees an empty major.
    // `run_tick_pending` is the aggregator both drains hang off, so call it
    // rather than hand-picking, and call it on both sides of the open.
    editor.run_tick_pending();

    let file = base.join("notes.org");
    std::fs::write(&file, text).unwrap();
    editor.do_edit(Some(file), false);
    editor.run_tick_pending();
    assert_eq!(
        editor
            .active_modes
            .get(&editor.document_buffer_id)
            .and_then(|m| m.major()),
        Some(ModeId::new("org-mode")),
        "the buffer is in org-mode before any chord is dispatched"
    );
    editor
}

// `enable_minor` lived here and every table test called it first. It is gone
// with `org-table-mode`'s addition to the manifest's `default_modes`, and the
// deletion is the point rather than tidying: a test that enables the mode by
// hand passes against the broken product too, which is exactly why eleven dead
// chords survived this file. The table tests below now reach `<Tab>` the way a
// user does, through enablement the manifest asked for.

fn chord(s: &str) -> KeyChord {
    parse_chord_sequence(s)
        .expect("parseable chord")
        .into_iter()
        .next()
        .expect("one chord")
}

/// Dispatch a multi-key sequence written with `<leader>`, expanding it the same
/// way the binding did — so the test types what the user types.
fn press(editor: &mut Editor, keys: &str) {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial: Vec<KeyChord> = Vec::new();
    for c in seq {
        let _ = editor.dispatch_chord(c, &mut partial);
    }
}

/// OC.3 — press a chord and then apply the RENDERER-owned effects it produced,
/// which is what both peers do after `dispatch_chord` returns.
///
/// `dispatch_chord` runs the action but discards its `DispatchOutcome`, so a
/// headless test that only presses proves the chord resolved and nothing more.
/// `Effect::OpenTransient` and `Effect::OpenPrompt` both live in that discarded
/// tail — which is exactly how OC.3a's bug (a plugin's prompt submit never
/// firing) survived every existing capture test.
async fn apply_renderer_effects(editor: &mut Editor, out: lattice_host::dispatch::DispatchOutcome) {
    for effect in out.effects {
        match effect {
            lattice_grammar::Effect::OpenTransient { source, args } => {
                editor.open_named_transient(source, args);
                // A PLUGIN menu builds off-thread — the seam calls the guest's
                // `build` on its own actor task — so it parks and seats on the
                // async-landed wake (TR.2a). In production the editor actor
                // drains it; here the test does what that arm does.
                settle_transient_build(editor).await;
            }
            lattice_grammar::Effect::OpenPrompt {
                prompt,
                initial,
                on_submit_action,
                buffer_name,
            } => {
                editor.open_prompt_line(prompt, initial, on_submit_action, buffer_name);
            }
            // TK.6: an edit an action produced is pushed onto `out.effects`
            // for the RENDERER to re-dispatch (`Action::ApplyEdit`, whose
            // body is host-resident) rather than applied where it was
            // produced — the deferral `apply_write_to_file` documents.
            //
            // This helper had no arm for it, so every test driving an edit
            // through a path that returns rather than applies saw the buffer
            // unchanged and read as a product bug. TK.6's menu rows are the
            // first to hit it: `press()` never did, because `dispatch_chord`
            // applies on the way through.
            lattice_grammar::Effect::ApplyEdit {
                target,
                edit,
                cursor,
            } => {
                let out = editor.dispatch(lattice_host::action::Action::ApplyEdit {
                    target,
                    edit,
                    cursor,
                });
                let _ = out;
            }
            _ => {}
        }
    }
}

/// Fire the action a chord is bound to, the way the renderer's dispatch
/// wrapper does — run it, then apply the renderer-coupled effects.
/// Drain a parked transient build until the menu seats.
///
/// A LOOP, not a single wait: `async_landed` is the editor's one shared wake
/// and anything else that lands fires it too, so one `notified()` can return
/// for an unrelated reason and leave the build still in flight — which is
/// exactly what happened the first time this was written as a single await.
async fn settle_transient_build(editor: &mut Editor) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while editor.pending_transient_build.is_some() && std::time::Instant::now() < deadline {
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            editor.async_landed.notified(),
        )
        .await;
        editor.drain_pending_transient_build();
    }
}

async fn press_chord(editor: &mut Editor, keys: &str) {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial: Vec<KeyChord> = Vec::new();
    let mut resolved = None;
    for c in seq {
        resolved = Some(editor.dispatch_chord(c, &mut partial));
    }
    // `dispatch_chord` already RAN the action; re-running it through
    // `dispatch` would double-apply. Instead the effects are recovered by
    // re-dispatching only when the resolved action is an invocation, which is
    // the only shape that carries them here.
    if let Some(lattice_host::action::Action::Invoke(inv)) = resolved {
        let out = editor.dispatch(lattice_host::action::Action::Invoke(inv));
        apply_renderer_effects(editor, out).await;
    }
}

/// Press a key inside an open transient menu — the renderer's own call.
async fn press_menu_key(editor: &mut Editor, key: &str) {
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_transient_trigger(key.to_string(), &mut out);
    apply_renderer_effects(editor, out).await;
}

/// Press a field's key and answer its prompt — the menu parks, the prompt
/// takes the value, and `resume_parked_transient` puts the menu back.
async fn answer_field(editor: &mut Editor, key: &str, value: &str) {
    press_menu_key(editor, key).await;
    // The prompt the field opened registers no submit action: the host routes
    // a parked transient itself (`do_prompt_line_submit` checks for one first).
    editor.open_prompt_line(
        String::new(),
        value.to_string(),
        String::new(),
        editor.pending_prompt_buffer_name.clone(),
    );
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_prompt_line_submit(&mut out);
    apply_renderer_effects(editor, out).await;
}

/// Type `text` into the open prompt and submit it, the way `<CR>` does.
fn submit_prompt(editor: &mut Editor, text: &str) {
    let action = editor
        .pending_prompt_submit_action
        .clone()
        .expect("a prompt is open");
    let name = editor.pending_prompt_buffer_name.clone();
    // Re-open with the text seeded: `open_prompt_line` writes `initial` into
    // the prompt buffer and submit reads that buffer's first line, so this is
    // the value the user would have typed.
    editor.open_prompt_line("".to_string(), text.to_string(), action, name);
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_prompt_line_submit(&mut out);
}

/// Put the caret on `line`, column 0.
fn goto_line(editor: &mut Editor, line: u32) {
    editor.cursor.line = line;
    editor.cursor.byte = 0;
}

fn text(editor: &Editor) -> String {
    editor.document.snapshot().text().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demote_and_promote_a_single_headline() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\nbody\n** Child\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>ol");
    assert_eq!(
        text(&editor),
        "** One\nbody\n** Child\n",
        "demote moved only the headline — the child is untouched"
    );

    press(&mut editor, "<leader>oh");
    assert_eq!(
        text(&editor),
        "* One\nbody\n** Child\n",
        "promote is the inverse"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demote_a_subtree_moves_every_headline_under_it() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\nbody\n** Child\nkid body\n* Two\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oL");
    assert_eq!(
        text(&editor),
        "** One\nbody\n*** Child\nkid body\n* Two\n",
        "the subtree moved together and stopped at the next level-1"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subtree_edit_is_one_undo_step() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* One\nbody\n** Child\n*** Grand\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oL");
    assert_eq!(text(&editor), "** One\nbody\n*** Child\n**** Grand\n");

    // The whole span is replaced as ONE edit precisely so this holds: a single
    // `u` puts every star back, not one headline at a time.
    press(&mut editor, "u");
    assert_eq!(
        text(&editor),
        original,
        "one undo reverses the whole subtree demote"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn promoting_works_from_inside_the_subtree_not_just_on_the_headline() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\n** Child\nkid body\n").await;

    // Caret in the child's BODY — the enclosing headline is what moves. This is
    // the common case in practice; the caret is rarely on the headline itself.
    goto_line(&mut editor, 2);
    press(&mut editor, "<leader>oh");
    assert_eq!(
        text(&editor),
        "* One\n* Child\nkid body\n",
        "the enclosing headline promoted, not the one under the caret"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn promoting_past_level_one_refuses_rather_than_flattening() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* One\n** Child\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oH");
    assert_eq!(
        text(&editor),
        original,
        "a subtree promote whose root is level 1 is refused whole — shifting \
         only the children would make Child a sibling of One"
    );

    // And the single-headline form refuses too, rather than turning `* One`
    // into body text.
    press(&mut editor, "<leader>oh");
    assert_eq!(text(&editor), original);
}

/// A refused shift consumes the key and moves NOTHING — not the text, and not
/// the caret.
///
/// The caret is the assertion that matters. These chords end in `h` / `l` /
/// `H` / `L`, so while they returned `Effect::Declined` the dispatcher
/// re-resolved the sequence with org's layer removed and ran that trailing key
/// on its own: a refused `<leader>ol` moved the cursor right. A text-only
/// assertion could not see it, which is how it survived OM.3. `<leader>oJ`'s
/// trailing `J` was the same bug with a loud symptom (it joined two lines).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_shift_moves_neither_text_nor_caret() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    // Level 1: promoting is refused, so `<leader>oh` / `<leader>oH` have
    // nothing to do.
    let original = "* One
** Child
";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 0);
    editor.cursor.byte = 2;
    for chord in ["<leader>oh", "<leader>oH", "<leader>ol", "<leader>oL"] {
        let before = editor.cursor;
        let text_before = text(&editor);
        if chord.ends_with('l') || chord.ends_with('L') {
            // Demote succeeds, so undo it and only check the refusals above.
            press(&mut editor, chord);
            press(&mut editor, "u");
            assert_eq!(text(&editor), text_before, "{chord}: undo restored");
            continue;
        }
        press(&mut editor, chord);
        assert_eq!(text(&editor), text_before, "{chord}: text untouched");
        assert_eq!(
            (editor.cursor.line, editor.cursor.byte),
            (before.line, before.byte),
            "{chord}: a refused shift must not move the caret either"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_buffer_with_no_headline_leaves_the_chord_to_fall_through() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    // A preamble with no headline anywhere above the caret.
    let original = "#+TITLE: Notes\njust prose\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 1);
    press(&mut editor, "<leader>ol");
    assert_eq!(
        text(&editor),
        original,
        "the action declined; no edit, and nothing on the undo stack"
    );
}

/// Correctness at scale. The interesting property here is what the plugin does
/// NOT do: `shift` reads lines one at a time through the `document` handle
/// rather than materialising the buffer, so a headline near the top of a long
/// file costs the handful of reads between the caret and its headline, not one
/// guest→host call per line. (The read-count bound itself is pinned by
/// `headline.rs`'s own unit test, which runs on the host target; this asserts
/// the behaviour that bound has to preserve.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_file_promotes_correctly_at_its_far_end() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut body = String::from("* One\n");
    for i in 0..5_000 {
        body.push_str(&format!("body {i}\n"));
    }
    body.push_str("** Last\ntail\n");
    let mut editor = org_editor(base.path(), &body).await;

    // Caret in the final subtree's body, 5000 lines below the first headline.
    let last = 5_002;
    goto_line(&mut editor, last);
    press(&mut editor, "<leader>ol");

    let out = text(&editor);
    assert!(
        out.starts_with("* One\nbody 0\n"),
        "the top of the file is untouched"
    );
    assert!(
        out.ends_with("* Last\ntail\n"),
        "the enclosing headline at the far end promoted; got tail: {:?}",
        &out[out.len().saturating_sub(40)..]
    );
}

// ── OM.4: headline motions ────────────────────────────────────────
//
// Two things these tests deliberately do NOT cover, both for reasons found
// while writing them rather than assumed:
//
// * **Counts** (`3]]`). Count accumulation lives at the App layer, above
//   `Editor::dispatch_chord` — a control assertion here showed `3j` on a
//   NATIVE motion also resolving as a single step, so the gap is the harness,
//   not the plugin. Covering it means driving `lattice-ui-tui`'s `press`
//   helper; carried to OM.4b.
// * **Operator composition** (`d]]`). Operator+motion paths are bound
//   explicitly at `KeymapLayer::Builtin` from a hardcoded `motion_rows` table
//   (`keymap_normal.rs:1467`), so a plugin motion bound in Normal is not
//   reachable after an operator — the SAME gap that blocks plugin text
//   objects, and the same fix. OM.4b covers both with one mechanism.

fn cursor_line(editor: &Editor) -> u32 {
    editor.cursor.line
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headline_motions_walk_forward_and_back() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\nbody\n** Child\nkid\n* Two\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "]]");
    assert_eq!(cursor_line(&editor), 2, "`]]` walks to the next headline");
    press(&mut editor, "]]");
    assert_eq!(cursor_line(&editor), 4, "at any level");

    press(&mut editor, "[[");
    assert_eq!(cursor_line(&editor), 2, "`[[` walks back");
    press(&mut editor, "[[");
    assert_eq!(cursor_line(&editor), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_headline_motion_at_the_edge_stays_put_rather_than_erroring() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\nbody\n* Two\n").await;

    // A motion must resolve somewhere. Returning `err` would log and no-op,
    // which is right for a broken motion and wrong for `]]` at the last one —
    // `}` at the end of a buffer stays put, it does not fail.
    goto_line(&mut editor, 2);
    press(&mut editor, "]]");
    assert_eq!(cursor_line(&editor), 2, "`]]` at the last headline stays");

    goto_line(&mut editor, 0);
    press(&mut editor, "[[");
    assert_eq!(cursor_line(&editor), 0, "`[[` at the first headline stays");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_skips_siblings_where_prev_headline_would_not() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\n** Two\n*** A\n*** B\n").await;

    // From `*** B`, `[[` reaches its level-3 SIBLING; `g{` must skip it to the
    // level-2 parent. This is the whole reason `g{` is not just `[[`.
    goto_line(&mut editor, 3);
    press(&mut editor, "[[");
    assert_eq!(cursor_line(&editor), 2, "`[[` finds the sibling");

    goto_line(&mut editor, 3);
    press(&mut editor, "g{");
    assert_eq!(
        cursor_line(&editor),
        1,
        "g-brace finds the parent, not the sibling"
    );

    // A level-1 headline has no parent; the cursor stays.
    goto_line(&mut editor, 0);
    press(&mut editor, "g{");
    assert_eq!(cursor_line(&editor), 0, "a level-1 headline has no parent");
}

// ── OM.4b: operators compose with org's grammar ───────────────────
//
// These assert at the KEYMAP, not by dispatching `dar` and checking the text,
// and that is a harness limit rather than a preference. Operator-pending is a
// modal state resolved at the App layer (`input::translate`), above
// `Editor::dispatch_chord` — dispatching `d` here returns `Bound` on the
// operator immediately and the following keys arrive as fresh Normal chords,
// so `dar` reads as `d`, then `a` (append), then a literal `r`. The same
// layering that stopped OM.4 covering counts.
//
// What is proven here is the mechanism: the rows exist, in the right layer,
// for the right operators, with text objects losing their Normal binding and
// motions keeping theirs. Driving `dar` end-to-end needs a `lattice-ui-tui`
// test on the App's `press` helper, and is carried as a named follow-up rather
// than left implied.

/// The boot wiring itself, not the function it calls.
///
/// `org_editor` drives `expand_plugin_mode_grammar_rows` directly so the other
/// tests are deterministic — which leaves the `PluginLoaded` subscription that
/// runs it in production untested, and an unrun expansion is exactly the
/// "works, but only after you press something" bug class. So this one loads
/// WITHOUT the direct call and waits for the event path to land the rows.
///
/// Waiting on a one-shot task, not polling a race: under load it takes longer,
/// and it fails only if the subscription is genuinely not wired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_boot_subscription_expands_rows_without_any_keypress() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\"]\ndefault_mode = \"org-todo-mode\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), &wasm).unwrap();

    let editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1);

    let org = ModeId::new("org-mode");
    let seq = parse_chord_sequence("dar").expect("parses");
    for _ in 0..200 {
        if matches!(
            editor
                .keymap
                .lookup_with_context(lattice_keymap::BindingMode::Normal, &seq, &[org]),
            lattice_keymap::LookupResult::Bound { .. }
        ) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("`dar` never gained a row — the PluginLoaded subscription is not wired");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_composable_operator_gets_a_row_not_just_delete() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor(base.path(), "* One\nbody\n").await;

    let org = ModeId::new("org-mode");
    for prefix in ["d", "c", "y", "="] {
        let seq = parse_chord_sequence(&format!("{prefix}ar")).expect("parses");
        assert!(
            matches!(
                editor.keymap.lookup_with_context(
                    lattice_keymap::BindingMode::Normal,
                    &seq,
                    &[org]
                ),
                lattice_keymap::LookupResult::Bound { .. }
            ),
            "`{prefix}ar` has a row in org-mode's layer"
        );
        // And ONLY in org-mode's layer — org's objects must not exist in a
        // Rust buffer.
        assert!(
            matches!(
                editor
                    .keymap
                    .lookup_with_context(lattice_keymap::BindingMode::Normal, &seq, &[]),
                lattice_keymap::LookupResult::Unbound
            ),
            "`{prefix}ar` is not global"
        );
    }
}

/// Every object org contributes gets its rows, and so do its motions — the two
/// halves of the gap OM.4b closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_four_text_objects_and_the_motions_are_reachable_after_an_operator() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor(base.path(), "* One\nbody\n").await;

    let org = ModeId::new("org-mode");
    for chord in ["ih", "ah", "ir", "ar", "]]", "[["] {
        let seq = parse_chord_sequence(&format!("d{chord}")).expect("parses");
        assert!(
            matches!(
                editor.keymap.lookup_with_context(
                    lattice_keymap::BindingMode::Normal,
                    &seq,
                    &[org]
                ),
                lattice_keymap::LookupResult::Bound { .. }
            ),
            "`d{chord}` resolves in org-mode's layer"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_object_loses_its_normal_binding_but_a_motion_keeps_one() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor(base.path(), "* One\nbody\n").await;

    let org = ModeId::new("org-mode");

    // `bind_mode_keymap` writes a Normal terminal binding for every declared
    // chord. The expansion REPLACES it for a text object: `ar` alone in Normal
    // is not something a user can mean.
    let tobj = parse_chord_sequence("ar").expect("parses");
    assert!(
        matches!(
            editor
                .keymap
                .lookup_with_context(lattice_keymap::BindingMode::Normal, &tobj, &[org]),
            lattice_keymap::LookupResult::Unbound
        ),
        "`ar` alone is not bound in Normal"
    );

    // Visual is the exception — selecting an object IS meaningful there.
    assert!(
        matches!(
            editor
                .keymap
                .lookup_with_context(lattice_keymap::BindingMode::Visual, &tobj, &[org]),
            lattice_keymap::LookupResult::Bound { .. }
        ),
        "`ar` is bound in Visual, where it extends the selection"
    );

    // A motion keeps its standalone binding; `]]` still moves on its own.
    let motion = parse_chord_sequence("]]").expect("parses");
    assert!(
        matches!(
            editor
                .keymap
                .lookup_with_context(lattice_keymap::BindingMode::Normal, &motion, &[org]),
            lattice_keymap::LookupResult::Bound { .. }
        ),
        "only text objects lose their Normal binding, not motions"
    );
}

// ── OM.5: `<Tab>` routing and the decline chain ───────────────────

/// `<Tab>` on a headline cycles; off one it DECLINES and the chord falls
/// through to whatever `<Tab>` natively means.
///
/// This is the property the whole mode decomposition rests on. If `Declined`
/// did not fall through, org would have to own every meaning `<Tab>` can have
/// in an org buffer, and `org-table-mode` could not be a separate mode at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tab_cycles_on_a_headline_and_declines_off_one() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\nbody\n** Child\nkid\n").await;

    // On the headline: folds appear, so the buffer's visible shape changes
    // without its text changing.
    goto_line(&mut editor, 0);
    let before = text(&editor);
    press(&mut editor, "<Tab>");
    assert_eq!(
        text(&editor),
        before,
        "cycling changes visibility, never text"
    );
    // NOT asserted here: that a fold appeared. Structure-driven folds come
    // from a landed tree-sitter parse, which is asynchronous, so `editor.folds`
    // is empty in this harness whatever `<Tab>` did. `CycleFoldAtCursor`'s own
    // behaviour is covered natively (it predates this plugin); what OM.5 adds
    // and what this test covers is the ROUTING — org's `<Tab>` reaching that
    // effect on a headline and declining off one.

    // Off a headline the action declines. The proof that it FELL THROUGH
    // rather than merely no-opped is that no fold appeared while the chord
    // still resolved — a swallowed chord and a declined one look identical
    // from the buffer, so the assertion is on the fold state the org action
    // would have produced had it handled the key.
    let base2 = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base2.path(), "#+TITLE: Notes\nprose\nmore\n").await;
    goto_line(&mut editor, 1);
    let before = text(&editor);
    press(&mut editor, "<Tab>");
    assert_eq!(
        text(&editor),
        before,
        "declining leaves the buffer untouched — no stray tab inserted, which \
         is what would happen if the chord fell all the way through to Insert"
    );
}

/// The chain must survive MORE than one declining layer.
///
/// `org-table-mode` (OM.12) will bind `<Tab>` above `org-mode` and decline
/// outside a table, so in a plain org paragraph the key passes through two
/// declining layers before reaching the builtin. If `Declined` only fell
/// through one, the mode decomposition would collapse into a single
/// mega-action.
///
/// What is checked here is the LAYERING that makes it possible — two layers
/// both holding `<Tab>`, the higher one winning — because the fall-through
/// itself happens during effect application, below this harness. The
/// end-to-end two-hop case lands with OM.12, when the second layer is real
/// rather than constructed; shipping a stub mode purely to make a test pass
/// would be worse than saying so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_higher_layer_can_take_tab_from_org_mode() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor(base.path(), "* One\nbody\n").await;

    let org = ModeId::new("org-mode");
    let stand_in = ModeId::new("org-table-stand-in-mode");
    let tab = parse_chord_sequence("<Tab>").expect("parses");

    // org-mode owns `<Tab>` today.
    let LookupResult::Bound {
        command: org_cmd, ..
    } = editor
        .keymap
        .lookup_with_context(lattice_keymap::BindingMode::Normal, &tab, &[org])
    else {
        panic!("org-mode binds <Tab>");
    };

    // A MINOR layer above it takes the key — the priority order
    // `Builtin < MajorMode < MinorMode` is what lets org-table-mode refine
    // its major without either of them shadowing the builtin globally.
    let cycle = editor
        .registry
        .load()
        .id_by_name("org-demote-headline")
        .expect("a distinct org command to bind");
    editor
        .keymap
        .try_bind_chord_string(
            lattice_keymap::KeymapCapability::Full,
            lattice_keymap::KeymapLayer::MinorMode(stand_in),
            lattice_keymap::BindingMode::Normal,
            "<Tab>",
            lattice_grammar::command::CommandInvocation::of(cycle),
            lattice_grammar::source::SourceLocation::plugin(0),
        )
        .expect("binds");

    let LookupResult::Bound {
        command: minor_cmd, ..
    } = editor.keymap.lookup_with_context(
        lattice_keymap::BindingMode::Normal,
        &tab,
        &[org, stand_in],
    )
    else {
        panic!("the minor layer binds <Tab>");
    };
    assert_ne!(
        minor_cmd.command, org_cmd.command,
        "the higher layer's binding wins, so a minor can refine its major's key"
    );

    // And with the minor inactive, org-mode has it back.
    let LookupResult::Bound { command: back, .. } =
        editor
            .keymap
            .lookup_with_context(lattice_keymap::BindingMode::Normal, &tab, &[org])
    else {
        panic!("org-mode still binds <Tab>");
    };
    assert_eq!(back.command, org_cmd.command);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_chords_are_scoped_to_org_buffers() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor(base.path(), "* One\n").await;

    // The bindings live in `MajorMode(org-mode)`, a GATED layer. With no modes
    // active they resolve to nothing — so a `.rs` buffer never sees them.
    let expanded = editor.keymap.expand_leader("<leader>ol");
    let seq = parse_chord_sequence(&expanded).unwrap();
    assert!(
        matches!(
            editor
                .keymap
                .lookup_with_context(lattice_keymap::BindingMode::Normal, &seq, &[]),
            lattice_keymap::LookupResult::Unbound
        ),
        "org's chords are not global"
    );
    assert!(
        matches!(
            editor.keymap.lookup_with_context(
                lattice_keymap::BindingMode::Normal,
                &seq,
                &[ModeId::new("org-mode")]
            ),
            lattice_keymap::LookupResult::Bound { .. }
        ),
        "and do resolve when org-mode is the active major"
    );

    // Guard against the sequence being reachable by accident: a lone `<Space>`
    // must not be a terminal binding, or `<leader>ol` could never be typed.
    let space = parse_chord_sequence(&editor.keymap.expand_leader("<leader>")).unwrap();
    assert!(
        !matches!(
            editor.keymap.lookup_with_context(
                lattice_keymap::BindingMode::Normal,
                &space,
                &[ModeId::new("org-mode")]
            ),
            lattice_keymap::LookupResult::Bound { .. }
        ),
        "the leader key alone must stay a prefix, never a terminal binding"
    );

    let _ = chord("x"); // keep the helper used if the assertions above change
}

// ── OM.6: subtree move, meta-return, toggle heading ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn moving_a_subtree_carries_its_children_and_swaps_with_a_sibling() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(
        base.path(),
        "* Root\n** One\nbody one\n** Two\n*** Kid\n** Three\n",
    )
    .await;

    // From `** Two`, move up: it trades places with `** One`, and `*** Kid`
    // travels with it rather than being left behind under `** One`.
    goto_line(&mut editor, 3);
    press(&mut editor, "<leader>oK");
    assert_eq!(
        text(&editor),
        "* Root\n** Two\n*** Kid\n** One\nbody one\n** Three\n",
        "the subtree moved whole, and the sibling it passed stayed intact"
    );

    // And back down again: the pair of chords is an identity.
    goto_line(&mut editor, 1);
    press(&mut editor, "<leader>oJ");
    assert_eq!(
        text(&editor),
        "* Root\n** One\nbody one\n** Two\n*** Kid\n** Three\n",
        "moving back down restores the original order"
    );
}

/// At either end of the sibling chain the chord CONSUMES the key and does
/// nothing — it does not decline.
///
/// This is the test that caught the difference. When these actions returned
/// `Effect::Declined`, the dispatcher re-resolved the sequence with org's
/// layer removed and ran its trailing key on its own: `<leader>oJ` executed
/// vim's `J` and joined two lines. A chord that found no sibling to move must
/// not edit the buffer. `<Tab>` still declines (OM.5) because it has a real
/// meaning to fall through to; a `<leader>o`-prefixed chord has none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn moving_past_the_end_of_the_sibling_chain_consumes_the_key() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* Root\n** Only child\n* Next root\n";
    let mut editor = org_editor(base.path(), original).await;

    // `** Only child` has no sibling in either direction — `* Next root` is
    // shallower, so it is a parent boundary, not a peer.
    goto_line(&mut editor, 1);
    press(&mut editor, "<leader>oK");
    assert_eq!(
        text(&editor),
        original,
        "no sibling above: nothing happened"
    );
    press(&mut editor, "<leader>oJ");
    assert_eq!(
        text(&editor),
        original,
        "no sibling below either — and crucially `J` did not join these lines"
    );
}

/// A move is ONE edit, so `u` restores both subtrees together.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subtree_move_is_one_undo_step() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* One\nbody\n* Two\nmore\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 2);
    press(&mut editor, "<leader>oK");
    assert_eq!(text(&editor), "* Two\nmore\n* One\nbody\n");
    press(&mut editor, "u");
    assert_eq!(text(&editor), original, "one undo reverses the whole swap");
}

/// Meta-return inserts AFTER the subtree, so a headline's children are not
/// reparented under the new sibling. This is the assertion that distinguishes
/// respect-content from the naive "insert on the next line".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn meta_return_inserts_a_sibling_after_the_whole_subtree() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\n** Child\nbody\n* Two\n").await;

    // On the level-1 headline, whose subtree runs through `body`.
    goto_line(&mut editor, 0);
    press(&mut editor, "<leader><CR>");
    assert_eq!(
        text(&editor),
        "* One\n** Child\nbody\n* \n* Two\n",
        "the new sibling landed after the subtree, not between One and Child"
    );

    // The level came from the enclosing headline, not from a fixed 1.
    goto_line(&mut editor, 1);
    press(&mut editor, "<leader><CR>");
    assert_eq!(
        text(&editor),
        "* One\n** Child\nbody\n** \n* \n* Two\n",
        "a level-2 headline gets a level-2 sibling"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn toggle_heading_converts_a_line_both_ways() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\n** Two\nsome note\n").await;

    // A body line becomes a SIBLING of the headline it sits under — level 2
    // here, not level 1.
    goto_line(&mut editor, 2);
    press(&mut editor, "<leader>o*");
    assert_eq!(text(&editor), "* One\n** Two\n** some note\n");

    // And back: the stars and their separating space both go.
    press(&mut editor, "<leader>o*");
    assert_eq!(text(&editor), "* One\n** Two\nsome note\n");
}

// ── OM.7: org-todo-mode ──

/// The slice's exit criterion: the minor rides `org-mode` and resolves
/// NOWHERE else. Activation is the host's, from `ActivationPolicy::Majors` —
/// the plugin names the major it wants and contributes no activation code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_todo_mode_is_a_minor_scoped_to_org_buffers() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor(base.path(), "* TODO Task\n").await;

    let todo_mode = ModeId::new("org-todo-mode");
    let registry = editor.mode_registry.load();
    assert_eq!(
        registry.get(todo_mode).expect("registered").kind(),
        lattice_mode::ModeKind::Minor,
        "it declared `minor` and must not have been promoted to a major"
    );
    // A minor must never be indexed as a language's major — that slot belongs
    // to org-mode, and taking it would leave org files with no outliner.
    assert_eq!(
        registry.find_major_for_lang("org"),
        Some(ModeId::new("org-mode")),
        "org-mode still owns the language"
    );
    drop(registry);

    // Active on this buffer, because its major is org-mode.
    assert!(
        editor
            .active_modes
            .get(&editor.document_buffer_id)
            .is_some_and(|m| m.is_active(todo_mode)),
        "the minor activated on an org buffer"
    );

    // Its chords resolve with org-mode context and not without it.
    let seq = parse_chord_sequence(&editor.keymap.expand_leader("<leader>ot")).unwrap();
    assert!(
        matches!(
            editor.keymap.lookup_with_context(
                lattice_keymap::BindingMode::Normal,
                &seq,
                &[ModeId::new("org-mode"), todo_mode]
            ),
            LookupResult::Bound { .. }
        ),
        "<leader>ot resolves in an org buffer"
    );
    assert!(
        !matches!(
            editor
                .keymap
                .lookup_with_context(lattice_keymap::BindingMode::Normal, &seq, &[]),
            LookupResult::Bound { .. }
        ),
        "and resolves nowhere else — no active mode, no binding"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn todo_keywords_cycle_forward_and_back_through_the_empty_state() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\n").await;

    goto_line(&mut editor, 0);
    // The default `org.todo-keywords` is `TODO | DONE`, so `|` is a separator
    // and not a state.
    press(&mut editor, "<leader>ot");
    assert_eq!(text(&editor), "* TODO Task\n");
    press(&mut editor, "<leader>ot");
    assert_eq!(text(&editor), "* DONE Task\n");
    press(&mut editor, "<leader>ot");
    assert_eq!(
        text(&editor),
        "* Task\n",
        "the empty state is part of the cycle"
    );
    press(&mut editor, "<leader>oT");
    assert_eq!(
        text(&editor),
        "* DONE Task\n",
        "and backwards wraps the other way"
    );
}

/// `:set org.todo-keywords=…` takes effect on the NEXT press, which is why the
/// option is read per keystroke rather than cached at load.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_keyword_sequence_comes_from_the_option() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\n").await;

    // The plugin's options are namespaced by its id.
    assert_eq!(
        editor
            .config
            .get_string_by_name("org.todo-keywords")
            .as_deref(),
        Some("TODO | DONE"),
        "the config seam registered the option under the plugin's namespace, \
         with the declared default"
    );
    // The path `:set org.todo-keywords=…` is backed by (per config.wit).
    editor
        .config
        .parse_and_set_command("org.todo-keywords=PROPOSED ACCEPTED")
        .expect("the option accepts a new sequence");

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>ot");
    assert_eq!(
        text(&editor),
        "* PROPOSED Task\n",
        "the reconfigured sequence is used on the very next press"
    );
    press(&mut editor, "<leader>ot");
    assert_eq!(text(&editor), "* ACCEPTED Task\n");
}

/// Cycling a keyword must not disturb the priority or the tags — the reason
/// the guest parses and re-renders the whole headline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cycling_preserves_priority_and_tags() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "** TODO [#A] Ship it :work:urgent:\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>ot");
    assert_eq!(text(&editor), "** DONE [#A] Ship it :work:urgent:\n");

    press(&mut editor, "<leader>o,");
    assert_eq!(
        text(&editor),
        "** DONE [#B] Ship it :work:urgent:\n",
        "priority advanced; keyword and tags untouched"
    );
}

/// From inside a subtree the keys mark the ENCLOSING headline, so they work
/// without navigating to it first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn todo_cycling_works_from_inside_the_subtree() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\nbody\nmore body\n").await;

    goto_line(&mut editor, 2);
    press(&mut editor, "<leader>ot");
    assert_eq!(text(&editor), "* TODO One\nbody\nmore body\n");
}

/// With no headline above the caret there is nothing to mark. Like OM.6, the
/// key is CONSUMED — declining would re-dispatch the trailing `t`, and in
/// Normal mode `t` waits for a target character, leaving the editor in a
/// pending state the user never asked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn todo_cycling_in_a_preamble_consumes_the_key() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "#+TITLE: Notes\njust prose\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 1);
    press(&mut editor, "<leader>ot");
    assert_eq!(text(&editor), original);
    press(&mut editor, "<leader>o,");
    assert_eq!(text(&editor), original);
}

/// `<leader>o:` is a TWO-HOP flow: the action returns `OpenPrompt` naming
/// `org-set-tags-submit`, the host runs the minibuffer, and submitting
/// dispatches that action with the typed text in `ctx.args`.
///
/// **Only the binding is asserted here, and that is a layer limit rather than
/// a gap in the feature.** `Effect::OpenPrompt` is applied by the RENDERER
/// (`lattice-ui-tui/src/app/dispatch.rs`), not by `Editor` — at this level the
/// chord surfaces as `Action::Invoke` and the effect is consumed inside it, so
/// `editor.modal` never leaves `Normal` however correct the plugin is. Hop two
/// is no more reachable: dispatching an invocation WITH args needs
/// `handle_action`, which is `pub(crate)` to `lattice-host`.
///
/// So the split is: `todo.rs`'s unit tests own the tag logic (`set_tags`
/// parses both `:a:b:` and bare words, replaces rather than appends, and
/// clears on empty), this owns the wiring, and the round trip through a real
/// minibuffer wants an app-layer test in `lattice-ui-tui`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tag_prompt_chord_is_wired_to_the_plugins_action() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor(base.path(), "* TODO Ship it\n").await;

    // Both halves of the flow must be registered: the chord the user presses,
    // and the action the payload names for the host to dispatch on submit. A
    // typo in either is a silent dead end at runtime.
    let seq = parse_chord_sequence(&editor.keymap.expand_leader("<leader>o:")).unwrap();
    assert!(
        matches!(
            editor.keymap.lookup_with_context(
                lattice_keymap::BindingMode::Normal,
                &seq,
                &[ModeId::new("org-mode"), ModeId::new("org-todo-mode")]
            ),
            LookupResult::Bound { .. }
        ),
        "<leader>o: is bound in an org buffer"
    );
    assert!(
        editor
            .registry
            .load()
            .id_by_name("org-set-tags-submit")
            .is_some(),
        "the action the OpenPrompt payload names is registered, so the host \
         has something to dispatch when the user submits"
    );
}

// ── OM.8: checkboxes + statistics cookies ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn toggling_a_checkbox_updates_the_parents_cookie_in_one_edit() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* Shopping [1/3]\n  - [X] bread\n  - [ ] milk\n  - [ ] eggs\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 2);
    press(&mut editor, "<C-Space>");
    assert_eq!(
        text(&editor),
        "* Shopping [2/3]\n  - [X] bread\n  - [X] milk\n  - [ ] eggs\n",
        "the box ticked and the cookie followed"
    );

    // ONE edit: a single `u` puts both back. A half-undone list showing
    // `[2/3]` above one ticked box is worse than either end state.
    press(&mut editor, "u");
    assert_eq!(text(&editor), original, "one undo reverses box and cookie");
}

/// A percentage cookie stays a percentage — rewriting it as a ratio would
/// change the document's style on a keypress.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_percentage_cookie_keeps_its_form() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(
        base.path(),
        "* Shopping [0%]\n  - [ ] bread\n  - [ ] milk\n  - [ ] eggs\n",
    )
    .await;

    goto_line(&mut editor, 1);
    press(&mut editor, "<C-Space>");
    assert_eq!(
        text(&editor),
        "* Shopping [33%]\n  - [X] bread\n  - [ ] milk\n  - [ ] eggs\n",
        "truncated, so 100% means genuinely complete"
    );
}

/// `<C-Space>` off a checkbox consumes the key and does nothing. It is org's
/// chord here with nothing beneath it to fall through to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn toggling_off_a_checkbox_line_does_nothing() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* Shopping\nplain prose\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 1);
    press(&mut editor, "<C-Space>");
    assert_eq!(text(&editor), original);
}

/// A nested list rolls up one level at a time: ticking a grandchild updates
/// its own parent's box-count cookie, and the headline's cookie counts only
/// its direct children.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_lists_roll_up_to_the_nearest_cookie() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(
        base.path(),
        "* Top [0/2]\n  - [ ] a [0/2]\n    - [ ] a1\n    - [ ] a2\n  - [ ] b\n",
    )
    .await;

    goto_line(&mut editor, 2);
    press(&mut editor, "<C-Space>");
    let out = text(&editor);
    assert!(
        out.contains("- [ ] a [1/2]"),
        "the nearest cookie moved: {out}"
    );
    assert!(
        out.contains("* Top [0/2]"),
        "the headline still counts only its DIRECT children: {out}"
    );
}

// ── OM.9: timestamps ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_a_steps_the_timestamp_component_under_the_cursor() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nSCHEDULED: <2026-08-25 Tue>\n").await;

    // On the day digits.
    goto_line(&mut editor, 1);
    editor.cursor.byte = 20;
    press(&mut editor, "<C-a>");
    assert_eq!(
        text(&editor),
        "* Task\nSCHEDULED: <2026-08-26 Wed>\n",
        "the day stepped AND the weekday was recomputed"
    );

    press(&mut editor, "<C-x>");
    assert_eq!(text(&editor), "* Task\nSCHEDULED: <2026-08-25 Tue>\n");
}

/// The one place declining is right in this plugin.
///
/// `<C-a>` / `<C-x>` are vim's increment / decrement — a genuinely SHARED
/// chord — so org must not swallow them off a timestamp, or shadowing them
/// inside org buffers would break incrementing ordinary numbers.
///
/// **Lattice has no increment command yet**, so today the decline resolves to
/// nothing and the buffer is simply untouched. That is the assertion here:
/// org did not consume the key and did not edit. When increment lands, this
/// binding composes with it for free — which is exactly what declining buys
/// and what consuming would have foreclosed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_a_timestamp_ctrl_a_declines_rather_than_swallowing() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* Task\ncount: 41\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 1);
    editor.cursor.byte = 7;
    press(&mut editor, "<C-a>");
    assert_eq!(
        text(&editor),
        original,
        "org declined; nothing else is bound to <C-a> yet"
    );
}

/// A time crossing midnight moves the date — wrapping in place would leave
/// the stamp silently lying about which day it means.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_time_crossing_midnight_moves_the_date_end_to_end() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "<2026-08-25 Tue 23:30>\n").await;

    goto_line(&mut editor, 0);
    editor.cursor.byte = 17;
    press(&mut editor, "<C-a>");
    assert_eq!(text(&editor), "<2026-08-26 Wed 00:30>\n");
}

// ── OM.10: links ──

/// An internal `[[*Headline]]` moves the cursor within this buffer — the one
/// link kind whose whole effect is observable without opening anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_internal_link_jumps_to_its_headline() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(
        base.path(),
        "* Index\nsee [[*Deep Work]] below\n* Other\n* TODO Deep Work\nbody\n",
    )
    .await;

    goto_line(&mut editor, 1);
    editor.cursor.byte = 8;
    press(&mut editor, "<leader>oo");
    assert_eq!(
        editor.cursor.line, 3,
        "jumped to the headline, and the TODO keyword is not part of the title"
    );
}

/// A reference that resolves to nothing must not move the cursor somewhere
/// arbitrary — landing on the wrong heading is worse than not moving.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unresolved_internal_link_leaves_the_cursor_alone() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Index\n[[*Nowhere]]\n* Other\n").await;

    goto_line(&mut editor, 1);
    editor.cursor.byte = 4;
    press(&mut editor, "<leader>oo");
    assert_eq!(editor.cursor.line, 1, "still on the link");
}

/// Off a link the chord is consumed — it is org's, behind `<leader>o`, with
/// nothing beneath it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_off_a_link_does_nothing() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* Index\nplain prose\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 1);
    press(&mut editor, "<leader>oo");
    assert_eq!(text(&editor), original);
    assert_eq!(editor.cursor.line, 1);
}

// ── OL.3: `<CR>` follows a link, and declines everywhere else ──

/// `<CR>` on a link does what `<leader>oo` does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cr_on_a_link_follows_it() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(
        base.path(),
        "* Index\nsee [[*Deep Work]] below\n* Other\n* TODO Deep Work\nbody\n",
    )
    .await;

    goto_line(&mut editor, 1);
    editor.cursor.byte = 8;
    press(&mut editor, "<CR>");
    assert_eq!(editor.cursor.line, 3, "`<CR>` followed the link");
}

/// **The decline, and what it does and does not prove.**
///
/// `org-follow-link` answers `Effect::Declined` off a link, which means
/// "org did not handle this, keep resolving". **Nothing binds `<CR>`
/// beneath org in a Document buffer today** — lattice has no equivalent
/// of vim's first-non-blank-of-next-line motion, and `input.rs` routes
/// `<CR>` to `FollowLink` only for Help / Dashboard / Oil / FileTree
/// buffers. So declining is observationally a no-op right now, and this
/// test cannot distinguish it from `Effect::None`; OM.5 records the same
/// limitation for `<Tab>`.
///
/// `Declined` is still the right answer, for two reasons a test cannot
/// see: it is the honest one, and it is what makes org compose for free
/// if `<CR>` ever gains a Document-buffer meaning.
///
/// What this test DOES prove is the thing worth protecting: `<CR>` in an
/// org buffer must not reach Insert-mode handling, where it would split
/// the line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cr_off_a_link_leaves_the_buffer_and_cursor_alone() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* Index\nplain prose\n    indented next\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 1);
    let before_line = editor.cursor.line;
    press(&mut editor, "<CR>");
    assert_eq!(
        text(&editor),
        original,
        "no newline inserted — the failure if the chord reached Insert handling"
    );
    assert_eq!(editor.cursor.line, before_line, "nothing beneath moved it");
}

/// The catastrophic version, pinned separately because it is a
/// *different* failure from "nothing happened".
///
/// Had `<CR>` been bound to `org-open-link` — which answers
/// `Effect::None` — this would also pass. Had org's action instead let
/// the key fall all the way through to Insert, `<CR>` would split every
/// line it was pressed on. One key, one buffer-destroying outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cr_in_an_org_buffer_never_enters_insert_or_splits_a_line() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* Index\nplain prose\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 1);
    editor.cursor.byte = 5;
    press(&mut editor, "<CR>");
    press(&mut editor, "<CR>");
    assert_eq!(
        text(&editor),
        original,
        "two presses, still one buffer — no line was split"
    );
    assert!(
        matches!(editor.modal, lattice_grammar::ModalState::Normal),
        "still Normal: {:?}",
        editor.modal
    );
}

/// An `id:` link is recognised and unresolvable, so `<CR>` on one must
/// NOT fall through to the motion — org consumed the chord and said so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cr_on_an_id_link_is_consumed_rather_than_declined() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Index\n[[id:6F39]]\n* Other\n").await;

    goto_line(&mut editor, 1);
    editor.cursor.byte = 4;
    press(&mut editor, "<CR>");
    assert_eq!(
        editor.cursor.line, 1,
        "the cursor stays: org answered with an echo, it did not decline"
    );
}

// ── TK.6: fast select ──

/// The menu opens with one row per configured keyword, and the state you
/// pick lands on the headline.
///
/// Driven through the real chord, the real menu and the real row action —
/// the failure mode this guards is a seam wired end to end that answers
/// nothing, which is what OC.3a hit when a plugin's prompt submit never
/// fired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tk6_the_todo_menu_offers_every_state_and_sets_the_one_you_pick() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO ship it\n").await;
    set_org_option(
        &mut editor,
        "todo-keywords",
        "sequence: TODO(t) NEXT(n) | DONE(d)\ntype: PROJECT",
    );

    press_chord(&mut editor, "<leader>os").await;

    let picker = editor.picker.as_ref().unwrap_or_else(|| {
        panic!(
            "the menu opened; editor said: {:?}",
            editor.last_message.as_ref().map(|m| m.text.clone())
        )
    });
    let spec = picker.transient.as_ref().expect("in transient mode");
    assert_eq!(spec.title, "TODO state");
    let labels: Vec<&str> = spec.groups[0]
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(
        labels,
        vec!["TODO", "NEXT", "DONE", "PROJECT", "(none)", "quit"],
        "every configured state, in declaration order, plus clear and quit"
    );

    // The keys the option gave, and one derived for the keyword that had
    // none — a state the file can contain must be reachable by a key.
    let keys: Vec<&str> = spec.groups[0]
        .items
        .iter()
        .map(|i| i.key.first().map(String::as_str).unwrap_or(""))
        .collect();
    assert_eq!(&keys[..4], &["t", "n", "d", "p"]);

    press_menu_key(&mut editor, "d").await;
    assert_eq!(
        text(&editor),
        "* DONE ship it\n",
        "the row's own args carried the state"
    );
}

/// Clearing a state is a state. A menu that can set every keyword but never
/// remove one is a one-way door.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tk6_the_menu_can_clear_the_state() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO ship it\n").await;
    press_chord(&mut editor, "<leader>os").await;
    press_menu_key(&mut editor, "<Space>").await;
    assert_eq!(text(&editor), "* ship it\n");
}

/// The menu is built per open, so a `:set` takes effect on the next press.
///
/// Deliberately contrasted with the per-keyword COLOURS, which resolve at
/// load because `register-element` drains once. Two halves of one option
/// with different liveness is exactly the kind of thing that gets forgotten,
/// so the live half is pinned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tk6_the_menu_follows_a_set_without_a_reload() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* ship it\n").await;
    set_org_option(
        &mut editor,
        "todo-keywords",
        "sequence: PROPOSED | ACCEPTED",
    );
    press_chord(&mut editor, "<leader>os").await;
    let spec = editor
        .picker
        .as_ref()
        .expect("menu opened")
        .transient
        .as_ref()
        .expect("in transient mode");
    let labels: Vec<&str> = spec.groups[0]
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(labels, vec!["PROPOSED", "ACCEPTED", "(none)", "quit"]);
}

// ── OM.12: tables ──

/// `<Tab>` in a table aligns it and steps a cell. The alignment is
/// whole-table and one edit, so `u` restores it in one step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tab_in_a_table_aligns_and_steps_a_cell() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "| Name | Qty |\n|---+---|\n| bread | 1 |\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 0);
    editor.cursor.byte = 2;
    press(&mut editor, "<Tab>");

    assert_eq!(
        text(&editor),
        "| Name  | Qty |\n|-------+-----|\n| bread | 1   |\n",
        "the whole table aligned to its widest cells"
    );

    press(&mut editor, "u");
    assert_eq!(text(&editor), original, "one undo restores the whole table");
}

/// The payoff from the dispatcher fix: `org-table-mode`'s `<Tab>` declines
/// off a table, and the chord reaches `org-mode`'s headline cycle rather than
/// falling past every mode layer to the builtin.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tab_off_a_table_falls_through_to_the_headline_cycle() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* One\nbody\n";
    let mut editor = org_editor(base.path(), original).await;

    // On a headline, NOT in a table: table-mode declines, org-mode cycles.
    goto_line(&mut editor, 0);
    let before = text(&editor);
    press(&mut editor, "<Tab>");
    assert_eq!(
        text(&editor),
        before,
        "cycling changes visibility, never text — and no stray tab was inserted"
    );
}

// ── OM.13: table rows and columns ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn table_rows_and_columns_move_and_the_caret_follows() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "| a | b |\n| c | d |\n").await;

    // Move the second row up.
    goto_line(&mut editor, 1);
    editor.cursor.byte = 2;
    press(&mut editor, "<leader>tK");
    assert_eq!(text(&editor), "| c | d |\n| a | b |\n");
    assert_eq!(editor.cursor.line, 0, "the caret followed its row");

    // Move the first column right.
    press(&mut editor, "<leader>tL");
    assert_eq!(text(&editor), "| d | c |\n| b | a |\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inserting_a_row_and_column_widens_the_whole_table() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "| a | b |\n").await;

    goto_line(&mut editor, 0);
    editor.cursor.byte = 2;
    press(&mut editor, "<leader>tr");
    assert_eq!(
        text(&editor),
        "| a | b |\n|   |   |\n",
        "as wide as the table"
    );

    press(&mut editor, "<leader>tc");
    let out = text(&editor);
    assert!(out.starts_with("| a |   | b |"), "every row widened: {out}");
}

/// A table with no rows is not a table, and a rule marks a section — both
/// refuse rather than silently destroying structure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_last_row_and_a_separator_refuse_to_be_destroyed() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "| a |\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>tdr");
    assert_eq!(text(&editor), original, "the only row survives");
    press(&mut editor, "<leader>tdc");
    assert_eq!(text(&editor), original, "the only column survives");
}

// ---- OM.6b: archive ------------------------------------------------------
//
// The first chord in this plugin that writes to a file other than the one it
// fired in. Everything below `<leader>o$` is ordinary plugin work; what made
// it possible is two host primitives — `Effect::WriteToFile` (XF) and
// `document.path()` (OM.6b), without which the guest could not name
// `<file>_archive` at all.

/// The `fs:write` grant an archiving org plugin needs: the directory its files
/// live in. In production that is the user's org directory; here, the tempdir.
fn fs_write(dir: &std::path::Path) -> Vec<String> {
    vec![format!("fs:write:{}", dir.display())]
}

fn archive_text(editor: &Editor, base: &std::path::Path) -> String {
    let path = base.join("notes.org_archive");
    let id = editor
        .find_document_by_path(&path)
        .expect("the archive file was opened");
    editor
        .buffers
        .document_handle(id)
        .unwrap()
        .snapshot()
        .text()
        .to_string()
}

/// The whole feature: a subtree leaves this file and lands in the one beside
/// it, named from the source's own path — which is the part that needed
/// `document.path()`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archiving_a_subtree_moves_it_beside_the_source_file() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor_with_caps(
        base.path(),
        "* One\nbody\n** Child\n* Two\n",
        &fs_write(base.path()),
    )
    .await;

    // From the body line, not the headline — archiving is something you do
    // while reading a subtree, and `enclosing_headline` walks up to it.
    goto_line(&mut editor, 1);
    press(&mut editor, "<leader>o$");

    assert_eq!(
        text(&editor),
        "* Two\n",
        "the subtree, its body and its child all left together"
    );
    assert_eq!(
        archive_text(&editor, base.path()),
        "* One\nbody\n** Child\n",
        "and arrived intact"
    );
}

/// The archive file is APPENDED to, so a second archive does not overwrite the
/// first — the file is a log, and reading it top-to-bottom is chronological.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_archive_appends_below_the_first() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor =
        org_editor_with_caps(base.path(), "* One\n* Two\n", &fs_write(base.path())).await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>o$");
    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>o$");

    assert_eq!(text(&editor), "", "both left");
    assert_eq!(archive_text(&editor, base.path()), "* One\n* Two\n");
}

/// **The capability, wired.** The same chord, the same buffer, no `fs:write` —
/// and the subtree stays put. The gate runs at the boundary, so the guest
/// cannot tell the difference and the plugin needs no code for this case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ungranted_org_plugin_cannot_archive() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* One\nbody\n";
    let mut editor = org_editor_with_caps(base.path(), original, &[]).await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>o$");

    assert_eq!(text(&editor), original, "nothing moved");
    assert!(
        editor
            .find_document_by_path(&base.path().join("notes.org_archive"))
            .is_none(),
        "and no archive file was even opened"
    );
}

/// In a file's preamble there is no subtree to archive. The chord CONSUMES
/// rather than declining: `$` on its own is vim's end-of-line motion, so a
/// decline would move the caret from a key that meant "archive" (OM.6's rule).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archiving_the_preamble_does_nothing_and_does_not_run_dollar() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "intro text\n* One\n";
    let mut editor = org_editor_with_caps(base.path(), original, &fs_write(base.path())).await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>o$");

    assert_eq!(text(&editor), original, "nothing archived");
    assert_eq!(
        editor.cursor.byte, 0,
        "and `$` did not run as a bare motion"
    );
}

// ---- OM.11: refile -------------------------------------------------------
//
// **`<leader>or` is not pressed here, and that is a test-harness limit rather
// than a gap in the feature.** The chord returns `Effect::OpenPicker`, which
// the RENDERER applies (`lattice-ui-tui/src/app/dispatch.rs`), not `Editor` —
// so at this level the chord surfaces as `Action::Invoke` with the effect
// consumed inside it and no picker ever opens. Same wall `<leader>o:` hit at
// OM.7, and now the fourth thing waiting on an app-layer test in
// `lattice-ui-tui`.
//
// So the split is: `refile.rs`'s unit tests own the target arithmetic, the
// binding test below owns the wiring, and everything from `open_picker`
// onwards — the source, the candidates, the accept, the write and the cut —
// is driven directly, because all of that IS reachable from `Editor`.

/// The picker's candidate labels, in order, so a test can assert what the user
/// would actually see rather than an index into an opaque list.
fn picker_labels(editor: &Editor) -> Vec<String> {
    editor
        .picker
        .as_ref()
        .map(|p| p.candidates.iter().map(|c| c.raw.text.clone()).collect())
        .unwrap_or_default()
}

/// Open refile's target picker the way the chord's effect would.
/// Open refile's target picker the way the chord's effect would, and wait for
/// it to seat.
///
/// A plugin picker source's `init` is ASYNC — it walks and reads the
/// filesystem in the plugin's own task — so `open_picker` returns with the
/// picker still `None` and a `(loading)` echo. `drain_pending_picker_init` is
/// what seats it, and in production the renderer's tick calls it. A test that
/// asserted straight after `open_picker` would see an empty list and read as a
/// broken source.
async fn open_refile(editor: &mut Editor) {
    let _ = editor.open_picker("org-refile".to_string(), Vec::new());
    for _ in 0..200 {
        let _ = editor.drain_pending_picker_init();
        if editor.picker.is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("refile's picker never seated: {:?}", editor.last_message);
}

/// Narrow the picker to one candidate the way the user does — by typing —
/// then accept it.
fn pick(editor: &mut Editor, query: &str) {
    {
        let picker = editor.picker.as_mut().expect("a picker is open");
        for c in query.chars() {
            picker.append_query(c);
        }
        picker.refilter();
    }
    let _ = editor.do_picker_accept();
}

/// Accept the highlighted candidate and wait for the outcome to land.
///
/// A plugin source's `accept` is ASYNC — it is a guest call — so
/// `do_picker_accept` only spawns it and `drain_pending_picker_accept` is what
/// applies the outcome. In production the actor's `async_landed` wake drives
/// that; here the test does, because there is no keystroke coming.
async fn settle_accept(editor: &mut Editor) {
    for _ in 0..200 {
        let _ = editor.drain_pending_picker_accept();
        editor.run_tick_pending();
        if editor.pending_picker_accept.is_none() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

fn text_of(editor: &Editor, path: &std::path::Path) -> String {
    let id = editor
        .find_document_by_path(path)
        .unwrap_or_else(|| panic!("{} was opened", path.display()));
    editor
        .buffers
        .document_handle(id)
        .unwrap()
        .snapshot()
        .text()
        .to_string()
}

/// Lay out a project (a `.git` marker, since refile targets are the PROJECT's
/// org files — the same scope the agenda walks) holding one target file.
fn refile_project(base: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(base.join(".git")).unwrap();
    let targets = base.join("targets.org");
    std::fs::write(&targets, "* Work\n** Q3\n* Personal\n").unwrap();
    targets
}

/// Both halves of the flow are registered: the chord the user presses, and the
/// action the picker's accept names. A typo in either is a silent dead end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_refile_chord_and_its_second_hop_are_both_wired() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;

    let seq = parse_chord_sequence(&editor.keymap.expand_leader("<leader>or")).unwrap();
    assert!(
        matches!(
            editor.keymap.lookup_with_context(
                lattice_keymap::BindingMode::Normal,
                &seq,
                &[ModeId::new("org-mode"), ModeId::new("org-todo-mode")]
            ),
            LookupResult::Bound { .. }
        ),
        "<leader>or is bound in an org buffer"
    );
    assert!(
        editor.registry.load().id_by_name("org-refile-to").is_some(),
        "the action the picker's accept invokes is registered, so the host has \
         something to dispatch when the user chooses a target"
    );
    assert!(
        editor.picker_registry.load().entry("org-refile").is_some(),
        "and the source the chord's effect names exists, so it opens something"
    );
}

/// The whole feature: pick a headline in another file, and the subtree lands
/// UNDER it rather than at the end of the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refiling_files_a_subtree_under_a_headline_in_another_file() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let targets = refile_project(base.path());
    let mut editor =
        org_editor_with_caps(base.path(), "* One\nbody\n* Two\n", &fs_write(base.path())).await;

    goto_line(&mut editor, 1);
    open_refile(&mut editor).await;
    assert!(
        picker_labels(&editor)
            .iter()
            .any(|l| l == "targets.org  Work"),
        "the headlines of the project's org files are the targets: {:?}",
        picker_labels(&editor)
    );

    pick(&mut editor, "Work");
    settle_accept(&mut editor).await;

    assert_eq!(text(&editor), "* Two\n", "the subtree left the source");
    assert_eq!(
        text_of(&editor, &targets),
        "* Work\n** Q3\n* One\nbody\n* Personal\n",
        "and landed after Work's whole subtree, not in front of its children"
    );
}

/// The file itself is a target, and it appends. That is the answer when none
/// of the headlines is the right home.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refiling_to_a_file_appends_at_the_end() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let targets = refile_project(base.path());
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;

    goto_line(&mut editor, 0);
    open_refile(&mut editor).await;
    // The bare file name is the file-level target; every headline row carries
    // a heading after it.
    pick(&mut editor, "targets.org");
    settle_accept(&mut editor).await;

    assert_eq!(
        text_of(&editor, &targets),
        "* Work\n** Q3\n* Personal\n* One\n"
    );
}

/// Refile is a filing action, not a navigation one: we stay in the source file
/// even though the target was opened.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refiling_does_not_follow_the_subtree() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let _targets = refile_project(base.path());
    let mut editor =
        org_editor_with_caps(base.path(), "* One\n* Two\n", &fs_write(base.path())).await;

    goto_line(&mut editor, 0);
    open_refile(&mut editor).await;
    pick(&mut editor, "Work");
    settle_accept(&mut editor).await;

    assert_eq!(text(&editor), "* Two\n", "we are still in the source file");
}

/// In the preamble there is no subtree to file, so nothing moves — the same
/// refusal archive makes, reached through the picker instead of the chord.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refiling_from_the_preamble_does_nothing() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let targets = refile_project(base.path());
    let before = std::fs::read_to_string(&targets).unwrap();
    let original = "intro text\n* One\n";
    let mut editor = org_editor_with_caps(base.path(), original, &fs_write(base.path())).await;

    goto_line(&mut editor, 0);
    open_refile(&mut editor).await;
    pick(&mut editor, "Work");
    settle_accept(&mut editor).await;

    assert_eq!(text(&editor), original, "the source is untouched");
    assert_eq!(
        std::fs::read_to_string(&targets).unwrap(),
        before,
        "and nothing was filed"
    );
}

// ---- OM.11: capture ------------------------------------------------------
//
// `<C-x>oc` is not pressed here either, and for the same reason as refile:
// the chord returns `Effect::OpenPrompt`, which the RENDERER applies. That is
// the `<leader>o:` wall from OM.7 a third time. What IS reachable — and what
// the feature actually is — is the submit hop: dispatch `org-capture-submit`
// with the text the user typed, and assert where it landed.

/// Set an org option the way `:set org.<name>=<value>` would.
fn set_org_option(editor: &mut Editor, name: &str, value: &str) {
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!("org.{name}={value}"),
    });
}

/// Dispatch the prompt's submit action with `text`, as the host does when the
/// user hits `<CR>` on the capture line.
/// OC.5b: fire the capture's FIRST hop — the one that records where it was
/// fired from. Separate from `submit_capture` because `%a` only means anything
/// when the two are distinct dispatches.
fn open_capture(editor: &mut Editor) {
    let id = editor
        .registry
        .load()
        .id_by_name("org-capture")
        .expect("the capture action is registered");
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.dispatch_invocation(lattice_grammar::CommandInvocation::of(id), &mut out);
}

fn submit_capture(editor: &mut Editor, text: &str) {
    let id = editor
        .registry
        .load()
        .id_by_name("org-capture-submit")
        .expect("the submit action is registered");
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.dispatch_invocation(
        lattice_grammar::CommandInvocation::of(id)
            .with_args(lattice_grammar::Args::String(text.to_string())),
        &mut out,
    );
}

/// Both halves of the flow are registered: the chord, and the action the
/// `OpenPrompt` payload names for the host to dispatch on submit.
///
/// OC.1 moved the chord off org's MAJOR keymap onto `org-global-mode`, a
/// `Universal` minor — so the assertion is deliberately made with NO org mode
/// but that one in the active set. That is the whole point of the move: the
/// thought you are trying not to lose arrives while you are reading code, and
/// a capture chord that only fires inside an org file is backwards.
///
/// OC.3 then moved it from `<C-x>oc` to `<leader>oc`, because `<C-x>` is
/// already a TERMINAL binding on org's major (timestamp decrement) and a
/// prefix in one layer beside a terminal binding in another cannot resolve
/// without an ambiguous-chord timeout this editor does not have.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_capture_chord_and_its_submit_hop_are_both_wired() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;

    let bound = |chord: &str, modes: &[ModeId]| {
        matches!(
            editor.keymap.lookup_with_context(
                lattice_keymap::BindingMode::Normal,
                &parse_chord_sequence(&editor.keymap.expand_leader(chord)).unwrap(),
                modes
            ),
            LookupResult::Bound { .. }
        )
    };

    let global_only = [ModeId::new("org-global-mode")];
    assert!(
        bound("<leader>oc", &global_only),
        "<leader>oc is bound wherever org-global-mode is, org file or not"
    );
    assert!(
        bound("<leader>oa", &global_only),
        "<leader>oa reaches the agenda from anywhere"
    );

    // The property the prefix choice rests on: the universal minor's
    // `<leader>oc` and the MAJOR's `<leader>oh` both resolve in an org buffer.
    // Layered prefixes compose — which is exactly what `<C-x>` could not do,
    // because the major binds it as a terminal chord.
    let in_org = [
        ModeId::new("org-mode"),
        ModeId::new("org-todo-mode"),
        ModeId::new("org-global-mode"),
    ];
    assert!(bound("<leader>oc", &in_org), "and still inside an org file");
    assert!(
        bound("<leader>oh", &in_org),
        "without costing the major its own <leader>o chords"
    );

    assert!(
        editor
            .registry
            .load()
            .id_by_name("org-capture-submit")
            .is_some(),
        "the action the OpenPrompt payload names is registered"
    );
}

/// The whole feature: what you type lands in the capture file, through the
/// template — in a file the editor had never opened.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_lands_in_the_capture_file_through_the_template() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(&mut editor, "capture-file", notes.to_str().unwrap());
    set_org_option(&mut editor, "capture-template", "* TODO %?");

    submit_capture(&mut editor, "call the bank");

    assert_eq!(text_of(&editor, &notes), "* TODO call the bank\n");
    assert_eq!(text(&editor), "* One\n", "capture MOVES nothing");
}

/// Captures accumulate. The file is a log, so a second one goes below the
/// first rather than over it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_capture_appends_below_the_first() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(&mut editor, "capture-file", notes.to_str().unwrap());
    set_org_option(&mut editor, "capture-template", "* %?");

    submit_capture(&mut editor, "first");
    submit_capture(&mut editor, "second");

    assert_eq!(text_of(&editor, &notes), "* first\n* second\n");
}

/// With `org.capture-file` unset the user is TOLD, not silently given a
/// `capture.org` in whichever directory the editor started in — a note filed
/// somewhere you never named is a note you will not find.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unset_capture_file_says_so() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;

    submit_capture(&mut editor, "a thought");

    let msg = editor.last_message.as_ref().expect("the user is told");
    assert!(
        msg.text.contains("org.capture-file"),
        "and told WHICH option to set: {}",
        msg.text
    );
}

/// The plugin needs `fs:write` over the capture file's directory, and without
/// it the write is refused at the boundary — the same gate archive meets.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ungranted_plugin_cannot_capture() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &[]).await;
    set_org_option(&mut editor, "capture-file", notes.to_str().unwrap());

    submit_capture(&mut editor, "a thought");

    assert!(
        editor.find_document_by_path(&notes).is_none(),
        "the capture file was never even opened"
    );
}

/// OC.2 — a capture driven by `org.capture-templates` rather than the single
/// `capture-file` / `capture-template` pair.
///
/// The option's value is TOML, which is forced rather than preferred: an option
/// is `boolean | integer | string` and a template is a record, so an
/// array-of-tables cannot reach an option at all. This is the test that proves
/// the round trip actually survives — including a `"""` body's newlines, which
/// is the property the whole arrangement rests on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_template_set_drives_the_capture() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"todo\"\n\
             target = {{ file = \"{}\" }}\nbody = \"\"\"\n* TODO %?\n  %U\n\"\"\"\n",
            notes.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "call the bank");

    let written = text_of(&editor, &notes);
    assert!(
        written.starts_with("* TODO call the bank\n  ["),
        "the body's newline survived the TOML-inside-an-option round trip: {written:?}"
    );
    assert_eq!(text(&editor), "* One\n", "capture MOVES nothing");
}

/// The set WINS over the legacy pair when both are set — otherwise upgrading a
/// config would leave the old single template quietly in charge and the new
/// templates doing nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_template_set_takes_precedence_over_the_single_template() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let old = base.path().join("old.org");
    let new = base.path().join("new.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(&mut editor, "capture-file", old.to_str().unwrap());
    set_org_option(&mut editor, "capture-template", "* OLD %?");
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ntarget = {{ file = \"{}\" }}\nbody = \"* NEW %?\"\n",
            new.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "a thought");

    assert_eq!(text_of(&editor, &new), "* NEW a thought\n");
    assert!(
        !old.exists(),
        "the legacy single-template path did not also fire"
    );
}

/// A malformed set captures NOTHING and says why. Writing the note somewhere
/// the user did not choose is the one outcome capture must not have, and an
/// empty menu built from the half that parsed would be guessing at intent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_template_set_captures_nothing_and_echoes() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    // A legacy pair that WOULD have captured, so "nothing was written" is a
    // real refusal rather than an unconfigured no-op.
    set_org_option(&mut editor, "capture-file", notes.to_str().unwrap());
    set_org_option(&mut editor, "capture-template", "* %?");
    set_org_option(&mut editor, "capture-templates", "[[template]\nkey = \"t\"");

    submit_capture(&mut editor, "a thought");

    let msg = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("capture-templates"),
        "the echo names the option at fault: {msg:?}"
    );
    assert!(
        !notes.exists(),
        "a broken set refuses outright — it does not quietly fall back to the \
         legacy single template, which would file the note somewhere the user \
         thought they had stopped using"
    );
}

/// Several templates and no way yet to choose between them: capture says which
/// keys exist rather than silently picking one. OC.3's menu replaces this echo
/// with the rows themselves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_multi_template_set_names_its_keys_until_the_menu_exists() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"todo\"\n\
             target = {{ file = \"{f}\" }}\nbody = \"* TODO %?\"\n\n\
             [[template]]\nkey = \"n\"\ndescription = \"note\"\n\
             target = {{ file = \"{f}\" }}\nbody = \"* %?\"\n",
            f = notes.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "a thought");

    let msg = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("t todo") && msg.contains("n note"),
        "the echo names every key and what it captures: {msg:?}"
    );
    assert!(
        !notes.exists(),
        "and nothing was captured to either of them"
    );
}

/// OC.3 — the capture menu, end to end through the real chord.
///
/// This is what makes a nine-template set usable: before it a chord carried no
/// way to say WHICH template, so capture could offer exactly one. The menu is a
/// plugin-contributed transient (TR.2b), its rows carry each template's key in
/// their own args (TR.2a), and pressing a key fires org's own capture action
/// with that key.
///
/// Driven through `press`, not by calling the builder: a menu that builds
/// correctly and cannot be opened by its chord is the failure this catches.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_capture_menu_opens_on_the_chord_with_a_row_per_template() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"todo\"\n\
             target = {{ file = \"{f}\" }}\nbody = \"* TODO %?\"\n\n\
             [[template]]\nkey = \"n\"\ndescription = \"note\"\n\
             target = {{ file = \"{f}\" }}\nbody = \"* %?\"\n",
            f = notes.to_str().unwrap()
        ),
    );

    press_chord(&mut editor, "<leader>oc").await;

    let picker = editor.picker.as_ref().unwrap_or_else(|| {
        panic!(
            "the menu opened; editor said: {:?}",
            editor.last_message.as_ref().map(|m| m.text.clone())
        )
    });
    let spec = picker.transient.as_ref().expect("in transient mode");
    assert_eq!(spec.title, "Capture");
    let keys: Vec<&str> = spec.groups[0]
        .items
        .iter()
        .map(|i| i.key[0].as_str())
        .collect();
    assert_eq!(
        keys,
        vec!["t", "n", "q"],
        "one row per template, in declaration order, plus a way out"
    );
    let labels: Vec<&str> = spec.groups[0]
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert!(labels.contains(&"todo") && labels.contains(&"note"));
}

/// The key you press is what decides the template — the whole point of the
/// per-row args slot. Two rows fire ONE action; only the argument differs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_key_you_press_decides_which_template_captures() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let todos = base.path().join("todo-target.org");
    let notes = base.path().join("note-target.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"todo\"\n\
             target = {{ file = \"{t}\" }}\nbody = \"* TODO %?\"\n\n\
             [[template]]\nkey = \"n\"\ndescription = \"note\"\n\
             target = {{ file = \"{n}\" }}\nbody = \"* NOTE %?\"\n",
            t = todos.to_str().unwrap(),
            n = notes.to_str().unwrap()
        ),
    );

    // Open the menu and press `n` — the SECOND template, so a first-wins bug
    // cannot pass this.
    press_chord(&mut editor, "<leader>oc").await;
    press_menu_key(&mut editor, "n").await;

    // The row fired `org-capture n`, which opened the prompt for that
    // template. Finish it the way the user would.
    submit_prompt(&mut editor, "a thought");

    assert_eq!(text_of(&editor, &notes), "* NOTE a thought\n");
    assert!(
        !todos.exists(),
        "the other template's file was not written — the key chose, not the order"
    );
}

/// And the whole chain through the REAL prompt rather than a direct dispatch:
/// chord → menu → key → prompt → submit. Every org capture test before this one
/// dispatched the submit action itself, which is how OC.3a's bug survived (a
/// plugin's prompt submit never fired at all).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_prompt_the_menu_opens_actually_files_the_note() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"todo\"\n\
             target = {{ file = \"{}\" }}\nbody = \"* TODO %?\"\n",
            notes.to_str().unwrap()
        ),
    );

    press_chord(&mut editor, "<leader>oc").await;
    press_menu_key(&mut editor, "t").await;
    assert!(
        editor.pending_prompt_submit_action.is_some(),
        "the row opened the capture prompt"
    );
    submit_prompt(&mut editor, "call the bank");

    assert_eq!(text_of(&editor, &notes), "* TODO call the bank\n");
    assert_eq!(text(&editor), "* One\n", "capture MOVES nothing");
}

/// A broken template set means the menu does NOT open, and says why. An empty
/// menu would tell the user nothing about what to fix.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_set_leaves_the_menu_closed_and_echoes() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(&mut editor, "capture-templates", "[[template]\nkey = \"t\"");

    press_chord(&mut editor, "<leader>oc").await;

    assert!(editor.picker.is_none(), "no menu opened");
    let msg = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("capture-templates"),
        "the echo names the option at fault: {msg:?}"
    );
}

/// OC.4 — a template with `%^{Question}`s collects them as menu FIELDS.
///
/// Three named answers before one write is what makes a vocabulary template
/// possible at all — it is not one line of typed text, it is several fields.
/// Collected through the mechanism the editor already has (the menu parks, a
/// prompt takes the value, the menu comes back), so the menu stays the surface
/// and an answer can be re-edited before anything is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_template_with_questions_collects_them_as_fields() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let vocab = base.path().join("vocab.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"v\"\ndescription = \"Vocab\"\n\
             target = {{ file = \"{}\" }}\n\
             body = \"\"\"\n* %^{{Word}} :fc:\n- Context: %^{{Context}}\n- T: %^{{Translation}}\n\"\"\"\n",
            vocab.to_str().unwrap()
        ),
    );

    press_chord(&mut editor, "<leader>oc").await;
    press_menu_key(&mut editor, "v").await;

    // The second menu is the FIELDS one, opened for this template — one row
    // per question, in template order, plus the body and a way to fire.
    let spec = editor
        .picker
        .as_ref()
        .and_then(|p| p.transient.as_ref())
        .expect("the fields menu opened")
        .clone();
    assert_eq!(spec.title, "Capture: Vocab");
    let labels: Vec<&str> = spec.groups[0]
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(
        labels,
        vec!["Word", "Context", "Translation", "body", "capture", "quit"],
        "a row per question in TEMPLATE order, then the body, then the fire row"
    );

    // Answer them, then fire.
    answer_field(&mut editor, "1", "chat").await;
    answer_field(&mut editor, "2", "le chat noir").await;
    answer_field(&mut editor, "3", "cat").await;
    press_menu_key(&mut editor, "c").await;

    assert_eq!(
        text_of(&editor, &vocab),
        "* chat :fc:\n- Context: le chat noir\n- T: cat\n",
        "each answer substituted at its OWN position"
    );
}

/// A template with no questions keeps the direct prompt — one hop, as before.
///
/// That is a UX decision rather than an omission: the common template is a
/// single `%?`, and routing it through a menu would cost three keystrokes to
/// collect the one value a prompt already asks for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_template_without_questions_still_captures_in_one_hop() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"todo\"\n\
             target = {{ file = \"{}\" }}\nbody = \"* TODO %?\"\n",
            notes.to_str().unwrap()
        ),
    );

    press_chord(&mut editor, "<leader>oc").await;
    press_menu_key(&mut editor, "t").await;
    assert!(
        editor.pending_prompt_submit_action.is_some(),
        "no questions, so the prompt opens directly rather than a second menu"
    );
    submit_prompt(&mut editor, "call the bank");

    assert_eq!(text_of(&editor, &notes), "* TODO call the bank\n");
}

/// Abandoning the fields menu writes nothing and leaves nothing behind — the
/// next capture starts clean rather than inheriting half of this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoning_the_fields_menu_writes_nothing() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let vocab = base.path().join("vocab.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"v\"\ndescription = \"Vocab\"\n\
             target = {{ file = \"{}\" }}\nbody = \"* %^{{Word}}\"\n",
            vocab.to_str().unwrap()
        ),
    );

    press_chord(&mut editor, "<leader>oc").await;
    press_menu_key(&mut editor, "v").await;
    answer_field(&mut editor, "1", "chat").await;
    press_menu_key(&mut editor, "q").await;

    assert!(!vocab.exists(), "abandoning wrote nothing");
    assert!(editor.picker.is_none(), "and closed the menu");

    // A fresh capture does not inherit the abandoned answer.
    press_chord(&mut editor, "<leader>oc").await;
    press_menu_key(&mut editor, "v").await;
    press_menu_key(&mut editor, "c").await;
    assert_eq!(
        text_of(&editor, &vocab),
        "* \n",
        "the new menu started empty — the abandoned answer did not survive"
    );
}

/// OC.5a — a `file+headline` target files the note under that headline.
///
/// **The behaviour this slice makes honest.** `Target::FileHeadline` has parsed
/// since OC.2 and been ignored ever since: both submit paths called
/// `Target::file()` and passed `FileAnchor::End`, so a template that said
/// "under Tasks" appended at the bottom of the file. The menu row even printed
/// the target, so it looked configured and behaved as if it were not.
///
/// It lands after the whole subtree, not right under the headline — otherwise
/// each new capture would sit in front of everything already filed there, and
/// the subtree would read newest-first while the file around it reads
/// oldest-first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_headline_target_files_the_note_under_that_headline() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("tasks.org");
    // On disk BEFORE the capture — the resolution reads the file, so a target
    // that does not exist yet is a different (append) case.
    std::fs::write(
        &notes,
        "* Inbox\nsomething\n* Tasks\n** Existing\nold\n* Archive\ndone\n",
    )
    .unwrap();

    // A DIFFERENT file from the one the editor opens: `org_editor_with_caps`
    // writes `base/notes.org` itself, so a capture target sharing that name is
    // silently overwritten by the fixture and the test measures nothing.
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"task\"\n\
             target = {{ file = \"{}\", headline = \"Tasks\" }}\n\
             body = \"\"\"\n* TODO %?\n\"\"\"\n",
            notes.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "water the plants");

    let written = text_of(&editor, &notes);
    let lines: Vec<&str> = written.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.contains("water the plants"))
        .unwrap_or_else(|| panic!("the note was filed somewhere: {written:?}"));
    let tasks = lines.iter().position(|l| *l == "* Tasks").expect("Tasks");
    let archive = lines
        .iter()
        .position(|l| *l == "* Archive")
        .expect("Archive");

    assert!(
        at > tasks,
        "the note is under Tasks, not above it: {written:?}"
    );
    assert!(
        at < archive,
        "and inside Tasks' subtree rather than at the end of the file: {written:?}"
    );
    assert!(
        lines[at - 1] == "old",
        "after the WHOLE subtree — behind `** Existing`'s body, not in front of it: {written:?}"
    );
}

/// OC.5a — a headline that is not there appends **and says so**.
///
/// Not "creates it" and not "refuses". Creating invents structure in a file the
/// user may not have opened in months; refusing loses a note they have already
/// typed, which is the one outcome capture must never produce. So the note
/// survives, somewhere findable, and the echo is what tells them their target
/// moved. `Warn`, not `Info`: a silent append is how someone loses track of
/// where their captures are going.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_whose_headline_is_gone_appends_and_says_so() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("tasks.org");
    std::fs::write(&notes, "* Inbox\nsomething\n").unwrap();

    // A DIFFERENT file from the one the editor opens: `org_editor_with_caps`
    // writes `base/notes.org` itself, so a capture target sharing that name is
    // silently overwritten by the fixture and the test measures nothing.
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"task\"\n\
             target = {{ file = \"{}\", headline = \"Renamed Away\" }}\n\
             body = \"\"\"\n* TODO %?\n\"\"\"\n",
            notes.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "still important");

    let written = text_of(&editor, &notes);
    assert!(
        written.contains("still important"),
        "the note is not lost: {written:?}"
    );
    assert!(
        written.trim_end().ends_with("still important"),
        "it appended at the end: {written:?}"
    );

    let msg = editor
        .last_message
        .as_ref()
        .expect("the user is told their target moved");
    assert!(
        msg.text.contains("Renamed Away"),
        "and told WHICH headline could not be found: {}",
        msg.text
    );
}

/// A plain `file` target is untouched by OC.5a — it still appends, and it does
/// so without reading the file at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_file_target_still_appends() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("tasks.org");
    std::fs::write(&notes, "* Inbox\nsomething\n").unwrap();

    // A DIFFERENT file from the one the editor opens: `org_editor_with_caps`
    // writes `base/notes.org` itself, so a capture target sharing that name is
    // silently overwritten by the fixture and the test measures nothing.
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"task\"\n\
             target = {{ file = \"{}\" }}\nbody = \"\"\"\n* TODO %?\n\"\"\"\n",
            notes.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "at the bottom");

    let written = text_of(&editor, &notes);
    assert!(written.trim_end().ends_with("at the bottom"), "{written:?}");
    assert!(
        editor
            .last_message
            .as_ref()
            .is_none_or(|m| !m.text.contains("appended at the end")),
        "and no fallback is reported — a plain `file` target appends by design, \
         not because a headline was missing"
    );
}

/// OC.5b — `%a` links back to where the capture fired from.
///
/// **The origin is gone by the time the note is written**, which is the whole
/// difficulty. Opening the prompt focuses a synthetic prompt buffer, so the
/// document reaching the submit action is the prompt — not the file the user
/// was reading when they pressed the chord. The annotation therefore has to be
/// taken at `capture_open`, while the source buffer is still current, and held
/// until submit.
///
/// This drives both hops for real, because a unit test of the expander proves
/// only that `%a` substitutes a string someone handed it — not that the string
/// still describes the right buffer two dispatches later.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_annotation_names_the_buffer_the_capture_fired_from() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let target = base.path().join("tasks.org");

    // The editor opens `notes.org` — that is the buffer `%a` must name.
    let mut editor =
        org_editor_with_caps(base.path(), "* One\n* Two\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"task\"\n\
             target = {{ file = \"{}\" }}\nbody = \"\"\"\n* TODO %?\n  from %a\n\"\"\"\n",
            target.to_str().unwrap()
        ),
    );

    // Open the capture from the org buffer, then submit — two dispatches, and
    // the buffer changes underneath between them.
    // Dispatched by name, like `submit_capture` beside it. What matters for
    // `%a` is that these are two SEPARATE dispatches with the focused buffer
    // changing in between — which is exactly the shape that loses the origin —
    // and going through the keymap would not make that any more true.
    open_capture(&mut editor);
    submit_capture(&mut editor, "check the roof");

    let written = text_of(&editor, &target);
    assert!(
        written.contains("[[file:"),
        "the annotation is an org link: {written:?}"
    );
    assert!(
        written.contains("notes.org"),
        "naming the buffer the capture fired FROM, not the prompt and not the \
         capture target: {written:?}"
    );
    assert!(
        !written.contains("tasks.org"),
        "the target is not the origin: {written:?}"
    );
}

/// A second capture must not inherit the first one's `%a`. The origin is
/// consumed on use, so a link that looks right and points at the previous
/// capture's buffer cannot happen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_annotation_is_not_reused_by_the_next_capture() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let target = base.path().join("tasks.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"task\"\n\
             target = {{ file = \"{}\" }}\nbody = \"\"\"\n* TODO %? [%a]\n\"\"\"\n",
            target.to_str().unwrap()
        ),
    );

    open_capture(&mut editor);
    submit_capture(&mut editor, "first");
    // No `capture_open` this time — submitting directly is the abandoned-flow
    // shape, and it must not pick up the previous capture's origin.
    submit_capture(&mut editor, "second");

    let written = text_of(&editor, &target);
    let with_link = written.lines().filter(|l| l.contains("[[file:")).count();
    assert_eq!(
        with_link, 1,
        "only the capture that recorded an origin carries one: {written:?}"
    );
}

// ── OT.4: headlines resolve through the parse tree ────────────────────
//
// The line matcher and the grammar disagree about exactly one thing, and it is
// not a corner case: a `* TODO` line between `#+BEGIN_SRC` and `#+END_SRC` is
// example text to the grammar and a headline to `^\*+ `. Every test below is
// that one disagreement, seen through a different chord.
//
// They dispatch through the REAL editor rather than calling `headline.rs`,
// because the thing that broke was never the walk — it was the tree not
// arriving. `DispatchEnv` carried no snapshot until OT.4, so every motion and
// text object got `none` on every keystroke while the guest-side code, its unit
// tests, and the WIT all looked correct. A test that hands the guest a tree it
// built itself passes on that broken version.

/// The invocation the App's operator-pending state builds for `dar` / `dir`.
///
/// Built directly because that state lives above `Editor::dispatch_chord` (see
/// the OM.4b note further up), so `press(editor, "dar")` types `d`, `a`, `r`
/// as three separate Normal chords and never composes the operator.
fn delete_with_object(editor: &mut Editor, object: &str) {
    let id = editor
        .registry
        .load()
        .id_by_name(object)
        .unwrap_or_else(|| panic!("`{object}` is registered"));
    let inv = lattice_grammar::CommandInvocation::of(editor.builtins.delete.0).with_target(
        lattice_grammar::Target::TextObject(
            lattice_grammar::TextObjectId(id),
            lattice_grammar::args::Args::None,
        ),
    );
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.dispatch_invocation(inv, &mut out);
}

/// A file whose only headline-looking line inside the block is not one.
const BLOCK_FILE: &str = "\
* Real
body
#+BEGIN_SRC org
* Fake heading in a block
example body
#+END_SRC
tail of Real
* Second
";

/// `dar` with the caret inside a source block must not delete a "subtree"
/// starting at a line that is not a headline.
///
/// On the text path the caret at the `* Fake` line resolves an enclosing
/// headline there, and the subtree runs to just before `* Second` — so `dar`
/// eats the rest of the block INCLUDING `#+END_SRC` and the tail of the real
/// section, splitting a block in half. The grammar has no section there, so the
/// object resolves the enclosing REAL one instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dar_inside_a_source_block_does_not_split_the_block() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), BLOCK_FILE).await;

    goto_line(&mut editor, 3); // `* Fake heading in a block`
    delete_with_object(&mut editor, "org-around-subtree");

    let after = text(&editor);
    assert!(
        !after.contains("* Fake heading in a block"),
        "the enclosing REAL subtree is what `dar` takes, and the block is \
         inside it: {after:?}"
    );
    assert!(
        after.contains("* Second"),
        "`* Second` is a different subtree and must survive: {after:?}"
    );
    // The tell-tale of the text path: it deletes from the example line to just
    // before `* Second`, so the block's OPENER survives with nothing after it.
    // BOTH paths remove `#+END_SRC`, which is why asserting on the opener is
    // what actually tells them apart — the first draft of this test asserted on
    // the closer and passed against the unmigrated guest.
    assert!(
        !after.contains("#+BEGIN_SRC"),
        "a `#+BEGIN_SRC` with no body and no `#+END_SRC` means the object \
         resolved a span STARTING inside the block — the line matcher's \
         answer, not the grammar's: {after:?}"
    );
    // The whole of `* Real` went and only it — plus the blank line `ar` has
    // always left behind. The object's range runs to the END of the subtree's
    // last line and stops short of its newline, so `d` over it empties the
    // lines without closing the gap. That is unchanged by this slice (the text
    // path leaves exactly the same blank), and it is a separate question from
    // where the subtree ENDS: `archive.rs` already works out which line break
    // travels with a subtree, and `ar` does not consult it. Pinned here so the
    // difference is recorded rather than discovered again.
    assert_eq!(after, "\n* Second\n");
}

/// The same disagreement seen by `]]`: the motion walks sections, so it steps
/// from `* Real` to `* Second` without stopping on the block's example line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_headline_motion_steps_over_a_block_heading() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), BLOCK_FILE).await;

    goto_line(&mut editor, 0);
    press(&mut editor, "]]");
    assert_eq!(
        cursor_line(&editor),
        7,
        "`]]` from `* Real` lands on `* Second`; line 3 is inside a block"
    );
}

/// `dar` on a real headline still takes exactly its subtree — the whole point
/// of routing through the tree is that the ordinary case is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dar_on_a_headline_takes_its_subtree_and_stops() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(
        base.path(),
        "* One\nbody\n** Child\nkid body\n* Two\ntail\n",
    )
    .await;

    // From inside the child's body: the CHILD is the subtree at point.
    goto_line(&mut editor, 3);
    delete_with_object(&mut editor, "org-around-subtree");
    let after = text(&editor);
    assert!(
        !after.contains("** Child") && !after.contains("kid body"),
        "the child subtree went: {after:?}"
    );
    assert!(
        after.contains("* One") && after.contains("body") && after.contains("* Two"),
        "its parent and its sibling stayed: {after:?}"
    );
}

/// `g{` climbs to the parent section. The text path re-derives "the nearest
/// shallower headline" from star counts; the tree walks to the ancestor node,
/// which is the same answer for well-formed org and the honest one when a
/// block sits in between.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_parent_motion_climbs_one_section() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* One\n** Two\n*** A\n*** B\nbody under B\n").await;

    goto_line(&mut editor, 3); // `*** B`
    press(&mut editor, "g{");
    assert_eq!(
        cursor_line(&editor),
        1,
        "the parent of `*** B` is `** Two`, not its `*** A` sibling"
    );

    goto_line(&mut editor, 4); // body under B
    press(&mut editor, "g{");
    assert_eq!(
        cursor_line(&editor),
        1,
        "same answer from the body under it"
    );
}

/// Demoting from inside a source block re-stars the enclosing REAL headline,
/// not the example line the caret is on.
///
/// `<leader>ol` has always acted on the section at point rather than the
/// cursor's own line — that is what makes it usable from body text. The block
/// is body text of `* Real`, so `* Real` is what moves. The text path instead
/// resolves the example line as the headline and rewrites a line inside
/// someone's code block.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_demote_inside_a_block_leaves_the_example_alone() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), BLOCK_FILE).await;

    goto_line(&mut editor, 3);
    press(&mut editor, "<leader>ol"); // demote headline
    let after = text(&editor);
    assert!(
        after.contains("\n* Fake heading in a block\n"),
        "the example line inside the block kept its single star: {after:?}"
    );
    assert!(
        after.starts_with("** Real\n"),
        "the enclosing section is what demoted: {after:?}"
    );
}

// ── OT.6: the list's shape comes from the tree ────────────────────────

/// A checkbox drawn inside a `#+BEGIN_SRC` block is example text, not an item.
///
/// `<C-Space>` on it must do nothing, and — the part that is invisible until
/// you look — it must not be counted into the enclosing cookie either. The
/// indent walk sees `- [ ] example` at column 0 and cannot tell it from a real
/// item, because nothing on the line says which side of `#+BEGIN_SRC` it is on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkbox_inside_a_block_is_neither_toggled_nor_counted() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* Shopping [0/1]\n- [ ] milk\n#+BEGIN_SRC org\n- [ ] example\n#+END_SRC\n";
    let mut editor = org_editor(base.path(), original).await;

    // On the block's example line: nothing happens at all.
    goto_line(&mut editor, 3);
    press(&mut editor, "<C-Space>");
    assert_eq!(
        text(&editor),
        original,
        "`<C-Space>` inside a source block must leave the example alone"
    );

    // On the real item: it ticks, and the cookie counts ONE item — not the
    // two the indent walk finds.
    goto_line(&mut editor, 1);
    press(&mut editor, "<C-Space>");
    assert_eq!(
        text(&editor),
        "* Shopping [1/1]\n- [X] milk\n#+BEGIN_SRC org\n- [ ] example\n#+END_SRC\n",
        "the cookie counts the one real item; `[1/2]` means the block's \
         example was tallied"
    );
}

/// The ordinary nesting case, unchanged: a nested list is counted by its own
/// parent, and the parent's box is not what the grandparent counts twice.
///
/// Structure comes from `listitem` / `list` nodes now rather than from leading
/// whitespace, so this is the regression guard that the switch did not move
/// any of the well-formed answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_list_still_rolls_up_one_level_at_a_time() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let original = "* Top [0/2]\n- [ ] a [0/2]\n  - [ ] a1\n  - [ ] a2\n- [ ] b\n";
    let mut editor = org_editor(base.path(), original).await;

    // Tick a grandchild: `a`'s cookie moves, `Top`'s does not — `a` is still
    // unticked, and `Top` counts boxes, not descendants.
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-Space>");
    assert_eq!(
        text(&editor),
        "* Top [0/2]\n- [ ] a [1/2]\n  - [X] a1\n  - [ ] a2\n- [ ] b\n",
        "the nested cookie moved and the outer one did not"
    );

    // Tick the outer item itself: now `Top` moves.
    goto_line(&mut editor, 1);
    press(&mut editor, "<C-Space>");
    assert_eq!(
        text(&editor),
        "* Top [1/2]\n- [X] a [1/2]\n  - [X] a1\n  - [ ] a2\n- [ ] b\n",
    );
}

// ── OT.7: a table is a node, and one drawn in a block is not ──────────

/// `<leader>o|` on a table written inside `#+BEGIN_SRC` must not realign it.
///
/// `is_table_line` is `trim_start().starts_with('|')`, which is true of example
/// content in a code block, so the line test finds a table there and aligns
/// someone's sample. The grammar parses the block as `block contents:` with no
/// `table` node in it at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_table_inside_a_block_is_not_aligned() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    // Deliberately ragged, so an alignment pass would be visible.
    let original = "* T\n#+BEGIN_SRC org\n|a|bb|\n|ccc|d|\n#+END_SRC\n";
    let mut editor = org_editor(base.path(), original).await;

    goto_line(&mut editor, 2);
    press(&mut editor, "<leader>o|");
    assert_eq!(
        text(&editor),
        original,
        "the example table inside the block must be left exactly as written"
    );
}

/// The ordinary table still aligns, and the bounds still stop at a blank line.
///
/// The bounds come from the `table` node's extent now rather than from walking
/// outward while lines start with `|`. Two tables separated by a blank line are
/// two `table` nodes, which is the same answer the walk gave — asserted so the
/// switch is known not to have merged them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alignment_stops_at_the_table_the_caret_is_in() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* T\n|a|bb|\n|ccc|d|\n\n|x|y|\n").await;

    goto_line(&mut editor, 1);
    press(&mut editor, "<leader>o|");
    assert_eq!(
        text(&editor),
        "* T\n| a   | bb |\n| ccc | d  |\n\n|x|y|\n",
        "the caret's table aligned; the one below the blank line is a \
         different node and must be untouched"
    );
}

// ── OT.8: capture and refile read structure from `parse-file` ─────────

/// OT.8 — a capture target must not match a headline written as an example.
///
/// The file has `* Vocabulary` in exactly one place: inside a `#+BEGIN_SRC org`
/// block, where it is sample text.
///
/// The text scan matches it. It does not file INSIDE the block — `subtree_end`
/// stops at the next real headline — it files just past `#+END_SRC`, attributed
/// to a heading that is not there, and reports success. That is wrong in a
/// quieter way than filing into the block would be, and quieter is worse: the
/// note sits somewhere the user has no reason to look. Pinned exactly in
/// `capture_target::tests::the_text_outline_matches_a_headline_written_inside_a_block`.
///
/// The tree has no section there, so the target is absent — and absent means
/// append WITH a warning, which is OC.5a's contract and exists for this.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_target_inside_a_block_is_not_a_target() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("tasks.org");
    std::fs::write(
        &notes,
        "* Inbox\n\
         #+BEGIN_SRC org\n\
         * Vocabulary\n\
         example content\n\
         #+END_SRC\n\
         * Later\n\
         tail\n",
    )
    .unwrap();

    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"task\"\n\
             target = {{ file = \"{}\", headline = \"Vocabulary\" }}\n\
             body = \"\"\"\n* TODO %?\n\"\"\"\n",
            notes.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "learn a word");

    let written = text_of(&editor, &notes);
    let lines: Vec<&str> = written.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.contains("learn a word"))
        .unwrap_or_else(|| panic!("the note survived somewhere: {written:?}"));
    let end_src = lines
        .iter()
        .position(|l| *l == "#+END_SRC")
        .expect("the block is intact");

    assert!(
        at > end_src,
        "the note must land after the block: {written:?}"
    );
    assert_eq!(
        at,
        lines.len() - 1,
        "with no real `* Vocabulary` the target is absent, so capture appends \
         (and warns): {written:?}"
    );
    assert!(
        written.contains("#+BEGIN_SRC org\n* Vocabulary\nexample content\n#+END_SRC"),
        "and the block is left exactly as written: {written:?}"
    );
}

// ── AG.1: org owns its agenda trigger ─────────────────────────────────

/// The agenda's ex-command is `:org-agenda`, and this plugin registers it.
///
/// It was `:agenda`, registered by `lattice-multibuffer` beside the provider,
/// because a plugin had no way to open a provider view — `OpenProviderView` was
/// withheld from the WIT surface. So a feature every user calls `org-agenda`
/// shipped under a generic name that org could not correct from its own side.
///
/// The VIEW is still generic host machinery and this plugin still supplies rows
/// only through the `scanned-excerpt-source` seam. What moved is the trigger.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agenda_command_is_org_agenda_and_org_registers_it() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor(base.path(), "* One\n").await;
    let commands = editor.registry.load();

    assert!(
        commands.id_by_name("org-agenda").is_some(),
        "the plugin registers `:org-agenda`"
    );
    // The generic name is GONE, not aliased. CLAUDE.md's ex-command rule is
    // explicit that a generic name implies the command works regardless of the
    // subsystem behind it, and no back-compat aliases pre-1.0.
    assert!(
        commands.id_by_name("agenda").is_none(),
        "`:agenda` must not survive as an alias"
    );

    // Every command this plugin ships carries the `org-` prefix. Asserted as a
    // property rather than a list, so a new command cannot quietly break the
    // convention by being added without anyone updating a fixture.
    let stray: Vec<String> = commands
        .names()
        .filter(|n| n.starts_with("org") && !n.starts_with("org-"))
        .map(str::to_string)
        .collect();
    assert!(
        stray.is_empty(),
        "org commands must be `org-` prefixed: {stray:?}"
    );
}

/// OC.11 — `clock-in = true` on a template starts a clock on the entry it
/// captures.
///
/// The interesting part is not the flag, it is WHERE the clock line comes from.
/// Capture files into another file, and an `apply-edit` names a buffer id an
/// unopened file does not have — so the drawer is built into the captured TEXT
/// and rides the same single write. That is what this asserts: one write, and
/// the entry lands already clocked, with no window in which it exists and its
/// clock does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_template_can_clock_in_on_what_it_captures() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"todo\"\nclock-in = true\n\
             target = {{ file = \"{}\" }}\nbody = \"\"\"\n* TODO %?\n\"\"\"\n",
            notes.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "call the bank");

    let written = text_of(&editor, &notes);
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines[0], "* TODO call the bank");
    assert_eq!(lines[1], ":LOGBOOK:", "the drawer rode in with the entry");
    assert!(
        lines[2].starts_with("CLOCK: [") && lines[2].ends_with(']'),
        "a RUNNING clock — a start stamp with no end: {:?}",
        lines[2]
    );
    assert_eq!(lines[3], ":END:");
}

/// The flag is opt-in, and its absence must not change a capture at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_template_without_the_flag_captures_no_clock() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"todo\"\n\
             target = {{ file = \"{}\" }}\nbody = \"\"\"\n* TODO %?\n\"\"\"\n",
            notes.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "call the bank");

    assert_eq!(
        text_of(&editor, &notes),
        "* TODO call the bank\n",
        "no drawer, no clock line — the default is unchanged"
    );
}

/// A body with more than a headline keeps its remaining lines, below the
/// drawer. The drawer goes directly under the headline, which is where
/// `clock.rs` puts one for a fresh entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_clock_drawer_sits_under_the_headline_not_over_the_body() {
    if org_plugin_wasm().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("inbox.org");
    let mut editor = org_editor_with_caps(base.path(), "* One\n", &fs_write(base.path())).await;
    set_org_option(
        &mut editor,
        "capture-templates",
        &format!(
            "[[template]]\nkey = \"t\"\ndescription = \"todo\"\nclock-in = true\n\
             target = {{ file = \"{}\" }}\nbody = \"\"\"\n* TODO %?\nsome body\n\"\"\"\n",
            notes.to_str().unwrap()
        ),
    );

    submit_capture(&mut editor, "call the bank");

    let written = text_of(&editor, &notes);
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines[0], "* TODO call the bank");
    assert_eq!(lines[1], ":LOGBOOK:");
    assert_eq!(lines[3], ":END:");
    assert_eq!(lines[4], "some body", "the body survived, below the drawer");
}

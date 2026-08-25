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
            ..Default::default()
        },
    )
}

/// Boot an editor, load the org plugin into its live registries, and open a
/// `.org` file holding `text`. Returns the editor and the file path.
async fn org_editor(base: &std::path::Path, text: &str) -> Editor {
    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\"]\ndefault_mode = \"org-todo-mode\"\n",
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

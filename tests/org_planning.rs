//! OA.25 — `s` and `d`, in a file and in the agenda, through a real editor.
//!
//! Slice plan: `lattice/docs/dev/operations/slice-plans/org-agenda.md` phase 7.
//!
//! `src/planning.rs`'s unit tests cover the edit rules on the host target, and
//! they are the sharper test of "does adding a SCHEDULED keep the DEADLINE".
//! These cover what a unit test structurally cannot: that the chord resolves,
//! that the prompt's second hop finds the same headline the first one did, and
//! — the whole reason phase 7 waited for a host seam — that the agenda's write
//! lands in the SOURCE document rather than nowhere.
//!
//! The agenda case is the one that has no cheaper coverage. An agenda row is
//! one line, the headline; the planning line goes below it, outside every
//! excerpt. So the edit cannot be a composed one, cannot be addressed by path
//! (which may name a different document — see `source-location.buffer`), and
//! could not be addressed at all until OA.23b.
//!
//! Skips when the component was not built — `cargo test` builds for the HOST,
//! and the component is a separate `--target wasm32-wasip2 --release` artefact.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_multibuffer::registry::MultibufferRegistryHandle;
use lattice_multibuffer::HeaderlineStatus;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::{parse_chord_sequence, KeyChord};
use lattice_runtime::Document as _;

fn org_plugin_wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    );
    std::fs::read(path).ok()
}

fn boot_sealed_editor() -> Editor {
    lattice_plugin_loader::disable_autoload();
    Editor::boot(CoreDocument::from_text("scratch\n"))
}

fn write_org_plugin_dir(root: &std::path::Path, wasm: &[u8]) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\n\
         provides = [\"scanned-excerpt-source\", \"multibuffer-view-source\", \"modes\", \
         \"grammar\", \"language\", \"help\", \"config\", \"transient-source\"]\n\
         default_modes = [\"org-todo-mode\", \"org-global-mode\"]\n\
         editor_capabilities = [\"tree-sitter\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();
}

fn loader_over_editor(editor: &Editor, base: &std::path::Path) -> PluginLoader {
    let host = Arc::new(
        PluginHost::with_dirs(base.join("cache"), base.join("data")).expect("host builds"),
    );
    // OA.23b: wire the resolver the way `lattice_plugin_loader::install` does
    // at boot. A harness that builds its own `PluginHost` bypasses `install`
    // entirely, so `excerpt-source` answers `none` and every agenda row looks
    // like a plain buffer — the guest then takes its FILE path and the test
    // passes through code the user never runs.
    if let Some(views) = editor.services.get::<MultibufferRegistryHandle>() {
        host.set_excerpt_source_resolver(Arc::new(
            lattice_multibuffer::registry::MultibufferExcerptSource::new((*views).clone()),
        ));
    }
    // OA.28: the same, for `view-args`. Unwired it answers an EMPTY LIST, which
    // the guest reads as a fresh view — so every span/filter chord silently
    // starts over from the default and the tests below pass on a build where
    // the seam does nothing. That is the exact failure this suite exists to
    // catch, so the harness has to mirror `install` here or catch nothing.
    if let Some(scan_views) = editor
        .services
        .get::<lattice_multibuffer::providers::scan_view::ScanViewServiceHandle>(
    ) {
        host.set_view_args_resolver(Arc::new(
            lattice_multibuffer::providers::scan_view::ScanViewArgs::new((*scan_views).clone()),
        ));
    }
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
            // OM.13: the TODO menu is a transient, and without somewhere to
            // register the source `<C-c><C-t>` reports `unknown source`.
            transient_registry: editor
                .services
                .get::<lattice_picker::TransientSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            agenda_registry: editor
                .services
                .get::<lattice_mode::ScannedExcerptSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            provider_view_registry: editor
                .services
                .get::<lattice_mode::ProviderViewRegistryHandle>()
                .map(|h| (*h).clone()),
            multibuffer_registry: editor
                .services
                .get::<MultibufferRegistryHandle>()
                .map(|h| (*h).clone()),
            ..Default::default()
        },
    )
}

/// Expand the plugin's grammar rows into the keymap, as the renderer does at
/// boot. Without it a plugin chord resolves to nothing and every assertion
/// below fails on the text with no clue why.
fn expand_plugin_keymaps(editor: &Editor) {
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

/// Press a chord and apply the RENDERER-owned effects it produced.
///
/// `dispatch_chord` runs the action but returns the resolved `Action` rather
/// than its `DispatchOutcome`, and `Effect::OpenPrompt` lives in that discarded
/// tail — the hole that let a plugin's prompt submit never fire while every
/// capture test still passed. Re-dispatching an `Invoke` is how the sibling
/// suites recover it; safe here because both chords are pure prompt-openers.
///
/// Returns what the prompt was pre-filled with, which the editor exposes
/// nowhere else — there is a `pending_prompt_submit_action` but no
/// `pending_prompt_initial`.
fn press(editor: &mut Editor, keys: &str) -> String {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial: Vec<KeyChord> = Vec::new();
    let mut resolved = None;
    for c in seq {
        resolved = Some(editor.dispatch_chord(c, &mut partial));
    }
    let Some(lattice_host::action::Action::Invoke(inv)) = resolved else {
        return String::new();
    };
    let out = editor.dispatch(lattice_host::action::Action::Invoke(inv));
    apply_effects(editor, out)
}

/// Apply the effects an outcome carries, returning an `OpenPrompt`'s `initial`.
/// `press`, but returning the outcome rather than a prompt's initial — for
/// chords that produce an effect instead of opening a prompt.
fn press_raw(editor: &mut Editor, keys: &str) -> lattice_host::dispatch::DispatchOutcome {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial: Vec<KeyChord> = Vec::new();
    let mut resolved = None;
    for c in seq {
        resolved = Some(editor.dispatch_chord(c, &mut partial));
    }
    match resolved {
        Some(lattice_host::action::Action::Invoke(inv)) => {
            editor.dispatch(lattice_host::action::Action::Invoke(inv))
        }
        _ => lattice_host::dispatch::DispatchOutcome::default(),
    }
}

fn apply_effects(editor: &mut Editor, out: lattice_host::dispatch::DispatchOutcome) -> String {
    let mut seen_initial = String::new();
    for effect in out.effects {
        match effect {
            lattice_grammar::Effect::OpenPrompt {
                prompt,
                initial,
                on_submit_action,
                buffer_name,
            } => {
                seen_initial = initial.clone();
                editor.open_prompt_line(prompt, initial, on_submit_action, buffer_name);
            }
            // A menu build is a guest call parked on the async-landed wake, so
            // the caller settles it after applying this.
            lattice_grammar::Effect::OpenTransient { source, args } => {
                editor.open_named_transient(source, args);
            }
            lattice_grammar::Effect::OpenBufferAt {
                path,
                position,
                force,
            } => {
                editor.do_edit(path.clone(), force);
                editor.run_tick_pending();
                editor.cursor = position;
            }
            lattice_grammar::Effect::ApplyEdit {
                target,
                edit,
                cursor,
            } => {
                let _ = editor.dispatch(lattice_host::action::Action::ApplyEdit {
                    target,
                    edit,
                    cursor,
                });
            }
            _ => {}
        }
    }
    seen_initial
}

/// Type `text` into the open prompt and submit it, the way `<CR>` does — then
/// apply what the submit produced, which is where the edit is.
fn submit_prompt(editor: &mut Editor, text: &str) {
    let action = editor
        .pending_prompt_submit_action
        .clone()
        .expect("the chord opened a prompt");
    let name = editor.pending_prompt_buffer_name.clone();
    // Re-open with the text seeded: `open_prompt_line` writes `initial` into
    // the prompt buffer and submit reads that buffer's first line, so this is
    // the value the user would have typed.
    editor.open_prompt_line(String::new(), text.to_string(), action, name);
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_prompt_line_submit(&mut out);
    apply_effects(editor, out);
    editor.run_tick_pending();
}

// ─────────────────────────────────────────────────────────────
//  In a file
// ─────────────────────────────────────────────────────────────

async fn org_editor(base: &std::path::Path, text: &str) -> Editor {
    let plugins_dir = base.join("plugins");
    write_org_plugin_dir(&plugins_dir, &org_plugin_wasm().expect("caller checked"));
    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base)
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1,
        "the org component loads"
    );
    expand_plugin_keymaps(&editor);
    editor.run_tick_pending();
    let file = base.join("notes.org");
    std::fs::write(&file, text).unwrap();
    editor.do_edit(Some(file), false);
    editor.run_tick_pending();
    editor
}

/// The agenda's composed text — what the user sees in the view.
fn composed_text(handle: &Arc<lattice_multibuffer::MultibufferDocumentHandle>) -> String {
    handle.snapshot().buffer.as_string()
}

fn text(editor: &Editor) -> String {
    editor.document.snapshot().text().to_string()
}

fn goto_line(editor: &mut Editor, line: u32) {
    editor.cursor.line = line;
    editor.cursor.byte = 0;
}

/// A date far enough out that no test asserts against "today" by accident, and
/// spelled absolutely so the assertion states the day it expects.
const WHEN: &str = "2026-09-03";
const WHEN_STAMP: &str = "<2026-09-03 Thu>";
const LATER: &str = "2026-09-10";
const LATER_STAMP: &str = "<2026-09-10 Thu>";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduling_an_unplanned_headline_adds_a_planning_line() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO Ship the thing\nbody\n").await;

    goto_line(&mut editor, 0);
    assert_eq!(
        press(&mut editor, "<leader>os"),
        "",
        "nothing scheduled yet, so nothing to pre-fill"
    );
    submit_prompt(&mut editor, WHEN);

    assert_eq!(
        text(&editor),
        format!("* TODO Ship the thing\nSCHEDULED: {WHEN_STAMP}\nbody\n"),
    );
}

/// The pair that a naive implementation gets wrong: two fields, one line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deadline_and_a_schedule_share_one_line() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO Ship the thing\nbody\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>od");
    submit_prompt(&mut editor, LATER);

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>os");
    submit_prompt(&mut editor, WHEN);

    assert_eq!(
        text(&editor),
        format!("* TODO Ship the thing\nDEADLINE: {LATER_STAMP} SCHEDULED: {WHEN_STAMP}\nbody\n"),
        "org reads only the FIRST line after a headline as planning, so a \
         second line would silently lose the deadline"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rescheduling_replaces_and_pre_fills() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(
        base.path(),
        "* TODO Ship the thing\n  SCHEDULED: <2026-09-01 Tue>\nbody\n",
    )
    .await;

    goto_line(&mut editor, 0);
    assert_eq!(
        press(&mut editor, "<leader>os"),
        "2026-09-01 Tue",
        "the prompt opens showing what is there — retyping a date you can see \
         is the difference between changing one and re-entering one"
    );
    submit_prompt(&mut editor, WHEN);

    assert_eq!(
        text(&editor),
        format!("* TODO Ship the thing\n  SCHEDULED: {WHEN_STAMP}\nbody\n"),
    );
}

/// An empty answer removes the line. Emacs' behaviour, and the only spelling
/// of "unschedule" that does not need a second key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_answer_unschedules() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(
        base.path(),
        "* TODO Ship the thing\n  SCHEDULED: <2026-09-01 Tue>\nbody\n",
    )
    .await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>os");
    submit_prompt(&mut editor, "");

    assert_eq!(
        text(&editor),
        "* TODO Ship the thing\nbody\n",
        "the whole line goes, not just its contents — a blank line between a \
         headline and its body is not what unscheduling means"
    );
}

/// The date grammar is the point of the prompt: `+1d` beats working out what
/// Thursday's date is. Asserted through a RELATIVE form against an absolute
/// anchor typed in the same test, so it states the day it expects.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relative_date_resolves() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO Ship the thing\nbody\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>os");
    submit_prompt(&mut editor, "+1d");

    let body = text(&editor);
    assert!(
        body.contains("SCHEDULED: <"),
        "a relative date produced a stamp: {body:?}"
    );
    assert!(
        !body.contains("+1d"),
        "…a resolved one, not the expression typed: {body:?}"
    );
}

/// A date the parser did not understand is echoed, not swallowed. A key that
/// silently does nothing teaches the user the feature is broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unparseable_date_leaves_the_buffer_alone() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let before = "* TODO Ship the thing\nbody\n";
    let mut editor = org_editor(base.path(), before).await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>os");
    submit_prompt(&mut editor, "sometime next week-ish");

    assert_eq!(text(&editor), before);
}

// ─────────────────────────────────────────────────────────────
//  In the agenda
// ─────────────────────────────────────────────────────────────

fn org_agenda_identity() -> lattice_multibuffer::providers::scan_view::ScanViewIdentity {
    lattice_multibuffer::providers::scan_view::ScanViewIdentity {
        provider: "agenda".to_string(),
        buffer_name: "*agenda*".to_string(),
        view_mode: None,
        no_rows_message: "no plugin provides agenda rows".to_string(),
    }
}

async fn settle_agenda(registry: &MultibufferRegistryHandle, view: lattice_core::BufferId) {
    for _ in 0..settle_budget(600) {
        if let Some(h) = registry.handle(view) {
            if matches!(
                *h.headerline(),
                HeaderlineStatus::Complete { .. } | HeaderlineStatus::Failed { .. }
            ) {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the agenda scan never settled");
}

/// Today's stamp, as the guest computes it — the row has to be dated to appear.
fn today_stamp() -> String {
    // LOCAL, like the guest — see the note on `today()` in `org_agenda.rs`.
    // Raw UTC seconds here agreed with the guest only while the two happened
    // to share a day, so this passed all afternoon and failed after local
    // midnight.
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let offset = i64::from(chrono::Local::now().offset().local_minus_utc());
    let z = (utc + offset).div_euclid(86_400) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let (zm, zy) = if m < 3 { (m + 12, y - 1) } else { (m, y) };
    let (k, j) = (zy % 100, zy / 100);
    let h = (d + (13 * (zm + 1)) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!(
        "<{y:04}-{m:02}-{d:02} {}>",
        NAMES[(((h + 6) % 7) as usize) % 7]
    )
}

/// Phase 7's exit criterion, and the reason it waited on a host seam.
///
/// A `DEADLINE:` set from the agenda goes on a line the view does not compose.
/// It cannot be a composed edit, and addressing the file by path would edit a
/// DIFFERENT document from the one the view owns and `:w` saves. Only the
/// source id reaches the right one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_a_deadline_in_the_agenda_writes_the_source_document() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("only.org"),
        format!(
            "* TODO Ship the thing\n  SCHEDULED: {}\nbody\n",
            today_stamp()
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    expand_plugin_keymaps(&editor);

    let view = match lattice_multibuffer::providers::scan_view::open_scan_view(
        &mut editor,
        &org_agenda_identity(),
        &lattice_grammar::Args::String(notes.display().to_string()),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        lattice_mode::ProviderViewOutcome::Declined { message } => {
            panic!("the agenda declined: {message}")
        }
    };
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();
    settle_agenda(&mb, view).await;
    let handle = mb.handle(view).unwrap();
    assert_eq!(handle.excerpts().len(), 1, "one dated row");
    let composed_before = composed_text(&handle);

    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    assert_eq!(
        press(&mut editor, "<leader>od"),
        "",
        "no deadline yet — and the prompt must not offer the SCHEDULED one"
    );
    submit_prompt(&mut editor, LATER);

    let source = handle.excerpts()[0].source;
    let source_text = handle
        .source_text(source)
        .expect("the row's source document is still attached");
    assert!(
        source_text.contains(&format!("DEADLINE: {LATER_STAMP} SCHEDULED:")),
        "the deadline joined the EXISTING planning line in the source, got \
         {source_text:?}"
    );

    assert_eq!(
        composed_text(&handle),
        composed_before,
        "the agenda must not grow a row — the planning line is outside the \
         excerpt, and a pixel change to content the user did not edit is the \
         thing the UX contract vetoes"
    );
}

// ─────────────────────────────────────────────────────────────
//  OA.26 — the rest of OA.23's consumers
// ─────────────────────────────────────────────────────────────

/// Build an agenda over `notes/` with two dated files, and return the pieces a
/// row-acting test needs.
async fn agenda_over_two_files(
    base: &std::path::Path,
) -> (Editor, lattice_core::BufferId, MultibufferRegistryHandle) {
    let plugins_dir = base.join("plugins");
    write_org_plugin_dir(&plugins_dir, &org_plugin_wasm().expect("caller checked"));
    let notes = base.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("alpha.org"),
        format!("* TODO From alpha\n  SCHEDULED: {}\n", today_stamp()),
    )
    .unwrap();
    std::fs::write(
        notes.join("beta.org"),
        format!("* TODO From beta\n  SCHEDULED: {}\n", today_stamp()),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base)
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    expand_plugin_keymaps(&editor);
    let view = match lattice_multibuffer::providers::scan_view::open_scan_view(
        &mut editor,
        &org_agenda_identity(),
        &lattice_grammar::Args::String(notes.display().to_string()),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        lattice_mode::ProviderViewOutcome::Declined { message } => {
            panic!("the agenda declined: {message}")
        }
    };
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();
    settle_agenda(&mb, view).await;
    (editor, view, mb)
}

/// `<CR>` on a row opens the FILE it came from, at the headline's own line —
/// not the composed line the row is displayed at. The two coordinate spaces
/// are unrelated, so a test whose row sits at composed 0 and source 0 would
/// pass on an implementation that confused them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_on_a_row_opens_the_file_it_came_from() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, view, mb) = agenda_over_two_files(base.path()).await;
    let handle = mb.handle(view).unwrap();
    assert_eq!(handle.excerpts().len(), 2, "one row per file");

    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    let out = press_raw(&mut editor, "<CR>");
    apply_effects(&mut editor, out);
    editor.run_tick_pending();

    let path = editor
        .document
        .snapshot()
        .path()
        .map(|p| p.to_path_buf())
        .expect("the row's file is open and focused");
    assert!(
        path.ends_with("alpha.org") || path.ends_with("beta.org"),
        "landed in one of the two source files, got {path:?}"
    );
    assert_ne!(
        editor.active_pane_buffer_id(),
        view,
        "the agenda is no longer what the pane shows"
    );
}

/// `<` restricts the agenda to the row's own file — the key OA.21 shipped the
/// `file:` term for and could not bind, because a guest in a multibuffer had
/// no way to learn which file a row came from.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn angle_restricts_the_agenda_to_the_rows_file() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, view, mb) = agenda_over_two_files(base.path()).await;
    assert_eq!(mb.handle(view).unwrap().excerpts().len(), 2);

    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    let out = press_raw(&mut editor, "<lt>");
    apply_effects(&mut editor, out);
    editor.run_tick_pending();

    // The re-opened view is the same named buffer; settle its new scan.
    let restricted = editor
        .services
        .get::<lattice_mode::BufferStoreHandle>()
        .expect("the buffer store is a service")
        .find_by_name("*agenda*")
        .expect("the agenda re-opened under its own name");
    settle_agenda(&mb, restricted).await;
    assert_eq!(
        mb.handle(restricted).unwrap().excerpts().len(),
        1,
        "only the row's own file survives the restriction"
    );
}

/// OA.22 — the headerline says what you are looking at.
///
/// The end-to-end half: `src/agenda_args.rs` tests the phrase, this tests that
/// the phrase crosses the seam and lands where the user reads it. A filtered
/// agenda that looks unfiltered is the trap the slice exists to close.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filtered_agendas_headerline_names_the_filter() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &org_plugin_wasm().unwrap());
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("only.org"),
        format!("* TODO Ship it :work:\n  SCHEDULED: {}\n", today_stamp()),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();

    // Unfiltered first: the plain form, so the filtered one is a difference
    // rather than a string that was always there.
    let view = match lattice_multibuffer::providers::scan_view::open_scan_view(
        &mut editor,
        &org_agenda_identity(),
        &lattice_grammar::Args::List(vec![lattice_grammar::args::ArgValue::String(
            notes.display().to_string(),
        )]),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        lattice_mode::ProviderViewOutcome::Declined { message } => panic!("{message}"),
    };
    settle_agenda(&mb, view).await;
    let plain = headerline_of(&mb, view);
    // `Week` before the date, not just the date: the span is NAMED as of
    // "the agenda headerline names its span", because `gD` switches between
    // four of them and the name is what tells you the key worked. This
    // asserted `"[agenda: 20"` until that landed — the window still starts
    // with its year, but no longer *first*, so the old form said the header
    // had lost the window when what it had gained was a label.
    //
    // `org.agenda-span` is unset here, so the default 7 makes it a week.
    assert!(
        plain.contains("[agenda: Week 20"),
        "an unfiltered agenda still says WHAT it is looking at and WHEN — the \
         span and the window are what `gD` and `f`/`b` change, and the only \
         thing that says where you are: {plain:?}"
    );
    assert!(
        !plain.contains('+'),
        "…and names no filter, so the filtered case below is a difference: \
         {plain:?}"
    );

    // Now the same corpus, narrowed by a tag.
    let filtered = match lattice_multibuffer::providers::scan_view::open_scan_view(
        &mut editor,
        &org_agenda_identity(),
        &lattice_grammar::Args::List(vec![
            lattice_grammar::args::ArgValue::String(notes.display().to_string()),
            lattice_grammar::args::ArgValue::String("tag:work".to_string()),
        ]),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        lattice_mode::ProviderViewOutcome::Declined { message } => panic!("{message}"),
    };
    settle_agenda(&mb, filtered).await;
    let said = headerline_of(&mb, filtered);
    assert!(
        said.contains("+work"),
        "the header names the filter that is narrowing the view, got {said:?}"
    );

    // …and the label is what the theme accents. Without this the phrase renders
    // in the same colour as the counts beside it, which is the part that means
    // least — see `multibuffer.status.query`.
    let accent = headerline_emphasis(&mb, filtered).expect("the label is accented");
    assert!(
        accent.contains("+work") && said.contains(&accent),
        "the accent is a substring of the summary, and names the filter: \
         {accent:?} in {said:?}"
    );
}

/// The substring the renderer paints with `multibuffer.status.query`.
fn headerline_emphasis(
    mb: &MultibufferRegistryHandle,
    view: lattice_core::BufferId,
) -> Option<String> {
    match &*mb
        .handle(view)
        .expect("the view is registered")
        .headerline()
    {
        HeaderlineStatus::Complete { emphasis, .. } => emphasis.clone(),
        other => panic!("the scan has not settled: {other:?}"),
    }
}

fn headerline_of(mb: &MultibufferRegistryHandle, view: lattice_core::BufferId) -> String {
    match &*mb
        .handle(view)
        .expect("the view is registered")
        .headerline()
    {
        HeaderlineStatus::Complete { summary, .. } => summary.clone(),
        other => panic!("the scan has not settled: {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────
//  OM.13 — `C-c C-t`, and the agenda's filter round-trip
// ─────────────────────────────────────────────────────────────

/// Drain until a parked transient build has seated its menu.
async fn settle_transient(
    editor: &mut Editor,
) -> Option<std::sync::Arc<lattice_picker::TransientSpec>> {
    for _ in 0..settle_budget(100) {
        editor.run_tick_pending();
        if let Some(spec) = editor.picker.as_ref().and_then(|p| p.transient.clone()) {
            return Some(spec);
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    None
}

/// `C-c C-t` offers the configured states, in a file.
///
/// The menu, not the cycle — emacs' own behaviour once fast-select is on, and
/// what makes reaching a state four presses away one keystroke.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_ctrl_t_offers_the_configured_states() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO ship it\n").await;
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: "org.todo-keywords=sequence: TODO(t) NEXT(n) | DONE(d)".to_string(),
    });

    goto_line(&mut editor, 0);
    let out = press_raw(&mut editor, "<C-c><C-t>");
    apply_effects(&mut editor, out);
    let spec = settle_transient(&mut editor).await.unwrap_or_else(|| {
        panic!(
            "`<C-c><C-t>` opened the state menu (last message: {:?})",
            editor.last_message.as_ref().map(|m| &m.text)
        )
    });

    let labels: Vec<&str> = spec.groups[0]
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert!(
        labels.contains(&"NEXT") && labels.contains(&"DONE"),
        "every configured state is offered, so any is one keystroke away: {labels:?}"
    );

    // And picking one sets it — a menu that lists states and cannot apply one
    // is the same non-feature as no menu.
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_transient_trigger("n".to_string(), &mut out);
    apply_effects(&mut editor, out);
    editor.run_tick_pending();
    assert_eq!(text(&editor), "* NEXT ship it\n");
}

/// The same key, in the agenda — the surface you reported it failing on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_ctrl_t_offers_the_states_in_the_agenda_too() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, view, mb) = agenda_over_two_files(base.path()).await;
    let handle = mb.handle(view).unwrap();

    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    let out = press_raw(&mut editor, "<C-c><C-t>");
    apply_effects(&mut editor, out);
    let spec = settle_transient(&mut editor).await.unwrap_or_else(|| {
        panic!(
            "`<C-c><C-t>` opened the state menu in the agenda (last message: {:?})",
            editor.last_message.as_ref().map(|m| &m.text)
        )
    });
    assert!(
        spec.groups[0].items.iter().any(|i| i.label == "DONE"),
        "the agenda offers the same states a file does"
    );

    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_transient_trigger("d".to_string(), &mut out);
    apply_effects(&mut editor, out);
    editor.run_tick_pending();

    let source = handle.excerpts()[0].source;
    let text = handle.source_text(source).expect("the source is attached");
    assert!(
        text.starts_with("* DONE "),
        "the pick landed in the SOURCE file, not only in the view: {text:?}"
    );
}

/// OA.21 — `/` narrows by tag and `|` clears every filter.
///
/// Written now because the keys shipped at OA.21 with NO integration test, and
/// "how do I reset the filter" is the question an untested clear produces. A
/// filter you cannot undo is worse than no filter: the agenda keeps showing a
/// subset and nothing on screen says why.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tag_filter_narrows_and_the_pipe_clears_it() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &org_plugin_wasm().unwrap());
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("only.org"),
        format!(
            "* TODO Tagged one :work:\n  SCHEDULED: {}\n* TODO Untagged one\n  SCHEDULED: {}\n",
            today_stamp(),
            today_stamp()
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    expand_plugin_keymaps(&editor);
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();

    let open = |editor: &mut Editor, args: Vec<&str>| {
        match lattice_multibuffer::providers::scan_view::open_scan_view(
            editor,
            &org_agenda_identity(),
            &lattice_grammar::Args::List(
                args.into_iter()
                    .map(|a| lattice_grammar::args::ArgValue::String(a.to_string()))
                    .collect(),
            ),
        ) {
            lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
            lattice_mode::ProviderViewOutcome::Declined { message } => panic!("{message}"),
        }
    };

    let root = notes.display().to_string();
    let view = open(&mut editor, vec![&root]);
    settle_agenda(&mb, view).await;
    assert_eq!(
        mb.handle(view).unwrap().excerpts().len(),
        2,
        "both rows before any filter"
    );

    // Narrow, the way `st` does — the chord opens a prompt, and the submit
    // re-scans with the tag appended. OA.29 moved this off `/`, which is the
    // builtin search again.
    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    let out = press_raw(&mut editor, "st");
    apply_effects(&mut editor, out);
    submit_prompt(&mut editor, "work");
    let narrowed = editor
        .services
        .get::<lattice_mode::BufferStoreHandle>()
        .unwrap()
        .find_by_name("*agenda*")
        .expect("the agenda re-opened");
    settle_agenda(&mb, narrowed).await;
    assert_eq!(
        mb.handle(narrowed).unwrap().excerpts().len(),
        1,
        "only the tagged row survives the filter"
    );

    // …and `|` puts them back. THE question this test exists to answer.
    let _ = editor.activate_buffer(narrowed);
    editor.cursor.line = 0;
    let out = press_raw(&mut editor, "|");
    apply_effects(&mut editor, out);
    let cleared = editor
        .services
        .get::<lattice_mode::BufferStoreHandle>()
        .unwrap()
        .find_by_name("*agenda*")
        .expect("the agenda re-opened");
    settle_agenda(&mb, cleared).await;
    assert_eq!(
        mb.handle(cleared).unwrap().excerpts().len(),
        2,
        "`|` restores every row — a filter you cannot undo is worse than none"
    );
}

/// `|` clears the FILTER and nothing else — the span the reader walked to
/// survives it.
///
/// Emacs' `org-agenda-filter-remove-all` removes the filters and redisplays;
/// it does not touch `org-agenda-span`. The span is a property of the reader
/// (OA.24), the same reasoning that makes it survive a view switch, and a
/// clear that also snapped the view back to the default week would cost the
/// reader their place for pressing the undo key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clearing_the_filter_keeps_the_span_the_reader_walked_to() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &org_plugin_wasm().unwrap());
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("only.org"),
        format!(
            "* TODO Tagged one :work:\n  SCHEDULED: {}\n* TODO Untagged one\n  SCHEDULED: {}\n",
            today_stamp(),
            today_stamp()
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    expand_plugin_keymaps(&editor);
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();

    let view = match lattice_multibuffer::providers::scan_view::open_scan_view(
        &mut editor,
        &org_agenda_identity(),
        &lattice_grammar::Args::String(notes.display().to_string()),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        lattice_mode::ProviderViewOutcome::Declined { message } => panic!("{message}"),
    };
    settle_agenda(&mb, view).await;

    // The agenda under its own name, whichever scan is current — every step
    // below re-opens the view, and `reuse: true` keeps the name stable while
    // the id need not be.
    fn current(editor: &Editor) -> lattice_core::BufferId {
        editor
            .services
            .get::<lattice_mode::BufferStoreHandle>()
            .unwrap()
            .find_by_name("*agenda*")
            .expect("the agenda is open under its own name")
    }
    fn header(mb: &MultibufferRegistryHandle, view: lattice_core::BufferId) -> String {
        match &*mb
            .handle(view)
            .expect("the view is registered")
            .headerline()
        {
            HeaderlineStatus::Complete { summary, .. } => summary.clone(),
            other => panic!("the agenda did not complete: {other:?}"),
        }
    }

    // Walk to a month, the way `gD m` does.
    let month_id = editor
        .registry
        .load()
        .id_by_name("org-agenda-month-view")
        .expect("`org-agenda-month-view` is registered");
    let out = editor.dispatch(lattice_host::action::Action::Invoke(
        lattice_grammar::CommandInvocation::of(month_id),
    ));
    apply_effects(&mut editor, out);
    let month_view = current(&editor);
    settle_agenda(&mb, month_view).await;
    let walked = header(&mb, month_view);
    assert!(
        walked.contains("Month "),
        "the reader walked to a month: {walked:?}"
    );

    // Narrow it. `st` since OA.29 — `/` is search.
    let _ = editor.activate_buffer(month_view);
    editor.cursor.line = 0;
    let out = press_raw(&mut editor, "st");
    apply_effects(&mut editor, out);
    submit_prompt(&mut editor, "work");
    let narrowed_view = current(&editor);
    settle_agenda(&mb, narrowed_view).await;
    let narrowed = header(&mb, narrowed_view);
    assert!(
        narrowed.contains("Month ") && narrowed.contains("+work"),
        "the filter narrows the month rather than replacing it: {narrowed:?}"
    );

    // …and clear it. The span must still be a month.
    let _ = editor.activate_buffer(narrowed_view);
    editor.cursor.line = 0;
    let out = press_raw(&mut editor, "|");
    apply_effects(&mut editor, out);
    let cleared_view = current(&editor);
    settle_agenda(&mb, cleared_view).await;
    let cleared = header(&mb, cleared_view);
    assert!(
        !cleared.contains("+work"),
        "`|` dropped the filter: {cleared:?}"
    );
    assert!(
        cleared.contains("Month "),
        "`|` clears the filter and nothing else — the span survives it, as \
         emacs' `org-agenda-filter-remove-all` leaves `org-agenda-span` alone. \
         Got {cleared:?} (was {narrowed:?})"
    );
}

/// OA.29 — `/` is the builtin forward search again, not the tag filter.
///
/// The slice's headline claim, and a chord test is the only thing that can see
/// it: the filter action still exists and still works, so nothing about the
/// guest says which key reaches it. What changed is the keymap, and the way
/// that fails is silently — a `/` still bound here would open a filter prompt
/// and the user would be unable to search inside the one buffer they most want
/// to search.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slash_is_search_in_the_agenda_not_the_tag_filter() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, view, _mb) = agenda_with_rows(base.path()).await;
    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;

    let out = press_raw(&mut editor, "/");
    let opened_a_prompt = out.effects.iter().any(|e| {
        matches!(
            e,
            lattice_grammar::Effect::OpenPrompt {
                on_submit_action,
                ..
            } if on_submit_action.contains("filter")
        )
    });
    assert!(
        !opened_a_prompt,
        "`/` must not reach org's filter prompt: {:?}",
        out.effects
    );
}

/// Every `s`-prefixed filter reaches its own action.
///
/// A prefix that is also bound alone is dead — `KeymapTrie::lookup` stops at
/// the first node carrying a binding — and the failure is quiet, because the
/// trailing letter falls through to the grammar in a read-only view. So this
/// presses all six and asserts each opened the prompt it should have.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_s_filter_chord_opens_its_own_prompt() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, view, _mb) = agenda_with_rows(base.path()).await;

    for (chord, want) in [
        ("st", "Filter by tag: "),
        ("sT", "Filter by title: "),
        ("sb", "Filter by body: "),
        ("sc", "Filter by category: "),
        ("sf", "Filter by file: "),
        ("sr", "Filter by regexp: "),
        ("\\", "Also filter by tag: "),
    ] {
        let _ = editor.activate_buffer(view);
        editor.cursor.line = 0;
        let out = press_raw(&mut editor, chord);
        let prompt = out.effects.iter().find_map(|e| match e {
            lattice_grammar::Effect::OpenPrompt { prompt, .. } => Some(prompt.clone()),
            _ => None,
        });
        assert_eq!(
            prompt.as_deref(),
            Some(want),
            "`{chord}` must open its own prompt"
        );
        // Open it for real, then back out with an empty answer — otherwise the
        // next chord is pressed into a minibuffer this one left open.
        apply_effects(&mut editor, out);
        submit_prompt(&mut editor, "");
        editor.run_tick_pending();
    }
}

/// The four text filters narrow a REAL agenda — the half a unit test cannot
/// reach, because it is the SCAN that has to consult them.
///
/// Asserted on what each filter claims rather than on a row count: a row is
/// fanned across every section that admits it (AS.1), so three headlines are
/// four entries and an exact number would be pinning the section set rather
/// than the filter. What matters is that a filter admits strictly less than no
/// filter, that a term matching nothing yields nothing — the sharpest single
/// check that it is applied at all — and that `S` puts everything back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_text_filters_narrow_a_real_agenda() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, view, mb) = agenda_with_rows(base.path()).await;
    let baseline = mb.handle(view).unwrap().excerpts().len();
    assert!(baseline >= 3, "the corpus produced rows: {baseline}");

    fn current(editor: &Editor) -> lattice_core::BufferId {
        editor
            .services
            .get::<lattice_mode::BufferStoreHandle>()
            .unwrap()
            .find_by_name("*agenda*")
            .expect("the agenda is open under its own name")
    }

    for (chord, answer, expect_some, why) in [
        ("sT", "ship", true, "title matches one headline"),
        (
            "sT",
            "nothing-matches-this",
            false,
            "a title nothing carries",
        ),
        ("sb", "invoice", true, "body text no row displays"),
        (
            "sb",
            "nothing-matches-this",
            false,
            "a body nothing carries",
        ),
        ("sc", "work", true, "the work.org category"),
        ("sc", "nosuchcategory", false, "a category nothing carries"),
        ("sr", "\\[#A\\]", true, "the one priority-A row"),
        ("sr", "^zzz", false, "a pattern nothing matches"),
    ] {
        let open_in = current(&editor);
        let _ = editor.activate_buffer(open_in);
        editor.cursor.line = 0;
        let out = press_raw(&mut editor, chord);
        apply_effects(&mut editor, out);
        submit_prompt(&mut editor, answer);
        let filtered = current(&editor);
        settle_agenda(&mb, filtered).await;
        let n = mb.handle(filtered).unwrap().excerpts().len();
        if expect_some {
            assert!(
                n > 0 && n < baseline,
                "`{chord} {answer}` — {why}: got {n} of {baseline}"
            );
        } else {
            assert_eq!(n, 0, "`{chord} {answer}` — {why}: got {n}");
        }
        // The headerline has to SAY it, or a narrowed agenda reads as an empty
        // one — the trap OA.22 exists for, and four new kinds are four ways in.
        let said = match &*mb.handle(filtered).unwrap().headerline() {
            HeaderlineStatus::Complete { summary, .. } => summary.clone(),
            other => panic!("{other:?}"),
        };
        assert!(
            said.contains(answer),
            "the header must name the filter: {said:?}"
        );

        // Clear, so each case is measured from the same baseline.
        let _ = editor.activate_buffer(filtered);
        editor.cursor.line = 0;
        let out = press_raw(&mut editor, "S");
        apply_effects(&mut editor, out);
        let cleared = current(&editor);
        settle_agenda(&mb, cleared).await;
        assert_eq!(
            mb.handle(cleared).unwrap().excerpts().len(),
            baseline,
            "`S` restores every row after `{chord} {answer}`"
        );
    }
}

/// A corpus with two files, three dated rows, a distinguishing body and one
/// `[#A]` — so every filter under test can tell some rows from others.
async fn agenda_with_rows(
    base: &std::path::Path,
) -> (Editor, lattice_core::BufferId, MultibufferRegistryHandle) {
    let plugins_dir = base.join("plugins");
    write_org_plugin_dir(&plugins_dir, &org_plugin_wasm().unwrap());
    let notes = base.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("work.org"),
        format!(
            "* TODO [#A] Ship it :urgent:\n  SCHEDULED: {}\n  the invoice is due\n\
             * TODO Review the deck\n  SCHEDULED: {}\n",
            today_stamp(),
            today_stamp()
        ),
    )
    .unwrap();
    std::fs::write(
        notes.join("home.org"),
        format!("* TODO Water the plants\n  SCHEDULED: {}\n", today_stamp()),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base)
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    expand_plugin_keymaps(&editor);
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();
    let view = match lattice_multibuffer::providers::scan_view::open_scan_view(
        &mut editor,
        &org_agenda_identity(),
        &lattice_grammar::Args::String(notes.display().to_string()),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        lattice_mode::ProviderViewOutcome::Declined { message } => panic!("{message}"),
    };
    settle_agenda(&mb, view).await;
    (editor, view, mb)
}

/// `f` walks by the view's OWN span, and keeps walking.
///
/// The sharpest form of the OA.28 defect, and the one no single-keypress test
/// could see: with the view's arguments unreadable, every press computed
/// `today + 1 day` from a default view — so `f` moved one day instead of one
/// month, and then never moved again, because the second press recomputed the
/// same answer from the same default. Three presses, because one press is
/// indistinguishable from a working build.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f_walks_by_the_views_own_span_and_keeps_walking() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    /// `YYYY-MM-DD` for today + `days`, in LOCAL time like the guest.
    fn day(days: i64) -> String {
        (chrono::Local::now().date_naive() + chrono::Duration::days(days))
            .format("%Y-%m-%d")
            .to_string()
    }

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &org_plugin_wasm().unwrap());
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("only.org"),
        format!("* TODO Ship it\n  SCHEDULED: {}\n", today_stamp()),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    expand_plugin_keymaps(&editor);
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();
    let view = match lattice_multibuffer::providers::scan_view::open_scan_view(
        &mut editor,
        &org_agenda_identity(),
        &lattice_grammar::Args::String(notes.display().to_string()),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        lattice_mode::ProviderViewOutcome::Declined { message } => panic!("{message}"),
    };
    settle_agenda(&mb, view).await;

    fn current(editor: &Editor) -> lattice_core::BufferId {
        editor
            .services
            .get::<lattice_mode::BufferStoreHandle>()
            .unwrap()
            .find_by_name("*agenda*")
            .expect("the agenda is open under its own name")
    }
    fn header(mb: &MultibufferRegistryHandle, view: lattice_core::BufferId) -> String {
        match &*mb
            .handle(view)
            .expect("the view is registered")
            .headerline()
        {
            HeaderlineStatus::Complete { summary, .. } => summary.clone(),
            other => panic!("the agenda did not complete: {other:?}"),
        }
    }
    fn invoke(editor: &mut Editor, name: &str) {
        let id = editor
            .registry
            .load()
            .id_by_name(name)
            .unwrap_or_else(|| panic!("`{name}` is registered"));
        let out = editor.dispatch(lattice_host::action::Action::Invoke(
            lattice_grammar::CommandInvocation::of(id),
        ));
        apply_effects(editor, out);
    }

    invoke(&mut editor, "org-agenda-month-view");
    let v = current(&editor);
    settle_agenda(&mb, v).await;
    assert!(header(&mb, v).contains(&day(0)), "a month starting today");

    // Three steps of thirty days. The SECOND is what fails on a build that
    // reads a default view: it recomputes the first answer and stands still.
    for step in 1..=3 {
        invoke(&mut editor, "org-agenda-later");
        let v = current(&editor);
        settle_agenda(&mb, v).await;
        let head = header(&mb, v);
        let want = day(30 * step);
        assert!(
            head.contains("Month "),
            "press {step} kept the span: {head:?}"
        );
        assert!(
            head.contains(&want),
            "press {step} must land on {want} — one month per press, from where \
             the view already was. Got {head:?}"
        );
    }

    // `.` comes home without giving up the span.
    invoke(&mut editor, "org-agenda-today");
    let v = current(&editor);
    settle_agenda(&mb, v).await;
    let home = header(&mb, v);
    assert!(
        home.contains("Month ") && home.contains(&day(0)),
        "`.` resets the day, not the span: {home:?}"
    );
}

/// Run an action ONCE by name and apply what it produced.
///
/// `press` cannot be used for a chord that mutates: its own doc says it
/// re-dispatches an `Invoke` to recover the effects `dispatch_chord` discards,
/// and that it is "safe here because both chords are pure prompt-openers".
/// `org-todo-cycle` is not one — pressing it through `press` cycles the keyword
/// TWICE, which is how this test first read and why it saw a transition it did
/// not make.
/// Returns the document's text as it stood the instant the action returned —
/// BEFORE any prompt was opened. `open_prompt_line` makes the prompt buffer the
/// active document, so `text(editor)` after it reads the empty prompt, not the
/// file. That is what "the state lands first" has to be measured against.
fn invoke_once(editor: &mut Editor, name: &str) -> String {
    let id = editor
        .registry
        .load()
        .id_by_name(name)
        .unwrap_or_else(|| panic!("`{name}` is registered"));
    let out = editor.dispatch(lattice_host::action::Action::Invoke(
        lattice_grammar::CommandInvocation::of(id),
    ));
    // Edits FIRST, then read, then the prompt — in that order and for two
    // separate reasons. The edits have to land before the read or there is
    // nothing to see; the prompt has to open after it, because
    // `open_prompt_line` makes the prompt buffer the active document and
    // `text(editor)` would then read the empty prompt rather than the file.
    let mut prompt = None;
    for effect in out.effects {
        match effect {
            lattice_grammar::Effect::ApplyEdit {
                target,
                edit,
                cursor,
            } => {
                let _ = editor.dispatch(lattice_host::action::Action::ApplyEdit {
                    target,
                    edit,
                    cursor,
                });
            }
            lattice_grammar::Effect::OpenPrompt {
                prompt: p,
                initial,
                on_submit_action,
                buffer_name,
            } => prompt = Some((p, initial, on_submit_action, buffer_name)),
            _ => {}
        }
    }
    let landed = text(editor);
    if let Some((p, initial, action, name)) = prompt {
        editor.open_prompt_line(p, initial, action, name);
    }
    landed
}

// ── TK.9: the note a `(@)` state asks for ──────────────────────────────────

/// The state changes FIRST and the note follows — org's order, and the reason
/// it matters: dismissing the prompt leaves the state changed and unnoted
/// rather than making the keyword appear not to respond.
///
/// Lives here rather than beside TK.8's tests because `press` discards the
/// outcome, and `Effect::OpenPrompt` lives in what it discards — a test that
/// pressed and asserted would report "no prompt" whether or not the feature
/// worked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tk9_a_note_state_changes_first_then_records_the_note() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO Task\n").await;
    editor
        .config
        .parse_and_set_command("org.todo-keywords=TODO | CANCELLED(c@/!)")
        .expect("the option accepts the sequence");

    goto_line(&mut editor, 0);
    let landed = invoke_once(&mut editor, "org-todo-cycle");

    assert!(
        landed.starts_with("* CANCELLED Task"),
        "the state lands before the note is answered: {landed:?}"
    );
    assert!(
        editor.pending_prompt_submit_action.is_some(),
        "and a prompt is waiting for the note"
    );

    submit_prompt(&mut editor, "client went quiet");

    let got = text(&editor);
    assert!(
        got.contains("- State \"CANCELLED\" from \"TODO\" ["),
        "the transition is recorded: {got:?}"
    );
    assert!(
        got.contains("client went quiet"),
        "…carrying the note: {got:?}"
    );
    assert!(
        got.contains("\\\\"),
        "…with org's continuation marker, so emacs reads it back: {got:?}"
    );
}

/// Dismissing the prompt leaves the state changed and records nothing. That is
/// the half the chosen ordering trades away, so it is asserted rather than
/// assumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tk9_a_dismissed_note_leaves_the_state_changed_and_unlogged() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO Task\n").await;
    editor
        .config
        .parse_and_set_command("org.todo-keywords=TODO | CANCELLED(c@/!)")
        .expect("the option accepts the sequence");

    goto_line(&mut editor, 0);
    let got = invoke_once(&mut editor, "org-todo-cycle");
    // No submit — the user pressed Escape.

    assert!(got.starts_with("* CANCELLED Task"), "{got:?}");
    assert!(
        !got.contains("- State"),
        "an unanswered note records nothing at all: {got:?}"
    );
}

// ─────────────────────────────────────────────────────────────
//  OE.2 — `org-set-property`, two prompts and a drawer
// ─────────────────────────────────────────────────────────────

/// The whole chain: chord → name prompt → value prompt → drawer.
///
/// **The key survives the hop through `buffer-name` and only an end-to-end
/// test can say so.** `open-prompt-payload` has no argument slot, so hop two
/// smuggles the name the way capture smuggles its template key; a unit test of
/// the writer proves the edit and proves nothing about whether the name
/// arrives. If it did not, this would write `::` or nothing at all.
///
/// The corpus carries a `SCHEDULED:` line because OE.0's rule is the thing
/// most likely to regress here: a drawer written above it takes the plan out
/// of the tree and the agenda stops seeing the date.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_property_writes_a_drawer_below_the_planning_line() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(
        base.path(),
        &format!("* TODO Ship it\n  SCHEDULED: {}\nbody\n", today_stamp()),
    )
    .await;

    goto_line(&mut editor, 0);
    let out = press_raw(&mut editor, "<C-c><C-x>p");
    apply_effects(&mut editor, out);
    submit_prompt(&mut editor, "CATEGORY");
    submit_prompt(&mut editor, "work");

    assert_eq!(
        text(&editor),
        format!(
            "* TODO Ship it\n  SCHEDULED: {}\n:PROPERTIES:\n:CATEGORY: work\n:END:\nbody\n",
            today_stamp()
        ),
        "the drawer goes below the plan, and the value is the one typed"
    );
}

/// Setting the same key twice replaces it. The `:ID:` writer refuses this
/// case; a user who typed the command means the value they typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_a_property_twice_replaces_rather_than_duplicates() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO Ship it\n").await;

    for value in ["first", "second"] {
        goto_line(&mut editor, 0);
        let out = press_raw(&mut editor, "<C-c><C-x>p");
        apply_effects(&mut editor, out);
        submit_prompt(&mut editor, "CATEGORY");
        submit_prompt(&mut editor, value);
    }

    assert_eq!(
        text(&editor),
        "* TODO Ship it\n:PROPERTIES:\n:CATEGORY: second\n:END:\n",
        "one drawer, one CATEGORY, the second value"
    );
}

/// An empty name backs out. `<CR>` on an empty prompt is how a person
/// abandons this, and a second question would make the escape hatch look like
/// part of the flow.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_property_name_backs_out() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO Ship it\n").await;

    goto_line(&mut editor, 0);
    let out = press_raw(&mut editor, "<C-c><C-x>p");
    apply_effects(&mut editor, out);
    submit_prompt(&mut editor, "");

    assert!(
        editor.pending_prompt_submit_action.is_none(),
        "no second prompt after an empty name"
    );
    assert_eq!(
        text(&editor),
        "* TODO Ship it\n",
        "and nothing written — asserted on the TEXT, since 'no edit recorded' \
         passes on a build that wrote to the wrong line"
    );
}

/// From the agenda, the drawer lands in the SOURCE file.
///
/// The case with no cheaper coverage, and OA.25's reason: an agenda row is one
/// line, so the drawer goes outside every excerpt. It cannot be a composed
/// edit and cannot be addressed by path — only through the row's source
/// buffer, which is what `PlanTarget` resolves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_property_from_the_agenda_writes_the_source_file() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, view, mb) = agenda_over_two_files(base.path()).await;
    let handle = mb.handle(view).unwrap();

    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    let out = press_raw(&mut editor, "<C-c><C-x>p");
    apply_effects(&mut editor, out);
    submit_prompt(&mut editor, "CATEGORY");
    submit_prompt(&mut editor, "work");
    editor.run_tick_pending();

    let source = handle.excerpts()[0].source;
    let written = handle.source_text(source).expect("the source is attached");
    assert!(
        written.contains(":PROPERTIES:\n:CATEGORY: work\n:END:"),
        "the drawer landed in the row's own file: {written:?}"
    );
    assert!(
        written.contains("SCHEDULED:"),
        "…without displacing the planning line: {written:?}"
    );
}

// ─────────────────────────────────────────────────────────────
//  OE.3 / OE.4 — `C-c C-c` acts on the thing at the cursor
// ─────────────────────────────────────────────────────────────

/// On a checkbox, `C-c C-c` toggles it and updates the ancestor cookie —

/// On a headline, `C-c C-c` prompts for tags — emacs' behaviour, and the
/// same prompt `<C-c><C-q>` opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_ctrl_c_on_a_headline_prompts_for_tags() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* TODO Ship it\nbody\n").await;

    goto_line(&mut editor, 0);
    let out = press_raw(&mut editor, "<C-c><C-c>");
    apply_effects(&mut editor, out);
    assert_eq!(
        editor.pending_prompt_submit_action.as_deref(),
        Some("org-set-tags-submit"),
        "the headline arm opens the tags prompt"
    );

    submit_prompt(&mut editor, "work");
    assert_eq!(
        text(&editor),
        "* TODO Ship it :work:\nbody\n",
        "…and submitting it writes the tags"
    );
}

/// Scale a settle loop's poll budget for machine load.
///
/// Every wait in this suite is `for _ in 0..N { if done { break } sleep(ms) }`,
/// which budgets ITERATIONS. That is fine on an idle machine and wrong under a
/// full `cargo test`: the work being waited on — a wasm instantiation, a guest
/// scan, an off-thread index — slows down with contention while the budget does
/// not stretch to match, so the loop gives up on work that was still coming.
///
/// Three suites flaked exactly this way in one session (`org_roam_index` twice,
/// on two different tests, and `org_highlight_from_component` once), each
/// passing cleanly in isolation. A red that is sometimes noise is a red that
/// gets argued with instead of obeyed, which is the real cost.
///
/// **A wider budget is close to free.** These loops exit the moment their
/// condition holds, so raising the ceiling costs nothing on the passing path;
/// it is only paid when something is genuinely broken, and waiting longer to
/// report a real failure is the cheaper mistake.
///
/// `LATTICE_TEST_SETTLE_SCALE` overrides the factor for a slower machine.
fn settle_budget(base: usize) -> usize {
    let scale: usize = std::env::var("LATTICE_TEST_SETTLE_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(10);
    base.saturating_mul(scale)
}

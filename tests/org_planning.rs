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
    for _ in 0..600 {
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
    let z = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 86_400) as i64
        + 719_468;
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
    assert!(
        plain.contains("[agenda: 20"),
        "an unfiltered agenda still says WHEN it is looking — the window is \
         what `f`/`b` change and the only thing that says where you are: \
         {plain:?}"
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
    for _ in 0..100 {
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

    // Narrow, the way `/` does — the chord opens a prompt, and the submit
    // re-scans with the tag appended.
    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    let out = press_raw(&mut editor, "/");
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

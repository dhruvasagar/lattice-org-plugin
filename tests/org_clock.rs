//! OC.6 — clocking, through the chords, in a real editor.
//!
//! `src/clock.rs`'s unit tests cover the drawer arithmetic on the host target.
//! These are the better test, for the reason the sibling files give: they run
//! the whole route — `<leader>oi` → org-mode's own keymap layer →
//! `Action::Invoke` → the sync grammar trampoline → the guest's `apply-action`
//! → `Effect::ApplyEdit` → the host's edit path — and nothing in `lattice-host`
//! knows what a `:LOGBOOK:` drawer is.
//!
//! They also exercise the thing that unit tests structurally cannot: the clock
//! spans **two `wasmtime::Store`s of the same component**. The chord runs in the
//! grammar store; the session, the minute wake and the modeline segment live in
//! the events store, with a separate linear memory. The only bridge is the event
//! bus (host slice OC.1). A test that stopped at the buffer text would pass with
//! that bridge entirely unbuilt.
//!
//! Skips when the component was not built — `cargo test` builds for the HOST,
//! and the component is a separate `--target wasm32-wasip2 --release` artefact.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_mode::ModeId;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::{parse_chord_sequence, KeyChord};

fn org_plugin_wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    );
    std::fs::read(path).ok()
}

/// The real timer, as `lattice_plugin_loader::install` wires it.
struct TokioSleeper;
impl lattice_plugin_host::Sleeper for TokioSleeper {
    fn sleep(&self, dur: Duration) -> futures::future::BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(dur))
    }
}

/// Boot an editor with org loaded and a `.org` file open.
///
/// Wires two things the sibling harness does not, because the clock is the
/// first feature to need either: the **modeline** (so `ui.register-segment`
/// finds a registry rather than answering `false`) and a **sleeper** (so
/// `wake-every` arms rather than answering `0`). Both are wired by `install` in
/// production; a harness that omitted them would leave the clock editing the
/// buffer correctly and silently never showing a segment — which is exactly the
/// half-working state these tests exist to catch.
async fn org_editor(base: &std::path::Path, text: &str) -> Editor {
    lattice_plugin_loader::disable_autoload();
    let mut editor = Editor::boot(CoreDocument::from_text("scratch\n"));

    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\n\
         provides = [\"modes\", \"grammar\", \"language\", \"help\", \"config\", \"events\"]\n\
         default_modes = [\"org-todo-mode\", \"org-global-mode\"]\n\
         editor_capabilities = [\"tree-sitter\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("component.wasm"),
        org_plugin_wasm().expect("caller checked"),
    )
    .unwrap();

    let host = Arc::new(PluginHost::with_dirs(base.join("cache"), base.join("data")).unwrap());
    host.set_modeline(editor.modeline.clone(), editor.event_bus.clone());
    host.set_sleeper(Arc::new(TokioSleeper));
    let loader = PluginLoader::with_services(
        Arc::clone(&host),
        LoaderServices {
            runtime: Some(tokio::runtime::Handle::current()),
            bus: Some(editor.event_bus.clone()),
            command_registry: Some(editor.registry.clone()),
            mode_registry: Some(editor.mode_registry.clone()),
            keymap: Some(editor.keymap.clone()),
            help_topics: Some(editor.help_topics.clone()),
            config_registry: Some(editor.config.clone()),
            modeline: Some(editor.modeline.clone()),
            ..Default::default()
        },
    );
    assert_eq!(
        loader
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1,
        "the org component loads — a component importing `events` and `ui` must \
         still instantiate on the SYNC grammar linker, or org loses its whole keymap"
    );
    // Leaked deliberately: the loader owns the event actor's task, and dropping
    // it here would end the actor before a single wake could fire.
    std::mem::forget(loader);

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

fn press(editor: &mut Editor, keys: &str) {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial: Vec<KeyChord> = Vec::new();
    for c in seq {
        let _ = editor.dispatch_chord(c, &mut partial);
    }
}

fn goto_line(editor: &mut Editor, line: u32) {
    editor.cursor.line = line;
    editor.cursor.byte = 0;
}

fn text(editor: &Editor) -> String {
    editor.document.snapshot().text().to_string()
}

/// Poll until `pred` holds or `limit` elapses, draining the editor's pending
/// work each turn — the async side lands on the plugin's own task, so a test
/// that read once would race it.
async fn until(
    editor: &mut Editor,
    limit: Duration,
    mut pred: impl FnMut(&mut Editor) -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + limit;
    loop {
        editor.run_tick_pending();
        if pred(editor) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The rendered text of org's modeline segment, or `None` when it is hidden.
fn segment(editor: &Editor) -> Option<String> {
    let snap = editor.modeline.snapshot();
    let id = lattice_mode::modeline::ElementId::new("org.clock");
    snap.content
        .get(&(lattice_mode::modeline::ModelineKey::Global, id))
        .filter(|c| !c.is_empty())
        .map(|c| c.spans.iter().map(|s| s.text.as_str()).collect())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clocking_in_writes_a_logbook_drawer() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nbody\n").await;

    // From the body line, not the headline — you clock in while reading an
    // entry, and the enclosing headline is what gets the drawer.
    goto_line(&mut editor, 1);
    press(&mut editor, "<leader>oi");

    let got = text(&editor);
    let lines: Vec<&str> = got.lines().collect();
    assert_eq!(lines[1], ":LOGBOOK:", "the drawer was created for us");
    assert!(
        lines[2].starts_with("CLOCK: [") && lines[2].ends_with(']'),
        "a running clock is a start stamp with no end: {:?}",
        lines[2]
    );
    assert_eq!(lines[3], ":END:");
    assert_eq!(lines[4], "body", "the body is untouched, below the drawer");
}

/// The one assertion that proves the two stores are actually bridged. The
/// buffer edit happens in the grammar store; this segment is pushed from the
/// events store, which learns a clock started ONLY through the event bus.
///
/// Asserted **without pressing another key**, because a wake and a bus delivery
/// both land off the keystroke path — a test that pressed something first would
/// pass on a version where nothing async works at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_modeline_segment_appears_without_another_keystroke() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Write the clocking slice\nbody\n").await;

    // The descriptor is registered at load, from `register-events`.
    assert!(
        editor
            .modeline
            .snapshot()
            .registry
            .get(&lattice_mode::modeline::ElementId::new("org.clock"))
            .is_some(),
        "org owns its modeline element, registered on load (modeline.md §6)"
    );

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oi");

    let shown = until(&mut editor, Duration::from_secs(5), |e| segment(e).is_some()).await;
    assert!(
        shown,
        "the segment must appear on its own — the grammar store emitted an \
         event, the events store received it and pushed content on the bus"
    );
    let text = segment(&editor).unwrap();
    assert!(
        text.contains("0:00"),
        "it shows elapsed time from the moment of clock-in, not on the first \
         wake a minute later: {text:?}"
    );
    assert!(
        text.contains("Write the clocking slice"),
        "and which entry it is on: {text:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clocking_out_closes_the_line_and_clears_the_segment() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nbody\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oi");
    assert!(until(&mut editor, Duration::from_secs(5), |e| segment(e).is_some()).await);

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oO");

    let got = text(&editor);
    let clock_line = got.lines().nth(2).unwrap();
    assert!(
        clock_line.contains("]--[") && clock_line.contains("=>"),
        "a closed clock carries both stamps and a duration: {clock_line:?}"
    );
    assert!(
        until(&mut editor, Duration::from_secs(5), |e| segment(e).is_none()).await,
        "and the segment goes away, again with no keystroke to prompt it"
    );
}

/// Cancel means "pretend this never happened", so it takes the drawer it
/// created with it rather than leaving an empty `:LOGBOOK:` / `:END:` pair.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_removes_the_drawer_it_created() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nbody\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oi");
    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oq");

    assert_eq!(
        text(&editor),
        "* Task\nbody\n",
        "the file is exactly as it started"
    );
}

/// D4 in one test: with no session at all — the after-a-restart case — the
/// buffer alone still says a clock is running, and clocking out works.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clocking_out_works_on_a_clock_this_session_never_started() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    // The clock line was written by a previous run of the editor. Nothing in
    // this process has ever heard of it.
    let mut editor = org_editor(
        base.path(),
        "* Task\n:LOGBOOK:\nCLOCK: [2026-08-28 Fri 09:15]\n:END:\n",
    )
    .await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oO");

    let got = text(&editor);
    let clock_line = got.lines().nth(2).unwrap();
    assert!(
        clock_line.starts_with("CLOCK: [2026-08-28 Fri 09:15]--["),
        "re-derived from the buffer, which is the durable record: {clock_line:?}"
    );
}

/// Above the first headline there is no entry to clock into. Refusing is the
/// answer; inventing a location writes a clock line that belongs to nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn there_is_nothing_to_clock_into_in_the_preamble() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "#+TITLE: Notes\n\n* Task\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oi");

    assert_eq!(
        text(&editor),
        "#+TITLE: Notes\n\n* Task\n",
        "the buffer is untouched"
    );
}

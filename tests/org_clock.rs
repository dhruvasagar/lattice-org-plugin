//! OC.6 — clocking, through the chords, in a real editor.
//!
//! `src/clock.rs`'s unit tests cover the drawer arithmetic on the host target.
//! These are the better test, for the reason the sibling files give: they run
//! the whole route — `<leader>oxi` → org-mode's own keymap layer →
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
/// [`org_editor`] with an `fs:write` grant over `base`.
///
/// OC.9's resume writes into a file that is not open, and a cross-file write is
/// refused at the boundary without the grant — silently to the guest, which
/// would look exactly like a broken resume.
async fn org_editor_with_write(base: &std::path::Path, text: &str) -> Editor {
    org_editor_inner(base, text, &[format!("fs:write:{}", base.display())]).await
}

async fn org_editor(base: &std::path::Path, text: &str) -> Editor {
    org_editor_inner(base, text, &[]).await
}

async fn org_editor_inner(base: &std::path::Path, text: &str, caps: &[String]) -> Editor {
    lattice_plugin_loader::disable_autoload();
    let mut editor = Editor::boot(CoreDocument::from_text("scratch\n"));

    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "id = \"org\"\n\
             provides = [\"modes\", \"grammar\", \"language\", \"help\", \"config\", \"events\"]\n\
             default_modes = [\"org-todo-mode\", \"org-global-mode\", \"org-table-mode\"]\n\
             editor_capabilities = [\"tree-sitter\"]\n\
             capabilities = [{}]\n",
            caps.iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
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

/// Submit a `:` line the way the renderer does.
///
/// `execute_ex_line` dispatches and leaves its effects in the `DispatchOutcome`
/// for the caller to drain — `dispatch_chord` (behind `press`) drains its own,
/// which is why a chord "just works" in a test and a `:` line does not. Missing
/// this reads exactly like a broken command: the guest ran, produced the right
/// edit, and nothing applied it.
fn ex(editor: &mut Editor, line: &str) {
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.execute_ex_line(line, &mut out);
    // Only `next_actions` are drained. `out.effects` is a RECORD of what the
    // dispatch did, not a to-do list: `Effect::WriteToFile` is applied INLINE by
    // the host (deliberately — it returns a Result, which is what makes "cut
    // only if the insert landed" expressible), so re-applying it here writes the
    // entry twice. Learned by doing exactly that.
    for action in out.next_actions {
        let _ = editor.dispatch(action);
    }
    editor.run_tick_pending();
}

fn goto_line(editor: &mut Editor, line: u32) {
    editor.cursor.line = line;
    editor.cursor.byte = 0;
}

/// The text of another buffer by path.
///
/// `WriteToFile` OPENS its target and edits the buffer rather than writing the
/// file — so a cross-file write is visible here and not on disk until a save.
/// Reading the disk instead is how this test first "failed" against a resume
/// that had worked perfectly.
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
    press(&mut editor, "<leader>oxi");

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
    press(&mut editor, "<leader>oxi");

    let shown = until(&mut editor, Duration::from_secs(5), |e| {
        segment(e).is_some()
    })
    .await;
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
    press(&mut editor, "<leader>oxi");
    assert!(
        until(&mut editor, Duration::from_secs(5), |e| segment(e)
            .is_some())
        .await
    );

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oxo");

    let got = text(&editor);
    let clock_line = got.lines().nth(2).unwrap();
    assert!(
        clock_line.contains("]--[") && clock_line.contains("=>"),
        "a closed clock carries both stamps and a duration: {clock_line:?}"
    );
    assert!(
        until(&mut editor, Duration::from_secs(5), |e| segment(e)
            .is_none())
        .await,
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
    press(&mut editor, "<leader>oxi");
    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oxq");

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
    press(&mut editor, "<leader>oxo");

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
    press(&mut editor, "<leader>oxi");

    assert_eq!(
        text(&editor),
        "#+TITLE: Notes\n\n* Task\n",
        "the buffer is untouched"
    );
}

/// OC.7 — the four are `:` commands as well as chords, from ONE registration.
///
/// This is design §5.2.1's unification, and it is the assertion that would have
/// failed silently: OC.6 registered them with `register_action`, and
/// `excommand.rs` answers `Unknown` for an action kind (there is no `action:`
/// prefix either), so `:org-clock-in` reported "unknown command" while
/// `<leader>oxi` worked perfectly. Nothing in a chord-driven test can see that.
///
/// It is reachable at all only because of OC.10: before it an ex-command was
/// handed no cursor and no buffer id, so it could locate no entry and name no
/// edit target.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_clock_is_reachable_from_the_ex_line() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nbody\n").await;

    goto_line(&mut editor, 1);
    ex(&mut editor, "org-clock-in");

    let got = text(&editor);
    let lines: Vec<&str> = got.lines().collect();
    assert_eq!(
        lines[1], ":LOGBOOK:",
        "`:org-clock-in` must do what `<leader>oxi` does; got {got:?}"
    );
    assert!(lines[2].starts_with("CLOCK: ["));

    // …and out again, so the pair is proven rather than just the entry point.
    goto_line(&mut editor, 0);
    ex(&mut editor, "org-clock-out");
    assert!(
        text(&editor).lines().nth(2).unwrap().contains("=>"),
        "`:org-clock-out` closed the line"
    );
}

/// The chord and the `:` line reach the SAME registration, so the chord did not
/// quietly stop working when the four became ex-commands.
///
/// A mode keymap binding resolves a command by NAME and does not filter on its
/// kind — which is what makes one registration serve both surfaces — but that is
/// a property of the host worth pinning from here, since org is what depends on
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_chord_still_works_now_that_it_binds_an_ex_command() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nbody\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oxi");

    assert_eq!(
        text(&editor).lines().nth(1),
        Some(":LOGBOOK:"),
        "the chord binds the ex-command by name and dispatches it"
    );
}

/// These take no arguments, and saying so beats ignoring what was typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_clock_commands_refuse_arguments() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nbody\n").await;

    goto_line(&mut editor, 0);
    ex(&mut editor, "org-clock-in nonsense");

    assert_eq!(
        text(&editor),
        "* Task\nbody\n",
        "a refused parse must not edit the buffer"
    );
}

/// OC.9 — resume the last clocked entry, in a file that is not even open.
///
/// This is the case that decided the design. `apply-edit` names a buffer id, and
/// an unopened file has none — so resume reaches the entry the way capture
/// reaches its target: `read-file` for characters, `parse-file` for structure,
/// one `WriteToFile`. `clock::Logbook` needs only a line accessor, so OC.5's
/// drawer primitive works over file text with no change at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_clocks_into_a_file_that_is_not_open() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    // The entry lives in `other.org`; the editor will be looking at `notes.org`.
    let other = base.path().join("other.org");
    std::fs::write(&other, "* Elsewhere\nbody\n").unwrap();

    let mut editor = org_editor_with_write(base.path(), "* Here\n").await;

    // Clock in on the OTHER file, then out — which leaves it as "last clocked".
    editor.do_edit(Some(other.clone()), false);
    editor.run_tick_pending();
    goto_line(&mut editor, 0);
    ex(&mut editor, "org-clock-in");
    ex(&mut editor, "org-clock-out");

    // Save, so the file on disk matches the buffer. Resume reads the FILE to
    // find the entry — see its doc — so a target with unsaved edits would have
    // it computing an insertion line against text the buffer no longer has.
    editor.do_write(None);
    editor.run_tick_pending();

    // Now go somewhere else entirely and resume.
    let notes = base.path().join("notes.org");
    editor.do_edit(Some(notes), false);
    editor.run_tick_pending();
    ex(&mut editor, "org-clock-resume");

    let written = text_of(&editor, &other);
    let running = written
        .lines()
        .filter(|l| l.trim_start().starts_with("CLOCK: ") && !l.contains("--"))
        .count();
    assert_eq!(
        running, 1,
        "resume wrote a running clock into a file nobody had open: {written:?}"
    );
}

/// With nothing clocked this session there is nowhere to resume, and saying so
/// beats guessing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_with_nothing_clocked_says_so() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nbody\n").await;
    ex(&mut editor, "org-clock-resume");
    assert_eq!(
        text(&editor),
        "* Task\nbody\n",
        "nothing to resume must edit nothing"
    );
}

/// Clock-out no longer forgets where it was, so `:org-clock-goto` still works
/// afterwards — which is org's own behaviour (it jumps to the current OR last
/// clocked entry) and is what makes resume possible at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_last_clocked_entry_outlives_clocking_out() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nbody\n").await;
    goto_line(&mut editor, 0);
    ex(&mut editor, "org-clock-in");
    ex(&mut editor, "org-clock-out");

    // Resume in the SAME buffer takes the ordinary in-buffer path, so a second
    // running clock appears without touching the disk behind the buffer's back.
    ex(&mut editor, "org-clock-resume");
    let running = text(&editor)
        .lines()
        .filter(|l| l.trim_start().starts_with("CLOCK: ") && !l.contains("--"))
        .count();
    assert_eq!(
        running,
        1,
        "resume re-clocked the entry clock-out had finished: {:?}",
        text(&editor)
    );
}

/// OA.27 — the clock's two spellings reach the same commands.
///
/// `<leader>ox…` is the vim-native one; `<C-c><C-x><C-…>` is emacs' own, letter
/// for letter. Two spellings of one `ActionId`, so there is no second handler to
/// keep in step — but a THREE-chord emacs sequence is exactly the shape that
/// fails silently: a binding that does not parse, or that never expands into a
/// keymap layer, simply reaches nothing and the key looks unbound.
///
/// Resolution rather than dispatch, deliberately. `org_clock.rs`'s other tests
/// already prove the handlers do the right thing through `<leader>ox…`; what is
/// unproven is that the second spelling arrives at the same place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_emacs_prefix_reaches_the_same_clock_commands() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let editor = org_editor(base.path(), "* Task\n").await;

    let resolve = |keys: &str| -> Option<String> {
        let expanded = editor.keymap.expand_leader(keys);
        let seq = parse_chord_sequence(&expanded).expect("parses");
        let modes: Vec<lattice_mode::ModeId> = editor
            .mode_registry
            .load()
            .iter_meta()
            .map(|(id, _)| id)
            .collect();
        editor
            .keymap
            .resolve_trace(lattice_keymap::BindingMode::Normal, &seq, &modes)
            .hits
            .last()
            .map(|h| format!("{:?}", h.command))
    };

    for (emacs, vim) in [
        ("<C-c><C-x><C-i>", "<leader>oxi"),
        ("<C-c><C-x><C-o>", "<leader>oxo"),
        ("<C-c><C-x><C-q>", "<leader>oxq"),
        ("<C-c><C-x><C-j>", "<leader>oxj"),
        ("<C-c><C-x><C-x>", "<leader>oxr"),
    ] {
        let a = resolve(emacs);
        let b = resolve(vim);
        assert!(
            a.is_some(),
            "`{emacs}` must resolve to a command — a declared binding that never \
             expands into a keymap layer reaches nothing"
        );
        assert_eq!(a, b, "`{emacs}` and `{vim}` must reach the same command");
    }
}

/// The keys the reorganisation FREED are free.
///
/// A move is only done if the old spelling stops working: leaving `<leader>oi`
/// bound would mean two ways to clock in, one of them undocumented, and the
/// whole point of the slice was to release `i` for inserting things.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_old_flat_clock_chords_are_gone() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut editor = org_editor(base.path(), "* Task\nbody\n").await;

    goto_line(&mut editor, 0);
    press(&mut editor, "<leader>oi");
    editor.run_tick_pending();
    assert_eq!(
        text(&editor),
        "* Task\nbody\n",
        "`<leader>oi` no longer clocks in — it is free for an insert group"
    );
}

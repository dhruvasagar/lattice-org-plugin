//! OB.4+ — `org-babel-mode`: running source blocks.
//!
//! Boots a real editor, loads the built org component through the loader, and
//! drives babel through its chords and commands, as a user would. Skips (with
//! the line CI greps for) when the component has not been built.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_mode::ModeId;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::{parse_chord_sequence, KeyChord};

fn component_cache() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("component-cache")
}

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
        "/../target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    );
    // The component embeds org's real manifest, and an embedded manifest wins
    // over a `plugin.toml`. These tests load org under manifests of their own
    // (a subset of seams, a grant on a temp dir), so take it out first.
    std::fs::read(path)
        .ok()
        .map(|bytes| lattice_plugin_host::manifest::without_embedded_manifest(&bytes))
}

fn loader_over_editor(editor: &Editor, base: &std::path::Path) -> PluginLoader {
    let host =
        Arc::new(PluginHost::with_dirs(component_cache(), base.join("data")).expect("host builds"));
    // OB.6: what `install` wires and a hand-built loader does not. Each one
    // unwired fails a run silently in its own way: no store, and every
    // buffer reads as closed; no active-modes query, and `buffer-text`
    // refuses; no sink, and a finished job's edit goes nowhere.
    if let Some(buffers) = editor.services.get::<lattice_mode::BufferStoreHandle>() {
        host.set_buffer_store((*buffers).clone());
    }
    if let Some(query) = editor
        .services
        .get::<lattice_mode::ActiveModesQueryHandle>()
    {
        host.set_active_modes_query((*query).clone());
    }
    // The sink is an inbound bus, as in `install`: its send wakes the editor
    // (`async_landed`), and its drain hands the posted batch to the next tick.
    let (posted, drain) = lattice_mode::inbound::make_inbound::<lattice_grammar::Effect, _>(
        editor.async_landed.clone(),
        |effect| vec![effect],
    );
    let ticks = editor
        .services
        .get::<lattice_mode::TickCallbackRegistryHandle>()
        .expect("the tick registry is a boot service");
    // Kept for the test's life: dropping the registration unregisters it.
    std::mem::forget(ticks.register(drain));
    host.set_effect_sink(Arc::new(move |effect| {
        let _ = posted.send(effect);
    }));
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
            // OB.6: the running sign — the editor's own sign and decoration
            // registries, so a test asks the producer the renderer asks.
            sign_registry: editor
                .services
                .get::<lattice_mode::SignRegistryHandle>()
                .map(|h| (*h).clone()),
            decoration_registry: editor
                .services
                .get::<lattice_mode::GutterDecorationSourceRegistryHandle>()
                .map(|h| (*h).clone()),
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
///
/// The loader comes back too, and a test must hold it: its option
/// subscriptions (`org.babel.enabled`'s gate among them) hold it weakly, as
/// the editor's own does, and stop when it drops.
async fn org_editor(base: &std::path::Path, text: &str) -> (Editor, Arc<PluginLoader>) {
    org_editor_with_caps(base, text, &[], &[]).await
}

/// [`org_editor`] with babel languages in the user's configuration, as
/// `(name, command)`: held BEFORE the plugin loads, because the process grant
/// is resolved at load from what the user wrote (OB.2).
async fn org_editor_with_languages(
    base: &std::path::Path,
    text: &str,
    languages: &[(&str, &str)],
) -> (Editor, Arc<PluginLoader>) {
    org_editor_with_caps(base, text, &[], languages).await
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
    languages: &[(&str, &str)],
) -> (Editor, Arc<PluginLoader>) {
    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    let caps = capabilities
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let sep = if caps.is_empty() { "" } else { ", " };
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            // OT.4: `editor_capabilities` mirrors the shipped manifest. Without
            // the `tree-sitter` grant the trampoline hands every seam a `none`
            // tree and the guest falls back to line matching — so a harness
            // that omitted it would test the text path while claiming to test
            // the tree one.
            "id = \"org\"\nprovides = [\"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\", \"picker-source\", \"transient-source\", \"events\", \"signs\", \"decorations\"]\ndefault_modes = [\"org-todo-mode\", \"org-global-mode\", \"org-table-mode\", \"org-babel-mode\"]\nmode_gates = {{ \"org-babel-mode\" = \"org.babel.enabled\" }}\neditor_capabilities = [\"tree-sitter\"]\ncapabilities = [\"proc:spawn:{{org.babel.languages:command}}\", \"state:write\"{sep}{caps}]\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("component.wasm"),
        org_plugin_wasm().expect("caller checked"),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    if !languages.is_empty() {
        use lattice_config::ConfigValue;
        let table = languages
            .iter()
            .map(|(name, command)| {
                ConfigValue::Record(
                    [
                        ("name".to_string(), ConfigValue::Str(name.to_string())),
                        ("command".to_string(), ConfigValue::Str(command.to_string())),
                    ]
                    .into(),
                )
            })
            .collect();
        editor.config.hold_tree(
            "org.babel.languages",
            ConfigValue::List(table),
            lattice_config::HeldSource::ConfigFile,
        );
    }
    let loader = Arc::new(loader_over_editor(&editor, base));
    // OB.3b: `org.babel.enabled` is followed by the loader's gate
    // subscription, which `install` starts at boot; start it here too.
    loader.subscribe_mode_gates();
    let loaded = loader
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
            lattice_host::keymap_normal::expand_grammar_rows(
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
    // The actor publishes after every dispatch; `buffer-text`'s gate reads
    // the published mode sets, so a test must publish too.
    editor.publish_render_state();
    assert_eq!(
        editor
            .active_modes
            .get(&editor.document_buffer_id)
            .and_then(|m| m.major()),
        Some(ModeId::new("org-mode")),
        "the buffer is in org-mode before any chord is dispatched"
    );
    // CD.4: captures are draft FILES under `{org.directory}/captures`. Set for
    // every test so a capture never falls back to the unsaveable path, whose
    // warning would otherwise be the last message a capture test reads.
    set_org_option(&mut editor, "directory", base.to_str().unwrap());
    (editor, loader)
}

fn press(editor: &mut Editor, keys: &str) {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial: Vec<KeyChord> = Vec::new();
    for c in seq {
        let _ = editor.dispatch_chord(c, &mut partial);
    }
    editor.publish_render_state();
}

fn goto_line(editor: &mut Editor, line: u32) {
    editor.cursor.line = line;
    editor.cursor.byte = 0;
}

fn last_echo(editor: &Editor) -> Option<String> {
    editor.last_message.as_ref().map(|m| m.text.clone())
}

fn text(editor: &Editor) -> String {
    editor.document.snapshot().text().to_string()
}

fn set_org_option(editor: &mut Editor, name: &str, value: &str) {
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!("org.{name}={value}"),
    });
}

fn babel_active(editor: &Editor) -> bool {
    editor
        .active_modes
        .get(&editor.document_buffer_id)
        .is_some_and(|m| m.is_active(ModeId::new("org-babel-mode")))
}

/// Wait until `done` holds, the way the actor does: on `async_landed`, then
/// a tick — never a key. Anything async that does not wake the editor fails
/// here, which is the bug class this exists to catch.
async fn settle(editor: &mut Editor, done: impl Fn(&Editor) -> bool) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    editor.run_tick_pending();
    while !done(editor) {
        let landed = editor.async_landed.clone();
        let woke = tokio::time::timeout_at(deadline, landed.notified()).await;
        assert!(
            woke.is_ok(),
            "did not settle; the buffer is:\n{}",
            text(editor)
        );
        editor.run_tick_pending();
        editor.publish_render_state();
    }
}

macro_rules! require_component {
    () => {
        if org_plugin_wasm().is_none() {
            eprintln!(
                "skipping: component not built (cargo build --release --target wasm32-wasip2)"
            );
            return;
        }
    };
}

/// A `sh` language, as a user would write it.
const SH: &str = "[[language]]\nname = \"sh\"\ncommand = \"sh\"\n";

const DOC: &str = "* Tasks\n\
                   - [ ] a box\n\
                   #+begin_src sh\n\
                   echo hi\n\
                   #+end_src\n\
                   #+begin_src python\n\
                   print(1)\n\
                   #+end_src\n\
                   #+begin_quote\n\
                   quoted\n\
                   #+end_quote\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn babel_mode_is_active_in_an_org_buffer_by_default() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (editor, _loader) = org_editor(base.path(), DOC).await;
    assert!(
        babel_active(&editor),
        "org-babel-mode is on in an org buffer"
    );
}

/// Outside a src block `C-c C-c` is org's: babel's layer declines and org's
/// answers underneath — a checkbox still toggles, and a quote block reaches
/// org's own "nothing to do here".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_ctrl_c_outside_a_src_block_falls_through() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor(base.path(), DOC).await;

    goto_line(&mut editor, 1);
    press(&mut editor, "<C-c><C-c>");
    assert!(
        text(&editor).contains("- [X] a box"),
        "the checkbox toggled: {}",
        text(&editor)
    );

    goto_line(&mut editor, 9);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: C-c C-c has nothing to do here")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_ctrl_c_in_a_src_block_is_babels() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor(base.path(), DOC).await;
    set_org_option(&mut editor, "babel.languages", SH);

    // A language nobody configured is named, with where to configure it.
    goto_line(&mut editor, 6);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: no babel language \"python\" — add it to org.babel.languages")
    );
    assert_eq!(text(&editor), DOC, "nothing was edited");

    // A configured one is babel's from every line of the block, markers too.
    for line in [2, 3, 4] {
        editor.last_message = None;
        goto_line(&mut editor, line);
        press(&mut editor, "<C-c><C-c>");
        let echo = last_echo(&editor).unwrap_or_default();
        assert!(
            !echo.contains("nothing to do here") && !echo.contains("no babel language"),
            "line {line}: {echo}"
        );
    }
}

/// `org.babel.enabled = false` removes babel's layer, while org stays: `C-c
/// C-c` in a src block then reaches org's own answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_babel_removes_its_layer() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor(base.path(), DOC).await;
    assert!(babel_active(&editor));

    set_org_option(&mut editor, "babel.enabled", "false");
    settle(&mut editor, |e| !babel_active(e)).await;

    goto_line(&mut editor, 3);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: C-c C-c has nothing to do here")
    );
    // The rest of org is untouched.
    assert!(editor
        .active_modes
        .get(&editor.document_buffer_id)
        .is_some_and(|m| m.is_active(ModeId::new("org-todo-mode"))));

    // And back on.
    set_org_option(&mut editor, "babel.enabled", "true");
    settle(&mut editor, babel_active).await;
}

// ── OB.6: `C-c C-c` runs a block ────────────────────────────────────────────

/// Replace `[start, end)` of the org buffer, as a user's edit would.
fn edit(editor: &mut Editor, start: (u32, u32), end: (u32, u32), with: &str) {
    use lattice_protocol::{Edit, EditKind, Position, Range};
    let target = editor.document_buffer_id;
    let _ = editor.dispatch(lattice_host::action::Action::ApplyEdit {
        target,
        edits: vec![Edit {
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
            kind: EditKind::Replace {
                text: with.to_string(),
            },
        }],
        cursor: None,
    });
    editor.publish_render_state();
}

/// One `sh` block, with `body` (lines joined by `\n`), under a headline.
fn sh_doc(body: &str) -> String {
    format!("* Run\n#+begin_src sh\n{body}\n#+end_src\nafter\n")
}

/// Run the block on `line` and wait for its result to land.
async fn run_and_settle(editor: &mut Editor, line: u32, done: impl Fn(&Editor) -> bool) {
    goto_line(editor, line);
    press(editor, "<C-c><C-c>");
    settle(editor, done).await;
}

fn has(needle: &'static str) -> impl Fn(&Editor) -> bool {
    move |e| text(e).contains(needle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_block_runs_and_its_output_appears_without_a_keypress() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("echo hi"), &[("sh", "sh")]).await;
    // `settle` waits on the editor's wake and ticks; it never presses a key,
    // so a result that needed one would time out here.
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    assert_eq!(
        text(&editor),
        "* Run\n#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nafter\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_results_are_replaced_not_appended() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh\necho new\n#+end_src\n\n#+RESULTS:\n: old\n: older\nafter\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 1, has(": new")).await;
    assert_eq!(
        text(&editor),
        "#+begin_src sh\necho new\n#+end_src\n\n#+RESULTS:\n: new\nafter\n"
    );
    // And again, over its own result: still one.
    let before = text(&editor);
    edit(&mut editor, (1, 0), (1, 8), "echo two");
    run_and_settle(&mut editor, 1, has(": two")).await;
    assert_eq!(text(&editor), before.replace("new", "two"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_that_looks_like_a_headline_cannot_become_one() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    // Few lines: `: ` lines, which cannot start a headline.
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("echo '* Not a headline'\necho '#+end_src'"),
        &[("sh", "sh")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    assert!(
        text(&editor).contains("#+RESULTS:\n: * Not a headline\n: #+end_src\n"),
        "{}",
        text(&editor)
    );

    // Many lines: an example block, its org-looking lines comma-escaped.
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("for i in 1 2 3 4 5 6 7 8 9 10; do echo \"* h$i\"; done\necho '#+end_example'"),
        &[("sh", "sh")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+end_example\nafter")).await;
    let t = text(&editor);
    assert!(t.contains("#+begin_example\n,* h1\n"), "{t}");
    assert!(t.contains(",* h10\n,#+end_example\n#+end_example\n"), "{t}");
    assert!(
        !t.lines().any(|l| l.starts_with("* h")),
        "no output line became a headline: {t}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ten_lines_become_an_example_block() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("for i in 1 2 3 4 5 6 7 8 9 10; do echo $i; done"),
        &[("sh", "sh")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+end_example")).await;
    assert!(
        text(&editor).contains("#+RESULTS:\n#+begin_example\n1\n2\n"),
        "{}",
        text(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_program_still_writes_its_output_and_echoes_the_status() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("echo partial\nexit 3"),
        &[("sh", "sh")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    assert!(
        text(&editor).contains("#+RESULTS:\n: partial\n"),
        "{}",
        text(&editor)
    );
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(
        echo.starts_with("org: the sh block failed:") && echo.contains('3'),
        "{echo}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_interpreter_reports_the_spawn_error() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("echo hi"),
        &[("sh", "lattice-no-such-interpreter")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    let t = text(&editor);
    let result = t.split("#+RESULTS:\n").nth(1).unwrap_or_default();
    assert!(
        result.starts_with(": ") && result.contains("lattice-no-such-interpreter"),
        "the spawn error is the result: {t}"
    );
    assert!(
        last_echo(&editor).unwrap_or_default().contains("failed"),
        "{:?}",
        last_echo(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editing_above_the_block_while_it_runs_still_lands_on_it() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    edit(&mut editor, (0, 0), (0, 0), "new line\nanother\n");
    settle(&mut editor, has("#+RESULTS:")).await;
    assert_eq!(
        text(&editor),
        "new line\nanother\n* Run\n#+begin_src sh\nsleep 0.5\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nafter\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editing_the_block_body_while_it_runs_sends_output_to_org_babel() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    // `echo hi` → `echo ho`: the block the run was for is not there any more.
    edit(&mut editor, (3, 6), (3, 7), "o");
    let edited = text(&editor);
    editor.last_message = None;
    settle(&mut editor, |e| e.last_message.is_some()).await;
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: the block changed while it ran — the sh block's output is in *org-babel*")
    );
    assert_eq!(text(&editor), edited, "nothing written at a stale place");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_the_block_while_it_runs_sends_output_to_org_babel() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    edit(&mut editor, (1, 0), (5, 0), "");
    editor.last_message = None;
    settle(&mut editor, |e| e.last_message.is_some()).await;
    assert!(
        last_echo(&editor)
            .unwrap_or_default()
            .ends_with("output is in *org-babel*"),
        "{:?}",
        last_echo(&editor)
    );
    assert_eq!(text(&editor), "* Run\nafter\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_identical_unnamed_blocks_each_get_their_own_result() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh\necho hi\n#+end_src\nmid\n#+begin_src sh\necho hi\n#+end_src\nend\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 5, has("#+RESULTS:")).await;
    assert_eq!(
        text(&editor),
        "#+begin_src sh\necho hi\n#+end_src\nmid\n#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nend\n",
        "the second block's result is the second block's"
    );
    run_and_settle(&mut editor, 1, |e| {
        text(e).matches("#+RESULTS:").count() == 2
    })
    .await;
    assert_eq!(
        text(&editor),
        "#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nmid\n#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nend\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_block_is_not_started_twice() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: the sh block is already running — :org-babel-cancel stops it")
    );
    settle(&mut editor, has("#+RESULTS:")).await;
    assert_eq!(
        text(&editor).matches(": hi").count(),
        1,
        "{}",
        text(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_past_the_cap_is_truncated_with_a_count() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("for i in 1 2 3 4 5 6 7 8; do echo $i; done"),
        &[("sh", "sh")],
    )
    .await;
    set_org_option(&mut editor, "babel.max-output-lines", "5");
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    assert!(
        text(&editor)
            .contains("#+RESULTS:\n: 1\n: 2\n: 3\n: 4\n: 5\n: … 3 lines truncated\nafter\n"),
        "{}",
        text(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_result_is_one_undo_step() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh\necho new\n#+end_src\n\n#+RESULTS:\n: old\nafter\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 1, has(": new")).await;
    press(&mut editor, "u");
    assert_eq!(
        text(&editor),
        doc,
        "one `u` restores the previous result exactly"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_script_file_is_removed_after_the_run() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("echo hi"), &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    let scripts: Vec<_> = walk(&base.path().join("data"))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "sh"))
        .collect();
    assert!(scripts.is_empty(), "scripts left behind: {scripts:?}");
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// The lines the gutter producers put a sign on, for the org buffer — the
/// question the renderer asks.
async fn signed_lines(editor: &Editor) -> Vec<u32> {
    let registry = editor
        .services
        .get::<lattice_mode::GutterDecorationSourceRegistryHandle>()
        .expect("a boot service");
    let id = u64::from(editor.document_buffer_id.0);
    let count = editor.document.snapshot().buffer.rope_line_count();
    let mut lines = Vec::new();
    for source in registry.load().sources() {
        for d in source.produce(id, None, count).await.unwrap_or_default() {
            let lattice_mode::GutterDecoration::Sign { line, .. } = d;
            lines.push(line);
        }
    }
    lines
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_block_carries_a_sign_until_it_finishes() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    assert!(signed_lines(&editor).await.is_empty(), "nothing runs yet");
    goto_line(&mut editor, 3);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(signed_lines(&editor).await, [1], "on its #+begin_src");
    // It follows the block when lines go in above it.
    edit(&mut editor, (0, 0), (0, 0), "above\n");
    assert_eq!(signed_lines(&editor).await, [2]);
    settle(&mut editor, has("#+RESULTS:")).await;
    assert!(
        signed_lines(&editor).await.is_empty(),
        "gone once it finished"
    );
}

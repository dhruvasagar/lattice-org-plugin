//! `org-export-mode`: the dispatcher, the gate, the target check, and an
//! export that lands without a further key.
//!
//! Boots a real editor, loads the built org component through the loader, and
//! drives export through its chords and commands, as a user would. Skips (with
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
    // What `install` wires and a hand-built loader does not. Each one
    // unwired fails a run silently in its own way: no buffer store, and every
    // buffer reads as closed; no active-modes query, and `buffer-text`
    // refuses; no sink, and a finished job's edit goes nowhere.
    if let Some(buffers) = editor.services.get::<lattice_mode::BufferStoreHandle>() {
        host.set_buffer_store((*buffers).clone());
    }
    // And no output store: `output-append` fails, and `*org-babel*` stays
    // empty however the run ended.
    if let Some(output) = editor
        .services
        .get::<lattice_plugin_host::output::PluginOutputHandle>()
    {
        host.set_plugin_output((*output).clone());
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
            // The media seam fails the whole load when unwired, by
            // design — a plugin declaring `media` whose images can never be
            // drawn should say so rather than load silently half-working.
            media_registry: Some(std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                lattice_mode::MediaSourceRegistry::new(),
            ))),
            // Refile's target list is a picker source, so the loader
            // needs somewhere to register it.
            picker_registry: Some(editor.picker_registry.clone()),
            // Babel's running sign — the editor's own sign and decoration
            // registries, so a test asks the producer the renderer asks.
            sign_registry: editor
                .services
                .get::<lattice_mode::SignRegistryHandle>()
                .map(|h| (*h).clone()),
            decoration_registry: editor
                .services
                .get::<lattice_mode::GutterDecorationSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            // Org's menus (capture, export) register into the editor's OWN
            // transient registry, the one `editor_boot` builds, so a plugin menu
            // does not depend on whether magit happened to load.
            transient_registry: editor
                .services
                .get::<lattice_picker::TransientSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            // The agenda is a plugin-owned view now, so the loader
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

/// Boot an editor, load the org plugin into its live registries, and open
/// `base/notes.org` holding `text`. `capabilities` are the grants beyond
/// `state:write`; an export needs `fs:write` over where it writes.
///
/// The loader comes back too, and a test must hold it: its option
/// subscriptions (`org.export.enabled`'s gate among them) hold it weakly and
/// stop when it drops.
async fn org_editor(
    base: &std::path::Path,
    text: &str,
    capabilities: &[String],
) -> (Editor, Arc<PluginLoader>) {
    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    let caps: String = capabilities.iter().map(|c| format!(", \"{c}\"")).collect();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "id = \"org\"\nprovides = [\"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\", \"picker-source\", \"transient-source\", \"events\", \"signs\", \"decorations\"]\ndefault_modes = [\"org-todo-mode\", \"org-global-mode\", \"org-table-mode\", \"org-babel-mode\", \"org-export-mode\"]\nmode_gates = {{ \"org-babel-mode\" = \"org.babel.enabled\", \"org-export-mode\" = \"org.export.enabled\" }}\neditor_capabilities = [\"tree-sitter\"]\ncapabilities = [\"state:write\"{caps}]\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("component.wasm"),
        org_plugin_wasm().expect("caller checked"),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loader = Arc::new(loader_over_editor(&editor, base));
    loader.subscribe_mode_gates();
    let loaded = loader
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");
    // Enablement before the open, activation after it: both drains hang off
    // the tick, so tick on both sides (see `org_babel.rs`).
    editor.run_tick_pending();
    let file = base.join("notes.org");
    std::fs::write(&file, text).unwrap();
    editor.do_edit(Some(file), false);
    editor.run_tick_pending();
    editor.publish_render_state();
    assert!(
        export_active(&editor),
        "org-export-mode is on in an org buffer"
    );
    (editor, loader)
}

fn export_active(editor: &Editor) -> bool {
    editor
        .active_modes
        .get(&editor.document_buffer_id)
        .is_some_and(|m| m.is_active(ModeId::new("org-export-mode")))
}

/// The title of the transient showing, if any.
fn menu_title(editor: &Editor) -> Option<String> {
    editor
        .picker
        .as_ref()
        .and_then(|p| p.transient.as_ref())
        .map(|t| t.title.clone())
}

/// A grant over `dir`, as the manifest's option-bound one resolves to.
fn write_grant(dir: &std::path::Path) -> Vec<String> {
    vec![format!("fs:write:{}", dir.display())]
}

const DOC: &str = "#+title: Plan\n* One\nfirst <b>\n* Two\nsecond\n";

/// Press `keys`, applying `OpenTransient` as the renderer peer's effect tail
/// does (`handle_effect` leaves it to the peer). The effects come back.
fn press(editor: &mut Editor, keys: &str) -> Vec<lattice_grammar::Effect> {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial: Vec<KeyChord> = Vec::new();
    let mut effects = Vec::new();
    for c in seq {
        let out = editor.dispatch_chord_with_outcome(c, &mut partial).1;
        for effect in out.effects {
            if let lattice_grammar::Effect::OpenTransient { source, args } = &effect {
                editor.open_named_transient(source.clone(), args.clone());
            }
            effects.push(effect);
        }
    }
    editor.publish_render_state();
    effects
}

/// `C-c C-e`, then wait for the menu: the guest builds it on a call parked on
/// the async-landed wake, so it is not seated when `OpenTransient` is applied.
async fn open_dispatcher(editor: &mut Editor) -> Option<String> {
    press(editor, "<C-c><C-e>");
    for _ in 0..200 {
        editor.run_tick_pending();
        if let Some(title) = menu_title(editor) {
            return Some(title);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    None
}

/// Run an ex line; the effects come back as from [`press`].
fn ex(editor: &mut Editor, line: &str) -> Vec<lattice_grammar::Effect> {
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.execute_ex_line(line, &mut out);
    // A host-applied edit comes back as a next action, which the dispatch
    // wrapper drains — do what it does.
    for action in std::mem::take(&mut out.next_actions) {
        let _ = editor.dispatch(action);
    }
    editor.publish_render_state();
    out.effects
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
/// `C-c C-e` is export's, and only in org: a text buffer never sees the
/// layer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_dispatcher_opens_in_org_buffers_only() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor(base.path(), DOC, &[]).await;

    assert_eq!(
        open_dispatcher(&mut editor).await.as_deref(),
        Some("Export")
    );
    press(&mut editor, "q");
    assert_eq!(menu_title(&editor), None);

    let txt = base.path().join("notes.txt");
    std::fs::write(&txt, "plain\n").unwrap();
    editor.do_edit(Some(txt), false);
    editor.run_tick_pending();
    editor.publish_render_state();
    assert!(!export_active(&editor));
    assert_eq!(
        open_dispatcher(&mut editor).await,
        None,
        "no layer, no menu"
    );
}

/// Off, the layer goes and the commands say so rather than export; the rest
/// of org stays on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_off_removes_the_dispatcher_and_the_commands_effect() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;

    set_org_option(&mut editor, "export.enabled", "false");
    settle(&mut editor, |e| !export_active(e)).await;
    assert_eq!(
        open_dispatcher(&mut editor).await,
        None,
        "no layer, no menu"
    );
    ex(&mut editor, "org-export-html");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: export is off — set org.export.enabled to use it")
    );
    assert!(!base.path().join("notes.html").exists());
    assert!(editor
        .active_modes
        .get(&editor.document_buffer_id)
        .is_some_and(|m| m.is_active(ModeId::new("org-todo-mode"))));

    set_org_option(&mut editor, "export.enabled", "true");
    settle(&mut editor, export_active).await;
}

/// Where org may not write is said at once, naming the option that would
/// allow it, and nothing is exported.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_target_is_reported_before_anything_runs() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let grant = write_grant(elsewhere.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;

    ex(&mut editor, "org-export-html");
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(echo.contains("org.export.directories"), "{echo}");
    assert!(echo.contains("notes.html"), "{echo}");
    // Given every chance to have run anyway.
    for _ in 0..5 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        editor.run_tick_pending();
    }
    assert!(!base.path().join("notes.html").exists());
    assert_eq!(last_echo(&editor).as_deref(), Some(echo.as_str()));
}

/// The chord emits and returns; the export runs on the events seam and its
/// file lands with no key pressed after the row's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_lands_on_disk_without_a_further_keypress() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;
    let html = base.path().join("notes.html");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "hh");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("Exporting notes.org → notes.html…")
    );
    settle(&mut editor, |_| html.exists()).await;
    let written = std::fs::read_to_string(&html).unwrap();
    assert!(written.contains("<title>Plan</title>"), "{written}");
    assert!(written.contains("first &lt;b&gt;"), "{written}");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("Exported notes.html")
    })
    .await;
}

/// A file whose parse does not fit one call's fuel is exported over several
/// steps, still with no key pressed, and the whole of it lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_too_large_for_one_call_is_exported_in_steps() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let section = "* TODO [#A] A heading :tag:\n\
                   SCHEDULED: <2026-01-05 Mon>\n\
                   - [ ] an item\n  continued\n- term :: description\n\
                   | a | b |\n|---+---|\n| 1 | 2 |\n\
                   #+begin_src sh\necho x\n#+end_src\n\
                   A paragraph of text.\n";
    let mut doc = String::new();
    for i in 0..4_000 {
        doc.push_str(&section.replace("A heading", &format!("Heading {i}")));
    }
    let (mut editor, _loader) = org_editor(base.path(), &doc, &grant).await;
    let md = base.path().join("notes.md");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    settle(&mut editor, |e| {
        md.exists() || last_echo(e).is_some_and(|m| !m.starts_with("Exporting"))
    })
    .await;
    let written = std::fs::read_to_string(&md)
        .unwrap_or_else(|_| panic!("nothing written; the echo is {:?}", last_echo(&editor)));
    assert!(written.contains("\"Heading 0\""), "the first section");
    assert!(written.contains("\"Heading 3999\""), "and the last");
}

/// The file's options shape what is exported, and what it asks for that the
/// export does not do is named in the closing echo rather than dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_files_options_shape_the_export_and_the_unsupported_are_named() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let doc = "#+SETUPFILE: theme.org\n* Kept\n* Dropped :noexport:\n* COMMENT Draft\n\
               * Last\n#+OPTIONS: ^:nil html-postamble:nil\na_b\n";
    let (mut editor, _loader) = org_editor(base.path(), doc, &grant).await;
    let md = base.path().join("notes.md");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    settle(&mut editor, |e| {
        last_echo(e).is_some_and(|m| m.starts_with("Exported"))
    })
    .await;
    let written = std::fs::read_to_string(&md).unwrap();
    assert!(
        written.contains("\"Kept\"") && written.contains("\"Last\""),
        "{written}"
    );
    assert!(
        !written.contains("Dropped") && !written.contains("Draft"),
        "{written}"
    );
    assert!(
        written.contains("\"a_b\""),
        "^:nil, set after the text: {written}"
    );
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("Exported notes.md; not supported, so left out: #+SETUPFILE, html-postamble:")
    );
}

/// A switch flipped in the dispatcher reaches the command, and is still set
/// the next time the dispatcher opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_switches_reach_the_export_and_stay_set() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;
    let md = base.path().join("notes.md");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "<C-b>mm");
    settle(&mut editor, |_| md.exists()).await;
    let body_only = std::fs::read_to_string(&md).unwrap();
    assert!(
        body_only.starts_with("```\n") && !body_only.contains("# Plan"),
        "no title heading: {body_only}"
    );

    std::fs::remove_file(&md).unwrap();
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    settle(&mut editor, |_| md.exists()).await;
    let again = std::fs::read_to_string(&md).unwrap();
    assert!(again.starts_with("```\n"), "still body only: {again}");
}

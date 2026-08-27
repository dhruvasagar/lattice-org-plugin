//! OM.2 — opening a `.org` file activates `org-mode`, a major mode that ships
//! inside a plugin.
//!
//! This is the slice's headline claim and the one that failed before it. The
//! host resolves a document's major through
//! `lattice_syntax::major_mode_id_for_lang`, a hand-written `match` over the
//! `Lang` enum whose `Lang::Plugin(_)` arm returns `None` **by design** — the
//! host cannot have an arm for a language it has never heard of. So however
//! good the org plugin's grammar was, a `.org` buffer landed in `text-mode`.
//!
//! The route was already designated (that arm's comment says a plugin
//! language's major "is contributed through the `modes` seam by the plugin that
//! owns it"); OM.1 built the registry's language index and OM.2 opened the seam
//! to majors. This test walks the whole path with the REAL reference plugin:
//! discover → load → `language` + `modes` seams drain → open a file → the
//! editor's ordinary activation resolves org-mode.
//!
//! ## Mode-ownership acid test
//!
//! Nothing in `lattice-host` knows what org is. No `Editor::` method, no
//! `Action` variant, no `BufferKind`, no `Lang` arm. The plugin contributed a
//! language and the major that owns it, and the generic resolver did the rest.
//!
//! Skips when the component was not built — `cargo test` builds this crate
//! for the HOST, and the component loaded below is a separate
//! `--target wasm32-wasip2 --release` artefact. The `org_folds.rs` precedent.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_mode::{ModeId, ModeKind};
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{discover, LoaderServices, PluginLoader};

/// Boot an editor that is sealed off from the developer's real
/// `~/.config/lattice`.
///
/// `Editor::boot` kicks off plugin auto-discovery on a spawned task, scanning
/// the core-plugins root, `init/` and `~/.config/lattice/plugins/`. Once org is
/// installed there for real use — which is the whole point of this plugin — that
/// task registers `org-mode` into the very registry these tests assert about,
/// and it does so asynchronously: it only makes progress at an `.await`, so the
/// tests pass or fail depending on how the load below happens to be scheduled.
/// It cost an afternoon as a ~1-in-6 flake.
///
/// `disable_autoload` is the documented seal (it is what
/// `lattice-ui-tui`'s `test_helpers` uses). Manual loading through the loader —
/// what every test here does — is unaffected. Process-global and idempotent.
fn boot_sealed_editor() -> Editor {
    lattice_plugin_loader::disable_autoload();
    Editor::boot(CoreDocument::from_text("scratch\n"))
}

/// The reference org component, if it was built. Built out-of-workspace with
/// `cargo build --release --target wasm32-wasip2`.
fn org_plugin_wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    );
    std::fs::read(path).ok()
}

/// Lay the plugin out on disk the way the manager would. Note the `provides`
/// order is deliberately NOT dependency order — OM.0 made the loader sort, and
/// org is exactly the plugin that would have suffered from it.
fn write_org_plugin_dir(root: &std::path::Path, wasm: &[u8]) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"modes\", \"language\", \"help\", \"config\", \"media\"]\ndefault_mode = \"org-todo-mode\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();
}

/// A loader wired to the booted editor's LIVE registries, so the plugin's mode
/// lands in the same `mode_registry` the editor's activation reads.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_an_org_file_activates_the_plugins_major_mode() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);

    let mut editor = boot_sealed_editor();

    // --- Before load: nothing in the editor knows what org is.
    assert!(
        !editor
            .mode_registry
            .load()
            .is_registered(ModeId::new("org-mode")),
        "org-mode is not a built-in"
    );

    assert_eq!(discover(&plugins_dir).len(), 1, "discovery finds org");
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    // --- The mode registered, AS A MAJOR, and claimed the language its own
    // `language` seam registered in the same load.
    let org_mode = ModeId::new("org-mode");
    let registry = editor.mode_registry.load();
    let entry = registry
        .get(org_mode)
        .expect("org-mode registered from the component");
    assert_eq!(
        entry.kind(),
        ModeKind::Major,
        "it registered as a major, not downgraded to a minor"
    );
    assert_eq!(
        registry.find_major_for_lang("org"),
        Some(org_mode),
        "and owns `org` in the registry's language index"
    );
    drop(registry);

    // --- Open a real `.org` file through the ordinary `:e` path.
    let file = base.path().join("notes.org");
    std::fs::write(&file, "* Top level\n** Second\nbody text\n").unwrap();
    editor.do_edit(Some(file.clone()), false);

    // The language seam's extension mapping resolved `.org`...
    let detected = lattice_syntax::Lang::detect_from_path(Some(file.as_path()));
    assert_eq!(
        detected.name(),
        "org",
        "the plugin's `language` seam claimed the .org extension"
    );

    // ...and the editor's GENERIC activation put the buffer in the plugin's
    // major. No org-specific code ran anywhere in the host to make this true.
    let buffer = editor.document_buffer_id;
    let major = editor
        .active_modes
        .get(&buffer)
        .and_then(|m| m.major())
        .expect("the buffer has an active major");
    assert_eq!(
        major, org_mode,
        "a .org buffer activates the plugin's major (it was text-mode before OM.2)"
    );
}

/// Graceful degradation, and the shape a partially-working install takes: the
/// `language` seam can land without the `modes` seam — a plugin may declare
/// only `language`, or its mode declaration may be rejected. An org buffer then
/// highlights and folds perfectly well in `text-mode`, which is a good outcome,
/// not an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_language_without_its_major_still_opens_in_text_mode() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    let dir = plugins_dir.join("org-lang-only");
    std::fs::create_dir_all(&dir).unwrap();
    // `provides` names the language seam ONLY — the component still exports
    // `register-modes`, but a seam the manifest does not declare is never
    // driven.
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org-lang-only\"\nprovides = [\"language\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), &wasm).unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the language-only plugin loads");

    assert!(
        !editor
            .mode_registry
            .load()
            .is_registered(ModeId::new("org-mode")),
        "no modes seam declared, so no org-mode"
    );

    let file = base.path().join("notes.org");
    std::fs::write(&file, "* Top level\n").unwrap();
    editor.do_edit(Some(file), false);

    let buffer = editor.document_buffer_id;
    let major = editor
        .active_modes
        .get(&buffer)
        .and_then(|m| m.major())
        .expect("the buffer has an active major");
    assert_eq!(
        major,
        ModeId::new("text-mode"),
        "an unclaimed plugin language falls back to text-mode, not to nothing"
    );
}

/// OC.1 — `org-global-mode` is active in a buffer that has nothing to do with
/// org, which is the entire reason it exists.
///
/// Capture and the agenda are the two org verbs whose value is that they work
/// from wherever you are: the thought you are trying not to lose arrives while
/// you are reading code. OM.11 bound capture on the org MAJOR, so it only fired
/// inside an org file — backwards for the one verb meant to reach it from
/// anywhere.
///
/// Asserted through ACTIVATION rather than a keymap lookup with the mode id
/// handed in. A `lookup_with_context` that is told the mode is active proves
/// the layer exists; it cannot prove the mode ever turns on, and the way this
/// fails is precisely that it does not (a plugin minor is inert until enabled,
/// and OC.1a had to make the manifest able to name two).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_global_mode_activates_in_a_buffer_that_is_not_org() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    // The shipped manifest's spelling: BOTH modes on by default, one gate.
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\n\
         provides = [\"modes\", \"language\", \"help\", \"config\", \"media\"]\n\
         default_modes = [\"org-todo-mode\", \"org-global-mode\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), &wasm).unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    // Enablement first, then the buffer: the order is load-bearing. The
    // enablement drain has to run BEFORE the major is entered, or activation
    // reads the mode as still disabled and refuses it.
    editor.run_tick_pending();

    // A plain text file — no org anywhere near it.
    let file = base.path().join("notes.txt");
    std::fs::write(&file, "just some prose\n").unwrap();
    editor.do_edit(Some(file), false);
    editor.run_tick_pending();

    let buffer = editor.document_buffer_id;
    let active = editor
        .active_modes
        .get(&buffer)
        .expect("the buffer has an active mode set");
    assert_ne!(
        active.major(),
        Some(ModeId::new("org-mode")),
        "sanity: this is NOT an org buffer"
    );
    assert!(
        active.minors().contains(&ModeId::new("org-global-mode")),
        "the universal minor is on, so <C-x>oc reaches capture from here; \
         active minors were {:?}",
        active.minors()
    );
    // And the org-file minor is correctly NOT on: `Majors(["org-mode"])` still
    // means what it says, so this did not turn everything on.
    assert!(
        !active.minors().contains(&ModeId::new("org-todo-mode")),
        "org-todo-mode stays scoped to org buffers"
    );
}

/// Opening a `.org` file in a real editor produces HIGHLIGHT SPANS.
///
/// The gap this closes is the one a user actually reports: "org files are not
/// coloured". Everything around it was green — the language seam registers
/// (`org_highlight_from_component`), the queries compile and match
/// (`org_folds`), the major mode activates (above) — because every one of
/// those goes through the process-wide registry.
///
/// The EDITOR did not. `Editor::lang_registry` is a snapshot taken at boot,
/// and a plugin language RCUs its compiled grammar into the global registry
/// when the plugin loads, which is always after boot. So `.org` resolved by
/// name, the major activated, and then the grammar lookup missed and the
/// buffer got no syntax at all: blank highlighting, folds fallen back to the
/// generic provider, and nothing anywhere saying why.
///
/// So this asserts on `editor.syntax` after a real `do_edit`, which is the
/// only thing that would have caught it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_an_org_file_produces_highlight_spans() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1,
        "the org component loads"
    );

    let file = base.path().join("notes.org");
    std::fs::write(
        &file,
        "* TODO Ship it :work:\nSCHEDULED: <2026-08-27 Thu>\n- [X] done\n- [ ] todo\n",
    )
    .unwrap();
    editor.do_edit(Some(file), false);
    editor.run_tick_pending();

    let syntax = editor.syntax.as_ref().expect(
        "the buffer has a syntax — a plugin language registered AFTER boot must still be found",
    );
    let styles: Vec<lattice_syntax::Style> = syntax.with_snapshot(|snap| {
        snap.highlight_lines(0, 4)
            .expect("highlighting runs")
            .into_iter()
            .flatten()
            .map(|s| s.style)
            .collect()
    });
    assert!(
        !styles.is_empty(),
        "org files are coloured — this is the user-visible assertion"
    );
    // And it is ORG colouring, not something incidental: a headline, its TODO
    // keyword and its tag all come from `highlights.scm`.
    assert!(
        styles.contains(&lattice_syntax::Style::Heading1),
        "the headline is a heading; got {styles:?}"
    );
}

/// MO.3 — org buffers fold by syntax because `org-mode` says so, not because
/// the user happened to configure it.
///
/// **This is the failure the whole option-override seam was built for.** Org's
/// folding is structural: headline nesting IS the fold tree, and
/// `foldmethod=syntax` is what produces it. Before the seam carried options,
/// org could only hope the user had set that globally — so `<Tab>` cycling
/// worked on the author's machine and silently did nothing on anyone else's,
/// with no error to explain the difference. A native major would simply have
/// declared it; MO.1/MO.2 let this one do the same.
///
/// The test asserts BOTH halves, and the second is what makes it about
/// overrides rather than about folding:
///   - in an org buffer `foldmethod` resolves to `syntax`,
///   - the user's global `foldmethod` is still `manual` — untouched. A mode
///     override is a resolution *layer*, so a plugin cannot quietly
///     reconfigure the editor for every other buffer while it is at it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_org_buffer_folds_by_syntax_without_the_user_setting_it() {
    use lattice_config::FoldMethodOption;
    use lattice_core::FoldMethod;

    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);

    let mut editor = boot_sealed_editor();

    // The precondition that makes this test mean anything: nobody has set
    // `foldmethod`. If the shipped default were already `syntax` the assertion
    // below would pass on the broken version too.
    assert_eq!(
        *editor
            .config
            .get_typed::<FoldMethodOption>()
            .expect("registered"),
        FoldMethod::Manual,
        "sanity: the global default is `manual`, so `syntax` can only come from the mode"
    );

    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    let file = base.path().join("notes.org");
    std::fs::write(&file, "* Top level\n** Second\nbody text\n").unwrap();
    editor.do_edit(Some(file), false);
    editor.run_tick_pending();

    let buffer = editor.document_buffer_id;
    assert_eq!(
        *editor.resolved_option::<FoldMethodOption>(buffer),
        FoldMethod::Syntax,
        "the org buffer folds by syntax because org-mode declared it"
    );

    // A LAYER, not a write. A `foldmethod=indent` user must still get indent
    // folds in every non-org buffer they open.
    assert_eq!(
        *editor
            .config
            .get_typed::<FoldMethodOption>()
            .expect("registered"),
        FoldMethod::Manual,
        "and the user's global setting is untouched"
    );
}

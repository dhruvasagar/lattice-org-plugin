//! OM.A1 — `:agenda` over real org files, produced by the real plugin.
//!
//! The slice's exit criterion: excerpts appear in a multibuffer. This walks
//! the whole path — discover → load → the `agenda-source` seam drains → the
//! host's agenda provider walks a directory of `.org` files → the plugin's
//! `scan` answers → the rows land as excerpts, in the plugin's order, under
//! the plugin's headers.
//!
//! ## Mode-ownership acid test
//!
//! Nothing in `lattice-host` knows what org is, and this time not even what a
//! `.org` file is. The host does not filter on the extension: the plugin's
//! `extensions()` export declares it, the registry answers `claims()` from
//! that, and a file nothing claims is never read. No `Editor::` method, no
//! host `Action` variant, no dispatch arm — `:agenda` reaches the view through
//! the generic provider-view seam.
//!
//! Skips when the component was not built — `cargo test` builds this crate
//! for the HOST, and the component loaded below is a separate
//! `--target wasm32-wasip2 --release` artefact.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_multibuffer::{HeaderlineStatus, MultibufferRegistryHandle};
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{discover, LoaderServices, PluginLoader};

/// See `org_major_mode.rs` — `Editor::boot` auto-discovers plugins on a
/// spawned task, which flakes ~1-in-6 against a developer's real
/// `~/.config/lattice`. This is the documented seal.
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

/// `provides` names `agenda-source` alongside the rest, and again NOT in
/// dependency order — OM.0 made the loader sort, so manifest order is
/// cosmetic and this is where that keeps being true.
fn write_org_plugin_dir(root: &std::path::Path, wasm: &[u8]) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"agenda-source\", \"modes\", \"language\", \"help\", \"config\", \"media\"]\ndefault_mode = \"org-todo-mode\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();
}

/// A loader wired to the booted editor's LIVE registries — including the
/// agenda registry the editor published at boot, so the producer the plugin
/// registers is the one the provider's scan reads.
fn loader_over_editor(editor: &Editor, base: &std::path::Path) -> PluginLoader {
    let host = Arc::new(
        PluginHost::with_dirs(base.join("cache"), base.join("data")).expect("host builds"),
    );
    let agenda_registry = editor
        .services
        .get::<lattice_mode::AgendaSourceRegistryHandle>()
        .map(|h| (*h).clone())
        .expect("the editor publishes the agenda registry at boot");
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
            media_registry: Some(Arc::new(arc_swap::ArcSwap::from_pointee(
                lattice_mode::MediaSourceRegistry::new(),
            ))),
            agenda_registry: Some(agenda_registry),
            ..Default::default()
        },
    )
}

/// A small org corpus, plus a file nothing claims.
fn write_corpus(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("work.org"),
        "#+TITLE: work\n* TODO Ship the thing\n** Sub task\nsome body\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("home.org"),
        "* Groceries\nmilk\n* TODO Fix the tap\n",
    )
    .unwrap();
    // Claimed by nobody: the provider must never read it, let alone cross it.
    std::fs::write(dir.join("main.rs"), "* Top\nfn main() {}\n").unwrap();
}

/// Poll until the scan reaches a terminal headerline. The scan hops through
/// `spawn_blocking` and the guest actor, so there is no single future to await
/// — and asserting before it settles is how this test would pass on a broken
/// build that simply never finished.
async fn settle_agenda(
    registry: &MultibufferRegistryHandle,
    view: lattice_core::BufferId,
) -> HeaderlineStatus {
    for _ in 0..600 {
        if let Some(h) = registry.handle(view) {
            let status = (*h.headerline()).clone();
            if matches!(
                status,
                HeaderlineStatus::Complete { .. } | HeaderlineStatus::Failed { .. }
            ) {
                return status;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the agenda scan never reached a terminal headerline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_collects_headlines_from_every_org_file_in_the_project() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);
    let notes = base.path().join("notes");
    write_corpus(&notes);

    let mut editor = boot_sealed_editor();

    // --- Before load: no producer, so `:agenda` declines rather than opening
    // an empty view the user has to guess about.
    let sources = editor
        .services
        .get::<lattice_mode::AgendaSourceRegistryHandle>()
        .unwrap();
    assert!(
        sources.load().is_empty(),
        "no plugin provides agenda rows yet"
    );

    assert_eq!(discover(&plugins_dir).len(), 1, "discovery finds org");
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    // --- The producer registered, and declared the extensions the HOST then
    // filters on. This is the acid test in one assertion: the host learned
    // what an org file is from the plugin, at load, over the ABI.
    let snapshot = sources.load();
    assert_eq!(snapshot.len(), 1, "org registered its agenda producer");
    let source = &snapshot.sources()[0];
    assert_eq!(
        source.extensions(),
        ["org".to_string(), "org_archive".to_string()],
        "declared by the guest, normalised by the loader"
    );
    assert!(source.claims(std::path::Path::new("/p/notes.org")));
    assert!(!source.claims(std::path::Path::new("/p/main.rs")));
    drop(snapshot);

    // --- `:agenda <dir>` through the ordinary ex-command path.
    let view = lattice_multibuffer::providers::agenda::open_agenda(
        &mut editor,
        &lattice_grammar::Args::String(notes.display().to_string()),
    );
    let view = match view {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        lattice_mode::ProviderViewOutcome::Declined { message } => {
            panic!("the agenda declined: {message}")
        }
    };

    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .expect("the multibuffer registry is a boot service");
    let status = settle_agenda(&mb, view).await;

    let handle = mb.handle(view).expect("the view is still open");
    let excerpts = handle.excerpts();

    // Four headlines across two files; `main.rs` contributes nothing because
    // nothing claimed it — its `* Top` line would show up if the walk had
    // read it.
    assert_eq!(excerpts.len(), 4, "got {status:?}");

    // One header per file (the OM.A1 grouping), blank on the file's other
    // rows. Walk order is not guaranteed between `home.org` and `work.org`,
    // so the assertion is on the SHAPE: two headers, two blanks, and every
    // header naming a file in the corpus.
    let titles: Vec<String> = excerpts.iter().map(|e| e.header.title.clone()).collect();
    let named: Vec<&String> = titles.iter().filter(|t| !t.is_empty()).collect();
    assert_eq!(named.len(), 2, "one header per file, got {titles:?}");
    for title in &named {
        assert!(
            ["work.org", "home.org"].contains(&title.as_str()),
            "a header names its file, got {title}"
        );
    }
    assert_eq!(titles[1], "", "a file's second row continues its group");

    // Rows of one file share one source document, so an edit through one row
    // is visible through its neighbour.
    let sources_used: std::collections::HashSet<_> = excerpts.iter().map(|e| e.source).collect();
    assert_eq!(sources_used.len(), 2, "one source document per file");

    match status {
        HeaderlineStatus::Complete { summary, .. } => {
            assert!(summary.contains("4 row(s)"), "got {summary}");
        }
        other => panic!("expected a Complete headerline, got {other:?}"),
    }
}

/// With no agenda plugin loaded, `:agenda` declines with a reason rather than
/// opening a blank view. Declining is a first-class outcome — an empty view
/// the user has to guess about is the worse UX.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_declines_when_no_plugin_provides_rows() {
    let mut editor = boot_sealed_editor();
    match lattice_multibuffer::providers::agenda::open_agenda(
        &mut editor,
        &lattice_grammar::Args::None,
    ) {
        lattice_mode::ProviderViewOutcome::Declined { message } => {
            assert!(
                message.contains("no plugin provides agenda rows"),
                "{message}"
            );
        }
        other => panic!("expected a Declined outcome, got {other:?}"),
    }
}

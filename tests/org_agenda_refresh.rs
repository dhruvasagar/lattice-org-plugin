//! Repro: "agenda on refresh breaks (does not load anything back)".
//!
//! Opens the agenda through the real plugin, settles, presses `gr`, and
//! asserts the rows come back. Same harness as `org_agenda.rs`.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_multibuffer::{HeaderlineStatus, MultibufferRegistryHandle};
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::parse_chord_sequence;
use lattice_runtime::document::Document as _;

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

fn write_org_plugin_dir(root: &std::path::Path, wasm: &[u8]) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"scanned-excerpt-source\", \"multibuffer-view-source\", \"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\"]\ndefault_mode = \"org-todo-mode\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();
}

fn loader_over_editor(editor: &Editor, base: &std::path::Path) -> PluginLoader {
    let host = Arc::new(
        PluginHost::with_dirs(base.join("cache"), base.join("data")).expect("host builds"),
    );
    let agenda_registry = editor
        .services
        .get::<lattice_mode::ScannedExcerptSourceRegistryHandle>()
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

fn today() -> i64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 86_400) as i64
}

fn stamp(offset: i64) -> String {
    let z = today() + offset + 719_468;
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
    let name = NAMES[(((h + 6) % 7) as usize) % 7];
    format!("<{y:04}-{m:02}-{d:02} {name}>")
}

async fn settle_long(
    registry: &MultibufferRegistryHandle,
    view: lattice_core::BufferId,
    label: &str,
) -> HeaderlineStatus {
    for i in 0..12_000 {
        if let Some(h) = registry.handle(view) {
            let status = (*h.headerline()).clone();
            if matches!(
                status,
                HeaderlineStatus::Complete { .. } | HeaderlineStatus::Failed { .. }
            ) {
                return status;
            }
            if i % 200 == 0 {
                eprintln!("  [{label} {}ms] {status:?}", i * 10);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{label}: never reached a terminal headerline");
}

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

fn press(editor: &mut Editor, keys: &str) {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial = Vec::new();
    for c in seq {
        let _ = editor.dispatch_chord(c, &mut partial);
    }
}

/// `gr` in the agenda must re-scan and repopulate the view.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gr_in_the_agenda_repopulates_the_view() {
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
            "* TODO Ship the thing\n  SCHEDULED: {}\n* TODO Water the plants\n  SCHEDULED: {}\n",
            stamp(0),
            stamp(1)
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    let view = match lattice_multibuffer::providers::agenda::open_agenda(
        &mut editor,
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
    let status = settle_agenda(&mb, view).await;
    let before = mb.handle(view).unwrap().excerpts().len();
    assert_eq!(before, 2, "the first open populates, got {status:?}");

    // A third row appears on disk. The refresh has to pick it up — which is
    // also what proves the refresh RAN: without this the view keeps its old
    // rows and a count assertion passes on a `gr` that did nothing at all.
    std::fs::write(
        notes.join("only.org"),
        format!(
            "* TODO Ship the thing\n  SCHEDULED: {}\n* TODO Water the plants\n  SCHEDULED: {}\n\
             * TODO Third thing\n  SCHEDULED: {}\n",
            stamp(0),
            stamp(1),
            stamp(2)
        ),
    )
    .unwrap();

    // --- the refresh, typed the way the user types it.
    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    press(&mut editor, "gr");
    editor.run_tick_pending();

    let status = settle_agenda(&mb, view).await;
    let after = mb.handle(view).unwrap().excerpts().len();
    let composed = mb.handle(view).unwrap().snapshot().buffer.as_string();
    eprintln!("AFTER-REFRESH excerpts={after} status={status:?}\ncomposed=<<<{composed}>>>");
    assert_eq!(
        after, 3,
        "`gr` must re-scan and repopulate the agenda, got {status:?}"
    );
    assert!(
        composed.contains("Third thing"),
        "the composed view must show the new row, got {composed:?}"
    );
}

/// The REAL user path: `:org-agenda` with no argument, roots from
/// `org.agenda-files`, then `gr`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gr_repopulates_when_the_roots_came_from_the_option() {
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
            "* TODO Ship the thing\n  SCHEDULED: {}\n* TODO Water the plants\n  SCHEDULED: {}\n",
            stamp(0),
            stamp(1)
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!("org.agenda-files={}", notes.display()),
    });

    let view = match lattice_multibuffer::providers::agenda::open_agenda(
        &mut editor,
        &lattice_grammar::Args::None,
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
    let status = settle_agenda(&mb, view).await;
    let before = mb.handle(view).unwrap().excerpts().len();
    assert_eq!(before, 2, "the first open populates, got {status:?}");

    std::fs::write(
        notes.join("only.org"),
        format!(
            "* TODO Ship the thing\n  SCHEDULED: {}\n* TODO Water the plants\n  SCHEDULED: {}\n\
             * TODO Third thing\n  SCHEDULED: {}\n",
            stamp(0),
            stamp(1),
            stamp(2)
        ),
    )
    .unwrap();

    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    press(&mut editor, "gr");
    editor.run_tick_pending();

    let status = settle_agenda(&mb, view).await;
    let after = mb.handle(view).unwrap().excerpts().len();
    let composed = mb.handle(view).unwrap().snapshot().buffer.as_string();
    eprintln!("OPTION-PATH AFTER-REFRESH excerpts={after} status={status:?}\ncomposed=<<<{composed}>>>");
    assert_eq!(
        after, 3,
        "`gr` must re-scan and repopulate the agenda, got {status:?}"
    );
    assert!(
        composed.contains("Third thing"),
        "the composed view must show the new row, got {composed:?}"
    );
}

/// Two `gr`s in a row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_gr_also_repopulates() {
    let Some(wasm) = org_plugin_wasm() else {
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("only.org"),
        format!("* TODO Ship the thing\n  SCHEDULED: {}\n", stamp(0)),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    let view = match lattice_multibuffer::providers::agenda::open_agenda(
        &mut editor,
        &lattice_grammar::Args::String(notes.display().to_string()),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        other => panic!("{other:?}"),
    };
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();
    settle_agenda(&mb, view).await;
    assert_eq!(mb.handle(view).unwrap().excerpts().len(), 1);

    let _ = editor.activate_buffer(view);
    for round in 1..=3 {
        editor.cursor.line = 0;
        editor.cursor.byte = 0;
        press(&mut editor, "gr");
        editor.run_tick_pending();
        let status = settle_agenda(&mb, view).await;
        let n = mb.handle(view).unwrap().excerpts().len();
        eprintln!("ROUND {round}: excerpts={n} status={status:?}");
        assert_eq!(n, 1, "refresh #{round} emptied the agenda: {status:?}");
    }
}

/// The real agenda workflow: change a TODO state in the view, then `gr`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gr_after_an_edit_in_the_agenda_repopulates() {
    let Some(wasm) = org_plugin_wasm() else {
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
            "* TODO Ship the thing\n  SCHEDULED: {}\n* TODO Water the plants\n  SCHEDULED: {}\n",
            stamp(0),
            stamp(1)
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
    let view = match lattice_multibuffer::providers::agenda::open_agenda(
        &mut editor,
        &lattice_grammar::Args::String(notes.display().to_string()),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        other => panic!("{other:?}"),
    };
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();
    settle_agenda(&mb, view).await;
    assert_eq!(mb.handle(view).unwrap().excerpts().len(), 2);

    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    press(&mut editor, "<leader>ot");
    editor.run_tick_pending();

    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    press(&mut editor, "gr");
    editor.run_tick_pending();
    let status = settle_agenda(&mb, view).await;
    let n = mb.handle(view).unwrap().excerpts().len();
    let composed = mb.handle(view).unwrap().snapshot().buffer.as_string();
    eprintln!("EDIT-THEN-REFRESH excerpts={n} status={status:?} composed=<<<{composed}>>>");
    let on_disk = std::fs::read_to_string(notes.join("only.org")).unwrap();
    eprintln!("ON-DISK=<<<{on_disk}>>>");
    assert!(n > 0, "the agenda emptied after an edit + refresh: {status:?}");
}

/// Against the REAL corpus the user's `org.agenda-files` points at.
/// Ignored by default (environment-dependent); run with `--ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn gr_over_the_real_corpus() {
    let Some(wasm) = org_plugin_wasm() else {
        return;
    };
    let corpus = std::path::PathBuf::from(std::env::var("AGENDA_CORPUS").unwrap());
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!("org.agenda-files={}", corpus.display()),
    });

    let view = match lattice_multibuffer::providers::agenda::open_agenda(
        &mut editor,
        &lattice_grammar::Args::None,
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        other => panic!("{other:?}"),
    };
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();
    let status = settle_long(&mb, view, "first").await;
    let before = mb.handle(view).unwrap().excerpts().len();
    eprintln!(">>> FIRST OPEN: excerpts={before} status={status:?}");

    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    press(&mut editor, "gr");
    editor.run_tick_pending();
    let status = settle_long(&mb, view, "refresh").await;
    let after = mb.handle(view).unwrap().excerpts().len();
    eprintln!(">>> AFTER gr: excerpts={after} status={status:?}");

    assert!(before > 0, "the first open found nothing; corpus wrong?");
    assert_eq!(after, before, "`gr` lost the agenda");
}

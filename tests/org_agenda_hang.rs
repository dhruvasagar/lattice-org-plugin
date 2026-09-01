//! Which file wedges the agenda scan?
//!
//! `AgendaActor::call_scan` runs `parse_for_scan` (host-side tree-sitter) on the
//! actor task before crossing to the guest. Both are suspects; this sweeps the
//! corpus and names the offender.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};

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
        .unwrap();
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

/// Host-side tree-sitter parse of every corpus file, on a watchdog thread so a
/// hang names its file instead of hanging the suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn which_file_wedges_the_host_parse() {
    let Some(wasm) = org_plugin_wasm() else {
        return;
    };
    let corpus = std::path::PathBuf::from(std::env::var("AGENDA_CORPUS").unwrap());
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);

    let editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1,
        "the org component loads (and registers its grammar)"
    );

    // The same candidate set the agenda walks, in the same order.
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                    continue;
                }
                walk(&path, out);
            } else {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase());
                if matches!(ext.as_deref(), Some("org") | Some("org_archive")) {
                    out.push(path);
                }
            }
        }
    }
    walk(&corpus, &mut files);
    eprintln!(">>> {} candidate files", files.len());

    for (i, path) in files.iter().enumerate() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let p = path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let text_for_thread = text.clone();
        let p_for_thread = p.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let lang = lattice_syntax::Lang::detect_from_path(Some(&p_for_thread));
            if let Ok(Some(mut syntax)) = lattice_syntax::Syntax::for_language(lang) {
                syntax.parse(&text_for_thread);
                let _ = syntax.snapshot_owned();
            }
            let _ = tx.send(start.elapsed());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(d) => {
                if d.as_millis() > 100 {
                    eprintln!("  SLOW  #{i} {:?} bytes={} took {:?}", path, text.len(), d);
                }
            }
            Err(_) => {
                panic!(
                    ">>> HOST PARSE HANGS on #{i} {:?} (bytes={})",
                    path,
                    text.len()
                );
            }
        }
    }
    eprintln!(">>> host parse completed for every file");
}

/// Drive the REAL guest `scan` per file with a timeout. The first file that
/// times out is the one that wedges the single-consumer `AgendaActor`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn which_file_wedges_the_guest_scan() {
    let Some(wasm) = org_plugin_wasm() else {
        return;
    };
    let corpus = std::path::PathBuf::from(std::env::var("AGENDA_CORPUS").unwrap());
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);

    let editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    let sources = editor
        .services
        .get::<lattice_mode::ScannedExcerptSourceRegistryHandle>()
        .unwrap();
    let snapshot = sources.load();
    let source = snapshot.sources()[0].clone();
    drop(snapshot);

    source.begin().await.expect("begin");

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                    continue;
                }
                walk(&path, out);
            } else {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase());
                if matches!(ext.as_deref(), Some("org") | Some("org_archive")) {
                    out.push(path);
                }
            }
        }
    }
    walk(&corpus, &mut files);
    eprintln!(">>> {} candidate files", files.len());

    for (i, path) in files.iter().enumerate() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let start = std::time::Instant::now();
        match tokio::time::timeout(
            std::time::Duration::from_secs(20),
            source.scan(path.clone(), text.clone()),
        )
        .await
        {
            Ok(Ok(rows)) => {
                let d = start.elapsed();
                if d.as_millis() > 300 {
                    eprintln!(
                        "  SLOW  #{i} {path:?} bytes={} rows={} {d:?}",
                        text.len(),
                        rows.len()
                    );
                }
            }
            Ok(Err(e)) => eprintln!("  ERR   #{i} {path:?}: {e}"),
            Err(_) => panic!(
                ">>> GUEST scan HANGS on #{i} {path:?} (bytes={})",
                text.len()
            ),
        }
    }
    eprintln!(">>> guest scan completed for every file");
}

/// Is the wedging file an infinite loop, or merely superlinear? Scan it alone
/// with a long budget and report elapsed / trap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn how_long_does_the_wedging_file_take() {
    let Some(wasm) = org_plugin_wasm() else {
        return;
    };
    let file = std::path::PathBuf::from(std::env::var("AGENDA_FILE").unwrap());
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);

    let editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    let sources = editor
        .services
        .get::<lattice_mode::ScannedExcerptSourceRegistryHandle>()
        .unwrap();
    let snapshot = sources.load();
    let source = snapshot.sources()[0].clone();
    drop(snapshot);
    source.begin().await.expect("begin");

    let text = std::fs::read_to_string(&file).unwrap();
    eprintln!(
        ">>> scanning {file:?} ({} bytes, {} lines)",
        text.len(),
        text.lines().count()
    );
    let start = std::time::Instant::now();
    match tokio::time::timeout(
        std::time::Duration::from_secs(600),
        source.scan(file.clone(), text),
    )
    .await
    {
        Ok(Ok(rows)) => eprintln!(
            ">>> COMPLETED in {:?} with {} rows",
            start.elapsed(),
            rows.len()
        ),
        Ok(Err(e)) => eprintln!(">>> ERRORED in {:?}: {e}", start.elapsed()),
        Err(_) => eprintln!(
            ">>> STILL RUNNING after {:?} — no fuel trap, no epoch trap",
            start.elapsed()
        ),
    }
}

/// Scan time vs file size, on a SYNTHETIC file shaped like the offender:
/// a couple of real headlines plus a large brace-heavy body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn scan_time_versus_size() {
    let Some(wasm) = org_plugin_wasm() else {
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);

    let editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    let sources = editor
        .services
        .get::<lattice_mode::ScannedExcerptSourceRegistryHandle>()
        .unwrap();
    let snapshot = sources.load();
    let source = snapshot.sources()[0].clone();
    drop(snapshot);
    source.begin().await.expect("begin");

    for lines in [50usize, 100, 200, 400, 800] {
        let mut text = String::from("#+title: big\n* TODO a thing\n");
        for i in 0..lines {
            text.push_str("     {\n\t \"key\": \"value\",\n\t \"n\": ");
            text.push_str(&i.to_string());
            text.push_str("\n     }\n");
        }
        let path = base.path().join(format!("big-{lines}.org"));
        std::fs::write(&path, &text).unwrap();
        let start = std::time::Instant::now();
        let r = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            source.scan(path.clone(), text.clone()),
        )
        .await;
        eprintln!(
            "  lines={:<6} bytes={:<8} -> {:?} ({})",
            lines * 4 + 2,
            text.len(),
            start.elapsed(),
            match &r {
                Ok(Ok(rows)) => format!("{} rows", rows.len()),
                Ok(Err(e)) => format!("err: {e}"),
                Err(_) => "TIMEOUT".to_string(),
            }
        );
        if r.is_err() {
            break;
        }
    }
}

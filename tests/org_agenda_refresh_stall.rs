//! **Failing tests for "agenda on refresh breaks (does not load anything back)".**
//!
//! Root cause: one guest `scan` call is O(n²) in file size and is NOT bounded
//! by the budget the host arms for it. `AgendaActor` is a single-consumer actor
//! (`crates/lattice-plugin-host/src/agenda_task.rs:197`), so a long `scan`
//! blocks every later call on that plugin — including the refresh's `begin()`.
//!
//! `open_scan_view` clears the view BEFORE spawning the scan
//! (`crates/lattice-multibuffer/src/providers/agenda.rs:351`), so the first
//! open's stall is invisible (you keep the rows collected so far) while the
//! refresh's stall is total (the view is already empty).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_multibuffer::{HeaderlineStatus, MultibufferRegistryHandle};
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::parse_chord_sequence;

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

fn today() -> i64 {
    // LOCAL, like the guest: the plugin resolves today through
    // `local-utc-offset-seconds`, which the host implements as
    // `chrono::Local::now().offset()`. Dividing raw UTC seconds here was a
    // second, WRONG implementation of the thing under test — it agreed with
    // the guest only while the two happened to share a day, so these tests
    // passed all afternoon and failed after local midnight (GMT+5:30), which
    // is the same bug `today_epoch_day` had.
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let offset = i64::from(chrono::Local::now().offset().local_minus_utc());
    (utc + offset).div_euclid(86_400)
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

fn press(editor: &mut Editor, keys: &str) {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial = Vec::new();
    for c in seq {
        let _ = editor.dispatch_chord(c, &mut partial);
    }
}

/// An org file with one real dated row and `blocks` blocks of brace-heavy body
/// — the shape of the file that wedges the real corpus
/// (`roam/20250912175738-aicrete_bcmi.org`, 284 KB, 9737 lines).
fn big_org(blocks: usize, title: &str) -> String {
    let mut text = format!(
        "#+title: {title}\n* TODO {title}\n  SCHEDULED: {}\n",
        stamp(0)
    );
    for i in 0..blocks {
        text.push_str("     {\n\t \"key\": \"value\",\n\t \"n\": ");
        text.push_str(&i.to_string());
        text.push_str("\n     }\n");
    }
    text
}

// ─────────────────────────────────────────────────────────────────────────────
// DEFECT 1 — a guest `scan` call is not bounded by the budget the host arms.
// ─────────────────────────────────────────────────────────────────────────────

/// `AgendaActor::call_scan` calls `arm_store(&mut self.store, self.budget)`
/// with `PluginBudget::default()`, whose `epoch_deadline` is 1000 ticks
/// (`EPOCH_TICK_INTERVAL` ≈ 1ms, so ≈ 1 second) — see
/// `crates/lattice-plugin-host/src/lib.rs:216-223` and `:481-497`.
///
/// So one `scan` must either finish inside about a second or trap. A 17 KB
/// file does neither: it runs for ~6.5s and returns `Ok`. The budget is armed
/// and not enforced, which is what lets one file wedge the whole plugin.
///
/// Measured curve on this guest (debug host, release guest):
///   2 KB → 0.17s · 4 KB → 0.44s · 8.5 KB → 1.6s · 17 KB → 6.5s · 34 KB → 27.5s
/// — 4x the time for 2x the bytes, i.e. O(n²).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_single_guest_scan_stays_inside_its_epoch_budget() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
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
    source.begin(&[]).await.expect("begin");

    // 17 KB — smaller than plenty of ordinary org files, and an order of
    // magnitude smaller than the one in the real corpus.
    let text = big_org(400, "big");
    let path = base.path().join("big.org");
    std::fs::write(&path, &text).unwrap();

    let start = std::time::Instant::now();
    let outcome = source.scan(path, text.clone()).await;
    let elapsed = start.elapsed();

    // A trap is an acceptable outcome — that IS the budget working. What must
    // not happen is a call that quietly runs for many seconds.
    if outcome.is_err() {
        return;
    }
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "one `scan` of a {} KB file ran for {elapsed:?} without tripping the \
         armed epoch deadline (~1s). `AgendaActor` is single-consumer, so this \
         call blocks every later call on the plugin — including a refresh's \
         `begin()`.",
        text.len() / 1024
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// DEFECT 2 — a refresh cannot even START while a scan is in flight.
// ─────────────────────────────────────────────────────────────────────────────

/// **Why `gr` leaves the agenda blank**, asserted at the seam rather than by
/// racing a clock through the whole editor.
///
/// `gr` runs `open_scan_view`, which empties the view SYNCHRONOUSLY
/// (`crates/lattice-multibuffer/src/providers/agenda.rs:351`) and then spawns a
/// scan whose first act is `source.begin(&[]).await`
/// (`crates/lattice-multibuffer/src/providers/agenda.rs:474`).
///
/// `AgendaActor` serves one call at a time off one queue
/// (`crates/lattice-plugin-host/src/agenda_task.rs:197-217`), so that `begin()`
/// cannot run until the in-flight `scan` returns. Combined with defect 1 — a
/// `scan` that runs for seconds to minutes and is not stopped by its armed
/// budget — the view stays empty for the rest of the first walk.
///
/// Over the reporter's real corpus (754 files; one of them 284 KB, 138s in a
/// single `scan`) the first walk never finished at all: the headerline sat on
/// "Building agenda (576/754 files)" indefinitely, so `gr` blanked the view
/// permanently. That is the reported symptom.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_can_begin_while_a_scan_is_in_flight() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
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
    source.begin(&[]).await.expect("begin");

    // One ordinary 21 KB org file. The reporter's corpus has files 13x this.
    let text = big_org(500, "big");
    let path = base.path().join("big.org");
    std::fs::write(&path, &text).unwrap();

    // The first walk is inside `scan` on this file.
    let scanning = {
        let source = source.clone();
        let path = path.clone();
        let text = text.clone();
        tokio::spawn(async move { source.scan(path, text).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // The user presses `gr`. The view is ALREADY blank by this point; the only
    // thing that can refill it is this call returning.
    let start = std::time::Instant::now();
    let _ = source.begin(&[]).await;
    let waited = start.elapsed();
    scanning.abort();

    assert!(
        waited < std::time::Duration::from_secs(1),
        "a refresh's `begin()` waited {waited:?} behind ONE in-flight `scan`. \
         `open_scan_view` has already emptied the view by the time this runs, \
         so the agenda shows nothing for the whole of that wait — and over a \
         real corpus the wait is the rest of the walk."
    );
}

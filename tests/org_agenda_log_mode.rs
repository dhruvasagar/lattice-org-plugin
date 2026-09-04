//! OA.15b — `l` turns log mode on, and log rows appear.
//!
//! Design: `lattice/docs/dev/architecture/org-agenda.md` §3a–§3b. Slice plan:
//! `lattice/docs/dev/operations/slice-plans/org-agenda.md` (OA.15a/b).
//!
//! ## Why this file exists at all
//!
//! Every link in the chain is unit-tested somewhere and none of those tests
//! prove the chain. `l` → `Effect::ToggleMode` → the host activates the guest
//! minor → `minor-activated` crosses back → the guest's handler derives
//! `log=…` → `refresh-view` → the request is drained → the agenda re-scans →
//! log rows land as excerpts. Eight hops across two repos, and the failure at
//! any one of them looks identical from the outside: you press `l` and nothing
//! happens.
//!
//! It is also the first consumer of OA.15a's seam, so it is the test that
//! proves the seam does anything at all. `refresh_view_reaches_the_editor.rs`
//! proves the host half in isolation, with a spy opener rather than a plugin.
//!
//! ## The assertion that matters
//!
//! **No key is pressed after the toggle.** The rows have to arrive because the
//! refresh request woke the actor, which is what OA.15a's typed event is for.
//! A test that dispatched anything after `l` would pass on a build with no
//! wake at all, and the bug would surface in use as "it works, but only after
//! I hit something" — the exact class this repo keeps re-introducing.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_multibuffer::{HeaderlineStatus, MultibufferRegistryHandle};
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::parse_chord_sequence;

fn org_agenda_identity() -> lattice_multibuffer::providers::scan_view::ScanViewIdentity {
    lattice_multibuffer::providers::scan_view::ScanViewIdentity {
        provider: "agenda".to_string(),
        buffer_name: "*agenda*".to_string(),
        view_mode: None,
        no_rows_message: "no plugin provides agenda rows".to_string(),
    }
}

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

/// **`events` is in `provides` here and is NOT in `org_agenda.rs`'s copy.**
///
/// That is the difference this file turns on. Log mode's whole body lives in a
/// `minor-activated` handler, so without the events seam the plugin loads,
/// registers the mode, accepts the chord, activates — and nothing happens,
/// because the handler is never delivered to. It cost a debug cycle to find,
/// and the symptom (`l` does nothing) is identical to five other failures.
///
/// `theme` is deliberately NOT here, and its absence is not about log mode:
/// this harness wires no theme registry, so declaring the seam fails the whole
/// component (`org_agenda.rs`'s copy omits it for the same reason). The log
/// annotation's spans then name elements nothing registered, which the seam
/// contract already answers — an unknown slot renders in the row's own
/// foreground. The ROWS, which is what this file asserts on, are unaffected.
fn write_org_plugin_dir(root: &std::path::Path, wasm: &[u8]) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"scanned-excerpt-source\", \
         \"multibuffer-view-source\", \"modes\", \"grammar\", \"language\", \
         \"help\", \"config\", \"media\", \"events\"]\n\
         default_mode = \"org-todo-mode\"\n",
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

/// `YYYY-MM-DD Day` for `today() + offset` — the guest's own arithmetic,
/// restated (its copies are compiled for wasm).
fn ymd(offset: i64) -> String {
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
    format!("{y:04}-{m:02}-{d:02} {name}")
}

/// One open TODO (an ordinary agenda row) and one headline that was CLOSED and
/// CLOCKED today (a log row, and NOTHING to the plain agenda).
///
/// The DONE headline is the load-bearing half: `agenda.rs` refuses it by
/// construction — "an agenda that lists what you finished is a log, not a
/// plan" — so if it appears at all, log mode is what put it there.
fn write_corpus(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("notes.org"),
        format!(
            "* TODO Still to do\n  SCHEDULED: <{}>\n\
             * DONE Finished this one\n  CLOSED: [{} 14:32]\n\
             :LOGBOOK:\n\
             CLOCK: [{} 09:00]--[{} 10:30] =>  1:30\n\
             :END:\n",
            ymd(0),
            ymd(0),
            ymd(0),
            ymd(0),
        ),
    )
    .unwrap();
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

/// Press `keys` and APPLY what comes back.
///
/// `org_agenda.rs`'s `press` drops the returned `Action`, which is fine for
/// chords whose effects the host applies itself (a TODO cycle is `Edits`).
/// `Effect::ToggleMode` is not one of those: `handle_effect` puts it in the
/// no-op band deliberately, because toggling a mode is renderer-routed
/// (`lattice-ui-tui`'s effect tail:
/// `Effect::ToggleMode { mode_name } => self.toggle_mode_by_name(&mode_name)`).
/// A test that dropped it would press `l`, see nothing happen, and be unable to
/// tell that from a missing binding.
///
/// So this runs the real peel — chord -> `Action` -> `Editor::dispatch` — and
/// then stands in for the renderer's ONE line. The mode name is read out of the
/// effect rather than hardcoded, so a guest returning the wrong name still
/// fails here.
fn press(editor: &mut Editor, keys: &str) {
    use lattice_grammar::Effect;
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial = Vec::new();
    for c in seq {
        let action = editor.dispatch_chord(c, &mut partial);
        let out = editor.dispatch(action);
        for effect in out.effects {
            if let Effect::ToggleMode { mode_name } = effect {
                let _ = editor.toggle_mode_by_name(&mode_name);
            }
        }
    }
}

/// Every row's own source line, so an assertion can ask what the view SHOWS.
///
/// A row is an excerpt — a live range into a source document — which is the
/// whole reason log rows are excerpts rather than virtual rows, so reading
/// them back through the source is reading them the way the renderer does.
fn row_texts(mb: &MultibufferRegistryHandle, view: lattice_core::BufferId) -> Vec<String> {
    let handle = mb.handle(view).expect("the view is open");
    handle
        .excerpts()
        .iter()
        .filter_map(|e| {
            let text = handle.source_text(e.source)?;
            text.lines().nth(e.start_line as usize).map(str::to_string)
        })
        .collect()
}

/// Poll until the view's rows satisfy `want`, WITHOUT pressing anything.
///
/// The refresh lands through OA.15a's request → drain → re-scan chain, so the
/// tick has to be driven — but a tick is not a keystroke. `run_tick_pending`
/// is exactly what the actor's `async_landed` arm calls when the wake fires;
/// driving it here is standing in for that arm, not for a user.
async fn settle_rows(
    editor: &mut Editor,
    mb: &MultibufferRegistryHandle,
    view: lattice_core::BufferId,
    want: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    for _ in 0..400 {
        let _ = editor.run_tick_pending();
        let rows = row_texts(mb, view);
        if want(&rows) {
            return rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    row_texts(mb, view)
}

fn has_done_row(rows: &[String]) -> bool {
    rows.iter().any(|r| r.contains("Finished this one"))
}

/// The whole chain, end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pressing_l_shows_the_log_and_pressing_it_again_hides_it() {
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
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1,
        "the org component loads"
    );

    // The mode has to exist before anything can toggle it — a `ToggleMode`
    // naming nothing echoes an error the user sees and no test does.
    assert!(
        editor
            .mode_registry
            .load()
            .is_registered(lattice_mode::ModeId::new("org-agenda-log-mode")),
        "org must contribute `org-agenda-log-mode` at load"
    );

    let view = match open_org_agenda(
        &mut editor,
        &lattice_grammar::Args::String(notes.display().to_string()),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        other => panic!("the agenda declined: {other:?}"),
    };
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .expect("the multibuffer registry is a boot service");
    let status = settle_agenda(&mb, view).await;

    // --- Before: the plain agenda refuses the DONE headline.
    let before = row_texts(&mb, view);
    assert!(
        before.iter().any(|r| r.contains("Still to do")),
        "the open TODO is an ordinary agenda row: {before:?} ({status:?})"
    );
    assert!(
        !has_done_row(&before),
        "a DONE headline is not a plan — it must not be here before `l`: {before:?}"
    );

    // --- `l`. Nothing else is pressed from here on.
    editor.activate_buffer(view);
    press(&mut editor, "l");

    assert!(
        editor
            .active_modes
            .get(&view)
            .map(|m| m.is_active(lattice_mode::ModeId::new("org-agenda-log-mode")))
            .unwrap_or(false),
        "`l` must activate the mode on the agenda view"
    );

    let after = settle_rows(&mut editor, &mb, view, |rows| has_done_row(rows)).await;
    assert!(
        has_done_row(&after),
        "log mode must admit the closed headline — this is the whole chain: \
         chord → ToggleMode → activation → minor-activated → refresh-view → \
         drain → re-scan. Rows: {after:?}"
    );
    assert!(
        after.iter().any(|r| r.contains("Still to do")),
        "…without losing the plan it sits beside: {after:?}"
    );

    // --- `l` again. The mode is the switch, so off must mean off.
    press(&mut editor, "l");
    assert!(
        !editor
            .active_modes
            .get(&view)
            .map(|m| m.is_active(lattice_mode::ModeId::new("org-agenda-log-mode")))
            .unwrap_or(false),
        "the second `l` must deactivate the mode"
    );
    let back = settle_rows(&mut editor, &mb, view, |rows| !has_done_row(rows)).await;
    assert!(
        !has_done_row(&back),
        "turning the mode off must take the log rows with it — a toggle whose \
         `off` left the rows on screen is the two-states-that-disagree failure \
         the mode-as-switch shape exists to prevent. Rows: {back:?}"
    );
}

/// `:org-agenda-log-mode` is auto-generated for every registered mode, so it
/// reaches the SAME switch the chord does. A mode whose ex-command flipped it
/// without changing the rows would be the silent-command class.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ex_command_is_the_same_switch_as_the_chord() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);
    let notes = base.path().join("notes");
    write_corpus(&notes);

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base.path())
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1
    );
    let view = match open_org_agenda(
        &mut editor,
        &lattice_grammar::Args::String(notes.display().to_string()),
    ) {
        lattice_mode::ProviderViewOutcome::Opened { view, .. } => view,
        other => panic!("declined: {other:?}"),
    };
    let mb = editor
        .services
        .get::<MultibufferRegistryHandle>()
        .map(|h| (*h).clone())
        .unwrap();
    let _ = settle_agenda(&mb, view).await;
    editor.activate_buffer(view);

    let _ = editor.toggle_mode_by_name("org-agenda-log-mode");
    let rows = settle_rows(&mut editor, &mb, view, |rows| has_done_row(rows)).await;
    assert!(
        has_done_row(&rows),
        "`:org-agenda-log-mode` must reach the same body `l` does: {rows:?}"
    );
}

fn open_org_agenda(
    editor: &mut Editor,
    args: &lattice_grammar::Args,
) -> lattice_mode::ProviderViewOutcome {
    lattice_multibuffer::providers::scan_view::open_scan_view(editor, &org_agenda_identity(), args)
}

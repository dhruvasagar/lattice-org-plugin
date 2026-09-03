//! OM.A1 / OM.A2 — `:agenda` over real org files, produced by the real plugin.
//!
//! The exit criterion for both: a 3-file fixture produces a date-grouped
//! agenda in the right order. This walks the whole path — discover → load →
//! the `scanned-excerpt-source` seam drains → the host's agenda provider walks a
//! directory of `.org` files → the plugin's `scan` answers with dated rows →
//! the rows land as excerpts, interleaved ACROSS files by date, under one
//! header per day.
//!
//! Dates in the corpus are written relative to the day the test runs, so the
//! assertions are about ordering and grouping rather than about a fixed
//! calendar that would rot. A hard-coded `2026-08-25` would pass today and
//! read "overdue" forever after.
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
use lattice_protocol::parse_chord_sequence;
use lattice_runtime::document::Document as _;

/// Org's own view identity — the same strings its `MultibufferViewSpec`
/// declares to the host.
///
/// The host no longer carries these. A scan view is generic machinery and the
/// name is the plugin's, so org names its own; in production the plugin loader
/// builds this same identity from org's declaration. A test that opened the
/// view through a host-side `open_agenda` would be testing a constant that no
/// longer exists.
fn org_agenda_identity() -> lattice_multibuffer::providers::scan_view::ScanViewIdentity {
    lattice_multibuffer::providers::scan_view::ScanViewIdentity {
        provider: "agenda".to_string(),
        buffer_name: "*agenda*".to_string(),
        view_mode: None,
        no_rows_message: "no plugin provides agenda rows".to_string(),
    }
}

fn open_org_agenda(
    editor: &mut Editor,
    args: &lattice_grammar::Args,
) -> lattice_mode::ProviderViewOutcome {
    lattice_multibuffer::providers::scan_view::open_scan_view(editor, &org_agenda_identity(), args)
}

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

/// `provides` names `scanned-excerpt-source` alongside the rest, and again NOT in
/// dependency order — OM.0 made the loader sort, so manifest order is
/// cosmetic and this is where that keeps being true.
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

/// A loader wired to the booted editor's LIVE registries — including the
/// agenda registry the editor published at boot, so the producer the plugin
/// registers is the one the provider's scan reads.
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

/// Today, as the guest computes it — days since the epoch.
fn today() -> i64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 86_400) as i64
}

/// `<YYYY-MM-DD Day>` for `today() + offset`, in org's active-stamp syntax.
///
/// Hinnant's `civil_from_days` + Zeller, restated here rather than imported:
/// the plugin's copies are in a `cdylib` compiled for wasm, and a test that
/// reused them would be checking the guest's arithmetic against itself.
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

    // Zeller, shifted to 0 = Sunday. 1970-01-01 was a Thursday, so the
    // simpler `(days + 4) % 7` would also do — but this matches what the
    // guest computes, which is what the header must agree with.
    let (zm, zy) = if m < 3 { (m + 12, y - 1) } else { (m, y) };
    let (k, j) = (zy % 100, zy / 100);
    let h = (d + (13 * (zm + 1)) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let name = NAMES[(((h + 6) % 7) as usize) % 7];

    format!("<{y:04}-{m:02}-{d:02} {name}>")
}

/// Three org files whose dated rows deliberately do NOT sort in walk order,
/// plus a file nothing claims.
///
/// The interleave is the point: `home.org` holds both the earliest row and
/// the latest, so an agenda that merely concatenated each file's rows would
/// come out in a different order and the assertion below would catch it.
fn write_corpus(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("work.org"),
        format!(
            "#+TITLE: work\n\
             * TODO Ship the thing\n  SCHEDULED: {}\n\
             * DONE Already shipped\n  SCHEDULED: {}\n\
             * TODO No date on this one\nbody\n",
            stamp(1),
            stamp(1),
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("home.org"),
        format!(
            "* TODO Fix the tap\n  DEADLINE: {}\n\
             * Groceries\nmilk\n\
             * TODO Water the plants\n  SCHEDULED: {}\n",
            stamp(-2),
            stamp(3),
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("notes.org"),
        format!(
            "* Standup {}\n\
             * Old note [{}]\n",
            stamp(1),
            // Inactive: never a row, whatever its date.
            &stamp(1)[1..stamp(1).len() - 1],
        ),
    )
    .unwrap();
    // Claimed by nobody: the provider must never read it, let alone cross it.
    std::fs::write(
        dir.join("main.rs"),
        "* TODO Top\n  SCHEDULED: <2026-01-01 Thu>\n",
    )
    .unwrap();
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
        .get::<lattice_mode::ScannedExcerptSourceRegistryHandle>()
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
    let view = open_org_agenda(
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

    // Five rows, and the misses are as load-bearing as the hits:
    //   * `main.rs` contributes nothing — nothing claimed `.rs`, so it was
    //     never read (its `* TODO Top` would be here if it had been);
    //   * `DONE Already shipped` is filtered — an agenda listing what you
    //     finished is a log, not a plan;
    //   * `Old note [<date>]` is filtered — an INACTIVE stamp never dates a
    //     row, and with no keyword it is not a row at all;
    //   * `Groceries` is filtered — a plain undated headline is prose
    //     structure, not a task.
    //
    // AS.1 changed this count from four. `TODO No date on this one` used to
    // be filtered as "undated is not agenda-able"; it is the fifth row now,
    // under Unscheduled. That rule was the bug — an undated TODO is the most
    // ordinary line in an org file and it could not reach the agenda at all.
    assert_eq!(excerpts.len(), 5, "got {status:?}");

    let titles: Vec<String> = excerpts.iter().map(|e| e.header.title.clone()).collect();

    // --- AS.1: rows arrive in SECTION blocks, and a section is a contiguous
    // run because the guest packs its rank into the high digits of the
    // `sort_key` the host orders on. Nothing host-side knows sections exist.
    assert!(
        titles[0].starts_with("Overdue"),
        "the overdue block leads, got {titles:?}"
    );
    assert!(
        titles[0].contains("overdue by 2 day(s)"),
        "…and a date-grouping section still dates its header, got {titles:?}"
    );
    assert!(
        titles[1].starts_with("Agenda") && titles[1].contains("(tomorrow)"),
        "then the dated block, got {titles:?}"
    );
    assert!(
        titles[3].starts_with("Agenda") && titles[3].contains("in 3 day(s)"),
        "…still ordered by date within the block, got {titles:?}"
    );
    assert_eq!(
        titles[4], "Unscheduled",
        "and the undated TODO closes the view under its own block"
    );

    // --- The headline claim of OM.A2, UNCHANGED by sections: rows interleave
    // ACROSS files by date *within* a block.
    //
    // home.org holds both the earliest (deadline, 2 days ago) and the latest
    // (scheduled, in 3 days). If each file's rows were merely concatenated,
    // those two would be adjacent — this ordering is only reachable through
    // the cross-file sort on the guest's `sort_key`.
    assert_eq!(
        titles[2], "",
        "…whose SECOND row continues the group and renders no header — and it \
         came from a different FILE, which is the property a per-file grouping \
         could not express"
    );

    // Tomorrow's group is drawn from two different files, which is exactly
    // what "a date group spans files" means.
    let tomorrow_sources: std::collections::HashSet<_> =
        excerpts[1..3].iter().map(|e| e.source).collect();
    assert_eq!(
        tomorrow_sources.len(),
        2,
        "one date group, two source documents"
    );

    // OA.1: every row is one line, whether or not the entry has a planning
    // line under it. This used to assert the opposite — a scheduled row spanned
    // down to its `SCHEDULED:` — which made dated rows two lines tall and
    // showed the date twice, since the row is already grouped under it.
    for e in &excerpts {
        assert_eq!(
            e.start_line,
            e.end_line,
            "an agenda row is one line; got {:?}",
            excerpts
                .iter()
                .map(|e| (e.start_line, e.end_line))
                .collect::<Vec<_>>()
        );
    }

    match status {
        HeaderlineStatus::Complete { summary, .. } => {
            assert!(summary.contains("5 row(s)"), "got {summary}");
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
    match open_org_agenda(&mut editor, &lattice_grammar::Args::None) {
        lattice_mode::ProviderViewOutcome::Declined { message } => {
            assert!(
                message.contains("no plugin provides agenda rows"),
                "{message}"
            );
        }
        other => panic!("expected a Declined outcome, got {other:?}"),
    }
}

/// Dispatch a multi-key sequence written with `<leader>`, expanding it the same
/// way the binding did — so the test types what the user types.
fn press(editor: &mut Editor, keys: &str) {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial = Vec::new();
    for c in seq {
        let _ = editor.dispatch_chord(c, &mut partial);
    }
}

/// OM.A3's exit criterion, and the claim §6.1 said decided the whole design:
/// **changing a TODO state in the agenda writes the source file.**
///
/// An agenda you can only read is a lesser feature wearing the name. This is
/// the assertion that says it is not one.
///
/// Three things have to be true at once and each has failed independently
/// during this slice:
///   1. `org-agenda-mode` is activated on the view — a MANUAL policy the host
///      drives off the plugin's `view-mode` export, because no policy can say
///      "the buffer the agenda provider just built";
///   2. its chord resolves in a buffer whose major is `multibuffer-mode`;
///   3. the edit is translated from composed to source coordinates and lands
///      in the file the row came from, which is the multibuffer substrate's
///      job and the reason an agenda row is an excerpt rather than a string.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_a_todo_state_in_the_agenda_writes_the_source_document() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let file = notes.join("only.org");
    std::fs::write(
        &file,
        format!("* TODO Ship the thing\n  SCHEDULED: {}\n", stamp(0)),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1);
    let view = match open_org_agenda(
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
    settle_agenda(&mb, view).await;

    let handle = mb.handle(view).unwrap();
    assert_eq!(handle.excerpts().len(), 1, "one dated row");

    // The plugin's mode is on the view, activated by the host off the
    // `view-mode` export. Without this the chord below resolves to nothing
    // and the test would fail on the text assertion with no clue why.
    assert!(
        editor
            .active_modes
            .get(&view)
            .is_some_and(|m| m.has_minor(lattice_mode::ModeId::new("org-agenda-mode"))),
        "the host activated the SOURCE's mode on the view it built"
    );

    // Type it the way the user does, in the agenda, on the row's headline.
    let _ = editor.activate_buffer(view);
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
    press(&mut editor, "<leader>ot");
    editor.run_tick_pending();

    // The edit landed in the SOURCE document — the file the row came from,
    // not the composed view.
    let source = handle.excerpts()[0].source;
    let source_text = handle
        .source_text(source)
        .expect("the row's source document is still attached");
    assert!(
        source_text.starts_with("* DONE Ship the thing"),
        "the TODO state changed in the source file, got {source_text:?}"
    );
    assert!(
        source_text.contains("SCHEDULED:"),
        "…and only the keyword changed"
    );
}

/// OT.3: a headline inside a `#+BEGIN_SRC` block is not an agenda row.
///
/// This is the divergence the tree migration actually buys, and it took three
/// wrong guesses to find — recorded here so the next reader does not repeat
/// them. A `:PROPERTIES:` drawer between a headline and its `SCHEDULED:` line
/// is NOT a counterexample (org's grammar puts `plan` before `property_drawer`,
/// so the planning line genuinely must come first), and `DEADLINE:` /
/// `SCHEDULED:` on separate lines is NOT one either (org's planning info is a
/// single line).
///
/// What the text scan cannot do is know it is inside a block. It matches
/// `* TODO ` at the start of any line, so example org inside a source block
/// becomes a phantom agenda row that no amount of care in the line matcher can
/// remove — the information simply is not on the line. The grammar has it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_headline_inside_a_source_block_is_not_a_row() {
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
        notes.join("blocks.org"),
        format!(
            "* TODO Real task\n  SCHEDULED: {}\n\
             #+BEGIN_SRC org\n\
             * TODO Fake task inside a block\n  SCHEDULED: {}\n\
             #+END_SRC\n",
            stamp(1),
            stamp(1),
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    let view = open_org_agenda(
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

    // ONE row. `agenda::scan_file`, the text fallback, returns TWO for this
    // same corpus — pinned as a unit test in `src/agenda.rs`, so the pair
    // documents the difference rather than just asserting the good half.
    assert_eq!(
        excerpts.len(),
        1,
        "the block's example headline must not become a row; got {status:?} rows={:?}",
        excerpts
            .iter()
            .map(|e| (e.start_line, e.end_line))
            .collect::<Vec<_>>()
    );
    assert_eq!(excerpts[0].start_line, 0, "the real task is the row");
    // OA.1: one line. The planning line is parsed for its date and not
    // composed into the view.
    assert_eq!(excerpts[0].end_line, 0, "and its excerpt is one line");
}

/// OT.5: two planning entries on one line, and which of them dates the row is
/// decided by org's precedence, not by which was typed first.
///
/// `plan` is `repeat1(entry)` and an `entry` is `name?: entry_name, ':',
/// timestamp: timestamp`, so `SCHEDULED: <…> DEADLINE: <…>` is two nodes and
/// the tree has no opinion about their order. The text path reads the line with
/// `line.trim_start().strip_prefix("DEADLINE:")`, which requires the keyword to
/// come FIRST — so it reports the SCHEDULED date here and files the entry five
/// days late.
///
/// Observed through the sort rather than by reading a date off a row: the view
/// orders by day, so "which date won" is "which headline came out first".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deadline_outranks_a_scheduled_written_before_it() {
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
        notes.join("plan.org"),
        format!(
            // SCHEDULED first on the line, DEADLINE second. The deadline is
            // sooner, so a correct agenda puts this entry ahead of the one
            // below it.
            "* TODO Ship it\n  SCHEDULED: {} DEADLINE: {}\n\
             * TODO Something else\n  SCHEDULED: {}\n",
            stamp(5),
            stamp(1),
            stamp(3),
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    let view = open_org_agenda(
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

    assert_eq!(excerpts.len(), 2, "both entries are rows: {status:?}");
    assert_eq!(
        excerpts[0].start_line,
        0,
        "`Ship it` is dated by its DEADLINE (+1 day), so it sorts first; \
         reading the line's FIRST keyword instead dates it +5 and swaps these \
         two. rows={:?}",
        excerpts
            .iter()
            .map(|e| (e.start_line, e.end_line))
            .collect::<Vec<_>>()
    );
    assert_eq!(excerpts[1].start_line, 2);
}

/// AF.3 — `org.agenda-files` decides which files the agenda scans.
///
/// The editor opens with NO argument, so without the option the scan would use
/// the project root (the tempdir's cwd-derived root) and find nothing useful.
/// The option names a directory AND a single file outside it, which is the
/// shape every real org config has — Dhruva's `org-agenda-files` is
/// `(list org-directory)` plus an `anniversaries.org` picked out by name.
///
/// Asserts on which SOURCES the excerpts came from, not just the count: a
/// count would pass if both rows came from the directory and the loose file
/// were silently ignored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agenda_files_option_decides_what_is_scanned() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);

    // Two configured places…
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("work.org"),
        format!("* TODO from the directory\n  SCHEDULED: {}\n", stamp(1)),
    )
    .unwrap();
    let loose = base.path().join("anniversaries.org");
    std::fs::write(
        &loose,
        format!("* TODO from the named file\n  SCHEDULED: {}\n", stamp(2)),
    )
    .unwrap();

    // …and one that is NOT configured, to prove the option is a decision and
    // not merely an addition to whatever the walk would have found anyway.
    let unlisted = base.path().join("unlisted");
    std::fs::create_dir_all(&unlisted).unwrap();
    std::fs::write(
        unlisted.join("ignore-me.org"),
        format!("* TODO must not appear\n  SCHEDULED: {}\n", stamp(1)),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    editor.handle_effect(lattice_grammar::Effect::SetOption {
        // TC.7: the option is a `list<string>` now, so a `:set` spec is
        // separated (comma or newline) rather than line-formatted. The `#`
        // comment this fixture used to carry is gone with the format that
        // needed it — a comment belongs beside the TOML array in
        // `lattice.toml`, where it is a comment rather than something the
        // option has to parse around.
        spec: format!("org.agenda-files={},{}", notes.display(), loose.display()),
    });

    // No argument: the roots must come from the option, through `roots()`.
    let view = open_org_agenda(&mut editor, &lattice_grammar::Args::None);
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

    let mut files: Vec<String> = excerpts
        .iter()
        .filter_map(|e| handle.source_path(e.source))
        .map(|p| p.display().to_string())
        .collect();
    files.sort();
    files.dedup();

    assert_eq!(
        files.len(),
        2,
        "one row from the configured directory and one from the configured \
         file; got {status:?} files={files:?}"
    );
    assert!(
        files.iter().any(|f| f.ends_with("work.org")),
        "the directory root was walked: {files:?}"
    );
    assert!(
        files.iter().any(|f| f.ends_with("anniversaries.org")),
        "the file root was taken as given: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.contains("unlisted")),
        "an unconfigured directory must not be scanned: {files:?}"
    );
}

/// **AS.2 — `org.agenda-sections` replaces the built-in blocks, and reaches
/// the guest through the ordinary option seam.**
///
/// The whole of the "TOML *and* `init.rs`" answer is that there is one option
/// and no new seam. This test drives it the way `lattice.toml` would (a
/// `SetOption` effect carrying the same string); `init.rs` reaches the same
/// option through `config::set_option`, which is already how a user sets
/// `auto-pair.style`. Asserting through the REAL component matters here for
/// the reason `agenda-files` does: a set that parses in a unit test but never
/// crosses the ABI is a set nobody's agenda uses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agenda_sections_option_replaces_the_built_in_blocks() {
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
        notes.join("work.org"),
        format!(
            "* TODO Overdue thing\n  SCHEDULED: {}\n\
             * TODO Soon thing\n  SCHEDULED: {}\n\
             * TODO Undated thing\nbody\n",
            stamp(-3),
            stamp(1),
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    // A set that is deliberately NOT the built-in one: two blocks, renamed,
    // in the opposite order (undated first). Neither the titles nor the
    // ordering could come from the defaults, so a pass proves the option was
    // read rather than ignored.
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: "org.agenda-sections=[[section]]\n\
               title = \"Inbox\"\n\
               when = \"undated\"\n\
               todo-only = true\n\
               \n\
               [[section]]\n\
               title = \"Late\"\n\
               when = \"overdue\"\n\
               todo-only = true\n"
            .to_string(),
    });

    let view = open_org_agenda(
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
    let titles: Vec<String> = excerpts.iter().map(|e| e.header.title.clone()).collect();

    // Two rows, not three: `Soon thing` is inside no configured block, and a
    // row nothing takes is a row that does not appear. That is the option
    // being a DECISION rather than an addition to the built-ins.
    assert_eq!(excerpts.len(), 2, "got {status:?} titles={titles:?}");
    assert_eq!(
        titles[0], "Inbox",
        "the user's first block leads, under the user's own title"
    );
    assert!(
        titles[1].starts_with("Late"),
        "…then the second, got {titles:?}"
    );
    // The order is the CONFIG's, not the built-ins' — undated is last in the
    // default set and first here.
    assert!(
        !titles.iter().any(|t| t.contains("Unscheduled")),
        "no built-in block survived, got {titles:?}"
    );
}

/// A malformed set falls back to the built-ins and SAYS SO in the first
/// header.
///
/// The guest cannot log — calling `logging::log` makes the component import
//// **TC.6 — a value that does not fit the declared shape is refused when it is
/// SET, not when the agenda is drawn.**
///
/// This test used to assert the opposite, and the change is the point of the
/// slice. `org.agenda-sections` was a string holding TOML: any text at all was
/// a legal value, so a malformed set was stored, and the breakage surfaced a
/// scan later as a complaint prefixed onto the first section header. That was
/// the best available answer while the host had no idea what the string meant.
///
/// It now declares a `list<record>` schema, so the host validates on WRITE.
/// `:set org.agenda-sections=<garbage>` fails at the command line, where the
/// user is looking, and the option keeps the value it had. The agenda then
/// draws its ordinary default set with nothing to complain about — because
/// nothing about the *stored* configuration is wrong.
///
/// The fallback machinery is not gone and is still worth having: `read` turns a
/// host refusal into `SectionError::Malformed` carrying its path, and
/// `agenda_sections`' unit tests pin what the view does with one. What changed
/// is that reaching it now requires a schema and a `from_value` that disagree,
/// which would be a bug rather than a user's typo.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_section_set_is_refused_when_it_is_set() {
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
        notes.join("work.org"),
        format!("* TODO Late thing\n  SCHEDULED: {}\n", stamp(-3)),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: "org.agenda-sections=[[section]]\ntitle = ".to_string(),
    });

    let view = open_org_agenda(
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
    let titles: Vec<String> = excerpts.iter().map(|e| e.header.title.clone()).collect();

    // The row is still there — a rejected `:set` costs you nothing at all now,
    // not even your layout.
    assert_eq!(excerpts.len(), 1, "got {status:?} titles={titles:?}");
    assert!(
        titles[0].starts_with("Overdue"),
        "the built-in block, with no complaint prefixed: the stored \
         configuration is not broken, the rejected write never landed. Got \
         {titles:?}"
    );
}

/// **Do the agenda's rows carry syntax spans at all?**
///
/// Written to find out rather than to pin a belief: the machinery is all
/// present on paper — the provider passes `lang_registry` to
/// `create_multibuffer_view`, `add_source` detects `.org` through the plugin
/// extension registry, and the cells worker reads `excerpt_syntax` — so this
/// asserts the chain end to end before anything is changed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_rows_carry_per_excerpt_syntax_handles() {
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
        notes.join("work.org"),
        format!("* TODO Ship the thing\n  SCHEDULED: {}\n", stamp(1)),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    let view = open_org_agenda(
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

    // The chain, reported in one line on failure so a regression names the
    // broken link instead of prompting another round of guessing. The
    // BOOT-registered registry is included deliberately: it is bundled-only
    // and answers `false` for org, which is the bug AH.1 fixed — reading it
    // here documents that the fix is "ask the live registry", not "the
    // service was empty".
    let detected = lattice_syntax::Lang::detect_from_path(Some(std::path::Path::new("x.org")));
    let boot_registry_knows_org = editor
        .services
        .get::<std::sync::Arc<lattice_syntax::LangRegistry>>()
        .and_then(|lr| {
            lattice_syntax::Syntax::for_language_with_registry(detected, (*lr).clone())
                .ok()
                .map(|o| o.is_some())
        });
    let entries = handle.excerpt_syntax_entries();
    assert!(
        !entries.is_empty(),
        "the agenda's excerpts have no syntax handles, so every row paints \
         uncoloured. detect_from_path(.org) = {detected:?}; the boot-registered \
         registry knows org = {boot_registry_knows_org:?} (false is EXPECTED — \
         `add_source` must consult the live registry); sources = {}; got {status:?}",
        handle.excerpts().len(),
    );

    // A handle is not enough — it must actually resolve spans for the row's
    // source lines. A handle whose grammar never loaded answers `None` and is
    // indistinguishable from no handle at the pixel.
    let (_, _, source_start, ref h) = entries[0];
    // Through the inherent `SyntaxHandle` API rather than the
    // `ExcerptHighlighter` trait, so this test needs no extra dependency to
    // ask a question about spans.
    let snap = h.snapshot();
    let spans: Option<Vec<Vec<lattice_syntax::StyledSpan>>> =
        snap.highlight_lines(source_start, source_start + 1).ok();
    assert!(
        spans
            .as_ref()
            .is_some_and(|s| s.iter().any(|l| !l.is_empty())),
        "the handle resolved no spans for the headline row: {spans:?}"
    );
}

/// **AF.1: the agenda folds by SECTION and DATE — what the user sees — not by
/// source file.**
///
/// Folding by file is the multibuffer default and it is wrong here, because
/// the agenda's headline property is that rows interleave across files by date
/// (OM.A2). A file's fold spans its first row to its last, so collapsing
/// `home.org` — which holds both the earliest and the latest entry in this
/// corpus — would swallow every `work.org` and `notes.org` row in between.
///
/// The provider declares `FoldGrouping::HeaderRuns` at view creation, so the
/// mode registers header-run folds instead. Asserted through the real
/// component and the real fold pipeline, because the declaration is only worth
/// anything if it survives to the folds the editor actually holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agenda_folds_by_header_group_not_by_source_file() {
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
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    let view = open_org_agenda(
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

    // The declaration reached the view.
    assert_eq!(
        handle.fold_grouping(),
        lattice_multibuffer::FoldGrouping::HeaderRuns,
        "the agenda provider must declare header-run folding"
    );

    // And the folds it produces are the groups. One per header run, and each
    // stops before the next begins — the property file folds cannot give an
    // interleaved view.
    let folds = lattice_core::FoldSource::compute_folds(
        &lattice_multibuffer::HeaderGroupFoldProvider::new((*handle).clone(), view),
    );
    let excerpts = handle.excerpts();
    let headers: Vec<&str> = excerpts
        .iter()
        .map(|e| e.header.title.as_str())
        .filter(|t| !t.is_empty())
        .collect();
    assert_eq!(
        folds.len(),
        headers.len(),
        "one fold per header run; headers={headers:?} folds={folds:?} {status:?}"
    );
    assert!(
        folds.len() >= 3,
        "the corpus spans several blocks: {headers:?}"
    );
    for pair in folds.windows(2) {
        assert!(
            pair[0].end_line < pair[1].start_line,
            "groups must not overlap — this is what folding by file got wrong: {folds:?}"
        );
    }
    // Every composed ROW is inside exactly one group, so nothing is
    // unfoldable. Rows, not excerpts: a scheduled entry's excerpt spans its
    // planning line too, so the two counts differ by design (§6.2).
    let total_rows: u32 = excerpts.iter().map(|e| e.line_count()).sum();
    let covered: u32 = folds.iter().map(|f| f.end_line - f.start_line + 1).sum();
    assert_eq!(
        covered, total_rows,
        "every agenda row belongs to a group: {folds:?}"
    );
}

/// **AF.2 reversed: the agenda buffer opens EXPANDED.**
///
/// It opened collapsed, on the reasoning that a view whose entire structure is
/// blocks should show its blocks. That reasoning was about the view; the
/// agenda's job is task tracking, planning and scheduling, and for those the
/// ROWS are the content — a plan you have to expand four folds to read is a
/// plan you do not read. Emacs's agenda opens with every entry visible for the
/// same reason.
///
/// The test is kept rather than deleted, and the second half is why: it was
/// always about SCOPING more than about folding. The agenda resolves 99 from
/// its own mode, and the user's global setting is untouched — so a future
/// change that reached for `multibuffer-mode` instead would still be caught,
/// because project search and project diff share that major and must keep
/// their own answer.
///
/// Also worth pinning because a MINOR mode's option override is a path little
/// else in this plugin exercises: `org-mode`'s overrides ride a major.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agenda_opens_expanded_to_its_rows() {
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
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    // The precondition that makes this mean anything.
    assert_eq!(
        *editor
            .config
            .get_typed::<lattice_config::core_options::FoldLevel>()
            .expect("registered"),
        99,
        "sanity: the global default. `org-agenda-mode` declares 99 too, so \
         this test cannot tell them apart on the number alone — which is why \
         the SCOPING assertion below is the load-bearing one"
    );

    let view = open_org_agenda(
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
    editor.run_tick_pending();

    assert_eq!(
        *editor.resolved_option::<lattice_config::core_options::FoldLevel>(view),
        99,
        "the agenda opens expanded, showing its rows; got {status:?}"
    );
    assert_eq!(
        *editor
            .config
            .get_typed::<lattice_config::core_options::FoldLevel>()
            .expect("registered"),
        99,
        "and the global setting is untouched — the answer comes from \
         `org-agenda-mode`, not from the multibuffer major that project \
         search and project diff also use"
    );
}

/// OA.1: an agenda row is ONE line.
///
/// `end_line` used to run down to the planning line so the row showed
/// `SCHEDULED: <…>` under its headline. That made every dated row two lines
/// tall in a view whose job is to be scannable, and showed the date twice —
/// the row is already GROUPED under it.
///
/// Asserted on the composed view, not on `Row`: what matters is how many lines
/// the user sees, and the excerpt→composed step is where a spanning row would
/// still show up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agenda_row_is_one_line_even_when_the_entry_has_a_planning_line() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    // Two dated entries, each with its date on a planning line beneath the
    // headline — the shape that used to compose four rows for two entries.
    std::fs::write(
        notes.join("plan.org"),
        format!(
            "* TODO Ship it\n  SCHEDULED: {}\n* TODO Reply to Ana\n  DEADLINE: {}\n",
            stamp(1),
            stamp(2),
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    let view = open_org_agenda(
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

    assert_eq!(excerpts.len(), 2, "two entries, two rows: {status:?}");
    for e in &excerpts {
        assert_eq!(
            e.start_line,
            e.end_line,
            "an agenda excerpt spans one line; got {:?}",
            excerpts
                .iter()
                .map(|e| (e.start_line, e.end_line))
                .collect::<Vec<_>>()
        );
    }

    // The composed view is what the user reads: two headlines, and no
    // `SCHEDULED:` / `DEADLINE:` line among them.
    let composed = handle.snapshot().buffer.as_string();
    let lines: Vec<&str> = composed
        .lines()
        .filter(|l: &&str| !l.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 2, "composed: {composed:?}");
    assert!(
        !composed.contains("SCHEDULED:") && !composed.contains("DEADLINE:"),
        "the planning line must not be composed into the agenda: {composed:?}"
    );
}

/// OA.4: `<Tab>` cycles the block under the cursor in the agenda.
///
/// `org-cycle` is bound on the `org-mode` MAJOR and the agenda's major is
/// `multibuffer-mode`, so it never fired here — only the core `z` chords
/// worked. `<Tab>` also has a global default (jump-list forward), which in a
/// read-only agenda would move you out of the view you are reading, so the
/// binding has to actually shadow it rather than fall through.
///
/// The agenda now opens EXPANDED (AF.2 reversed — the rows are the content of
/// a planning view), so `<Tab>` here CLOSES a block rather than opening one.
/// Which direction it moves is not what this test is about; that it moves
/// exactly one block, the one under the cursor, is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tab_cycles_a_block_in_the_agenda() {
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
        notes.join("plan.org"),
        format!(
            // TWO overdue dates, two entries each: two blocks of two rows.
            // Multi-row so folding actually hides something — a one-row group
            // is not foldable at all (OA.4c) and would prove nothing — and
            // two of them so "only the cursor's block moved" is assertable.
            "* TODO Older one\n  SCHEDULED: {}\n\
             * TODO Older two\n  SCHEDULED: {}\n\
             * TODO Newer one\n  SCHEDULED: {}\n\
             * TODO Newer two\n  SCHEDULED: {}\n",
            stamp(-3),
            stamp(-3),
            stamp(-2),
            stamp(-2),
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    let view = open_org_agenda(
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
    // PRODUCTION ORDER, and it is the whole point of this test: the provider
    // activates the view when it CREATES it, which is before the scan lands.
    // Activating after `settle_agenda` instead would let `activate_buffer`'s
    // seed compute folds over an already-populated view — which is exactly
    // what hid this bug the first time.
    let _ = editor.activate_buffer(view);
    // Drain the mode-activation cascade WHILE THE VIEW IS STILL EMPTY, which
    // is what production does: `org-agenda-mode` activates on the next tick
    // and the file scan lands long after. Ticking only after the scan let a
    // re-activation seed the folds over an already-populated view, which is
    // why the first two attempts at this test passed on the broken code.
    editor.run_tick_pending();
    let status = settle_agenda(&mb, view).await;
    editor.run_tick_pending();

    editor.cursor.line = 0;
    editor.cursor.byte = 0;

    // OA.4d: the folds must exist from the async landing alone. The view is
    // activated while the scan is still running, so `activate_buffer`'s seed
    // computes folds over an EMPTY view; nothing typed into it afterwards, so
    // the edit path never recomputes either. Reported as "there are no folds
    // when org agenda is created, if I use redraw! only then the folds appear
    // and after that <Tab> works".
    //
    // Asserted BEFORE any further tick so a fix that merely recomputes on the
    // next keystroke does not pass: the rows arrived without one, and so must
    // their folds.
    assert!(
        !editor.folds.is_empty(),
        "the agenda's folds must follow its async population, with no redraw \
         and no keypress; got {status:?}"
    );

    // The jump list is what a fallen-through `<Tab>` would walk. Recording it
    // is how this test tells "cycled a fold" from "did the global thing".
    let jumps_before = editor.position_history.len();

    let snapshot = |ed: &Editor| -> Vec<(u32, u32, bool)> {
        ed.folds
            .iter()
            .map(|f| (f.start_line, f.end_line, f.closed))
            .collect()
    };
    let before = snapshot(&editor);
    assert!(before.len() > 1, "several blocks to cycle; got {status:?}");
    assert!(
        before.iter().all(|f| !f.2),
        "the agenda opens expanded, so every block starts open: {before:?}"
    );

    press(&mut editor, "<Tab>");
    editor.run_tick_pending();
    let after = snapshot(&editor);

    // The BLOCK AT THE CURSOR, and only it. `CycleFoldAtCursor` falls back to
    // a global cycle when no fold contains the cursor, so asserting merely
    // "some fold changed" passes on that fallback — which is `<S-Tab>`'s job,
    // not `<Tab>`'s, and is exactly the confusion this test exists to catch.
    let changed: Vec<usize> = before
        .iter()
        .zip(&after)
        .enumerate()
        .filter(|(_, (b, a))| b.2 != a.2)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        changed,
        vec![0],
        "`<Tab>` must open ONLY the block under the cursor (line 0). \
         before={before:?} after={after:?}"
    );
    assert_eq!(
        editor.position_history.len(),
        jumps_before,
        "`<Tab>` must not fall through to the global jump-list-forward"
    );
}

/// OA.6: an agenda row is coloured by ORG's semantics, not by the source
/// file's grammar.
///
/// Which word is a TODO keyword depends on `org.todo-keywords`, a runtime
/// option; a priority cookie and a tag list are headline structure the pinned
/// grammar does not model. So a row painted from the file's tree-sitter parse
/// showed none of them, and the agenda read as org text that happened to be
/// out of order.
///
/// Asserted on the EditorS extra-highlights — the end of the pipe — because
/// every step between the guest and there is a place the spans could be
/// dropped: validation, the cache, the composed-row translation, the drain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_rows_carry_org_semantic_colour() {
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
        notes.join("plan.org"),
        format!("* TODO [#A] Ship it :work:\n  SCHEDULED: {}\n", stamp(-1)),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    let view = open_org_agenda(
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
    let _ = editor.activate_buffer(view);
    let status = settle_agenda(&mb, view).await;
    editor.run_tick_pending();

    let spans: Vec<Vec<lattice_syntax::StyledSpan>> = editor
        .buffer_locals
        .get(&view)
        .and_then(|l| l.get::<lattice_host::modes::ExtraHighlights>())
        .map(|e| e.0.clone())
        .unwrap_or_default();
    let first = spans.first().cloned().unwrap_or_default();
    assert!(
        !first.is_empty(),
        "the row must carry org's own spans; got {spans:?}, status {status:?}"
    );

    // The keyword, the priority cookie and the tag list — the three things
    // the grammar cannot tell the host about.
    let composed = mb.handle(view).unwrap().snapshot().buffer.as_string();
    let line = composed.lines().next().unwrap_or_default();
    let covered: Vec<&str> = first
        .iter()
        .map(|s| &line[s.start.min(line.len())..s.end.min(line.len())])
        .collect();
    assert_eq!(covered, vec!["TODO", "[#A]", ":work:"], "line was {line:?}");
}

/// OA.10: a section's `match` filters rows by org's tags/todo syntax.
///
/// The end of phase 3's chain, and the reason each earlier link exists: the
/// row must carry inherited tags (OA.8), the match must survive being written
/// in TOML (OA.9), and the section must evaluate it.
///
/// Uses an INHERITED exclusion, because that is the shape a real config
/// relies on: you cancel a project by tagging the project, and its tasks are
/// expected to leave the agenda untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_section_match_filters_by_tags_including_inherited_ones() {
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
        notes.join("plan.org"),
        "* Live project\n\
         ** TODO Keep me\n\
         * Dead project :CANCELLED:\n\
         ** TODO Drop me\n\
         * TODO Also keep me\n",
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    // One block, undated TODOs, excluding anything under a cancelled parent.
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: "org.agenda-sections=[[section]]\n\
               title = \"Open\"\n\
               when = \"undated\"\n\
               todo-only = true\n\
               match = \"-CANCELLED/!\"\n"
            .to_string(),
    });

    let view = open_org_agenda(
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
    let composed = handle.snapshot().buffer.as_string();

    assert!(
        composed.contains("Keep me") && composed.contains("Also keep me"),
        "got {composed:?}, status {status:?}"
    );
    assert!(
        !composed.contains("Drop me"),
        "`Drop me` inherits :CANCELLED: from its parent and must be excluded; \
         got {composed:?}"
    );
}

/// **OA.11 — a named agenda's sections are what its scan runs.**
///
/// The end-to-end assertion for the whole of phase 4's mechanism: the view is
/// opened with a command KEY in the scan-arg slot OA.11a added, the host routes
/// it to `begin` without reading it, and org resolves it against
/// `org.agenda-custom-commands` into a different section set.
///
/// Driven through the REAL component rather than the unit tests, for AS.2's
/// reason and one sharper: the unit tests prove the parser, and the parser was
/// never the doubtful part. What is doubtful is whether a key typed into a menu
/// survives the boundary, the actor and the generation key to change which rows
/// a scan produces. A set that parses in a unit test but never crosses the ABI
/// is a set nobody's agenda uses.
///
/// The command's `match` uses a tag, so this also pins that a custom command's
/// sections went through the SAME validation as `org.agenda-sections`' — they
/// share `RawSection` precisely so they cannot drift.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_custom_command_supplies_the_sections_its_scan_runs() {
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
        notes.join("work.org"),
        "* WAITING Blocked on legal                                        :WAITING:\n\
         * TODO Ordinary task\n\
         * TODO Another ordinary task\n",
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: "org.todo-keywords=TODO WAITING | DONE".to_string(),
    });
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: "org.agenda-custom-commands=[[command]]\n\
               key = \"w\"\n\
               description = \"Waiting\"\n\
               \n\
               [[command.section]]\n\
               title = \"Blocked\"\n\
               when = \"any\"\n\
               match = \"WAITING\"\n"
            .to_string(),
    });

    // Position 0 is the root the HOST reads; position 1 is the command key,
    // which it does not. This is the two-slot split from OA.11a, exercised the
    // way the dispatcher will use it.
    let view = open_org_agenda(
        &mut editor,
        &lattice_grammar::Args::List(vec![
            lattice_grammar::args::ArgValue::String(notes.display().to_string()),
            lattice_grammar::args::ArgValue::String("w".to_string()),
        ]),
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
    let titles: Vec<String> = excerpts.iter().map(|e| e.header.title.clone()).collect();

    // ONE row: the command's single section takes only the `:WAITING:` entry,
    // and the two ordinary TODOs are inside no configured block. The default
    // agenda would have shown all three, so the count alone separates "the
    // command ran" from "the command was ignored".
    assert_eq!(
        excerpts.len(),
        1,
        "only the command's section admits a row; got {status:?} titles={titles:?}"
    );
    assert_eq!(
        titles[0], "Blocked",
        "under the COMMAND's own section title, not a default one"
    );
    assert!(
        !titles.iter().any(|t| t.contains("Unscheduled")),
        "no built-in block survived, got {titles:?}"
    );
}

/// **OA.11 — a key that names nothing falls back, and says which keys exist.**
///
/// Distinct from a malformed set, and the notice says so: the configuration
/// parsed, so what the user has is a stale binding or a typo in the key. Naming
/// the keys that DO exist is the shortest path from the symptom to the fix, and
/// it is information only this code has.
///
/// The fallback matters more than the message. Three separate ways of failing
/// to find a command — unset option, broken TOML, unknown key — all have to
/// land on a working agenda, because an empty agenda and a correct-but-empty
/// agenda look identical and "you have no tasks" is the worst thing this view
/// can say incorrectly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_command_key_falls_back_and_names_the_ones_that_exist() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    // An OVERDUE row, deliberately. The notice rides the FIRST section's title
    // and a section with no rows renders no header at all, so a fixture with
    // nothing overdue would show the fallback working and the notice missing —
    // a real (inherited) limitation, recorded in `agenda_custom_commands`'
    // header, and not the thing this test is for.
    std::fs::write(
        notes.join("work.org"),
        format!(
            "* TODO Late thing\n  SCHEDULED: {}\n* TODO Undated thing\n",
            stamp(-3)
        ),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: "org.agenda-custom-commands=[[command]]\n\
               key = \"w\"\n\
               \n\
               [[command.section]]\n\
               title = \"Blocked\"\n\
               when = \"any\"\n"
            .to_string(),
    });

    let view = open_org_agenda(
        &mut editor,
        &lattice_grammar::Args::List(vec![
            lattice_grammar::args::ArgValue::String(notes.display().to_string()),
            lattice_grammar::args::ArgValue::String("nope".to_string()),
        ]),
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
    let titles: Vec<String> = excerpts.iter().map(|e| e.header.title.clone()).collect();

    assert!(
        !excerpts.is_empty(),
        "the agenda still works — a bad key costs your layout, never your rows; \
         got {status:?}"
    );
    assert!(
        titles[0].contains("no command `nope`"),
        "the first header names what went wrong, got {titles:?}"
    );
    assert!(
        titles[0].contains("have: w"),
        "…and the keys that do exist, got {titles:?}"
    );
}

// HB.2b — completing a habit FROM THE AGENDA is not wired yet, and there is
// deliberately no test here asserting the current behaviour.
//
// An agenda excerpt is ONE line, the headline, so the multi-line rewrite finds
// no planning line in the composed view and falls back to the plain DONE. The
// fix is to read and write the SOURCE through the OA.23b seam rather than the
// view — a real slice, not a tweak. A test pinning today's answer would
// enshrine the destructive behaviour as intended; the org-file case is covered
// in `org_structure.rs`.

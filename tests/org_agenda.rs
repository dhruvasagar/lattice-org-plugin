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
        "id = \"org\"\nprovides = [\"scanned-excerpt-source\", \"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\"]\ndefault_mode = \"org-todo-mode\"\n",
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

    // Four rows, and the misses are as load-bearing as the hits:
    //   * `main.rs` contributes nothing — nothing claimed `.rs`, so it was
    //     never read (its `* TODO Top` would be here if it had been);
    //   * `DONE Already shipped` is filtered — an agenda listing what you
    //     finished is a log, not a plan;
    //   * `TODO No date on this one` is filtered — undated is not agenda-able;
    //   * `Old note [<date>]` is filtered — an INACTIVE stamp is never a row.
    assert_eq!(excerpts.len(), 4, "got {status:?}");

    // --- The headline claim of OM.A2: rows interleave ACROSS files by date.
    //
    // home.org holds both the earliest (deadline, 2 days ago) and the latest
    // (scheduled, in 3 days). If each file's rows were merely concatenated,
    // those two would be adjacent — this ordering is only reachable through
    // the cross-file sort on the guest's `sort_key`.
    let titles: Vec<String> = excerpts.iter().map(|e| e.header.title.clone()).collect();
    assert!(
        titles[0].contains("overdue by 2 day(s)"),
        "the overdue deadline leads, got {titles:?}"
    );
    assert!(
        titles[1].contains("(tomorrow)"),
        "then tomorrow's group, got {titles:?}"
    );
    assert_eq!(
        titles[2], "",
        "…whose SECOND row continues the group and renders no header — and it \
         came from a different FILE, which is the property a per-file grouping \
         could not express"
    );
    assert!(
        titles[3].contains("in 3 day(s)"),
        "then the furthest-out group, got {titles:?}"
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

    // The overdue deadline's excerpt spans its planning line, so the row
    // shows the date rather than a bare title the user has to jump to read.
    assert_eq!(
        excerpts[0].end_line,
        excerpts[0].start_line + 1,
        "a scheduled/deadline row spans its planning line"
    );
    // …and a bare-timestamp row does not, because there is no planning line
    // under it to show.
    assert_eq!(excerpts[2].end_line, excerpts[2].start_line);

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
    assert_eq!(
        excerpts[0].end_line, 1,
        "and its excerpt spans down to its planning line"
    );
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
        spec: format!(
            "org.agenda-files={}\n# a comment the option must ignore\n{}",
            notes.display(),
            loose.display()
        ),
    });

    // No argument: the roots must come from the option, through `roots()`.
    let view = lattice_multibuffer::providers::agenda::open_agenda(
        &mut editor,
        &lattice_grammar::Args::None,
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

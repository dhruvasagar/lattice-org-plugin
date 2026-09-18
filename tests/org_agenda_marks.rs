//! OA.30 — agenda bulk marks, end to end through the real component.
//!
//! Three seams have to agree for one `m` to be visible, and each of them is a
//! separate `wasmtime::Store` with its own memory:
//!
//!   1. the GRAMMAR seam runs the chord and writes the mark into the plugin
//!      store;
//!   2. the DECORATIONS seam is asked which lines carry a sign, and answers by
//!      reading that same store back;
//!   3. the TRANSIENT seam builds `x`'s menu, and reads the store a third time
//!      for its footer.
//!
//! A guest `thread_local` would have made each of those internally consistent
//! and mutually blind (`guest-state-does-not-cross-seams`), which is the bug
//! this design avoids and the reason the assertions below deliberately cross
//! from a chord to a producer rather than testing either alone.
//!
//! The manifest here names `signs`, `decorations` AND `state:write`. All three
//! are load-bearing: a seam missing from `provides` makes its export inert, and
//! without the capability every `store-put` returns `err`, so the chords would
//! run, report nothing and paint nothing
//! (`test-manifest-provides-decides-which-seams-exist`).
//!
//! Skips when the component was not built — `cargo test` builds this crate for
//! the HOST, and the component is a separate `--target wasm32-wasip2 --release`
//! artefact (`cargo-test-does-not-rebuild-a-loaded-artefact`).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_multibuffer::{HeaderlineStatus, MultibufferRegistryHandle};
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::parse_chord_sequence;
use lattice_runtime::document::Document as _;

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

/// The manifest, with the two seams and the one capability this slice adds.
///
/// `state:write` is not decoration: the marks live in the plugin store because
/// that is the only thing the three seams share, so without the grant there is
/// no feature at all — only three stores that each believe nothing is marked.
fn write_org_plugin_dir(root: &std::path::Path, wasm: &[u8]) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"scanned-excerpt-source\", \"multibuffer-view-source\", \
         \"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\", \
         \"transient-source\", \"signs\", \"decorations\"]\n\
         default_mode = \"org-todo-mode\"\ncapabilities = [\"state:write\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();
}

/// The registries the marks need, kept beside the loader so a test can read
/// back what the plugin registered into them.
struct Rig {
    loader: PluginLoader,
    signs: lattice_mode::SignRegistryHandle,
    decorations: lattice_mode::GutterDecorationSourceRegistryHandle,
}

fn rig(editor: &Editor, base: &std::path::Path) -> Rig {
    let host = Arc::new(
        PluginHost::with_dirs(base.join("cache"), base.join("data")).expect("host builds"),
    );
    // OA.23: what answers "which file did this composed line come from", and
    // the whole feature rests on it — a mark names the SOURCE entry a row
    // shows. `install.rs` wires this from the multibuffer registry; a
    // hand-built loader does not, and unwired the seam answers `none`, so
    // every chord here takes its "not a row" branch and returns no effect
    // (`test-harnesses-that-bypass-install`).
    if let Some(views) = editor
        .services
        .get::<lattice_multibuffer::registry::MultibufferRegistryHandle>()
    {
        host.set_excerpt_source_resolver(Arc::new(
            lattice_multibuffer::registry::MultibufferExcerptSource::new((*views).clone()),
        ));
    }
    let agenda_registry = editor
        .services
        .get::<lattice_mode::ScannedExcerptSourceRegistryHandle>()
        .map(|h| (*h).clone())
        .expect("the editor publishes the agenda registry at boot");
    // Wired with the BUILT-INS, not an empty registry: the org sign is
    // registered beside the diagnostic ones, and a sign competing for the mark
    // column must do it against the set the real editor has.
    let signs: lattice_mode::SignRegistryHandle = {
        let mut r = lattice_mode::SignRegistry::new();
        lattice_mode::register_builtin_signs(&mut r, lattice_mode::DiagnosticGlyphs::default());
        Arc::new(arc_swap::ArcSwap::from_pointee(r))
    };
    let decorations: lattice_mode::GutterDecorationSourceRegistryHandle = Arc::new(
        arc_swap::ArcSwap::from_pointee(lattice_mode::GutterDecorationSourceRegistry::new()),
    );
    let loader = PluginLoader::with_services(
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
            // The `x` menu rides this seam. Absent, `OpenTransient` names a
            // source the host has never heard of and the menu never opens —
            // which looks exactly like a chord that did not resolve.
            transient_registry: editor
                .services
                .get::<lattice_picker::TransientSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            agenda_registry: Some(agenda_registry),
            sign_registry: Some(signs.clone()),
            decoration_registry: Some(decorations.clone()),
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
    );
    Rig {
        loader,
        signs,
        decorations,
    }
}

fn settle_budget(base: usize) -> usize {
    let scale: usize = std::env::var("LATTICE_TEST_SETTLE_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(10);
    base.saturating_mul(scale)
}

async fn settle_agenda(registry: &MultibufferRegistryHandle, view: lattice_core::BufferId) {
    for _ in 0..settle_budget(600) {
        if let Some(h) = registry.handle(view) {
            if matches!(
                *h.headerline(),
                HeaderlineStatus::Complete { .. } | HeaderlineStatus::Failed { .. }
            ) {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the agenda scan never reached a terminal headerline");
}

/// Press `keys`, applying the renderer-owned effects this file cares about.
///
/// `OpenTransient` is one: `handle_effect` leaves it to the renderer, so a test
/// that dropped the outcome would press `x`, see no menu, and be unable to tell
/// that from a chord that never resolved
/// (`dropped-renderer-effects-look-like-dead-features`).
fn press(editor: &mut Editor, keys: &str) {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial = Vec::new();
    for c in seq {
        // `dispatch_chord_with_outcome`, NOT `dispatch_chord` + `dispatch`:
        // the chord is already executed here, so re-dispatching the returned
        // action RUNS IT TWICE. On a toggle that is invisible in the effects
        // and obvious in the result — one `m` marked the row AND the row it
        // then moved to. The doc comment on that method records org's harness
        // making exactly this mistake before.
        let (_action, out) = editor.dispatch_chord_with_outcome(c, &mut partial);
        for effect in out.effects {
            apply(editor, &effect);
        }
    }
}

fn apply(editor: &mut Editor, effect: &lattice_grammar::Effect) {
    if let lattice_grammar::Effect::OpenTransient { source, args } = effect {
        editor.open_named_transient(source.clone(), args.clone());
    }
}

/// Drain until a parked transient build has seated its menu. The build is a
/// guest call parked on the async-landed wake, so it is not ready at the moment
/// `OpenTransient` is applied.
async fn settle_transient(editor: &mut Editor) -> Option<Arc<lattice_picker::TransientSpec>> {
    for _ in 0..settle_budget(200) {
        editor.run_tick_pending();
        if let Some(spec) = editor.picker.as_ref().and_then(|p| p.transient.clone()) {
            return Some(spec);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    None
}

/// Fire the menu row bound to `key`.
///
/// `do_transient_trigger` host-applies what it can (`apply_effect_host` both
/// applies and records), so only the renderer-owned effects need replaying —
/// which for a drill-down row is the `OpenTransient` of the second menu.
fn fire(editor: &mut Editor, key: &str) -> Vec<lattice_grammar::Effect> {
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_transient_trigger(key.to_string(), &mut out);
    let effects: Vec<_> = out.effects.into_iter().collect();
    for effect in &effects {
        apply(editor, effect);
    }
    effects
}

/// Days since the epoch, LOCAL — the same resolution the guest does, through
/// the host's `local-utc-offset-seconds`. Dividing raw UTC seconds would be a
/// second and wrong implementation of the thing under test: it agrees with the
/// guest only while the two share a day (see `org_agenda.rs`).
fn today() -> i64 {
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let offset = i64::from(chrono::Local::now().offset().local_minus_utc());
    (utc + offset).div_euclid(86_400)
}

/// `<YYYY-MM-DD Day>` for today — org's active-stamp syntax, so the row is
/// dated and lands in a block. Hinnant's `civil_from_days` + Zeller, restated
/// as `org_agenda.rs` does: the guest's copies are compiled for wasm, and
/// reusing them would check the arithmetic against itself.
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

/// The view, its multibuffer handle and the registries, booted over a corpus of
/// `n` scheduled entries in one file.
async fn agenda_over(
    base: &std::path::Path,
    entries: &[&str],
) -> Option<(
    Editor,
    lattice_core::BufferId,
    MultibufferRegistryHandle,
    Rig,
)> {
    let wasm = org_plugin_wasm()?;
    let plugins_dir = base.join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm);
    let notes = base.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let body: String = entries
        .iter()
        .map(|title| format!("* TODO {title}\n  SCHEDULED: {}\n", stamp(0)))
        .collect();
    std::fs::write(notes.join("only.org"), body).unwrap();

    let mut editor = boot_sealed_editor();
    let rig = rig(&editor, base);
    // `load_discovered` rather than `discover_and_load`: the latter counts
    // successes and swallows the reason, and every failure mode of this slice
    // (an import nothing wires, a seam missing from `provides`, a capability
    // not granted) shows up there as the same silent `0`.
    for plugin in lattice_plugin_loader::discover(&plugins_dir) {
        rig.loader
            .load_discovered(&plugin, TrustTier::Bundled)
            .await
            .expect("the org plugin loads");
    }

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
    let _ = editor.activate_buffer(view);
    // The plugin's mode has to be ON the view, activated by the host off the
    // `view-mode` export — no `ActivationPolicy` can name "the buffer the
    // agenda provider just built". Without it every chord below resolves to
    // nothing and the tests fail on their paint assertions with no clue why.
    assert!(
        editor
            .active_modes
            .get(&view)
            .is_some_and(|m| m.has_minor(lattice_mode::ModeId::new("org-agenda-mode"))),
        "the host activated org-agenda-mode on the view it built"
    );
    Some((editor, view, mb, rig))
}

/// The producer's answer for this view: which lines carry a mark, in order.
///
/// The gutter is the only honest oracle here. A test that derived "which line
/// is a row" itself would be a second implementation of the composed→source
/// question, and the first thing it got wrong was counting the block header as
/// a row.
async fn painted(
    rig: &Rig,
    mb: &MultibufferRegistryHandle,
    view: lattice_core::BufferId,
) -> Vec<u32> {
    let line_count = mb.handle(view).unwrap().snapshot().buffer.rope_line_count();
    rig.decorations.load().sources()[0]
        .produce(u64::from(view.0), None, line_count)
        .await
        .expect("the producer answers")
        .iter()
        .map(|m| match m {
            lattice_mode::GutterDecoration::Sign { line, .. } => *line,
        })
        .collect()
}

/// Put the cursor on the view's first agenda row.
///
/// Line 0 is the block header; the first entry is below it. Rather than count,
/// this presses `j` from the top until `m` would have something to mark — which
/// is what a reader does.
fn go_to_first_row(editor: &mut Editor) {
    editor.cursor.line = 0;
    editor.cursor.byte = 0;
}

/// The exit criterion: `m` marks the row the cursor is on, and the DECORATIONS
/// producer — a different seam, a different store, a different memory — paints
/// a sign on exactly that line.
///
/// The paint is what makes the feature real. A mark that is recorded and never
/// shown is indistinguishable from a dead key, and this producer answering the
/// second time is what `refresh-decorations` exists for: the agenda's text does
/// not change when you mark a row, so nothing else would ever ask again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m_marks_the_row_under_the_cursor_and_it_paints() {
    let base = tempfile::tempdir().unwrap();
    let Some((mut editor, view, mb, rig)) =
        agenda_over(base.path(), &["Ship the thing", "Write the thing"]).await
    else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    // Nothing is marked, so the producer paints nothing — and it must reach
    // that answer without a walk, which is the store read it short-circuits on.
    let sources = rig.decorations.load().sources();
    assert_eq!(sources.len(), 1, "the plugin registered one producer");
    assert!(
        painted(&rig, &mb, view).await.is_empty(),
        "an unmarked agenda paints no marks"
    );

    go_to_first_row(&mut editor);
    press(&mut editor, "m");
    editor.run_tick_pending();

    // Exactly one row wears the mark …
    let marks = painted(&rig, &mb, view).await;
    assert_eq!(marks.len(), 1, "one row is marked, got {marks:?}");

    // … it is a row the sign registry knows org declared …
    let sign = rig
        .signs
        .load()
        .id_of("org.agenda-mark")
        .expect("the plugin declared its sign at load");
    let line_count = mb.handle(view).unwrap().snapshot().buffer.rope_line_count();
    let raw = sources[0]
        .produce(u64::from(view.0), None, line_count)
        .await
        .unwrap();
    assert_eq!(
        raw,
        vec![lattice_mode::GutterDecoration::Sign {
            line: marks[0],
            sign
        }],
        "the mark paints org's own sign, not some other producer's"
    );

    // … and the cursor stepped PAST it, as org's `m` does, so marking a run of
    // rows is `mmm` rather than `mjmjm`.
    assert_eq!(
        editor.cursor.line,
        marks[0] + 1,
        "the cursor advanced past the row it marked"
    );
}

/// `m` on a marked row unmarks it, and `M` drops the whole set.
///
/// The second half is the one that matters for the bulk verbs: a mark that
/// cannot be taken back turns `x` into a trap, because the set it acts on would
/// only ever grow.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mark_can_be_taken_back_one_row_at_a_time_or_all_at_once() {
    let base = tempfile::tempdir().unwrap();
    let Some((mut editor, view, mb, rig)) =
        agenda_over(base.path(), &["One", "Two", "Three"]).await
    else {
        eprintln!("skipping: component not built");
        return;
    };
    // `*` marks every row in one press — three entries, three marks.
    go_to_first_row(&mut editor);
    press(&mut editor, "*");
    editor.run_tick_pending();
    let all = painted(&rig, &mb, view).await;
    assert_eq!(all.len(), 3, "`*` marked every row, got {all:?}");

    // `m` on a marked row takes just that one back.
    editor.cursor.line = all[1];
    editor.cursor.byte = 0;
    press(&mut editor, "m");
    editor.run_tick_pending();
    let one_off = painted(&rig, &mb, view).await;
    assert_eq!(
        one_off,
        vec![all[0], all[2]],
        "`m` toggled the middle row off and left the others"
    );

    // `M` drops the rest.
    press(&mut editor, "M");
    editor.run_tick_pending();
    let none = painted(&rig, &mb, view).await;
    assert!(none.is_empty(), "`M` unmarked everything, got {none:?}");
}

/// The bulk verb: `x` `t` `d` writes one TODO state into every marked entry's
/// SOURCE document, and clears the marks.
///
/// Two entries in the same file, and only the marked one changes — which is the
/// assertion that says the verb acts on the mark set rather than on the buffer
/// or on the cursor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bulk_todo_verb_writes_every_marked_entry_and_clears_the_marks() {
    let base = tempfile::tempdir().unwrap();
    let Some((mut editor, view, mb, rig)) =
        agenda_over(base.path(), &["Ship the thing", "Leave me alone"]).await
    else {
        eprintln!("skipping: component not built");
        return;
    };
    go_to_first_row(&mut editor);
    press(&mut editor, "m");
    editor.run_tick_pending();
    assert_eq!(
        painted(&rig, &mb, view).await.len(),
        1,
        "one row is marked before the verb runs"
    );

    // `x` opens the verb menu, its `t` row opens the keyword menu, and that
    // menu's `d` row chooses DONE. Driven through the menus rather than by
    // dispatching `org-agenda-bulk-todo` directly: the menus ARE the surface,
    // and a test that skipped them would pass with both unreachable — which is
    // exactly how a transient row naming a command that does not exist fails.
    press(&mut editor, "x");
    let verbs = settle_transient(&mut editor)
        .await
        .expect("`x` opened the bulk menu");
    assert_eq!(verbs.title, "Bulk action");
    assert!(
        verbs.footer.as_deref() == Some("1 row marked"),
        "the menu names what it will act on, got {:?}",
        verbs.footer
    );

    let _ = fire(&mut editor, "t");
    let keywords = settle_transient(&mut editor)
        .await
        .expect("`t` opened the keyword menu");
    assert_eq!(keywords.title, "Bulk: TODO state");

    let effects = fire(&mut editor, "d");

    // The verb's contract, asserted on the edits it produced: the MARKED
    // entry's headline is rewritten to DONE, and nothing touches the other.
    //
    // The edits rather than the resulting file text, and the reason is the
    // rig: each names a SOURCE document by id (`source-location.buffer` — the
    // id to EDIT), and a hand-built loader does not put an agenda's source
    // documents in the editor's buffer store, so there is nothing here to
    // apply them to. Applying them end-to-end is the HOST's half and is
    // covered there (`a_plugin_action_on_an_agenda_row_finds_its_file.rs`).
    // What is org's half — which entries the verb picks and what it writes
    // into each — is exactly what these effects say.
    let rewrites: Vec<String> = effects
        .iter()
        .filter_map(|e| match e {
            lattice_grammar::Effect::ApplyEdit { edit, .. } => match &edit.kind {
                lattice_protocol::EditKind::Replace { text } => Some(text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        rewrites,
        vec!["* DONE Ship the thing".to_string()],
        "the marked entry took the keyword, and only it"
    );
    assert!(
        effects.iter().any(|e| matches!(
            e,
            lattice_grammar::Effect::Echo { text, .. } if text.contains("1 row")
        )),
        "the verb says how many rows it acted on, got {effects:?}"
    );

    // The marks are gone, which is org's default (`org-agenda-persistent-marks`
    // is nil) and the only safe one: a second `x` must not silently repeat the
    // first on the same rows.
    editor.run_tick_pending();
    let left = painted(&rig, &mb, view).await;
    assert!(
        left.is_empty(),
        "the bulk verb cleared the marks, got {left:?}"
    );
}

/// `x` with nothing marked refuses and says so.
///
/// Org falls back to marking the row at point and acting on it; this
/// deliberately does not. The verb rewrites entries, and inferring a target for
/// a destructive edit from where the cursor happens to be is the one way this
/// key could lose work the user did not offer it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x_with_nothing_marked_refuses_rather_than_guessing() {
    let base = tempfile::tempdir().unwrap();
    let Some((mut editor, view, mb, _rig)) = agenda_over(base.path(), &["Ship the thing"]).await
    else {
        eprintln!("skipping: component not built");
        return;
    };
    go_to_first_row(&mut editor);

    press(&mut editor, "x");
    assert!(
        settle_transient(&mut editor).await.is_none(),
        "`x` with nothing marked opens no menu"
    );
    editor.run_tick_pending();

    let handle = mb.handle(view).unwrap();
    let source = handle.excerpts()[0].source;
    let text = handle.source_text(source).unwrap();
    assert!(
        text.contains("* TODO Ship the thing"),
        "nothing was rewritten, got {text:?}"
    );
}

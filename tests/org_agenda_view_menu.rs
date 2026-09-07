//! OA.18 — `gD` opens the view dispatch, and its rows reach live actions.
//!
//! Design: `lattice/docs/dev/architecture/org-agenda.md` §4. Slice plan:
//! `lattice/docs/dev/operations/slice-plans/org-agenda.md` (OA.18).
//!
//! ## What can go wrong here, and why a unit test would not see it
//!
//! A transient row names a command by STRING, and nothing at build time
//! checks that the string resolves — `agenda_view_menu()` is a pure function
//! returning a spec, and it returns a perfectly well-formed one whether or not
//! `org-agenda-day-view` exists. What happens to a row that names nothing is
//! worse than an error: `transient_source.rs` **drops it**, with a `debug!`
//! nobody reads. The menu opens one row shorter and the key you press does
//! nothing, which is indistinguishable from a missing binding — the class this
//! repo keeps paying for (OC.3's dispatch fallback; OA.15b's `l`).
//!
//! So the first test enumerates every key the menu must offer, which is what
//! catches a dropped row, and the rest FIRE rows and look at what changed. The
//! clock-report row is the one most likely to rot: its command is a host
//! constant this guest cannot depend on (it compiles to wasm), so the string
//! is written twice and only a test can keep the two copies equal.
//!
//! ## The keys are not routed here
//!
//! `do_transient_trigger` is called directly, as the sibling transient suites
//! do. Key→row routing lives in the host's `Action::PickerAppend` arm and is
//! covered there; a test that tried to press `d` into an open menu would be
//! testing the host's picker plumbing, not org's menu.

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

/// **`transient-source` is in `provides` and is NOT in `org_agenda.rs`'s
/// copy.** Without it the seam does not exist for this component, so `gD`
/// resolves, fires, returns `OpenTransient` — and the host answers "unknown
/// source". The symptom is a chord that does nothing, again.
///
/// `theme` is deliberately absent: this harness wires no theme registry and
/// declaring the seam fails the whole component (the scar OA.15b's file
/// records). `events` is absent because nothing here needs the log chain —
/// `org_agenda_log_mode.rs` owns that, and this file only asserts the `l` row
/// reaches the same switch.
fn write_org_plugin_dir(root: &std::path::Path, wasm: &[u8]) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"scanned-excerpt-source\", \
         \"multibuffer-view-source\", \"modes\", \"grammar\", \"language\", \
         \"help\", \"config\", \"media\", \"transient-source\"]\n\
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
            transient_registry: editor
                .services
                .get::<lattice_picker::TransientSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            agenda_registry: Some(agenda_registry),
            provider_view_registry: editor
                .services
                .get::<lattice_mode::ProviderViewRegistryHandle>()
                .map(|h| (*h).clone()),
            multibuffer_registry: editor
                .services
                .get::<MultibufferRegistryHandle>()
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

/// One row today and one three days out, so a day span and a week span differ.
/// Without the second row every span would render the same view and the
/// span-changing test would pass on a build where the row did nothing.
fn write_corpus(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("notes.org"),
        format!(
            "* TODO Today's thing\n  SCHEDULED: <{}>\n\
             * TODO Later this week\n  SCHEDULED: <{}>\n",
            ymd(0),
            ymd(3),
        ),
    )
    .unwrap();
}

fn open_org_agenda(
    editor: &mut Editor,
    args: &lattice_grammar::Args,
) -> lattice_mode::ProviderViewOutcome {
    lattice_multibuffer::providers::scan_view::open_scan_view(editor, &org_agenda_identity(), args)
}

async fn settle_agenda(
    registry: &MultibufferRegistryHandle,
    view: lattice_core::BufferId,
) -> HeaderlineStatus {
    for _ in 0..settle_budget(600) {
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

/// Drain until a parked transient build has seated its menu. The build is a
/// guest call parked on the async-landed wake, so it is not ready when
/// `OpenTransient` is applied.
async fn settle_transient(
    editor: &mut Editor,
) -> Option<std::sync::Arc<lattice_picker::TransientSpec>> {
    for _ in 0..settle_budget(200) {
        editor.run_tick_pending();
        if let Some(spec) = editor.picker.as_ref().and_then(|p| p.transient.clone()) {
            return Some(spec);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    None
}

/// Press `keys` and return the outcome, applying the renderer-owned effects
/// this file cares about.
///
/// `OpenTransient` is one of them: `handle_effect` leaves it to the renderer,
/// so a test that dropped the outcome would press `gD`, see no menu, and be
/// unable to tell that from a chord that never resolved.
fn press(editor: &mut Editor, keys: &str) -> Vec<lattice_grammar::Effect> {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial = Vec::new();
    let mut effects = Vec::new();
    for c in seq {
        let action = editor.dispatch_chord(c, &mut partial);
        let out = editor.dispatch(action);
        for effect in out.effects {
            apply(editor, &effect);
            effects.push(effect);
        }
    }
    effects
}

fn apply(editor: &mut Editor, effect: &lattice_grammar::Effect) {
    match effect {
        lattice_grammar::Effect::OpenTransient { source, args } => {
            editor.open_named_transient(source.clone(), args.clone());
        }
        // The TUI's effect tail, in one line: `ToggleMode` is renderer-routed.
        lattice_grammar::Effect::ToggleMode { mode_name } => {
            let _ = editor.toggle_mode_by_name(mode_name);
        }
        _ => {}
    }
}

/// Fire the row bound to `key` and return the effects it produced.
fn fire(editor: &mut Editor, key: &str) -> Vec<lattice_grammar::Effect> {
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_transient_trigger(key.to_string(), &mut out);
    let effects: Vec<_> = out.effects.into_iter().collect();
    for effect in &effects {
        apply(editor, effect);
    }
    effects
}

/// Open the agenda over a corpus, settle it, and leave it focused.
async fn agenda_editor(
    base: &std::path::Path,
) -> (Editor, lattice_core::BufferId, MultibufferRegistryHandle) {
    let plugins_dir = base.join("plugins");
    write_org_plugin_dir(
        &plugins_dir,
        &org_plugin_wasm().expect("callers check for the component first"),
    );
    let notes = base.join("notes");
    write_corpus(&notes);

    let mut editor = boot_sealed_editor();
    assert_eq!(
        loader_over_editor(&editor, base)
            .discover_and_load(&plugins_dir, TrustTier::Bundled)
            .await,
        1,
        "the org component loads"
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
    let _ = settle_agenda(&mb, view).await;
    editor.activate_buffer(view);
    (editor, view, mb)
}

fn menu_keys(spec: &lattice_picker::TransientSpec) -> Vec<String> {
    spec.groups
        .iter()
        .flat_map(|g| g.items.iter())
        .filter_map(|i| i.key.first().cloned())
        .collect()
}

/// Every row's command NAME, paired with its key.
///
/// The seam converts a row's command string to a `CommandId` when the menu
/// crosses (`transient_source.rs`), so the name has to be read back out of the
/// registry — which is also the point: an id in hand is proof the string
/// resolved to something registered.
fn menu_commands(editor: &Editor, spec: &lattice_picker::TransientSpec) -> Vec<(String, String)> {
    let registry = editor.registry.load();
    spec.groups
        .iter()
        .flat_map(|g| g.items.iter())
        .filter_map(|i| match &i.kind {
            lattice_picker::TransientItemKind::Action { command, .. } => Some((
                i.key.first().cloned().unwrap_or_default(),
                registry
                    .lookup(*command)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| format!("<unregistered {command:?}>")),
            )),
            _ => None,
        })
        .collect()
}

/// `gD` opens the dispatch, and **every row names a command that exists**.
///
/// The second half is the assertion that earns this file. A spec is a value:
/// it is well-formed whether or not its command strings resolve, so the only
/// thing standing between a typo and a menu of dead rows is this loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gd_opens_the_view_dispatch_and_every_row_names_a_live_command() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _view, _mb) = agenda_editor(base.path()).await;

    press(&mut editor, "gD");
    let spec = settle_transient(&mut editor).await.unwrap_or_else(|| {
        panic!(
            "`gD` must open the view dispatch (last message: {:?})",
            editor.last_message.as_ref().map(|m| &m.text)
        )
    });

    let keys = menu_keys(&spec);
    for want in ["d", "w", "m", "y", "l", "r", "q"] {
        assert!(
            keys.iter().any(|k| k == want),
            "emacs' view-dispatch letter `{want}` must be offered: {keys:?}"
        );
    }
    // No time-grid row: `org-agenda-time-grid-mode` is not built (OA.17), and
    // a row for it would be a menu entry that does nothing.
    assert!(
        !keys.iter().any(|k| k == "G"),
        "the time grid is not built, so it must not be advertised: {keys:?}"
    );

    // Every row survived the crossing, which is the assertion the key list
    // above already makes and is worth naming: `transient_source.rs` DROPS a
    // row whose command does not resolve, with a `debug!` nobody reads. A typo
    // in one of these six strings does not fail loudly — the row is simply not
    // in the menu, which is why the loop above enumerates all seven keys
    // rather than spot-checking one.
    let commands = menu_commands(&editor, &spec);
    assert_eq!(
        commands.len(),
        6,
        "six action rows plus `q`, all of them resolved: {commands:?}"
    );
    for (key, name) in &commands {
        assert!(
            !name.starts_with("<unregistered"),
            "row `{key}` crossed with an id that resolves to nothing: {name}"
        );
    }
}

/// The clock-report row names the HOST's toggle, character for character.
///
/// The guest compiles to wasm and cannot depend on `lattice-multibuffer`, so
/// the action id is written twice: once as a `const` there, once as a literal
/// in org. This is the only thing that keeps the copies equal — and if they
/// drift, the row goes silent rather than failing loudly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_clock_report_row_names_the_hosts_own_toggle() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _view, _mb) = agenda_editor(base.path()).await;

    press(&mut editor, "gD");
    let spec = settle_transient(&mut editor).await.expect("the menu opens");
    let commands = menu_commands(&editor, &spec);
    let (_, clock) = commands
        .iter()
        .find(|(k, _)| k == "r")
        .expect("the report row is offered");
    assert_eq!(
        clock,
        lattice_multibuffer::providers::clock_report::TOGGLE_ACTION,
        "org must name the host's clock-report toggle rather than declaring a \
         second one — the report is generic over any scan view (OA.16), and \
         two toggles for one switch is two states that can disagree"
    );
}

/// The span rows change the span, and the headerline says so.
///
/// **Inherited from `org_agenda.rs`'s `the_span_chords_change_the_span…`,
/// which OA.18 made impossible to keep there.** That test pressed `gDm` and
/// `gDd`; those chords are gone, and its own harness cannot open a menu (no
/// `transient-source` in its `provides`). The journey it protected is the same
/// one — the same keystrokes, now through the dispatch — so it moved rather
/// than being deleted.
///
/// **Its history is this slice's argument.** The span bindings originally
/// shipped as `vd` / `vw` / `vm` / `vy` and were unreachable, because `v` is
/// bound at `KeymapLayer::Builtin` and a trie node carrying both a terminal
/// binding and children resolves to the terminal. OA.20 moved them to `gD…`.
/// OA.18 hits the identical rule from the other side: `gD` cannot be a menu
/// *and* a prefix, so the four sub-chords had to go. Same rule, third
/// encounter — which is why this asserts the VIEW changed rather than that a
/// binding is registered.
///
/// Month **and then** day, so it proves a change rather than a constant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_span_rows_change_the_span_and_the_header_says_so() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, view, mb) = agenda_editor(base.path()).await;

    let summary = |s: &HeaderlineStatus| match s {
        HeaderlineStatus::Complete { summary, .. } => summary.clone(),
        other => panic!("the agenda did not complete: {other:?}"),
    };
    let first = summary(&settle_agenda(&mb, view).await);
    assert!(
        first.contains("Week ") || first.contains("Day "),
        "the header names the span it is showing: {first:?}"
    );

    /// Open the dispatch, pick `key`, and wait for the re-scan's header.
    ///
    /// The re-open is a fresh scan, so the settle is driven by ticks — the
    /// same thing the actor's `async_landed` arm does. No key is pressed to
    /// make it land.
    async fn pick(
        editor: &mut Editor,
        mb: &MultibufferRegistryHandle,
        view: lattice_core::BufferId,
        key: &str,
        want: &str,
    ) -> String {
        press(editor, "gD");
        let _ = settle_transient(editor).await.expect("the menu opens");
        fire(editor, key);
        let mut seen = String::new();
        for _ in 0..settle_budget(600) {
            editor.run_tick_pending();
            if let Some(h) = mb.handle(view) {
                if let HeaderlineStatus::Complete { summary, .. } = &*h.headerline() {
                    seen = summary.clone();
                    if seen.contains(want) {
                        break;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        seen
    }

    let month = pick(&mut editor, &mb, view, "m", "Month ").await;
    assert!(
        month.contains("Month "),
        "`gD m` switched the view to a month: {month:?} (was {first:?})"
    );

    let day = pick(&mut editor, &mb, view, "d", "Day ").await;
    assert!(
        day.contains("Day "),
        "`gD d` switched it to a day: {day:?} (was {month:?})"
    );
}

/// The `l` row is the same switch as the bare `l` — one `ToggleMode`, one mode.
///
/// A menu that flipped a second piece of state would drift from the chord and
/// both would look right in isolation, which is the disagreement the
/// mode-as-the-switch shape exists to prevent (OA.15b).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_log_row_flips_the_same_mode_the_bare_l_does() {
    if org_plugin_wasm().is_none() {
        eprintln!("skipping: component not built");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let (mut editor, view, _mb) = agenda_editor(base.path()).await;

    press(&mut editor, "gD");
    let _ = settle_transient(&mut editor).await.expect("the menu opens");
    let effects = fire(&mut editor, "l");

    let toggled: Vec<&String> = effects
        .iter()
        .filter_map(|e| match e {
            lattice_grammar::Effect::ToggleMode { mode_name } => Some(mode_name),
            _ => None,
        })
        .collect();
    assert_eq!(
        toggled,
        vec![&"org-agenda-log-mode".to_string()],
        "the row must return the one `ToggleMode` the chord does: {effects:?}"
    );
    assert!(
        editor
            .active_modes
            .get(&view)
            .map(|m| m.is_active(lattice_mode::ModeId::new("org-agenda-log-mode")))
            .unwrap_or(false),
        "…and it must reach the view the menu was opened over"
    );
}

/// Scale a settle loop's poll budget for machine load.
///
/// Every wait in this suite is `for _ in 0..N { if done { break } sleep(ms) }`,
/// which budgets ITERATIONS. That is fine on an idle machine and wrong under a
/// full `cargo test`: the work being waited on — a wasm instantiation, a guest
/// scan, an off-thread index — slows down with contention while the budget does
/// not stretch to match, so the loop gives up on work that was still coming.
///
/// Three suites flaked exactly this way in one session (`org_roam_index` twice,
/// on two different tests, and `org_highlight_from_component` once), each
/// passing cleanly in isolation. A red that is sometimes noise is a red that
/// gets argued with instead of obeyed, which is the real cost.
///
/// **A wider budget is close to free.** These loops exit the moment their
/// condition holds, so raising the ceiling costs nothing on the passing path;
/// it is only paid when something is genuinely broken, and waiting longer to
/// report a real failure is the cheaper mistake.
///
/// `LATTICE_TEST_SETTLE_SCALE` overrides the factor for a slower machine.
fn settle_budget(base: usize) -> usize {
    let scale: usize = std::env::var("LATTICE_TEST_SETTLE_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(10);
    base.saturating_mul(scale)
}

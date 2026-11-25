//! `org-export-mode`: the dispatcher, the gate, the target check, and an
//! export that lands without a further key.
//!
//! Boots a real editor, loads the built org component through the loader, and
//! drives export through its chords and commands, as a user would. Skips (with
//! the line CI greps for) when the component has not been built.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_mode::ModeId;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::{parse_chord_sequence, KeyChord};

fn component_cache() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("component-cache")
}

/// Boot an editor sealed off from the developer's real `~/.config/lattice`.
///
/// `Editor::boot` starts plugin auto-discovery on a spawned task, which scans
/// `~/.config/lattice/plugins/`. Once org is installed there for real use, that
/// task loads a SECOND copy of this plugin into the same registries the test
/// loads its own copy into — a duplicate-registration race that only makes
/// progress at an `.await`. See the longer note in `org_major_mode.rs`.
fn boot_sealed_editor() -> Editor {
    lattice_plugin_loader::disable_autoload();
    Editor::boot(CoreDocument::from_text("scratch\n"))
}

fn org_plugin_wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    );
    // The component embeds org's real manifest, and an embedded manifest wins
    // over a `plugin.toml`. These tests load org under manifests of their own
    // (a subset of seams, a grant on a temp dir), so take it out first.
    std::fs::read(path)
        .ok()
        .map(|bytes| lattice_plugin_host::manifest::without_embedded_manifest(&bytes))
}

fn loader_over_editor(editor: &Editor, base: &std::path::Path) -> PluginLoader {
    let host =
        Arc::new(PluginHost::with_dirs(component_cache(), base.join("data")).expect("host builds"));
    // What `install` wires and a hand-built loader does not. Each one
    // unwired fails a run silently in its own way: no buffer store, and every
    // buffer reads as closed; no active-modes query, and `buffer-text`
    // refuses; no sink, and a finished job's edit goes nowhere.
    if let Some(buffers) = editor.services.get::<lattice_mode::BufferStoreHandle>() {
        host.set_buffer_store((*buffers).clone());
    }
    // And no output store: `output-append` fails, and `*org-babel*` stays
    // empty however the run ended.
    if let Some(output) = editor
        .services
        .get::<lattice_plugin_host::output::PluginOutputHandle>()
    {
        host.set_plugin_output((*output).clone());
    }
    if let Some(query) = editor
        .services
        .get::<lattice_mode::ActiveModesQueryHandle>()
    {
        host.set_active_modes_query((*query).clone());
    }
    // The sink is an inbound bus, as in `install`: its send wakes the editor
    // (`async_landed`), and its drain hands the posted batch to the next tick.
    let (posted, drain) = lattice_mode::inbound::make_inbound::<lattice_grammar::Effect, _>(
        editor.async_landed.clone(),
        |effect| vec![effect],
    );
    let ticks = editor
        .services
        .get::<lattice_mode::TickCallbackRegistryHandle>()
        .expect("the tick registry is a boot service");
    // Kept for the test's life: dropping the registration unregisters it.
    std::mem::forget(ticks.register(drain));
    host.set_effect_sink(Arc::new(move |effect| {
        let _ = posted.send(effect);
    }));
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
            // The media seam fails the whole load when unwired, by
            // design — a plugin declaring `media` whose images can never be
            // drawn should say so rather than load silently half-working.
            media_registry: Some(std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                lattice_mode::MediaSourceRegistry::new(),
            ))),
            // Refile's target list is a picker source, so the loader
            // needs somewhere to register it.
            picker_registry: Some(editor.picker_registry.clone()),
            // Babel's running sign — the editor's own sign and decoration
            // registries, so a test asks the producer the renderer asks.
            sign_registry: editor
                .services
                .get::<lattice_mode::SignRegistryHandle>()
                .map(|h| (*h).clone()),
            decoration_registry: editor
                .services
                .get::<lattice_mode::GutterDecorationSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            // Org's menus (capture, export) register into the editor's OWN
            // transient registry, the one `editor_boot` builds, so a plugin menu
            // does not depend on whether magit happened to load.
            transient_registry: editor
                .services
                .get::<lattice_picker::TransientSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            // The agenda is a plugin-owned view now, so the loader
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

/// Boot an editor, load the org plugin into its live registries, and open
/// `base/notes.org` holding `text`. `capabilities` are the grants beyond
/// `state:write`; an export needs `fs:write` over where it writes.
///
/// The loader comes back too, and a test must hold it: its option
/// subscriptions (`org.export.enabled`'s gate among them) hold it weakly and
/// stop when it drops.
async fn org_editor(
    base: &std::path::Path,
    text: &str,
    capabilities: &[String],
) -> (Editor, Arc<PluginLoader>) {
    org_editor_holding(base, text, capabilities, Vec::new()).await
}

/// [`org_editor`] with `held` as the user's config file holds it: before the
/// plugin loads, since a process grant is resolved at load from what the
/// user wrote.
async fn org_editor_holding(
    base: &std::path::Path,
    text: &str,
    capabilities: &[String],
    held: Vec<(&str, lattice_config::ConfigValue)>,
) -> (Editor, Arc<PluginLoader>) {
    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    let caps: String = capabilities.iter().map(|c| format!(", \"{c}\"")).collect();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "id = \"org\"\nprovides = [\"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\", \"picker-source\", \"transient-source\", \"events\", \"signs\", \"decorations\"]\ndefault_modes = [\"org-todo-mode\", \"org-global-mode\", \"org-table-mode\", \"org-babel-mode\", \"org-export-mode\"]\nmode_gates = {{ \"org-babel-mode\" = \"org.babel.enabled\", \"org-export-mode\" = \"org.export.enabled\" }}\neditor_capabilities = [\"tree-sitter\"]\ncapabilities = [\"state:write\"{caps}]\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("component.wasm"),
        org_plugin_wasm().expect("caller checked"),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    for (name, value) in held {
        editor
            .config
            .hold_tree(name, value, lattice_config::HeldSource::ConfigFile);
    }
    let loader = Arc::new(loader_over_editor(&editor, base));
    loader.subscribe_mode_gates();
    let loaded = loader
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");
    // Enablement before the open, activation after it: both drains hang off
    // the tick, so tick on both sides (see `org_babel.rs`).
    editor.run_tick_pending();
    let file = base.join("notes.org");
    std::fs::write(&file, text).unwrap();
    editor.do_edit(Some(file), false);
    editor.run_tick_pending();
    editor.publish_render_state();
    assert!(
        export_active(&editor),
        "org-export-mode is on in an org buffer"
    );
    (editor, loader)
}

fn export_active(editor: &Editor) -> bool {
    editor
        .active_modes
        .get(&editor.document_buffer_id)
        .is_some_and(|m| m.is_active(ModeId::new("org-export-mode")))
}

/// The title of the transient showing, if any.
fn menu_title(editor: &Editor) -> Option<String> {
    editor
        .picker
        .as_ref()
        .and_then(|p| p.transient.as_ref())
        .map(|t| t.title.clone())
}

/// A grant over `dir`, as the manifest's option-bound one resolves to.
fn write_grant(dir: &std::path::Path) -> Vec<String> {
    vec![format!("fs:write:{}", dir.display())]
}

const DOC: &str = "#+title: Plan\n* One\nfirst <b>\n* Two\nsecond\n";

/// Press `keys`, applying `OpenTransient` as the renderer peer's effect tail
/// does (`handle_effect` leaves it to the peer). The effects come back.
fn press(editor: &mut Editor, keys: &str) -> Vec<lattice_grammar::Effect> {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial: Vec<KeyChord> = Vec::new();
    let mut effects = Vec::new();
    for c in seq {
        let out = editor.dispatch_chord_with_outcome(c, &mut partial).1;
        for effect in out.effects {
            if let lattice_grammar::Effect::OpenTransient { source, args } = &effect {
                editor.open_named_transient(source.clone(), args.clone());
            }
            effects.push(effect);
        }
    }
    editor.publish_render_state();
    effects
}

/// `C-c C-e`, then wait for the menu: the guest builds it on a call parked on
/// the async-landed wake, so it is not seated when `OpenTransient` is applied.
async fn open_dispatcher(editor: &mut Editor) -> Option<String> {
    press(editor, "<C-c><C-e>");
    for _ in 0..200 {
        editor.run_tick_pending();
        if let Some(title) = menu_title(editor) {
            return Some(title);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    None
}

/// Run an ex line; the effects come back as from [`press`].
fn ex(editor: &mut Editor, line: &str) -> Vec<lattice_grammar::Effect> {
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.execute_ex_line(line, &mut out);
    // A host-applied edit comes back as a next action, which the dispatch
    // wrapper drains — do what it does.
    for action in std::mem::take(&mut out.next_actions) {
        let _ = editor.dispatch(action);
    }
    editor.publish_render_state();
    out.effects
}

fn last_echo(editor: &Editor) -> Option<String> {
    editor.last_message.as_ref().map(|m| m.text.clone())
}

fn text(editor: &Editor) -> String {
    editor.document.snapshot().text().to_string()
}

fn set_org_option(editor: &mut Editor, name: &str, value: &str) {
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!("org.{name}={value}"),
    });
}

/// Wait until `done` holds, the way the actor does: on `async_landed`, then
/// a tick — never a key. Anything async that does not wake the editor fails
/// here, which is the bug class this exists to catch.
async fn settle(editor: &mut Editor, done: impl Fn(&Editor) -> bool) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    editor.run_tick_pending();
    while !done(editor) {
        let landed = editor.async_landed.clone();
        let woke = tokio::time::timeout_at(deadline, landed.notified()).await;
        assert!(
            woke.is_ok(),
            "did not settle; echo {:?}; the buffer is:\n{}",
            last_echo(editor),
            text(editor)
        );
        editor.run_tick_pending();
        editor.publish_render_state();
    }
}

macro_rules! require_component {
    () => {
        if org_plugin_wasm().is_none() {
            eprintln!(
                "skipping: component not built (cargo build --release --target wasm32-wasip2)"
            );
            return;
        }
    };
}
/// `C-c C-e` is export's, and only in org: a text buffer never sees the
/// layer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_dispatcher_opens_in_org_buffers_only() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor(base.path(), DOC, &[]).await;

    assert_eq!(
        open_dispatcher(&mut editor).await.as_deref(),
        Some("Export")
    );
    press(&mut editor, "q");
    assert_eq!(menu_title(&editor), None);

    let txt = base.path().join("notes.txt");
    std::fs::write(&txt, "plain\n").unwrap();
    editor.do_edit(Some(txt), false);
    editor.run_tick_pending();
    editor.publish_render_state();
    assert!(!export_active(&editor));
    assert_eq!(
        open_dispatcher(&mut editor).await,
        None,
        "no layer, no menu"
    );
}

/// Off, the layer goes and the commands say so rather than export; the rest
/// of org stays on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_off_removes_the_dispatcher_and_the_commands_effect() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;

    set_org_option(&mut editor, "export.enabled", "false");
    settle(&mut editor, |e| !export_active(e)).await;
    assert_eq!(
        open_dispatcher(&mut editor).await,
        None,
        "no layer, no menu"
    );
    ex(&mut editor, "org-export-html");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: export is off — set org.export.enabled to use it")
    );
    assert!(!base.path().join("notes.html").exists());
    assert!(editor
        .active_modes
        .get(&editor.document_buffer_id)
        .is_some_and(|m| m.is_active(ModeId::new("org-todo-mode"))));

    set_org_option(&mut editor, "export.enabled", "true");
    settle(&mut editor, export_active).await;
}

/// Where org may not write is said at once, naming the option that would
/// allow it, and nothing is exported.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_target_is_reported_before_anything_runs() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let grant = write_grant(elsewhere.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;

    ex(&mut editor, "org-export-html");
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(echo.contains("org.export.directories"), "{echo}");
    assert!(echo.contains("notes.html"), "{echo}");
    // Given every chance to have run anyway.
    for _ in 0..5 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        editor.run_tick_pending();
    }
    assert!(!base.path().join("notes.html").exists());
    assert_eq!(last_echo(&editor).as_deref(), Some(echo.as_str()));
}

/// The chord emits and returns; the export runs on the events seam and its
/// file lands with no key pressed after the row's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_lands_on_disk_without_a_further_keypress() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;
    let html = base.path().join("notes.html");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "hh");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("Exporting notes.org → notes.html…")
    );
    settle(&mut editor, |_| html.exists()).await;
    let written = std::fs::read_to_string(&html).unwrap();
    assert!(written.contains("<title>Plan</title>"), "{written}");
    assert!(written.contains("first &lt;b&gt;"), "{written}");
    // The transcoder's document, with the default stylesheet inline.
    assert!(
        written.contains("<h2 id=\"") && written.contains("<style>\n:root"),
        "{written}"
    );
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("Exported notes.html")
    })
    .await;
}

/// `l l` writes ox-latex's document beside the file: the preamble, a
/// labelled section a headline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_latex_export_writes_the_document() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;
    let tex = base.path().join("notes.tex");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "ll");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("Exported notes.tex")
    })
    .await;
    let written = std::fs::read_to_string(&tex).unwrap();
    assert!(
        written
            .starts_with("% Intended LaTeX compiler: pdflatex\n\\documentclass[11pt]{article}\n"),
        "{written}"
    );
    assert!(written.contains("\\title{Plan}\n"), "{written}");
    assert!(
        written.contains("\\section{One}\n\\label{") && written.contains("first <b>\n"),
        "{written}"
    );
    assert!(written.ends_with("\\end{document}\n"), "{written}");
}

/// The contents options are the export's defaults, and one set to a value
/// it does not take stops the export, naming it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_export_defaults_are_options() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;
    for spec in [
        "org.export.with-toc=false",
        "org.export.author=Ada Lovelace",
    ] {
        editor.handle_effect(lattice_grammar::Effect::SetOption {
            spec: spec.to_string(),
        });
    }
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "hh");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("Exported notes.html")
    })
    .await;
    let written = std::fs::read_to_string(base.path().join("notes.html")).unwrap();
    assert!(!written.contains("id=\"table-of-contents\""), "{written}");
    assert!(written.contains("Ada Lovelace"), "{written}");

    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: "org.export.with-tags=sometimes".to_string(),
    });
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "hh");
    settle(&mut editor, |e| {
        last_echo(e).is_some_and(|m| m.starts_with("org: org.export.with-tags is `sometimes`"))
    })
    .await;
}

/// The `org.export.latex-*` options reach the export: a compiler, a
/// source-block backend and a declared class, the file naming the class.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_latex_options_shape_the_export() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(
        base.path(),
        "#+LATEX_CLASS: memoir\n* One\n#+begin_src python\nx = 1\n#+end_src\n",
        &grant,
    )
    .await;
    for spec in [
        "org.export.latex-compiler=xelatex",
        "org.export.latex-src-block-backend=listings",
        "org.export.latex-classes=[[class]]\nname = \"memoir\"\nsections = [\"chapter\", \"section\"]\n",
    ] {
        editor.handle_effect(lattice_grammar::Effect::SetOption {
            spec: spec.to_string(),
        });
    }
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "ll");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("Exported notes.tex")
    })
    .await;
    let written = std::fs::read_to_string(base.path().join("notes.tex")).unwrap();
    assert!(
        written.starts_with(
            "% Intended LaTeX compiler: xelatex\n\\documentclass[11pt]{memoir}\n\\usepackage{fontspec}\n"
        ),
        "{written}"
    );
    assert!(written.contains("\\usepackage{listings}\n"), "{written}");
    assert!(
        written.contains("\\chapter{One}")
            && written.contains("\\begin{lstlisting}[language={Python}]\nx = 1\n\\end{lstlisting}"),
        "{written}"
    );
}

/// The manifest's process grant, which resolves from the option.
const PDF_GRANT: &str = "proc:spawn:{org.export.latex-pdf-process:command}";

/// `org.export.latex-pdf-process` of `steps`, each a command and its args.
fn pdf_process(steps: &[(&str, &[&str])]) -> lattice_config::ConfigValue {
    use lattice_config::ConfigValue;
    ConfigValue::List(
        steps
            .iter()
            .map(|(command, args)| {
                ConfigValue::Record(
                    [
                        ("command".to_string(), ConfigValue::Str(command.to_string())),
                        (
                            "args".to_string(),
                            ConfigValue::List(
                                args.iter()
                                    .map(|a| ConfigValue::Str(a.to_string()))
                                    .collect(),
                            ),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect(),
    )
}

/// A compiler that makes `%b.pdf` in `%o`, but only from a `.tex` whose
/// end is already on disk when it starts.
const FAKE_COMPILER: (&str, &[&str]) = (
    "sh",
    &[
        "-c",
        "grep -q 'end{document}' \"$1\" && printf '%%PDF-1.4\\n' > \"$2/$3.pdf\"",
        "fake",
        "%f",
        "%o",
        "%b",
    ],
);

async fn pdf_editor(
    base: &std::path::Path,
    steps: &[(&str, &[&str])],
) -> (Editor, Arc<PluginLoader>) {
    let mut grant = write_grant(base);
    grant.push(PDF_GRANT.to_string());
    org_editor_holding(
        base,
        DOC,
        &grant,
        vec![("org.export.latex-pdf-process", pdf_process(steps))],
    )
    .await
}

/// `l p` writes the `.tex`, complete, before the compiler starts, and the
/// compiler makes the PDF beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pdf_export_compiles_the_written_tex() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = pdf_editor(base.path(), &[FAKE_COMPILER]).await;
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "lp");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("Exported notes.pdf")
    })
    .await;
    let pdf = std::fs::read_to_string(base.path().join("notes.pdf")).unwrap();
    assert!(pdf.starts_with("%PDF-1.4"), "{pdf}");
    let tex = std::fs::read_to_string(base.path().join("notes.tex")).unwrap();
    assert!(tex.ends_with("\\end{document}\n"), "{tex}");
}

/// A failing step stops the list, and the echo names the log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_step_stops_the_pdf_process() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = pdf_editor(
        base.path(),
        &[("sh", &["-c", "exit 3"]), ("sh", &["-c", "touch never"])],
    )
    .await;
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "lp");
    settle(&mut editor, |e| {
        last_echo(e).is_some_and(|m| m.starts_with("org: notes.pdf was not made: `sh` failed"))
    })
    .await;
    assert!(
        last_echo(&editor).is_some_and(|m| m.ends_with(":org-export-log shows its output")),
        "{:?}",
        last_echo(&editor)
    );
    assert!(!base.path().join("never").exists());
}

/// The process runs only from a value the user's config file wrote: one
/// set with `:set` is not granted, and the echo says where it must be.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pdf_process_set_at_runtime_is_not_granted() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let mut grant = write_grant(base.path());
    grant.push(PDF_GRANT.to_string());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: "org.export.latex-pdf-process=[[step]]\ncommand = \"sh\"\nargs = [\"-c\", \"touch ran\"]\n"
            .to_string(),
    });
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "lp");
    settle(&mut editor, |e| {
        last_echo(e).is_some_and(|m| m.starts_with("org: could not start `sh`"))
    })
    .await;
    assert!(!base.path().join("ran").exists());
}

/// `l o` opens the PDF, and again opens the same buffer, not a second.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_the_pdf_again_reuses_its_buffer() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = pdf_editor(base.path(), &[FAKE_COMPILER]).await;
    let pdf = base.path().join("notes.pdf");
    let showing_pdf =
        |e: &Editor| buffers(e).path_for(e.document_buffer_id).as_deref() == Some(pdf.as_path());
    for _ in 0..2 {
        assert!(open_dispatcher(&mut editor).await.is_some() || showing_pdf(&editor));
        if showing_pdf(&editor) {
            // Back to the org file for the second export.
            editor.do_edit(Some(base.path().join("notes.org")), false);
            editor.run_tick_pending();
            assert!(open_dispatcher(&mut editor).await.is_some());
        }
        press(&mut editor, "lo");
        settle(&mut editor, |e| showing_pdf(e)).await;
    }
    let with_pdf = editor
        .buffers
        .listed_ids_sorted()
        .into_iter()
        .filter(|id| buffers(&editor).path_for(*id).as_deref() == Some(pdf.as_path()))
        .count();
    assert_eq!(with_pdf, 1);
}

/// `:org-export-cancel` stops a running compile, and the echo says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_stops_a_running_compile() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        pdf_editor(base.path(), &[("sh", &["-c", "sleep 30; touch late"])]).await;
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "lp");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("Compiling notes.pdf…")
    })
    .await;
    ex(&mut editor, "org-export-cancel");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("org: the export to notes.pdf was cancelled")
    })
    .await;
    ex(&mut editor, "org-export-cancel");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("org: no export is running")
    })
    .await;
}

/// `org.babel.languages` with `sh`, able to take `:var` and run sessions.
fn sh_language() -> lattice_config::ConfigValue {
    use lattice_config::ConfigValue;
    let field = |s: &str| ConfigValue::Str(s.to_string());
    ConfigValue::List(vec![ConfigValue::Record(
        [
            ("name".to_string(), field("sh")),
            ("command".to_string(), field("sh")),
            ("value".to_string(), field("sh")),
            ("session-args".to_string(), ConfigValue::List(Vec::new())),
        ]
        .into_iter()
        .collect(),
    )])
}

/// An org editor over `doc` that may write exports and run `sh` blocks,
/// asking first only when `confirm` is set.
async fn babel_export_editor(
    base: &std::path::Path,
    doc: &str,
    confirm: bool,
) -> (Editor, Arc<PluginLoader>) {
    let mut grant = write_grant(base);
    grant.push("proc:spawn:{org.babel.languages:command}".to_string());
    let (mut editor, loader) = org_editor_holding(
        base,
        doc,
        &grant,
        vec![("org.babel.languages", sh_language())],
    )
    .await;
    if !confirm {
        set_org_option(&mut editor, "babel.confirm", "false");
    }
    (editor, loader)
}

/// Export `editor`'s file to Markdown from the dispatcher and wait for the
/// file, or for an echo that says why there is none.
async fn export_markdown(editor: &mut Editor, md: &std::path::Path) {
    assert!(open_dispatcher(editor).await.is_some());
    press(editor, "mm");
    settle(editor, |e| {
        md.exists() || last_echo(e).is_some_and(|m| !m.starts_with("Exporting"))
    })
    .await;
}

const RUNS: &str =
    "* Results\n#+NAME: greet\n#+begin_src sh :exports results\necho fresh-greet\n#+end_src\n\n\
                    #+RESULTS: greet\n: stale-greet\n\n\
                    #+begin_src sh :exports results :eval no-export\ntouch ran\n#+end_src\n\n\
                    #+RESULTS:\n: kept-as-written\n";

/// An export runs the blocks whose results it shows, and shows their fresh
/// results — in the export only: the file is byte for byte what it was. A
/// block marked `:eval no-export` does not run, and keeps its result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_runs_its_blocks_on_a_copy() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = babel_export_editor(base.path(), RUNS, false).await;
    let md = base.path().join("notes.md");
    export_markdown(&mut editor, &md).await;
    let out = std::fs::read_to_string(&md)
        .unwrap_or_else(|_| panic!("nothing written; the echo is {:?}", last_echo(&editor)));
    assert!(out.contains("fresh-greet"), "{out}");
    assert!(!out.contains("stale-greet"), "{out}");
    assert!(out.contains("kept-as-written"), "{out}");
    assert!(!out.contains("echo fresh-greet"), "only the result: {out}");
    assert!(!base.path().join("ran").exists(), "`:eval no-export` ran");
    assert_eq!(text(&editor), RUNS, "the file is untouched");
}

/// A `:var` chain and a `:session` block export with fresh results: what
/// a block needs runs first, even when it exports nothing itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_var_chain_and_a_session_export_fresh_results() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "* Chain\n#+NAME: base\n#+begin_src sh :exports none\necho 40\n#+end_src\n\n\
               #+begin_src sh :var x=base :exports results\necho $((x + 2))\n#+end_src\n\n\
               #+begin_src sh :session :exports results :results output\ny=5\necho \"session-$y\"\n#+end_src\n";
    let (mut editor, _loader) = babel_export_editor(base.path(), doc, false).await;
    let md = base.path().join("notes.md");
    export_markdown(&mut editor, &md).await;
    let out = std::fs::read_to_string(&md)
        .unwrap_or_else(|_| panic!("nothing written; the echo is {:?}", last_echo(&editor)));
    assert!(out.contains("42"), "{out}");
    assert!(!out.contains("40"), "`:exports none` shows nothing: {out}");
    assert!(out.contains("session-5"), "{out}");
    assert_eq!(text(&editor), doc, "the file is untouched");
}

/// With `org.babel.confirm` on, the export asks once, naming what runs,
/// and writes nothing until `y`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_asks_once_before_running() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = babel_export_editor(base.path(), RUNS, true).await;
    let md = base.path().join("notes.md");
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    let prompt = "Export runs sh block \"greet\"? (y/n)";
    settle(&mut editor, |e| menu_title(e).as_deref() == Some(prompt)).await;
    assert!(!md.exists(), "nothing is written before the answer");
    press(&mut editor, "y");
    settle(&mut editor, |e| md.exists()).await;
    let out = std::fs::read_to_string(&md).unwrap();
    assert!(out.contains("fresh-greet"), "{out}");
}

/// `n` to the question still exports, with the results already in the
/// file, and runs nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_to_the_question_exports_the_results_in_the_file() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = RUNS.replace("echo fresh-greet", "touch ran-anyway; echo fresh-greet");
    let (mut editor, _loader) = babel_export_editor(base.path(), &doc, true).await;
    let md = base.path().join("notes.md");
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    let prompt = "Export runs sh block \"greet\"? (y/n)";
    settle(&mut editor, |e| menu_title(e).as_deref() == Some(prompt)).await;
    press(&mut editor, "n");
    settle(&mut editor, |e| md.exists()).await;
    let out = std::fs::read_to_string(&md).unwrap();
    assert!(
        out.contains("stale-greet") && !out.contains("fresh-greet"),
        "{out}"
    );
    assert!(
        !base.path().join("ran-anyway").exists(),
        "`n` ran the block"
    );
    assert_eq!(text(&editor), doc);
}

/// Wait for a block's process to make `path`: a process is outside the
/// editor, so nothing wakes it when the file appears.
async fn wait_for_file(path: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "{path:?} never appeared"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// A run cancelled while a block runs writes nothing, says so, and stops
/// the block's process: it never reaches its end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_run_stops_its_block_and_writes_nothing() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "* Slow\n#+begin_src sh :exports results\ntouch started; sleep 2; touch finished\n#+end_src\n";
    let (mut editor, _loader) = babel_export_editor(base.path(), doc, false).await;
    let md = base.path().join("notes.md");
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    wait_for_file(&base.path().join("started")).await;
    ex(&mut editor, "org-export-cancel");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("org: the export was cancelled")
    })
    .await;
    // Past when the block would have finished.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    editor.run_tick_pending();
    assert!(!base.path().join("finished").exists(), "the block ran on");
    assert!(!md.exists(), "a cancelled export wrote");
    assert_eq!(text(&editor), doc);
}

fn buffers(editor: &Editor) -> Arc<lattice_mode::BufferStoreHandle> {
    editor
        .services
        .get::<lattice_mode::BufferStoreHandle>()
        .expect("the buffer store is a service")
        .clone()
}

fn buffer_text(editor: &Editor, id: lattice_core::BufferId) -> String {
    buffers(editor)
        .handle_for(id)
        .map(|d| d.snapshot().text().to_string())
        .unwrap_or_default()
}

/// `h H` exports into `*Org HTML Export*`; exporting again replaces that
/// buffer's text in place, which seeding alone would not (a seed is
/// ignored for a buffer that exists).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_buffer_export_opens_then_replaces_its_buffer() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;
    let source = base.path().join("notes.org");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "hH");
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("Exported *Org HTML Export*")
    })
    .await;
    let out = buffers(&editor)
        .find_by_name("*Org HTML Export*")
        .expect("the export buffer opened");
    assert_eq!(editor.document_buffer_id, out, "and has focus");
    assert!(buffer_text(&editor, out).contains("<title>Plan</title>"));
    assert!(
        !base.path().join("notes.html").exists(),
        "no file is written"
    );

    // Back in the org buffer, change the title and export again.
    editor.do_edit(Some(source), false);
    editor.run_tick_pending();
    ex(&mut editor, "1d");
    assert!(text(&editor).starts_with("* One"), "{}", text(&editor));
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "hH");
    settle(&mut editor, |e| {
        buffer_text(e, out).contains("<title>notes</title>")
    })
    .await;
    assert_eq!(
        buffers(&editor).find_by_name("*Org HTML Export*"),
        Some(out),
        "the same buffer"
    );
    assert!(!buffer_text(&editor, out).contains("<title>Plan</title>"));
}

/// `h o` writes the file and then opens it, in lattice by default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_open_export_writes_then_opens_the_file() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;
    let html = base.path().join("notes.html");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "ho");
    settle(&mut editor, |e| {
        buffers(e).path_for(e.document_buffer_id).as_deref() == Some(html.as_path())
    })
    .await;
    assert!(std::fs::read_to_string(&html)
        .unwrap()
        .contains("<title>Plan</title>"));
}

/// A file whose parse does not fit one call's fuel is exported over several
/// steps, still with no key pressed, and the whole of it lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_too_large_for_one_call_is_exported_in_steps() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let section = "* TODO [#A] A heading :tag:\n\
                   SCHEDULED: <2026-01-05 Mon>\n\
                   - [ ] an item\n  continued\n- term :: description\n\
                   | a | b |\n|---+---|\n| 1 | 2 |\n\
                   #+begin_src sh\necho x\n#+end_src\n\
                   A paragraph of text.\n";
    let mut doc = String::new();
    for i in 0..4_000 {
        doc.push_str(&section.replace("A heading", &format!("Heading {i}")));
    }
    let (mut editor, _loader) = org_editor(base.path(), &doc, &grant).await;
    let md = base.path().join("notes.md");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    settle(&mut editor, |e| {
        md.exists() || last_echo(e).is_some_and(|m| !m.starts_with("Exporting"))
    })
    .await;
    let written = std::fs::read_to_string(&md)
        .unwrap_or_else(|_| panic!("nothing written; the echo is {:?}", last_echo(&editor)));
    assert!(
        written.contains("\n# <a id=\""),
        "headings: {}",
        &written[..200]
    );
    assert!(
        written.contains("1. TODO Heading 0   `:tag:`\n"),
        "the first section"
    );
    assert!(
        written.contains("4000. TODO Heading 3999   `:tag:`\n"),
        "and the last"
    );
}

/// The file's options and macros shape what is exported, and what it asks
/// for that the export does not do is named in the closing echo rather than
/// dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_files_options_and_macros_shape_the_export_and_the_unsupported_are_named() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let doc = "#+SETUPFILE: theme.org\n#+MACRO: who Ada\n* Kept\n* Dropped :noexport:\n\
               * COMMENT Draft\n* Last\n#+OPTIONS: ^:nil html-postamble:nil\na_b by {{{who}}}\n";
    let (mut editor, _loader) = org_editor(base.path(), doc, &grant).await;
    let md = base.path().join("notes.md");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    settle(&mut editor, |e| {
        last_echo(e).is_some_and(|m| m.starts_with("Exported"))
    })
    .await;
    let written = std::fs::read_to_string(&md).unwrap();
    assert!(
        written.contains("1. Kept\n") && written.contains("2. Last\n"),
        "{written}"
    );
    assert!(
        !written.contains("Dropped") && !written.contains("Draft"),
        "{written}"
    );
    assert!(
        written.contains("\na\\_b by Ada\n"),
        "^:nil, set after the text, and the macro: {written}"
    );
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("Exported notes.md; not supported, so left out: #+SETUPFILE, html-postamble:")
    );
}

/// An include is read through the host's `read-file`: one the grant covers
/// is spliced in, and one outside it fails the export, naming the path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn includes_read_what_the_grant_allows_and_name_what_it_does_not() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(base.path().join("part.org"), "* Included\nfrom part\n").unwrap();
    std::fs::write(outside.path().join("secret.org"), "* Secret\n").unwrap();
    let grant = write_grant(base.path());
    let doc = "* Top\n#+INCLUDE: \"part.org\"\n";
    let (mut editor, _loader) = org_editor(base.path(), doc, &grant).await;
    let md = base.path().join("notes.md");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    settle(&mut editor, |e| {
        last_echo(e).is_some_and(|m| m.starts_with("Exported"))
    })
    .await;
    let written = std::fs::read_to_string(&md).unwrap();
    assert!(
        written.contains("\n## <a id=") && written.contains("1.1. Included\n\nfrom part\n"),
        "the include's headline under the one it is in: {written}"
    );

    let secret = outside.path().join("secret.org");
    let doc = format!("* Top\n#+INCLUDE: \"{}\"\n", secret.display());
    let (mut editor, _loader) = org_editor(base.path(), &doc, &grant).await;
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    settle(&mut editor, |e| {
        last_echo(e).is_some_and(|m| m.starts_with("org: cannot include"))
    })
    .await;
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(echo.contains(&secret.display().to_string()), "{echo}");
}

/// A link to nothing stops the export by default, as in emacs, and says how
/// to export it anyway; nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_internal_link_stops_the_export() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let doc = "* One\nSee [[Two]] and [[Three]].\n* Two\n";
    let (mut editor, _loader) = org_editor(base.path(), doc, &grant).await;
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    settle(&mut editor, |e| {
        last_echo(e).is_some_and(|m| !m.starts_with("Exporting"))
    })
    .await;
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(
        echo.contains("[[Three]]") && echo.contains("broken-links"),
        "{echo}"
    );
    assert!(!base.path().join("notes.md").exists());
}

/// A switch flipped in the dispatcher reaches the command, and is still set
/// the next time the dispatcher opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_switches_reach_the_export_and_stay_set() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let grant = write_grant(base.path());
    let (mut editor, _loader) = org_editor(base.path(), DOC, &grant).await;
    let md = base.path().join("notes.md");

    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "<C-b>mm");
    settle(&mut editor, |_| md.exists()).await;
    // Body only leaves out the contents, and with them the anchors only
    // they link to.
    let body_only = std::fs::read_to_string(&md).unwrap();
    assert!(
        body_only.starts_with("# 1. One\n"),
        "no contents: {body_only}"
    );

    std::fs::remove_file(&md).unwrap();
    assert!(open_dispatcher(&mut editor).await.is_some());
    press(&mut editor, "mm");
    settle(&mut editor, |_| md.exists()).await;
    let again = std::fs::read_to_string(&md).unwrap();
    assert!(again.starts_with("# 1. One\n"), "still body only: {again}");
}

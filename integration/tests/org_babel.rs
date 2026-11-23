//! OB.4+ — `org-babel-mode`: running source blocks.
//!
//! Boots a real editor, loads the built org component through the loader, and
//! drives babel through its chords and commands, as a user would. Skips (with
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
    // OB.6: what `install` wires and a hand-built loader does not. Each one
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
            // IM.6b: the media seam fails the whole load when unwired, by
            // design — a plugin declaring `media` whose images can never be
            // drawn should say so rather than load silently half-working.
            media_registry: Some(std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                lattice_mode::MediaSourceRegistry::new(),
            ))),
            // OM.11: refile's target list is a picker source, so the loader
            // needs somewhere to register it.
            picker_registry: Some(editor.picker_registry.clone()),
            // OB.6: the running sign — the editor's own sign and decoration
            // registries, so a test asks the producer the renderer asks.
            sign_registry: editor
                .services
                .get::<lattice_mode::SignRegistryHandle>()
                .map(|h| (*h).clone()),
            decoration_registry: editor
                .services
                .get::<lattice_mode::GutterDecorationSourceRegistryHandle>()
                .map(|h| (*h).clone()),
            // OC.3: the capture menu registers into the editor's OWN transient
            // registry — the one TR.1 moved to `editor_boot` so a plugin menu
            // does not depend on whether magit happened to load.
            transient_registry: editor
                .services
                .get::<lattice_picker::TransientSourceRegistryHandle>()
                .map(|h| (*h).clone()),
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

/// Boot an editor, load the org plugin into its live registries, and open a
/// `.org` file holding `text`. Returns the editor and the file path.
///
/// The loader comes back too, and a test must hold it: its option
/// subscriptions (`org.babel.enabled`'s gate among them) hold it weakly, as
/// the editor's own does, and stop when it drops.
async fn org_editor(base: &std::path::Path, text: &str) -> (Editor, Arc<PluginLoader>) {
    org_editor_with_caps(base, text, &[], &[]).await
}

/// [`org_editor`] with babel languages in the user's configuration, as
/// `(name, command)`: held BEFORE the plugin loads, because the process grant
/// is resolved at load from what the user wrote (OB.2).
async fn org_editor_with_languages(
    base: &std::path::Path,
    text: &str,
    languages: &[(&str, &str)],
) -> (Editor, Arc<PluginLoader>) {
    let records: Vec<Vec<(&str, &str)>> = languages
        .iter()
        .map(|(name, command)| vec![("name", *name), ("command", *command)])
        .collect();
    org_editor_with_records(base, text, &records).await
}

/// [`org_editor_with_languages`] with whole `org.babel.languages` records:
/// `(field, value)` pairs, `args` split on whitespace into its list.
async fn org_editor_with_records(
    base: &std::path::Path,
    text: &str,
    records: &[Vec<(&str, &str)>],
) -> (Editor, Arc<PluginLoader>) {
    let (mut editor, loader) = org_editor_with_caps(base, text, &[], records).await;
    // Most tests are about the run, not the question before it; the confirm
    // tests turn it back on.
    set_org_option(&mut editor, "babel.confirm", "false");
    (editor, loader)
}

/// [`org_editor`], plus the OS capabilities the manifest declares.
///
/// OM.6b is the first slice that needs any: a cross-file write is refused at
/// the boundary unless the plugin holds `fs:write` over the target, and the
/// refusal is silent to the guest (the effect is replaced with an `Echo`
/// before it reaches the editor). A test that forgot the grant would look
/// exactly like a broken archive.
async fn org_editor_with_caps(
    base: &std::path::Path,
    text: &str,
    capabilities: &[String],
    languages: &[Vec<(&str, &str)>],
) -> (Editor, Arc<PluginLoader>) {
    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    let caps = capabilities
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let sep = if caps.is_empty() { "" } else { ", " };
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            // OT.4: `editor_capabilities` mirrors the shipped manifest. Without
            // the `tree-sitter` grant the trampoline hands every seam a `none`
            // tree and the guest falls back to line matching — so a harness
            // that omitted it would test the text path while claiming to test
            // the tree one.
            "id = \"org\"\nprovides = [\"modes\", \"grammar\", \"language\", \"help\", \"config\", \"media\", \"picker-source\", \"transient-source\", \"events\", \"signs\", \"decorations\"]\ndefault_modes = [\"org-todo-mode\", \"org-global-mode\", \"org-table-mode\", \"org-babel-mode\"]\nmode_gates = {{ \"org-babel-mode\" = \"org.babel.enabled\" }}\neditor_capabilities = [\"tree-sitter\"]\ncapabilities = [\"proc:spawn:{{org.babel.languages:command}}\", \"state:write\"{sep}{caps}]\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("component.wasm"),
        org_plugin_wasm().expect("caller checked"),
    )
    .unwrap();

    let mut editor = boot_sealed_editor();
    if !languages.is_empty() {
        use lattice_config::ConfigValue;
        let table = languages
            .iter()
            .map(|fields| {
                ConfigValue::Record(
                    fields
                        .iter()
                        .map(|(k, v)| {
                            let value = if *k == "args" {
                                ConfigValue::List(
                                    v.split_whitespace()
                                        .map(|a| ConfigValue::Str(a.to_string()))
                                        .collect(),
                                )
                            } else {
                                ConfigValue::Str(v.to_string())
                            };
                            (k.to_string(), value)
                        })
                        .collect(),
                )
            })
            .collect();
        editor.config.hold_tree(
            "org.babel.languages",
            ConfigValue::List(table),
            lattice_config::HeldSource::ConfigFile,
        );
    }
    let loader = Arc::new(loader_over_editor(&editor, base));
    // OB.3b: `org.babel.enabled` is followed by the loader's gate
    // subscription, which `install` starts at boot; start it here too.
    loader.subscribe_mode_gates();
    let loaded = loader
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");

    // OM.4b: boot subscribes to `PluginLoaded` and expands a plugin mode's
    // motion / text-object bindings into operator rows on a spawned task. That
    // is right in production and a race in a test that dispatches immediately,
    // so drive the same function directly here. The event-driven wiring has its
    // own test below rather than every test racing it.
    {
        let commands = editor.registry.load();
        for (mode_id, kind) in editor.mode_registry.load().iter_meta() {
            let layer = match kind {
                lattice_mode::ModeKind::Major => lattice_keymap::KeymapLayer::MajorMode(mode_id),
                lattice_mode::ModeKind::Minor => lattice_keymap::KeymapLayer::MinorMode(mode_id),
            };
            lattice_host::keymap_normal::expand_grammar_rows(
                &editor.keymap,
                &commands,
                &editor.builtins,
                layer,
            );
        }
    }

    // OM.7: `org-todo-mode` needs TWO per-tick drains, in this order, and both
    // are races against a test that dispatches immediately.
    //
    //   1. The manifest's `default_mode` makes the loader publish
    //      `ModeEnablementRequested`, and a plugin minor is INERT until that
    //      lands — `auto_activatable_minors` filters on enablement (CI.3).
    //   2. Opening the file publishes `MajorEntered`, and the minor is
    //      activated from THAT, against the buffer's now-org major.
    //
    // Enablement must precede the open or step 2 finds the mode still
    // disabled; activation must follow it or the policy sees an empty major.
    // `run_tick_pending` is the aggregator both drains hang off, so call it
    // rather than hand-picking, and call it on both sides of the open.
    editor.run_tick_pending();

    let file = base.join("notes.org");
    std::fs::write(&file, text).unwrap();
    editor.do_edit(Some(file), false);
    editor.run_tick_pending();
    // The actor publishes after every dispatch; `buffer-text`'s gate reads
    // the published mode sets, so a test must publish too.
    editor.publish_render_state();
    assert_eq!(
        editor
            .active_modes
            .get(&editor.document_buffer_id)
            .and_then(|m| m.major()),
        Some(ModeId::new("org-mode")),
        "the buffer is in org-mode before any chord is dispatched"
    );
    // CD.4: captures are draft FILES under `{org.directory}/captures`. Set for
    // every test so a capture never falls back to the unsaveable path, whose
    // warning would otherwise be the last message a capture test reads.
    set_org_option(&mut editor, "directory", base.to_str().unwrap());
    (editor, loader)
}

/// Press `keys`; the effects a renderer peer would apply (a confirm) come
/// back, since a bare `Editor` does not apply them.
fn press(editor: &mut Editor, keys: &str) -> Vec<lattice_grammar::Effect> {
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial: Vec<KeyChord> = Vec::new();
    let mut effects = Vec::new();
    for c in seq {
        effects.extend(
            editor
                .dispatch_chord_with_outcome(c, &mut partial)
                .1
                .effects,
        );
    }
    editor.publish_render_state();
    effects
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

/// The confirm among `effects`: its prompt, and what `y` would dispatch.
fn confirm_in(
    effects: &[lattice_grammar::Effect],
) -> Option<(String, String, lattice_grammar::Args)> {
    effects.iter().find_map(|e| match e {
        lattice_grammar::Effect::Confirm {
            prompt,
            yes_action,
            args,
        } => Some((prompt.clone(), yes_action.clone(), args.clone())),
        _ => None,
    })
}

/// Answer `y`: dispatch the yes-action with the arguments the confirm
/// carried, as the dialog does.
fn answer_yes(editor: &mut Editor, confirm: (String, String, lattice_grammar::Args)) {
    let (_, yes, args) = confirm;
    let _ = editor.invoke_command_named(&yes, args);
    editor.publish_render_state();
}

fn goto_line(editor: &mut Editor, line: u32) {
    editor.cursor.line = line;
    editor.cursor.byte = 0;
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

fn babel_active(editor: &Editor) -> bool {
    editor
        .active_modes
        .get(&editor.document_buffer_id)
        .is_some_and(|m| m.is_active(ModeId::new("org-babel-mode")))
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
            "did not settle; the buffer is:\n{}",
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

/// A `sh` language, as a user would write it.
const SH: &str = "[[language]]\nname = \"sh\"\ncommand = \"sh\"\n";

const DOC: &str = "* Tasks\n\
                   - [ ] a box\n\
                   #+begin_src sh\n\
                   echo hi\n\
                   #+end_src\n\
                   #+begin_src python\n\
                   print(1)\n\
                   #+end_src\n\
                   #+begin_quote\n\
                   quoted\n\
                   #+end_quote\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn babel_mode_is_active_in_an_org_buffer_by_default() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (editor, _loader) = org_editor(base.path(), DOC).await;
    assert!(
        babel_active(&editor),
        "org-babel-mode is on in an org buffer"
    );
}

/// Outside a src block `C-c C-c` is org's: babel's layer declines and org's
/// answers underneath — a checkbox still toggles, and a quote block reaches
/// org's own "nothing to do here".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_ctrl_c_outside_a_src_block_falls_through() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor(base.path(), DOC).await;

    goto_line(&mut editor, 1);
    press(&mut editor, "<C-c><C-c>");
    assert!(
        text(&editor).contains("- [X] a box"),
        "the checkbox toggled: {}",
        text(&editor)
    );

    goto_line(&mut editor, 9);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: C-c C-c has nothing to do here")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_ctrl_c_in_a_src_block_is_babels() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor(base.path(), DOC).await;
    set_org_option(&mut editor, "babel.languages", SH);

    // A language nobody configured is named, with where to configure it.
    goto_line(&mut editor, 6);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: no babel language \"python\" — add it to org.babel.languages")
    );
    assert_eq!(text(&editor), DOC, "nothing was edited");

    // A configured one is babel's from every line of the block, markers too.
    for line in [2, 3, 4] {
        editor.last_message = None;
        goto_line(&mut editor, line);
        press(&mut editor, "<C-c><C-c>");
        let echo = last_echo(&editor).unwrap_or_default();
        assert!(
            !echo.contains("nothing to do here") && !echo.contains("no babel language"),
            "line {line}: {echo}"
        );
    }
}

/// `org.babel.enabled = false` removes babel's layer, while org stays: `C-c
/// C-c` in a src block then reaches org's own answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_babel_removes_its_layer() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor(base.path(), DOC).await;
    assert!(babel_active(&editor));

    set_org_option(&mut editor, "babel.enabled", "false");
    settle(&mut editor, |e| !babel_active(e)).await;

    goto_line(&mut editor, 3);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: C-c C-c has nothing to do here")
    );
    // The rest of org is untouched.
    assert!(editor
        .active_modes
        .get(&editor.document_buffer_id)
        .is_some_and(|m| m.is_active(ModeId::new("org-todo-mode"))));

    // And back on.
    set_org_option(&mut editor, "babel.enabled", "true");
    settle(&mut editor, babel_active).await;
}

// ── OB.6: `C-c C-c` runs a block ────────────────────────────────────────────

/// Replace `[start, end)` of the org buffer, as a user's edit would.
fn edit(editor: &mut Editor, start: (u32, u32), end: (u32, u32), with: &str) {
    use lattice_protocol::{Edit, EditKind, Position, Range};
    let target = editor.document_buffer_id;
    let _ = editor.dispatch(lattice_host::action::Action::ApplyEdit {
        target,
        edits: vec![Edit {
            range: Range {
                start: Position {
                    line: start.0,
                    byte: start.1,
                },
                end: Position {
                    line: end.0,
                    byte: end.1,
                },
            },
            kind: EditKind::Replace {
                text: with.to_string(),
            },
        }],
        cursor: None,
    });
    editor.publish_render_state();
}

/// One `sh` block, with `body` (lines joined by `\n`), under a headline.
fn sh_doc(body: &str) -> String {
    format!("* Run\n#+begin_src sh\n{body}\n#+end_src\nafter\n")
}

/// Run the block on `line` and wait for its result to land.
async fn run_and_settle(editor: &mut Editor, line: u32, done: impl Fn(&Editor) -> bool) {
    goto_line(editor, line);
    press(editor, "<C-c><C-c>");
    settle(editor, done).await;
}

fn has(needle: &'static str) -> impl Fn(&Editor) -> bool {
    move |e| text(e).contains(needle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_block_runs_and_its_output_appears_without_a_keypress() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("echo hi"), &[("sh", "sh")]).await;
    // `settle` waits on the editor's wake and ticks; it never presses a key,
    // so a result that needed one would time out here.
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    assert_eq!(
        text(&editor),
        "* Run\n#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nafter\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_results_are_replaced_not_appended() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh\necho new\n#+end_src\n\n#+RESULTS:\n: old\n: older\nafter\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 1, has(": new")).await;
    assert_eq!(
        text(&editor),
        "#+begin_src sh\necho new\n#+end_src\n\n#+RESULTS:\n: new\nafter\n"
    );
    // And again, over its own result: still one.
    let before = text(&editor);
    edit(&mut editor, (1, 0), (1, 8), "echo two");
    run_and_settle(&mut editor, 1, has(": two")).await;
    assert_eq!(text(&editor), before.replace("new", "two"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_that_looks_like_a_headline_cannot_become_one() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    // Few lines: `: ` lines, which cannot start a headline.
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("echo '* Not a headline'\necho '#+end_src'"),
        &[("sh", "sh")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    assert!(
        text(&editor).contains("#+RESULTS:\n: * Not a headline\n: #+end_src\n"),
        "{}",
        text(&editor)
    );

    // Many lines: an example block, its org-looking lines comma-escaped.
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("for i in 1 2 3 4 5 6 7 8 9 10; do echo \"* h$i\"; done\necho '#+end_example'"),
        &[("sh", "sh")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+end_example\nafter")).await;
    let t = text(&editor);
    assert!(t.contains("#+begin_example\n,* h1\n"), "{t}");
    assert!(t.contains(",* h10\n,#+end_example\n#+end_example\n"), "{t}");
    assert!(
        !t.lines().any(|l| l.starts_with("* h")),
        "no output line became a headline: {t}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ten_lines_become_an_example_block() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("for i in 1 2 3 4 5 6 7 8 9 10; do echo $i; done"),
        &[("sh", "sh")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+end_example")).await;
    assert!(
        text(&editor).contains("#+RESULTS:\n#+begin_example\n1\n2\n"),
        "{}",
        text(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_program_still_writes_its_output_and_echoes_the_status() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("echo partial\nexit 3"),
        &[("sh", "sh")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    assert!(
        text(&editor).contains("#+RESULTS:\n: partial\n"),
        "{}",
        text(&editor)
    );
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(
        echo.starts_with("org: the sh block failed:") && echo.contains('3'),
        "{echo}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_interpreter_reports_the_spawn_error() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("echo hi"),
        &[("sh", "lattice-no-such-interpreter")],
    )
    .await;
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    let t = text(&editor);
    let result = t.split("#+RESULTS:\n").nth(1).unwrap_or_default();
    assert!(
        result.starts_with(": ") && result.contains("lattice-no-such-interpreter"),
        "the spawn error is the result: {t}"
    );
    assert!(
        last_echo(&editor).unwrap_or_default().contains("failed"),
        "{:?}",
        last_echo(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editing_above_the_block_while_it_runs_still_lands_on_it() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    edit(&mut editor, (0, 0), (0, 0), "new line\nanother\n");
    settle(&mut editor, has("#+RESULTS:")).await;
    assert_eq!(
        text(&editor),
        "new line\nanother\n* Run\n#+begin_src sh\nsleep 0.5\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nafter\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editing_the_block_body_while_it_runs_sends_output_to_org_babel() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    // `echo hi` → `echo ho`: the block the run was for is not there any more.
    edit(&mut editor, (3, 6), (3, 7), "o");
    let edited = text(&editor);
    editor.last_message = None;
    settle(&mut editor, |e| e.last_message.is_some()).await;
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: the block changed while it ran — :org-babel-output shows the sh block's output")
    );
    assert_eq!(text(&editor), edited, "nothing written at a stale place");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_the_block_while_it_runs_sends_output_to_org_babel() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    edit(&mut editor, (1, 0), (5, 0), "");
    editor.last_message = None;
    settle(&mut editor, |e| e.last_message.is_some()).await;
    assert!(
        last_echo(&editor)
            .unwrap_or_default()
            .ends_with(":org-babel-output shows the sh block's output"),
        "{:?}",
        last_echo(&editor)
    );
    assert_eq!(text(&editor), "* Run\nafter\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_identical_unnamed_blocks_each_get_their_own_result() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh\necho hi\n#+end_src\nmid\n#+begin_src sh\necho hi\n#+end_src\nend\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 5, has("#+RESULTS:")).await;
    assert_eq!(
        text(&editor),
        "#+begin_src sh\necho hi\n#+end_src\nmid\n#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nend\n",
        "the second block's result is the second block's"
    );
    run_and_settle(&mut editor, 1, |e| {
        text(e).matches("#+RESULTS:").count() == 2
    })
    .await;
    assert_eq!(
        text(&editor),
        "#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nmid\n#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nend\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_block_is_not_started_twice() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: the sh block is already running — :org-babel-cancel stops it")
    );
    settle(&mut editor, has("#+RESULTS:")).await;
    assert_eq!(
        text(&editor).matches(": hi").count(),
        1,
        "{}",
        text(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_past_the_cap_is_truncated_with_a_count() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(
        base.path(),
        &sh_doc("for i in 1 2 3 4 5 6 7 8; do echo $i; done"),
        &[("sh", "sh")],
    )
    .await;
    set_org_option(&mut editor, "babel.max-output-lines", "5");
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    assert!(
        text(&editor)
            .contains("#+RESULTS:\n: 1\n: 2\n: 3\n: 4\n: 5\n: … 3 lines truncated\nafter\n"),
        "{}",
        text(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_result_is_one_undo_step() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh\necho new\n#+end_src\n\n#+RESULTS:\n: old\nafter\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 1, has(": new")).await;
    press(&mut editor, "u");
    assert_eq!(
        text(&editor),
        doc,
        "one `u` restores the previous result exactly"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_script_file_is_removed_after_the_run() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("echo hi"), &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    let scripts: Vec<_> = walk(&base.path().join("data"))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "sh"))
        .collect();
    assert!(scripts.is_empty(), "scripts left behind: {scripts:?}");
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// The lines the gutter producers put a sign on, for the org buffer — the
/// question the renderer asks.
async fn signed_lines(editor: &Editor) -> Vec<u32> {
    let registry = editor
        .services
        .get::<lattice_mode::GutterDecorationSourceRegistryHandle>()
        .expect("a boot service");
    let id = u64::from(editor.document_buffer_id.0);
    let count = editor.document.snapshot().buffer.rope_line_count();
    let mut lines = Vec::new();
    for source in registry.load().sources() {
        for d in source.produce(id, None, count).await.unwrap_or_default() {
            let lattice_mode::GutterDecoration::Sign { line, .. } = d;
            lines.push(line);
        }
    }
    lines
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_block_carries_a_sign_until_it_finishes() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    assert!(signed_lines(&editor).await.is_empty(), "nothing runs yet");
    goto_line(&mut editor, 3);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(signed_lines(&editor).await, [1], "on its #+begin_src");
    // It follows the block when lines go in above it.
    edit(&mut editor, (0, 0), (0, 0), "above\n");
    assert_eq!(signed_lines(&editor).await, [2]);
    settle(&mut editor, has("#+RESULTS:")).await;
    assert!(
        signed_lines(&editor).await.is_empty(),
        "gone once it finished"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirm_yes_runs_and_no_does_nothing() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "* Run\n#+NAME: greet\n#+begin_src sh\necho hi\n#+end_src\nafter\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    set_org_option(&mut editor, "babel.confirm", "true");
    goto_line(&mut editor, 3);
    let confirm = confirm_in(&press(&mut editor, "<C-c><C-c>")).expect("a confirm first");
    assert_eq!(confirm.0, "Run sh block \"greet\"? (y/n)");
    // `n` dispatches nothing: nothing claimed, nothing signed, nothing ran.
    assert!(signed_lines(&editor).await.is_empty());
    assert_eq!(text(&editor), doc);
    answer_yes(&mut editor, confirm);
    settle(&mut editor, has("#+RESULTS: greet")).await;
    assert!(
        text(&editor).contains("#+RESULTS: greet\n: hi\n"),
        "{}",
        text(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirm_carries_the_key_across_an_edit_made_while_the_prompt_is_open() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("echo hi"), &[("sh", "sh")]).await;
    set_org_option(&mut editor, "babel.confirm", "true");
    goto_line(&mut editor, 2);
    let confirm = confirm_in(&press(&mut editor, "<C-c><C-c>")).expect("a confirm");
    // Two lines above the block while the prompt is up, and the cursor
    // somewhere else entirely: what runs is still the block the prompt named.
    edit(&mut editor, (0, 0), (0, 0), "one\ntwo\n");
    goto_line(&mut editor, 0);
    answer_yes(&mut editor, confirm);
    settle(&mut editor, has("#+RESULTS:")).await;
    assert_eq!(
        text(&editor),
        "one\ntwo\n* Run\n#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\nafter\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirm_whose_block_changed_runs_nothing() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = sh_doc("echo hi");
    let (mut editor, _loader) = org_editor_with_languages(base.path(), &doc, &[("sh", "sh")]).await;
    set_org_option(&mut editor, "babel.confirm", "true");
    goto_line(&mut editor, 2);
    let confirm = confirm_in(&press(&mut editor, "<C-c><C-c>")).expect("a confirm");
    edit(&mut editor, (2, 6), (2, 7), "o");
    answer_yes(&mut editor, confirm);
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: the blocks changed while the prompt was open — nothing ran")
    );
    assert!(signed_lines(&editor).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirm_off_runs_at_once() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("echo hi"), &[("sh", "sh")]).await;
    goto_line(&mut editor, 2);
    assert!(confirm_in(&press(&mut editor, "<C-c><C-c>")).is_none());
    settle(&mut editor, has("#+RESULTS:")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_buffer_asks_once_and_runs_in_order() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let order = base.path().join("order");
    let order = order.display();
    // The second block reads what the first writes after a pause: it shows
    // `first` only if it started after the first finished.
    let doc = format!(
        "#+begin_src sh\nsleep 0.3\necho first > {order}\necho a\n#+end_src\n\
         #+begin_src sh\ncat {order}\n#+end_src\n"
    );
    let (mut editor, _loader) = org_editor_with_languages(base.path(), &doc, &[("sh", "sh")]).await;
    set_org_option(&mut editor, "babel.confirm", "true");
    let confirm = confirm_in(&ex(&mut editor, "org-babel-execute-buffer")).expect("one confirm");
    assert_eq!(confirm.0, "Run 2 blocks (sh ×2)? (y/n)");
    answer_yes(&mut editor, confirm);
    settle(&mut editor, |e| text(e).matches("#+RESULTS:").count() == 2).await;
    let t = text(&editor);
    assert!(t.contains("#+RESULTS:\n: a\n"), "{t}");
    assert!(t.contains("#+RESULTS:\n: first\n"), "{t}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_subtree_runs_only_its_blocks() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "* A\n#+begin_src sh\necho a\n#+end_src\n** A1\n#+begin_src sh\necho a1\n#+end_src\n\
               * B\n#+begin_src sh\necho b\n#+end_src\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    goto_line(&mut editor, 0);
    // By its key, so the binding is proven along with the command.
    press(&mut editor, "<C-c><C-v>s");
    settle(&mut editor, |e| text(e).matches("#+RESULTS:").count() == 2).await;
    let t = text(&editor);
    assert!(t.contains(": a\n") && t.contains(": a1\n"), "{t}");
    assert!(!t.contains(": b\n"), "B is not under A: {t}");
    assert!(signed_lines(&editor).await.is_empty(), "the batch is over");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_kills_a_never_ending_block() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 1000"), &[("sh", "sh")]).await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(signed_lines(&editor).await, [1]);
    ex(&mut editor, "org-babel-cancel");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: cancelling 1 block")
    );
    settle(&mut editor, |e| {
        last_echo(e).as_deref() == Some("org: the sh block was cancelled")
    })
    .await;
    assert_eq!(
        text(&editor),
        sh_doc("sleep 1000"),
        "a cancelled run writes nothing"
    );
    assert!(signed_lines(&editor).await.is_empty(), "the sign is gone");
    ex(&mut editor, "org-babel-cancel");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: no source block is running here")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_result_deletes_exactly_the_results() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let bare = sh_doc("echo hi");
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &bare, &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 2, has("#+RESULTS:")).await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-v>k");
    assert_eq!(text(&editor), bare, "run then remove is no change at all");
    ex(&mut editor, "org-babel-remove-result");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: this block has no result")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_result_at_the_end_of_a_file_with_no_newline() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    goto_line(&mut editor, 1);
    ex(&mut editor, "org-babel-remove-result");
    assert_eq!(text(&editor), "#+begin_src sh\necho hi\n#+end_src");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_buffer_while_it_runs_sends_output_to_org_babel() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) =
        org_editor_with_languages(base.path(), &sh_doc("sleep 0.5\necho hi"), &[("sh", "sh")])
            .await;
    goto_line(&mut editor, 2);
    press(&mut editor, "<C-c><C-c>");
    ex(&mut editor, "bd!");
    editor.last_message = None;
    settle(&mut editor, |e| e.last_message.is_some()).await;
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: its buffer closed while it ran — :org-babel-output shows the sh block's output")
    );
    // Nothing opened by itself: the layout is the user's.
    assert!(editor.buffers.by_name("*org-babel*").is_none());
    let effects = ex(&mut editor, "org-babel-output");
    for e in effects {
        editor.handle_effect(e);
    }
    let out = editor
        .buffers
        .by_name("*org-babel*")
        .expect(":org-babel-output opens it");
    let shown = |e: &Editor| {
        e.buffers
            .document_handle(out)
            .map(|h| h.snapshot().text().to_string())
            .unwrap_or_default()
    };
    // The mode fills the buffer from the kept lines once it is open.
    settle(&mut editor, |e| shown(e).contains("\nhi")).await;
    assert!(
        shown(&editor).contains("sh block: its buffer closed while it ran"),
        "{}",
        shown(&editor)
    );
}

/// Whether `program` is on this host. A `value` test needs the interpreter,
/// and says so when it is skipped rather than passing quietly.
fn have(program: &str) -> bool {
    let found = std::process::Command::new(program)
        .arg("--version")
        .output()
        .is_ok();
    if !found {
        eprintln!("skipping: `{program}` is not on this host");
    }
    found
}

fn block(header: &str, body: &str) -> String {
    format!("#+begin_src sh {header}\n{body}\n#+end_src\nafter\n")
}

/// Run the block on line 0 of a one-block `doc` and return the text once
/// `done` holds.
async fn run_doc(doc: &str, done: impl Fn(&Editor) -> bool) -> String {
    let base = tempfile::tempdir().unwrap();
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 0, done).await;
    text(&editor)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_append_adds_under_the_existing_result() {
    require_component!();
    let doc = "#+begin_src sh :results append\necho new\n#+end_src\n\n#+RESULTS:\n: old\nafter\n";
    let t = run_doc(doc, has(": new")).await;
    assert!(t.ends_with("#+RESULTS:\n: old\n: new\nafter\n"), "{t}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_prepend_adds_above_the_existing_result() {
    require_component!();
    let doc = "#+begin_src sh :results prepend\necho new\n#+end_src\n\n#+RESULTS:\n: old\nafter\n";
    let t = run_doc(doc, has(": new")).await;
    assert!(t.ends_with("#+RESULTS:\n: new\n: old\nafter\n"), "{t}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_silent_echoes_and_writes_nothing() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = block(":results silent", "echo quiet");
    let (mut editor, _loader) = org_editor_with_languages(base.path(), &doc, &[("sh", "sh")]).await;
    goto_line(&mut editor, 0);
    press(&mut editor, "<C-c><C-c>");
    editor.last_message = None;
    settle(&mut editor, |e| e.last_message.is_some()).await;
    assert_eq!(last_echo(&editor).as_deref(), Some("quiet"));
    assert_eq!(text(&editor), doc);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_none_writes_and_echoes_nothing() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    // The second block runs after the first has finished: when its result
    // is in, the first one's is long settled.
    let doc = "#+begin_src sh :results none\necho gone\n#+end_src\n#+begin_src sh\necho kept\n#+end_src\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    ex(&mut editor, "org-babel-execute-buffer");
    settle(&mut editor, has(": kept")).await;
    let t = text(&editor);
    assert_eq!(t.matches("#+RESULTS:").count(), 1, "{t}");
    assert!(!t.contains("gone\n:") && !t.contains(": gone"), "{t}");
    assert_ne!(last_echo(&editor).as_deref(), Some("gone"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_table_is_an_aligned_org_table() {
    require_component!();
    let t = run_doc(
        &block(":results table", "printf 'a\\t1\\nbb\\t22\\n'"),
        has("#+RESULTS:"),
    )
    .await;
    assert!(
        t.contains("#+RESULTS:\n| a  |  1 |\n| bb | 22 |\nafter\n"),
        "{t}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_raw_is_written_as_org() {
    require_component!();
    let t = run_doc(&block(":results raw", "echo '- item'"), has("#+RESULTS:")).await;
    assert!(t.contains("#+RESULTS:\n- item\nafter\n"), "{t}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_drawer_wraps_in_a_results_drawer() {
    require_component!();
    let t = run_doc(&block(":results drawer", "echo x"), has(":END:")).await;
    assert!(
        t.contains("#+RESULTS:\n:RESULTS:\nx\n:END:\nafter\n"),
        "{t}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_code_is_a_src_block_in_the_language_and_escaped() {
    require_component!();
    let t = run_doc(&block(":results code", "echo '* x'"), has("#+RESULTS:")).await;
    assert!(
        t.contains("#+RESULTS:\n#+begin_src sh\n,* x\n#+end_src\nafter\n"),
        "{t}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_result_kind_that_is_not_built_is_refused_before_anything_runs() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = block(":results file", "echo x");
    let (mut editor, _loader) = org_editor_with_languages(base.path(), &doc, &[("sh", "sh")]).await;
    goto_line(&mut editor, 0);
    press(&mut editor, "<C-c><C-c>");
    assert!(
        last_echo(&editor)
            .unwrap_or_default()
            .starts_with("org: `:results file` is not supported"),
        "{:?}",
        last_echo(&editor)
    );
    assert!(signed_lines(&editor).await.is_empty(), "nothing started");
}

/// The value of `body` in `lang`, run through its built-in wrapper.
async fn value_of(lang: &str, command: &str, ext: &str, body: &str) -> String {
    let base = tempfile::tempdir().unwrap();
    let doc = format!("#+begin_src {lang}\n{body}\n#+end_src\n");
    let record = vec![
        ("name", lang),
        ("command", command),
        ("ext", ext),
        ("value", lang),
    ];
    let (mut editor, _loader) = org_editor_with_records(base.path(), &doc, &[record]).await;
    run_and_settle(&mut editor, 0, has("#+RESULTS:")).await;
    let t = text(&editor);
    t.split_once("#+RESULTS:\n")
        .map(|(_, r)| r.to_string())
        .unwrap_or(t)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn value_python_is_the_last_expression_not_what_it_printed() {
    require_component!();
    if !have("python3") {
        return;
    }
    let v = value_of("python", "python3", "py", "x = 6\nprint('noise')\nx * 7").await;
    assert_eq!(v, ": 42\n");
    let v = value_of("python", "python3", "py", "x = 6\nreturn x * 7").await;
    assert_eq!(v, ": 42\n", "a body that returns, babel's older rule");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn value_ruby_is_the_last_expression() {
    require_component!();
    if !have("ruby") {
        return;
    }
    let v = value_of("ruby", "ruby", "rb", "x = 6\nputs 'noise'\nx * 7").await;
    assert_eq!(v, ": 42\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn value_node_is_the_last_expression() {
    require_component!();
    if !have("node") {
        return;
    }
    let v = value_of(
        "node",
        "node",
        "js",
        "const x = 6;\nconsole.log('noise');\nx * 7",
    )
    .await;
    assert_eq!(v, ": 42\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn value_sh_is_what_it_printed() {
    require_component!();
    let v = value_of("sh", "sh", "sh", "echo one\necho two").await;
    assert_eq!(v, ": one\n: two\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_value_body_writes_what_explains_it() {
    require_component!();
    if !have("python3") {
        return;
    }
    let v = value_of("python", "python3", "py", "1 / 0").await;
    assert!(v.contains("ZeroDivisionError"), "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn value_without_a_wrapper_falls_back_to_output_and_says_so() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = block(":results value", "echo hi");
    let (mut editor, _loader) = org_editor_with_languages(base.path(), &doc, &[("sh", "sh")]).await;
    goto_line(&mut editor, 0);
    press(&mut editor, "<C-c><C-c>");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some(
            "org: sh has no value wrapper, so `:results value` runs as output \
             — name one in its org.babel.languages entry"
        )
    );
    settle(&mut editor, has("#+RESULTS:\n: hi\n")).await;
    // Said once a session, not on every run.
    editor.last_message = None;
    press(&mut editor, "<C-c><C-c>");
    assert!(last_echo(&editor).is_none(), "{:?}", last_echo(&editor));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dir_with_a_tilde_runs_there() {
    require_component!();
    let Some(home) = std::env::var_os("HOME") else {
        eprintln!("skipping: no HOME");
        return;
    };
    let there = tempfile::tempdir_in(&home).unwrap();
    let name = there
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let real = there.path().canonicalize().unwrap();
    let base = tempfile::tempdir().unwrap();
    let doc = block(&format!(":dir ~/{name}"), "pwd -P");
    let (mut editor, _loader) = org_editor_with_languages(base.path(), &doc, &[("sh", "sh")]).await;
    run_and_settle(&mut editor, 0, has("#+RESULTS:")).await;
    assert!(
        text(&editor).contains(&format!("#+RESULTS:\n: {}\n", real.display())),
        "{}",
        text(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dir_that_is_not_a_directory_does_not_start() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = block(":dir /no/such/dir", "pwd");
    let (mut editor, _loader) = org_editor_with_languages(base.path(), &doc, &[("sh", "sh")]).await;
    goto_line(&mut editor, 0);
    press(&mut editor, "<C-c><C-c>");
    editor.last_message = None;
    settle(&mut editor, |e| e.last_message.is_some()).await;
    assert!(
        last_echo(&editor)
            .unwrap_or_default()
            .starts_with("org: the sh block did not start:"),
        "{:?}",
        last_echo(&editor)
    );
    assert_eq!(text(&editor), doc);
}

// ---------------------------------------------------------------------------
// OB.10 — `:tangle`.
// ---------------------------------------------------------------------------

/// An org editor whose plugin may write under `base`, as the shipped manifest
/// lets it write under `org.directory`.
async fn tangle_editor(base: &std::path::Path, doc: &str) -> (Editor, Arc<PluginLoader>) {
    let grant = vec![format!("fs:write:{}", base.display())];
    org_editor_with_caps(base, doc, &grant, &[]).await
}

fn on_disk(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tangle_yes_names_the_file_after_the_org_file() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src python :tangle yes\nprint(1)\n#+end_src\n";
    let (mut editor, _loader) = tangle_editor(base.path(), doc).await;
    ex(&mut editor, "org-babel-tangle");
    assert_eq!(on_disk(&base.path().join("notes.py")), "print(1)\n");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: tangled 1 block to 1 file")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocks_to_one_file_concatenate_in_order() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "* A\n#+begin_src sh :tangle out.sh\necho one\n#+end_src\n\
               * B\n#+begin_src sh :tangle other.sh\necho x\n#+end_src\n\
               #+begin_src sh :tangle out.sh\necho two\n#+end_src\n";
    let (mut editor, _loader) = tangle_editor(base.path(), doc).await;
    ex(&mut editor, "org-babel-tangle");
    assert_eq!(
        on_disk(&base.path().join("out.sh")),
        "echo one\n\necho two\n"
    );
    assert_eq!(on_disk(&base.path().join("other.sh")), "echo x\n");
    assert_eq!(
        last_echo(&editor).as_deref(),
        Some("org: tangled 3 blocks to 2 files")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn padline_no() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh :tangle out.sh\na\n#+end_src\n\
               #+begin_src sh :tangle out.sh :padline no\nb\n#+end_src\n";
    let (mut editor, _loader) = tangle_editor(base.path(), doc).await;
    ex(&mut editor, "org-babel-tangle");
    assert_eq!(on_disk(&base.path().join("out.sh")), "a\nb\n");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shebang_sets_the_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh :tangle run.sh :shebang \"#!/bin/sh\"\necho hi\n#+end_src\n";
    let (mut editor, _loader) = tangle_editor(base.path(), doc).await;
    ex(&mut editor, "org-babel-tangle");
    let target = base.path().join("run.sh");
    assert_eq!(on_disk(&target), "#!/bin/sh\necho hi\n");

    // The bit is set from the target's `document-saved`, off the keystroke,
    // and it touches only the disk: nothing in the editor changes, so there is
    // no wake to settle on. Poll the file itself, bounded.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let executable = || {
        std::fs::metadata(&target)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    while !executable() {
        assert!(
            std::time::Instant::now() < deadline,
            "run.sh was never made executable"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_disallowed_target_writes_nothing() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere.path().join("x.sh");
    let doc = format!(
        "#+begin_src sh :tangle inside.sh\na\n#+end_src\n\
         #+begin_src sh :tangle {}\nb\n#+end_src\n",
        outside.display()
    );
    let (mut editor, _loader) = tangle_editor(base.path(), &doc).await;
    ex(&mut editor, "org-babel-tangle");
    assert!(!base.path().join("inside.sh").exists(), "all or nothing");
    assert!(!outside.exists());
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(echo.contains("nothing tangled"), "{echo}");
    assert!(echo.contains(&outside.display().to_string()), "{echo}");
    assert!(echo.contains("org.babel.tangle-directories"), "{echo}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tangle_into_an_open_buffer_is_undoable() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let target = base.path().join("out.sh");
    std::fs::write(&target, "old one\nold two\n").unwrap();
    let doc = "#+begin_src sh :tangle out.sh\nnew\n#+end_src\n";
    let (mut editor, _loader) = tangle_editor(base.path(), doc).await;
    let notes = editor.document_buffer_id;
    editor.do_edit(Some(target.clone()), false);
    let opened = editor.document_buffer_id;
    let _ = editor.activate_buffer(notes);
    editor.publish_render_state();

    ex(&mut editor, "org-babel-tangle");
    assert_eq!(on_disk(&target), "new\n");

    let _ = editor.activate_buffer(opened);
    editor.undo_blocking().expect("undo");
    assert_eq!(
        text(&editor),
        "old one\nold two\n",
        "one `u` restores the target"
    );
}

/// `#+PROPERTY: header-args :tangle …` reaches every block, and `C-c C-v t`
/// is the same command.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_wide_property_tangle_applies() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+PROPERTY: header-args :tangle all.sh\n\
               #+begin_src sh\na\n#+end_src\n\
               #+begin_src sh :tangle no\nskipped\n#+end_src\n\
               #+begin_src sh\nb\n#+end_src\n";
    let (mut editor, _loader) = tangle_editor(base.path(), doc).await;
    press(&mut editor, "<C-c><C-v>t");
    assert_eq!(on_disk(&base.path().join("all.sh")), "a\n\nb\n");
}

// ---------------------------------------------------------------------------
// OB.11 — `:var`.
// ---------------------------------------------------------------------------

/// Run, in `lang`, a block taking every kind of `:var` value: a number, a
/// quoted literal, a named table, and a named block's two-line result. The
/// strings carry `'`, `"`, `$` and a newline, which a prelude that pasted
/// them in would break on. Returns the block's result lines.
async fn vars_in(lang: &str, command: &str, ext: &str, body: &str) -> String {
    let base = tempfile::tempdir().unwrap();
    let doc = format!(
        "#+NAME: tbl\n| a | 1 |\n| b | 2 |\n\n\
         #+NAME: prev\n#+begin_src {lang}\nx\n#+end_src\n\n\
         #+RESULTS: prev\n: it's \"q\" $HOME\n: line two\n\n\
         #+begin_src {lang} :results output :var n=41 s=\"say \\\"hi\\\" it's $HOME\" t=tbl r=prev\n\
         {body}\n#+end_src\n"
    );
    let at = doc.lines().position(|l| l.contains(":var")).unwrap() as u32;
    let record = vec![
        ("name", lang),
        ("command", command),
        ("ext", ext),
        ("value", lang),
    ];
    let (mut editor, _loader) = org_editor_with_records(base.path(), &doc, &[record]).await;
    run_and_settle(&mut editor, at, |e| {
        text(e).matches("#+RESULTS:").count() == 2
    })
    .await;
    let t = text(&editor);
    t.rsplit_once("#+RESULTS:\n")
        .map(|(_, r)| r.to_string())
        .unwrap_or(t)
}

const VARS_SEEN: &str = ": 42\n: say \"hi\" it's $HOME\n: b 3\n: it's \"q\" $HOME\n: line two\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn var_reaches_sh_as_strings() {
    require_component!();
    let body = "echo $((n + 1))\necho \"$s\"\n\
                set -- $(printf '%s\\n' \"$t\" | sed -n 2p)\necho \"$1 $(($2 + 1))\"\n\
                echo \"$r\"";
    assert_eq!(vars_in("sh", "sh", "sh", body).await, VARS_SEEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn var_reaches_python_as_values() {
    require_component!();
    if !have("python3") {
        return;
    }
    let body = "print(n + 1)\nprint(s)\nprint(t[1][0], t[1][1] + 1)\nprint(r)";
    assert_eq!(vars_in("python", "python3", "py", body).await, VARS_SEEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn var_reaches_ruby_as_values() {
    require_component!();
    if !have("ruby") {
        return;
    }
    let body = "puts n + 1\nputs s\nputs \"#{t[1][0]} #{t[1][1] + 1}\"\nputs r";
    assert_eq!(vars_in("ruby", "ruby", "rb", body).await, VARS_SEEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn var_reaches_node_as_values() {
    require_component!();
    if !have("node") {
        return;
    }
    let body =
        "console.log(n + 1)\nconsole.log(s)\nconsole.log(t[1][0], t[1][1] + 1)\nconsole.log(r)";
    assert_eq!(vars_in("node", "node", "js", body).await, VARS_SEEN);
}

/// `:results value` sees the variables too: the prelude goes in before the
/// value wrapper does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn var_is_seen_by_a_value_wrapper() {
    require_component!();
    if !have("python3") {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src python :var x=20\nx * 2 + 2\n#+end_src\n";
    let record = vec![
        ("name", "python"),
        ("command", "python3"),
        ("ext", "py"),
        ("value", "python"),
    ];
    let (mut editor, _loader) = org_editor_with_records(base.path(), doc, &[record]).await;
    run_and_settle(&mut editor, 0, has("#+RESULTS:")).await;
    assert!(
        text(&editor).ends_with("#+RESULTS:\n: 42\n"),
        "{}",
        text(&editor)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_reference_runs_nothing() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh :var x=nope\necho ran\n#+end_src\n";
    let (mut editor, _loader) = org_editor_with_records(
        base.path(),
        doc,
        &[vec![("name", "sh"), ("command", "sh"), ("value", "sh")]],
    )
    .await;
    goto_line(&mut editor, 0);
    press(&mut editor, "<C-c><C-c>");
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(echo.contains("`nope`"), "{echo}");
    assert_eq!(text(&editor), doc, "nothing ran, nothing written");
}

/// A language whose record names no `value` has no prelude, and says which
/// field to add rather than running the body with `x` unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn var_in_a_language_without_a_prelude_is_refused_by_name() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+begin_src sh :var x=1\necho $x\n#+end_src\n";
    let (mut editor, _loader) = org_editor_with_languages(base.path(), doc, &[("sh", "sh")]).await;
    goto_line(&mut editor, 0);
    press(&mut editor, "<C-c><C-c>");
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(echo.contains("cannot take `:var`"), "{echo}");
    assert!(echo.contains("value = \"sh\""), "{echo}");
    assert_eq!(text(&editor), doc);
}

// ---------------------------------------------------------------------------
// OB.12 — dependency chains.
// ---------------------------------------------------------------------------

/// An org editor with `sh` able to take `:var` (its record names `value`).
async fn chain_editor(base: &std::path::Path, doc: &str) -> (Editor, Arc<PluginLoader>) {
    org_editor_with_records(
        base,
        doc,
        &[vec![("name", "sh"), ("command", "sh"), ("value", "sh")]],
    )
    .await
}

fn line_of(doc: &str, needle: &str) -> u32 {
    doc.lines().position(|l| l.contains(needle)).unwrap() as u32
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_result_is_computed_first() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+NAME: a\n#+begin_src sh\necho 21\n#+end_src\n\n\
               #+NAME: b\n#+begin_src sh :var x=a\necho $((x * 2))\n#+end_src\n";
    let (mut editor, _loader) = chain_editor(base.path(), doc).await;
    set_org_option(&mut editor, "babel.confirm", "true");
    goto_line(&mut editor, line_of(doc, ":var x=a"));
    // One question for the chain: `a` is asked about with `b`, not after.
    let confirm = confirm_in(&press(&mut editor, "<C-c><C-c>")).expect("a confirm");
    assert_eq!(confirm.0, "Run 2 blocks (sh ×2)? (y/n)");
    answer_yes(&mut editor, confirm);
    settle(&mut editor, |e| text(e).matches("#+RESULTS:").count() == 2).await;
    assert_eq!(
        text(&editor),
        "#+NAME: a\n#+begin_src sh\necho 21\n#+end_src\n\n#+RESULTS: a\n: 21\n\n\
         #+NAME: b\n#+begin_src sh :var x=a\necho $((x * 2))\n#+end_src\n\n#+RESULTS: b\n: 42\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_three_block_chain() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+NAME: a\n#+begin_src sh\necho 1\n#+end_src\n\n\
               #+NAME: b\n#+begin_src sh :var x=a\necho $((x + 10))\n#+end_src\n\n\
               #+NAME: c\n#+begin_src sh :var y=b z=a\necho $((y * 2 + z))\n#+end_src\n";
    let (mut editor, _loader) = chain_editor(base.path(), doc).await;
    goto_line(&mut editor, line_of(doc, ":var y=b"));
    press(&mut editor, "<C-c><C-c>");
    settle(&mut editor, |e| text(e).matches("#+RESULTS:").count() == 3).await;
    let t = text(&editor);
    assert!(t.contains("#+RESULTS: a\n: 1\n"), "{t}");
    assert!(t.contains("#+RESULTS: b\n: 11\n"), "{t}");
    assert!(t.ends_with("#+RESULTS: c\n: 23\n"), "{t}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cycle_is_refused_before_anything_runs() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+NAME: a\n#+begin_src sh :var x=b\necho a\n#+end_src\n\n\
               #+NAME: b\n#+begin_src sh :var y=a\necho b\n#+end_src\n";
    let (mut editor, _loader) = chain_editor(base.path(), doc).await;
    goto_line(&mut editor, 1);
    press(&mut editor, "<C-c><C-c>");
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(echo.contains("a → b → a"), "{echo}");
    assert_eq!(text(&editor), doc, "nothing ran");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_dependency_stops_the_chain_and_says_where() {
    require_component!();
    let base = tempfile::tempdir().unwrap();
    let doc = "#+NAME: a\n#+begin_src sh\necho broke; exit 3\n#+end_src\n\n\
               #+NAME: b\n#+begin_src sh :var x=a\necho \"got $x\"\n#+end_src\n";
    let (mut editor, _loader) = chain_editor(base.path(), doc).await;
    goto_line(&mut editor, line_of(doc, ":var x=a"));
    press(&mut editor, "<C-c><C-c>");
    settle(&mut editor, |e| {
        last_echo(e).is_some_and(|m| m.contains("did not run"))
    })
    .await;
    let echo = last_echo(&editor).unwrap_or_default();
    assert!(echo.contains("sh block \"b\""), "says which block: {echo}");
    assert!(echo.contains("`a`"), "and which dependency: {echo}");
    let t = text(&editor);
    assert!(
        t.contains("#+RESULTS: a\n: broke\n"),
        "the failure shows: {t}"
    );
    assert_eq!(t.matches("#+RESULTS:").count(), 1, "`b` never ran: {t}");
}

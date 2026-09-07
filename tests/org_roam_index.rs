//! OR.4 — the roam index, built by the real plugin through a real editor.
//!
//! The unit tests in `src/roam.rs` cover what a node is. What they cannot cover
//! is the half that only exists inside wasm: the tree walk that finds the
//! drawers, the `host-services` store the records land in, and the watcher that
//! keeps it current. So these boot an editor, load the component, point
//! `org.roam-directory` at a corpus, and read the store back through the host.
//!
//! **Reading the store from the host is the point.** The guest writes it, the
//! host holds it, and the assertions below decode the same bytes a *different
//! seam instance* would — which is the drift the whole design is arranged
//! around (`org-roam.md` §2). A test that asked the writing instance what it
//! wrote would pass against a per-instance store.
//!
//! Skips when the component was not built — `cargo test` builds this crate for
//! the HOST, and the component is a separate `--target wasm32-wasip2 --release`
//! artefact.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{discover, LoaderServices, PluginLoader};
use serde::Deserialize;

/// The node record shape the guest writes. Declared here rather than shared,
/// because the plugin is a `cdylib` compiled for wasm — a test that imported
/// the guest's own type would be checking the encoder against itself. This is
/// the *consumer's* view of the contract, which is what a second seam has too.
#[derive(Debug, Deserialize)]
struct Node {
    id: String,
    title: String,
    aliases: Vec<String>,
    tags: Vec<String>,
    #[allow(dead_code)]
    refs: Vec<String>,
    file: String,
    line: u32,
    level: u32,
}

/// Press a chord the way the renderer does. Mirrors `org_structure.rs`'s
/// helper: `dispatch_chord` already RAN the action, so only an `Invoke` is
/// re-dispatched, and only to recover its effects.
async fn press_chord(editor: &mut Editor, keys: &str) {
    use lattice_protocol::parse_chord_sequence;
    let expanded = editor.keymap.expand_leader(keys);
    let seq = parse_chord_sequence(&expanded).expect("parses");
    let mut partial = Vec::new();
    for c in seq {
        // See `org_structure.rs`'s copy: re-dispatching the resolved action to
        // recover its effects RAN IT TWICE. `dispatch_chord_with_outcome` hands
        // the effects back from the single run that already happened.
        let (_action, out) = editor.dispatch_chord_with_outcome(c, &mut partial);
        apply_renderer_effects(editor, out);
    }
}

/// Apply the effects an action RETURNS, the way the renderer does.
///
/// These are the arms this file needs, and every one of them has already
/// masqueraded as a product bug once: an action that returns
/// `Effect::OpenPicker` looks like a dead chord if the test drops it, and one
/// that returns `Effect::OpenBufferAt` looks like an id that would not resolve.
/// `org_structure.rs`'s `apply_renderer_effects` is the fuller peer.
fn apply_renderer_effects(editor: &mut Editor, out: lattice_host::dispatch::DispatchOutcome) {
    for effect in out.effects {
        match effect {
            lattice_grammar::Effect::OpenPicker { source, args } => {
                let _ = editor.open_picker(source, args);
            }
            // OR.11b: with templates configured the accept opens the chooser
            // rather than writing. Without this arm the effect is printed and
            // dropped, and a working flow reads as a create that did nothing.
            lattice_grammar::Effect::OpenTransient { source, args } => {
                editor.open_named_transient(source, args);
            }
            lattice_grammar::Effect::OpenBufferAt {
                path,
                position,
                force,
            } => {
                editor.do_edit(path, force);
                editor.set_cursor_clamped(position);
            }
            // OR.10: opening an EXISTING daily is a plain `OpenBuffer`, and
            // the renderer is the only thing that applies one
            // (`lattice-ui-tui/src/app/dispatch.rs`). Dropping it here would
            // make "open the journal that is already there" indistinguishable
            // from a command that does nothing.
            lattice_grammar::Effect::OpenBuffer { path, force } => {
                editor.do_edit(path, force);
            }
            // OR.10: `:org-roam-dailies-goto-date` with no date prompts, and
            // the prompt is the renderer's too.
            lattice_grammar::Effect::OpenPrompt {
                prompt,
                initial,
                on_submit_action,
                buffer_name,
            } => {
                editor.open_prompt_line(prompt, initial, on_submit_action, buffer_name);
            }
            lattice_grammar::Effect::ApplyEdit {
                target,
                edit,
                cursor,
            } => {
                let _ = editor.dispatch(lattice_host::action::Action::ApplyEdit {
                    target,
                    edit,
                    cursor,
                });
            }
            // Echo and the rest are applied where they were produced.
            _ => {}
        }
    }
}

fn boot_sealed_editor() -> Editor {
    lattice_plugin_loader::disable_autoload();
    Editor::boot(CoreDocument::from_text("scratch\n"))
}

fn org_plugin_wasm() -> Option<Vec<u8>> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    ))
    .ok()
}

/// Stage the plugin with an `fs:read` grant over `corpus` — without it the walk
/// reaches nothing, which is the honest degradation and not what is under test
/// here.
fn write_org_plugin_dir(root: &Path, wasm: &[u8], corpus: &Path) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "id = \"org\"\n\
             provides = [\"events\", \"modes\", \"grammar\", \"language\", \"config\", \"picker-source\", \"completion-source\", \"transient-source\"]\n\
             capabilities = [\"fs:write:{}\", \"state:write\"]\n\
             editor_capabilities = [\"tree-sitter\"]\n\
             default_modes = [\"org-todo-mode\", \"org-global-mode\", \"org-table-mode\"]\n",
            corpus.display()
        ),
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();
}

fn loader_over_editor(editor: &Editor, host: Arc<PluginHost>) -> PluginLoader {
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
            // OR.6: without this the picker seam has nowhere to register and
            // `drain_picker` reports `NotWired` — the source would silently not
            // exist.
            picker_registry: Some(editor.picker_registry.clone()),
            // OR.11b: the create-from-template flow opens a transient, and
            // without somewhere to register it the source is `NotWired` — the
            // menu then reports `unknown source` and the create does nothing.
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

/// The corpus the assertions below are about: both node grains, a link that
/// crosses files, an inherited tag, and an `:ID:` inside a source block that
/// must NOT become a node.
fn write_corpus(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("chicken.org"),
        ":PROPERTIES:\n\
         :ID:       FILE-AAAA\n\
         :ROAM_ALIASES: \"Honey Garlic\" Chicken\n\
         :END:\n\
         #+title: Honey Garlic Chicken Breast\n\
         #+filetags: :food:recipe:\n\
         \n\
         Cooked with [[id:FILE-BBBB][the other note]].\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("rust.org"),
        ":PROPERTIES:\n\
         :ID:       FILE-BBBB\n\
         :END:\n\
         #+title: Rust\n\
         #+filetags: :lang:\n\
         \n\
         * Async                                              :async:\n\
         :PROPERTIES:\n\
         :ID:       HEAD-CCCC\n\
         :END:\n\
         \n\
         Nested note pointing at [[id:FILE-AAAA]].\n",
    )
    .unwrap();
    // The phantom guard: an `:ID:` written INSIDE a source block is example
    // text, not a node, and no line scanner can tell the difference.
    std::fs::write(
        dir.join("example.org"),
        ":PROPERTIES:\n\
         :ID:       FILE-DDDD\n\
         :END:\n\
         #+title: Example\n\
         \n\
         #+BEGIN_SRC org\n\
         * Not a real headline\n\
         :PROPERTIES:\n\
         :ID:       PHANTOM-EEEE\n\
         :END:\n\
         #+END_SRC\n",
    )
    .unwrap();
}

/// Everything the store holds, decoded — the reader's view.
struct Index {
    host: Arc<PluginHost>,
}

impl Index {
    fn raw(&self, key: &str) -> Option<Vec<u8>> {
        self.host.plugin_store_get("org", key)
    }
    fn node(&self, id: &str) -> Option<Node> {
        let key = format!("n/{}", id.to_lowercase());
        self.raw(&key)
            .and_then(|b| rmp_serde::from_slice::<Node>(&b).ok())
    }
    fn nodes(&self) -> Vec<Node> {
        self.raw("nodes")
            .and_then(|b| rmp_serde::from_slice::<Vec<Node>>(&b).ok())
            .unwrap_or_default()
    }
    fn backlinks(&self, id: &str) -> Vec<String> {
        let key = format!("b/{}", id.to_lowercase());
        self.raw(&key)
            .and_then(|b| rmp_serde::from_slice::<Vec<String>>(&b).ok())
            .unwrap_or_default()
    }
}

/// Boot an editor with the plugin loaded and roam pointed at `corpus`, then
/// wait for the index to appear.
async fn index_corpus(base: &Path, corpus: &Path) -> Option<Index> {
    index_corpus_with_editor(base, corpus).await.map(|(i, _)| i)
}

/// OR.6's harness: the same boot, but the editor is kept so a test can drive
/// the picker through it. `index_corpus` is the OR.4 shape, which does not need
/// it.
async fn index_corpus_with_editor(base: &Path, corpus: &Path) -> Option<(Index, Editor)> {
    let wasm = org_plugin_wasm()?;
    let plugins = base.join("plugins");
    write_org_plugin_dir(&plugins, &wasm, corpus);

    let mut editor = boot_sealed_editor();
    let host = Arc::new(
        PluginHost::with_dirs(base.join("cache"), base.join("data")).expect("host builds"),
    );
    let loader = loader_over_editor(&editor, host.clone());
    for found in discover(&plugins) {
        loader
            .load_discovered(&found, TrustTier::Bundled)
            .await
            .expect("the org plugin loads");
    }
    // Set the option AFTER the load, which is the documented shape: an option
    // does not EXIST until the plugin declaring it has loaded, so `init.rs`
    // sets it from a `plugin-loaded` handler. `:org-roam-sync` is then what
    // makes the index catch up — and it is a real command, not a test hook.
    editor
        .config
        .parse_and_set_command(&format!("org.roam-directory={}", corpus.display()))
        .expect("the plugin registered `org.roam-directory`");

    // OR.6: the two steps a chord needs before it can dispatch, both of which
    // this harness lacked — which is why `<leader>onf` was once believed
    // broken and reverted. Neither is roam's; `org_structure.rs`'s harness has
    // carried both since OM.4b/OM.7 and its `<leader>oc` test passes.
    //
    //   1. Boot expands a plugin mode's grammar rows into keymap layers from a
    //      `PluginLoaded` subscription, on a SPAWNED task — right in
    //      production, a race against a test that dispatches immediately.
    //   2. A plugin minor is INERT until `ModeEnablementRequested` lands, and
    //      `run_tick_pending` is the aggregator that drain hangs off.
    //
    // Without them the mode registers, the binding is present in the
    // declaration, and the chord still reaches nothing — which reads exactly
    // like a bad binding.
    {
        let commands = editor.registry.load();
        for (mode_id, kind) in editor.mode_registry.load().iter_meta() {
            let layer = match kind {
                lattice_mode::ModeKind::Major => lattice_keymap::KeymapLayer::MajorMode(mode_id),
                lattice_mode::ModeKind::Minor => lattice_keymap::KeymapLayer::MinorMode(mode_id),
            };
            lattice_host::keymap_normal::expand_plugin_mode_grammar_rows(
                &editor.keymap,
                &commands,
                &editor.builtins,
                layer,
            );
        }
    }
    editor.run_tick_pending();

    sync(&editor).await;
    Some((Index { host }, editor))
}

/// Run `:org-roam-sync`. The ex-command rings a bus doorbell and the EVENT
/// store does the walk, so this returns before the index exists — every
/// assertion below polls.
async fn sync(editor: &Editor) {
    let id = editor
        .registry
        .load()
        .id_by_name("org-roam-sync")
        .expect("`:org-roam-sync` is registered");
    let _ = id;
    editor.event_bus.publish(lattice_protocol::Event::Plugin {
        name: "org/roam-sync".to_string(),
        payload: Vec::new(),
    });
}

/// Poll until `f` is satisfied, or give up. The index lands on the event
/// actor's own task, so there is nothing to await — and deliberately nothing to
/// press, either.
async fn settle(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..settle_budget(200) {
        if f() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_corpus_becomes_an_index() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some(index) = index_corpus(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };

    assert!(
        settle(|| index.nodes().len() >= 4).await,
        "the walk indexed every node: {:?}",
        index.nodes().iter().map(|n| &n.id).collect::<Vec<_>>()
    );

    // The file grain — 81% of the reference corpus, and the one whose drawer is
    // NOT a `property_drawer`.
    let chicken = index.node("FILE-AAAA").expect("the file node");
    assert_eq!(chicken.title, "Honey Garlic Chicken Breast");
    assert_eq!(chicken.aliases, vec!["Honey Garlic", "Chicken"]);
    assert_eq!(chicken.tags, vec!["food", "recipe"]);
    assert_eq!(chicken.line, 0);
    assert_eq!(chicken.level, 0);
    assert!(chicken.file.ends_with("chicken.org"));

    // The headline grain, with its tag inherited from the file.
    let head = index.node("HEAD-CCCC").expect("the headline node");
    assert_eq!(head.title, "Async");
    assert_eq!(head.line, 6, "a headline node lands on its headline");
    assert_eq!(head.level, 1);
    assert_eq!(head.tags, vec!["async", "lang"], "own, then the file's");

    // Ids compare case-insensitively — a link that failed over letter case
    // would look exactly like a missing note.
    assert!(
        index.node("file-aaaa").is_some(),
        "the same node, asked for in the other case"
    );

    // Backlinks, in both directions and across files.
    assert_eq!(index.backlinks("FILE-BBBB"), vec!["FILE-AAAA"]);
    assert_eq!(index.backlinks("FILE-AAAA"), vec!["HEAD-CCCC"]);
}

/// **The phantom guard.** An `:ID:` inside a `#+BEGIN_SRC` block is example
/// text. No line scanner can tell; the tree can, and this is the assertion that
/// says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_id_inside_a_source_block_is_not_a_node() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some(index) = index_corpus(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.node("FILE-DDDD").is_some()).await);
    assert!(
        index.node("PHANTOM-EEEE").is_none(),
        "an :ID: inside a source block is example text, not a node"
    );
}

/// A note written while the editor is running reaches the index with **no key
/// pressed** — through the watcher, on the event actor's own task.
///
/// A test that dispatched anything after the write would pass against a seam
/// that only ever indexes when something else happens to run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_note_is_indexed_without_a_keypress() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some(index) = index_corpus(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.node("FILE-AAAA").is_some()).await);

    std::fs::write(
        corpus.join("late.org"),
        ":PROPERTIES:\n:ID: LATE-FFFF\n:END:\n#+title: Written Later\n",
    )
    .unwrap();

    assert!(
        settle(|| index.node("LATE-FFFF").is_some()).await,
        "the watcher indexed it with nothing dispatched"
    );
    assert_eq!(index.node("LATE-FFFF").unwrap().title, "Written Later");
}

/// **The case a cache-shaped design forgets.** A node deleted from a file
/// leaves the index — from `n/<id>`, from `nodes`, and from every `b/<id>` that
/// named it. Without the last one a backlinks view cites a note that no longer
/// links there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deleted_node_leaves_the_index_and_its_backlinks() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some(index) = index_corpus(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.backlinks("FILE-BBBB") == vec!["FILE-AAAA"]).await);

    // Rewrite chicken.org without its link, and without its id.
    std::fs::write(corpus.join("chicken.org"), "#+title: No longer a node\n").unwrap();

    assert!(
        settle(|| index.node("FILE-AAAA").is_none()).await,
        "the node record is gone"
    );
    assert!(
        settle(|| index.backlinks("FILE-BBBB").is_empty()).await,
        "and so is the backlink it created"
    );
    assert!(
        settle(|| !index.nodes().iter().any(|n| n.id == "FILE-AAAA")).await,
        "and the blob the picker reads no longer offers it"
    );
}

/// **Roam is inert when unconfigured.** No walk, no watcher, no store write —
/// an org user who keeps no zettelkasten pays nothing for this existing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn roam_is_inert_with_no_directory_configured() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);

    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("SKIP: org component not built");
        return;
    };
    let plugins = base.path().join("plugins");
    write_org_plugin_dir(&plugins, &wasm, &corpus);

    // Boot WITHOUT setting `org.roam-directory`.
    let editor = boot_sealed_editor();
    let host = Arc::new(
        PluginHost::with_dirs(base.path().join("cache"), base.path().join("data"))
            .expect("host builds"),
    );
    let loader = loader_over_editor(&editor, host.clone());
    for found in discover(&plugins) {
        loader
            .load_discovered(&found, TrustTier::Bundled)
            .await
            .unwrap();
    }
    // No option set, and a sync rung anyway — inert must mean inert even when
    // something asks for a scan, not merely when nothing does.
    sync(&editor).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let index = Index { host };
    assert!(
        index.nodes().is_empty(),
        "nothing was indexed: {:?}",
        index.nodes().iter().map(|n| &n.id).collect::<Vec<_>>()
    );
    assert!(
        index.raw("nodes").is_none(),
        "and nothing was written at all — not even an empty blob"
    );
}

// ---- OR.6: `:org-roam-find-node`, driven through a real picker ------------

/// Open the find-node picker and wait for its candidates to land.
///
/// The source's `init` is an async guest call, so `open_picker` returns before
/// the rows exist — the host resolves the future and seats them on a later
/// tick. Polling for them is the honest wait; asserting straight after the open
/// would be asserting on an empty picker.
async fn open_find_node(editor: &mut Editor) -> Vec<String> {
    // **Open ONCE.** `init` runs once per open, so re-opening to retry leaves
    // several inits in flight — and a late one re-seats the picker after the
    // caller has typed a query and accepted a row, which shows up as an accept
    // that quietly did nothing.
    //
    // Opening once is only correct because every caller settles on the `nodes`
    // BLOB first. `n/<id>` lands during the indexing batch; the blob the picker
    // reads is rebuilt once at the end of it, so settling on the per-id key
    // opens against a half-written index. That was the actual bug the re-open
    // loop was papering over.
    let _ = editor.open_picker("org-roam-node".to_string(), Vec::new());
    for _ in 0..settle_budget(200) {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        editor.run_tick_pending();
        let rows: Vec<String> = editor
            .picker
            .as_ref()
            .map(|p| p.candidates.iter().map(|c| c.raw.display.clone()).collect())
            .unwrap_or_default();
        if !rows.is_empty() {
            return rows;
        }
    }
    Vec::new()
}

/// Type `query` into the open picker, then read the rows back.
fn query_picker(editor: &mut Editor, query: &str) -> Vec<String> {
    if let Some(picker) = editor.picker.as_mut() {
        for ch in query.chars() {
            picker.append_query(ch);
        }
    }
    editor
        .picker
        .as_ref()
        .map(|p| p.candidates.iter().map(|c| c.raw.display.clone()).collect())
        .unwrap_or_default()
}

/// Every indexed node is offered, and a node is findable by its **title**.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn find_node_offers_the_indexed_notes_by_title() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(
        settle(|| index.nodes().len() >= 4).await,
        "the index is built"
    );

    let rows = open_find_node(&mut editor).await;
    assert!(
        rows.iter()
            .any(|r| r.contains("Honey Garlic Chicken Breast")),
        "the file node is offered: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("Async")),
        "and so is the headline node: {rows:?}"
    );

    let matched = query_picker(&mut editor, "Honey");
    assert!(
        matched
            .iter()
            .any(|r| r.contains("Honey Garlic Chicken Breast")),
        "typing a title narrows to it: {matched:?}"
    );
}

/// **Findable by ALIAS, not only by title.** 12% of the reference corpus is
/// reachable only this way, so an alias that is displayed but not matched is
/// the feature appearing to work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn find_node_matches_an_alias() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await);
    let _ = open_find_node(&mut editor).await;

    // `Honey Garlic` is an ALIAS of the note titled *Honey Garlic Chicken
    // Breast*; `Chicken` is its other alias. Query the second, which appears
    // nowhere in the title's leading words.
    let matched = query_picker(&mut editor, "Chicken");
    assert!(
        matched
            .iter()
            .any(|r| r.contains("Honey Garlic Chicken Breast")),
        "an alias matched: {matched:?}"
    );
}

/// **The picker never matches filenames.** The corpus's own slug is a fossil of
/// an earlier title, so a filename match would rank notes by what they used to
/// be called.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn find_node_does_not_match_filenames() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    // A note whose FILENAME says one thing and whose title says another — the
    // `20250603103551-chicken_breast_honey_garlic.org` shape, in miniature.
    std::fs::write(
        corpus.join("fossilised_old_name.org"),
        ":PROPERTIES:\n:ID: FOSSIL-1\n:END:\n#+title: Something Else Entirely\n",
    )
    .unwrap();

    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    // Settle on the BLOB, not on the per-id key. `n/<id>` lands during the
    // batch; `nodes` — the thing the picker actually reads — is rebuilt once at
    // the END of it. Waiting on the wrong key family opens the picker against
    // an index that is half-written.
    assert!(
        settle(|| index.nodes().iter().any(|n| n.id == "FOSSIL-1")).await,
        "the fossil note reached the blob the picker reads"
    );
    let rows = open_find_node(&mut editor).await;

    // **The positive half first.** Without it this test passes against an EMPTY
    // picker — a negative assertion is satisfied by nothing being there, which
    // is exactly how it passed while find-node was reporting itself
    // unconfigured for a corpus it had just indexed.
    assert!(
        rows.iter().any(|r| r.contains("Something Else Entirely")),
        "the note IS offered, under its title: {rows:?}"
    );

    let matched = query_picker(&mut editor, "fossilised");
    assert!(
        !matched
            .iter()
            .any(|r| r.contains("Something Else Entirely")),
        "…but its filename does not match: {matched:?}"
    );
}

/// **The create row is offered and is pinned last**, so `<CR>` on a query that
/// has a real match never creates a duplicate by ranking accident.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn find_node_offers_to_create_and_pins_it_last() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await);
    let _ = open_find_node(&mut editor).await;

    // A query that MATCHES something — the harder case, and the one that makes
    // the pin load-bearing.
    let rows = query_picker(&mut editor, "Honey");
    assert!(rows.len() > 1, "a real match survived alongside: {rows:?}");
    assert_eq!(
        rows.last().map(String::as_str),
        Some("Create note: Honey"),
        "the offer is last: {rows:?}"
    );
}

/// **Create produces a note buffer carrying a fresh id and the typed title.**
///
/// A buffer, NOT a file — and that distinction is the design rather than a
/// shortfall. `Effect::WriteToFile` resolves the path to a buffer, so a new
/// note is a draft the user finalizes, which is org-roam-capture's own model
/// and the reason an abandoned draft never enters the index: the watcher sees
/// it when it lands on disk, which is when the user saves.
///
/// This test asserted a file on disk in its first draft and failed against
/// exactly that design. The half it was reaching for — a note on disk becomes
/// findable — is `a_new_note_is_indexed_without_a_keypress`'s job.
///
/// **This was `#[ignore]`d through OR.6 and OR.7, and the product was never
/// broken.** The whole failure was this test looking the draft up through
/// `BufferStore::name_of`, which is the SYNTHETIC-name slot (`*messages*`,
/// `*lsp-log*`). A path-backed Document has no synthetic name, so the
/// predicate dropped every real file buffer — including the scratch buffer the
/// editor boots with. It would have reported "no draft" against any product,
/// working or not.
///
/// Worth the words because of how the search went. The symptom — an accept
/// producing no message at all, not the write's and not a refusal's — reads
/// like something swallowing a result, so four rounds of investigation looked
/// for the swallower: the masking echo, a stale `.wasm`, the re-opening
/// harness, the runtime, and finally `drain_pending_picker_accept`'s dropped
/// `.effects`. That last one is even true as a description of the code, and a
/// patch for it made this test pass — but reverting the patch left it passing,
/// because `handle_effect` already applies `Effect::WriteToFile` inline. An
/// assertion that cannot observe a success is indistinguishable from a product
/// that cannot produce one, and it makes every fix look plausible.
///
/// **OR.13 update, `#[ignore]`d again — this time for a REAL reason the four
/// rounds above had already found and let go of.** The `active_pane_buffer_id`
/// assertion below is the one this test always lacked; with it in place the
/// old "dead end" resurfaces as the live bug. `ROAM_CREATE_NODE`'s stub path
/// (`src/lib.rs`) now returns `[WriteToFile, OpenBufferAt]`, and confirmed via
/// a temporary `tracing` probe: BOTH effects reach
/// `drain_pending_picker_accept` (`lattice-host/src/dispatch.rs:~13436`) as
/// `Discriminant(4)` (`WriteToFile`) and `Discriminant(13)` (`OpenBufferAt`).
/// `WriteToFile` still lands because `apply_picker_outcome` already ran
/// `handle_effect` on it upstream (true, as the paragraph above says) — but
/// `OpenBufferAt` gets NO such upstream handling (`handle_effect`'s own
/// catch-all is `_ => {}`, `lattice-host/src/dispatch.rs:4346`), so it is
/// truly lost rather than redundantly re-applied. `drain_pending_picker_accept`'s post-loop
/// converts exactly one effect to a renderer signal, `Effect::OpenTransient`
/// (added for OR.11b's template picker); every other effect — now including
/// `OpenBufferAt` — falls to `other => tracing::debug!(...)` and is dropped.
/// This IS the same gap the paragraph above tried once: "a patch for it made
/// this test pass" was this exact class of fix, abandoned because the test's
/// OLD assertion could not tell a focused draft from an unfocused one.
///
/// This is a **host** fix (`lattice-host/src/dispatch.rs`), out of scope for
/// the plugin-only slice that owns this file — mirror the existing
/// `Effect::OpenTransient` arm: `do_edit(path, force)` +
/// `land_cursor_at(position)`, the same two calls the TUI renderer's own
/// `Effect::OpenBufferAt` arm makes (`lattice-ui-tui/src/app/dispatch.rs:1050`).
/// **That host fix landed as OR.16**, so this runs again — the arm calls
/// `do_edit` then `land_cursor_at`, exactly as predicted here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn creating_a_note_opens_a_draft_with_an_id_and_title() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await);
    let _ = open_find_node(&mut editor).await;

    // Type a title nothing matches, then accept — which selects the create row,
    // because it is the only row left.
    let rows = query_picker(&mut editor, "Zettelkasten");
    assert_eq!(
        rows,
        vec!["Create note: Zettelkasten"],
        "only the offer remains: {rows:?}"
    );
    // The accept's effects are the RENDERER's to apply — `Effect::WriteToFile`
    // among them. Dropping the outcome, as this test first did, is the same
    // mistake that made `<leader>onf` look like a dead chord.
    let out = editor.do_picker_accept();
    apply_accept_effects(&mut editor, out);
    // The accept is an async guest call whose outcome commits through
    // `drain_pending_picker_accept` — which `run_tick_pending` runs. Tick until
    // the draft appears rather than for a fixed count, so a slow machine does
    // not decide the result.
    // Look the draft up by its PATH, not by `name_of`.
    //
    // `name_of` is the SYNTHETIC-name slot — `*messages*`, `*lsp-log*` — and a
    // path-backed Document has none, so a `name_of` predicate drops every real
    // file buffer including the scratch one the editor booted with. This test
    // spent its whole ignored life asserting through that filter, which is why
    // a draft that was being created looked like a draft that never was.
    let draft = |editor: &Editor| -> Option<(lattice_core::BufferId, String)> {
        let mut ids = Vec::new();
        editor.buffers.for_each(|entry| ids.push(entry.id));
        ids.into_iter().find_map(|id| {
            let handle = editor.buffers.document_handle(id)?;
            let path = handle.snapshot().path.clone()?;
            if !path.to_string_lossy().contains("zettelkasten") {
                return None;
            }
            Some((id, lattice_runtime::Document::text(handle.as_ref())))
        })
    };
    let mut found = None;
    for _ in 0..settle_budget(240) {
        editor.run_tick_pending();
        found = draft(&editor);
        if found.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    let (draft_id, text) = found.unwrap_or_else(|| {
        panic!(
            "a draft buffer was opened (last message: {:?})",
            editor.last_message.as_ref().map(|m| &m.text)
        )
    });
    assert!(
        text.contains("#+title: Zettelkasten"),
        "the draft carries the typed title: {text:?}"
    );
    assert!(
        text.contains(":ID:"),
        "and a freshly minted id — without one it is not a node: {text:?}"
    );
    // OR.13: the registry assertions above prove the draft `Document` EXISTS —
    // they do not prove anything shows it. This is the assertion four rounds
    // of investigation lacked: the draft must also be what the active pane is
    // showing, not merely a buffer sitting in the registry unseen while the
    // user's pane stays on whatever it had open before `<CR>`.
    assert_eq!(
        editor.active_pane_buffer_id(),
        draft_id,
        "the draft is focused, not just created"
    );

    // The id is a real v4, not a placeholder: the mint happened on the grammar
    // seam, which is the whole reason `new-uuid` is host-side.
    let id_line = text
        .lines()
        .find(|l| l.trim_start().starts_with(":ID:"))
        .expect("an :ID: line");
    let id = id_line.split_whitespace().nth(1).unwrap_or_default();
    assert_eq!(id.split('-').count(), 5, "canonical 8-4-4-4-12: {id:?}");

    // And nothing was indexed, because nothing is on disk yet — the draft is a
    // draft. That is the half that makes an abandoned note cost nothing.
    assert!(
        !index.nodes().iter().any(|n| n.title == "Zettelkasten"),
        "an unsaved draft is not in the index"
    );
}

/// **Roam inert says so rather than showing an empty picker.** "No notes" and
/// "roam is not configured" look identical in an empty list and have entirely
/// different fixes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn find_node_with_no_directory_says_so() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);

    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("SKIP: org component not built");
        return;
    };
    let plugins = base.path().join("plugins");
    write_org_plugin_dir(&plugins, &wasm, &corpus);

    // Boot WITHOUT setting `org.roam-directory`.
    let mut editor = boot_sealed_editor();
    let host = Arc::new(
        PluginHost::with_dirs(base.path().join("cache"), base.path().join("data"))
            .expect("host builds"),
    );
    let loader = loader_over_editor(&editor, host.clone());
    for found in discover(&plugins) {
        loader
            .load_discovered(&found, TrustTier::Bundled)
            .await
            .unwrap();
    }

    let _ = editor.open_picker("org-roam-node".to_string(), Vec::new());
    // Unconditional drain — no condition to wait FOR, so a wider budget
    // would only cost wall clock. Deliberately unscaled.
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        editor.run_tick_pending();
    }
    let message = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        message.contains("org.roam-directory"),
        "the picker named the option to set rather than showing nothing: {message:?}"
    );
}

/// OR.7 — **the node completion source reaches the popup.**
///
/// The scar this is shaped against: PH7.6 gave a WASM completion source a WIT
/// export, an actor, a carrier mode and an `AsyncCompletionSource` adapter, the
/// loader registered all of it — and the host drove only LSP, so `generate` was
/// never called. Everything looked wired. Nothing completed. So this asserts on
/// the rows in the popup, which is the only place the whole chain is visible.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typing_a_link_opener_offers_the_indexed_nodes() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    let rows = complete_in_org(&mut editor, &corpus, "see [[").await;
    assert!(
        rows.iter().any(|r| r == "Honey Garlic Chicken Breast"),
        "the file node is offered as a completion: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r == "Async"),
        "and so is the headline node: {rows:?}"
    );
}

/// The source declines outside a link — the guard that keeps 500-node corpora
/// out of every ordinary word completion in an org buffer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_words_do_not_offer_nodes() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    // Positive control first: the same editor DOES offer nodes after `[[`, so
    // an empty result below cannot be "the source never ran".
    let inside = complete_in_org(&mut editor, &corpus, "see [[Hon").await;
    assert!(
        inside.iter().any(|r| r == "Honey Garlic Chicken Breast"),
        "control: inside a link the node is offered: {inside:?}"
    );

    let outside = complete_in_org(&mut editor, &corpus, "Hon").await;
    assert!(
        !outside.iter().any(|r| r == "Honey Garlic Chicken Breast"),
        "a bare word must not offer nodes: {outside:?}"
    );
}

/// Accepting a node writes an `[[id:…][title]]` link — the title is what was
/// matched, the link is what lands, and the opener already in the buffer is
/// not repeated.
///
/// Typed the way it is actually done: trigger at the opener, then keep typing.
/// The anchor is fixed when the popup opens, so a query typed after it grows
/// **across spaces** — which is the only way a multi-word title can be narrowed
/// to, and the reason the source insists the anchor sit at the opener.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepting_a_node_inserts_an_id_link() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    let rows = complete_in_org(&mut editor, &corpus, "see [[").await;
    assert!(
        rows.iter().any(|r| r == "Honey Garlic Chicken Breast"),
        "the node is offered at the opener: {rows:?}"
    );

    // Narrow by typing — through the real insert path, so the host re-derives
    // the query from the anchor exactly as it does for a user.
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_insert_text("Honey Garlic Chicken B", &mut out);
    let narrowed: Vec<String> = editor
        .insert_completion
        .as_ref()
        .map(|s| s.rendered.iter().map(|c| c.raw.display.clone()).collect())
        .unwrap_or_default();
    assert_eq!(
        narrowed,
        vec!["Honey Garlic Chicken Breast".to_string()],
        "a multi-word query narrows to the one node"
    );

    editor.do_completion_accept();

    let line = editor.document.snapshot().buffer.line(0).unwrap();
    assert_eq!(
        line, "see [[id:FILE-AAAA][Honey Garlic Chicken Breast]]",
        "the id link replaced the typed title, opener included exactly once"
    );
}

/// Open a scratch `.org` buffer holding `line`, put the cursor at its end in
/// Insert, fire the completion fan-out, and read the popup's rows back.
///
/// The anchor is what makes multi-word titles work and it is computed by the
/// host at trigger time, so the test types the whole line and triggers at the
/// end — the same order a user does it in, and the only order in which the
/// anchor lands where the source requires.
async fn complete_in_org(editor: &mut Editor, corpus: &Path, line: &str) -> Vec<String> {
    let file = corpus.join("draft.org");
    std::fs::write(&file, format!("{line}\n")).unwrap();
    editor.do_edit(Some(file), false);
    // The plugin's completion source rides a MINOR mode, and a minor is
    // enabled and then activated across two separate drains — so a buffer
    // opened without ticking between has the major and none of the plugin
    // minors. `ActiveCompletionSources` is rebuilt on each transition, and
    // reading it before both drains have run shows only the native sources.
    // Unconditional drain — no condition to wait FOR, so a wider budget
    // would only cost wall clock. Deliberately unscaled.
    for _ in 0..8 {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        editor.run_tick_pending();
    }
    editor.modal = lattice_grammar::ModalState::Insert;
    editor.cursor = lattice_protocol::Position::new(0, line.len() as u32);
    editor.do_completion_trigger();

    // The fan-out runs off-thread, so settle on the ASYNC ROUND finishing —
    // not on "any rows", which the sync buffer-words source satisfies
    // immediately and which would read every async result as absent.
    for _ in 0..settle_budget(100) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        editor.drain_pending_insert_completion_lsp();
        if editor.pending_insert_completion_async_token.is_none() {
            // The token clears on the first outcome; drain once more so a
            // second source landing in the same tick is not left behind.
            editor.drain_pending_insert_completion_lsp();
            break;
        }
    }
    editor
        .insert_completion
        .as_ref()
        .map(|s| s.rendered.iter().map(|c| c.raw.display.clone()).collect())
        .unwrap_or_default()
}

/// OR.6 — **`<leader>onf` opens find-node**, from a buffer that is not an org
/// file.
///
/// This binding was written once, appeared not to dispatch, and was reverted on
/// the principle that a chord which silently does nothing is worse than none.
/// The chord was fine; the harness was missing the two steps `org_structure.rs`
/// has carried since OM.4b/OM.7 — the spawned grammar-row expansion and the
/// enablement drain — so the mode was registered, disabled, and unreachable.
/// Three wrong explanations were tried before that one, which is why the fix is
/// in `index_corpus_with_editor` with a comment rather than here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_find_node_chord_opens_the_picker_from_any_buffer() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    // The scratch buffer the editor booted with — deliberately NOT an org file.
    // `org-global-mode` is Universal for capture's and the agenda's reason, and
    // finding a note has that reason too: the note you want is rarely the file
    // you are in.
    assert!(editor.picker.is_none(), "no picker before the chord");
    press_chord(&mut editor, "<leader>onf").await;

    let rows = settle_picker_rows(&mut editor).await;
    assert!(
        rows.iter()
            .any(|r| r.contains("Honey Garlic Chicken Breast")),
        "the chord opened find-node with the corpus in it: {rows:?}"
    );
}

/// Apply the effects an accept returns, the way the renderer does.
fn apply_accept_effects(editor: &mut Editor, out: lattice_host::dispatch::DispatchOutcome) {
    for effect in out.effects {
        match effect {
            lattice_grammar::Effect::WriteToFile {
                path,
                anchor,
                text,
                cut,
                create_parents,
            } => {
                editor.apply_write_to_file(path, anchor, text, cut, create_parents);
            }
            lattice_grammar::Effect::OpenPicker { source, args } => {
                let _ = editor.open_picker(source, args);
            }
            // OR.13: RENDERER-applied, like `OpenTransient` / `OpenSyntheticBuffer`
            // below — `handle_effect` deliberately does NOT apply it inline
            // (see `cross-file-writes.md` §2: `WriteToFile` must not steal
            // focus, so the effect that DOES focus a buffer is a separate,
            // renderer-side step). Mirrors the real renderer's arm
            // (`lattice-ui-tui/src/app/dispatch.rs`): `do_edit` switches the
            // active document, then `land_cursor_at` seats the cursor and
            // reveals the target. Dropping this arm — as the wildcard below
            // used to — makes a working focus-the-draft fix indistinguishable
            // from one that never focuses anything, the same failure mode the
            // `OpenSyntheticBuffer` comment already warns about.
            lattice_grammar::Effect::OpenBufferAt {
                path,
                position,
                force,
            } => {
                editor.do_edit(path, force);
                editor.land_cursor_at(position);
            }
            // OR.11b: the roam draft. RENDERER-applied, like `OpenTransient`
            // below — so dropping it here would make a working capture buffer
            // look like a create that did nothing, which is the failure mode
            // this helper exists to prevent.
            lattice_grammar::Effect::OpenSyntheticBuffer {
                name,
                mode_id,
                content,
                cursor,
                activate_minor,
            } => {
                editor.open_synthetic_buffer_seeded(
                    &name,
                    &mode_id,
                    content.as_deref(),
                    cursor,
                    activate_minor.as_deref(),
                );
            }
            // Closing the draft is part of both `C-c C-c` and `C-c C-k`.
            // Applied rather than ignored so "the draft is gone afterwards" is
            // a thing these tests can assert.
            lattice_grammar::Effect::BufferDelete { force } => {
                let _ = editor.do_buffer_delete(force);
            }
            // OR.11b: the roam template CHOOSER. Reached as a direct effect of
            // the create action rather than through `next_actions`, so it
            // needs its own arm — the caller polls `run_tick_pending`
            // afterwards, which is what seats a plugin menu's off-thread
            // `build`.
            lattice_grammar::Effect::OpenTransient { source, args } => {
                editor.open_named_transient(source, args);
            }
            // OR.17: a template that asks `%^{…}` questions opens this
            // directly — one question at a time, chained through
            // `on-submit-action` — rather than the chooser's row landing on a
            // second menu the way it did before OR.17.
            lattice_grammar::Effect::OpenPrompt {
                prompt,
                initial,
                on_submit_action,
                buffer_name,
            } => {
                editor.open_prompt_line(prompt, initial, on_submit_action, buffer_name);
            }
            other => {
                eprintln!("unapplied accept effect: {other:?}");
            }
        }
    }
    // The dispatched action's OWN outcome carries effects too, and dropping it
    // is invisible for `WriteToFile` — which `handle_effect` applies inline —
    // but fatal for `OpenTransient`, which is the renderer's to apply. The
    // create flow produces exactly that, so a working chooser looked like an
    // accept that did nothing.
    for action in out.next_actions {
        let nested = editor.dispatch(action);
        apply_accept_effects(editor, nested);
    }
}

/// Drain until the picker's async source has seated its rows.
async fn settle_picker_rows(editor: &mut Editor) -> Vec<String> {
    for _ in 0..settle_budget(100) {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        editor.run_tick_pending();
        let rows: Vec<String> = editor
            .picker
            .as_ref()
            .map(|p| p.candidates.iter().map(|c| c.raw.display.clone()).collect())
            .unwrap_or_default();
        if !rows.is_empty() {
            return rows;
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// OR.8 — `[[id:…]]` resolves, and `:org-roam-id-create` mints one.
//
// Driven through the REAL dispatch gate: `<CR>` is pressed in a real editor
// over a real indexed corpus, never against a hand-built `GrammarEnv`. The
// failure this slice most expects is the one OC.10 and OT.4 both hit — a seam
// wired end to end that answers nothing, because the gate synthesises a
// context no host test goes through.
// ---------------------------------------------------------------------------

/// Open `file` and put the cursor ON the link on `line`, with the drains a
/// plugin mode needs between the two.
///
/// The byte matters: `open_link` asks `links::link_at(text, cursor.byte)`, so a
/// cursor parked at column 0 of a line whose link starts at column 12 is not in
/// a link at all and `<CR>` correctly declines. A test that did that would read
/// as "following is broken" when following was never asked for.
async fn open_at_link(editor: &mut Editor, file: &Path, line: u32) {
    editor.do_edit(Some(file.to_path_buf()), false);
    // Unconditional drain — no condition to wait FOR, so a wider budget
    // would only cost wall clock. Deliberately unscaled.
    for _ in 0..8 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        editor.run_tick_pending();
    }
    let text = editor
        .document
        .snapshot()
        .buffer
        .line(line)
        .unwrap_or_default();
    let byte = text
        .find("[[")
        .unwrap_or_else(|| panic!("line {line} has no link: {text:?}")) as u32;
    editor.cursor = lattice_protocol::Position { line, byte };
}

/// Open `file` with the cursor at the start of `line` — for the paths that are
/// not about links.
async fn open_at(editor: &mut Editor, file: &Path, line: u32) {
    editor.do_edit(Some(file.to_path_buf()), false);
    // Unconditional drain — no condition to wait FOR, so a wider budget
    // would only cost wall clock. Deliberately unscaled.
    for _ in 0..8 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        editor.run_tick_pending();
    }
    editor.cursor = lattice_protocol::Position { line, byte: 0 };
}

/// The path the editor is currently showing, and the cursor's line.
fn where_am_i(editor: &Editor) -> (Option<String>, u32) {
    let path = editor
        .document
        .snapshot()
        .path
        .clone()
        .map(|p| p.to_string_lossy().to_string());
    (path, editor.cursor.line)
}

/// `<CR>` on a link to a FILE node opens that file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn following_an_id_link_opens_the_file_node() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    // `chicken.org` line 7 links to FILE-BBBB, which is `rust.org`'s file node.
    let chicken = corpus.join("chicken.org");
    open_at_link(&mut editor, &chicken, 7).await;
    press_chord(&mut editor, "<CR>").await;

    let (path, line) = where_am_i(&editor);
    let path = path.expect("a file is open");
    assert!(
        path.ends_with("rust.org"),
        "followed the id to its file: {path} (msg: {:?})",
        editor.last_message.as_ref().map(|m| m.text.clone())
    );
    assert_eq!(line, 0, "a FILE node lands at the top of its file");
}

/// `<CR>` on a link to a HEADLINE node lands on the headline, not on line 0.
///
/// 19% of the reference corpus is headline nodes; without the line every one of
/// them would arrive at the top of a file it shares with other notes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn following_an_id_link_lands_on_a_headline_nodes_line() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");
    let head = index
        .node("HEAD-CCCC")
        .expect("the headline node is indexed");
    assert!(head.line > 0, "fixture: the headline is not on line 0");

    let linking = corpus.join("link-to-headline.org");
    std::fs::write(
        &linking,
        ":PROPERTIES:\n:ID:       FILE-EEEE\n:END:\n#+title: Pointer\n\nSee [[id:HEAD-CCCC][async]].\n",
    )
    .unwrap();
    assert!(settle(|| index.nodes().len() >= 5).await, "reindexed");

    open_at_link(&mut editor, &linking, 5).await;
    press_chord(&mut editor, "<CR>").await;

    let (path, line) = where_am_i(&editor);
    assert!(path.expect("a file is open").ends_with("rust.org"));
    assert_eq!(line, head.line, "landed on the headline, not the file top");
}

/// An id that differs only in case still resolves — the store lowercases keys
/// because org ids are written however the tool that minted them felt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_id_resolves_whatever_its_case() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    let linking = corpus.join("lowercase-link.org");
    std::fs::write(
        &linking,
        ":PROPERTIES:\n:ID:       FILE-FFFF\n:END:\n#+title: Shouty\n\nSee [[id:file-bbbb][rust]].\n",
    )
    .unwrap();
    assert!(settle(|| index.nodes().len() >= 5).await, "reindexed");

    open_at_link(&mut editor, &linking, 5).await;
    press_chord(&mut editor, "<CR>").await;

    let (path, _) = where_am_i(&editor);
    assert!(
        path.expect("a file is open").ends_with("rust.org"),
        "a lowercase link resolved an uppercase id (msg: {:?})",
        editor.last_message.as_ref().map(|m| m.text.clone())
    );
}

/// An unknown id says so, and says something different from "no directory" and
/// from "index not built" — three problems with three different fixes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_id_names_itself_and_does_not_move_the_cursor() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    let linking = corpus.join("broken-link.org");
    std::fs::write(
        &linking,
        ":PROPERTIES:\n:ID:       FILE-GGGG\n:END:\n#+title: Broken\n\nSee [[id:NO-SUCH-ID][gone]].\n",
    )
    .unwrap();
    assert!(settle(|| index.nodes().len() >= 5).await, "reindexed");

    open_at_link(&mut editor, &linking, 5).await;
    press_chord(&mut editor, "<CR>").await;

    let (path, _) = where_am_i(&editor);
    assert!(
        path.expect("still here").ends_with("broken-link.org"),
        "a broken link does not move you somewhere arbitrary"
    );
    let msg = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("NO-SUCH-ID"),
        "the message names the id: {msg:?}"
    );
    assert!(
        !msg.contains("roam-directory") && !msg.contains("org-roam-sync"),
        "and does not send the reader to the wrong fix: {msg:?}"
    );
}

/// `:org-roam-id-create` gives the headline at point an `:ID:`, and running it
/// again is a no-op rather than a second drawer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn id_create_mints_once_and_declines_the_second_time() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    let file = corpus.join("plain.org");
    std::fs::write(&file, "#+title: Plain\n\n* A section\nbody\n").unwrap();
    open_at(&mut editor, &file, 2).await;

    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.execute_ex_line("org-roam-id-create", &mut out);
    apply_renderer_effects(&mut editor, out);

    let text = editor.document.snapshot().buffer.as_string();
    assert!(
        text.contains(":PROPERTIES:") && text.contains(":ID:"),
        "the headline got a drawer: {text:?}"
    );
    let first_id_count = text.matches(":ID:").count();
    assert_eq!(first_id_count, 1);

    // Again — a no-op, not a second drawer. org cannot read an entry carrying
    // two `:PROPERTIES:` blocks.
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.execute_ex_line("org-roam-id-create", &mut out);
    apply_renderer_effects(&mut editor, out);

    let text = editor.document.snapshot().buffer.as_string();
    assert_eq!(
        text.matches(":PROPERTIES:").count(),
        1,
        "no second drawer: {text:?}"
    );
    assert_eq!(text.matches(":ID:").count(), 1, "no second id: {text:?}");
    let msg = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(msg.contains("already has"), "and it says why: {msg:?}");
}

// ---------------------------------------------------------------------------
// OR.9 — `:org-roam-backlinks`: what points at the note you are in.
//
// A picker, not a multibuffer: backlinks is navigation. The read-in-place
// question is `plugin-multibuffer-views.md`'s, and MV.1 has since made it
// buildable — this stays a picker because that is what navigation wants, not
// because a view was impossible.
// ---------------------------------------------------------------------------

/// The corpus links `chicken.org` → FILE-BBBB (`rust.org`) and `rust.org`'s
/// headline node → FILE-AAAA (`chicken.org`). So each has exactly one backlink,
/// and they are different notes — a symmetric fixture would not catch a source
/// and target being swapped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backlinks_lists_the_notes_that_link_here() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    // In `rust.org`'s FILE node, which `chicken.org` links to.
    open_at(&mut editor, &corpus.join("rust.org"), 3).await;
    let rows = run_backlinks(&mut editor).await;

    assert!(
        rows.iter()
            .any(|r| r.contains("Honey Garlic Chicken Breast")),
        "the linking note is listed: {rows:?}"
    );
}

/// A node nothing points at gets an HONEST EMPTY view, not an error.
///
/// "Nothing links here yet" is a true and useful answer about a real note. That
/// is the opposite of the unconfigured case, which errors precisely because an
/// empty list there would be indistinguishable from "you have no notes".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_with_no_backlinks_is_empty_not_an_error() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    // `example.org` is linked from nowhere.
    open_at(&mut editor, &corpus.join("example.org"), 3).await;
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.execute_ex_line("org-roam-backlinks", &mut out);
    apply_renderer_effects(&mut editor, out);
    settle_picker_rows(&mut editor).await;

    let msg = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        !msg.contains("not inside a node"),
        "it IS a node — the message should not say otherwise: {msg:?}"
    );
    let rows: Vec<String> = editor
        .picker
        .as_ref()
        .map(|p| p.candidates.iter().map(|c| c.raw.display.clone()).collect())
        .unwrap_or_default();
    assert!(rows.is_empty(), "and it lists nothing: {rows:?}");
}

/// Outside a node, the command SAYS so rather than opening an empty picker.
///
/// "Nothing links here" and "you are not in a note" look identical in an empty
/// list and have entirely different fixes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outside_a_node_it_says_so() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    let plain = corpus.join("plain-no-ids.org");
    std::fs::write(&plain, "#+title: Plain\n\n* A headline\nbody\n").unwrap();
    open_at(&mut editor, &plain, 3).await;

    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.execute_ex_line("org-roam-backlinks", &mut out);
    apply_renderer_effects(&mut editor, out);

    assert!(editor.picker.is_none(), "no picker opened");
    let msg = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("not inside a node"),
        "and it says why: {msg:?}"
    );
}

/// Run `:org-roam-backlinks` and read the picker's rows back.
async fn run_backlinks(editor: &mut Editor) -> Vec<String> {
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.execute_ex_line("org-roam-backlinks", &mut out);
    apply_renderer_effects(editor, out);
    settle_picker_rows(editor).await
}

// ---------------------------------------------------------------------------
// OR.10 — dailies.
//
// The arithmetic is unit-tested in `src/roam_dailies.rs`; what only exists
// inside wasm is the pair of paths through the host: `read-file` deciding
// whether the entry is already there, and the two different effects that
// follow. Those are what these drive.
// ---------------------------------------------------------------------------

/// The text of the buffer whose path ends with `suffix`, or `None`.
///
/// By PATH, not by `name_of` — `name_of` is the SYNTHETIC-name slot, and a
/// path-backed Document has none, which is the filter that made
/// `creating_a_note_opens_a_draft_with_an_id_and_title` look broken for two
/// slices.
fn buffer_text_ending_with(editor: &Editor, suffix: &str) -> Option<String> {
    let mut ids = Vec::new();
    editor.buffers.for_each(|entry| ids.push(entry.id));
    ids.into_iter().find_map(|id| {
        let handle = editor.buffers.document_handle(id)?;
        let path = handle.snapshot().path.clone()?;
        if !path.to_string_lossy().ends_with(suffix) {
            return None;
        }
        Some(lattice_runtime::Document::text(handle.as_ref()))
    })
}

/// Run a dailies ex-command and let the effects land.
async fn run_dailies(editor: &mut Editor, line: &str) {
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.execute_ex_line(line, &mut out);
    apply_renderer_effects(editor, out);
    // Unconditional drain — no condition to wait FOR, so a wider budget
    // would only cost wall clock. Deliberately unscaled.
    for _ in 0..8 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        editor.run_tick_pending();
    }
}

/// The filename of the journal entry `offset` days from today, in LOCAL time.
///
/// From `chrono::Local`, which is the same source the host's
/// `local-utc-offset-seconds` is implemented on top of — so the test and the
/// guest agree about which day it is even when UTC disagrees with both.
/// Hardcoding a date would make these fail once a day; re-deriving the offset
/// from `SystemTime` would be a second implementation of the thing under test.
fn daily_name(offset: i64) -> String {
    let day = chrono::Local::now().date_naive() + chrono::Duration::days(offset);
    format!("{}.org", day.format("%Y-%m-%d"))
}

/// `:org-roam-dailies-today` creates the entry when it is absent — with an
/// `:ID:` and a `#+title:`, which is what makes a journal day linkable.
///
/// A BUFFER, not a file on disk: `Effect::WriteToFile` resolves the path to a
/// buffer, so a new entry is a draft the user finalizes. Same design and same
/// reason as `:org-roam-create-node`'s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dailies_today_creates_the_entry_when_it_is_absent() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    let name = daily_name(0);
    run_dailies(&mut editor, "org-roam-dailies-today").await;

    // The message on failure, not just the absence: this exact assertion has
    // failed twice for reasons the buffer list could not tell apart — a denied
    // `fs:write` grant and a refused `mkdir` both read as "no draft".
    let text = buffer_text_ending_with(&editor, &name).unwrap_or_else(|| {
        let msg = editor
            .last_message
            .as_ref()
            .map(|m| m.text.clone())
            .unwrap_or_default();
        panic!("today's journal draft `daily/{name}` was opened; last message: {msg:?}")
    });
    assert!(
        text.contains(":ID:"),
        "a new daily is a NODE — without an id the day cannot be linked to: {text:?}"
    );
    let stem = name.trim_end_matches(".org");
    assert!(
        text.contains(&format!("#+title: {stem}")),
        "and its title is the date: {text:?}"
    );
}

/// Running it twice OPENS rather than appending a second header.
///
/// This is the assertion that fails if existence is read from the roam index
/// instead of from `read-file`: the index lags the watcher's debounce, so a
/// file written a moment ago still reads as absent and the header lands twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dailies_today_opens_the_entry_that_is_already_there() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    // On disk BEFORE the command runs, and deliberately never indexed — the
    // whole point is that `read-file` sees it and the index does not have to.
    let name = daily_name(0);
    let dir = corpus.join("daily");
    std::fs::create_dir_all(&dir).unwrap();
    let existing =
        ":PROPERTIES:\n:ID:       ALREADY-HERE\n:END:\n#+title: yesterday's words\n\nkept\n";
    std::fs::write(dir.join(&name), existing).unwrap();

    run_dailies(&mut editor, "org-roam-dailies-today").await;

    let text = buffer_text_ending_with(&editor, &name)
        .unwrap_or_else(|| panic!("the existing `daily/{name}` was opened"));
    assert_eq!(
        text.matches(":ID:").count(),
        1,
        "opened, not appended to — a second `:ID:` is a file org cannot read: {text:?}"
    );
    assert!(
        text.contains("ALREADY-HERE") && text.contains("kept"),
        "and it is the user's file, untouched: {text:?}"
    );
}

/// Yesterday and tomorrow name the days either side of today. The month- and
/// year-boundary arithmetic is unit-tested; what this pins is that the two
/// commands are wired to the right shift and not, say, both to today.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn yesterday_and_tomorrow_are_the_days_either_side() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    run_dailies(&mut editor, "org-roam-dailies-yesterday").await;
    let yesterday = daily_name(-1);
    assert!(
        buffer_text_ending_with(&editor, &yesterday).is_some(),
        "yesterday's entry `daily/{yesterday}` was opened"
    );

    run_dailies(&mut editor, "org-roam-dailies-tomorrow").await;
    let tomorrow = daily_name(1);
    assert!(
        buffer_text_ending_with(&editor, &tomorrow).is_some(),
        "tomorrow's entry `daily/{tomorrow}` was opened"
    );
    assert_ne!(yesterday, tomorrow, "and they are different days");
}

/// `:org-roam-dailies-goto-date 2024-02-29` opens that day — an explicit date,
/// and a leap day, which is the one a month-length table gets wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn goto_date_opens_the_day_it_was_given() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    run_dailies(&mut editor, "org-roam-dailies-goto-date 2024-02-29").await;

    let text = buffer_text_ending_with(&editor, "2024-02-29.org")
        .expect("the leap day's entry was opened");
    assert!(
        text.contains("#+title: 2024-02-29"),
        "titled by the date asked for: {text:?}"
    );
}

/// A malformed date is refused BY THE `:` LINE, before any file is touched.
///
/// The refusal happening at parse time is the point: the text is still on
/// screen and editable there, whereas an echo after the fact scrolls away. And
/// nothing is created — a typo must not leave a file named after a day that
/// does not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_malformed_date_is_refused_and_creates_nothing() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    run_dailies(&mut editor, "org-roam-dailies-goto-date 2026-02-30").await;

    let msg = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("not a day that exists"),
        "the refusal says the day is impossible rather than blaming the format: {msg:?}"
    );
    assert!(
        buffer_text_ending_with(&editor, "2026-02-30.org").is_none(),
        "and no entry was opened for a day that does not exist"
    );
    assert!(
        !corpus.join("daily").join("2026-02-30.org").exists(),
        "and nothing was written to disk"
    );
}

/// With no date, `-goto-date` PROMPTS — pre-filled with today — and submitting
/// opens the day typed. This is the second hop `<leader>ondD` depends on, and
/// the hop that a plugin prompt has silently failed at before (OC.3a).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn goto_date_without_one_prompts_and_the_submit_opens_the_day() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await, "index is built");

    run_dailies(&mut editor, "org-roam-dailies-goto-date").await;

    let action = editor
        .pending_prompt_submit_action
        .clone()
        .expect("a bare `-goto-date` opens a prompt rather than guessing a day");
    assert_eq!(action, "org-roam-dailies-goto-date-submit");
    let stem = daily_name(0);
    assert_eq!(
        editor.prompt_line_text(),
        stem.trim_end_matches(".org"),
        "pre-filled with today, so the common edit is two keystrokes"
    );

    // Hop two: re-open with the typed text seeded and submit, which is what
    // `org_structure.rs`'s `submit_prompt` does for capture.
    editor.open_prompt_line(
        String::new(),
        "2025-11-04".to_string(),
        action,
        editor.pending_prompt_buffer_name.clone(),
    );
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_prompt_line_submit(&mut out);
    apply_renderer_effects(&mut editor, out);
    // Unconditional drain — no condition to wait FOR, so a wider budget
    // would only cost wall clock. Deliberately unscaled.
    for _ in 0..8 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        editor.run_tick_pending();
    }

    let text = buffer_text_ending_with(&editor, "2025-11-04.org")
        .expect("submitting the prompt opened the day typed into it");
    assert!(
        text.contains("#+title: 2025-11-04"),
        "titled by the date submitted: {text:?}"
    );
}

/// With `org.roam-directory` unset, dailies SAY so rather than creating a
/// journal wherever the editor happened to be started. Roam's inert contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dailies_refuse_when_roam_is_not_configured() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((_index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    editor
        .config
        .parse_and_set_command("org.roam-directory=")
        .expect("the option exists");

    run_dailies(&mut editor, "org-roam-dailies-today").await;

    let msg = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("roam-directory"),
        "the refusal names the option to set: {msg:?}"
    );
    assert!(
        buffer_text_ending_with(&editor, &daily_name(0)).is_none(),
        "and no journal was invented outside a corpus"
    );
}

// ---------------------------------------------------------------------------
// OR.4a — setting `org.roam-directory` builds the index, WITHOUT a manual sync.
//
// Every other test in this file calls `:org-roam-sync` by hand, and the
// harness comment says why: the option does not exist until org has loaded, so
// `init.rs` sets it from a `plugin-loaded` handler. What none of them noticed
// is that the same ordering breaks the PRODUCT, not just the test setup.
//
// `register_events` runs org's boot walk at plugin-load time. The user's
// `init.rs` sets `org.roam-directory` on `PluginLoaded`, which fires after. So
// the walk reads an unset option, indexes nothing, and — with nothing
// subscribed to `OptionChanged` — never runs again. A user following the
// documented deferred-config pattern gets `0/0` in the find-node picker
// forever, and the only way out is to know `:org-roam-sync` exists.
//
// The manual sync in every other test is exactly what hid it: a test that
// syncs by hand passes against a product that never syncs on its own. Same
// shape as `org-table-mode`'s hand-enable.
// ---------------------------------------------------------------------------

/// Boot + load org, set `org.roam-directory`, and DO NOT sync. The index must
/// build anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn setting_the_roam_directory_builds_the_index_without_a_manual_sync() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);

    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("SKIP: org component not built");
        return;
    };
    let plugins = base.path().join("plugins");
    write_org_plugin_dir(&plugins, &wasm, &corpus);

    let editor = boot_sealed_editor();
    let host = Arc::new(
        PluginHost::with_dirs(base.path().join("cache"), base.path().join("data"))
            .expect("host builds"),
    );
    let loader = loader_over_editor(&editor, host.clone());
    for found in discover(&plugins) {
        loader
            .load_discovered(&found, TrustTier::Bundled)
            .await
            .expect("the org plugin loads");
    }

    // The documented shape, and the ONLY step: set the option after the load,
    // exactly as an `init.rs` `plugin-loaded` handler does. No `:org-roam-sync`
    // anywhere below — that is the whole point.
    editor
        .config
        .parse_and_set_command(&format!("org.roam-directory={}", corpus.display()))
        .expect("the plugin registered `org.roam-directory`");

    let index = Index { host };
    assert!(
        settle(|| index.nodes().len() >= 4).await,
        "setting the directory must build the index on its own — a user who \
         follows the documented `plugin-loaded` config pattern never types \
         `:org-roam-sync`, and an empty picker is indistinguishable from a \
         corpus that has no notes in it. Got {} node(s).",
        index.nodes().len()
    );
}

/// **OR.4b — a real-sized corpus indexes without trapping.**
///
/// The bug this pins: `sync_all` did the entire cold walk inside ONE guest
/// call. The async seam's budget is `epoch_deadline: 1_000` — about a second —
/// and 706 files take roughly twenty-seven, so the guest ran past the deadline,
/// trapped, and the host quarantined the plugin for the session. The picker
/// then showed `0/0` forever and `:org-roam-sync` echoed "re-scanning" and
/// never finished, because a quarantined plugin never runs again.
///
/// **Every other test in this file uses a four-file corpus**, which finishes in
/// milliseconds — so the whole suite passed against a product that could not
/// index any real zettelkasten. The bug is a function of corpus SIZE, which is
/// exactly the axis none of the fixtures varied.
///
/// So this one writes enough files to need several batches. It is not the
/// 706-file corpus (a fixture that slow would not earn its place in the suite),
/// but it is comfortably more than one `BATCH`, which is what makes the chain
/// itself the thing under test: a single-batch corpus would pass against the
/// broken version too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_corpus_larger_than_one_batch_indexes_without_trapping() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    std::fs::create_dir_all(&corpus).unwrap();

    // 120 files — several times the 25-file batch, so the scan must hop at
    // least four times to finish.
    const FILES: usize = 120;
    for i in 0..FILES {
        std::fs::write(
            corpus.join(format!("note{i:03}.org")),
            format!(
                ":PROPERTIES:\n:ID:       BULK-{i:03}\n:END:\n#+title: Bulk Note {i}\n\nbody\n"
            ),
        )
        .unwrap();
    }

    let Some((index, _editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };

    // A longer window than `settle`'s 5s: this corpus is deliberately several
    // batches, and each batch is a full event round trip through the actor plus
    // 25 tree-sitter parses under a debug host. The point is that the chain
    // FINISHES, not that it finishes fast.
    let mut ok = false;
    for _ in 0..settle_budget(600) {
        if index.nodes().len() >= FILES {
            ok = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        ok,
        "every file must be indexed across however many batches it takes; the \
         single-call version trapped on the epoch deadline and quarantined the \
         plugin instead. Got {} of {FILES}.",
        index.nodes().len()
    );

    // And the plugin is still alive afterwards — a trap would have quarantined
    // it, which is the failure mode that made this invisible: the index being
    // empty and the plugin being dead look identical from the picker.
    let one = index
        .node("bulk-042")
        .expect("a node from the middle of the queue survived the chain");
    assert_eq!(one.title, "Bulk Note 42");
}

/// **The emacs `<C-c>` spellings reach the same commands as the
/// `<leader>o…` ones.**
///
/// Emacs org's own prefixes, letter for letter, kept alongside the vim-native
/// spelling rather than replacing it — three keystrokes instead of five for a
/// hand that already knows `C-c n f`, and `C-c a` / `C-c c` are the first two
/// lines of every org setup guide ever written.
///
/// Asserted through the real keymap rather than by reading the declaration: a
/// binding that is present in the mode's `keymap` and never expands into a
/// layer resolves to nothing, which is exactly how `<leader>onf` was once
/// believed broken and reverted (see this file's harness comment).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_emacs_prefix_reaches_the_same_roam_commands() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((_index, editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };

    // Each pair must land on the SAME command id — two spellings, one handler.
    for (emacs, vim) in [
        ("<C-c>nf", "<leader>onf"),
        ("<C-c>ndd", "<leader>ondd"),
        ("<C-c>ndy", "<leader>ondy"),
        ("<C-c>ndt", "<leader>ondt"),
        ("<C-c>ndD", "<leader>ondD"),
        ("<C-c>a", "<leader>oa"),
        ("<C-c>c", "<leader>oc"),
    ] {
        let resolve = |keys: &str| -> Option<String> {
            let expanded = editor.keymap.expand_leader(keys);
            let seq = lattice_protocol::parse_chord_sequence(&expanded).expect("parses");
            let modes: Vec<lattice_mode::ModeId> = editor
                .mode_registry
                .load()
                .iter_meta()
                .map(|(id, _)| id)
                .collect();
            editor
                .keymap
                .resolve_trace(lattice_keymap::BindingMode::Normal, &seq, &modes)
                .hits
                .last()
                .map(|h| format!("{:?}", h.command))
        };
        let a = resolve(emacs);
        let b = resolve(vim);
        assert!(
            a.is_some(),
            "`{emacs}` must resolve to a command — a declared binding that never \
             expands into a keymap layer reaches nothing"
        );
        assert_eq!(a, b, "`{emacs}` and `{vim}` must reach the same command");
    }
}

/// OR.11a + OR.11b — a note made from a template gets the template, expanded.
///
/// The bug this closes: `roam_capture.rs` was committed with `expand_fields`
/// and a full test module, and `mod roam_capture;` was never added to
/// `lib.rs`. The file did not compile, the function had no caller, and its
/// tests never ran — so a template's `#+title: ${title}` reached the user's
/// note verbatim. Tests passing in isolation is exactly why it looked done.
///
/// The flow is emacs org-roam's: pick a title the corpus does not have, choose
/// a template, and the note is written from it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_templated_note_expands_its_fields() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await);

    // A template using BOTH placeholder syntaxes, so the ordering is covered:
    // `${}` for the node being made, `%` for the capture context.
    // A single-line TOML basic string with `\n` escapes rather than a `"""`
    // block: a `:set` spec is one line by construction, and a block string in
    // it is the kind of thing that parses in a fixture file and not here.
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: concat!(
            "org.roam-capture-templates=[[template]]\n",
            "key = \"d\"\n",
            "description = \"Default note\"\n",
            "body = \":PROPERTIES:\\n:ID:       ${id}\\n:END:\\n",
            "#+title: ${title}\\n\\nfrom ${slug}\\n\"\n",
        )
        .to_string(),
    });
    assert!(
        editor
            .last_message
            .as_ref()
            .map(|m| !m.text.contains("expected") && !m.text.contains("error"))
            .unwrap_or(true),
        "the template set was accepted: {:?}",
        editor.last_message.as_ref().map(|m| &m.text)
    );

    let _ = open_find_node(&mut editor).await;
    let rows = query_picker(&mut editor, "Zettelkasten");
    assert_eq!(rows, vec!["Create note: Zettelkasten"], "{rows:?}");

    // Accept now opens the CHOOSER rather than writing — that is the flow
    // change, and asserting it here is what would catch a regression back to
    // writing the stub without asking.
    let out = editor.do_picker_accept();
    apply_accept_effects(&mut editor, out);
    for _ in 0..settle_budget(80) {
        editor.run_tick_pending();
        if editor
            .picker
            .as_ref()
            .and_then(|p| p.transient.as_ref())
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let spec = editor
        .picker
        .as_ref()
        .and_then(|p| p.transient.clone())
        .unwrap_or_else(|| {
            panic!(
                "the template chooser opened (last message: {:?})",
                editor.last_message.as_ref().map(|m| &m.text)
            )
        });
    assert!(
        spec.title.contains("Zettelkasten"),
        "the menu names the node it is making: {:?}",
        spec.title
    );

    // Pick the template.
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_transient_trigger("d".to_string(), &mut out);
    apply_accept_effects(&mut editor, out);

    // OR.11b: the draft is now the CAPTURE BUFFER, not a file. Asserting on
    // the synthetic buffer rather than on a path is the flow change — before
    // it, picking a template wrote the note straight to disk and there was
    // nothing to edit and nothing to abort.
    let text = settle_roam_draft(&mut editor).await.unwrap_or_else(|| {
        panic!(
            "the template opened a draft (last message: {:?})",
            editor.last_message.as_ref().map(|m| &m.text)
        )
    });

    assert!(
        !text.contains("${"),
        "no placeholder survives into the user's note — the whole bug: {text:?}"
    );
    assert!(
        text.contains("#+title: Zettelkasten"),
        "`${{title}}` became the typed title: {text:?}"
    );
    assert!(
        text.contains("from zettelkasten"),
        "`${{slug}}` became the slug: {text:?}"
    );
    let id_line = text
        .lines()
        .find(|l| l.trim_start().starts_with(":ID:"))
        .expect("`${id}` became an :ID: line");
    let id = id_line.split_whitespace().nth(1).unwrap_or_default();
    assert_eq!(id.split('-').count(), 5, "a real v4 id: {id:?}");

    // …and the note does not exist yet. The draft is a draft: `capture_effects`
    // does not run until `C-c C-c`, so there is no buffer at the note's path
    // and nothing on disk either.
    assert!(
        note_buffer(&editor, &corpus).is_none(),
        "an unfiled draft has produced no note"
    );

    finalize_roam_capture(&mut editor).await;
    let (path, filed) = note_buffer(&editor, &corpus).unwrap_or_else(|| {
        panic!(
            "`C-c C-c` filed the note (last message: {:?})",
            editor.last_message.as_ref().map(|m| &m.text)
        )
    });
    assert!(
        path.to_string_lossy().ends_with("-zettelkasten.org"),
        "…into the timestamped, slug-named file: {}",
        path.display()
    );
    assert!(
        filed.contains("#+title: Zettelkasten") && filed.contains(id),
        "…and it is the draft that was on screen, id and all: {filed:?}"
    );
}

/// OR.14 — a template's body can be READ FROM A FILE instead of inlined, the
/// way emacs org-roam's `(file "…/template.org")` does. This is also the
/// empirical answer to the capability question OR.14's brief asked for: the
/// harness below grants only `fs:write:{corpus}` — the same shape the real
/// `plugin.toml` grants over `~/src/dhruvasagar/org-files` — and the template
/// file lives under that root (`{corpus}/templates/source.org`), exactly as
/// Dhruva's real templates live under `org-files/roam/templates/`. If a write
/// grant did not also permit reading, this test would see the missing-file
/// warn path instead of a draft — it does not, so no separate `fs:read` grant
/// is needed for this reach.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_templated_note_reads_its_body_from_a_file() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await);

    let templates_dir = corpus.join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    let template_path = templates_dir.join("source.org");
    std::fs::write(
        &template_path,
        ":PROPERTIES:\n:ID:       ${id}\n:END:\n#+title: ${title}\n\nfrom ${slug}\n",
    )
    .unwrap();

    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!(
            concat!(
                "org.roam-capture-templates=[[template]]\n",
                "key = \"s\"\n",
                "description = \"Source\"\n",
                "body-file = \"{}\"\n",
            ),
            template_path.display()
        ),
    });
    assert!(
        editor
            .last_message
            .as_ref()
            .map(|m| !m.text.contains("expected") && !m.text.contains("error"))
            .unwrap_or(true),
        "the template set was accepted: {:?}",
        editor.last_message.as_ref().map(|m| &m.text)
    );

    let _ = open_find_node(&mut editor).await;
    let rows = query_picker(&mut editor, "Zettelkasten");
    assert_eq!(rows, vec!["Create note: Zettelkasten"], "{rows:?}");

    let out = editor.do_picker_accept();
    apply_accept_effects(&mut editor, out);
    for _ in 0..settle_budget(80) {
        editor.run_tick_pending();
        if editor
            .picker
            .as_ref()
            .and_then(|p| p.transient.as_ref())
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        editor
            .picker
            .as_ref()
            .and_then(|p| p.transient.as_ref())
            .is_some(),
        "the template chooser opened (last message: {:?})",
        editor.last_message.as_ref().map(|m| &m.text)
    );

    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_transient_trigger("s".to_string(), &mut out);
    apply_accept_effects(&mut editor, out);

    let text = settle_roam_draft(&mut editor).await.unwrap_or_else(|| {
        panic!(
            "the file-sourced template opened a draft (last message: {:?})",
            editor.last_message.as_ref().map(|m| &m.text)
        )
    });

    assert!(
        !text.contains("${"),
        "no placeholder survives into the user's note: {text:?}"
    );
    assert!(
        text.contains("#+title: Zettelkasten"),
        "the file's `${{title}}` became the typed title: {text:?}"
    );
    assert!(
        text.contains("from zettelkasten"),
        "the file's `${{slug}}` became the slug: {text:?}"
    );
    assert!(
        text.contains(":ID:") && !text.contains(":ID:       ${id}"),
        "the file's `${{id}}` became a minted id: {text:?}"
    );
}

/// A `body-file` naming a path that does not exist is a SKIP — a warn naming
/// the template and the path — never a trap, and never a note with nothing in
/// it. Same channel `roam_draft` already uses for "no template `{key}`" and
/// every other reason a note cannot be made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_body_file_is_skipped_not_a_trap() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await);

    // Under the write grant's prefix (so this is a "file is missing", not a
    // "capability denied" — the failure this test is actually about), but
    // never written to disk.
    let missing_path = corpus.join("templates").join("gone.org");

    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!(
            concat!(
                "org.roam-capture-templates=[[template]]\n",
                "key = \"s\"\n",
                "description = \"Source\"\n",
                "body-file = \"{}\"\n",
            ),
            missing_path.display()
        ),
    });

    let _ = open_find_node(&mut editor).await;
    let rows = query_picker(&mut editor, "Zettelkasten");
    assert_eq!(rows, vec!["Create note: Zettelkasten"], "{rows:?}");

    let out = editor.do_picker_accept();
    apply_accept_effects(&mut editor, out);
    for _ in 0..settle_budget(80) {
        editor.run_tick_pending();
        if editor
            .picker
            .as_ref()
            .and_then(|p| p.transient.as_ref())
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        editor
            .picker
            .as_ref()
            .and_then(|p| p.transient.as_ref())
            .is_some(),
        "the chooser still opens — a bad template does not hide the row"
    );

    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_transient_trigger("s".to_string(), &mut out);
    apply_accept_effects(&mut editor, out);
    // Unconditional drain — no condition to wait FOR, so a wider budget
    // would only cost wall clock. Deliberately unscaled.
    for _ in 0..8 {
        editor.run_tick_pending();
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    let message = editor
        .last_message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        message.contains("`s`") && message.contains("could not be read"),
        "names the template and says why: {message:?}"
    );

    let mut ids = Vec::new();
    editor
        .buffers
        .for_each(|entry| ids.push((entry.id, entry.name.clone())));
    assert!(
        !ids.iter().any(|(_, name)| name
            .as_deref()
            .unwrap_or_default()
            .starts_with("*org-roam-capture")),
        "no draft — and so no note — was ever opened from an unreadable template"
    );

    // Not a trap: the plugin is not quarantined, and a second create still
    // reaches the chooser (the broken template's row and all) rather than
    // going silent — the whole point of "skip, don't trap".
    let _ = open_find_node(&mut editor).await;
    let rows = query_picker(&mut editor, "Another Note");
    assert_eq!(rows, vec!["Create note: Another Note"], "{rows:?}");
    let out = editor.do_picker_accept();
    apply_accept_effects(&mut editor, out);
    for _ in 0..settle_budget(80) {
        editor.run_tick_pending();
        if editor
            .picker
            .as_ref()
            .and_then(|p| p.transient.as_ref())
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        editor
            .picker
            .as_ref()
            .and_then(|p| p.transient.as_ref())
            .is_some(),
        "the plugin survives a bad template and keeps offering the chooser"
    );
}

/// The buffer a roam create filed into, if there is one.
///
/// A note is filed through `Effect::WriteToFile`, which resolves the path to a
/// BUFFER rather than writing bytes — the note is unsaved until `:w`, which is
/// org-roam-capture's own model. So this looks for the buffer, not the file;
/// asserting on the filesystem here would fail on a working capture.
fn note_buffer(editor: &Editor, corpus: &Path) -> Option<(std::path::PathBuf, String)> {
    let mut ids = Vec::new();
    editor.buffers.for_each(|entry| ids.push(entry.id));
    ids.into_iter().find_map(|id| {
        let handle = editor.buffers.document_handle(id)?;
        let path = handle.snapshot().path.clone()?;
        if path.parent() != Some(corpus) {
            return None;
        }
        // The fixture's own files are written by `write_corpus`; only a created
        // note carries this slug.
        if !path.to_string_lossy().contains("zettelkasten") {
            return None;
        }
        Some((
            path.as_ref().clone(),
            lattice_runtime::Document::text(handle.as_ref()),
        ))
    })
}

/// Drain until the roam capture buffer exists, and return its text.
///
/// Found by NAME rather than by path: a capture buffer is synthetic, so
/// `document.path()` is `none` — which is the whole reason the destination is
/// remembered guest-side rather than recovered from the buffer.
async fn settle_roam_draft(editor: &mut Editor) -> Option<String> {
    for _ in 0..settle_budget(240) {
        editor.run_tick_pending();
        let mut found = None;
        let mut ids = Vec::new();
        editor
            .buffers
            .for_each(|entry| ids.push((entry.id, entry.name.clone())));
        for (id, name) in ids {
            if !name.unwrap_or_default().starts_with("*org-roam-capture") {
                continue;
            }
            if let Some(handle) = editor.buffers.document_handle(id) {
                found = Some(lattice_runtime::Document::text(handle.as_ref()));
            }
        }
        if found.is_some() {
            return found;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    None
}

/// `C-c C-c` in the roam draft: file it.
async fn finalize_roam_capture(editor: &mut Editor) {
    dispatch_capture_action(editor, "org-capture-finalize").await;
}

/// `C-c C-k` in the roam draft: throw it away.
async fn abort_roam_capture(editor: &mut Editor) {
    dispatch_capture_action(editor, "org-capture-abort").await;
}

/// The shared minor's chords are ONE pair for both capture and roam — which is
/// why these dispatch `org-capture-*` on a roam draft rather than a roam-
/// specific peer. A second pair would be the duplication the minor exists to
/// prevent.
async fn dispatch_capture_action(editor: &mut Editor, name: &str) {
    let id = editor
        .registry
        .load()
        .id_by_name(name)
        .unwrap_or_else(|| panic!("`{name}` is registered"));
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.dispatch_invocation(lattice_grammar::CommandInvocation::of(id), &mut out);
    apply_accept_effects(editor, out);
    // Unconditional drain — no condition to wait FOR, so a wider budget
    // would only cost wall clock. Deliberately unscaled.
    for _ in 0..40 {
        editor.run_tick_pending();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Configure `templates`, then drive the create flow for `title` as far as the
/// template chooser, and pick `key`.
///
/// The shared prefix of every OR.11b test below. What each of them asserts is
/// what happens AFTER the pick, which is the whole of what OR.11b changed.
async fn pick_roam_template(
    editor: &mut Editor,
    index: &Index,
    templates: &str,
    title: &str,
    key: &str,
) {
    assert!(settle(|| index.nodes().len() >= 4).await);
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!("org.roam-capture-templates={templates}"),
    });
    let _ = open_find_node(editor).await;
    let rows = query_picker(editor, title);
    assert_eq!(rows, vec![format!("Create note: {title}")], "{rows:?}");

    let out = editor.do_picker_accept();
    apply_accept_effects(editor, out);
    for _ in 0..settle_budget(80) {
        editor.run_tick_pending();
        if editor
            .picker
            .as_ref()
            .and_then(|p| p.transient.as_ref())
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_transient_trigger(key.to_string(), &mut out);
    apply_accept_effects(editor, out);
    // Wait for the OUTCOME, not for a fixed number of ticks. Choosing a
    // template arms the first `%^{...}` question's prompt (OR.17), and that is
    // what every caller goes on to assert — so waiting for it is both correct
    // and fast, where draining a fixed 40 ticks was a guess that held on an idle
    // machine and lost under a full `cargo test`. Both
    // `a_roam_template_asks_its_questions` and
    // `a_body_file_templates_questions_are_asked_too` failed exactly here.
    //
    // A template with no questions arms no prompt, so the budget is still the
    // floor: this exits early when there is something to wait for and behaves
    // as before when there is not.
    for _ in 0..settle_budget(40) {
        editor.run_tick_pending();
        if editor.pending_prompt_submit_action.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// OR.17: type `text` into the open question prompt and submit it, then apply
/// what the submit produced — which may be the NEXT question's prompt, or the
/// draft once the last one lands. The sequential peer of the old field-menu
/// row press.
async fn submit_roam_question(editor: &mut Editor, text: &str) {
    let action = editor
        .pending_prompt_submit_action
        .clone()
        .expect("a question prompt is open");
    let name = editor.pending_prompt_buffer_name.clone();
    editor.open_prompt_line(String::new(), text.to_string(), action, name);
    let mut out = lattice_host::dispatch::DispatchOutcome::default();
    editor.do_prompt_line_submit(&mut out);
    apply_accept_effects(editor, out);
    editor.run_tick_pending();
}

/// OR.11b — `C-c C-k` on a roam draft leaves NOTHING: no note, no buffer, and
/// no `:ID:` minted into anything.
///
/// This is the property the direct write could not have and the reason the
/// buffer surface is worth its wiring. Before OR.11b, choosing a template WAS
/// the write — there was no draft to abandon, so "I picked the wrong template"
/// cost you a file with a real id in it that the indexer would then pick up.
///
/// Asserted three ways because they fail independently: the note buffer must
/// not exist, the DRAFT buffer must be gone, and the corpus directory must hold
/// no new file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_aborted_roam_capture_leaves_nothing_behind() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    let before: Vec<_> = std::fs::read_dir(&corpus)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();

    pick_roam_template(
        &mut editor,
        &index,
        concat!(
            "[[template]]\n",
            "key = \"d\"\n",
            "description = \"Default note\"\n",
            "body = \":PROPERTIES:\\n:ID:       ${id}\\n:END:\\n#+title: ${title}\\n\"\n",
        ),
        "Zettelkasten",
        "d",
    )
    .await;
    assert!(
        settle_roam_draft(&mut editor).await.is_some(),
        "the draft opened, so there is something to abort"
    );

    abort_roam_capture(&mut editor).await;

    assert!(
        note_buffer(&editor, &corpus).is_none(),
        "an aborted capture files no note"
    );
    assert!(
        editor.buffers.by_name("*org-roam-capture:d*").is_none(),
        "…and the draft buffer is gone with it"
    );
    let after: Vec<_> = std::fs::read_dir(&corpus)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    assert_eq!(
        before.len(),
        after.len(),
        "…and nothing new is on disk: {after:?}"
    );
}

/// OR.11b — a roam template's `%^{Question}` is ASKED, and `%?` places the
/// caret. OR.17 changed HOW it is asked (one prompt at a time rather than a
/// fields menu); this test's assertions moved with it, but the bug it guards
/// is the same one.
///
/// Both were silently dropped before OR.11b: the roam path called
/// `capture::expand_with` with no answers and no typed text, so each expanded
/// to the empty string and vanished from the note. A template written around
/// `%^{Category}` produced a note with the field simply missing — a failure
/// with no error and no trace.
///
/// **There is no menu hop left to lose the id across.** Before OR.17 the
/// minted id crossed the boundary twice (once out to the fields menu, once
/// back with the answers), and the template was deliberately re-read between
/// those hops — so a re-minted id disagreeing with the menu's was a real way
/// to fail. OR.17 collapsed both hops into one guest-side flow struct, which
/// cannot disagree with itself; what is still worth pinning is that the id
/// reaching the draft looks like a real one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_roam_template_asks_its_questions() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };

    pick_roam_template(
        &mut editor,
        &index,
        concat!(
            "[[template]]\n",
            "key = \"c\"\n",
            "description = \"Concept\"\n",
            "body = \":PROPERTIES:\\n:ID:       ${id}\\n:END:\\n",
            "#+title: ${title}\\n#+category: %^{Category}\\n\\n* Summary\\n%?\\n\"\n",
        ),
        "Zettelkasten",
        "c",
    )
    .await;

    // OR.17: choosing the template asks its one question DIRECTLY — a prompt,
    // not a second menu — emacs's order (questions, then the buffer) and now
    // emacs's mechanism too.
    assert_eq!(
        editor.pending_prompt_submit_action.as_deref(),
        Some("org-capture-question-submit"),
        "the template's question opened a prompt: last message {:?}",
        editor.last_message.as_ref().map(|m| &m.text)
    );
    assert!(
        editor
            .last_message
            .as_ref()
            .is_some_and(|m| m.text.contains("Category")),
        "the prompt names the question: {:?}",
        editor.last_message.as_ref().map(|m| &m.text)
    );

    submit_roam_question(&mut editor, "concept").await;

    let text = settle_roam_draft(&mut editor).await.unwrap_or_else(|| {
        panic!(
            "the answer opened a draft (last message: {:?})",
            editor.last_message.as_ref().map(|m| &m.text)
        )
    });
    assert!(
        text.contains("#+category: concept"),
        "`%^{{Category}}` became the answer rather than vanishing: {text:?}"
    );
    let id_line = text
        .lines()
        .find(|l| l.contains(":ID:"))
        .expect("the ID drawer line is in the draft");
    let id = id_line
        .trim()
        .strip_prefix(":ID:")
        .expect("starts with :ID:")
        .trim();
    assert_eq!(id.split('-').count(), 5, "a real v4 id landed: {id:?}");

    finalize_roam_capture(&mut editor).await;
    let (_, filed) = note_buffer(&editor, &corpus).expect("`C-c C-c` filed the note");
    assert!(
        filed.contains("#+category: concept") && filed.contains(id),
        "…and what was filed is what was on screen: {filed:?}"
    );
}

/// OR.14 — a `body-file` template's `%^{Question}` is asked too, not just its
/// `${…}` fields. OR.17 moved the mechanism (a prompt chain rather than a
/// fields menu); this pins the same OR.14 bug against it.
///
/// The bug this guards against: the question-listing code once read
/// `template.body` directly, which is the EMPTY string for a `body_file`
/// template — the text lives on disk, not in that field. `roam_draft`'s own
/// scan resolves the file and correctly decides the note asks questions, so
/// the flow starts; but questions extracted from the wrong (empty) body would
/// silently ask NONE for a file whose questions it just promised to ask —
/// same failure shape as the original OR.11b bug this test's sibling
/// (`a_roam_template_asks_its_questions`) already guards, just reached through
/// a file instead of an inline string. `pkos-source.org`'s
/// `%^{Aliases (short title, acronym)}` is this exact shape in the reference
/// corpus.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_file_templates_questions_are_asked_too() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };

    let templates_dir = corpus.join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    let template_path = templates_dir.join("concept.org");
    std::fs::write(
        &template_path,
        ":PROPERTIES:\n:ID:       ${id}\n:END:\n\
         #+title: ${title}\n#+category: %^{Category}\n\n* Summary\n%?\n",
    )
    .unwrap();

    pick_roam_template(
        &mut editor,
        &index,
        &format!(
            concat!(
                "[[template]]\n",
                "key = \"c\"\n",
                "description = \"Concept\"\n",
                "body-file = \"{}\"\n",
            ),
            template_path.display()
        ),
        "Zettelkasten",
        "c",
    )
    .await;

    // The file's `%^{Category}` opened a prompt, same as an inline template's
    // would — the bug this test guards is exactly that it once did not.
    assert_eq!(
        editor.pending_prompt_submit_action.as_deref(),
        Some("org-capture-question-submit"),
        "last message: {:?}",
        editor.last_message.as_ref().map(|m| &m.text)
    );
    assert!(
        editor
            .last_message
            .as_ref()
            .is_some_and(|m| m.text.contains("Category")),
        "the prompt names the question read off disk: {:?}",
        editor.last_message.as_ref().map(|m| &m.text)
    );

    submit_roam_question(&mut editor, "concept").await;

    let text = settle_roam_draft(&mut editor).await.unwrap_or_else(|| {
        panic!(
            "the answer opened a draft (last message: {:?})",
            editor.last_message.as_ref().map(|m| &m.text)
        )
    });
    assert!(
        text.contains("#+category: concept"),
        "`%^{{Category}}` became the answer rather than vanishing: {text:?}"
    );
}

/// With NO templates configured, creating a note still writes the built-in
/// stub and shows no menu.
///
/// The back-compat half, and the reason the option defaults to empty: making
/// the feature depend on configuration would break note creation for every
/// user who has never heard of templates, to add a menu with one row in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_templates_a_note_is_still_the_built_in_stub() {
    let base = tempfile::tempdir().unwrap();
    let corpus = base.path().join("roam");
    write_corpus(&corpus);
    let Some((index, mut editor)) = index_corpus_with_editor(base.path(), &corpus).await else {
        eprintln!("SKIP: org component not built");
        return;
    };
    assert!(settle(|| index.nodes().len() >= 4).await);
    let _ = open_find_node(&mut editor).await;
    let _ = query_picker(&mut editor, "Zettelkasten");

    let out = editor.do_picker_accept();
    apply_accept_effects(&mut editor, out);
    // Unconditional drain — no condition to wait FOR, so a wider budget
    // would only cost wall clock. Deliberately unscaled.
    for _ in 0..40 {
        editor.run_tick_pending();
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        editor
            .picker
            .as_ref()
            .and_then(|p| p.transient.as_ref())
            .is_none(),
        "no menu is shown when nothing is configured"
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

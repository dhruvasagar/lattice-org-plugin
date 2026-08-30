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
             provides = [\"events\", \"modes\", \"grammar\", \"language\", \"config\"]\n\
             capabilities = [\"fs:read:{}\", \"state:write\"]\n\
             editor_capabilities = [\"tree-sitter\"]\n",
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
    let wasm = org_plugin_wasm()?;
    let plugins = base.join("plugins");
    write_org_plugin_dir(&plugins, &wasm, corpus);

    let editor = boot_sealed_editor();
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
    sync(&editor).await;
    Some(Index { host })
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
    for _ in 0..200 {
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

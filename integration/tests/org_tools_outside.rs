//! AT.14 — org's tools, reached by an agent outside the editor.
//!
//! The real component loads into a real editor, the editor's listener serves
//! its plugin tools, and an MCP client reaches them through the bridge that
//! `lattice --mcp` runs: it finds the editor by its lock, sends the token and
//! relays. What crosses is checked end to end:
//!
//!   - the agent lists org's tools under the names a chat uses, and nothing
//!     of lattice's own;
//!   - `org__agenda` answers with today's entries, read from the agenda files
//!     the editor has set;
//!   - a bad input comes back as a tool error the agent can act on, and the
//!     connection keeps working.
//!
//! The `lattice --mcp` binary itself is lattice's own test
//! (`the_mcp_bridge_serves_plugin_tools.rs`); this one is about org's tools.
//!
//! Skips when the component wasn't built (no `wasm32-wasip2` target).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use lattice_ai::llm::chat::LlmChatsHandle;
use lattice_ai::llm::tools::plugin::PluginToolsHandle;
use lattice_ai::mcp::tools_bridge;
use lattice_ai::mcp::tools_listener::ToolsListener;
use lattice_ai::mcp::tools_server::PluginToolsServer;
use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

fn component_cache() -> std::path::PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("component-cache")
}

/// See `org_major_mode.rs`: the seal against autoloading a developer's own
/// plugins.
fn boot_sealed_editor() -> Editor {
    lattice_plugin_loader::disable_autoload();
    Editor::boot(CoreDocument::from_text("scratch\n"))
}

fn org_plugin_wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    );
    std::fs::read(path)
        .ok()
        .map(|bytes| lattice_plugin_host::manifest::without_embedded_manifest(&bytes))
}

/// As `org_llm_tools.rs`: a write grant over `corpus`, which a tool call's
/// store narrows to read, and tree-sitter for parsing.
fn write_org_plugin_dir(root: &Path, wasm: &[u8], corpus: &Path) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "id = \"org\"\n\
             provides = [\"language\", \"config\", \"grammar\", \"help\", \"llm-tools\"]\n\
             capabilities = [\"fs:write:{}\"]\n\
             editor_capabilities = [\"tree-sitter\"]\n",
            corpus.display()
        ),
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();
}

fn loader_over_editor(editor: &Editor, base: &Path) -> PluginLoader {
    let host =
        Arc::new(PluginHost::with_dirs(component_cache(), base.join("data")).expect("host builds"));
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
            plugin_tools: editor
                .services
                .get::<PluginToolsHandle>()
                .map(|h| (*h).clone()),
            ..Default::default()
        },
    )
}

/// Today's date as the guest computes it (local), `YYYY-MM-DD`.
fn today_iso() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn today_stamp(rest: &str) -> String {
    format!("<{}{rest}>", chrono::Local::now().format("%Y-%m-%d %a"))
}

/// An MCP client on the far side of the bridge.
struct Agent {
    write: DuplexStream,
    read: BufReader<DuplexStream>,
}

impl Agent {
    async fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    /// The reply to request `id`, skipping notifications.
    async fn reply(&mut self, id: u64) -> Value {
        let wait = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let mut line = String::new();
                let n = self.read.read_line(&mut line).await.unwrap();
                assert!(n > 0, "the bridge closed before replying to {id}");
                let message: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("the bridge carries MCP only ({e}): {line}"));
                if message["id"] == json!(id) {
                    return message;
                }
            }
        });
        wait.await
            .unwrap_or_else(|_| panic!("no reply to {id} within a minute"))
    }

    async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        self.reply(id).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_outside_agent_reads_the_agenda_through_the_bridge() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: the org component is not built");
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("plan.org"),
        format!("* TODO Standup\n  SCHEDULED: {}\n", today_stamp(" 09:30")),
    )
    .unwrap();
    let plugins_dir = base.path().join("plugins");
    write_org_plugin_dir(&plugins_dir, &wasm, &notes);

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins_dir, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!("org.agenda-files={}", notes.display()),
    });

    // The editor's own broker and plugin tools, served as `llm.serve_tools`
    // serves them; the lock goes in the test's directory, not the user's cache.
    let chats = editor
        .services
        .get::<LlmChatsHandle>()
        .map(|c| (*c).clone())
        .expect("the llm layer is installed at boot");
    let plugins = editor
        .services
        .get::<PluginToolsHandle>()
        .map(|p| (*p).clone())
        .expect("plugin tools are installed at boot");
    let rules = {
        let chats = chats.clone();
        Arc::new(move || Some(chats.tool_rules()))
    };
    let server = PluginToolsServer::new(chats.tools().clone(), plugins, rules, 0);
    let locks = base.path().join("mcp");
    let (changed, _) = tokio::sync::broadcast::channel(4);
    let _listener = ToolsListener::start(server, base.path(), &locks, Arc::new(|_| {}), changed)
        .await
        .unwrap();

    // What `lattice --mcp` does, started inside the project.
    let found = tools_bridge::candidates(&locks, &notes);
    assert_eq!(found.len(), 1, "the editor's lock is found");
    let stream = tools_bridge::connect(&found).await.unwrap();
    let (agent_write, bridge_read) = tokio::io::duplex(1 << 16);
    let (bridge_write, agent_read) = tokio::io::duplex(1 << 16);
    tokio::spawn(tools_bridge::relay(stream, bridge_read, bridge_write));
    let mut agent = Agent {
        write: agent_write,
        read: BufReader::new(agent_read),
    };

    let init = agent
        .request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "outside-agent", "version": "0"}
            }),
        )
        .await;
    assert!(
        init["result"]["capabilities"]["tools"].is_object(),
        "{init}"
    );
    agent
        .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;

    let list = agent.request(2, "tools/list", json!({})).await;
    let mut names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("a tool list: {list}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "org__agenda",
            "org__change",
            "org__clock",
            "org__entry",
            "org__notes",
            "org__query"
        ],
        "org's tools under a chat's names, none of lattice's own"
    );

    let agenda = agent
        .request(
            3,
            "tools/call",
            json!({"name": "org__agenda", "arguments": {}}),
        )
        .await;
    assert_eq!(agenda["result"]["isError"], json!(false), "{agenda}");
    let text = agenda["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("Today is ") && text.lines().next().unwrap().contains(&today_iso()),
        "{text}"
    );
    assert!(text.contains("- 09:30 TODO Standup (scheduled)"), "{text}");
    assert!(text.contains("plan.org::1::Standup]"), "{text}");

    let bad = agent
        .request(
            4,
            "tools/call",
            json!({"name": "org__agenda", "arguments": {"from": "next week"}}),
        )
        .await;
    assert_eq!(
        bad["result"]["isError"],
        json!(true),
        "a bad input is a tool error the agent reads, not a protocol failure: {bad}"
    );

    let again = agent
        .request(
            5,
            "tools/call",
            json!({"name": "org__agenda", "arguments": {}}),
        )
        .await;
    assert_eq!(again["result"]["isError"], json!(false), "{again}");
}

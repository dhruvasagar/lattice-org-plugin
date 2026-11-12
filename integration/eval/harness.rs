//! One task, one model, one run: a fresh corpus, a real editor with org
//! loaded, and a chat on the task's profile that the model answers through
//! the same tool loop a user's chat runs.
//!
//! The chat is trusted (`<C-t>`) and the corpus is the project, so a change
//! applies without a review: the eval grades what the model asked for, not
//! whether someone pressed accept.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lattice_ai::conversation::{Block, ToolStatus};
use lattice_ai::llm::chat::LlmChatsHandle;
use lattice_ai::llm::config::LlmConfig;
use lattice_ai::llm::tools::plugin::PluginToolsHandle;
use lattice_ai::llm::tools::{CallContext, Offer};
use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};

use crate::corpus::Corpus;
use crate::tasks::{Call, Outcome, Task, REVIEW};

/// How long one turn may take before it counts as failed.
pub const TURN_LIMIT: Duration = Duration::from_secs(300);

/// What one run scored.
pub struct Run {
    pub verdict: Result<(), String>,
    /// Tool rounds: the exchanges after the first.
    pub rounds: usize,
    pub calls: usize,
    pub elapsed: Duration,
}

pub fn org_plugin_wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    );
    std::fs::read(path)
        .ok()
        .map(|bytes| lattice_plugin_host::manifest::without_embedded_manifest(&bytes))
}

fn component_cache() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("component-cache")
}

/// Org, granted the corpus, with the seams the tools and the roam index use.
fn write_org_plugin_dir(root: &Path, wasm: &[u8], corpus: &Path) -> std::io::Result<()> {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "id = \"org\"\n\
             provides = [\"events\", \"language\", \"config\", \"grammar\", \"help\", \"llm-tools\"]\n\
             capabilities = [\"fs:write:{}\", \"state:write\"]\n\
             editor_capabilities = [\"tree-sitter\"]\n",
            corpus.display()
        ),
    )?;
    std::fs::write(dir.join("component.wasm"), wasm)
}

fn loader_over_editor(editor: &Editor, base: &Path) -> Result<PluginLoader, String> {
    let host = PluginHost::with_dirs(component_cache(), base.join("data"))
        .map_err(|e| format!("the plugin host: {e}"))?;
    Ok(PluginLoader::with_services(
        Arc::new(host),
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
    ))
}

fn set(editor: &mut Editor, spec: String) {
    editor.handle_effect(lattice_grammar::Effect::SetOption { spec });
}

/// An editor with org over a fresh corpus, its roam index built.
struct Booted {
    _dir: tempfile::TempDir,
    corpus: Corpus,
    editor: Editor,
    chats: LlmChatsHandle,
    offer: Offer,
}

async fn boot(wasm: &[u8]) -> Result<Booted, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let base = dir.path().to_path_buf();
    let corpus = Corpus::write(&base.join("corpus")).map_err(|e| e.to_string())?;
    let plugins = base.join("plugins");
    write_org_plugin_dir(&plugins, wasm, &corpus.root).map_err(|e| e.to_string())?;

    lattice_plugin_loader::disable_autoload();
    let mut editor = Editor::boot(CoreDocument::from_text("scratch\n"));
    let loaded = loader_over_editor(&editor, &base)?
        .discover_and_load(&plugins, TrustTier::Bundled)
        .await;
    if loaded != 1 {
        return Err(format!("org did not load ({loaded} plugins loaded)"));
    }
    set(&mut editor, "org.todo-keywords=TODO WAITING | DONE".into());
    set(
        &mut editor,
        format!("org.agenda-files={}", corpus.agenda_dir().display()),
    );
    set(
        &mut editor,
        format!("org.roam-directory={}", corpus.roam_dir().display()),
    );
    editor.run_tick_pending();
    editor.event_bus.publish(lattice_protocol::Event::Plugin {
        name: "org/roam-sync".to_string(),
        payload: Vec::new(),
    });

    let chats = editor
        .services
        .get::<LlmChatsHandle>()
        .map(|c| (*c).clone())
        .ok_or("the llm layer is not installed")?;
    let offer = chats
        .tools()
        .offer(&BTreeSet::from(["org.*".to_string()]))
        .await;
    // The index lands on the plugin's event task: wait until the notes
    // tool sees it, or the notes task would grade the indexer.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let listed = offer
            .call(
                "org__notes",
                serde_json::json!({"query": "migration"}),
                CallContext::default(),
            )
            .await
            .map(|o| o.text)
            .unwrap_or_else(|e| e.to_string());
        if listed.contains("Database migration plan") {
            break;
        }
        if Instant::now() > deadline {
            return Err(format!("the roam index never listed the notes: {listed}"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    chats.set_project_root(base);
    Ok(Booted {
        _dir: dir,
        corpus,
        editor,
        chats,
        offer,
    })
}

/// The file the change task expects, made by `org.change` itself on a copy:
/// what a correct model's one call writes.
async fn expected_change(b: &Booted) -> Result<(PathBuf, String), String> {
    let real = b.corpus.projects();
    let text = std::fs::read_to_string(&real).map_err(|e| e.to_string())?;
    let copy_dir = b.corpus.root.join("expected");
    std::fs::create_dir_all(&copy_dir).map_err(|e| e.to_string())?;
    let copy = copy_dir.join("projects.org");
    std::fs::write(&copy, &text).map_err(|e| e.to_string())?;
    let line = text
        .lines()
        .position(|l| l.contains(REVIEW))
        .ok_or("the corpus has no review task")?
        + 1;
    let r = format!("{}::{line}::{REVIEW}", copy.display());
    b.offer
        .call(
            "org__change",
            serde_json::json!({"op": "set-state", "ref": r, "state": "DONE"}),
            CallContext {
                trusted: true,
                ..CallContext::default()
            },
        )
        .await
        .map_err(|e| format!("the expected change failed: {e}"))?;
    let want = std::fs::read_to_string(&copy).map_err(|e| e.to_string())?;
    std::fs::remove_dir_all(&copy_dir).map_err(|e| e.to_string())?;
    if want == text {
        return Err("the expected change changed nothing".to_string());
    }
    Ok((real, want))
}

/// The calls the chat made, in order, with what each returned.
fn calls_of(chats: &LlmChatsHandle) -> Vec<Call> {
    let mut out = Vec::new();
    for store in chats.stores() {
        let conv = store.snapshot();
        for turn in &conv.turns {
            for block in &turn.blocks {
                if let Block::ToolCall {
                    id, status, output, ..
                } = block
                {
                    let data = conv.tool_calls.get(id);
                    out.push(Call {
                        name: data.map(|d| d.name.clone()).unwrap_or_default(),
                        input: data.map(|d| d.input.clone()).unwrap_or_default(),
                        output: output.clone().unwrap_or_default(),
                        ok: *status == ToolStatus::Ok,
                    });
                }
            }
        }
    }
    out
}

/// Exchanges sent, over every request the chats made.
fn exchanges(chats: &LlmChatsHandle) -> usize {
    let requests = chats.requests();
    requests
        .snapshot()
        .iter()
        .map(|info| requests.sent(info.id).len())
        .sum()
}

/// Run `task` once on `model` (`provider/model`), the providers coming from
/// `config` (the text of a config file holding `[llm.providers]`).
pub async fn run(wasm: &[u8], config: &str, model: &str, task: &Task) -> Run {
    let started = Instant::now();
    let failed = |why: String| Run {
        verdict: Err(why),
        rounds: 0,
        calls: 0,
        elapsed: started.elapsed(),
    };
    let mut b = match boot(wasm).await {
        Ok(b) => b,
        Err(e) => return failed(format!("harness: {e}")),
    };
    let expected = if task.name == "mark-done" {
        match expected_change(&b).await {
            Ok(e) => Some(e),
            Err(e) => return failed(format!("harness: {e}")),
        }
    } else {
        None
    };

    // Through a file, as the user's own config is read: what a run uses is
    // what `:llm` would.
    let file = b.corpus.root.with_file_name("llm.toml");
    if let Err(e) = std::fs::write(&file, config) {
        return failed(format!("harness: {e}"));
    }
    let loaded = LlmConfig::load(&file);
    if !loaded.warnings.is_empty() {
        return failed(format!("harness: the config: {:?}", loaded.warnings));
    }
    b.chats.set_config(loaded);
    set(&mut b.editor, format!("llm.model={model}"));
    let before = b.corpus.files();
    let session = b.chats.create_with(Some(task.profile), None);
    session.toggle_trust();
    let turn = session.wait_turn_end();
    let started = Instant::now();
    b.chats.submit(&session, task.prompt.to_string());
    let ended = tokio::time::timeout(TURN_LIMIT, turn).await;
    let elapsed = started.elapsed();

    let calls = calls_of(&b.chats);
    let rounds = exchanges(&b.chats).saturating_sub(1);
    let verdict = match ended {
        Err(_) => Err(format!("no answer within {}s", TURN_LIMIT.as_secs())),
        Ok(Err(_)) => Err("the turn was dropped".to_string()),
        Ok(Ok(Err(e))) => Err(format!("the turn failed: {e}")),
        Ok(Ok(Ok(_))) => {
            let outcome = Outcome {
                answer: session.last_reply(),
                calls: calls.clone(),
                before,
                after: b.corpus.files(),
                expected,
            };
            (task.grade)(&outcome)
        }
    }
    .map_err(|why| {
        // What it called, so a failure can be read without rerunning it.
        let tried: Vec<String> = calls
            .iter()
            .map(|c| {
                let args: String = c.input.to_string().chars().take(120).collect();
                format!("{}({args}){}", c.name, if c.ok { "" } else { " ERR" })
            })
            .collect();
        format!("{why} [calls: {}]", tried.join(", "))
    });
    Run {
        verdict,
        rounds,
        calls: calls.len(),
        elapsed,
    }
}

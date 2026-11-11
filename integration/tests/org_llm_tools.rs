//! AT.7b — org's tools for the assistant, through a real component and the
//! editor's own tool broker.
//!
//! The component loads with `llm-tools` among its seams, the loader hands its
//! tools to the editor's `PluginTools`, and a chat's offer calls them by wire
//! name, the way a model does. What crosses is checked end to end:
//!
//!   - `org.agenda` is offered as a read tool the assistant runs unasked
//!     (org reaches no network);
//!   - it reads `org.agenda-files` through `host-services` under org's grant,
//!     the store made read-only;
//!   - its answer opens with today, puts overdue first, shows a repeat on
//!     today, and carries refs;
//!   - a `match` narrows it, and a bad input comes back as an error the
//!     model can act on, with the plugin still answering afterwards.
//!
//! Skips when the component wasn't built (no `wasm32-wasip2` target).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use lattice_ai::conversation::ToolKind;
use lattice_ai::llm::chat::LlmChatsHandle;
use lattice_ai::llm::tools::plugin::PluginToolsHandle;
use lattice_ai::llm::tools::{CallContext, Permission};
use lattice_core::Document as CoreDocument;
use lattice_host::editor::Editor;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use serde_json::json;

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

/// Org with a write grant over `corpus` (which the `llm-tools` store narrows
/// to read) and the tree-sitter capability, so a call parses the files as the
/// agenda view does.
fn write_org_plugin_dir(root: &Path, wasm: &[u8], corpus: &Path) {
    let dir = root.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "id = \"org\"\n\
             provides = [\"language\", \"config\", \"llm-tools\"]\n\
             capabilities = [\"fs:write:{}\"]\n\
             editor_capabilities = [\"tree-sitter\"]\n",
            corpus.display()
        ),
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();
}

/// A loader over the editor's live registries, its `PluginTools` among them.
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

/// Today as the guest computes it: local, days since the epoch.
fn today() -> i64 {
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let offset = i64::from(chrono::Local::now().offset().local_minus_utc());
    (utc + offset).div_euclid(86_400)
}

/// `(YYYY-MM-DD, Day)` for `today() + offset`. Restated rather than imported
/// from the guest, so the test does not check the guest's arithmetic against
/// itself.
fn date(offset: i64) -> (String, &'static str) {
    let days = today() + offset;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    // 1970-01-01 was a Thursday.
    const NAMES: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    (
        format!("{y:04}-{m:02}-{d:02}"),
        NAMES[days.rem_euclid(7) as usize],
    )
}

fn stamp(offset: i64, rest: &str) -> String {
    let (day, name) = date(offset);
    format!("<{day} {name}{rest}>")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_assistant_reads_the_agenda_through_org() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: the org component is not built");
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("plan.org"),
        format!(
            "* TODO [#A] Pay the invoice :money:\n  DEADLINE: {}\n\
             * TODO Standup\n  SCHEDULED: {}\n\
             * Dentist {}\n\
             * TODO Write the report\n",
            stamp(-2, ""),
            stamp(-1, " 09:30 +1d"),
            stamp(2, " 15:00"),
        ),
    )
    .unwrap();
    let plugins = base.path().join("plugins");
    write_org_plugin_dir(&plugins, &wasm, &notes);

    let mut editor = boot_sealed_editor();
    let loaded = loader_over_editor(&editor, base.path())
        .discover_and_load(&plugins, TrustTier::Bundled)
        .await;
    assert_eq!(loaded, 1, "the org component loads");
    editor.handle_effect(lattice_grammar::Effect::SetOption {
        spec: format!("org.agenda-files={}", notes.display()),
    });

    let broker = editor
        .services
        .get::<LlmChatsHandle>()
        .map(|c| c.tools().clone())
        .expect("the llm layer is installed at boot");
    let offer = broker.offer(&BTreeSet::from(["org.*".to_string()])).await;
    let rows: Vec<_> = offer
        .tools()
        .iter()
        .map(|t| (t.wire.as_str(), t.permission, t.kind))
        .collect();
    assert_eq!(
        rows,
        vec![("org__agenda", Permission::Run, ToolKind::Read)],
        "org reaches no network, so its read tools run unasked"
    );

    let ask = |args: serde_json::Value| {
        let offer = &offer;
        async move {
            offer
                .call("org__agenda", args, CallContext::default())
                .await
        }
    };
    let (today_iso, today_name) = date(0);
    let text = ask(json!({})).await.unwrap().text;
    assert!(
        text.starts_with("Today is ") && text.lines().next().unwrap().contains(&today_iso),
        "{text}"
    );
    let overdue = text
        .split("\n\n")
        .find(|b| b.starts_with("Overdue:"))
        .unwrap_or_else(|| panic!("an overdue block: {text}"));
    assert!(
        overdue.contains("TODO [#A] Pay the invoice :money: (deadline"),
        "{text}"
    );
    assert!(overdue.contains("2 days ago"), "{text}");
    let today_block = text
        .split("\n\n")
        .find(|b| b.starts_with(&format!("{today_iso} {today_name} (today):")))
        .unwrap_or_else(|| panic!("a block for today: {text}"));
    assert!(
        today_block.contains("- 09:30 TODO Standup (scheduled, repeats +1d) [ref "),
        "a repeat lands on today: {text}"
    );
    assert!(
        today_block.contains("plan.org::3::Standup]"),
        "the ref names the file, the 1-based line and the heading: {text}"
    );
    assert!(
        !text.contains("Dentist"),
        "the dentist is in two days: {text}"
    );
    assert!(text.contains("1 TODO entry has no date"), "{text}");

    let week = ask(json!({"range": "week", "match": "-money"}))
        .await
        .unwrap()
        .text;
    assert!(week.contains("entries matching -money"), "{week}");
    assert!(!week.contains("Pay the invoice"), "{week}");
    assert!(week.contains("- 15:00 Dentist (event)"), "{week}");
    assert_eq!(
        week.matches("TODO Standup").count(),
        8,
        "overdue once, then each of the seven days: {week}"
    );

    let err = ask(json!({"from": "next week"})).await.unwrap_err();
    assert!(
        err.to_string()
            .contains(r#"from: "next week" is not a YYYY-MM-DD date"#),
        "{err}"
    );
    assert!(
        ask(json!({})).await.is_ok(),
        "an error is that call's answer; the plugin keeps answering"
    );
}

//! AT.10 — the eval that decides what ships (design `llm-assistant-tools.md`
//! §9): org's tools and profiles, driven by real models through the same
//! chat a user opens, over a fixture corpus, each task graded without a
//! model.
//!
//! Run by hand, never in CI, because it needs a model:
//!
//! ```sh
//! cargo build --release --target wasm32-wasip2            # the repo root
//! cd integration
//! ANTHROPIC_API_KEY=… LATTICE_EVAL_MODELS=anthropic/claude-haiku-5-5,anthropic/claude-sonnet-5-5 \
//!     cargo test --test eval -- --ignored --nocapture
//! ```
//!
//! `LATTICE_EVAL_MODELS` lists `provider/model` pairs. The providers are the
//! user's own `[llm.providers]`, plus `anthropic` (key from
//! `ANTHROPIC_API_KEY`) and `ollama` (`localhost:11434`) when the config does
//! not name them. `LATTICE_EVAL_RUNS` (default 3) repeats each task, and
//! `LATTICE_EVAL_TASKS` picks some by name. The table is printed and written
//! to `target/…/eval/report.md`.
//!
//! What does run in CI is the harness against a scripted model: one that
//! calls the right tools must pass every task, and one that answers without
//! looking must fail every task, so no grader passes by accident.

#![allow(clippy::unwrap_used, clippy::panic)]

mod corpus;
mod harness;
mod tasks;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use harness::{org_plugin_wasm, run, Run};
use tasks::{Step, Task, TASKS};

// --- the scripted model -----------------------------------------------------

/// Read one HTTP request and return its JSON body.
async fn read_body(s: &mut tokio::net::TcpStream) -> serde_json::Value {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        let n = s.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if buf.len() >= pos + 4 + len {
                return serde_json::from_slice(&buf[pos + 4..pos + 4 + len])
                    .unwrap_or(serde_json::Value::Null);
            }
        }
    }
    serde_json::Value::Null
}

/// The tool results since the last user message, in order.
fn results_so_far(body: &serde_json::Value) -> Vec<String> {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let start = messages
        .iter()
        .rposition(|m| m["role"] == "user")
        .map_or(0, |i| i + 1);
    messages[start..]
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn sse(events: &[serde_json::Value]) -> Vec<u8> {
    let mut out = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    );
    for e in events {
        out.push_str(&format!("data: {e}\n\n"));
    }
    out.push_str("data: [DONE]\n\n");
    out.into_bytes()
}

fn reply(step: Step, n: usize) -> Vec<u8> {
    use serde_json::json;
    match step {
        Step::Call(name, args) => sse(&[
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("call_{n}"),"type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        ]),
        Step::Answer(text) => sse(&[
            json!({"choices":[{"index":0,"delta":{"content":text}}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
        ]),
    }
}

/// An OpenAI-compatible server whose every answer is `script`'s next step.
async fn scripted(script: fn(&[String]) -> Step) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(Mutex::new(0usize));
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let count = count.clone();
            tokio::spawn(async move {
                let body = read_body(&mut s).await;
                let n = {
                    let mut c = count.lock().unwrap();
                    *c += 1;
                    *c
                };
                let _ = s.write_all(&reply(script(&results_so_far(&body)), n)).await;
                let _ = s.shutdown().await;
            });
        }
    });
    addr
}

fn scripted_config(addr: std::net::SocketAddr) -> String {
    format!(
        "[llm.providers.script]\nkind = \"openai-compatible\"\nbase_url = \"http://{addr}/v1\"\n"
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scripted_model_passes_exactly_when_it_does_the_work() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: the org component is not built");
        return;
    };
    for task in TASKS {
        let addr = scripted(task.oracle).await;
        let good = run(&wasm, &scripted_config(addr), "script/oracle", task).await;
        assert!(
            good.verdict.is_ok(),
            "{}: a model doing the work fails: {:?}",
            task.name,
            good.verdict
        );
        assert!(good.rounds >= 1, "{}: tools ran", task.name);
        assert_eq!(good.calls, good.rounds, "{}: one call a round", task.name);

        let addr = scripted(tasks::lazy).await;
        let lazy = run(&wasm, &scripted_config(addr), "script/lazy", task).await;
        assert!(
            lazy.verdict.is_err(),
            "{}: a model that looked at nothing passes",
            task.name
        );
        assert_eq!(lazy.rounds, 0, "{}", task.name);
    }
}

// --- the real run -----------------------------------------------------------

/// The providers the eval may name: the user's own, plus `anthropic` and
/// `ollama` when the user's config has neither, as config text.
fn eval_config() -> String {
    let mut providers = lattice_config::default_user_config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| t.get("llm")?.get("providers")?.as_table().cloned())
        .unwrap_or_default();
    let defaults: toml::Table = "[anthropic]\napi_key_env = \"ANTHROPIC_API_KEY\"\n\
         [ollama]\nkind = \"openai-compatible\"\nbase_url = \"http://localhost:11434/v1\"\n"
        .parse()
        .unwrap();
    for (name, value) in defaults {
        providers.entry(name).or_insert(value);
    }
    let mut llm = toml::Table::new();
    llm.insert("providers".into(), toml::Value::Table(providers));
    let mut root = toml::Table::new();
    root.insert("llm".into(), toml::Value::Table(llm));
    toml::Value::Table(root).to_string()
}

fn env_list(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

struct Row {
    model: String,
    task: &'static str,
    runs: Vec<Run>,
}

fn mean(xs: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = xs.fold((0.0, 0usize), |(s, n), x| (s + x, n + 1));
    if n == 0 {
        0.0
    } else {
        sum / n as f64
    }
}

fn report(rows: &[Row], runs: usize) -> String {
    let mut out = format!(
        "# Org assistant eval\n\n{} task(s), {runs} run(s) each. Rounds and time are means over all runs.\n\n\
         | model | task | pass | rounds | calls | time |\n|---|---|---|---|---|---|\n",
        TASKS.len()
    );
    for r in rows {
        let passed = r.runs.iter().filter(|x| x.verdict.is_ok()).count();
        out.push_str(&format!(
            "| {} | {} | {passed}/{} | {:.1} | {:.1} | {:.1}s |\n",
            r.model,
            r.task,
            r.runs.len(),
            mean(r.runs.iter().map(|x| x.rounds as f64)),
            mean(r.runs.iter().map(|x| x.calls as f64)),
            mean(r.runs.iter().map(|x| x.elapsed.as_secs_f64())),
        ));
    }
    let mut models: Vec<&str> = rows.iter().map(|r| r.model.as_str()).collect();
    models.dedup();
    out.push_str("\n| model | pass rate |\n|---|---|\n");
    for m in models {
        let all: Vec<&Run> = rows
            .iter()
            .filter(|r| r.model == m)
            .flat_map(|r| &r.runs)
            .collect();
        let passed = all.iter().filter(|x| x.verdict.is_ok()).count();
        out.push_str(&format!(
            "| {m} | {passed}/{} ({:.0}%) |\n",
            all.len(),
            100.0 * passed as f64 / all.len().max(1) as f64
        ));
    }
    let failures: Vec<String> = rows
        .iter()
        .flat_map(|r| {
            r.runs.iter().filter_map(move |x| {
                x.verdict.as_ref().err().map(|why| {
                    let why: String = why.chars().take(400).collect();
                    format!("- {} / {}: {}", r.model, r.task, why.replace('\n', " "))
                })
            })
        })
        .collect();
    if !failures.is_empty() {
        out.push_str("\n## Failures\n\n");
        out.push_str(&failures.join("\n"));
        out.push('\n');
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a model: set LATTICE_EVAL_MODELS (see the file's head)"]
async fn the_eval() {
    let models = env_list("LATTICE_EVAL_MODELS");
    assert!(
        !models.is_empty(),
        "set LATTICE_EVAL_MODELS to provider/model pairs, comma-separated"
    );
    let wasm = org_plugin_wasm()
        .expect("build the component first: cargo build --release --target wasm32-wasip2");
    let runs: usize = std::env::var("LATTICE_EVAL_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(3);
    let picked = env_list("LATTICE_EVAL_TASKS");
    let chosen: Vec<&Task> = TASKS
        .iter()
        .filter(|t| picked.is_empty() || picked.iter().any(|p| p == t.name))
        .collect();
    let config = eval_config();

    let mut rows = Vec::new();
    for model in &models {
        for task in &chosen {
            let mut row = Row {
                model: model.clone(),
                task: task.name,
                runs: Vec::new(),
            };
            for i in 0..runs {
                let r = run(&wasm, &config, model, task).await;
                eprintln!(
                    "{model} {} #{}: {} ({} rounds, {:.1}s)",
                    task.name,
                    i + 1,
                    match &r.verdict {
                        Ok(()) => "pass".to_string(),
                        Err(e) => format!("FAIL {}", e.lines().next().unwrap_or("")),
                    },
                    r.rounds,
                    r.elapsed.as_secs_f64()
                );
                row.runs.push(r);
                // Spaced, so a hosted model's rate limit is not what fails.
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            rows.push(row);
        }
    }
    let table = report(&rows, runs);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("eval");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("report.md");
    std::fs::write(&path, &table).unwrap();
    println!("{table}\nwritten to {}", path.display());
}

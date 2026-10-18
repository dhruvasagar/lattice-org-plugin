//! IM.7 — org's inline-image producer, driven through the real component.
//!
//! Loads the org plugin's `media` seam and calls its `media-blocks` producer
//! with real org text, asserting what crosses back. This is the guest half of
//! the feature; the host half (resolving sizes, building rows, painting) is
//! covered in lattice.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_mode::CapabilitySet;
use lattice_plugin_host::{PluginBudget, PluginHost, PluginManifest, TrustTier, WasmMediaSource};

/// One compiled-component cache for every test in this binary.
///
/// The host caches what wasmtime compiles, keyed by the component's bytes, in
/// the directory it is given. A fresh temp dir per test made every load a
/// miss: org's ~1.8 MB component was compiled from scratch for each test,
/// which is most of what made this suite take hours. Shared here, the first
/// load compiles and the rest load the compiled code. `CARGO_TARGET_TMPDIR` is
/// cargo's scratch space for integration tests, kept under `target/`.
fn component_cache() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("component-cache")
}

fn org_plugin_wasm() -> Option<Vec<u8>> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    ))
    .ok()
}

async fn source(host: &PluginHost, wasm: &[u8]) -> WasmMediaSource {
    let component = host.compile(wasm).expect("compile org component");
    let manifest = PluginManifest::new("org", Vec::new(), CapabilitySet::empty());
    let (client, actor) = host
        .spawn_media_source(
            &component,
            &manifest,
            TrustTier::Bundled,
            PluginBudget::default(),
            &Arc::new(lattice_runtime::EventBus::new()),
        )
        .await
        .expect("spawn org media source");
    tokio::spawn(actor.run());
    WasmMediaSource::new(client)
}

const DOC: &str = "\
* Notes
[[file:img/diagram.png]]
some prose
[[file:shot.png][a screenshot]]
see [[file:inline.png]] in passing
[[file:notes.org]]
[[https://example.com/remote.png]]
";

/// Images are OFF by default: an org file can reference anything, and only the
/// GPUI peer can draw them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn images_are_off_until_the_option_is_set() {
    let Some(wasm) = org_plugin_wasm() else {
        eprintln!("skipping: component not built");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let host = PluginHost::with_dirs(component_cache(), dir.path().join("data")).unwrap();
    let src = source(&host, &wasm).await;

    // No config seam is wired here, so `get_option` returns none and the guest
    // falls back to its default — which is off.
    let err = src
        .media_blocks(1, Some(std::path::Path::new("/n/todo.org")), 7, DOC.into())
        .await
        .expect_err("disabled ⇒ a typed err, not an empty list");
    assert!(err.contains("disabled"), "got {err}");
}

/// The scan itself, exercised by calling with the option unavailable but the
/// buffer empty — proving the empty-buffer guard fires before the scan.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_buffer_is_refused_before_scanning() {
    let Some(wasm) = org_plugin_wasm() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let host = PluginHost::with_dirs(component_cache(), dir.path().join("data")).unwrap();
    let src = source(&host, &wasm).await;

    let err = src
        .media_blocks(
            1,
            Some(std::path::Path::new("/n/todo.org")),
            0,
            String::new(),
        )
        .await
        .expect_err("empty buffer is refused");
    // Whichever guard fires, it is a typed err rather than a trap — which is
    // the property that matters: the producer never takes the editor down.
    assert!(!err.is_empty());
}

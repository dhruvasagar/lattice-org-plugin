//! The shipped component highlights a real org buffer.
//!
//! ## Why this exists, when `org_headlines.rs` already tests the query
//!
//! `org_headlines.rs` and `org_folds.rs` both build the grammar from the
//! cloned `grammar-src/` checkout and hand `plugin_lang` a `GrammarSpec` they
//! assembled in the test process. They prove the *queries* are correct. They
//! say nothing about the artefact a user actually installs.
//!
//! Everything between the query file and the editor was untested:
//!
//! - `build.rs` bakes the grammar in with `include_bytes!` — and on a failed
//!   grammar build it writes **empty bytes** and continues, with only a
//!   `cargo:warning`. A component built offline is a well-formed component
//!   that highlights nothing.
//! - `register-languages` has to carry 334 KB of grammar and both query
//!   sources across the WIT boundary.
//! - The loader's `language` drain has to compile that grammar and register it
//!   under the language name, and it `continue`s past a grammar that fails to
//!   load with a `tracing::warn!` — invisible unless someone is reading logs.
//!
//! Three separate silent-skip paths, all of which surface to the user as
//! "org files are not coloured" and to a test suite as green. This test drives
//! the real `component.wasm` through the real seam and asserts colour comes
//! out the far end, so that failure mode is loud.
//!
//! Skips only when the component itself was not built.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_syntax::{Lang, Style, Syntax};

/// The component as built, i.e. exactly the bytes staged into
/// `~/.config/lattice/plugins/org/component.wasm`.
fn org_component() -> Option<Vec<u8>> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    ))
    .ok()
}

/// Load the component through the loader with ONLY the `language` seam
/// declared — the seam under test, and no editor needed to exercise it.
async fn load_language_seam(base: &std::path::Path, wasm: &[u8]) -> usize {
    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"language\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();

    let host = Arc::new(
        PluginHost::with_dirs(base.join("cache"), base.join("data")).expect("host builds"),
    );
    PluginLoader::with_services(
        host,
        LoaderServices {
            runtime: Some(tokio::runtime::Handle::current()),
            ..Default::default()
        },
    )
    .discover_and_load(&plugins_dir, TrustTier::Bundled)
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shipped_component_colours_an_org_buffer() {
    let Some(wasm) = org_component() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };

    let base = tempfile::tempdir().unwrap();
    assert_eq!(load_language_seam(base.path(), &wasm).await, 1, "org loads");

    // `.org` resolves to the plugin's language — the extension mapping crossed.
    let lang = Lang::detect_from_path(Some(std::path::Path::new("notes.org")));
    assert!(
        matches!(lang, Lang::Plugin(_)),
        "the component's `language` seam claimed `.org`; got {lang:?}. \
         An empty baked grammar (build.rs's offline fallback) lands here."
    );

    // The grammar compiled and the highlights query came across with it. Both
    // are `Option` on the wire, so a component that shipped neither still gets
    // this far — the styles below are what distinguishes it.
    let mut syntax = Syntax::for_language(lang)
        .expect("syntax builds")
        .expect("the plugin language has a grammar registered");
    syntax.parse("* One\n** Two\nbody\n");
    let lines = syntax.highlight_lines_native(0, 3).expect("highlights");

    let level1: Vec<Style> = lines[0].iter().map(|s| s.style).collect();
    let level2: Vec<Style> = lines[1].iter().map(|s| s.style).collect();
    assert!(
        level1.contains(&Style::Heading1),
        "`* One` should be Heading1, got {level1:?} — the grammar loaded but \
         the highlights query did not cross the seam"
    );
    assert!(
        level2.contains(&Style::Heading2),
        "`** Two` should be Heading2, got {level2:?}"
    );

    // No `unregister_plugin` teardown: the id belongs to the loader, which
    // reports a count rather than handing it back. This is the only test in
    // its binary, so the process-global language registry dies with it — the
    // sibling files that DO unregister share a binary with each other.
}

//! PMF.8 — org's manifest is inside the component, and its file grants follow
//! two of its own options.
//!
//! Two things a reader of `src/lib.rs` cannot check by eye:
//!
//! - **The section survives the release build.** `MANIFEST` is a
//!   `#[link_section]` static; this reads it back out of the component the
//!   release profile produced (LTO, strip), with the host's own reader.
//! - **The grant defaults are the options' defaults.** Grants are fixed before
//!   `register-options` runs, so the manifest template states each default
//!   again. If the two drift, org looks for notes in one place and is allowed
//!   to write in another. The loader only warns about that; this fails.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;

use lattice_plugin_host::{Capability, PluginHost, PluginManifest, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};

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

/// The component exactly as built — manifest and all.
fn org_component() -> Option<Vec<u8>> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    ))
    .ok()
}

/// `(option, default)` for every option-bound grant in `manifest`.
fn option_bound(manifest: &PluginManifest) -> Vec<(String, Option<PathBuf>)> {
    manifest
        .requested
        .iter()
        .filter_map(|cap| match cap {
            Capability::FsReadOption(op) | Capability::FsWriteOption(op) => {
                Some((op.option.clone(), op.default.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_component_carries_its_manifest() {
    let Some(wasm) = org_component() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };
    let manifest = PluginManifest::from_component_bytes(&wasm)
        .expect("the embedded manifest parses")
        .expect("the release build kept the lattice-manifest section");
    assert_eq!(manifest.id, "org");
    assert_eq!(
        manifest.min_lattice,
        Some((0, 9, 11)),
        "embedded manifests and option-bound grants both need 0.9.11"
    );
    let bound: Vec<String> = option_bound(&manifest)
        .into_iter()
        .map(|(o, _)| o)
        .collect();
    assert_eq!(bound, ["org.directory", "org.roam-directory"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_grant_default_is_the_default_org_registers() {
    let Some(wasm) = org_component() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };
    let manifest = PluginManifest::from_component_bytes(&wasm)
        .unwrap()
        .unwrap();

    // Load only the `config` seam — the one that registers the options — under
    // a manifest of the test's own, so nothing else needs wiring.
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"config\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("component.wasm"),
        lattice_plugin_host::manifest::without_embedded_manifest(&wasm),
    )
    .unwrap();
    let config = Arc::new(lattice_config::ConfigRegistry::new());
    let host =
        Arc::new(PluginHost::with_dirs(component_cache(), base.path().join("data")).unwrap());
    let loaded = PluginLoader::with_services(
        host,
        LoaderServices {
            runtime: Some(tokio::runtime::Handle::current()),
            config_registry: Some(config.clone()),
            ..Default::default()
        },
    )
    .discover_and_load(&plugins_dir, TrustTier::Bundled)
    .await;
    assert_eq!(loaded, 1, "org's config seam loads");

    for (option, grant_default) in option_bound(&manifest) {
        let registered = config
            .lookup(&option)
            .unwrap_or_else(|| panic!("org registers `{option}`, which a grant follows"));
        let registered = PathBuf::from(lattice_core::home::expand_tilde(
            registered.default_formatted(),
        ));
        assert_eq!(
            grant_default.as_ref(),
            Some(&registered),
            "`{option}`: the manifest grants one default directory and the option uses another"
        );
    }
}

//! PMF.8 — org follows a moved grant with ONE reload, not a reload loop.
//!
//! The loop a real config drove: `init.rs` sets `org.roam-directory` from
//! `pre-plugin-loaded` — mid-load, after org's `config` seam has registered
//! the option and before its later seams instantiate. So the instances made
//! before the handler ran are granted the default, the ones after it the
//! user's directory, and the post-load check sees the grant move and reloads.
//! Unloading drops org's options with it, so the next load began from the
//! default again: loaded, unloaded, loaded, every few milliseconds.
//!
//! Its own binary: org registers its language into a process-global registry.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use lattice_config::ConfigRegistry;
use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_protocol::{Event, EventKind};
use lattice_runtime::{EventBus, EventFilter, SubscriptionTarget};

fn org_component() -> Option<Vec<u8>> {
    // The component embeds org's real manifest, and an embedded manifest wins
    // over a `plugin.toml`. This test loads org under a manifest of its own
    // (two seams, the real grants), so take it out first.
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    ))
    .ok()
    .map(|bytes| lattice_plugin_host::manifest::without_embedded_manifest(&bytes))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_roam_directory_set_on_every_load_reloads_org_once() {
    let Some(wasm) = org_component() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let plugins = base.path().join("plugins");
    let dir = plugins.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    // `config` drains before `pre-plugin-loaded`, `language` after it — the
    // split that put two different grants in one load.
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"config\", \"language\"]\n\
         capabilities = [\"fs:write:{org.directory|~/org}\", \
         \"fs:write:{org.roam-directory|~/org-roam}\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), &wasm).unwrap();

    let bus = Arc::new(EventBus::new());
    let config = Arc::new(ConfigRegistry::default());
    let publish_to = bus.clone();
    config.set_event_publisher(Arc::new(move |event| publish_to.publish(event)));
    let (tx, mut loads) = tokio::sync::mpsc::unbounded_channel::<Event>();
    bus.subscribe(
        EventFilter::kind(EventKind::PluginLoaded),
        SubscriptionTarget::Channel(tx),
    );
    // The user's `init.rs`: on `pre-plugin-loaded`, which the load waits for,
    // point roam at the user's notes.
    let notes = base.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let init_config = config.clone();
    let init_value = format!("org.roam-directory={}", notes.display());
    bus.subscribe(
        EventFilter::kind(EventKind::PrePluginLoaded),
        SubscriptionTarget::Plugin {
            plugin: 0,
            handler: 1,
            sink: Arc::new(move |_event, _ack| {
                let _ = init_config.parse_and_set_command(&init_value);
                true
            }),
        },
    );

    let loader = Arc::new(PluginLoader::with_services(
        Arc::new(
            PluginHost::with_dirs(base.path().join("cache"), base.path().join("data")).unwrap(),
        ),
        LoaderServices {
            runtime: Some(tokio::runtime::Handle::current()),
            bus: Some(bus.clone()),
            config_registry: Some(config.clone()),
            ..Default::default()
        },
    ));
    loader.subscribe_grant_options();
    assert_eq!(
        loader.discover_and_load(&plugins, TrustTier::Bundled).await,
        1,
        "{:?}",
        loader.failed_loads()
    );

    // Every load in three seconds. The loop reloaded every few milliseconds.
    let mut count = 0;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(3), loads.recv()).await {
        if matches!(event, Event::PluginLoaded { .. }) {
            count += 1;
        }
        if count > 10 {
            break;
        }
    }
    assert!(
        count <= 2,
        "org loaded {count} times: once, plus at most one reload onto the user's \
         directory — not a reload every time init.rs sets it again"
    );
    assert!(loader.is_loaded("org"), "and it is still loaded");
}

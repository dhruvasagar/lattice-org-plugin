//! TK.3 + TK.4 — a TODO state is its own theme element, end to end.
//!
//! ## Why this is its own test binary
//!
//! The plugin-language registry is **process-global** and the component
//! always declares its language as `org`, so only the first claim in a
//! process wins. `org_highlight_from_component.rs` loads the component with
//! `provides = ["language"]` — no theme — and that registration compiles the
//! query with no theme registry, so every element capture in it resolves to
//! `Style::Default`.
//!
//! Two tests in one binary therefore do not merely race: whichever load wins
//! the claim is the one `Lang::detect_from_path` resolves for *both*, and a
//! TK.4 assertion running against the theme-less registration sees TODO
//! unstyled and reports a product bug that does not exist. That happened, and
//! a `tokio::sync::Mutex` did not fix it — serialising the loads does not
//! unclaim the name.
//!
//! A separate integration test is a separate process, which is the only
//! isolation boundary that actually holds here.
//!
//! Skips only when the component itself was not built.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use lattice_plugin_host::{PluginHost, TrustTier};
use lattice_plugin_loader::{LoaderServices, PluginLoader};
use lattice_syntax::{Lang, Style, Syntax};

fn org_component() -> Option<Vec<u8>> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/target/wasm32-wasip2/release/lattice_org_plugin.wasm"
    ))
    .ok()
}

/// Load with the `theme`, `config` and `language` seams, returning the
/// registry the component's elements landed in.
///
/// The seam ORDER is the loader's, not this test's: `drain_rank` puts
/// `theme` and `config` at 0 and `language` at 1, so the elements exist
/// before the query that names them is compiled. If that ever inverted,
/// every capture would resolve to `Style::Default` and org would render
/// unstyled — silently, which is why the assertions below are on the
/// resolved SPAN and not merely on the element existing.
async fn load(base: &std::path::Path, wasm: &[u8]) -> (usize, lattice_theme::ThemeRegistryHandle) {
    let plugins_dir = base.join("plugins");
    let dir = plugins_dir.join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"org\"\nprovides = [\"theme\", \"config\", \"language\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("component.wasm"), wasm).unwrap();

    let theme: lattice_theme::ThemeRegistryHandle = Arc::new(
        lattice_theme::InMemoryThemeRegistry::new(lattice_theme::default_palette()),
    );
    let host = Arc::new(
        PluginHost::with_dirs(base.join("cache"), base.join("data")).expect("host builds"),
    );
    let n = PluginLoader::with_services(
        host,
        LoaderServices {
            runtime: Some(tokio::runtime::Handle::current()),
            theme_registry: Some(theme.clone()),
            config_registry: Some(Arc::new(lattice_config::ConfigRegistry::new())),
            ..Default::default()
        },
    )
    .discover_and_load(&plugins_dir, TrustTier::Bundled)
    .await;
    (n, theme)
}

/// One test, one load — see the module header for why splitting it would
/// test less rather than more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn todo_states_are_their_own_theme_elements() {
    let Some(wasm) = org_component() else {
        eprintln!("skipping: component not built (cargo build --release --target wasm32-wasip2)");
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let (n, theme) = load(base.path(), &wasm).await;
    assert_eq!(n, 1, "the org component loads");

    let id = |name: &str| theme.id(&lattice_theme::ElementName::from(name.to_string()));
    let el = |name: &str| Style::Element(id(name).unwrap_or_else(|| panic!("{name} registered")));

    // ---- TK.3: the elements exist ----

    for name in ["org.todo", "org.todo.active", "org.todo.done"] {
        assert!(
            id(name).is_some(),
            "{name} must exist so an unconfigured keyword still inherits"
        );
    }
    // The default keyword set is `TODO | DONE`.
    for name in ["org.todo.TODO", "org.todo.DONE"] {
        assert!(
            id(name).is_some(),
            "{name} must exist for `org-todo-keyword-faces` to have somewhere to land"
        );
    }

    // ---- TK.3: the reported symptom, pinned ----

    let resolved = theme.resolved();
    let todo_fg = resolved.get(id("org.todo.TODO").unwrap()).fg;
    let done_fg = resolved.get(id("org.todo.DONE").unwrap()).fg;
    assert!(todo_fg.is_some(), "TODO has a colour of its own");
    assert_ne!(
        todo_fg, done_fg,
        "achieved and outstanding must not look the same"
    );
    // The old behaviour: `TODO` resolved to `Style::Keyword`, i.e. whatever
    // the theme paints `if` and `return`. That is the purple that was
    // reported, and it must not be what a TODO state gets now.
    let ids = lattice_theme::BuiltinElementIds::capture(theme.as_ref());
    let keyword_fg =
        lattice_syntax::theme_style::resolve_syntax_style(&resolved, &ids, Style::Keyword).fg;
    assert_ne!(
        todo_fg, keyword_fg,
        "a TODO state is not a language keyword and must not borrow its colour"
    );

    // ---- TK.4: the generated query actually uses them ----

    let lang = Lang::detect_from_path(Some(std::path::Path::new("notes.org")));
    let highlight = |src: &str, rows: u32| {
        let mut syntax = Syntax::for_language(lang)
            .expect("registry")
            .expect("org is registered");
        syntax.parse(src);
        syntax.highlight_lines_native(0, rows).expect("highlights")
    };

    let rows = highlight("* TODO ship it\n* DONE shipped\n", 2);
    assert!(
        rows[0].iter().any(|s| s.style == el("org.todo.TODO")),
        "TODO must paint as org.todo.TODO, not @keyword: {:?}",
        rows[0]
    );
    assert!(
        rows[1].iter().any(|s| s.style == el("org.todo.DONE")),
        "DONE must paint as org.todo.DONE, not @comment: {:?}",
        rows[1]
    );
    assert!(
        !rows[0].iter().any(|s| s.style == Style::Keyword),
        "nothing on a TODO headline may still be painted as a language keyword"
    );

    // A word the option does not name is title text — the degradation the
    // old hardcoded comment promised and the hardcoded list could not
    // actually deliver.
    let rows = highlight("* WAITING on someone\n", 1);
    if let Some(waiting) = id("org.todo.WAITING") {
        assert!(
            !rows[0].iter().any(|s| s.style == Style::Element(waiting)),
            "an unconfigured word must not highlight as a TODO state"
        );
    }

    // **The case that decided the mechanism.** A `* TODO` line inside a
    // block is example text, not a headline. The grammar knows; a regex over
    // lines cannot, and OT.3 spent a phase proving that for the agenda.
    // Asserted here so the choice is on the record.
    let rows = highlight("#+begin_example\n* TODO not a headline\n#+end_example\n", 3);
    assert!(
        !rows[1].iter().any(|s| s.style == el("org.todo.TODO")),
        "example text is not a TODO state: {:?}",
        rows[1]
    );

    // Only the FIRST word of a headline — the `.` anchor, preserved from the
    // query this replaced.
    let rows = highlight("* write the TODO list\n", 1);
    assert!(
        !rows[0].iter().any(|s| s.style == el("org.todo.TODO")),
        "a TODO mid-title is prose: {:?}",
        rows[0]
    );
}

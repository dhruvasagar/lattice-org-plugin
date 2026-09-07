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
/// Serialise this file's tests: they share PROCESS-GLOBAL state.
///
/// `lattice-syntax` keeps its plugin-language registry in globals — a
/// `OnceLock<PluginLanguagesHandle>`, an `ArcSwap<LangRegistry>` and a shared
/// name set. Every test here registers org's language into that registry and
/// then reads it back, so running two at once means one test's `for_language`
/// can resolve against another's half-installed registry.
///
/// That is what made `a_src_block_is_highlighted_by_its_own_language` fail once
/// under a full-suite run and pass 5/5 in isolation. It is a SHARED-STATE race,
/// not a timeout — no test in this file polls or sleeps — so the settle-budget
/// widening that fixed the `org_roam_index` flakes does not touch it.
///
/// A mutex rather than one test thread for the whole binary: the serialisation
/// belongs to the global these tests share, and saying so here is what stops
/// the next person reintroducing parallelism by removing a flag they cannot see
/// the reason for. Poisoning is ignored — a panicking test has already failed,
/// and turning its neighbours red too would hide which one broke.
/// A TOKIO mutex, not a `std` one: the critical section spans `.await` points
/// (loading the component, driving the seam), which is precisely what a
/// blocking guard must not be held across — it parks a runtime worker and
/// clippy's `await_holding_lock` says so. The async mutex yields instead.
///
/// No poisoning to handle, either: `tokio::sync::Mutex` has none, so a
/// panicking test cannot turn its neighbours red and hide which one broke.
static LANGUAGE_REGISTRY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn serialise_registry() -> tokio::sync::MutexGuard<'static, ()> {
    LANGUAGE_REGISTRY.lock().await
}

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

// ── TK.3 / TK.4: TODO states are their own theme elements ──

/// Load with the `theme` AND `language` seams, returning the registry the
/// component's elements landed in.
///
/// The order matters and is the loader's, not this test's: `theme` drains
/// before `language`, so a query capture naming an element finds it. If that
/// ever inverted, the capture would resolve to `Style::Default` and org would
/// render unstyled — silently, which is why the assertion below is on the
/// resolved STYLE and not merely on the element existing.
async fn load_theme_and_language(
    base: &std::path::Path,
    wasm: &[u8],
) -> (usize, lattice_theme::ThemeRegistryHandle) {
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
    // The plugin's own options register here, so `get_option` answers the
    // way it does in the editor rather than falling back for a different
    // reason than production would.
    let config = Arc::new(lattice_config::ConfigRegistry::new());

    let host = Arc::new(
        PluginHost::with_dirs(base.join("cache"), base.join("data")).expect("host builds"),
    );
    let n = PluginLoader::with_services(
        host,
        LoaderServices {
            runtime: Some(tokio::runtime::Handle::current()),
            theme_registry: Some(theme.clone()),
            config_registry: Some(config),
            ..Default::default()
        },
    )
    .discover_and_load(&plugins_dir, TrustTier::Bundled)
    .await;
    (n, theme)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shipped_component_colours_an_org_buffer() {
    let _serialised = serialise_registry().await;
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

/// A `#+begin_src rust` block is highlighted by Rust's grammar, not org's.
///
/// The mechanism is the one markdown's fenced blocks use, so what is worth
/// asserting is not that injections exist but that ORG's query drives them: the
/// language is org's first block *parameter* rather than a dedicated node, and
/// `block` covers every `#+begin_X`, so a wrong query either injects nothing or
/// injects into `#+begin_quote`.
///
/// `Style::Keyword` on `fn` is the discriminator. Org's own highlights have no
/// rule that would paint it — inside a block org sees `contents`, one opaque
/// node — so the style can only have come from Rust's grammar running over the
/// injected range.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_src_block_is_highlighted_by_its_own_language() {
    let _serialised = serialise_registry().await;
    let Some(wasm) = org_component() else {
        eprintln!("skipping: component not built");
        return;
    };
    let base = tempfile::tempdir().unwrap();
    assert_eq!(load_language_seam(base.path(), &wasm).await, 1, "org loads");
    let lang = Lang::detect_from_path(Some(std::path::Path::new("notes.org")));

    let mut syntax = Syntax::for_language(lang).unwrap().unwrap();
    // Line 1 is Rust; line 4 is the same text inside an EXAMPLE block that
    // NAMES rust — which is the case a `name:`-blind query gets wrong, and the
    // reason the guard is not decoration. An example block is verbatim text by
    // definition; a quote block would not discriminate here because it carries
    // no parameter, so the query would fail to match it either way.
    let src = "#+begin_src rust\nfn main() {}\n#+end_src\n#+begin_example rust\nfn main() {}\n#+end_example\n";
    syntax.parse(src);
    let lines = syntax.highlight_lines_native(0, 6).expect("highlights");

    let in_src: Vec<Style> = lines[1].iter().map(|s| s.style).collect();
    assert!(
        in_src.contains(&Style::Keyword),
        "`fn` inside #+begin_src rust should be a Rust keyword, got {in_src:?} — \
         org's own highlights cannot produce this, so its absence means the \
         injection never ran"
    );

    let in_example: Vec<Style> = lines[4].iter().map(|s| s.style).collect();
    assert!(
        !in_example.contains(&Style::Keyword),
        "an #+begin_example block is verbatim text even when it names a \
         language; got {in_example:?}"
    );
}

/// An org block whose first parameter is not a language anyone has must degrade
/// to plain text, not fail the file.
///
/// `#+begin_src` also routinely carries header arguments — `:results output`,
/// `:tangle yes` — and the query's anchor is what keeps those from being handed
/// to the host as language names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_or_absent_language_degrades_to_plain_text() {
    let _serialised = serialise_registry().await;
    let Some(wasm) = org_component() else {
        eprintln!("skipping: component not built");
        return;
    };
    let base = tempfile::tempdir().unwrap();
    assert_eq!(load_language_seam(base.path(), &wasm).await, 1, "org loads");
    let lang = Lang::detect_from_path(Some(std::path::Path::new("notes.org")));

    let mut syntax = Syntax::for_language(lang).unwrap().unwrap();
    // No language at all, a language nothing bundles, and a real one followed
    // by header args — the third must still inject.
    let src = "#+begin_src\nfn main() {}\n#+end_src\n\
               #+begin_src cobol\nfn main() {}\n#+end_src\n\
               #+begin_src rust :results output\nfn main() {}\n#+end_src\n";
    syntax.parse(src);
    let lines = syntax.highlight_lines_native(0, 9).expect("highlights");

    for (row, what) in [(1usize, "no language"), (4, "an unknown language")] {
        let styles: Vec<Style> = lines[row].iter().map(|s| s.style).collect();
        assert!(
            !styles.contains(&Style::Keyword),
            "{what} must leave the body plain, got {styles:?}"
        );
    }
    let with_args: Vec<Style> = lines[7].iter().map(|s| s.style).collect();
    assert!(
        with_args.contains(&Style::Keyword),
        "header arguments after the language must not defeat the injection, \
         got {with_args:?}"
    );
}

/// TK.3: the shipped component's TODO states are real theme elements.
///
/// Asserted through the loader with the same registry builtins use, because
/// the failure this guards is not "wrong colour" but "no element at all" —
/// which renders as plain title text and looks exactly like the old hardcoded
/// query missing a keyword.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tk3_todo_states_register_as_theme_elements() {
    let _serialised = serialise_registry().await;
    let Some(wasm) = org_component() else {
        eprintln!("skipping: component not built");
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let (n, theme) = load_theme_and_language(base.path(), &wasm).await;
    assert_eq!(n, 1, "the org component loads");

    for name in ["org.todo", "org.todo.active", "org.todo.done"] {
        assert!(
            theme
                .id(&lattice_theme::ElementName::from(name.to_string()))
                .is_some(),
            "{name} must exist so an unconfigured keyword still inherits"
        );
    }
    // The default keyword set is `TODO | DONE`.
    for name in ["org.todo.TODO", "org.todo.DONE"] {
        assert!(
            theme
                .id(&lattice_theme::ElementName::from(name.to_string()))
                .is_some(),
            "{name} must exist for `org-todo-keyword-faces` to have somewhere to land"
        );
    }
}

/// The reported symptom, pinned: TODO must not be the theme's *keyword*
/// colour, and DONE must not be the same colour as TODO.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tk3_todo_is_no_longer_painted_as_a_language_keyword() {
    let _serialised = serialise_registry().await;
    let Some(wasm) = org_component() else {
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let (_, theme) = load_theme_and_language(base.path(), &wasm).await;
    let resolved = theme.resolved();
    let style = |n: &str| {
        resolved.get(
            theme
                .id(&lattice_theme::ElementName::from(n.to_string()))
                .unwrap_or_else(|| panic!("{n} registered")),
        )
    };

    let todo = style("org.todo.TODO");
    let done = style("org.todo.DONE");
    assert!(todo.fg.is_some(), "TODO has a colour of its own");
    assert_ne!(
        todo.fg, done.fg,
        "achieved and outstanding must not look the same"
    );

    // The old behaviour: `TODO` resolved to `Style::Keyword`, i.e. whatever
    // the theme paints `if` and `return`. That is the purple that was
    // reported, and it must not be what a TODO state gets now.
    let ids = lattice_theme::BuiltinElementIds::capture(theme.as_ref());
    let keyword_fg =
        lattice_syntax::theme_style::resolve_syntax_style(&resolved, &ids, Style::Keyword).fg;
    assert_ne!(
        todo.fg, keyword_fg,
        "a TODO state is not a language keyword and must not borrow its colour"
    );
}

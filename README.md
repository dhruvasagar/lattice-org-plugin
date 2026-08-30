# org — the reference plugin

The first real consumer of lattice's `language` seam, and by now of twelve
others. Design: `docs/dev/architecture/org-mode.md` in the lattice tree —
plus `org-capture.md`, `org-roam.md` and `org-todo-keywords.md`. Sequencing
lives under `docs/dev/operations/slice-plans/` (finished plans move to
`slice-plans/archive/`).

**This lives outside the lattice tree, and that is the point.** It is what a
user's plugin manager clones and builds on boot, so it has to work as an
ordinary external repository rather than as a directory someone remembered to
exclude. It began as `examples/org-plugin` and moved out at OM.6.

It is not a bundled plugin and never will be: org's grammar is 2.2 MB of
generated C maintained outside crates.io, and design §1's whole argument is
that such a grammar is *the plugin's* build artefact, not the editor's.
Vendoring it would mean every lattice build either carried that weight or
reached the network.

## What it does

| seam | what org contributes |
|---|---|
| `language` | the `org` language: `.org` / `.org_archive`, the grammar, `queries/highlights.scm` + `queries/folds.scm` |
| `modes` | `org-mode` (major) plus the minors — `org-todo-mode`, `org-table-mode`, `org-agenda-mode`, `org-global-mode` |
| `grammar` | every action, motion and text object the modes bind: promote/demote, subtree move, `]]` / `[[` / `g{`, `ih`/`ah`/`ir`/`ar`, TODO and priority cycling, checkboxes, timestamps, links, table editing, the clock, archive / refile / capture, and org-roam's commands |
| `config` | `org.todo-keywords`, `org.todo-keyword-styles`, `org.highest-priority`, `org.inline-images`, `org.capture-templates`, `org.agenda-files`, `org.roam-directory`, `org.roam-dailies-directory` |
| `theme` | one theme element per TODO keyword, so `:colorscheme` recolours your own states |
| `media` | inline `[[file:diagram.png]]` images, on the GPUI peer |
| `scanned-excerpt-source` | dated rows for the agenda — what a row is, when it falls, how it sorts, and which files to scan |
| `multibuffer-view-source` | the agenda **view** itself: its buffer name, reuse policy and input model |
| `picker-source` | three pickers — refile targets, `org-roam-find-node`, `org-roam-backlinks` |
| `completion-source` | org-roam nodes, offered inside an `[[…]]` link |
| `transient-source` | the capture menu, one key per template |
| `events` | the clock's session, its minute wake and its modeline segment |
| `help` | `doc/org.md`, shipped inside the component; `:help org` |

The grammar is [`nvim-orgmode/tree-sitter-org`](https://github.com/nvim-orgmode/tree-sitter-org),
compiled to wasm by `build.rs`.

**Nothing in lattice knows what a headline is.** The host changes org needed
are all generic and none of them names org: a language index on the mode
registry, a `target-language` field on the mode declaration, a lifted
restriction that had kept majors off the `modes` seam, and — most recently —
the `multibuffer-view-source` seam, which lets any plugin own a view rather
than only feeding rows to one the host built. The `scanned-excerpt-source`
seam is generic in the same way: the plugin declares which file extensions it
wants offered, so lattice never learns what a `.org` file is either. It was
called `agenda-source` until it was pointed out that its record is an excerpt
plus an ordering plus a group header, with nothing about org, dates or TODOs
in it.

## Thirteen seams from one component

A component implements exactly ONE WIT world, so a plugin providing thirteen
seams needs a world importing all thirteen. Bundled plugins get theirs written
into lattice's own `wit/` — but an external plugin cannot add a world to
someone else's package.

It does not need to. WIT `include` composes worlds, and `wit-bindgen` resolves
an `inline` package against the interfaces found at `path`, so this plugin
declares its own world locally and gets one `Guest` trait carrying both
exports. **Nothing in lattice changes to allow it** — see `src/lib.rs`.

Three details, each a build error if missed: `include` needs the version
(`@0.1.0`); `generate_all` is required or wit-bindgen demands a `with` mapping
per reached interface; and the inline package needs a name distinct from
lattice's.

**And one that is not a build error — it is worse.** Several seams are declared
BARE (`export lattice:plugin-host/picker-source@0.1.0;`) rather than by
`include`-ing their world, because those worlds import `logging`. A component's
import set is fixed for the whole artefact and must resolve on EVERY linker it
is instantiated against, including the grammar seam's **synchronous** one where
`logging` is deliberately absent. An import that cannot resolve there fails the
WHOLE component, silently — one `logging::log` call once took the entire plugin
down.

`wit/` here is a **vendored copy** of lattice's, and the two must describe the
same lattice or the component builds against one ABI and is tested against
another.

**The dev-dependencies are worse than that, and you will hit it immediately.**
Fifteen of them are *absolute* paths into the author's home
(`/Users/dhruva/src/dhruvasagar/lattice/crates/...`), so `cargo test` cannot
work on any other machine without editing `Cargo.toml`. Building the
component — `cargo build --release --target wasm32-wasip2` — does **not** touch
them and works anywhere; it is only the host-side integration tests that need a
lattice checkout. Switching these to a relative path or a pinned `git =` is the
outstanding chore, and it is the one thing standing between this repo and being
genuinely clonable.

## Per-level headlines, and why they were the interesting part

Org's headline marker is **one node whose text length is the level**:
`(headline (stars) (item))`, where `stars` is `*`, `**`, `***`… Markdown's
grammar instead gives each level its own node (`atx_h1_marker` …
`atx_h6_marker`), so per-level capture there is six ordinary patterns.

Here the level has to come from the *text*, which means `#eq?` predicates —
and **no query bundled with lattice used one**, so this plugin is also what
proved the pipeline evaluates them. It does: tree-sitter's `QueryMatches`
filters on text predicates as it advances, so nothing host-side was needed.
`tests/org_headlines.rs` **in this repo** pins it, including the negative —
that a headline carries *only* its own level, which is what fails if
predicates are ever silently dropped. It lives here rather than in lattice
because the query it exercises is this plugin's, and lattice's CI should not
depend on a grammar it does not ship.

Upstream's own `highlights.scm` cycles three levels with `#match?` regexes
(`^(\*{3})*\*$` matches 1, 4 and 7 stars). This one wants true per-level 1–6,
and `#eq?` says that directly.

## Variable-font headlines come free

The stars are captured as `@punctuation.special` (→ `Style::Markup`, the same
style markdown's `#` markers take) and the title as `@text.title.N`. The GPUI
peer's `heading_scale_split` looks for the first run whose resolved
`scale > 1.0` and knows nothing about which grammar produced it — so
`[stars at base size][title scaled]` renders as two pieces on one baseline,
with **zero renderer changes**. Pinned by
`cells_paint::tests::org_headlines_scale_without_any_renderer_change`.

## Installing it

Two ways in. Both end with the component staged under a directory lattice
discovers at boot; the difference is who does the staging.

### The normal way — `require` it from your `init.rs`

Lattice's config is itself a WASM component (`~/.config/lattice/init/`), and the
`plugin-manager` seam is how it declares the plugins it wants. `require` is
use-package's shape: name a source, optionally name a mode to enable, and the
editor clones, builds and loads it.

`~/.config/lattice/init/plugin.toml`:

```toml
id = "init"
provides = ["plugin-manager"]
doc = "My lattice config."
```

`~/.config/lattice/init/src/lib.rs`:

```rust
wit_bindgen::generate!({
    world: "plugin-manager-plugin",
    path: "wit",
});

use lattice::plugin_host::plugin_manager::{self, GitSource, PluginSource, PluginSpec};

struct Component;

impl Guest for Component {
    fn register_plugins() {
        plugin_manager::require(&PluginSpec {
            // The directory it caches under, and how the editor reports it.
            name: "org".to_string(),
            source: PluginSource::Git(GitSource {
                url: "https://github.com/dhruvasagar/lattice-org-plugin".to_string(),
                // `None` tracks the default branch; pin a rev for reproducible
                // boots.
                rev: None,
            }),
            // Sugar for "enable this mode once the plugin loads". Org declares
            // its own `default_modes`, so this is only for a mode you want on
            // that the plugin leaves off.
            enable_mode: None,
            // `true` = build only if the artifact is missing, never rebuild.
            pinned: false,
        });
    }
}

export!(Component);
```

`require` **records** the spec and returns; the host resolves, clones, builds
and loads it after your registration export returns, off-thread. That is
deliberate — a `require` that resolved inline would put a git clone and a cargo
build inside a guest call on the boot path. So org's contributions appear a
frame or two after boot on a cold first run, and instantly thereafter from the
cache under `~/.config/lattice/plugins/`.

Building the component needs the `wasm32-wasip2` target and clang; a cold first
boot also needs network access for the grammar clone.

### The manual way — stage it yourself

Useful when you are developing the plugin, or want no build on the editor's
boot path at all.

```sh
git clone https://github.com/dhruvasagar/lattice-org-plugin
cd lattice-org-plugin
cargo build --release --target wasm32-wasip2

mkdir -p ~/.config/lattice/plugins/org
cp target/wasm32-wasip2/release/lattice_org_plugin.wasm \
   ~/.config/lattice/plugins/org/component.wasm
cp plugin.toml ~/.config/lattice/plugins/org/
```

The `.wasm` filename does not matter — the manifest does not name the
component, so the contract is **exactly one** `.wasm` in the directory. Two is
an error, and so is none.

Edit the copied `plugin.toml` so `capabilities` points at *your* notes
directory: the shipped one names the author's, and a grant you do not hold is
what makes archive, refile and capture refuse. Then start lattice — discovery
finds the directory, reads the manifest, and drains each seam.

### Configuring it

Options are ordinary typed settings, so `:set` works live and
`~/.config/lattice/lattice.toml` persists them (a project may override in
`<workspace>/.lattice/config.toml`):

```toml
[org]
todo-keywords = """
sequence: TODO(t) NEXT(n) | DONE(d)
type: PROJECT TO-READ READING
"""
agenda-files = "~/org"
roam-directory = "~/org/roam"
inline-images = true
```

`:set org.<Tab>` lists the full catalogue with its docs, and `:help org` opens
the plugin's own manual — which ships *inside* the component, so it is always
the manual for the version you are running.

## Building it

`build.rs` clones the grammar into `grammar-src/` and builds it with the
repo's `scripts/build-wasm-grammar.sh` — **clang and a rustup toolchain
only**, no emscripten, no docker, no tree-sitter CLI. Offline it embeds empty
bytes; the host then rejects the registration with a named reason rather than
failing the build.

```sh
cargo build --release --target wasm32-wasip2
```

The component lands at `target/wasm32-wasip2/release/lattice_org_plugin.wasm`.
Point a plugin directory at it with a `plugin.toml`:

```toml
id = "org"
provides = [
    "language", "modes", "grammar", "config", "theme", "media",
    "scanned-excerpt-source", "multibuffer-view-source",
    "picker-source", "completion-source", "transient-source",
    "events", "help",
]
default_modes = ["org-todo-mode", "org-global-mode"]
capabilities = ["fs:write:/home/you/org", "state:write"]
editor_capabilities = ["tree-sitter"]
```

The order of `provides` is cosmetic — the loader sorts by real registration
dependency before draining, so a manifest that lists `modes` before `grammar`
still resolves every keymap binding. That was not always true, and org is the
plugin it would have bitten.

`capabilities` covers two things. `fs:write` is what archiving, refile and
capture need: all three write to a
file other than the one you are in, and the host checks the target against this
grant at the plugin boundary before the effect reaches the editor. Point it at
the directory your org files live in. The same grant also covers the *read* a
`headline` capture target needs to find its insertion point — a write grant
implies read over the same directory, so there is no second thing to declare. Without it the outliner, tables and the
agenda all still work and those three chords say they were refused.

`default_modes` is load-bearing: it is what publishes the enablement request
for each mode named, and without it a mode registers correctly and simply never
activates — the chords are silently dead. Two are named because they answer
different questions: `org-todo-mode` is keyword cycling *inside org files*,
`org-global-mode` is the `<leader>o` prefix (capture, agenda, roam) that has
to work *wherever you are*. It is `<leader>o` and not `<C-x>o`: org's MAJOR
keymap binds a TERMINAL `<C-x>` (timestamp decrement), so inside an org buffer
`<C-x>` fires that and never waits for a second key — a prefix in one layer
against a terminal binding in another is the ambiguity vim settles with
`timeoutlen`, which lattice does not have. `org-table-mode` and `org-agenda-mode` are absent on
purpose — the table minor rides the major, and the agenda view's minor is
activated by the provider that builds the view.

It also auto-registers the single `org.enabled` gate, so `:set org.enabled=false`
turns both off together and leaves the outliner, tables and highlighting.

In normal use the plugin manager does this for you from the git source
(PM.5–PM.8) and caches the build under `~/.config/lattice/plugins/`.

## Tests

`cargo test` compiles this crate for the HOST and boots a real editor against
the component — so build the component first or the integration tests skip
and you have tested nothing:

```sh
cargo build --release --target wasm32-wasip2 && cargo test
```

The skip is deliberate and silent-ish (it prints `SKIP:`), because the
component is a *separate* build artefact: `cargo test` alone compiles this
crate for the host and never produces the `.wasm` the tests load. A green run
that never built the component has tested the pure functions and nothing else.

Requires a lattice checkout, for the reason in the dev-dependency note above.

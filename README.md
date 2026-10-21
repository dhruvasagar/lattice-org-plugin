# org — the reference plugin

Org-mode for **[lattice](https://github.com/dhruvasagar/lattice)** — a modal,
GPU-accelerated, plugin-first text editor in Rust. Outline editing, TODO
workflow, tables, the clock, capture, refile, archive, the agenda and
org-roam, shipped as a single WebAssembly component.

| | |
|---|---|
| Version | `v0.1.1` · see [`CHANGELOG.md`](CHANGELOG.md) for what each release changed and the plugin API it builds against |
| The editor | [github.com/dhruvasagar/lattice](https://github.com/dhruvasagar/lattice) · [website](https://dhruvasagar.github.io/lattice/) · [install](https://dhruvasagar.github.io/lattice/install/) |
| This plugin's page | [dhruvasagar.github.io/lattice/plugins/org/](https://dhruvasagar.github.io/lattice/plugins/org/) |
| Writing your own | [plugin authoring guide](https://github.com/dhruvasagar/lattice/blob/main/docs/dev/guides/plugin-authoring.md) |
| The API it builds against | [`lattice-wit`](https://crates.io/crates/lattice-wit) · [`lattice-plugin-sdk`](https://crates.io/crates/lattice-plugin-sdk) |

It is the first real consumer of lattice's `language` seam, and by now of
fourteen others — see the table below, which the manifest's `provides` (at the
top of `src/lib.rs`) is the source of truth for.

Design lives in the lattice tree:
[`org-mode.md`](https://github.com/dhruvasagar/lattice/blob/main/docs/dev/architecture/org-mode.md),
plus `org-capture.md`, `org-roam.md` and `org-todo-keywords.md` beside it.
Sequencing is under
[`slice-plans/`](https://github.com/dhruvasagar/lattice/tree/main/docs/dev/operations/slice-plans)
(finished plans move to `slice-plans/archive/`).

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
| `grammar` | every action, motion and text object the modes bind: promote/demote, subtree move, `]]` / `[[` / `g{`, `ih`/`ah`/`ir`/`ar`, TODO and priority cycling, checkboxes, timestamps, links, table editing, the clock, archive / refile / capture, the agenda's bulk marks, and org-roam's commands |
| `config` | `org.todo-keywords`, `org.todo-keyword-styles`, `org.highest-priority`, `org.inline-images`, `org.capture-templates`, `org.directory`, `org.capture-drafts-directory`, `org.agenda-files`, `org.roam-directory`, `org.roam-dailies-directory`, `org.roam-capture-templates` |
| `theme` | one theme element per TODO keyword, so `:colorscheme` recolours your own states |
| `media` | inline `[[file:diagram.png]]` images, on the GPUI peer |
| `scanned-excerpt-source` | dated rows for the agenda — what a row is, when it falls, how it sorts, and which files to scan |
| `multibuffer-view-source` | the agenda **view** itself: its buffer name, reuse policy and input model |
| `picker-source` | refile targets, capture drafts (`<leader>oC`), and org-roam's find-node, insert-node and backlinks pickers |
| `completion-source` | org-roam nodes, offered inside an `[[…]]` link |
| `transient-source` | the capture menu (one key per template), the TODO-state menu, the agenda's `gD` view menu and its `x` bulk menu |
| `signs` | the `>` an agenda row wears when it is marked for a bulk action |
| `decorations` | which agenda rows carry that mark — re-asked through `refresh-decorations`, because a mark changes no text |
| `events` | the clock's session, its minute wake and its modeline segment |
| `help` | `doc/org.md` and `doc/roam.md`, shipped inside the component; `:help org`, `:help org.roam`. The host adds the `org` prefix from the manifest id, so the files are named for the topic without it |

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

## Fifteen seams from one component

A component implements exactly ONE WIT world, so a plugin providing fifteen
seams needs a world importing all fifteen. Bundled plugins get theirs written
into lattice's own `wit/` — but an external plugin cannot add a world to
someone else's package.

It does not need to. WIT `include` composes worlds, and `wit-bindgen` resolves
an `inline` package against the interfaces found at `path`, so this plugin
declares its own world locally and gets one `Guest` trait carrying both
exports. **Nothing in lattice changes to allow it** — see `src/lib.rs`.

Three details, each a build error if missed: `include` needs the version
(`@0.3.0`); `generate_all` is required or wit-bindgen demands a `with` mapping
per reached interface; and the inline package needs a name distinct from
lattice's.

**And one that is not a build error — it is worse.** Several seams are declared
BARE (`export lattice:plugin-host/picker-source@0.3.0;`) rather than by
`include`-ing their world, because those worlds import `logging`. A component's
import set is fixed for the whole artefact and must resolve on EVERY linker it
is instantiated against, including the grammar seam's **synchronous** one where
`logging` is deliberately absent. An import that cannot resolve there fails the
WHOLE component, silently — one `logging::log` call once took the entire plugin
down.

`wit/` here is **generated, not vendored** — `build.rs` writes it from the
[`lattice-wit`](https://crates.io/crates/lattice-wit) dependency on every
build, and it is gitignored. A hand-copied `wit/` is how this repo once drifted
three ABI changes behind the editor, with the only symptom being that org
silently stopped loading.

That dependency pin **is** the ABI generation this plugin targets:

```toml
[dependencies]
lattice-plugin-sdk = "0.3"   # typed config shapes
[build-dependencies]
lattice-wit = "0.3"          # the WIT package -> wit/
```

Both come from crates.io, so building this component needs no checkout of
lattice. That was not always true: every lattice dependency here was once an
*absolute* path into the author's home, and because cargo resolves
`[dev-dependencies]` as part of the BUILD graph — not just the test graph —
even `cargo build --release --target wasm32-wasip2` failed on any other
machine. The host-side tests that need a real editor now live in a separate
package (see [Tests](#tests)) precisely so they cannot gate anyone's install
again.

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

You need **lattice 0.9.11 or later** and an `init.rs` — your lattice config,
which is itself a plugin (`lattice --scaffold-init` writes one). Org is
installed from it: ready-built from a release, or built from source.

### First: your `init.rs` needs the `plugin-manager` seam

A config scaffolded by lattice 0.9.11 or later has it already —
`register_plugins` is there, with a commented-out example that installs org.
An older config needs three additions in `~/.config/lattice/init/`:

1. `wit/init.wit`, inside `world init { … }`:

   ```wit
   import plugin-manager;
   export register-plugins: func();
   ```

2. `"plugin-manager"` in the manifest's `provides` — at the top of
   `src/lib.rs`, or in `plugin.toml` if your config has one;
3. a `fn register_plugins() { … }` in your `impl Guest`, holding one of the
   two calls below.

### The released build — no Rust toolchain

Every release publishes org ready-built, so this needs no Rust, no
`wasm32-wasip2` target and no clang:

```rust
fn register_plugins() {
    use lattice::plugin_host::plugin_manager::{self, ReleaseSpec};
    plugin_manager::require_release(&ReleaseSpec {
        name: "org".to_string(),
        repo: "dhruvasagar/lattice-org-plugin".to_string(),
        // A release tag. `None` takes the newest release built for your
        // editor instead, and `:plugin-update org` moves it to a newer one.
        version: Some("v0.1.1".to_string()),
        // Optionally pin the exact bytes: the first word of the release's
        // `org-<version>-api<M>.<m>.wasm.sha256`.
        sha256: None,
        // org-mode turns itself on for `.org` files.
        enable_mode: None,
    });
}
```

Then `:reload-config` (or restart lattice). Lattice downloads the build made
for its own plugin API (`org-<version>-api<M>.<m>.wasm`), checks it against
the checksum the release publishes beside it and against the manifest inside
it, and installs it into `~/.config/lattice/plugins/org/`. From then on,
starting the editor makes no network request.

**Check it worked:** `:plugins` lists `org` with its source,
`release dhruvasagar/lattice-org-plugin@v…`, and opening a `.org` file puts
you in `org-mode`. If it was refused, it is under the failed loads in
`:plugins` with the reason — a release with no build for your editor says
which it has, and an editor older than 0.9.11 says the version org needs.

**Updating:** with a tag, change it and `:reload-config`. With `None`,
`:plugin-update org` (or `u` on its row in `:plugins`) installs a newer
release when there is one, and keeps what you have when there is not.

Then point org at your notes — see [Where org may write](#where-org-may-write).

### From source — `require` it

Builds org on your machine: needs Rust, the `wasm32-wasip2` target and clang
(org's tree-sitter grammar is C), and network access on a cold first build for
the grammar clone. The way to run an unreleased `main`, or your own fork.

```rust
fn register_plugins() {
    use lattice::plugin_host::plugin_manager::{self, GitSource, PluginSource, PluginSpec};
    plugin_manager::require(&PluginSpec {
        name: "org".to_string(),
        source: PluginSource::Git(GitSource {
            url: "https://github.com/dhruvasagar/lattice-org-plugin".to_string(),
            // A release tag, so every boot builds the same plugin. `None`
            // tracks the default branch instead — newer, and unreleased.
            rev: Some("v0.1.1".to_string()),
        }),
        enable_mode: None,
        // `true` = build only if the artifact is missing, never rebuild.
        pinned: false,
    });
}
```

`require` **records** the spec and returns; the host clones, builds and loads
org after your config's registration returns, off-thread — a git clone and a
cargo build never sit inside a guest call on the boot path. So on a cold
first run org appears once its build finishes, and instantly thereafter from
`~/.config/lattice/plugins/`. `PluginSource::Local("~/src/lattice-org-plugin".to_string())`
builds a checkout of yours in place instead, so an edit and `:reload-config`
picks it up.

To switch between the two, replace one call with the other (keep
`name: "org"`) and `:reload-config`.

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
```

That one file is the whole plugin: its manifest is embedded in it. The `.wasm`
filename does not matter; the contract is **exactly one** `.wasm` in the
directory. Two is an error, and so is none.

Then set `org.directory` (and `org.roam-directory`, if you use roam) to where
your notes live — see [Where org may write](#where-org-may-write) — and start
lattice. Discovery finds the directory, reads the manifest, and drains each
seam.

### Configuring it

Options are ordinary typed settings: `:set org.<name>=…` changes one live,
and your `init.rs` sets them for every session — in `register_options`, even
though org has not loaded yet: lattice holds each value and applies it when
org declares the option
([lattice's `init.rs` guide](https://github.com/dhruvasagar/lattice/blob/main/docs/user/init.md#configuring-user-plugins-that-load-after-you)):

```rust
fn register_options() {
    config::set_option("org.directory", "~/notes");
    config::set_option("org.roam-directory", "~/notes/roam");
    config::set_option("org.agenda-files", "~/notes");
    config::set_option("org.inline-images", "true");
}
```

Setting `org.directory` and `org.roam-directory` here rather than from a
`pre-plugin-loaded` handler matters: org's file access follows them, and set
up front they are in place for org's first load instead of costing a reload.

Values that need no code can go in `~/.config/lattice/lattice.toml` instead —
lattice holds them the same way until org declares the options:

```toml
[org]
directory = "~/notes"
roam-directory = "~/notes/roam"
agenda-files = "~/notes"
```

Where both set an option, `init.rs` wins: it is read after `lattice.toml`.

### Where org may write

Archive, refile and capture write to files other than the one you are in, and
lattice checks every such write against org's file grant. That grant **follows
two of org's own options**:

| Option | Default | Org may write under |
|---|---|---|
| `org.directory` | `~/org` | your org directory |
| `org.roam-directory` | `~/org-roam` | your org-roam notes |

The defaults are emacs's own (`org-directory`, `org-roam-directory`). Setting
either option — from your `init.rs`, or live with `:set` — is how you
move the access; there is no separate grant to edit. Org reloads itself when
the value changes, and `:plugins` shows what is in force
(`fs:write ~/notes (org.directory)`). A write outside both is refused with a
message naming the option to change. Org itself cannot change these options:
only your configuration can.

A directory that does not exist is fine: roam stays off until
`org.roam-directory` exists, and capture drafts fall back as if `org.directory`
were unset. This needs lattice **0.9.11** or later; an older editor refuses the
plugin by name.

`:set org.<Tab>` lists the full catalogue with its docs, and `:help org` opens
the plugin's own manual — which ships *inside* the component, so it is always
the manual for the version you are running.

## Building it

```sh
cargo build --release --target wasm32-wasip2
```

Nothing else — no lattice checkout, no emscripten, no docker, no tree-sitter
CLI. `build.rs` clones the grammar into `grammar-src/` and compiles it with
`scripts/build-wasm-grammar.sh`, which needs **clang and a rustup toolchain**
and nothing more.

> **The clang has to be able to target wasm32**, and on macOS the default one
> cannot. Apple clang ships without the WebAssembly backend:
>
> ```
> error: unable to create target: 'No available targets are compatible
> with triple "wasm32-unknown-unknown"'
> ```
>
> `brew install llvm`, then build with `CLANG=$(brew --prefix llvm)/bin/clang`.
> Linux distributions' clang has the backend already.

**A failed grammar build does not fail the build.** `build.rs` embeds empty
bytes and warns, so the host rejects the registration with a named reason
instead of a reference plugin becoming a compile error. The cost is that a
green build is not proof of a working plugin — if org loads but parses
nothing, check the build log for `grammar build failed`, and check your clang.
CI asserts the grammar is over 100 KB for exactly this reason; a real one is
~340 KB.

The component lands at `target/wasm32-wasip2/release/lattice_org_plugin.wasm`,
with its manifest embedded — the `lattice-manifest` section written at the top
of `src/lib.rs`, where each of its fields is explained. There is no
`plugin.toml` to stage beside it.

The order of `provides` is cosmetic — the loader sorts by real registration
dependency before draining, so a manifest that lists `modes` before `grammar`
still resolves every keymap binding. That was not always true, and org is the
plugin it would have bitten.

`capabilities` covers two things. The two `fs:write` grants follow
`org.directory` and `org.roam-directory` ([Where org may
write](#where-org-may-write)); archiving, refile and capture need them, and
the same grant covers the *read* a `headline` capture target needs to find its
insertion point. Without them the outliner, tables and the agenda all still
work and those three chords say they were refused.

`state:write` is the plugin's own store. Each capture records its target and
the buffer it was started from there, which is what lets a saved draft be filed
after a restart.

`default_modes` is load-bearing: it is what publishes the enablement request
for each mode named, and without it a mode registers correctly and simply never
activates — the chords are silently dead. Three are named because they answer
different questions: `org-todo-mode` is keyword cycling *inside org files*,
`org-table-mode` is table editing in them, and `org-global-mode` is the
`<leader>o` prefix (capture, agenda, roam) that has to work *wherever you are*.
It is `<leader>o` and not `<C-x>o`: org's MAJOR keymap binds a TERMINAL `<C-x>`
(timestamp decrement), so inside an org buffer `<C-x>` fires that and never
waits for a second key — a prefix in one layer against a terminal binding in
another is the ambiguity vim settles with `timeoutlen`, which lattice does not
have.

**An `ActivationPolicy` is not a substitute for being listed here**, and this
README said otherwise until 2026-08-30: `org-table-mode` was described as
riding the major. It does not. `auto_activatable_minors` filters on enablement
*before* policy, so a minor that is never enabled is never asked where it may
activate — and the eleven table chords, `<Tab>` between cells among them, did
nothing on a default install. Nothing caught it because no test read this file;
one does now.

`org-agenda-mode` is the one genuinely absent, for a reason about activation
rather than enablement: the agenda view's major is `multibuffer-mode`, which no
policy here could reach, so the provider that builds the view activates it.

It also auto-registers the single `org.enabled` gate, so `:set org.enabled=false`
turns all three off together and leaves the outliner and highlighting.

In normal use the plugin manager does this for you from the git source
(PM.5–PM.8) and caches the build under `~/.config/lattice/plugins/`.

## Releasing

A release is a commit and a `vX.Y.Z` tag. Pushing the tag runs
`.github/workflows/release.yml`, which calls lattice's reusable
`plugin-release.yml`: it checks the tag is the crate version, builds org with
lattice's `scripts/build-plugin.sh`, and publishes a GitHub release — the tag
message as its notes — carrying `org-<version>-api<M>.<m>.wasm` and its
`.sha256`. That asset is what `require_release` installs.

```sh
scripts/release.sh                 # the current version and what each bump gives
scripts/release.sh minor --dry-run # every step, nothing changed
scripts/release.sh minor           # bump, promote the changelog, commit, tag
git push origin main && git push origin vX.Y.Z
```

Write what changed under `## Unreleased` in `CHANGELOG.md` as it lands; the
script retitles that section with the version and date, records the plugin API
the release builds against (`lattice:plugin-host@…`, from the locked
`lattice-wit`), and uses the section as the tag message. It also bumps `Cargo.toml`, `Cargo.lock` and the
version in this README — the `| Version |` row, the `rev:` pin and the
`version:` pin above — and refuses to tag if any of those lines is out of
shape. It stops before pushing.

## Tests

Two suites, deliberately in different packages.

**The component's own** — pure functions, no editor:

```sh
cargo test
```

**The integration suite** — boots a real editor, loads this component through
the loader and dispatches chords. It lives in `integration/`, its own package
with its own workspace, because its dependencies are lattice's host crates and
those would otherwise sit in this package's build graph and break everyone's
install. It needs a lattice checkout **beside this one**:

```
<somewhere>/lattice-org-plugin
<somewhere>/lattice
```

```sh
cargo build --release --target wasm32-wasip2   # first, always
cd integration && cargo test
```

The component build is not optional. Every test **skips** when the artefact is
absent — printing `skipping: component not built` — so running them without it
reports green while testing nothing. CI greps its own run for that line.

(`integration/Cargo.lock` is committed and load-bearing: the tree reaches a
yanked `bisync`, which stays usable only when a lockfile already names it.)

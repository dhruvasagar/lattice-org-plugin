# org — the reference plugin

The first real consumer of lattice's `language` seam, and by now of six
others. Design: `docs/dev/architecture/org-mode.md` in the lattice tree;
sequencing: `docs/dev/operations/slice-plans/org-mode.md`.

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
| `modes` | four modes — `org-mode` (major), `org-todo-mode`, `org-table-mode`, `org-agenda-mode` |
| `grammar` | every action, motion and text object the modes bind: promote/demote, subtree move, `]]` / `[[` / `g{`, `ih`/`ah`/`ir`/`ar`, TODO and priority cycling, checkboxes, timestamps, links, table editing, archive / refile / capture |
| `config` | `org.todo-keywords`, `org.highest-priority`, `org.inline-images`, `org.capture-file`, `org.capture-template` |
| `media` | inline `[[file:diagram.png]]` images, on the GPUI peer |
| `agenda-source` | dated rows for `:agenda` — what a row is, when it falls, how it sorts |
| `picker-source` | `org-refile`'s target list: every headline in the project's org files |
| `help` | `doc/org.md`, shipped inside the component; `:help org` |

The grammar is [`nvim-orgmode/tree-sitter-org`](https://github.com/nvim-orgmode/tree-sitter-org),
compiled to wasm by `build.rs`.

**Nothing in lattice knows what a headline is.** Three host changes were
needed across the whole of it and none of them names org: a language index on
the mode registry, a `target-language` field on the mode declaration, and a
lifted restriction that had kept majors off the `modes` seam. The
`agenda-source` seam that landed last is generic in the same way — the plugin
declares which file extensions it wants offered, so lattice never learns what
a `.org` file is either.

## Seven seams from one component

A component implements exactly ONE WIT world, so a plugin providing seven
seams needs a world importing all seven. Bundled plugins get theirs written
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

`wit/` here is a **vendored copy** of lattice's, and the two must describe the
same lattice or the component builds against one ABI and is tested against
another. The dev-dependencies below are path deps into a local checkout for
the same reason — portable enough for the author, not for a stranger cloning
this repo. Switching them to a pinned `git =` is the outstanding chore.

## Per-level headlines, and why they were the interesting part

Org's headline marker is **one node whose text length is the level**:
`(headline (stars) (item))`, where `stars` is `*`, `**`, `***`… Markdown's
grammar instead gives each level its own node (`atx_h1_marker` …
`atx_h6_marker`), so per-level capture there is six ordinary patterns.

Here the level has to come from the *text*, which means `#eq?` predicates —
and **no query bundled with lattice used one**, so this plugin is also what
proved the pipeline evaluates them. It does: tree-sitter's `QueryMatches`
filters on text predicates as it advances, so nothing host-side was needed.
`crates/lattice-syntax/tests/org_headlines.rs` pins it, including the
negative — that a headline carries *only* its own level, which is what fails
if predicates are ever silently dropped.

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
provides = ["language", "modes", "grammar", "config", "media", "agenda-source", "picker-source", "help"]
default_modes = ["org-todo-mode", "org-global-mode"]
capabilities = ["fs:write:/home/you/org"]
```

The order of `provides` is cosmetic — the loader sorts by real registration
dependency before draining, so a manifest that lists `modes` before `grammar`
still resolves every keymap binding. That was not always true, and org is the
plugin it would have bitten.

`capabilities` is what archiving, refile and capture need: all three write to a
file other than the one you are in, and the host checks the target against this
grant at the plugin boundary before the effect reaches the editor. Point it at
the directory your org files live in. Without it the outliner, tables and the
agenda all still work and those three chords say they were refused.

`default_modes` is load-bearing: it is what publishes the enablement request
for each mode named, and without it a mode registers correctly and simply never
activates — the chords are silently dead. Two are named because they answer
different questions: `org-todo-mode` is keyword cycling *inside org files*,
`org-global-mode` is the `<C-x>o` prefix (capture, agenda) that has to work
*wherever you are*. `org-table-mode` and `org-agenda-mode` are absent on
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

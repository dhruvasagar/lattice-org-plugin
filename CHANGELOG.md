# Changelog

## Unreleased

Needs lattice 0.10.3 or later (lattice-wit 0.6.0, `lattice:plugin-host@0.6.0`).

### Added
- **Babel: `org-babel-mode`**, on in every org buffer and off with `:set
  org.babel.enabled=false`, which leaves the rest of org on. It owns `C-c C-c`
  inside a source block (anywhere else the key is org's, as before) and the
  `C-c C-v` prefix: `b` runs the buffer's blocks, `s` the subtree's, `k`
  removes a block's result; `:org-babel-cancel` stops them. Nothing runs until
  you list a language in `org.babel.languages`, and that list is also all org
  may run. See `:help org.babel`.
- **Babel runs a block.** `C-c C-c` runs it in the background with a `▶` in
  the gutter, and writes its output under it as `#+RESULTS:`, replacing the
  old result, in one undo step. Long output goes in an example block that
  output cannot break out of. The result finds its block even if you typed
  above it meanwhile. If the block itself changed, the output goes to
  `*org-babel*` instead of the file. Header arguments merge from
  `#+PROPERTY:`, headline drawers, `#+HEADER:` and the block line, as in
  emacs.
- **Babel asks, runs many, and stops.** `C-c C-c` asks before it runs
  (`org.babel.confirm`), and `y` runs the block it asked about even after an
  edit. `C-c C-v b` / `C-c C-v s` run a buffer's or subtree's blocks in order,
  asking once. `:org-babel-cancel` stops them, and `C-c C-v k` removes a
  result.
- **Babel's `:results`, `value` and `:dir`.** Results can be appended or
  prepended, shown only in the echo area, or not kept, and written as an
  aligned table, raw org, a drawer or a code block. With a `value` wrapper
  (`python`, `ruby`, `node`, `sh`) a block's result is its last expression,
  not what it printed. A block runs in `:dir`, or else in its file's
  directory.
- **Babel tangles.** `C-c C-v t` (`:org-babel-tangle`) writes each block
  with a `:tangle` to its file: `yes` is the org file's name with the
  language's extension, and a path is relative to the org file. Blocks for
  one file are joined in order (`:padline no` drops the blank line between),
  and `:shebang` heads the file and makes it executable. Each file is
  replaced whole, as one undo step if you have it open. Nothing is written
  if any target is outside `org.directory` and
  `org.babel.tangle-directories`.
- **`:org-babel-output`** shows `*org-babel*`, where a run's output goes when
  its block changed, was deleted or closed before it finished. The echo used
  to name that buffer, but nothing ever created it.

## 0.4.0 — 2026-11-17

Builds against `lattice:plugin-host@0.5.x` (lattice-wit 0.5.0).

Needs lattice 0.10.1 or later. Lattice 0.10.1 speaks plugin API 0.5 only,
so org 0.3.0, built against 0.4, loads in no released editor: use this
release instead.

### Changed
- **Plugin API 0.5.** org now builds against `lattice-wit` and
  `lattice-plugin-sdk` 0.5.0. Its `apply-operator` takes the new parse-tree
  argument and, as before, contributes no operators; nothing org does
  changes.

## 0.3.0 — 2026-11-17

Builds against `lattice:plugin-host@0.4.x` (lattice-wit 0.4.2).

Needs lattice 0.10.1 or later, the first with the `llm-tools` seam the
assistant's tools are offered through; an older editor refuses org by name.

### Added
- **Org answers lattice's LLM chats.** `org.agenda` tells the assistant what
  is scheduled, due and overdue for a day, a week or a date range, optionally
  only what matches a tag or TODO expression (`+work`, `-meeting`, `/WAITING`).
  It reads your `org.agenda-files` (else `org.directory`) the way `:org-agenda`
  does, repeats included, so the two never disagree. Every answer starts with
  today's date, and every row carries a ref (`file::line::heading`). The tool
  only reads: the seam's store is read-only whatever org's grant says. Turn it
  on in a chat profile with `tools = ["org.*"]`.
- **`org.query` and `org.entry`.** `org.query` finds entries by tags,
  properties or TODO state whatever their dates (`+proj/WAITING`,
  `OWNER="Sam"`, `/DONE`); `org.entry` reads one entry whole (its notes,
  properties, log and subheadings) by the ref another tool listed, or by its
  `ID`. A ref whose line has moved still finds its heading when one headline
  carries it, and is refused with "query again" when none or several do.
- **`org.clock`** totals the time you clocked over a range of days (the last
  week by default), per entry, per file or per tag, largest first. It counts
  what the agenda's clock report counts: a running clock adds nothing.
- **`org.notes`** finds your org-roam notes by words in their title or
  aliases, or by tag, and names the notes that link to each. A note's ref is
  its `ID`, and `org.entry` reads it: a whole-file note whole, a headline
  note as its subtree.
- **`org.change`: the assistant changes an entry the way the keys do.** It
  sets a TODO state, schedules an entry, sets or removes its deadline (dates
  as the `s`/`d` prompt reads them: `+2d`, `fri`, `2026-11-14 +1w`, `none`)
  adds or removes tags, and adds a note to the entry's log (`- Note taken
  on …`, where org's `org-add-note` puts it), and refiles it under another
  headline or to a file, as `<leader>or` does. A refile across two files is
  two edits, the copy first, so declining the second leaves the entry in both
  files rather than in neither. It also captures through one of your
  `org.capture-templates`, with the capture keys' own expansion and targets
  (headline, outline path, datetree, table line), the template's `%^{…}`
  questions answered by the assistant or the capture refused with the list
  of them. A template that clocks in does not start your clock, and one whose
  target file does not exist yet is refused. Org writes the text, not the model: a repeating
  task repeats, a state with `!` or `@` gets its log line (with the note the
  assistant gives), exactly as `<leader>ot` writes them. Each change is a
  proposal lattice puts in front of you as a diff before anything is written.

- **Workflows for the assistant: `:org-plan-day`, `:org-plan-week`,
  `:org-review`, `:org-report`.** Each opens an `:llm` chat on one of five
  profiles org now contributes (`org`, `org-plan`, `org-review`,
  `org-report`, `org-research`), with its own instructions and only the tools
  it needs, and asks the first question for you. Pick a model per workflow
  in your config, `[llm.profiles.org-plan] model = "…"`; org names none.
  `:help org.assistant` covers the tools, the workflows and what a change
  from a chat can and cannot do.
- **Org's tools in Claude Code and other agents.** With a lattice that serves
  plugin tools (`:set llm.serve_tools`), `claude mcp add lattice -- lattice
  --mcp` gives Claude Code `org__agenda`, `org__change` and the rest, and an
  ACP agent lattice starts gets them with no setup. A change an agent proposes
  always opens a diff in lattice and waits for you. `:help org.assistant` now
  walks through setup, each workflow, choosing a model, what leaves your
  machine, every tool's inputs, using the tools from other agents, and
  troubleshooting.

### Changed
- **The agenda shows a repeating entry on every day it repeats.** A
  `SCHEDULED: <… +1d>` standup appears on each day of a week view, and a
  `+1w` review on its day, as in Org. The day written on the line is still
  the one that turns overdue. `.+` repeaters (counted from when you next do
  the task) have no future days to show, so a habit is shown once, by its
  consistency graph.

### Fixed
- **Refile files a subtree under its target.** `<leader>or` onto `Work` put
  the entry after Work's subtree at its own level, a sibling of Work; it is
  now re-levelled to be Work's child, deeper headings shifting with it, as
  org's refile and org's capture both file. Refiling to a file makes it a
  top-level entry.
- **A state-change note sits under its log line's text.** The note an `@`
  state prompts for was written level with the `- State …` bullet; it is now
  two columns further in, as org writes it and reads it back.
- **A section matching a TODO state finds DONE entries that have no tags.**
  `match = "/DONE"` used to miss them: an untagged DONE headline was never
  scanned as a row.
- **A week view no longer includes an eighth day.** The dated block took in
  rows from the day after its header ended, filed under a header of their own.

## 0.2.1 — 2026-11-07

Builds against `lattice:plugin-host@0.4.x` (lattice-wit 0.4.1).

Builds against `lattice:plugin-host@0.4.x` (lattice-wit 0.4.1).

Needs **lattice 0.9.14** or later, for `ui.notify`.

### Added
- **A roam scan reports how it finished in `*messages*`:** the nodes and files
  it indexed, or that it found no nodes, no `.org` files or an unreadable
  directory.
- **Roam says when the index can fall behind.** If lattice cannot watch the
  roam directory, a scan says that notes changed outside lattice will be
  picked up only by `:org-roam-sync` or a restart. If a scan or a re-index
  cannot save the list "find a node" reads, it says the picker still shows the
  previous list, where it used to report success.

### Changed
- **The docs show capture templates and lists in `lattice.toml`.** lattice
  0.9.14 keeps a plugin's lists and tables there until the plugin loads, so
  `[[org.capture-templates]]` and `agenda-files = [...]` no longer need
  `init.rs`. The capture section now says which file its templates go in.
- **`:org-roam-sync` says when there is nothing to scan.** With no usable
  `org.roam-directory`, it now says so instead of echoing "re-scanning…".

### Fixed
- **`:org-roam-find-node` could stay empty for good after an interrupted
  scan.** The picker's list is written at the end of a scan, and a later scan
  only rewrote it if some note had changed. So a first scan that was cut short
  left every note indexed and no list, and neither a restart nor
  `:org-roam-sync` brought it back. Every completed scan now rewrites the list.
- **Building org from a local checkout uses the API it pins.** Before each
  build, lattice rewrites a local plugin's `wit/` with its own API, and the
  rewrite did not cause org's build script to rerun. An editor older than
  org's `lattice-wit` pin therefore left its older API in place, and the build
  failed: lattice 0.9.13 has no `ui.notify`.

## 0.2.0 — 2026-10-26

Builds against `lattice:plugin-host@0.4.x` (lattice-wit 0.4.0).

Needs **lattice 0.9.12** or later: an editor speaking plugin API 0.3
refuses this build by name.

### Changed
- **Plugin API 0.4.** org now builds against `lattice-wit` and
  `lattice-plugin-sdk` 0.4.0, the API generation lattice 0.9.12 speaks.
  Nothing org does changes. On lattice 0.9.11, pin org 0.1.2.

## 0.1.2 — 2026-10-21

Builds against `lattice:plugin-host@0.3.x` (lattice-wit 0.3.1).

Needs **lattice 0.9.11** or later; an older editor refuses the plugin by name.

### Added
- **Install org without building it.** Each release now publishes the
  component ready-built — `org-<version>-api<M>.<m>.wasm` and its
  checksum — so `require_release` in your `init.rs` installs it with no
  Rust toolchain, `wasm32-wasip2` target or clang (see the README).
  Building from source with `require` still works as before.

### Changed
- **Org is one file.** Its manifest is embedded in the component, so a
  release is the `.wasm` alone and there is no `plugin.toml` to copy or
  edit.
- **Where org may write follows your configuration.** Archive, refile and
  capture write only under `org.directory` and `org.roam-directory`. Set
  those to where your notes live — that setting is the permission, and org
  reloads itself when it changes; `:plugins` shows what is in force. The
  shipped manifest used to grant a fixed path (the author's notes folder),
  which you had to edit by hand and which every update put back.
- **`org.directory` defaults to `~/org` and `org.roam-directory` to
  `~/org-roam`**, emacs's and org-roam's own defaults. Both used to be
  unset. A directory that does not exist changes nothing: roam stays off
  until `org.roam-directory` exists, and capture drafts fall back as they
  did when `org.directory` was unset.

### Documentation
- **Configuring org.** The README's "Configuring it" shows the two places
  org's options go: an `[org]` table in `lattice.toml` — which, from
  lattice 0.9.11, is held until org loads instead of being reported as
  unknown options — or `config::set_option` in your `init.rs`'s
  `register_options`, which wins where both set one. It previously showed
  an `[org]` table that lattice ignored.

### Upgrading
- If your notes are not in `~/org`, set `org.directory` — in `lattice.toml`
  (`[org]` / `directory = "~/notes"`), in your `init.rs`'s
  `register_options`, or live with `:set`. Without it, archive, refile and capture into files
  outside `~/org` and `~/org-roam` are refused, with a message naming the
  option.

## 0.1.1 — 2026-10-17

Builds against `lattice:plugin-host@0.3.x` (lattice-wit 0.3.0).

### Fixed
- **A relative file link opens the file beside the note.** `<CR>` on
  `[[file:./About/screenshot.png]]` looked for the file relative to the
  directory Lattice was started in, so it only opened when you launched the
  editor from the note's own folder. It now resolves against the folder of the
  note the link is written in, as org does — the same place the inline image
  was already drawn from. Absolute and `~/` paths are unchanged.

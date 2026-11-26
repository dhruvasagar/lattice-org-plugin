# Changelog

## Unreleased

### Changed
- **The README starts with installing.** Installing and configuring now come
  first, and the internals move under "How it is built" and "Developing it".
  The simplest install is listed first: a `[plugins.org]` table in
  `lattice.toml`, which needs no Rust toolchain and no `init.rs` (lattice
  after 0.10.3). An example installs and configures org from that one file.

## 0.6.0 — 2026-11-26

Builds against `lattice:plugin-host@0.7.x` (lattice-wit 0.7.0).

Builds against `lattice:plugin-host@0.7.x` (lattice-wit 0.7.0), and so needs
the lattice release that carries it.

### Added
- **Export: `org-export-mode`**, on in every org buffer and off
  with `:set org.export.enabled=false`. `C-c C-e` opens the dispatcher, emacs's
  `org-export-dispatch`: `h h`, `m m` and `l l` write HTML, Markdown or LaTeX
  beside the file, and `C-s` / `C-b` switch to the subtree at point and to the
  body only, and stay set for the next export. `:org-export-html`,
  `:org-export-markdown` and `:org-export-latex` do the same, with
  `--subtree` and `--body-only`. The export runs in the background and writes
  only where org may write: a target outside `org.directory` is refused at once,
  naming `org.export.directories`, the list that allows more. A large file
  exports in steps, a piece per background call, so no file is too big to
  export and none holds the plugin up while it runs; a second export of the
  same file replaces one still running. The file's `#+OPTIONS:`, `#+TITLE:`,
  `#+SELECT_TAGS:` / `#+EXCLUDE_TAGS:`, `COMMENT` headlines and a subtree's
  `EXPORT_*` properties decide what is exported, with emacs's defaults; what
  the file asks for and the export does not do (`#+SETUPFILE`, an unknown
  option) is named when the export finishes. `{{{macros}}}` expand: `#+MACRO:`
  definitions with `$1`… arguments, and emacs's built-ins (`title`, `author`,
  `date`, `time`, `property`, `keyword`, `n`, `input-file`); an undefined one
  stops the export, naming it. `#+INCLUDE:` brings in another file as emacs does,
  with `:lines`, `:minlevel`, `::*heading` / `::#id`, `:only-contents` and
  `src` / `example` / `export` wrapping; a file org may not read stops the
  export, naming it. Internal links resolve as emacs's do (`#custom-id`,
  `*Heading`, `id:`, targets, names, headline text, radio targets), and a link
  to nothing stops the export unless `#+OPTIONS: broken-links:mark` or `t`.
  HTML is written in emacs's shape and with its class names, so a stylesheet
  made for emacs's output styles it: a table of contents, numbered sections,
  footnotes numbered as referenced, tables with a header and aligned columns,
  figures and captions, source blocks classed by language, math through
  MathJax when there is any, and an author / date footer. Links to other org
  files go to their HTML, and an `id:` link to the node's file.
  `org.export.html-stylesheet` is `default` (org's own, inline), `none`, or a
  stylesheet to link. `h H`, `m M` and `l L` export into `*Org HTML Export*`,
  `*Org MD Export*` or `*Org LATEX Export*` instead, and exporting again
  replaces that buffer's text in one undo step; `h o` and `m o` write the
  file and then open it, in lattice, or with the system's handler when
  `org.export.open-with` is `system`. In lattice an export never takes your
  window: the file or buffer is shown beside the org file, in a vertical
  split by default, and the cursor stays where you are typing.
  `org.export.open-in` is `vsplit`, `split` or `tab` (a new tab you are not
  switched to), and exporting again refreshes the pane that already shows
  it. Each has a `:` command
  (`:org-export-html-buffer`, `:org-export-html-open`, …).
  Markdown is GitHub-flavoured, in emacs's `ox-md` shape: ATX headings (and
  numbered list items below `H:`), the table of contents as a nested list,
  pipe tables with their column alignment, `[^n]` footnotes, fenced code with
  its language, task-list checkboxes, autolinks, `$…$` math, and raw
  `#+begin_export markdown` / `@@md:…@@`. A heading carries an anchor only
  when something links to it, and text that Markdown would read as markup
  stays text. `\vert` puts a `|` in a table cell, as in emacs.
  LaTeX is written in emacs's `ox-latex` shape, so a `#+LATEX_HEADER` written
  for emacs works here: its preamble and default packages, `#+LATEX_CLASS`
  (`article`, `report` or `book`, with their sectioning; another class stops
  the export, naming it), `#+LATEX_CLASS_OPTIONS`, `\hypersetup`, the title
  block and table of contents, headlines below `H:` as list items, `\label` on
  every headline so internal links are `\ref` / `\hyperref`, footnotes at
  their first reference, `tabular` tables, `verbatim` source, figures with
  captions, the ten specials escaped, and raw `#+begin_export latex` /
  `@@latex:…@@` / `#+LATEX:` passed through.
  The LaTeX export is configurable as emacs's is. `org.export.latex-compiler`
  or `#+LATEX_COMPILER` (`pdflatex`, `xelatex`, `lualatex`) picks the
  encoding packages, with `fontspec` for Unicode text and system fonts.
  `org.export.latex-classes` adds classes (`memoir`, a house class), each
  with its sectioning and a header that may place or leave out the default
  packages. `org.export.latex-class`, `-class-options` and `-header` set what
  `#+LATEX_CLASS`, `#+LATEX_CLASS_OPTIONS` and `#+LATEX_HEADER` override, and
  `#+LATEX_HEADER_EXTRA` is read. `org.export.latex-src-block-backend` writes
  source blocks as `listings` or `minted` and adds the package. `#+ATTR_LATEX:`
  sizes an image (`:width`, `:height`, `:options`) and picks a table's
  `:environment` (`longtable`, `tabularx`…) and `:align`, and
  `org.export.latex-image-width` is the default width.
  Every export default is an option, set once rather than in every file and
  overridden by the file: each `#+OPTIONS:` item as emacs's variable for it
  (`org.export.with-toc`, `-section-numbers`, `headline-levels`, `with-tags`,
  `with-drawers`, `with-latex`, `broken-links` and the rest), and
  `org.export.author`, `email`, `language`, `select-tags`, `exclude-tags`,
  `html-head` and `html-head-extra`. A value an option does not take stops
  the export, naming the option and the values it does take.
  **PDF.** `l p` (`:org-export-pdf`) writes the `.tex` and compiles it with
  `org.export.latex-pdf-process`, emacs's `org-latex-pdf-process`: steps of a
  `command` and `args`, run in the file's directory, each after the last
  succeeds, with `%f`, `%b`, `%o` and `%latex` filled in. `l o`
  (`:org-export-pdf-open`) then opens the PDF, which reloads in place when it
  is already open. The compile runs in the background; its output is in
  `*org-export*` (`:org-export-log`), whose headerline says what is running
  and how it ended, and a failing step stops the rest, naming the log. No
  compiler runs until your config file names one (a value set with `:set` is
  not granted), and until then `l p` writes the `.tex` and says how.
  `:org-export-cancel` stops a running export and its compile.
  `:help org.export` is the whole reference: the dispatcher, what is
  exported, source blocks, where the output goes, PDF, and every option.
- **Export honours `:exports`.** A source block exports as emacs exports
  it: `code` (the default) shows the block and leaves its `#+RESULTS:` out,
  `results` shows only the result, `both` shows both, and `none` shows
  neither. The value is inherited as babel inherits it, from the file's and
  the headlines' `header-args`. Before this, every block and every result
  was exported.
- **Export runs the blocks whose results it shows**, as emacs's
  `org-export-use-babel` does: a `results` or `both` block runs through
  babel, with its languages table, `:var` chains, noweb and sessions, and
  its fresh result appears in the export. It runs on a copy, so the file's
  own `#+RESULTS:` are never touched. With `org.babel.confirm` on, the
  export asks once (`Export runs 2 blocks (sh ×2)? (y/n)`). `n` still
  exports, with the results already in the file; `q` exports nothing. `:eval
  no-export` (or `never-export`, `no`, `never`) keeps a block from running
  for an export, so it shows the result already in the file, as does a
  block whose language has no `org.babel.languages` entry (the echo names
  it). With babel off, every block shows the result already in the file.
  `:org-export-cancel` stops the blocks an export is running.

### Fixed
- A link whose path escapes a bracket, `[[file:a\]b.org]]`, opens `a]b.org`
  as in emacs, rather than a file with a backslash in its name.
- `:help org` no longer lists babel among what the plugin does not do; it
  points to `:help org.babel` instead.

## 0.5.0 — 2026-11-24

Builds against `lattice:plugin-host@0.6.x` (lattice-wit 0.6.0).

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
- **Babel sessions.** A block with `:session` runs in an interpreter that
  stays alive, so what one block defines the next can use. `:session name`
  picks a session by name; each belongs to one buffer and runs one block at a
  time. A language gets sessions from its entry's `session-args`
  (`["-u", "-c", "{driver}"]` for python, `[]` for sh). A session ends when
  its buffer closes, on `:org-babel-cancel`, or when the interpreter exits.
  `*org-babel*` logs each start and end.
- **Babel tangles.** `C-c C-v t` (`:org-babel-tangle`) writes each block
  with a `:tangle` to its file: `yes` is the org file's name with the
  language's extension, and a path is relative to the org file. Blocks for
  one file are joined in order (`:padline no` drops the blank line between),
  and `:shebang` heads the file and makes it executable. Each file is
  replaced whole, as one undo step if you have it open. Nothing is written
  if any target is outside `org.directory` and
  `org.babel.tangle-directories`. A block's `:var`s are written into the
  file ahead of it, as readable assignments.
- **Babel's `:var`.** A block can take numbers, strings, a named table
  (as rows) or a named block's `#+RESULTS:` as variables, set ahead of its
  body. python, ruby and node get real values, and sh gets strings. A value
  with quotes, `$` or a newline arrives intact. A name that is not in the
  file refuses the run before anything starts.
- **Babel runs what a block needs first.** When a `:var` names a block with
  no result yet, that block runs first, under the same question, and its
  fresh result is the value. Chains run in order. A failure stops what
  depended on it and says which block it was, and blocks that need each
  other refuse before anything runs.
- **Babel's noweb.** With `:noweb yes`, `<<name>>` in a body becomes the
  named block's body (or the `:noweb-ref name` blocks'), indented as the
  reference is, before the block runs or is tangled. `<<name()>>` becomes
  its result. `:noweb tangle` expands only when tangling, and `eval` only
  when running. A loop refuses, and a name that is not there is left
  alone.
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

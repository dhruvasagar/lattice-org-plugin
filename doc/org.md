# org

Org files (`.org`, `.org_archive`) get syntax highlighting and folding from
this plugin. It contributes the language through lattice's `language` seam —
the grammar and its queries ship inside the plugin, so unloading it takes org
support with it and leaves nothing behind.

## Headlines

Each level gets its own size and colour, `*` through `******`:

```org
* Top level
** Second
*** Third
```

The stars stay at body size while the title scales, sharing one baseline —
the same two-piece rendering markdown's `#` headings get. Beyond six stars a
headline keeps heading styling at level six; org has no depth limit, but the
theme's size ramp does.

This is the one place org's grammar is genuinely harder than markdown's.
Markdown gives each heading level its own node, so a query can name them.
Org's marker is a **single node whose text length is the level**, so the query
compares that text — `#eq?` predicates, which no built-in lattice query had
needed before org.

## Folding

`za` toggles, `zR` opens everything, `zM` closes everything — the ordinary
fold commands, because org folds through the ordinary fold pipeline.

What folds:

| | |
|---|---|
| headlines | the whole subtree beneath, including nested headlines |
| `#+BEGIN_…` / `#+END_…` | source, example, quote and dynamic blocks |
| `:PROPERTIES:` / `:END:` | property drawers and hand-rolled drawers |
| lists, tables, LaTeX environments | when they span more than one line |

Nested headlines fold independently, so closing a top-level headline hides
everything under it while its children keep their own fold state.

A headline with nothing under it is not foldable — there would be nothing to
hide, and a fold marker on every bare headline is noise.

## Source blocks

A `#+BEGIN_SRC` block is highlighted as a block and folds as one. Highlighting
the code *inside* it in its own language is injection, and is not wired yet.

## Moving around

| | |
|---|---|
| `]]` `[[` | next / previous headline, at any level |
| `g{` | up to the parent headline |

These are **motions**, so they compose with operators and take counts the way
vim's own do: `d]]` deletes to the next headline, `3]]` moves down three.

## Structure

| | |
|---|---|
| `<leader>oh` `<leader>ol` | promote / demote the headline |
| `<leader>oH` `<leader>oL` | promote / demote the whole subtree |
| `<leader>oK` `<leader>oJ` | move the subtree up / down past a sibling |
| `<leader><CR>` | new headline at the same level, after this subtree |
| `<leader>o*` | toggle the current line between headline and text |
| `<leader>o$` | archive the subtree into `<this file>_archive` |
| `<Tab>` `<S-Tab>` | cycle this headline / the whole buffer |

Each is one edit, so one `u` undoes it whole — demoting a subtree puts every
star back in a single step, and a subtree move restores both subtrees at once.

Three refusals are deliberate:

- **Promoting a level-1 subtree is refused entirely**, not applied to the
  children that could move. Shifting only those would turn a child into a
  sibling of its own parent.
- **Moving a subtree stops at its parent.** `<leader>oK` swaps with the
  previous *sibling*, skipping over any deeper headlines in between; at either
  end of the chain it does nothing rather than splicing the subtree into
  another parent's children.
- **`<leader><CR>` inserts after the whole subtree**, not on the next line.
  Inserting directly under a headline would put the new one in front of that
  headline's children and silently adopt them.

### Archiving

`<leader>o$` takes the subtree at the cursor out of this file and appends it
to `<this file>_archive`, org's default `org-archive-location`. From inside a
subtree it archives the one you are reading; from a child headline it archives
the child, not its parent.

The archive file is opened in the background and left **modified, not saved** —
`u` undoes the whole move, and `:w` on the archive commits it. If the file did
not exist it is created. The subtree leaves the source only once the write has
landed, so a target that cannot be written leaves your text exactly where it
was.

The plugin needs `fs:write` over the directory your org files live in; without
it the chord is refused at the boundary and says so. Add it to the plugin's
`plugin.toml`:

```toml
capabilities = ["fs:write:/home/you/org"]
```

`<Tab>` is the one key here that falls through: on a headline it cycles
visibility, and anywhere else it means what it usually means. The
`<leader>o` chords are org's alone, so when there is nothing to do they
simply do nothing.

## Tasks

These live in `org-todo-mode`, a minor mode that switches itself on in every
org buffer. `:org-todo-mode` toggles it, and `:set org.enabled=false` keeps it
off — the outliner above is unaffected either way.

| | |
|---|---|
| `<leader>ot` `<leader>oT` | cycle the TODO keyword forward / back |
| `<leader>o,` | cycle the priority |
| `<leader>o:` | set tags, prompting with the current ones |

The empty state is part of both cycles, so a keyword or priority can always be
cleared by cycling past the end rather than deleting it by hand.

Each key acts on the headline you are *under*, so you can mark a task from
anywhere in its body without navigating to it first. Cycling a keyword leaves
the priority and tags exactly where they were.

### Options

| | |
|---|---|
| `org.todo-keywords` | the sequence, default `TODO \| DONE` |
| `org.highest-priority` | last priority letter, default `C` (so A, B, C) |

`\|` separates not-done from done states, as in org's `#+TODO:` line. It is
ignored when cycling. Both options are read on each keypress, so
`:set org.todo-keywords=PROPOSED ACCEPTED REJECTED` takes effect immediately.

## Checkboxes

| | |
|---|---|
| `<C-Space>` | toggle the checkbox on this line |

```org
* Shopping [1/3]
  - [X] bread
  - [ ] milk
  - [ ] eggs
```

Ticking a box updates the nearest statistics cookie above it in the **same
edit**, so one `u` puts both back — a list showing `[2/3]` above one ticked
box is a worse state to be left in than either end.

A cookie keeps its form: `[n/m]` stays a ratio, `[p%]` stays a percentage.
Percentages truncate, as org's do, so a cookie reads `100%` only when
everything is genuinely done.

Cookies count **direct children only**. In a nested list each level rolls up
to its own parent, because a grandchild's state is already reflected in its
parent's box.

`[-]` is org's partial state. Toggling one completes it.

## Tables

These live in `org-table-mode`, a minor mode on org buffers.

| | |
|---|---|
| `<Tab>` `<S-Tab>` | align the table and step a cell forward / back |
| `<leader>o\|` | align without moving |

```org
| Name  | Qty |
|-------+-----|
| bread |   1 |
```

Alignment is a **whole-table** operation — a column is as wide as its widest
cell, so touching one cell can change every row — and it lands as one edit, so
`u` restores the table in a single step.

Stepping past the last cell of a row moves to the next row, so `<Tab>` walks
the table rather than stalling at its right edge. A ragged row is padded
rather than refused: mid-edit is exactly when a table *is* ragged, and that is
when the key is most wanted.

`<Tab>` here is the second hop of a chain. In a table it aligns; on a headline
it falls through to the outline cycle; anywhere else it means whatever `<Tab>`
usually means.

| | |
|---|---|
| `<leader>tK` `<leader>tJ` | move this row up / down |
| `<leader>tH` `<leader>tL` | move this column left / right |
| `<leader>tr` `<leader>tc` | insert a row below / column after |
| `<leader>tdr` `<leader>tdc` | delete this row / column |

The directional letters are the outliner's, so one mnemonic covers subtrees
and table rows alike. The caret follows what it moved.

A separator refuses to be dragged through the body — a rule marks a section,
and moving it would silently re-section the table. The last row and the last
column refuse deletion, because a table with neither is not a table.

Column widths are counted in characters, so accented Latin lines up. CJK and
emoji, which occupy two cells, do not yet.

## Links

| | |
|---|---|
| `<leader>oo` | open the link under the cursor |

Three destinations, decided by what the link says:

| | |
|---|---|
| `[[file:notes.org]]`, `[[notes/a.org]]` | opens as a buffer |
| `[[https://example.com]]` | goes to the system handler |
| `[[*Some Heading]]` | jumps to that headline in this file |

Unlike an image, a link does **not** have to be alone on its line — opening
one mid-sentence is the whole point of the key.

Internal references match the headline **title** exactly, so a `TODO` keyword
or priority on the target does not get in the way, and case matters. A
reference that resolves to nothing says so and leaves the cursor where it is;
jumping to the wrong heading would be worse than not jumping.

## Timestamps

| | |
|---|---|
| `<C-a>` `<C-x>` | step the component under the cursor forward / back |

```org
SCHEDULED: <2026-08-25 Tue>
[2026-08-25 Tue 10:30]
```

The cursor picks the component: the year, month, day, hour or minute it sits
on. Anywhere else in the stamp — including on a bracket or the day name — it
means the **day**, which is what you usually want.

The day name is recomputed on every edit, so it can never disagree with the
date. `31 Jan` stepped by a month lands on the end of February rather than an
impossible `31 Feb`, and a time crossing midnight moves the date rather than
wrapping in place and quietly meaning the wrong day.

These are the one pair of org keys that **fall through**. `<C-a>` / `<C-x>`
are vim's increment and decrement, so off a timestamp they are left to mean
that — org shadows them only where a timestamp actually is. (Lattice has no
increment command yet, so today they simply do nothing off a stamp.)

## Images

`[[file:diagram.png]]` on a line of its own draws the image inline, in the
GPUI build. `<leader>oI` toggles them; `:set org.inline-images=true` is the
persistent form.

| | |
|---|---|
| `<leader>oI` | show / hide inline images |
| `org.inline-images` | bool, default **off** |

Off by default for two reasons. An org file can reference anything, and a
buffer that silently reads and decodes every referenced file the moment it
opens is a surprise. And the terminal build cannot draw images at all — it
shows the link's description (or the file name) in a box of the same height,
so the two builds scroll identically and only one shows pictures.

Only a link **alone on its line** becomes an image. A block occupies whole
rows, so it can only hang below a line; a link inside a sentence would put its
picture on the following row, detached from the text that introduced it.
Emacs draws the same line for the same reason.

`png`, `jpg`, `gif`, `webp`, `svg` and `bmp` are treated as images. Anything
else stays a link — `[[file:notes.org]]` is not a broken picture, and probing
every linked file to find out would read files you never asked about.
Remote links (`[[https://…/a.png]]`) stay links too: fetching them would mean
network traffic you did not ask for.

## Agenda

`:agenda` collects every dated headline across your org files into one view,
ordered by date and grouped by day.

| | |
|---|---|
| `:agenda` | build it over the current project |
| `:agenda ~/notes` | build it over somewhere else |
| `<CR>` | jump to the entry's file and line |
| `gr` | re-scan |
| `<leader>ot` `<leader>oT` `<leader>o,` | change the TODO state or priority, **from the agenda** |

A row is an **open** headline carrying a date:

```org
* TODO Ship the thing
  DEADLINE: <2026-08-25 Tue>
* TODO Water the plants
  SCHEDULED: <2026-08-29 Sat>
* Standup <2026-08-26 Wed 09:30>
```

The rows are real excerpts of the files they came from, not rendered text.
That is what makes the last row of the table above possible: editing in the
agenda edits the file, so you can mark something DONE without opening it.
An agenda you can only read would be a lesser feature wearing the name.

Three things are deliberately **not** rows:

- **A done headline**, however dated. An agenda that lists what you finished
  is a log, not a plan. What counts as done is the right-hand side of the `|`
  in `org.todo-keywords`; a keyword list with no `|` has no done states at all,
  which is org's own rule.
- **An inactive `[2026-08-25 Tue]` stamp.** That is what inactive means —
  counting them would drag every `CLOSED:` line and every logbook entry in.
- **A `SCHEDULED:` that is not directly under its headline.** Only the line
  immediately below counts, because one under a *child* headline belongs to
  the child, and dating the parent with it would send `<CR>` to the wrong line.

Within a day, deadlines come before scheduled items, which come before bare
timestamps; within those, `[#A]` before `[#B]` before no priority — unranked
rather than urgent. Days you have missed are labelled as such, because a
deadline the view stays quiet about is the failure the tool exists to prevent.

The scan runs off the UI thread and reads only the files this plugin claims,
so `:agenda` in a source checkout with no org files in it costs a directory
walk. One malformed file is skipped and the scan continues; if the plugin
stops answering entirely the view keeps the rows it collected and the
headerline says it is partial.

Lattice itself contributes the walk, the ordering and the view — see
`:help agenda-view-mode`. What a *dated row* is comes entirely from here.

## What this plugin is not

Code highlighting *inside* a source block is not here; it needs injection.
Export backends, babel, table formulas, column view and org-roam are out of
scope rather than pending.

Everything above rides seams that already exist (`language`, `modes`,
`grammar`, `config`, `media`, `agenda-source`, `help`) — nothing in lattice
knows what a headline is.

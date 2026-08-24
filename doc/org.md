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

`<Tab>` is the one key here that falls through: on a headline it cycles
visibility, and anywhere else it means what it usually means. The
`<leader>o` chords are org's alone, so when there is nothing to do they
simply do nothing.

## What this plugin is not

TODO cycling, tables, and the agenda are not here yet. Nor is code
highlighting *inside* a source block, which needs injection.

Everything above rides seams that already exist (`language`, `modes`,
`grammar`, `help`) — nothing in lattice knows what a headline is.

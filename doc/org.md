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

## Source blocks

A `#+begin_src` block is highlighted by the language it names, not by org:

```org
#+begin_src rust
fn main() { println!("hi"); }
#+end_src
```

`fn` there is a Rust keyword, coloured by Rust's own grammar. This is the same
mechanism markdown's fenced blocks use — org contributes an injection query and
the host resolves the named language against the SAME registry a top-level
buffer uses, so any bundled language works and so does another plugin's.

Header arguments are fine: `#+begin_src rust :results output` still injects,
because only the block's first parameter is read as the language.

A language nothing has a grammar for — `#+begin_src cobol` — leaves the body
plain rather than failing. Short names resolve through the same aliases the rest
of the editor uses (`rs`, `py`, `js`, `sh`, `rb`, `ts`, `yml`), so
`#+begin_src py` is Python. A name with no grammar and no alias, `emacs-lisp`
being the one org users hit most, is simply plain text.

Only `src` blocks inject. `#+begin_example`, `#+begin_quote` and the rest are
verbatim or prose by definition, and stay that way even when they name a
language.

## Clocking

Track time against an entry. The clock line lives in a `:LOGBOOK:` drawer under
the headline, newest first, and the file is the only record — nothing is kept
elsewhere, so a clock survives a restart and can be closed by any editor.

| chord | command | what it does |
|---|---|---|
| `<leader>oi` | `:org-clock-in` | start a clock on the entry at the cursor |
| `<leader>oO` | `:org-clock-out` | close it, writing the end stamp and elapsed time |
| `<leader>oq` | `:org-clock-cancel` | discard it, leaving no trace |
| `<leader>oj` | `:org-clock-goto` | jump to the entry the clock is on |
| `<leader>oR` | `:org-clock-resume` | start a clock on the last entry clocked |

Each is one registration reachable two ways — the chord and the `:` line run the
same code.

```org
* TODO Write the clocking slice
:LOGBOOK:
CLOCK: [2026-08-29 Sat 10:45]--[2026-08-29 Sat 12:15] =>  1:30
:END:
```

While a clock runs the modeline shows `◷ 0:14` and the entry's headline, updated
once a minute off the keystroke path — typing is never delayed by it.

Times are **local**, not UTC, and the drawer is created for you if the entry has
none. The clock line goes below any `SCHEDULED:`/`DEADLINE:` line and any
`:PROPERTIES:` drawer, which is where org puts it.

`:org-clock-resume` picks the last entry you clocked back up, **wherever it
is** — including a file you do not have open, which it reaches the same way
capture reaches its target. If that file happens to be open with unsaved
changes, save it first: resume reads the file to find the entry, so unsaved
edits can move the line out from under it.

A capture template can start a clock on what it captures:

```toml
[[template]]
key = "t"
description = "todo"
clock-in = true
target = { file = "~/org/inbox.org" }
body = """
* TODO %?
"""
```

The clock line is written as part of the entry, so it is correct the moment the
capture lands.

Clocking in on an entry that already has a running clock is refused rather than
stacking a second one. Clocking out re-reads the buffer rather than trusting
anything remembered, so it works on a clock started before the editor was last
closed. `<leader>oj` and `<leader>oR` are the two that need this session's memory —
after a restart neither has anywhere to go, and both say so. Clocking OUT does
not forget: the entry stays the "last clocked" one, which is what makes resume
work and matches org, where `org-clock-goto` finds the current *or last* clocked
entry.

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

## Text objects

Four, in two pairs — `h` is the headline **line**, `r` is the whole
**subtree**. As everywhere in vim, `i` is *inner* and `a` is *around*: around
takes the structural marker with it, inner leaves it.

| | |
|---|---|
| `ih` | the headline's title, without its stars |
| `ah` | the whole headline line, stars included |
| `ir` | the subtree's body — everything under the headline, headline left standing |
| `ar` | the whole subtree: the headline and everything under it |

They compose with any operator, so `dar` / `yar` / `car` all act on a subtree
and `viw`-style visual selection works too (`var` selects one).

On this file, with the cursor anywhere in `* One` or its body:

```org
* One
body of one
** Child
kid body
* Two
```

| | |
|---|---|
| `dih` | leaves `* ` — the stars stay, the title goes |
| `dah` | the `* One` line empties; `body of one`, `** Child` and `kid body` stay |
| `dir` | `* One` stays; the three lines under it empty |
| `dar` | all four lines empty, `* Two` onward untouched |

**"Empties", not "removes"** — a known wart. The linewise objects resolve to the
end of their last line and stop short of its newline, so deleting one leaves a
blank line where it was. `dd`-style behaviour would close the gap; these do not
yet. Follow it with `dd` if it bothers you.

**`ar` includes nested children**, because that is what a subtree is — the same
definition folding uses (`ar` on `* One` takes `** Child` with it). To act on a
child alone, put the cursor in the child: `dar` on `** Child` takes only the
child and its body.

**They resolve from the parse tree**, so a headline written as an example
inside a `#+BEGIN_SRC` block is not one: `dar` there acts on the real enclosing
subtree rather than on the sample.

`:describe-key ar` and `:describe-command org-around-subtree` will tell you the
same thing without leaving the editor.

## Structure

| | |
|---|---|
| `<leader>oh` `<leader>ol` | promote / demote the headline |
| `<leader>oH` `<leader>oL` | promote / demote the whole subtree |
| `<leader>oK` `<leader>oJ` | move the subtree up / down past a sibling |
| `<leader><CR>` | new headline at the same level, after this subtree |
| `<leader>o*` | toggle the current line between headline and text |
| `<leader>o$` | archive the subtree into `<this file>_archive` |
| `<leader>or` | refile the subtree under a headline you pick |
| `<leader>oc` | capture — opens the template menu, from any buffer |
| `<leader>oa` | the agenda, from any buffer |
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

### Refiling

`<leader>or` opens a picker of every headline in the project's `.org` files —
three levels deep by default — plus each file itself. Choose one and the
subtree at the cursor moves there, landing *after* that headline's whole
subtree rather than in front of its children.

The list is shown as `notes.org  Work / Q3`, so two headings with the same
title under different parents are told apart, and the query matches everything
you can see — including a `TODO` keyword, which is a useful thing to type when
you are looking for where unfinished work lives. `:picker org-refile 5` goes
deeper for one invocation.

You stay where you are. Refile files something; it does not navigate.

### Capturing

`<leader>oc` opens the **capture menu** — one key per template — and the key
you press picks the template. It then prompts for a line and files it through
that template.

**`<leader>oc` and `<leader>oa` work in ANY buffer**, not just org files. That
is the point: the thought you are trying not to lose arrives while you are
reading code, not while you already have an org file open. Everything else
under `<leader>o` acts on an org file and stays inside one.

Templates live in **one option whose value is TOML**:

```toml
[org]
capture-templates = '''
[[template]]
key = "t"
description = "todo"
target = { file = "/home/you/org/refile.org" }
body = """
* TODO %?
%U
"""

[[template]]
key = "m"
description = "Meeting"
target = { file = "/home/you/org/refile.org", headline = "Meetings" }
body = """
* MEETING with %? :meeting:
%U
"""
'''
```

TOML inside a string option is not a style choice: an option can only be a
boolean, an integer or a string, and a template is a record. The `'''` block
carries the payload verbatim, so a `"""` body keeps its newlines. `init.rs`
sets the identical string as a Rust raw literal — one format, both homes.

`target` is either `{ file = "…" }` (append at the end) or
`{ file = "…", headline = "…" }` (after that headline's subtree). A named
headline that is absent appends and says so, rather than creating it or
refusing: the note is not lost, and the echo tells you your target moved.

A template that cannot be used — no `key`, no `target.file`, or a key another
template already took — is skipped and the rest of the set still works. One
typo should not cost you the feature. A set whose **TOML** does not parse
refuses outright and names the option, because a menu built from the half that
survived would be guessing at what you meant.

The older single-template pair still works and is what runs when
`capture-templates` is unset:

```toml
org.capture-file = "/home/you/org/inbox.org"
org.capture-template = "* TODO %?\n  %U"
```

| | |
|---|---|
| `%?` | what you typed. A template without it appends your text on its own line. |
| `%U` | today, inactive: `[2026-08-26 Wed]` |
| `%T` | today, active: `<2026-08-26 Wed>` — the agenda sees this one |
| `%t` | today, active, date only. Same as `%T` for now — `%T` will grow a time of day, `%t` never will |
| `%^{Question}` | asks for a named value. Several become a **fields menu**, not a run of prompts |
| `%a` | a link back to where you fired the capture: `[[file:/path/notes.org::42][notes.org]]` |
| `%%` | a literal `%` |

Anything else is left alone, so a `%d` you meant as text stays a `%d`.

`%a` is what makes capture-while-reading-code useful: the note remembers the
file and line you were looking at, and the link is the shape org itself writes
so following it works in emacs too. A capture fired from a buffer with no file
— a scratch buffer, or the capture menu — expands it to nothing rather than to
a link with an empty target, which would look followable and not be.

A headline target files the note **after that headline's whole subtree**, not
directly under the headline. Filing at the top would put each new note in front
of everything already there, so the subtree would read newest-first while the
file around it reads oldest-first. The headline is matched ignoring case, extra
spacing, a leading TODO keyword and trailing `:tags:` — so adding `:drill:` to
a headline months later does not silently send every future capture to the
bottom of the file.

Neither option has a default on purpose. A key that quietly created
`capture.org` in whichever directory the editor happened to start in would
scatter notes somewhere you would never think to look; unset, `<leader>oc`
tells you to set it.

Both refile and capture need the same `fs:write` grant archiving does, over
the directory your org files live in.

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

`:org-agenda` collects every dated headline across your org files into one
view, ordered by date and grouped by day.

| | |
|---|---|
| `:org-agenda` | build it over your configured agenda files |
| `:org-agenda ~/notes` | build it over somewhere else instead |
| `<leader>oa` | the same, from any buffer |
| `<CR>` | jump to the entry's file and line |
| `gr` | re-scan |
| `<leader>ot` `<leader>oT` `<leader>o,` | change the TODO state or priority, **from the agenda** |

### Which files it scans

Set `org.agenda-files`, one path per line. An entry is a **directory** (walked)
or a **file** (scanned as given, whatever its extension):

```toml
[org]
agenda-files = """
# everything I keep
~/src/dhruvasagar/org-files
# and the one that lives elsewhere
~/src/dhruvasagar/org-files/anniversaries.org
"""
```

`~` is expanded; blank lines and `#` comments are ignored. A path that does not
exist is skipped and the rest still scan — one bad entry must not cost you the
whole agenda.

**Unset, it scans the current project.** That is the old behaviour and it is
still the right one for a repo with org files in it; the option is for the
agenda that follows you between checkouts, which is what org means by an
agenda. `:org-agenda ~/notes` overrides the option for one invocation.

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
so `:org-agenda` in a source checkout with no org files in it costs a directory
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

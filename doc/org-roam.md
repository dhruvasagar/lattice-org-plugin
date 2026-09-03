# org-roam

A zettelkasten layer over the org files you already have. Notes are addressed
by an `:ID:` rather than by where they live, so a link survives a rename, a
move, and a title you changed your mind about.

Nothing here is a separate store. Roam indexes your **files** — the same ones
emacs indexes — so both tools can read the same corpus without either owning
it. There is no database to migrate and no cache to share.

**Roam is inert until you configure it.** With `org.roam-directory` unset there
is no walk, no watcher and no index; an org user who keeps no notes pays
nothing for this existing.

## Turning it on

```toml
org.roam-directory = "~/notes"
```

`~` is expanded. That directory must also be inside this plugin's `fs:read`
grant — the walk reaches nothing without it, and the symptom is an index that
stays empty rather than an error.

Setting the option is enough. Roam walks the directory, arms a filesystem
watcher on it, and builds the index; changing the option later re-walks against
the new root and stops the old scan. You do not have to restart, and you do not
have to set it before the plugin loads — an `init.rs` that sets it from a
`plugin-loaded` handler works, which is the documented pattern.

### Why there is a watcher

Because a corpus is edited from outside the editor. Emacs writes a note, a
`git pull` lands twenty, a sync daemon rewrites a folder. A save hook sees none
of that, and a picker missing notes you know you wrote reads as data loss. The
watcher is debounced host-side and only ever reports paths under the directory
you named.

The boot walk exists for the same reason in reverse: lattice was not running
while the corpus changed, so the watcher cannot report what it never observed.

### Watching it work

A scan of any size shows its progress in the modeline, on the right:

```
⟳ roam 340/706
```

Not an echo. A scan outlives the message line, and a long scan and a dead one
must not look the same.

A large corpus is indexed **across many steps** rather than in one pass — each
step runs for a bounded slice of wall-clock and then rings its own doorbell
until the queue drains. That is why the count climbs instead of the editor
pausing, and it is also why a cold first scan of several hundred files finishes
without anything going wrong.

Files whose content has not changed since the last scan are read and skipped
without being parsed, so a re-scan is much cheaper than the first one.

## What counts as a node

Both grains, because real corpora have both.

| | |
|---|---|
| **File node** | a file whose top-level property drawer carries an `:ID:`. Title from `#+title:`, tags from `#+filetags:` |
| **Headline node** | any headline whose property drawer carries an `:ID:`. Title is the headline text with the TODO keyword, priority and tags stripped |

A headline node's tags include the ones it inherits — from its ancestors and
from the file — already resolved. `:ROAM_ALIASES:` and `:ROAM_REFS:` are read
where present.

Keyword parsing ignores case: `#+TITLE:`, `#+title:`, `#+Filetags:` and
`#+filetags:` are all what org itself writes at different times, and a
case-sensitive reader would index half a corpus.

Ids are compared ignoring case and written uppercase. A link that fails to
resolve because one tool wrote lowercase looks exactly like a missing note,
which is the worst thing a link can look like.

A file with no `#+title:` is still findable — under its slug, which is better
than being invisible.

## Finding a note

`<leader>onf`, or `C-c nf`, or `:org-roam-find-node`. A picker over every node
in the index.

It matches **titles and aliases, never filenames**. The corpus is the argument:
`20250603103551-chicken_breast_honey_garlic.org` holds a note titled *Honey
Garlic Chicken Breast*, and the slug in that filename is a fossil of a title
the note had years ago. Matching filenames would rank your notes by what they
used to be called.

Aliases are not a nicety for the same reason — a note you reach under a name
that is not its title is a note you would otherwise never find.

Picking a node **jumps to it**, file and line, so a headline node lands on its
headline rather than at the top of a file it shares with four other nodes.

### Creating what is not there

The picker always offers one extra row at the bottom while you have typed
something:

```
Create note: Rust Async
```

Two things about it are deliberate. It appears whenever the query is non-empty,
**not** only when nothing matched — otherwise you could never create *Rust*
while *Rust Async* exists, which is exactly when you want to. And it is pinned
last and never ranked, so `<CR>` on a query that has a real match never creates
a duplicate by ranking accident. Creating is always a deliberate `<C-n>` past
the answers.

`:org-roam-create-node Some Title` does the same thing from the command line.

A new note gets a host-minted uppercase id, a filename
`YYYYMMDDHHMMSS-<slug>.org` in your roam directory, and this body:

```org
:PROPERTIES:
:ID:       6F398E54-3B5E-4E6D-9E29-8A0A1C4C1B77
:END:
#+title: Rust Async

```

The slug is the title lowercased, with runs of anything non-alphanumeric
collapsed to a single `_`. Non-ASCII survives — a corpus is not all English,
and transliterating is a guess.

**The new note is an unsaved buffer until you save it.** That is org-roam's own
model, where a new note is a draft you finalize. The watcher indexes it when it
lands on disk, so a draft you abandon never enters the index.

## Linking

### Inserting a link

Type `[[` in an org buffer and completion offers your nodes. Pick one and it
becomes a full id link:

```org
see [[id:6F398E54-3B5E-4E6D-9E29-8A0A1C4C1B77][Rust Async]]
```

This is completion rather than a command because you insert a link
*mid-sentence*, and a normal-mode chord that opens a picker means leaving
Insert, picking, and coming back.

The query carries across spaces, so a multi-word title like *Honey Garlic
Chicken Breast* can be narrowed to. Open the popup **right after the `[[`** for
that: starting to complete in the middle of a title anchors on the last word
instead, and the source declines rather than splicing a link over that one
word.

Nodes are offered only inside an unclosed `[[` in an org buffer — which is also
what keeps a 500-note corpus out of your ordinary word completion. `[[a]] [[b`
completes `b`; `[[a]] x` completes nothing.

The description written into the link is the title *as it stands now*. It is a
snapshot and yours to edit; the `id:` is what keeps resolving after a rename.

The source registers as `gen:org-roam-node`, so
`:set completion.source.gen:org-roam-node.priority=…` moves it in the list.

### Following a link

`<CR>` on an `[[id:…]]` link jumps to the node — file **and** line.

Three different failures say three different things, because they send you to
three different places:

| | |
|---|---|
| `[[id:…]] needs a note directory — set org.roam-directory` | roam is not configured |
| `the roam index is empty, so [[id:…]] cannot be resolved — run :org-roam-sync` | configured, nothing indexed |
| `no note with id …` | the link is broken |

`<CR>` on anything that is not a link does what it always did.

### Making an existing headline a node

`:org-roam-id-create` gives the headline at the cursor an `:ID:`, which is what
makes it linkable. The cursor stays on the headline rather than following the
drawer down.

It refuses rather than guessing when there is nothing to act on: outside a
headline it says so, and on an entry that already has an `:ID:` it leaves the
existing one alone.

## Backlinks

`:org-roam-backlinks` — a picker over the notes that link **to** the node you
are in.

A picker rather than a view, and that is a decision rather than a limitation:
backlinks is *navigation*. You look at what points here and go read it; you do
not sit in the list editing. The agenda is a multibuffer because you change
TODO states in it — a jump list is what going somewhere wants.

A row jumps to the linking **note**, not to the line the link sits on. The
index answers "which notes link here" in one lookup and does not record where
inside them; storing that belongs with a read-in-place view that would need it.

Outside a node — no `:ID:` on the entry you are in or on its file — it says so
rather than opening an empty list.

## The journal

One file per day, `daily/YYYY-MM-DD.org` under your roam directory.

| | | |
|---|---|---|
| `<leader>ondd` | `C-c ndd` | `:org-roam-dailies-today` |
| `<leader>ondy` | `C-c ndy` | `:org-roam-dailies-yesterday` |
| `<leader>ondt` | `C-c ndt` | `:org-roam-dailies-tomorrow` |
| `<leader>ondD` | `C-c ndD` | `:org-roam-dailies-goto-date [YYYY-MM-DD]` |

`:org-roam-dailies-goto-date` with no date asks for one.

These work in **any** buffer, not only org files. The point of "open today's
journal" is that you are somewhere else when you want it.

The whole feature is a filename: a date names exactly one path, so there is no
registry and no state to get out of step. Dates come from your local clock, not
UTC — a journal filed under yesterday because you live east of Greenwich is
invisible until the one night it eats an entry.

`daily/` is created when it is missing, so the very first
`:org-roam-dailies-today` on a fresh corpus works.

**A new journal entry is a node**, with its own `:ID:` and `#+title:`, because
without one a day cannot be linked to — and linking a day to what happened on
it is most of what a journal is for.

Whether a day's file already exists is answered by reading it, never by the
index: the index lags the watcher by a debounce, and "absent" here would mean
writing the header onto a file that already has one.

## Templates

Unset, creating a note writes the built-in stub above and asks nothing. That is
the default on purpose — note creation has to work for someone who has never
heard of templates.

Set `org.roam-capture-templates` and creating a note opens a **menu**, one key
per template, the same shape `<leader>oc` uses for capture:

```toml
[org]
roam-capture-templates = '''
[[template]]
key = "d"
description = "default"
body = """
:PROPERTIES:
:ID:       ${id}
:END:
#+title: ${title}
#+filetags: :note:

"""

[[template]]
key = "c"
description = "concept"
file = "${slug}.org"
body = """
:PROPERTIES:
:ID:       ${id}
:END:
#+Title: ${title}
#+date: %U

* Summary
"""
'''
```

Roam templates are capture templates with one addition, and one subtraction.

**Added: `${…}`, which interpolates the node being created.**

| | |
|---|---|
| `${title}` | the title you typed |
| `${slug}` | its slug — `rust_async` |
| `${id}` | the minted id |

**Removed: `target`.** A capture template says *where* its text lands; a roam
note's destination is a file that does not exist yet, named after the node
being made. So there is no `target`, and an optional `file` names the note's
**filename** instead — `${…}` expands there too. Absent, the timestamped
default is used.

The two syntaxes answer different questions — `%` interpolates the *capture
context*, `${}` interpolates the *node* — which is why they coexist rather than
compete.

**Not every `%` placeholder works here yet.** A roam note is written straight
out; it does not open the capture buffer `<leader>oc` opens, and the three
placeholders that need one are **silently dropped**:

| | |
|---|---|
| `%U` `%T` `%t` | ✅ dates, exactly as in a capture template |
| `%%` | ✅ a literal `%` |
| `%^{Question}` | ⚠️ never asked — expands to nothing |
| `%?` | ⚠️ nothing to place a cursor for — expands to nothing |
| `%a` | ⚠️ no capture origin to link back to — expands to nothing |

That is a gap rather than a design, and it is the half of this feature still
being built: roam should get the same editable buffer capture has, with
`C-c C-c` to file it and `C-c C-k` to throw it away. Until it does, keep roam
templates to text, `${…}` and the date placeholders — a template written
around `%^{…}` will quietly produce a note with the field missing.

An unknown `${x}` is left alone, for the same reason an unknown `%x` is: a
template is your text, and a placeholder that vanished cannot be found and
fixed.

A template missing a `key`, or reusing one another template took, is skipped
and the menu names it in the footer. TOML that does not parse at all refuses
outright and names the option — a menu built from the half that survived would
be guessing.

## Keys and commands

Everything here works in any buffer unless the entry says otherwise.

| Key | Emacs key | Command | |
|---|---|---|---|
| `<leader>onf` | `C-c nf` | `:org-roam-find-node` | find a note by title or alias |
| `<leader>ondd` | `C-c ndd` | `:org-roam-dailies-today` | today's journal |
| `<leader>ondy` | `C-c ndy` | `:org-roam-dailies-yesterday` | yesterday's |
| `<leader>ondt` | `C-c ndt` | `:org-roam-dailies-tomorrow` | tomorrow's |
| `<leader>ondD` | `C-c ndD` | `:org-roam-dailies-goto-date` | a date you are asked for |
| | | `:org-roam-create-node <title>` | create and open a note |
| | | `:org-roam-id-create` | `:ID:` for the headline at point — in an org file |
| | | `:org-roam-backlinks` | what links to the node at point — in an org file |
| | | `:org-roam-sync` | force a full re-scan |
| `<CR>` | | | follow an `[[id:…]]` link — in an org file |

`<leader>on…` is the vim-native spelling; `C-c n…` is emacs org-roam's own
prefix, letter for letter, for a hand that already knows it. Both resolve to
the same command, so there is no second behaviour to keep in step. `C-c` alone
is still vim's interrupt — only `C-c n` continues into org.

`:org-roam-sync` is the escape hatch, not the normal path. Reach for it when
the watcher missed something — a corpus restored from backup, a directory
mounted after the editor started.

## Options

| Option | Default | |
|---|---|---|
| `org.roam-directory` | unset | where your notes live. `~` expanded. Unset means roam is inert |
| `org.roam-dailies-directory` | `daily` | relative to `org.roam-directory` unless it starts with `/` |
| `org.roam-capture-templates` | unset | what a new note starts as. Unset means the built-in stub and no menu |

`:options org.roam-` lists them with their current values, which is more
reliable than this table — it is generated from the plugin you actually have.

These are separate from `org.agenda-files` on purpose. The agenda wants files
with TODOs and dates in them; roam wants files with ids. For a working setup
those are different sets, and one option serving both would force them to be
the same.

## When something goes wrong

Every path degrades rather than failing whole.

- **A malformed file during a scan** is skipped and the scan continues. One bad
  file must not cost you the index.
- **A watcher that cannot be armed** falls back to indexing on boot plus
  `:org-roam-sync`. Degraded and honest, rather than appearing to work and
  going stale.
- **A store that is corrupt or from an older version** is discarded and rebuilt
  whole. A partial rebuild from bytes that failed their own check is how a
  cache starts serving plausible nonsense.
- **An id minted while the system has no entropy** refuses and says so, rather
  than writing an empty `:ID:` into your file. An id outlives the session and
  every other tool's view of that note.
- **The same `:ID:` in two files** resolves to one of them, and nothing tells
  you which. That is a real gap: the collision is a problem in your corpus and
  it should be reported. It is not, yet.

Roam's diagnostics go to `debug`, never `info` — a watcher over 700 files
during a `git pull` would flood `*messages*`. Run with `--log-level debug` when
you want to see them.

## What is not here

**Cut, rather than pending:**

- **A graph view.** It needs a layout engine and a canvas, and the question
  people open a graph to answer — *what connects to this?* — is what backlinks
  answers.
- **Unlinked references** — every mention of a title that is not a link. A real
  feature, and a full-text scan of the corpus per query; it wants the index to
  carry something it does not carry today.
- **`org-roam-db` compatibility.** We index the same files emacs does, not
  emacs's SQLite cache. Sharing the cache would couple this to org-roam's
  schema version and its migration timing; sharing files couples it to org,
  which is the durable thing. Both tools index independently, which is cheap.
- **org-roam-protocol** — capture from a browser. It needs a URL handler and an
  external-invocation path that has no equivalent here yet.

**Deferred, not cut:**

- **`:ROAM_REFS:` search.** The field is indexed already; a picker over it waits
  until a corpus has enough refs to be worth a command.
- **An insert-node picker.** Completion covers inserting a link mid-sentence,
  which is what it is for. The picker form is for the case completion cannot
  serve.
- **A read-in-place backlinks view.** Buildable on the multibuffer seam
  whenever someone wants one — it is not what navigating wants.

See [`org`](help:org) for everything else the plugin does.

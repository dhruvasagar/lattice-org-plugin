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

That is the zero-configuration path. With `org.roam-capture-templates` set you
get a template menu and then an editable draft instead — see
[Templates](#templates).

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

### Notes started inside a capture

Start a note with `C-c n f` → create while you are writing another capture,
and the two are connected by **one** link, in the direction that fits:

- `C-c n i` → create links **forward**: the link goes into the capture you
  were writing (below).
- `C-c n f` → create links **back**: the new note gets a link to the capture
  you were writing, and nothing is written into that capture. Filing the new
  note returns you to it.

The link back goes where the template says `${origin}`. A template that does
not mention it gets a `Reference: <link>` line at the end instead. What it
points at:

| The capture you were writing | `${origin}` |
|---|---|
| a new roam note (it has an `:ID:`) | `[[id:…][its title]]`, which keeps working once that note is filed |
| anything else | `[[file:…][its title]]` to the file it will be filed into. Its draft file is deleted when it is filed, so a link to the draft would break |

No `:ID:` is ever added to the capture you were writing, and nothing in it
changes. `org.roam-capture-reference-origin = false` turns the link back off;
`${origin}` is then empty.

Started anywhere other than a capture, a new note has no link back, and filing
it opens it, as `org-roam-node-find` does in emacs.

### From a picker: `C-c n i`

`C-c n i` in Insert mode opens a picker over every node. Choosing one inserts
its link at the cursor, and you carry on typing. Use it when you do not
remember the title well enough for completion.

**Select text first** and `C-c n i` in Visual mode opens the picker already
searching for that text. The link then **replaces** the selection, which is
org-roam's `org-roam-node-insert` with an active region. Select a phrase in
your prose and it becomes a link.

While you have typed something, the picker offers `Create and link: <title>`
as its last row. With `org.roam-capture-templates` set, that row starts an
ordinary note capture: the template menu, its questions, a draft. The link is
**not** inserted yet; it is written into your text when you file the draft
with `C-c C-c`, at the place you started from, and you are returned there.
Throw the draft away with `C-c C-k` and no link is written.

That makes notes **nest**. Inside a draft, `C-c n i` → create starts another
draft whose link goes into the first, and so on as deep as you like. Each draft
remembers its own caller, so you can file them in any order, leave one open
and come back to it later, and the caller can be any buffer, not just an org
file.

Two things can happen to the caller while a draft is open:

- **It got shorter.** The link goes to the nearest place that still exists,
  such as the end of a line you shortened, rather than being lost.
- **It was closed.** The note is still filed, and the message says the link
  had nowhere to go and shows it, so you can paste it yourself.

That includes filing or discarding the caller first when it is itself a
draft. Doing so warns how many captures started from it can no longer insert
their links.

Without templates, `Create and link` stays a single step: the note is written
from the built-in stub, saved, and linked at once.

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

When templates are unset, creating a note writes the built-in stub above and
asks nothing. That default is deliberate: note creation has to work for
someone who has never heard of templates.

Set `org.roam-capture-templates` and creating a note opens a **menu** instead,
with one key per template, the same shape `<leader>oc` uses for capture:

```toml
[[org.roam-capture-templates]]
key = "d"
description = "default"
target = { kind = "file", file = "%<%Y%m%d%H%M%S>-${slug}.org" }
body = """
#+title: ${title}
#+filetags: :note:

%?
"""

[[org.roam-capture-templates]]
key = "c"
description = "concept"
target = { kind = "file+head", file = "concepts/${slug}.org", head = "#+title: ${title}\n#+category: %^{Category}\n#+date: %U" }
body = """
* Summary
%?
"""
```

**A roam template is a capture template.** It has the same fields, the same
target kinds, `type`, `body-file`, `seed`, and the same placeholders. The
[capture documentation](help:org#templates) covers all of them. Roam differs in
four places.

**`target` is required**, as it is in org-roam, which refuses a template
without `:target`. Its `file` is where the note goes:

- `${…}` and `%<fmt>` are filled in first.
- A relative path is then taken as relative to `org.roam-directory`. An
  absolute or `~/…` path is used as written.
- `file = "%<%Y%m%d%H%M%S>-${slug}.org"` is org-roam's own default name.

**Two more target kinds**, org-roam's:

| `kind` | Fields | |
|---|---|---|
| `file+head` | `file`, `head`? | `file`, plus `head` written at the top **when the capture creates the file** |
| `file+head+olp` | `file`, `olp`, `head`? | `file+olp`, with the same `head` rule |

A capture into a file that already exists writes only the body; its head and
`:ID:` are already there. Whether the file is new is decided when the draft
opens, so the draft shows exactly what will be written. The capture list
refuses both kinds by name, because `org-capture` has neither.

**`${…}` fills in the node being created**, in the body, the head, the target
path and a `body-file` path:

| | |
|---|---|
| `${title}` | the title you typed |
| `${slug}` | its slug: `rust_async` |
| `${id}` | the minted id |
| `${origin}` | a link back to the capture this note was started from, or nothing (see [Notes started inside a capture](#notes-started-inside-a-capture)) |

The `%` placeholders fill in the *capture context* and `${}` fills in the
*node*, which is why the two syntaxes live side by side. An unknown `${x}` is
left alone, for the same reason an unknown `%x` is: the template is your text,
and a placeholder that silently vanished could not be found and fixed. In a
roam template, `%a` expands to nothing, because a new note has no buffer to
link back to. `%^{…}` questions are asked before the draft opens, one prompt
each, in template order.

**The `:ID:` is org-roam's job, not the template's.** A new file always gets a
`:PROPERTIES:` drawer with the minted id at the top, above the head. A template
that already writes `:ID: ${id}` is left as it is. So a template may have no
`body` at all: a head alone is a complete note, and an empty body works as
`%?`, as in emacs.

`body-file` reads the body from a file, which is emacs org-roam's
`(file "…/template.org")`:

```toml
[[org.roam-capture-templates]]
key = "c"
description = "concept"
target = { kind = "file", file = "${slug}.org" }
body-file = "~/org-files/roam/templates/pkos-concept.org"
```

Keeping templates as org files you edit directly means there is no inlined
copy in `init.rs` to drift out of step with them. A `body-file` you cannot read
stops the create with a message naming the template and the path, and nothing
is written. Reading it needs no extra capability: `fs:write:<prefix>` also
permits reading under `<prefix>`. A template file outside every granted prefix
behaves like a missing one.

### The draft

Choosing a template does not write the note. It opens a **draft**: the same
capture buffer `<leader>oc` opens, holding the template expanded, with the
cursor where `%?` was.

| | |
|---|---|
| `C-c C-c` | file it: the note is written into its target and saved. Started inside another capture, you return there; started anywhere else, the new note is shown |
| `C-c C-k` | throw it away. **Nothing is created, not even the id** |
| `:w` | keep the draft to finish later |
| `<leader>oC` | reopen a kept draft |

A roam draft is an ordinary capture draft, so everything in
[A capture is a file](help:org#a-capture-is-a-file) applies:

- several can be open at once, and each files into its own target;
- a draft survives a restart once saved;
- the target is checked before the draft opens;
- a write that fails leaves the draft open.

Roam drafts live in the capture drafts directory. The index skips that
directory even when it is inside `org.roam-directory`, so a saved,
unfinished note never shows up in find-node. It becomes a node when it is
filed.

Edit the draft freely first. What gets filed is what is on screen when you
press `C-c C-c`, not the template you started from.

**An abandoned draft leaves nothing behind.** The note is created on
`C-c C-c`, so `C-c C-k` has nothing to undo, and the id minted for the note is
simply dropped. Before drafts, picking the wrong template cost you a file with
a real `:ID:` in it, which the indexer would then pick up.

**The zero-template path stays one step.** With `org.roam-capture-templates`
unset there is no menu and no draft: creating a note opens the built-in stub
directly, at its real path, with the cursor at the end. A stub has no `%?` and
no questions, so a draft would only add a `C-c C-c` to the one flow that should
cost nothing.

A template that cannot be used is skipped, and the menu names it in its
footer. That covers an empty `key`, a key already taken, `body` and
`body-file` both set, or a target that does not resolve. (A template with no
`target` at all does not fit the option's shape, so it is rejected when the
option is set.) A set that failed
to load entirely is **reported**, and the menu stays closed. It does not fall
back to the stub, because a note created from a template you did not choose is
worse than no note. A `body-file` that cannot be read is caught when you pick
that template, because only then is the node known well enough to fill in its
path.

## Keys and commands

Everything here works in any buffer unless the entry says otherwise.

| Key | Emacs key | Command | |
|---|---|---|---|
| `<leader>onf` | `C-c nf` | `:org-roam-find-node` | find a note by title or alias |
| `<leader>ondd` | `C-c ndd` | `:org-roam-dailies-today` | today's journal |
| `<leader>ondy` | `C-c ndy` | `:org-roam-dailies-yesterday` | yesterday's |
| `<leader>ondt` | `C-c ndt` | `:org-roam-dailies-tomorrow` | tomorrow's |
| `<leader>ondD` | `C-c ndD` | `:org-roam-dailies-goto-date` | a date you are asked for |
| `<leader>oC` | | `:org-capture-drafts` | reopen a kept capture or note draft |
| `C-c n i` | `C-c n i` | `:org-roam-insert-node` | pick a note and link it at the cursor — Insert mode; in Visual, the selection becomes the link |
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
| `org.roam-capture-reference-origin` | `true` | give a note started inside another capture a link back to it |

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

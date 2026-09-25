# The `org-roam-insert` picker

Insert a link to an org-roam note at the cursor. Type to filter by title
or alias. Open it with `:org-roam-insert-node` or
`:picker org-roam-insert`. See `:help roam`.

---

## Keys in this picker

| Key | Here |
|---|---|
| *(type)* | Filter by title **or alias** |
| `<CR>` | Insert a link to the selected note (`[[id:…][title]]`) at the cursor |
| `<CR>` on **`Create and link: …`** | Create a new note titled with what you typed, then insert a link to it. Pinned last, so it never wins by ranking accident |
| `<C-n>` / `<C-p>`, `<Down>` / `<Up>`, `<Tab>` / `<S-Tab>` | Move the selection |
| `<BS>` / `<C-w>` | Delete a character / the previous word of the query |
| `<C-r>` | Append something from your yank history to the query |
| `<Esc>` / `<C-c>` | Close |
| `<C-h>` | This page |

The link is by **id**, not title, so renaming the target note later
leaves the link intact.

---

## What is listed

The same notes the [`org-roam-node`](help:org.picker-org-roam-node)
picker lists — one row per note, titles rendered as headlines, aliases
and `:tags:` beside them — read once from the org-roam index. The
difference is the verb: this one links from the buffer you are in rather
than opening the note.

---

## See also

- `:help roam` — id-addressed notes, backlinks, dailies and templates.
- [`org-roam-node`](help:org.picker-org-roam-node) — find and open a note.
- [`picker`](help:picker) — keys and options shared by every picker.

# The `org-roam-backlinks` picker

The notes that link **to** the note at point — org-roam's answer to
"what refers to this?". Open it with `:org-roam-backlinks` or
`:picker org-roam-backlinks`. See `:help roam`.

---

## Keys in this picker

| Key | Here |
|---|---|
| *(type)* | Filter the linking notes by title |
| `<CR>` | Jump to the note that links here |
| `<C-n>` / `<C-p>`, `<Down>` / `<Up>`, `<Tab>` / `<S-Tab>` | Move the selection |
| `<BS>` / `<C-w>` | Delete a character / the previous word of the query |
| `<C-r>` | Append something from your yank history to the query |
| `<Esc>` / `<C-c>` | Close |
| `<C-h>` | This page |

There is no create row: a backlink is a fact about existing notes, not
something you make from here.

---

## What is listed

One row per note that contains a link to the current note, read from the
org-roam index. An empty list means nothing links here yet — a note
worth linking from somewhere. Open it from within a roam note; with no
note at point there is nothing to find backlinks for.

---

## See also

- `:help roam` — id-addressed notes, backlinks, dailies and templates.
- [`org-roam-node`](help:org.picker-org-roam-node) — find and open a note.
- [`picker`](help:picker) — keys and options shared by every picker.

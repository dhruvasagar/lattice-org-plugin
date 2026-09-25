# The `org-roam-node` picker

Every org-roam note, by title. Type to filter; aliases are searchable
too, and each row shows its aliases and `:tags:` beside it. Open it with
`:org-roam-find-node` or `:picker org-roam-node`. See `:help roam` for
the whole idea.

---

## Keys in this picker

| Key | Here |
|---|---|
| *(type)* | Filter by title **or alias** |
| `<CR>` | Open the selected note |
| `<CR>` on **`Create note: …`** | Create a new note titled with what you typed (a fresh file with an `:ID:`), then open it. Pinned last, so it never wins by ranking accident — you can make *Rust* while *Rust Async* exists |
| `<C-n>` / `<C-p>`, `<Down>` / `<Up>`, `<Tab>` / `<S-Tab>` | Move the selection |
| `<BS>` / `<C-w>` | Delete a character / the previous word of the query |
| `<C-r>` | Append something from your yank history to the query |
| `<Esc>` / `<C-c>` | Close |
| `<C-h>` | This page |

`<C-d>` does **not** delete a note here — removing a note is a
filesystem delete, which belongs to oil and the file tree, not to a
keystroke in a list.

---

## What is listed

One row per note, its title rendered as the headline it is. Aliases show
in a descriptive column and `:tags:` in a classifying one; both are shown
beside the row, and aliases are matched as part of the row's text so a
note findable only under an alias still turns up. The list is the
org-roam index, read once when the picker opens.

The picker is empty and says so when `org.roam-directory` is unset —
which is a different problem from having no notes, and has a different
fix.

---

## See also

- `:help roam` — id-addressed notes, backlinks, dailies and templates.
- [`org-roam-insert`](help:org.picker-org-roam-insert) — link to a note
  from the buffer you are in.
- [`org-roam-backlinks`](help:org.picker-org-roam-backlinks) — the notes
  that link *to* this one.
- [`picker`](help:picker) — keys and options shared by every picker.

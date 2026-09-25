# The `org-capture-drafts` picker

Capture drafts — saved or still open — waiting to be resumed. Open it
with `:org-capture-resume` or `:picker org-capture-drafts`. See
`:help org`.

---

## Keys in this picker

| Key | Here |
|---|---|
| *(type)* | Filter the drafts |
| `<CR>` | Reopen the selected draft to finish it |
| `<C-n>` / `<C-p>`, `<Down>` / `<Up>`, `<Tab>` / `<S-Tab>` | Move the selection |
| `<BS>` / `<C-w>` | Delete a character / the previous word of the query |
| `<C-r>` | Append something from your yank history to the query |
| `<Esc>` / `<C-c>` | Close |
| `<C-h>` | This page |

---

## What is listed

One row per capture draft — a capture you began and either saved for
later or left open. Resuming one puts you back in the capture buffer
where you stopped; finishing it files the entry as an ordinary capture
would. The list is the same in every project.

---

## See also

- `:help org` — headlines, folding and what this plugin does.
- `:help roam` — captures that become roam notes.
- [`picker`](help:picker) — keys and options shared by every picker.

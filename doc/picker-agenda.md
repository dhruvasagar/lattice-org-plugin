# The `agenda` picker

Every dated row across your agenda files, in one list — a jump-to view of
what is scheduled, deadlined or timestamped. Open it with `:picker
agenda`. See `:help org`.

---

## Keys in this picker

| Key | Here |
|---|---|
| *(type)* | Filter the rows by headline text |
| `<CR>` | Jump to the entry's source in its org file |
| `<C-n>` / `<C-p>`, `<Down>` / `<Up>`, `<Tab>` / `<S-Tab>` | Move the selection |
| `<BS>` / `<C-w>` | Delete a character / the previous word of the query |
| `<C-r>` | Append something from your yank history to the query |
| `<Esc>` / `<C-c>` | Close |
| `<C-h>` | This page |

---

## What is listed

One row per dated entry — a `SCHEDULED:`, `DEADLINE:` or plain active
timestamp — found by scanning `org.agenda-files` (or the project root when
that is unset). Archives are never scanned. This is a quick jump list; the
full agenda **view**, with its own mode and chords, is `org-agenda-mode`.

---

## See also

- `:help org` — headlines, folding and what this plugin does.
- [`picker`](help:picker) — keys and options shared by every picker.

# The `org-refile` picker

Choose the headline to move the current subtree **under**. Type to filter
the candidate headlines. Open it with `:org-refile` or
`:picker org-refile`. See `:help org`.

---

## Keys in this picker

| Key | Here |
|---|---|
| *(type)* | Filter the target headlines |
| `<CR>` | Refile the current subtree under the selected headline |
| `<C-n>` / `<C-p>`, `<Down>` / `<Up>`, `<Tab>` / `<S-Tab>` | Move the selection |
| `<BS>` / `<C-w>` | Delete a character / the previous word of the query |
| `<C-r>` | Append something from your yank history to the query |
| `<Esc>` / `<C-c>` | Close |
| `<C-h>` | This page |

---

## What is listed

One row per candidate headline a subtree can be refiled under, drawn from
your org files. The subtree that moves is the one at point when you open
the picker, so open it with the cursor on (or in) the entry you mean to
move.

---

## See also

- `:help org` — headlines, folding and what this plugin does.
- [`picker`](help:picker) — keys and options shared by every picker.

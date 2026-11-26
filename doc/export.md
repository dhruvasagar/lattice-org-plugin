# Org export

Turn an org file into HTML, Markdown (GitHub-flavoured) or LaTeX, and LaTeX
into a PDF, as emacs' `ox` does: the same `C-c C-e` dispatcher and letters,
the same `#+OPTIONS:` line and keywords, the same tags to select and exclude.
A file written for emacs' export exports the same way here.

Export is `org-export-mode`, on in every org buffer. `org.export.enabled =
false` turns it off and leaves the rest of org on.

## Exporting

`C-c C-e` opens the dispatcher. Press the two letters of what you want:

| keys | does | command |
|---|---|---|
| `h h` | HTML, to `notes.html` beside the file | `:org-export-html` |
| `h H` | HTML, into the buffer `*Org HTML Export*` | `:org-export-html-buffer` |
| `h o` | HTML to the file, then open it | `:org-export-html-open` |
| `m m` / `m M` / `m o` | the same for Markdown, `notes.md` | `:org-export-markdown`… |
| `l l` / `l L` | LaTeX, to `notes.tex` / into a buffer | `:org-export-latex`… |
| `l p` | LaTeX, then compiled to `notes.pdf` | `:org-export-pdf` |
| `l o` | the PDF, then open it | `:org-export-pdf-open` |
| `C-s` | toggle: the subtree at point only | `--subtree` |
| `C-b` | toggle: the body only, no preamble or `<head>` | `--body-only` |

`C-s` and `C-b` flip in place and stay set for the session, as emacs' do. The
commands take the same switches: `:org-export-html --subtree`.

The export runs in the background. The echo area says `Exporting notes.org →
notes.html…`, then `Exported notes.html` or what stopped it, and you can keep
typing meanwhile. `:org-export-cancel` stops every export running, and the
PDF compiles with them.

## What is exported

Everything in the file, less what you leave out:

- A `:noexport:` subtree, and a `COMMENT` headline, are dropped. When a
  headline carries `:export:`, only the subtrees tagged so (and the headlines
  above them) are kept. `#+SELECT_TAGS:` and `#+EXCLUDE_TAGS:` name other
  tags; `org.export.select-tags` and `org.export.exclude-tags` change the
  defaults.
- `#+OPTIONS:` turns the parts on and off, with emacs' items and defaults:
  `toc:`, `num:`, `H:`, `author:`, `date:`, `title:`, `email:`, `tags:`,
  `todo:`, `pri:`, `<:` (timestamps), `p:` (planning), `d:` (drawers), `f:`
  (footnotes), `tex:`, `broken-links:`, `*:`, `-:`, `::`, `|:`, `e:`, `^:`
  and `\n:`. Each has an option, `org.export.with-toc` and so on, holding the
  default the line overrides.
- `#+TITLE:`, `#+SUBTITLE:`, `#+AUTHOR:`, `#+EMAIL:`, `#+DATE:` and
  `#+LANGUAGE:` fill the title block. `org.export.author`, `org.export.email`
  and `org.export.language` stand in for a file with none.
- A subtree export reads the headline's `EXPORT_*` properties
  (`:EXPORT_TITLE:`, `:EXPORT_OPTIONS:`, …) over the file's keywords.
- `#+INCLUDE: "other.org"` exports that file in its place, and `#+MACRO:`
  defines the `{{{name(args)}}}` the text uses.
- `#+CAPTION:`, `#+NAME:`, `#+ATTR_HTML:` and `#+ATTR_LATEX:` apply to the
  table, image or block under them.

An internal link (`[[#custom-id]]`, `[[*Headline]]`, `[[name]]`,
`[[target]]`) becomes a link in the output. One that points at nothing stops
the export and is named, unless `#+OPTIONS: broken-links:t` (leave it out) or
`broken-links:mark` (mark it).

`#+INCLUDE` reads only what org may read: a file outside the directories org
is given is an error naming it, not a silent gap.

## Source blocks

A block's `:exports` header says what of it is exported, as in emacs:

| `:exports` | the output shows | the block runs |
|---|---|---|
| `code` (the default) | the code | no |
| `results` | its result | yes |
| `both` | the code, then its result | yes |
| `none` | nothing | no |

A block runs through [babel](help:org.babel), so only a language in
`org.babel.languages` runs, with the same header arguments, `:var`, noweb
and sessions as `C-c C-c`. The export runs on a copy: an export never
rewrites your file's `#+RESULTS:`.

Before anything runs, the export asks once, `Export runs 3 blocks (sh ×2,
python)?`. `y` runs them; `n` exports with the results already in the file;
`q` exports nothing.
`org.babel.confirm = false` skips the question. A block marked `:eval
no-export` or `:eval never-export` never runs during export, and one in a
language you have not configured uses the result already in the file, as
every block does with babel turned off.

## Where the output goes

**Files** are written beside the org file, `notes.org` → `notes.html`. Org may
write only under `org.directory`, `org.roam-directory` and the directories
you list in `org.export.directories`, which is empty by default. To export a
project's `README.org`, list the project:

```toml
[org.export]
directories = ["~/src/my-project"]
```

An export anywhere else is refused, and the echo says why.

**Buffers** (`h H`, `m M`, `l L`) are `*Org HTML Export*` and its peers.
Exporting again replaces the text, as one change you can undo, and keeps your
place in it.

**Opening** (`h o`, `m o`, `l o`) follows `org.export.open-with`: `editor`
shows the file in lattice, `system` hands it to your system's viewer (a
browser for HTML). In lattice the output never takes your window: it is
shown beside the org file, and the cursor stays where you were typing.
`org.export.open-in` says where: `vsplit` (the default), `split`, or `tab`,
a new tab you are not switched to. A pane already showing the output is
reused, so exporting again refreshes it in place.

## PDF

`l p` writes the `.tex`, then runs the steps you give in
`org.export.latex-pdf-process`, each in the file's directory, each after the
last succeeds. Nothing runs until you name a step, and only in your config
file: a value set with `:set` is not granted.

```toml
[[org.export.latex-pdf-process]]
command = "latexmk"
args    = ["-pdf", "-pdflatex=%latex", "-interaction=nonstopmode",
           "-output-directory=%o", "%f"]
```

In an argument `%f` is the `.tex` file, `%b` its name without `.tex`, `%o` its
directory and `%latex` the file's compiler (`org.export.latex-compiler`, or
`#+LATEX_COMPILER:`). The compiler's output is in `*org-export*`
(`:org-export-log`); a failed compile names it in the echo.

`l o` opens the PDF in pdf-mode in the GUI. A PDF already open reloads in
place, on the page you were reading, so `l o` while you write is a live
preview. In the TUI a PDF page is alt text, and `open-with = "system"` is the
better choice.

The class comes from `#+LATEX_CLASS:` or `org.export.latex-class`: `article`,
`report`, `book`, or one you declare in `org.export.latex-classes` with its
section commands. `#+LATEX_HEADER:` lines and `org.export.latex-header` add
to the preamble. Source blocks are `verbatim` unless
`org.export.latex-src-block-backend` is `listings` or `minted`.

## Keys and commands

| key | command | does |
|---|---|---|
| `C-c C-e` | `org-export-dispatch` | open the dispatcher (above) |
| | `:org-export-html`, `-markdown`, `-latex` | export to the file beside this one |
| | `:org-export-html-buffer`, `-markdown-buffer`, `-latex-buffer` | export into a buffer |
| | `:org-export-html-open`, `-markdown-open` | export, then open the file |
| | `:org-export-pdf`, `:org-export-pdf-open` | export and compile to PDF; `-open` then opens it |
| | `:org-export-cancel` | stop the exports running, and their compiles |
| | `:org-export-log` | show `*org-export*`, the last compile's output |

## Options

| option | default | |
|---|---|---|
| `org.export.enabled` | `true` | `false` turns export off and leaves the rest of org on |
| `org.export.directories` | none | where an export may write, besides `org.directory` and `org.roam-directory` |
| `org.export.open-with` | `editor` | `editor` or `system` |
| `org.export.open-in` | `vsplit` | `vsplit`, `split` or `tab` |
| `org.export.with-*`, `headline-levels`, `preserve-breaks`, `broken-links` | emacs' | the `#+OPTIONS:` defaults |
| `org.export.author`, `email`, `language` | none, none, `en` | for a file with no keyword |
| `org.export.select-tags`, `exclude-tags` | `export`, `noexport` | what is kept and dropped |
| `org.export.html-stylesheet` | `default` | `default` (org's, inlined), `none`, or a URL or path to link |
| `org.export.html-head`, `html-head-extra` | none | lines for the HTML `<head>` |
| `org.export.latex-compiler` | `pdflatex` | `pdflatex`, `xelatex` or `lualatex` |
| `org.export.latex-class`, `latex-class-options` | `article`, none | the document class |
| `org.export.latex-classes` | none | more classes, with their section commands |
| `org.export.latex-header` | none | preamble lines for every LaTeX export |
| `org.export.latex-src-block-backend` | `verbatim` | `verbatim`, `listings` or `minted` |
| `org.export.latex-image-width` | `.9\linewidth` | an image's width with no `#+ATTR_LATEX:` size |
| `org.export.latex-pdf-process` | none | the steps that make the PDF (above) |

`:options org.export.` lists them with their current values, and
`:describe-option` gives each one's whole text.

## Not here

Visible-only export (`C-v` in emacs' dispatcher) needs the fold state, which
org cannot read yet. Source blocks are not syntax-highlighted in the output:
HTML gets `src-<lang>` classes for a highlight.js or Prism stylesheet, and
LaTeX can use `listings` or `minted`. ODT, texinfo, beamer, iCalendar and
`org-publish` projects are not built. `#+BIND` binds elisp variables, which
lattice does not have.

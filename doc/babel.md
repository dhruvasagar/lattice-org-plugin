# Org babel

Run an org file's source blocks and keep what they print in the file, under
the block, as emacs' org-babel does: the same `#+begin_src` lines, the same
header arguments, the same `#+RESULTS:`. A file babel has run in emacs
reads the same here, and the other way round.

Babel is `org-babel-mode`, on in every org buffer. It runs nothing until you
say what may run.

## Saying what may run

Nothing runs until you add a language to `org.babel.languages`. That list is
also the permission: org may run a program only if it is the `command` of one
of your entries, and it cannot add one itself.

```toml
[[org.babel.languages]]
name    = "sh"          # the word after #+begin_src
command = "sh"          # the program, on PATH or by path
args    = ["{file}"]    # {file} is the block, written to a script (the default)
ext     = "sh"          # the script's extension (default: the name)

[[org.babel.languages]]
name    = "python"
command = "python3"
ext     = "py"
value   = "python"      # a value wrapper: python, ruby, node or sh
```

Listing a program trusts it with whatever it can do, so `sh` lets a block do
anything a shell script can. Changing the list restarts org.

## Running a block

`C-c C-c` in a source block runs it in the background, and you can keep
working. A `▶` in the gutter marks the block while it runs. When it finishes,
its output goes under it as `#+RESULTS:`, replacing what was there, and
undoing that is one `u`.

```org
#+begin_src sh
echo hi
#+end_src

#+RESULTS:
: hi
```

Short output is written as `: ` lines. Output of
`org.babel.min-lines-for-block-output` lines or more goes in an
`#+begin_example` block instead, where a line that would read as org (`* …`,
`#+…`) gets a `,` in front, so output can never turn into a headline. A
program that fails still writes what it printed, and the echo area says how it
ended. A program that could not start at all writes the reason instead.

The result goes where the block is when it finishes, so typing above it while
it runs is fine. If the block itself was changed or deleted, nothing is
written to the file: the output goes to `*org-babel*`, and the echo area
says so. `:org-babel-output` shows that buffer. It never opens by itself,
because a run finishing in the background should not rearrange the window
you are typing in. A block that is already running is not started again.

The block's header arguments are read as emacs reads them: the
`#+PROPERTY: header-args` lines, the `header-args` properties of the headlines
above it, its `#+HEADER:` lines, then its `#+begin_src` line, later winning.

## What a result looks like

`:results` takes babel's words, on the `#+begin_src` line, a `#+HEADER:` or
a `header-args` property:

| axis | words | |
|---|---|---|
| collect | `output`, `value` | what it printed, or its last expression's value |
| type | `verbatim`, `table` | `table` splits each line on tabs, or else on spaces, into an aligned org table |
| format | `raw`, `drawer`, `code`, `org` | written as org; in a `:RESULTS:` drawer; in a `#+begin_src` block of the block's language, or of `org` |
| handling | `replace`, `append`, `prepend`, `silent`, `none` | `silent` shows the result in the echo area only; `none` keeps it nowhere |

The default is `replace`, verbatim. Words babel has that are not built here
(`file`, `list`, `html`, `latex`, `link`, `graphics`, `pp`) are refused before
anything runs, with a message naming them, rather than written some other way.

`value` needs the language to name a **value wrapper**. The wrapper runs the
block and prints its last expression's value after a marker line, and only
what follows the marker is the result, so a block's own `print`s are not.
Python takes either a last expression or a `return`. A shell's value is what
it printed, as in emacs. A language with a wrapper collects `value` unless
you say `output`, as babel does. A language without one runs `value` as
`output`, and the echo area says so once a session.

`:dir` is where the block runs, with `~` expanded. A relative `:dir` is taken
from the file's directory, and a block without one runs in the file's
directory, as in emacs. A `:dir` that is not a directory stops the block from
starting, and the echo area says why.

## Asking first, running many, stopping

With `org.babel.confirm` on (the default) a run asks first: `Run sh block
"disk"? (y/n)`. What runs on `y` is the block the question named, even if
you typed elsewhere in the file while it was open. If that block itself
changed, nothing runs and the echo area says so.

`C-c C-v b` runs every block in the buffer, and `C-c C-v s` every block
under the headline at the cursor. Each asks once for all of them (`Run 4
blocks (sh ×3, python)? (y/n)`). The blocks run one after another, in the
order they appear, so a later block can read what an earlier one wrote. A
block that fails does not stop the rest. A block in a language with no
`org.babel.languages` entry refuses the whole run before anything starts.

`:org-babel-cancel` stops every block running in the buffer and drops the
ones still waiting their turn. A cancelled block keeps the result it had.

`C-c C-v k` deletes the block's `#+RESULTS:` and the blank line above it.
Running a block and then removing its result leaves the file as it was.

## Handing a block values: `:var`

`:var` binds a variable before the body runs:

```org
#+NAME: hosts
| web | 10.0.0.5 |
| db  | 10.0.0.6 |

#+begin_src python :var rows=hosts n=2 who="ops"
print(rows[n - 1][1], who)
#+end_src
```

- A number (`n=2`) or a quoted string (`who="ops"`, with `\"` for a quote).
- The name of a table, which becomes a list of rows. A number in a cell
  arrives as a number.
- The name of a block, which gives its `#+RESULTS:`: a table as rows, one
  line that is a number as a number, anything else as text. A block with a
  result is not run again. One with no result yet runs first, and so does
  one that is part of the same run (`C-c C-v b`), so you get its fresh
  result. One question covers them all, and each writes its own result.

A block that fails stops what needs it: the blocks waiting on its result do
not run, and the echo area names the block that failed. Blocks that need
each other's results (`a` needs `b`, which needs `a`) refuse before
anything runs.

Several go on one `:var`, separated by spaces or commas. A name given in
`#+PROPERTY:` or a headline's drawer and again on the block takes the
block's.

The variables are set by lines put ahead of the body, in the language its
`org.babel.languages` entry names with `value`. python, ruby and node get
real values (numbers, strings, lists of lists). sh gets strings, a table as
lines of tab-separated cells. A language whose entry has no `value` cannot
take `:var`, and the echo area says to add one. Indexing (`hosts[0]`),
calling a block (`x=disk()`) and other files are not supported. Each is
refused by name and nothing runs, as with a name that is not there.

## Putting one block in another: noweb

With `:noweb yes`, `<<name>>` in a body is replaced by the body of the block
named `name` before the block runs or is tangled:

```org
#+NAME: connect
#+begin_src python
db = open_db()
#+end_src

#+begin_src python :noweb yes
def report():
    <<connect>>
    return db.count()
#+end_src
```

- What comes before `<<` on the line goes in front of every line put in its
  place, and what follows `>>` after the last, so the reference above stays
  indented inside `report`.
- With no block named `connect`, the bodies of the blocks marked
  `:noweb-ref connect` are used instead, one after another.
- `<<name()>>` is the block's `#+RESULTS:` instead of its body. A block with
  no result refuses, and arguments (`<<name(x=1)>>`) are not supported.
- A block put in place is expanded in turn when its own `:noweb` allows it.
  Blocks that would put each other in place refuse, and nothing runs.
- A `<<...>>` that names nothing is left as written, so a shell heredoc
  (`cat <<EOF`) is safe.

`:noweb` says when: `yes` always, `tangle` only when tangling, `eval` only
when running, `no` (the default) never. `no-export` and `strip-export` act
as `yes`, since lattice does not export.

## Sessions: `:session`

A block with `:session` runs in an interpreter that stays alive, so what one
block defines the next can use:

```org
#+begin_src python :session
rows = load("disk.csv")
#+end_src

#+begin_src python :session :results value
len(rows)
#+end_src
```

`:session name` picks a session by name, and a bare `:session` is the
language's default one. Each session belongs to one buffer, so two files
never share one. `:session none` (the default) runs the block as a script.

A language has sessions only when its entry says how to start one, with
`session-args`, and names a value wrapper:

```toml
[[org.babel.languages]]
name         = "python"
command      = "python3"
value        = "python"
session-args = ["-u", "-c", "{driver}"]

[[org.babel.languages]]
name         = "sh"
command      = "sh"
value        = "sh"
session-args = []        # sh reads its commands from stdin as it is
```

`{driver}` is a small program, built into the wrapper, that the
interpreter runs instead of its own prompt. It reads the blocks one at a time
and runs each where the last one left off. Ruby and node take it with `-e`
(`["-e", "{driver}"]`). Without `{driver}`, python, ruby or node would never
read a block. A prompt would end up in the results, so the driver is used
instead.

A session starts when its first block runs, in that block's directory. It
runs one block at a time; a block whose session is busy waits for it. An
error in a block is printed as its result and the session carries on. In
node, `let` and `const` last from block to block. Running a block that
declares one again is an error, which `var` avoids.

A session ends:
- when you close its buffer;
- on `:org-babel-cancel`, which also cancels a block it is running;
- when the interpreter exits, for example on an `exit` in a block.

Running a block afterwards starts a fresh session, so `:org-babel-cancel` is
also how you reset one. `*org-babel*` (`:org-babel-output`) logs each
session's start and end.

## Tangling

`C-c C-v t` (`:org-babel-tangle`) writes every block with a `:tangle` header
argument out to a file. Babel's default is `:tangle no`, so a block is
tangled only when it, its headline or the file asks:

```org
#+PROPERTY: header-args:sh :tangle setup.sh

#+begin_src sh :shebang "#!/bin/sh"
echo one
#+end_src
```

- `:tangle yes` names the file after the org file, with the language's
  extension: `notes.org` tangles its python to `notes.py`. The `ext` in the
  language's `org.babel.languages` entry wins when it has one.
- Any other value is a path, relative to the org file. `~` is your home.
- A block's `:noweb` references are expanded in what is written, when its
  `:noweb` is `yes` or `tangle`.
- A block's `:var`s are written ahead of it as plain assignments
  (`n = 5`), in the language its `value` names. One with no `value` is
  written without them, and the echo area says so.
- Blocks tangling to one file are joined in the order they appear, with a
  blank line between two of them unless the later says `:padline no`.
- `:shebang` puts its line first and makes the file executable.

A tangle **replaces** each file it writes, which is what lets you tangle
again after editing. A file you have open changes in place, and one `u`
there brings back what it held. A file you did not have open is opened
unlisted, so `:ls` does not fill up.

Org may write only under `org.directory` and the directories in
`org.babel.tangle-directories`. Every file is checked before any is
written: if one is outside, nothing is tangled and the echo area names it.
A block that would tangle into the org file it is in (an `org` block with
`:tangle yes` does) refuses the tangle the same way. Directories are not
created (no `:mkdirp`).

## Keys and commands

| key | command | does |
|---|---|---|
| `C-c C-c` in a block | `org-babel-execute-src-block` | run it, and replace its `#+RESULTS:` |
| `C-c C-v b` | `:org-babel-execute-buffer` | run every block in the buffer, in order |
| `C-c C-v s` | `:org-babel-execute-subtree` | run every block under the headline |
| `C-c C-v k` | `:org-babel-remove-result` | delete the block's `#+RESULTS:` |
| `C-c C-v t` | `:org-babel-tangle` | write the `:tangle` blocks to their files |
| | `:org-babel-cancel` | stop the blocks running in this buffer, and end its sessions |
| | `:org-babel-output` | show `*org-babel*`, where output with no block to go to lands |

`C-c C-c` anywhere outside a source block is org's, as before.

## Options

| option | default | |
|---|---|---|
| `org.babel.enabled` | `true` | `false` turns babel off and leaves the rest of org on |
| `org.babel.confirm` | `true` | ask before running a block |
| `org.babel.languages` | none | what blocks may run, and how to start their sessions (above) |
| `org.babel.tangle-directories` | none | where tangling may write, besides `org.directory` |
| `org.babel.min-lines-for-block-output` | `10` | results this long become an `#+begin_example` block |
| `org.babel.max-output-lines` | `10000` | output past this is counted, not kept |

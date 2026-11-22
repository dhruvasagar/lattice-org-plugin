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

## Keys and commands

| key | command | does |
|---|---|---|
| `C-c C-c` in a block | `org-babel-execute-src-block` | run it, and replace its `#+RESULTS:` |
| `C-c C-v b` | `:org-babel-execute-buffer` | run every block in the buffer, in order |
| `C-c C-v s` | `:org-babel-execute-subtree` | run every block under the headline |
| `C-c C-v k` | `:org-babel-remove-result` | delete the block's `#+RESULTS:` |
| | `:org-babel-cancel` | stop the blocks running in this buffer |
| | `:org-babel-output` | show `*org-babel*`, where output with no block to go to lands |

`C-c C-c` anywhere outside a source block is org's, as before.

## Options

| option | default | |
|---|---|---|
| `org.babel.enabled` | `true` | `false` turns babel off and leaves the rest of org on |
| `org.babel.confirm` | `true` | ask before running a block |
| `org.babel.languages` | none | what blocks may run (above) |
| `org.babel.tangle-directories` | none | where tangling may write, besides `org.directory` |
| `org.babel.min-lines-for-block-output` | `10` | results this long become an `#+begin_example` block |
| `org.babel.max-output-lines` | `10000` | output past this is counted, not kept |

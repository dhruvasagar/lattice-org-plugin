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
```

Listing a program trusts it with whatever it can do, so `sh` lets a block do
anything a shell script can. Changing the list restarts org.

## Keys and commands

| key | command | does |
|---|---|---|
| `C-c C-c` in a block | `org-babel-execute-src-block` | run it, and replace its `#+RESULTS:` |
| `C-c C-v b` | `:org-babel-execute-buffer` | run every block in the buffer, in order |
| `C-c C-v s` | `:org-babel-execute-subtree` | run every block under the headline |
| `C-c C-v k` | `:org-babel-remove-result` | delete the block's `#+RESULTS:` |
| | `:org-babel-cancel` | stop the blocks running in this buffer |

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

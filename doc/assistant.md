# The org assistant

Org gives lattice's LLM chats tools over your org files, and five workflows
that use them. The model answers from what org reads, the way the agenda
reads it, and changes your files only through org's own commands, each one
shown to you as a diff first.

Nothing here runs until you open a chat. Org reaches no network, so its read
tools run without asking; every change it proposes is reviewed.

## Starting

| Command | Opens a chat on | And asks |
|---|---|---|
| `:org-plan-day` | `org-plan` | Plan my day. |
| `:org-plan-week` | `org-plan` | Plan my week. |
| `:org-review` | `org-review` | Run my weekly review. |
| `:org-report` | `org-report` | Report what I worked on this week. |

Or open any workflow yourself with `:llm <profile>`, and a first question
after it: `:llm org what is waiting on Sam?`

| Profile | For | Tools |
|---|---|---|
| `org` | general help over your org files | `org.*`, `lattice.read` |
| `org-plan` | planning a day or a week | `org.agenda`, `org.query`, `org.entry`, `org.change` |
| `org-review` | a weekly review: done, slipped, stuck | `org.agenda`, `org.query`, `org.clock`, `org.change` |
| `org-report` | a report of work over a period | `org.clock`, `org.query`, `org.entry` |
| `org-research` | researching a topic into your roam notes | `org.notes`, `lattice.read`, `web.*`, `org.change` |

A profile picks no model: org cannot know which ones you have. Choose one per
workflow in your own config, which also widens or narrows its tools:

```toml
[llm.profiles.org-plan]
model = "local/qwen3"
```

## The tools

| Tool | What it does |
|---|---|
| `org.agenda` | what is scheduled, due and overdue for a day, a week or a range, optionally only what matches a tag or TODO expression; repeats included |
| `org.query` | entries by tags, properties or TODO state (`+work/WAITING`, `OWNER="Sam"`), whatever their dates |
| `org.entry` | one entry whole: its notes, properties, log and subheadings |
| `org.clock` | clocked time over a range, per entry, file or tag |
| `org.notes` | org-roam notes by title, alias or tag, with the notes linking to each |
| `org.change` | one change to one entry, as a proposal you review |

They read `org.agenda-files` (else `org.directory`), and the roam index for
notes. Every answer opens with today's date, so a model never has to guess it.

Rows carry a **ref**, `file::line::heading`, which the model hands back to
`org.entry` and `org.change`. A ref whose line has moved still finds its
heading when one headline carries it; otherwise the tool says so and the model
queries again. An entry with an `ID` property can be named by it instead.

## Changes

`org.change` takes what to do, not text to write:

| Op | Does what the key does |
|---|---|
| `set-state` | `<leader>ot`: the keyword's log line, a repeating task repeating, the note an `@` state asks for |
| `schedule`, `deadline` | `s` / `d`, with the same date grammar (`+2d`, `fri`, `2026-11-14 +1w`, `none`) |
| `tag` | adds tags, and `-tag` removes one |
| `add-note` | a `- Note taken on` entry in the log, as `org-add-note` writes |
| `refile` | `<leader>or`: under another headline, as its child, or to a file's top level |
| `capture` | through one of your `org.capture-templates`, its questions answered by the model |

Each change is a diff you accept or decline. In a chat you trust (`<C-t>`), a
change inside the project you opened lattice in applies without one; a change
to your org files outside it is **always** reviewed. A refile across two files
is two diffs, the copy first, so declining the second leaves the entry in both
files rather than in neither.

What org will not do from a chat: create a file (a capture whose target file
does not exist yet is refused), write into an empty one, or start your clock
(a `:clock-in` template captures without clocking in).

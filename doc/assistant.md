# The org assistant

Org gives a language model tools over your org files: what is on your agenda,
which entries match a tag or a state, where your time went, what your roam notes
say. The model answers from what org reads, the way `:org-agenda` reads it, and
changes your files only through org's own commands, each change shown to you as
a diff before it is made.

You can use the tools in three places:

- **In a lattice chat** (`:llm`), with five ready-made workflows: plan a day or
  a week, a weekly review, a report of your work, and research into your notes.
- **In Claude Code, or any other MCP client**, through `lattice --mcp`.
- **In an ACP agent lattice starts** (`:opencode-acp`), with no setup.

Nothing runs until you ask. Org reaches no network, so its read tools run
without asking you; every change it proposes is reviewed.

## Before you start

**A model.** The assistant uses whatever model you have configured for
lattice's chats. Org cannot know which models you have, so it picks none. Set
one up as [`llm-guide`](help:llm-guide) describes under "Setup in five
minutes". A hosted model (Anthropic, OpenAI, Gemini) or a local one (Ollama,
llama.cpp, LM Studio) both work. A local model keeps everything on your machine;
see "Choosing a model" below.

**Your agenda files.** The tools read `org.agenda-files`, the files and
directories `:org-agenda` scans (see [`org`](help:org)). Until you set it, they
read `org.directory` (default `~/org`).

```toml
[org]
agenda-files = """
~/org
~/work/notes.org
"""
```

**For notes, org-roam.** `org.notes` searches your roam index. It finds nothing
until `org.roam-directory` exists and has been indexed; see
[`org.roam`](help:org.roam).

**For research, a web search backend.** The `org-research` workflow uses
`web.search`, which lattice offers only once you name a backend (SearXNG, Brave
or DuckDuckGo); see "`web.search` and `web.fetch`" in
[`llm-guide`](help:llm-guide). `web.fetch` needs no setup.

`:llm-tools` lists org under "Plugins" with each tool and whether it runs, is
reviewed or asks. If org is missing there, see "Troubleshooting".

## Asking in a chat

`:llm org` opens a chat on the general `org` workflow. Put your question after
it to send it straight away:

```
:llm org what is waiting on Sam?
:llm org which of my +work tasks have no date?
:llm org how much time did I spend on the migration this month?
:llm org what have I written about postgres replication?
```

The model chooses the tools; you only ask. It looks before it answers:
`org.query` for what waits on Sam, `org.agenda` for a day, `org.clock` for time,
`org.notes` and then `org.entry` for notes. Every org answer opens with today's
date, so "this week" and "overdue" mean the same to the model as they do to
you.

Ask for a change in the same chat and the model proposes it:

```
mark the dentist appointment done
move the report deadline to friday
file "Call the bank" under Finances
```

Each opens as a diff (see "Changes and review"). The general `org` workflow
changes nothing unless you ask.

Any chat can use org's tools, not only org's workflows: add `org.*`, or single
tools, to a profile of your own.

```toml
[llm.profiles.coding]
tools = ["lattice.read", "lattice.edit", "org.query", "org.entry"]
```

## The workflows

Four commands open a chat on a workflow and ask its first question for you:

| Command | Workflow | First question |
|---|---|---|
| `:org-plan-day` | `org-plan` | Plan my day. |
| `:org-plan-week` | `org-plan` | Plan my week. |
| `:org-review` | `org-review` | Run my weekly review. |
| `:org-report` | `org-report` | Report what I worked on this week. |

All five open with `:llm <workflow>`, with or without a question:
`:llm org-research the history of CRDTs`, `:llm org-report what did I do in
October?`.

| Workflow | For | Tools |
|---|---|---|
| `org` | questions about your org files, and changes you ask for | `org.*`, `lattice.read` |
| `org-plan` | planning a day or a week | `org.agenda`, `org.query`, `org.entry`, `org.change` |
| `org-review` | a weekly review | `org.agenda`, `org.query`, `org.clock`, `org.change` |
| `org-report` | a report of work over a period | `org.clock`, `org.query`, `org.entry` |
| `org-research` | researching a topic into your roam notes | `org.notes`, `org.entry`, `org.change`, `lattice.read`, `web.*` |

### Planning a day or a week

`:org-plan-day` reads today's agenda: what is overdue, scheduled and due, and
any unscheduled TODOs worth considering. Then it proposes a short plan: what to
do, in what order, and what to move. **Overdue items come first, and each one is
either done, rescheduled or dropped**, never quietly carried to tomorrow. The
plan is meant to be shorter than the agenda.

Nothing changes until you agree. Say "yes, do that", or adjust it ("keep the
report today, push the rest to monday"), and the model makes each change with
`org.change`, one diff per entry: a new schedule, a deadline, a state.

`:org-plan-week` does the same for the seven days from today.

### The weekly review

`:org-review` reports in four short sections, each entry with its ref:

1. **Done**: what you finished in the last week, and where the time went.
2. **Slipped**: what is overdue or was rescheduled.
3. **Stuck**: open TODOs with no schedule, no deadline and no recent activity,
   and WAITING items that need chasing.
4. **Next**: what to schedule for the coming week.

Then it asks what to change, and changes only what you agree to.

### A report of your work

`:org-report` totals your clocked time (`CLOCK:` lines) by entry, file or tag,
lists what you finished (`/DONE`) and what is in progress, and reads the notes of
entries worth describing. The report is grouped by project or tag, leads with
outcomes rather than activity, gives hours where the clock has them, and says
which period it covers. This workflow cannot change anything: it has no
`org.change`.

Name another period in the question: `:llm org-report last month`,
`:llm org-report 2026-10-01 to 2026-10-15, by tag`.

### Researching into your notes

`:llm org-research <topic>` starts from what you already know: your roam notes
on the topic, read in full. Then it looks outward with `web.search` and `web.fetch`, and reads
local files you point it at with `lattice.read`. Every claim it brings in
carries its URL. It adds what it found to a note you already have, or captures
a new one through one of your capture templates, and says what it is adding and
where before it proposes the diff.

Each page it fetches asks you first, showing the URL ("allow this site for this
chat" covers that site afterwards); searches run without asking.

### Making a workflow your own

A workflow sets the instructions and the tools, never the model. Pick a model
per workflow, and widen or narrow its tools, in your own config:

```toml
[llm.profiles.org-plan]
model = "local/qwen3"          # planning on a local model

[llm.profiles.org-report]
model = "anthropic/claude-haiku-5-5"
tools = ["org.clock", "org.query", "org.entry", "lattice.read"]

[llm.profiles.org-review]
system = "~/.config/lattice/llm/review.md"   # your own instructions instead
```

Your table overlays org's key by key, so a table that sets only `model` keeps
org's instructions and tools. `:llm-menu` shows `plugin profile: org-plan, from
org` beside each value that came from org. What a plugin's profile may and may
not set is in [`llm-guide`](help:llm-guide), under "Plugin profiles".

## Choosing a model

The tools do the reading, so the model does not need to be large. In org's eval
(a fixture corpus, each task graded without a model), Claude Haiku, Sonnet and
Opus each passed every task: listing today's agenda took one round, marking an
entry done two, a week of clocked time one or two, and summarising notes two
(Haiku took five, searching more widely before it read). A small model costs
breadth, not correctness.

Laptop-sized local models have not been measured. A local model that calls
tools reliably (most current ones do, through Ollama's OpenAI-compatible API)
is worth trying: start with `:llm org what is on today?` and an entry you know.
A model that answers without calling a tool, or invents entries, is the one to
replace. To run the eval against your own models, see `integration/eval/` in
the plugin's repository.

## What leaves your machine

A chat sends the model what you type and what the tools return: the agenda
rows, entries, notes and totals the model asked for, never your whole org
directory. On a hosted model, that text goes to the provider. On a local model
it stays on your machine; only `web.search` and `web.fetch` reach out, and you
can drop them from `org-research`'s tools.

`:llm-inspect` shows exactly what the next request sends. Org's tools read only
your agenda files and roam notes; `llm.exclude` keeps files from `lattice.read`.

## The tools

Org's tools read the files the same way `:org-agenda` and the roam index do, so
the assistant and the views agree.

| Tool | Answers | Takes |
|---|---|---|
| `org.agenda` | what is scheduled, due and overdue for a day, a week or a range, repeats included | `range` (`day`, the default, or `week`); `from` and `to` (`YYYY-MM-DD`, today when left out, at most a year apart); `match` |
| `org.query` | entries by tags, properties or TODO state, whatever their dates | `match` (required); `limit` (50 by default, at most 150) |
| `org.entry` | one entry whole: its notes, properties, log and subheadings | `ref` |
| `org.clock` | clocked time over a range, per entry, file or tag, largest first | `from` (six days before `to`); `to` (today); `by` (`entry`, `file` or `tag`) |
| `org.notes` | roam notes whose title or aliases contain every word, or that carry a tag, with the notes linking to each | `query`; `tag`; `limit` (20 by default, at most 150) |
| `org.change` | one change to one entry, as a diff you review | see "Changes and review" |

`match` is org's match syntax, as an agenda view's: `+work`, `+work-meeting`,
`/WAITING`, `+home/TODO`, `OWNER="Sam"`, `/DONE`. In `org.query` it covers tags,
properties and states only; dates are `org.agenda`'s.

An answer is text for the model, close to what a view shows:

```
Today is Thursday 2026-11-12.
Agenda for Thursday 2026-11-12:

Overdue:
- TODO [#A] Pay the invoice :money: (deadline 2026-11-10, 2 days ago) [ref ~/org/plan.org::1::Pay the invoice]

2026-11-12 Thu (today):
- 09:30 TODO Standup (scheduled, repeats +1d) [ref ~/org/plan.org::3::Standup]

1 TODO entry has no date; org.query with match "/TODO" lists them.
```

A long answer lists the first rows and counts the rest. A file that could not
be read is named in the answer, so the model never takes a missing file for an
empty one.

### Refs

Every row carries a **ref**, `file::line::heading`, and the model passes it back
to `org.entry` and `org.change`. If you edit the file in between and the line
moves, the ref still finds its heading when exactly one headline in the file
carries it, and the answer gives the fresh ref. If none or several do, the tool
says so and the model queries again: org refuses rather than guesses. An entry
with an `ID` property can be named by its ID instead, which survives any edit;
roam notes are always named by ID.

## Changes and review

`org.change` takes what to do, not text to write. Org writes the timestamps,
log lines and drawers itself, exactly as the keys do:

| Op | Does what this does | Needs |
|---|---|---|
| `set-state` | `<leader>ot`: the log line your keyword asks for, a repeating task repeating, the note an `@` state asks for | `ref`; `state` (`DONE`; empty clears it); `note` when the state wants one |
| `schedule`, `deadline` | `s` / `d` | `ref`; `date`: `2026-11-14`, `+3d`, `tomorrow`, `fri`, optionally with a repeater (`fri +1w`); `none` removes it |
| `tag` | adds tags; `-tag` removes one | `ref`; `tags` |
| `add-note` | `org-add-note`: a `- Note taken on` entry in the log | `ref`; `note` |
| `refile` | `<leader>or`: under another headline as its child, or to a file's top level | `ref`; `target` (a headline's ref, or a file's path) |
| `capture` | one of your `org.capture-templates`, its questions answered by the model | `template` (its key); `text`; `answers` |

**Every change opens as a diff** in the editor. `:diff-accept` applies it,
`:diff-reject` refuses it, and you may edit the right side before accepting. The
model is told which you chose. A file you have open changes in its buffer,
unsaved, as one change `u` undoes. If the file changed while the diff was open,
nothing is applied and the model is told to look again.

**Trusting a chat** (`<C-t>`) lets its edits apply without a diff, **but only
inside the project you opened lattice in.** Your org files usually live
elsewhere (`~/org`), and a change there is always reviewed, trusted or not.

**A refile across two files is two diffs, the copy first.** Declining the second
leaves the entry in both files, never in neither.

What org will not do from a chat:

- create a file: a capture whose target file does not exist yet is refused;
- write into an empty file;
- start your clock: a `:clock-in` template captures without clocking in;
- change more than one entry per call: a plan of five changes is five diffs.

An `llm.deny` rule on `edit_file` refuses org's changes too.

## In Claude Code and other agents

Org's tools work in agents outside the editor while a lattice with org loaded
is running. Turn serving on in lattice:

```
:set llm.serve_tools
```

or for good, in your `lattice.toml`:

```toml
[llm]
serve_tools = true
```

### Claude Code

Add lattice as a stdio MCP server, once:

```sh
claude mcp add lattice -- lattice --mcp                  # this project
claude mcp add --scope user lattice -- lattice --mcp     # every project
```

Claude Code then sees `org__agenda`, `org__query`, `org__entry`, `org__clock`,
`org__notes` and `org__change`: the names a chat uses, with `__` for `.`. Ask it
as you would ask a chat: "what's on my agenda today?", "schedule the migration
notes review for friday".

### Other MCP clients

Any client that starts stdio servers takes the same command. Most read an
`mcpServers` table:

```json
{
	"mcpServers": {
		"lattice": { "command": "lattice", "args": ["--mcp"] }
	}
}
```

### ACP agents lattice starts

An agent started from lattice (`:opencode-acp`) needs no setup. While
`llm.serve_tools` is on, its session is handed `lattice --mcp`, pointed at the
editor that started it. Turn serving on *before* starting the agent: a session
already open keeps the servers it opened with.

### Which editor answers

`lattice --mcp` opens no editor; it finds a running one and relays to it.
Started inside a project, as Claude Code starts it in its working directory, it
reaches the editor whose project holds that directory. Otherwise it reaches the
editor started most recently. An ACP agent always reaches the editor that
started it. With no editor serving, the server fails to start and says to turn
on `llm.serve_tools`.

### What differs from a chat

- **Changes are always reviewed.** Each `org__change` opens a diff in lattice,
  and the agent waits for your answer; its result says whether you accepted.
  Trusting a chat does not reach an outside agent. If the agent quits
  mid-review, the diff closes.
- **The workflows stay in lattice.** An agent gets the tools, not `org-plan`'s
  or `org-review`'s instructions. The tools' own descriptions tell it how to use
  refs and dates, so plain questions work. For a workflow, ask for it ("plan my
  day from my org agenda, overdue first, then propose the changes one at a
  time"), or put that in the agent's own instructions file.
- **Only plugin tools are served.** `lattice.*` and `web.*` are not: the agent
  has its own file, shell and web tools.
- **Tools that would ask are refused**, since nothing outside lattice can put
  the question to you. Org's tools never ask, so all six work; another plugin
  that reaches the network may have some refused.

Your `llm.allow` and `llm.deny` rules apply as they do in a chat. The editor
listens on loopback only, behind a token kept in a file only you can read
(under `<config-home>/lattice/cache/mcp/`). See "In other agents" in
[`llm-tools-mode`](help:llm-tools-mode).

## Troubleshooting

| You see | Why, and what to do |
|---|---|
| org is not under "Plugins" in `:llm-tools` | The plugin is not loaded (`:plugins` shows each plugin and its health), or its tools were not offered, and `*llm-tools*` says why |
| `:llm org` says the profile is unknown | The same: the workflows come with the tools |
| the agenda answer is empty, or names the wrong files | `org.agenda-files` is unset or names other files; `:set org.agenda-files?` shows it. A file that could not be read is named in the answer |
| `org.notes` finds nothing | Roam is not set up, or its index is empty: set `org.roam-directory` and run `:org-roam-sync` ([`org.roam`](help:org.roam)) |
| `org-research` has no `web.search` | No search backend is configured ([`llm-guide`](help:llm-guide), "`web.search` and `web.fetch`") |
| the tool says a ref no longer names an entry | The file changed under it and the heading is not unique. The model queries again; give the entry an `ID` property for a ref that never moves |
| a capture is refused | Its target file does not exist yet, or is empty. Create the file once, with a first headline |
| a change applied without a diff | It was inside your project, in a chat you trusted (`<C-t>`). Press `<C-t>` again to stop |
| Claude Code lists no lattice tools | `llm.serve_tools` is off, or no lattice with org is running; `claude mcp list` shows lattice failing. Turn it on, then reconnect from `/mcp` in Claude Code |
| Claude Code reaches the wrong editor | It reaches the editor of the project it was started in, else the newest. Start Claude Code inside the project of the editor you want |
| an ACP agent has no org tools | It was started before `llm.serve_tools` was on. Turn it on and start the agent again |
| a tool comes back refused in an outside agent | It would ask for your approval, which an outside agent cannot get. Allow it by name in `llm.allow`, or use it from a chat |

## See also

[`llm-guide`](help:llm-guide) for chats, profiles and providers;
[`llm-tools-mode`](help:llm-tools-mode) for `:llm-tools` and serving tools to
other agents; [`org`](help:org) for agenda files and capture templates;
[`org.roam`](help:org.roam) for notes.

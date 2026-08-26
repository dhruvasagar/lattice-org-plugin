; Org highlights.
;
; Richer than markdown's on purpose. Markdown's grammar has prose and a handful
; of markers; org's has *interactive structure* — checkbox states, TODO
; keywords, property drawers, planning lines, timestamps with repeaters — and a
; highlight file that only coloured headlines would be throwing most of the
; document away.
;
; Node and FIELD names are checked against `grammar-src/src/node-types.json`.
; Queries compile at REGISTRATION, so a name that does not exist is a load-time
; error naming this file rather than a silently dead pattern — which is the
; reason to be exact here rather than hopeful.
;
; Capture names are the shared vocabulary `lattice-syntax`'s `name_to_style`
; maps to a `Style`; the theme resolves the colour. Nothing here bakes one.

; ── Headlines ────────────────────────────────────────────────────────────────
;
; Org's headline marker is ONE node whose *text length* is the level:
; `(headline (stars) (item))`, where `stars` is `*`, `**`, `***`… This is
; unlike markdown, whose grammar gives each level its own marker node
; (`atx_h1_marker` … `atx_h6_marker`), so a per-level capture there is just six
; ordinary patterns. Here the level has to come from the text, which means
; `#eq?` predicates — and this file is what proves the pipeline evaluates them.
;
; The `stars` capture is `@punctuation.special` → `Style::Markup`, the same
; style markdown's `#` markers take. That is what keeps the stars at base size
; while the title scales: `heading_scale_split` looks for the first run whose
; resolved `scale > 1.0`, so `[stars at 1.0][title at N]` renders as two pieces
; on one baseline, exactly as `## Title` does.

(headline (stars) @punctuation.special (item) @text.title.1
  (#eq? @punctuation.special "*"))
(headline (stars) @punctuation.special (item) @text.title.2
  (#eq? @punctuation.special "**"))
(headline (stars) @punctuation.special (item) @text.title.3
  (#eq? @punctuation.special "***"))
(headline (stars) @punctuation.special (item) @text.title.4
  (#eq? @punctuation.special "****"))
(headline (stars) @punctuation.special (item) @text.title.5
  (#eq? @punctuation.special "*****"))
(headline (stars) @punctuation.special (item) @text.title.6
  (#eq? @punctuation.special "******"))

; Everything deeper than six shares level 6 rather than losing its heading
; identity — org has no depth limit, the theme's scale ramp does.
(headline (stars) @punctuation.special (item) @text.title.6
  (#match? @punctuation.special "^\\*{7,}$"))

; ── TODO keywords ────────────────────────────────────────────────────────────
;
; The FIRST word of a headline, and only there — `.` anchors the expr to the
; start of the item, so a "TODO" in the middle of a title is prose and stays
; prose.
;
; The keyword set is `org.todo-keywords`, which is user-configurable, and a
; static query cannot read an option. These are the defaults (`TODO | DONE`)
; plus the conventional extras a user is likely to add, so the common
; configurations light up without one. A keyword not listed here still cycles
; and still drives the agenda — it simply renders as ordinary title text, which
; is the right way for this to degrade.
;
; Not-done reads as a keyword (the theme's attention colour); done reads as a
; comment, because a finished item should recede rather than compete with the
; work that is left.
(headline (item . (expr) @keyword)
  (#any-of? @keyword "TODO" "NEXT" "STARTED" "WAITING" "HOLD" "PROJ"))
(headline (item . (expr) @comment)
  (#any-of? @comment "DONE" "CANCELLED" "CANCELED" "KILL"))

; Tags: `:work:urgent:` at the end of a headline.
(headline (tag_list) @attribute)
(tag) @attribute

; ── Checkboxes ───────────────────────────────────────────────────────────────
;
; The most interactive thing in an org file, and three distinct states rather
; than one. Collapsing them would defeat the point: the whole value of a
; checkbox list at a glance is which items are done, which are part-done, and
; which have not been started.
(listitem (checkbox) @string
  (#match? @string "^\\[[xX]\\]$"))
(listitem (checkbox) @attribute
  (#eq? @attribute "[-]"))
(listitem (checkbox) @punctuation.special
  (#match? @punctuation.special "^\\[\\s*\\]$"))

; List bullets — the marker, never the text after it.
(listitem bullet: (bullet) @punctuation.special)

; ── Planning and timestamps ──────────────────────────────────────────────────
;
; `SCHEDULED:` / `DEADLINE:` / `CLOSED:` lead a planning line. The keyword and
; its timestamp are captured separately so the label can recede and the date
; stay legible, which is the way round you actually read them.
(plan (entry name: (entry_name) @keyword))

; A timestamp's parts. `date` / `time` are the payload; `repeat` (`+1w`) and
; `delay` (`-2d`) are modifiers that change what the entry MEANS, so they are
; not left to blend into the date.
(timestamp date: (date) @constant)
(timestamp time: (time) @constant)
(timestamp day: (day) @constant)
(timestamp repeat: (repeat) @operator)
(timestamp delay: (delay) @operator)
(timestamp duration: (duration) @operator)

; The brackets themselves, so active `<…>` and inactive `[…]` are told apart —
; only the active kind reaches the agenda, and that is worth seeing.
(timestamp "<" @punctuation.special)
(timestamp ">" @punctuation.special)
(timestamp "[" @comment)
(timestamp "]" @comment)

; ── Drawers and properties ───────────────────────────────────────────────────
;
; `:PROPERTIES:` … `:END:` and hand-rolled `:LOGBOOK:` drawers. The NAME is
; structure and reads as a label; the contents recede. Previously the whole
; drawer was one `@comment`, which hid the one part worth finding.
(drawer name: (expr) @label)
(drawer contents: (contents) @comment)

; `:CUSTOM_ID: foo` — the key is an attribute, the value ordinary text.
(property name: (expr) @attribute)
(property value: (value) @string)

; ── Blocks, directives, and the rest ─────────────────────────────────────────
;
; `#+BEGIN_SRC rust` … `#+END_SRC`. The name and parameter are the interesting
; part (which language, which switches); the body is literal text.
(block name: (expr) @keyword)
(block end_name: (expr) @keyword)
(block parameter: (expr) @attribute)
(block contents: (contents) @text.literal)

; `#+TITLE: …`, `#+FILETAGS: …`. Directive names are keywords; values read as
; the strings they are.
(directive name: (expr) @keyword)
(directive value: (value) @string)

; Footnotes: `[fn:1] the note`.
(fndef label: (expr) @label)

; Tables. The separator row and any `#+TBLFM:` formula are structure; cells are
; ordinary text and deliberately left alone, or a table of prose would become a
; wall of colour.
(table (hr) @punctuation.special)
(table (formula) @function)

; `-----` horizontal rules.
(hr) @punctuation.special

; `\begin{equation}` … `\end{equation}`.
(latex_env) @text.literal

; `# a comment line`.
(comment) @comment

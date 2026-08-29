; Org source blocks carry another language, and it should look like that
; language. The mechanism is the one markdown's fenced blocks already use —
; `@injection.language` + `@injection.content`, resolved host-side against the
; SAME registry a top-level buffer uses — so a `#+begin_src rust` block is
; highlighted by the bundled Rust grammar with no special case anywhere.
;
; Two things differ from markdown's version, and both are why this is a query
; rather than a copy.
;
; The language is the block's FIRST PARAMETER, not a dedicated node. Org's
; grammar is `#+begin_ name (parameter)* contents`, so `#+begin_src python
; :results output` has three parameters and only the first is a language. The `.`
; anchor after `name:` restricts the capture to the one immediately following it.
;
; Honesty about that anchor: no test here fails without it, and that is a
; property of the host rather than of the query. Unanchored, the pattern matches
; once per parameter and the host tries to resolve each; `:results` resolves to
; nothing and is dropped, so the visible result is the same and only the wasted
; lookups differ. It would become visible the day a LATER header argument
; happened to spell a real language name — the injection would then be decided by
; query-match order rather than by org's rule that the language comes first.
; Constraining by construction is cheaper than discovering that.
;
; And `name:` HAS to be checked, because `block` is every `#+begin_X` — `quote`,
; `example`, `verse`, `comment` — and only `src` carries code. This one is not
; theoretical: `#+begin_example rust` is verbatim text that names a language, and
; without the guard its body is highlighted as Rust. (A `#+begin_quote` would not
; have caught it: quote blocks carry no parameter, so the pattern fails to match
; them either way — the first version of the test made exactly that mistake and
; passed against a guard-less query.) `#+begin_` is case-insensitive in the
; grammar, so the match is too.
(block
  name: (expr) @_kind
  .
  parameter: (expr) @injection.language
  contents: (contents) @injection.content
  (#match? @_kind "^(?i:src)$"))

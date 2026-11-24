//! noweb — `<<name>>` in a body (OB.13, design §6.2).
//!
//! Pure text, guest-side, before a block runs or is tangled. `<<name>>` is
//! the body of the block named `name`, or else the bodies of the blocks
//! whose `:noweb-ref` is `name`, joined by newlines. `<<name()>>` is the
//! block's `#+RESULTS:`, read as `:var` reads it. The text before `<<` on the
//! line prefixes every line expanded into it, and the text after `>>`
//! follows the last, so a reference indented inside a function stays
//! indented.
//!
//! When `:noweb` applies is babel's: `yes` (and `no-export`, `strip-export`,
//! which differ only at export, which lattice has none of) always, `tangle`
//! only when tangling, `eval` only when running, and `no`, the default,
//! never. A reference that names nothing is left as written, as babel leaves
//! it: `<<EOF` and `a << b` are ordinary code in many languages.

use super::block::{effective_header_args, src_blocks, SrcBlock};
use super::vars::{value_of_result, Value};

/// The most text one expansion may produce. References form a graph, not
/// a tree: a block naming the next twice, thirty deep, is 2^30 lines, and
/// this runs on the keystroke path, before the confirm. Counted at every
/// level, so a nested line counts once per level it is copied through: the
/// budget bounds the work, not only the result.
const MAX_EXPANSION: usize = 1 << 20;

/// Spend `n` bytes of the budget, or refuse.
fn spend(budget: &mut usize, n: usize) -> Result<(), String> {
    *budget = budget.checked_sub(n).ok_or_else(|| {
        "org: noweb references expand past 1 MiB — a reference used more than once is \
         expanded each time"
            .to_string()
    })?;
    Ok(())
}

/// What the body is being expanded for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    Run,
    Tangle,
}

/// Whether `:noweb` expands `block` for `ctx`.
fn applies(lines: &[&str], block: &SrcBlock, ctx: Context) -> Result<bool, String> {
    let header = effective_header_args(lines, block);
    let Some(words) = header.get("noweb") else {
        return Ok(false);
    };
    let word = words.join(" ").trim().to_ascii_lowercase();
    match word.as_str() {
        "yes" | "no-export" | "strip-export" => Ok(true),
        "no" => Ok(false),
        "tangle" => Ok(ctx == Context::Tangle),
        "eval" => Ok(ctx == Context::Run),
        other => Err(format!(
            "org: `:noweb {other}` — yes, no, tangle, eval, no-export or strip-export"
        )),
    }
}

/// `block`'s body as it runs or tangles: expanded when its `:noweb` says so
/// for `ctx`, as written otherwise.
pub(crate) fn expand(lines: &[&str], block: &SrcBlock, ctx: Context) -> Result<String, String> {
    let all = src_blocks(lines);
    let mut path = vec![block
        .name
        .clone()
        .unwrap_or_else(|| "this block".to_string())];
    let mut budget = MAX_EXPANSION;
    expand_body(lines, &all, block, ctx, &mut path, &mut budget)
}

fn expand_body(
    lines: &[&str],
    all: &[SrcBlock],
    block: &SrcBlock,
    ctx: Context,
    path: &mut Vec<String>,
    budget: &mut usize,
) -> Result<String, String> {
    if !applies(lines, block, ctx)? {
        return Ok(block.body.clone());
    }
    let mut out: Vec<String> = Vec::new();
    for line in block.body.lines() {
        let Some((prefix, reference, suffix)) = reference_in(line) else {
            spend(budget, line.len() + 1)?;
            out.push(line.to_string());
            continue;
        };
        let Some(text) = resolve(lines, all, reference, ctx, path, budget)? else {
            spend(budget, line.len() + 1)?;
            out.push(line.to_string());
            continue;
        };
        let expanded: Vec<&str> = text.lines().collect();
        match expanded.as_slice() {
            [] => out.push(format!("{prefix}{suffix}")),
            [rest @ .., last] => {
                for l in rest {
                    spend(budget, prefix.len() + l.len() + 1)?;
                    out.push(format!("{prefix}{l}"));
                }
                spend(budget, prefix.len() + last.len() + suffix.len() + 1)?;
                out.push(format!("{prefix}{last}{suffix}"));
            }
        }
    }
    let mut body = out.join("\n");
    if block.body.ends_with('\n') {
        body.push('\n');
    }
    Ok(body)
}

/// The first `<<name>>` on `line`: what is before it, the name, and what is
/// after it. A name holds no whitespace, so `a << b >> c` is not one, and a
/// `<<` that starts none (`cat <<EOF; echo <<real>>`) is passed over for
/// the next.
fn reference_in(line: &str) -> Option<(&str, &str, &str)> {
    line.match_indices("<<").find_map(|(start, _)| {
        let rest = &line[start + 2..];
        let end = rest.find(">>")?;
        let name = &rest[..end];
        (!name.is_empty() && !name.contains(char::is_whitespace) && !name.contains('<'))
            .then(|| (&line[..start], name, &rest[end + 2..]))
    })
}

/// The text `<<reference>>` stands for, or `None` when nothing is named so.
fn resolve(
    lines: &[&str],
    all: &[SrcBlock],
    reference: &str,
    ctx: Context,
    path: &mut Vec<String>,
    budget: &mut usize,
) -> Result<Option<String>, String> {
    if let Some(name) = reference.strip_suffix("()") {
        let Some(block) = all.iter().find(|b| b.name.as_deref() == Some(name)) else {
            return Ok(None);
        };
        let Some(r) = &block.results else {
            return Err(format!(
                "org: `<<{reference}>>` needs `{name}`'s result, and it has none yet — run it \
                 first"
            ));
        };
        let body = lines
            .get(r.keyword_line as usize + 1..=r.end_line as usize)
            .unwrap_or_default();
        return Ok(Some(text_of(&value_of_result(body))));
    }
    if reference.contains('(') {
        return Err(format!(
            "org: `<<{reference}>>` calls a block with arguments, which is not supported"
        ));
    }
    let named: Vec<&SrcBlock> = match all.iter().find(|b| b.name.as_deref() == Some(reference)) {
        Some(b) => vec![b],
        // Babel's other source: every block that says `:noweb-ref name`,
        // in document order.
        None => all
            .iter()
            .filter(|b| {
                effective_header_args(lines, b)
                    .get("noweb-ref")
                    .is_some_and(|w| w.join(" ").trim().trim_matches('"') == reference)
            })
            .collect(),
    };
    if named.is_empty() {
        return Ok(None);
    }
    if let Some(at) = path.iter().position(|p| p == reference) {
        let mut cycle = path[at..].to_vec();
        cycle.push(reference.to_string());
        return Err(format!(
            "org: `<<{reference}>>` expands into itself ({})",
            cycle.join(" → ")
        ));
    }
    path.push(reference.to_string());
    let mut parts = Vec::with_capacity(named.len());
    for b in named {
        let body = expand_body(lines, all, b, ctx, path, budget)?;
        parts.push(body.strip_suffix('\n').unwrap_or(&body).to_string());
    }
    path.pop();
    Ok(Some(parts.join("\n")))
}

/// A result as text in a body: a table's rows on lines, cells tab-separated.
fn text_of(v: &Value) -> String {
    match v {
        Value::Number(s) | Value::Str(s) => s.clone(),
        Value::Table(rows) => rows
            .iter()
            .map(|r| r.iter().map(text_of).collect::<Vec<_>>().join("\t"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_named(doc: &str, name: &str, ctx: Context) -> Result<String, String> {
        let lines: Vec<&str> = doc.lines().collect();
        let block = src_blocks(&lines)
            .into_iter()
            .find(|b| b.name.as_deref() == Some(name))
            .unwrap();
        expand(&lines, &block, ctx)
    }

    #[test]
    fn a_reference_is_the_named_blocks_body() {
        let doc = "#+NAME: greet\n#+begin_src sh\necho hi\n#+end_src\n\
                   #+NAME: main\n#+begin_src sh :noweb yes\n<<greet>>\necho bye\n#+end_src\n";
        assert_eq!(
            expand_named(doc, "main", Context::Run).unwrap(),
            "echo hi\necho bye"
        );
    }

    #[test]
    fn references_expand_recursively_when_their_own_noweb_says_so() {
        let doc = "#+NAME: inner\n#+begin_src sh\necho in\n#+end_src\n\
                   #+NAME: mid\n#+begin_src sh :noweb yes\n<<inner>>\necho mid\n#+end_src\n\
                   #+NAME: lit\n#+begin_src sh\n<<inner>>\n#+end_src\n\
                   #+NAME: main\n#+begin_src sh :noweb yes\n<<mid>>\n<<lit>>\n#+end_src\n";
        assert_eq!(
            expand_named(doc, "main", Context::Run).unwrap(),
            "echo in\necho mid\n<<inner>>",
            "`lit` has no `:noweb`, so its own reference stays as written"
        );
    }

    #[test]
    fn a_cycle_is_refused_with_its_path() {
        let doc = "#+NAME: a\n#+begin_src sh :noweb yes\n<<b>>\n#+end_src\n\
                   #+NAME: b\n#+begin_src sh :noweb yes\n<<a>>\n#+end_src\n";
        let e = expand_named(doc, "a", Context::Run).unwrap_err();
        assert!(e.contains("a → b → a"), "{e}");
    }

    #[test]
    fn the_reference_lines_prefix_goes_on_every_line() {
        let doc = "#+NAME: body\n#+begin_src python\nx = 1\nreturn x\n#+end_src\n\
                   #+NAME: main\n#+begin_src python :noweb yes\ndef f():\n    <<body>>  # end\n#+end_src\n";
        assert_eq!(
            expand_named(doc, "main", Context::Run).unwrap(),
            "def f():\n    x = 1\n    return x  # end"
        );
    }

    #[test]
    fn tangle_expands_only_when_tangling_and_eval_only_when_running() {
        let doc = "#+NAME: x\n#+begin_src sh\necho x\n#+end_src\n\
                   #+NAME: t\n#+begin_src sh :noweb tangle\n<<x>>\n#+end_src\n\
                   #+NAME: e\n#+begin_src sh :noweb eval\n<<x>>\n#+end_src\n";
        assert_eq!(expand_named(doc, "t", Context::Run).unwrap(), "<<x>>");
        assert_eq!(expand_named(doc, "t", Context::Tangle).unwrap(), "echo x");
        assert_eq!(expand_named(doc, "e", Context::Run).unwrap(), "echo x");
        assert_eq!(expand_named(doc, "e", Context::Tangle).unwrap(), "<<x>>");
    }

    #[test]
    fn noweb_ref_blocks_join_and_unknown_names_stay() {
        let doc = "#+begin_src sh :noweb-ref setup\na\n#+end_src\n\
                   #+begin_src sh :noweb-ref setup\nb\n#+end_src\n\
                   #+NAME: main\n#+begin_src sh :noweb yes\n<<setup>>\ncat <<EOF\n<<nope>>\n#+end_src\n";
        assert_eq!(
            expand_named(doc, "main", Context::Run).unwrap(),
            "a\nb\ncat <<EOF\n<<nope>>"
        );
    }

    #[test]
    fn a_heredoc_before_a_reference_does_not_hide_it() {
        assert_eq!(
            reference_in("cat <<EOF; echo <<real>> done"),
            Some(("cat <<EOF; echo ", "real", " done"))
        );
        assert_eq!(reference_in("a << b >> c"), None);
        assert_eq!(reference_in("x <<<<y>>"), Some(("x <<", "y", "")));
    }

    /// Review: thirty blocks each naming the next twice is 2^30 lines. It
    /// must be refused, not computed.
    #[test]
    fn an_exponential_expansion_is_refused() {
        let mut doc = String::new();
        for i in 0..30 {
            doc.push_str(&format!(
                "#+NAME: b{i}\n#+begin_src sh :noweb yes\n<<b{n}>>\n<<b{n}>>\n#+end_src\n",
                n = i + 1
            ));
        }
        doc.push_str("#+NAME: b30\n#+begin_src sh\necho leaf\n#+end_src\n");
        let e = expand_named(&doc, "b0", Context::Run).unwrap_err();
        assert!(e.contains("1 MiB"), "{e}");
        assert_eq!(
            expand_named(&doc, "b28", Context::Run).unwrap(),
            "echo leaf\necho leaf\necho leaf\necho leaf",
            "a small one still expands"
        );
    }

    #[test]
    fn a_call_is_the_blocks_result() {
        let doc = "#+NAME: n\n#+begin_src sh\nx\n#+end_src\n\n#+RESULTS: n\n: 42\n\n\
                   #+NAME: main\n#+begin_src sh :noweb yes\necho <<n()>>\n#+end_src\n\
                   #+NAME: bad\n#+begin_src sh :noweb yes\n<<n(x=1)>>\n#+end_src\n";
        assert_eq!(expand_named(doc, "main", Context::Run).unwrap(), "echo 42");
        assert!(expand_named(doc, "bad", Context::Run)
            .unwrap_err()
            .contains("arguments"));
    }
}

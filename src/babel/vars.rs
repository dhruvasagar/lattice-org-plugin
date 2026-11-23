//! `:var` — values handed to a block (OB.11, design §6.1).
//!
//! `:var n=5`, `:var s="text"`, `:var x=disk` (the named block's
//! `#+RESULTS:`) and `:var t=tbl` (a named org table) are resolved here,
//! guest-side, from the buffer's text, and reach the program as a PRELUDE:
//! assignments generated ahead of the body, in the language the record's
//! `value` names. The prelude goes in before the value wrapper runs, so
//! `:results value` sees the variables too.
//!
//! Values are written as READABLE literals, because a tangled file keeps
//! them: `n = 5`, `rows = [["a", 1]]`. JSON's literal syntax is a valid
//! python and JS expression for numbers, strings and lists, and ruby's
//! with `#` escaped too (`#{…}` interpolates in a ruby string). sh gets a
//! single-quoted string. Every string is escaped by the one encoder here,
//! so no quote, backslash, `$`, `#` or newline in a value can end the
//! literal it is in.

use serde::{Deserialize, Serialize};

use super::block::{keyword_value, src_blocks, unescape, SrcBlock};
use super::header_args::HeaderArgs;
use super::wrappers::Wrapper;

/// A resolved `:var` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Value {
    /// Written as a JSON number: babel reads `5` and `: 5` as numbers.
    Number(String),
    Str(String),
    /// Rows of cells, horizontal rules dropped.
    Table(Vec<Vec<Value>>),
}

/// A variable's value, or the block it waits on (OB.12): one that runs
/// earlier in the same chain, whose fresh result is the value. The events
/// seam fills it when that block finishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Binding {
    Ready(Value),
    After(String),
}

/// Whether `s` is a number as JSON writes one, so it can cross as-is.
fn json_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    if b.first() == Some(&b'-') {
        i += 1;
    }
    if b.get(i) == Some(&b'0') {
        i += 1;
    } else if !digits(&mut i) {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

/// A cell or scalar as babel reads it: a number when it is one.
fn scalar(s: &str) -> Value {
    if json_number(s) {
        Value::Number(s.to_string())
    } else {
        Value::Str(s.to_string())
    }
}

/// The `name=value` assignments in a block's `:var` entries, outermost
/// first. Babel allows several per entry, separated by commas or spaces, and
/// quotes keep a value whole.
fn assignments(header: &HeaderArgs) -> Vec<String> {
    let mut tokens = Vec::new();
    for entry in header.all("var") {
        let joined = entry.join(" ");
        let mut cur = String::new();
        let mut quoted = false;
        let mut chars = joined.chars();
        while let Some(c) = chars.next() {
            match c {
                // `\"` inside quotes is a quote, as babel's reader takes it.
                '\\' if quoted => {
                    cur.push(c);
                    cur.extend(chars.next());
                }
                '"' => {
                    quoted = !quoted;
                    cur.push(c);
                }
                ',' | ' ' | '\t' if !quoted => {
                    if !cur.is_empty() {
                        tokens.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }
    }
    // `x = 1` and `x= 1` are `x=1`.
    let mut out: Vec<String> = Vec::new();
    for t in tokens {
        match out.last_mut() {
            Some(last) if last.ends_with('=') || t.starts_with('=') => last.push_str(&t),
            _ => out.push(t),
        }
    }
    out
}

fn identifier(name: &str) -> bool {
    let mut c = name.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Table rows from `|` lines: cells trimmed, rules (`|---+---|`) dropped.
fn table_rows<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<Vec<Value>> {
    lines
        .map(str::trim)
        .take_while(|l| l.starts_with('|'))
        .filter(|l| !l.starts_with("|-"))
        .map(|l| {
            let inner = l.trim_start_matches('|');
            let inner = inner.strip_suffix('|').unwrap_or(inner);
            inner.split('|').map(|c| scalar(c.trim())).collect()
        })
        .collect()
}

/// A named block's `#+RESULTS:` as a value ([`value_of_result`]).
fn result_value(lines: &[&str], block: &SrcBlock) -> Option<Value> {
    let r = block.results.as_ref()?;
    Some(value_of_result(
        lines
            .get(r.keyword_line as usize + 1..=r.end_line as usize)
            .unwrap_or_default(),
    ))
}

/// The lines under a `#+RESULTS:` as a value: a table when they are one,
/// else their text, a number when that is one line that reads as one. Read
/// from the file for a block that is not run, and from a fresh result's
/// lines for one that ran earlier in the chain, so both read alike.
pub(crate) fn value_of_result(body: &[&str]) -> Value {
    if body
        .first()
        .is_some_and(|l| l.trim_start().starts_with('|'))
    {
        return Value::Table(table_rows(body.iter().copied()));
    }
    let wrapped = body
        .first()
        .and_then(|l| l.trim().strip_prefix("#+begin_"))
        .is_some();
    let text: Vec<String> = if wrapped {
        // `#+begin_example` … `#+end_example`, and the other wrappers.
        body.get(1..body.len().saturating_sub(1))
            .unwrap_or_default()
            .iter()
            .map(|l| unescape(l))
            .collect()
    } else {
        body.iter()
            .map(|l| {
                let t = l.trim_start();
                t.strip_prefix(": ")
                    .or_else(|| (t == ":").then_some(""))
                    .unwrap_or(l)
                    .to_string()
            })
            .collect()
    };
    match text.as_slice() {
        [one] => scalar(one),
        _ => Value::Str(text.join("\n")),
    }
}

/// The table under the `#+NAME: name` line, when one is there.
fn named_table(lines: &[&str], name: &str) -> Option<Value> {
    let at = lines
        .iter()
        .position(|l| keyword_value(l, "name") == Some(name))?;
    // Other affiliated keywords (`#+CAPTION:`, `#+ATTR_…`) may sit between.
    let first = (at + 1..lines.len()).find(|&i| !lines[i].trim_start().starts_with("#+"))?;
    lines[first]
        .trim_start()
        .starts_with('|')
        .then(|| Value::Table(table_rows(lines[first..].iter().copied())))
}

/// What `value` (the right of one `name=value`) means.
fn resolve_one(
    lines: &[&str],
    blocks: &[SrcBlock],
    scheduled: &[String],
    value: &str,
) -> Result<Binding, String> {
    if is_reference(value) && scheduled.iter().any(|s| s == value) {
        return Ok(Binding::After(value.to_string()));
    }
    resolve_literal(lines, blocks, value).map(Binding::Ready)
}

/// Whether `value` (the right of one `name=value`) is a reference, not a
/// literal: what a block's name could be.
fn is_reference(value: &str) -> bool {
    !value.starts_with('"') && !json_number(value) && !value.contains(['[', '(', ':'])
}

/// The names a block's `:var`s refer to: what may be a block to run first.
pub(crate) fn references(header: &HeaderArgs) -> Vec<String> {
    assignments(header)
        .into_iter()
        .filter_map(|a| Some(a.split_once('=')?.1.to_string()))
        .filter(|v| is_reference(v))
        .collect()
}

fn resolve_literal(lines: &[&str], blocks: &[SrcBlock], value: &str) -> Result<Value, String> {
    if let Some(s) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.extend(chars.next()),
                c => out.push(c),
            }
        }
        return Ok(Value::Str(out));
    }
    if json_number(value) {
        return Ok(Value::Number(value.to_string()));
    }
    if value.contains(['[', '(', ':']) {
        return Err(format!(
            "org: `:var` takes a literal or a name; `{value}` (indexing, a call or another \
             file) is not supported"
        ));
    }
    if let Some(block) = blocks.iter().find(|b| b.name.as_deref() == Some(value)) {
        return result_value(lines, block).ok_or_else(|| {
            format!("org: the block `{value}` has no result yet — run it first (`C-c C-c` in it)")
        });
    }
    named_table(lines, value)
        .ok_or_else(|| format!("org: `:var` names `{value}`, and nothing here is named that"))
}

/// The variables a block's `header` declares, resolved against `lines`, in
/// order. A name bound at two levels takes the innermost binding (the
/// block's over its headline's over the file's), in the outer one's place.
/// A reference to a block in `scheduled` waits for that block's run.
pub(crate) fn resolve(
    lines: &[&str],
    header: &HeaderArgs,
    scheduled: &[String],
) -> Result<Vec<(String, Binding)>, String> {
    let blocks = src_blocks(lines);
    let mut out: Vec<(String, Binding)> = Vec::new();
    for a in assignments(header) {
        let Some((name, value)) = a.split_once('=') else {
            return Err(format!("org: `:var {a}` needs a value — `:var {a}=…`"));
        };
        if !identifier(name) {
            return Err(format!("org: `:var {name}` is not a variable name"));
        }
        let value = resolve_one(lines, &blocks, scheduled, value)?;
        match out.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = value,
            None => out.push((name.to_string(), value)),
        }
    }
    Ok(out)
}

/// [`resolve`] outside a chain, as a tangle reads its blocks: with nothing
/// scheduled before, every reference is read from the file.
pub(crate) fn resolve_ready(
    lines: &[&str],
    header: &HeaderArgs,
) -> Result<Vec<(String, Value)>, String> {
    Ok(resolve(lines, header, &[])?
        .into_iter()
        .filter_map(|(n, b)| match b {
            Binding::Ready(v) => Some((n, v)),
            // Only a scheduled name becomes `After`, and none is.
            Binding::After(_) => None,
        })
        .collect())
}

/// `v` as a literal: JSON's syntax, which python and JS read as written.
/// `ruby` also escapes `#`, which would start interpolation there.
fn literal(v: &Value, ruby: bool, out: &mut String) {
    match v {
        Value::Number(n) => out.push_str(n),
        Value::Str(s) => {
            out.push('"');
            for c in s.chars() {
                match c {
                    '#' if ruby => out.push_str("\\#"),
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        Value::Table(rows) => {
            out.push('[');
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('[');
                for (j, cell) in row.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    literal(cell, ruby, out);
                }
                out.push(']');
            }
            out.push(']');
        }
    }
}

fn literal_of(v: &Value, ruby: bool) -> String {
    let mut s = String::new();
    literal(v, ruby, &mut s);
    s
}

/// A value as sh text: a table's rows on lines, its cells split by tabs.
fn sh_text(v: &Value) -> String {
    match v {
        Value::Number(s) | Value::Str(s) => s.clone(),
        Value::Table(rows) => rows
            .iter()
            .map(|r| r.iter().map(sh_text).collect::<Vec<_>>().join("\t"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// The lines that bind `vars` ahead of a body, in `wrapper`'s language.
pub(crate) fn prelude(wrapper: Wrapper, vars: &[(String, Value)]) -> String {
    if vars.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (name, v) in vars {
        let line = match wrapper {
            // `'…'` holds anything but `'`, which closes, escapes and reopens.
            Wrapper::Sh => format!("{name}='{}'", sh_text(v).replace('\'', r"'\''")),
            Wrapper::Python => format!("{name} = {}", literal_of(v, false)),
            Wrapper::Ruby => format!("{name} = {}", literal_of(v, true)),
            Wrapper::Node => format!("var {name} = {};", literal_of(v, false)),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::block::effective_header_args;
    use super::*;

    fn vars_of(doc: &str) -> Result<Vec<(String, Value)>, String> {
        let lines: Vec<&str> = doc.lines().collect();
        let block = src_blocks(&lines)
            .into_iter()
            .find(|b| b.name.as_deref() == Some("main"))
            .expect("a block named main");
        resolve(&lines, &effective_header_args(&lines, &block), &[]).map(|v| {
            v.into_iter()
                .map(|(n, b)| match b {
                    Binding::Ready(v) => (n, v),
                    Binding::After(a) => panic!("{n} waits on {a}"),
                })
                .collect()
        })
    }

    #[test]
    fn a_block_run_earlier_in_the_chain_is_waited_on() {
        let doc = "#+NAME: a\n#+begin_src sh\nx\n#+end_src\n\n#+RESULTS: a\n: old\n\n\
                   #+NAME: main\n#+begin_src sh :var x=a y=2\nx\n#+end_src\n";
        let lines: Vec<&str> = doc.lines().collect();
        let main = src_blocks(&lines).pop().unwrap();
        let header = effective_header_args(&lines, &main);
        assert_eq!(references(&header), vec!["a".to_string()]);
        assert_eq!(
            resolve(&lines, &header, &["a".to_string()]).unwrap()[0].1,
            Binding::After("a".to_string()),
            "a fresh run, not the old result"
        );
    }

    fn n(s: &str) -> Value {
        Value::Number(s.to_string())
    }
    fn s(s: &str) -> Value {
        Value::Str(s.to_string())
    }

    #[test]
    fn literals_numbers_and_quoted_strings() {
        let v = vars_of(
            "#+NAME: main\n#+begin_src sh :var n=5 s=\"a \\\"b\\\", c\" :var f=-1.5e3\nx\n#+end_src\n",
        )
        .unwrap();
        assert_eq!(
            v,
            vec![
                ("n".into(), n("5")),
                ("s".into(), s("a \"b\", c")),
                ("f".into(), n("-1.5e3"))
            ]
        );
    }

    #[test]
    fn the_innermost_binding_wins() {
        let doc = "#+PROPERTY: header-args :var x=1 y=2\n\
                   #+NAME: main\n#+begin_src sh :var x=3\nx\n#+end_src\n";
        assert_eq!(
            vars_of(doc).unwrap(),
            vec![("x".into(), n("3")), ("y".into(), n("2"))]
        );
    }

    #[test]
    fn a_named_blocks_result_verbatim_scalar_and_table() {
        let doc = "#+NAME: one\n#+begin_src sh\nx\n#+end_src\n\n#+RESULTS: one\n: 42\n\n\
                   #+NAME: lines\n#+begin_src sh\nx\n#+end_src\n\n#+RESULTS: lines\n: a\n: b\n\n\
                   #+NAME: tab\n#+begin_src sh\nx\n#+end_src\n\n#+RESULTS: tab\n| 1 | x |\n|---|\n| 2 | y |\n\n\
                   #+NAME: main\n#+begin_src sh :var a=one, b=lines, c=tab\nx\n#+end_src\n";
        let v = vars_of(doc).unwrap();
        assert_eq!(v[0].1, n("42"));
        assert_eq!(v[1].1, s("a\nb"));
        assert_eq!(
            v[2].1,
            Value::Table(vec![vec![n("1"), s("x")], vec![n("2"), s("y")]])
        );
    }

    #[test]
    fn a_named_table() {
        let doc = "#+NAME: tbl\n| a | 1 |\n| b | 2 |\n\n\
                   #+NAME: main\n#+begin_src sh :var t=tbl\nx\n#+end_src\n";
        assert_eq!(
            vars_of(doc).unwrap()[0].1,
            Value::Table(vec![vec![s("a"), n("1")], vec![s("b"), n("2")]])
        );
    }

    #[test]
    fn what_cannot_be_resolved_is_named() {
        let e = |doc: &str| vars_of(doc).unwrap_err();
        assert!(e("#+NAME: main\n#+begin_src sh :var x=nope\nx\n#+end_src\n").contains("`nope`"));
        assert!(e("#+NAME: r\n#+begin_src sh\nx\n#+end_src\n\
                   #+NAME: main\n#+begin_src sh :var x=r\nx\n#+end_src\n")
        .contains("no result yet"));
        assert!(e("#+NAME: main\n#+begin_src sh :var x=t[1]\nx\n#+end_src\n").contains("indexing"));
        assert!(
            e("#+NAME: main\n#+begin_src sh :var 1x=2\nx\n#+end_src\n").contains("variable name")
        );
    }

    #[test]
    fn preludes_are_readable_and_cannot_be_escaped() {
        let v = vec![("q".to_string(), s("it's \"$HOME\" #{x}\n\\"))];
        assert_eq!(
            prelude(Wrapper::Sh, &v),
            "q='it'\\''s \"$HOME\" #{x}\n\\'\n"
        );
        assert_eq!(
            prelude(Wrapper::Python, &v),
            "q = \"it's \\\"$HOME\\\" #{x}\\n\\\\\"\n"
        );
        assert_eq!(
            prelude(Wrapper::Ruby, &v),
            "q = \"it's \\\"$HOME\\\" \\#{x}\\n\\\\\"\n",
            "ruby's `#{{` cannot interpolate"
        );
        let t = vec![("t".to_string(), Value::Table(vec![vec![s("a"), n("1")]]))];
        assert_eq!(prelude(Wrapper::Node, &t), "var t = [[\"a\",1]];\n");
        assert_eq!(prelude(Wrapper::Python, &[]), "");
    }

    #[test]
    fn json_numbers_are_strict() {
        for ok in ["0", "-3", "1.5", "2e10", "-0.5E-3"] {
            assert!(json_number(ok), "{ok}");
        }
        for no in ["", "05", "+1", ".5", "5.", "1e", "x", "1_000"] {
            assert!(!json_number(no), "{no}");
        }
    }
}

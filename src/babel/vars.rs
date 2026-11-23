//! `:var` — values handed to a block (OB.11, design §6.1).
//!
//! `:var n=5`, `:var s="text"`, `:var x=disk` (the named block's
//! `#+RESULTS:`) and `:var t=tbl` (a named org table) are resolved here,
//! guest-side, from the buffer's text, and reach the program as a PRELUDE:
//! assignments generated ahead of the body, in the language the record's
//! `value` names. The prelude goes in before the value wrapper runs, so
//! `:results value` sees the variables too.
//!
//! Values cross as hex, as the wrappers' bodies do. Python, ruby and node
//! decode JSON from hex; sh gets a single-quoted string. No quote,
//! backslash, `$` or newline in a value can end the literal it is in.

use super::block::{keyword_value, src_blocks, unescape, SrcBlock};
use super::header_args::HeaderArgs;
use super::wrappers::Wrapper;

/// A resolved `:var` value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    /// Written as a JSON number: babel reads `5` and `: 5` as numbers.
    Number(String),
    Str(String),
    /// Rows of cells, horizontal rules dropped.
    Table(Vec<Vec<Value>>),
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

/// A named block's `#+RESULTS:` as a value: a table when it is one, else its
/// text, a number when that is one line that reads as one.
fn result_value(lines: &[&str], block: &SrcBlock) -> Option<Value> {
    let r = block.results.as_ref()?;
    let body: Vec<&str> = lines
        .get(r.keyword_line as usize + 1..=r.end_line as usize)
        .unwrap_or_default()
        .to_vec();
    if body
        .first()
        .is_some_and(|l| l.trim_start().starts_with('|'))
    {
        return Some(Value::Table(table_rows(body.into_iter())));
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
    Some(match text.as_slice() {
        [one] => scalar(one),
        _ => Value::Str(text.join("\n")),
    })
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
fn resolve_one(lines: &[&str], blocks: &[SrcBlock], value: &str) -> Result<Value, String> {
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
pub(crate) fn resolve(lines: &[&str], header: &HeaderArgs) -> Result<Vec<(String, Value)>, String> {
    let blocks = src_blocks(lines);
    let mut out: Vec<(String, Value)> = Vec::new();
    for a in assignments(header) {
        let Some((name, value)) = a.split_once('=') else {
            return Err(format!("org: `:var {a}` needs a value — `:var {a}=…`"));
        };
        if !identifier(name) {
            return Err(format!("org: `:var {name}` is not a variable name"));
        }
        let value = resolve_one(lines, &blocks, value)?;
        match out.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = value,
            None => out.push((name.to_string(), value)),
        }
    }
    Ok(out)
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn json(v: &Value, out: &mut String) {
    match v {
        Value::Number(n) => out.push_str(n),
        Value::Str(s) => {
            out.push('"');
            for c in s.chars() {
                match c {
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
                    json(cell, out);
                }
                out.push(']');
            }
            out.push(']');
        }
    }
}

fn to_json(v: &Value) -> String {
    let mut s = String::new();
    json(v, &mut s);
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
    match wrapper {
        Wrapper::Python => out.push_str("import json as __ob_json\n"),
        Wrapper::Ruby => out.push_str("require \"json\"\n"),
        Wrapper::Node | Wrapper::Sh => {}
    }
    for (name, v) in vars {
        let line = match wrapper {
            // `'…'` holds anything but `'`, which closes, escapes and reopens.
            Wrapper::Sh => format!("{name}='{}'", sh_text(v).replace('\'', r"'\''")),
            Wrapper::Python => format!(
                "{name} = __ob_json.loads(bytes.fromhex(\"{}\").decode(\"utf-8\"))",
                hex(&to_json(v))
            ),
            // Wrapped in an array: not every `json` takes a bare scalar.
            Wrapper::Ruby => format!(
                "{name} = JSON.parse([\"{}\"].pack(\"H*\").force_encoding(\"UTF-8\"))[0]",
                hex(&format!("[{}]", to_json(v)))
            ),
            Wrapper::Node => format!(
                "var {name} = JSON.parse(Buffer.from(\"{}\", \"hex\").toString(\"utf8\"));",
                hex(&to_json(v))
            ),
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
        resolve(&lines, &effective_header_args(&lines, &block))
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
    fn preludes_cannot_be_escaped() {
        let v = vec![("q".to_string(), s("it's \"$HOME\"\n\\"))];
        assert_eq!(prelude(Wrapper::Sh, &v), "q='it'\\''s \"$HOME\"\n\\'\n");
        for w in [Wrapper::Python, Wrapper::Ruby, Wrapper::Node] {
            let p = prelude(w, &v);
            assert!(!p.contains("HOME"), "{w:?}: the value is not in the clear");
        }
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

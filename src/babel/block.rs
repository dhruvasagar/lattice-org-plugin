//! Where a source block is (OB.4).
//!
//! Read from the LINES, not the syntax tree. The tree is a cache the host
//! fills off the keystroke path, so straight after an edit an action may be
//! handed none, or one a parse behind — and `C-c C-c` deciding whether it is
//! babel's from parse timing would mean the same key on the same text runs a
//! block once and falls through to org the next. The lines are always the
//! text the user sees, and a block's bounds are two marker lines: there is
//! nothing the tree knows here that the text does not.

/// A `#+begin_src` … `#+end_src` pair around a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SrcBounds {
    /// The `#+begin_src` line.
    pub(crate) begin_line: u32,
    /// The `#+end_src` line.
    pub(crate) end_line: u32,
    /// The word after `#+begin_src`, as written; empty when there is none.
    pub(crate) lang: String,
}

/// The rest of `line` after a case-insensitive `#+<marker>` that starts it
/// (after indentation), when the marker is followed by a space or the end.
fn after_marker<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let trimmed = line.trim_start();
    let head = trimmed.get(..marker.len())?;
    if !head.eq_ignore_ascii_case(marker) {
        return None;
    }
    let rest = &trimmed[marker.len()..];
    (rest.is_empty() || rest.starts_with([' ', '\t'])).then_some(rest)
}

/// `#+begin_src`'s header: everything after the marker, or `None` for any
/// other line.
pub(crate) fn begin_src_header(line: &str) -> Option<&str> {
    after_marker(line, "#+begin_src")
}

/// Whether `line` is a block's `#+end_src`.
pub(crate) fn is_end_src(line: &str) -> bool {
    after_marker(line, "#+end_src").is_some_and(|rest| rest.trim().is_empty())
}

/// A headline: one or more `*` at column 0, then a space. A block cannot
/// span one, so the search for a marker stops there.
fn is_headline(line: &str) -> bool {
    let stars = line.bytes().take_while(|b| *b == b'*').count();
    stars > 0 && line[stars..].starts_with(' ')
}

/// The source block `at` is in — on its `#+begin_src` line, its body, or its
/// `#+end_src` line — or `None`.
///
/// Bounded by the nearest headline either way, so the cost is the size of
/// the section, not of the file. An unterminated `#+begin_src` is not a block
/// (org does not run one either).
pub(crate) fn src_bounds_at(
    line: &dyn Fn(u32) -> Option<String>,
    line_count: u32,
    at: u32,
) -> Option<SrcBounds> {
    let mut begin = None;
    for i in (0..=at.min(line_count.saturating_sub(1))).rev() {
        let text = line(i)?;
        if let Some(header) = begin_src_header(&text) {
            let lang = header.split_whitespace().next().unwrap_or("").to_string();
            begin = Some((i, lang));
            break;
        }
        // An `#+end_src` above the cursor closes some other block — unless it
        // is the cursor's own line, which belongs to the block above it.
        if (is_end_src(&text) && i != at) || is_headline(&text) {
            return None;
        }
    }
    let (begin_line, lang) = begin?;
    for j in begin_line + 1..line_count {
        let text = line(j)?;
        if is_end_src(&text) {
            return (at <= j).then_some(SrcBounds {
                begin_line,
                end_line: j,
                lang,
            });
        }
        if is_headline(&text) {
            return None;
        }
    }
    None
}

use super::header_args::HeaderArgs;

/// A source block, read whole (OB.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SrcBlock {
    /// The word after `#+begin_src`, as written.
    pub(crate) lang: String,
    /// `#+NAME:` above it.
    pub(crate) name: Option<String>,
    /// Its OWN header arguments: its `#+HEADER:` lines, then its
    /// `#+begin_src` line, later winning. Inherited ones are
    /// [`effective_header_args`]'s.
    pub(crate) header: HeaderArgs,
    /// The code, with org's comma-escapes (`,*`, `,#+`) undone.
    pub(crate) body: String,
    /// The first line that belongs to the block: its `#+NAME:` / `#+HEADER:`
    /// lines when it has them, else its `#+begin_src`.
    pub(crate) first_line: u32,
    /// The `#+begin_src` line.
    pub(crate) begin_line: u32,
    /// The `#+end_src` line.
    pub(crate) end_line: u32,
    /// Its `#+RESULTS:`, when it has one.
    pub(crate) results: Option<Results>,
}

/// A `#+RESULTS:` block: the keyword line and what follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Results {
    /// The `#+RESULTS:` (or `#+RESULTS[hash]:`) line.
    pub(crate) keyword_line: u32,
    /// The last line of the result, inclusive — the keyword line itself for
    /// an empty result, or one whose shape this cannot tell (raw output),
    /// which is then left in place rather than guessed at.
    pub(crate) end_line: u32,
    /// The `[hash]` emacs's `:cache` writes. Read so such a result is
    /// replaced cleanly; never written (design §8).
    pub(crate) hash: Option<String>,
}

/// The value after a case-insensitive `#+<keyword>:` (indentation allowed).
fn keyword_value<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let t = line.trim_start();
    let head = t.get(..keyword.len() + 3)?;
    let (marker, colon) = head.split_at(keyword.len() + 2);
    (marker.eq_ignore_ascii_case(&format!("#+{keyword}")) && colon == ":")
        .then(|| t[keyword.len() + 3..].trim())
}

/// `#+RESULTS:` / `#+RESULTS[hash]:`, as `(hash, name)`.
fn results_keyword(line: &str) -> Option<(Option<String>, String)> {
    let t = line.trim_start();
    let head = t.get(..9)?;
    if !head.eq_ignore_ascii_case("#+results") {
        return None;
    }
    let rest = &t[9..];
    let (hash, rest) = match rest.strip_prefix('[') {
        Some(r) => {
            let close = r.find(']')?;
            (Some(r[..close].to_string()), &r[close + 1..])
        }
        None => (None, rest),
    };
    let name = rest.strip_prefix(':')?.trim().to_string();
    Some((hash, name))
}

/// Undo org's escape of code lines that would otherwise read as org:
/// `,*` and `,#+` (and `,,*` for a line that really began `,*`). One comma
/// goes, the indentation stays.
fn unescape(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    match rest.strip_prefix(',') {
        Some(r) if r.starts_with('*') || r.starts_with("#+") || r.starts_with(",*") => {
            format!("{}{}", &line[..indent], r)
        }
        _ => line.to_string(),
    }
}

/// The extent of the result whose keyword is on `k`.
fn results_extent(lines: &[&str], k: usize) -> usize {
    let Some(first) = lines.get(k + 1) else {
        return k;
    };
    let t = first.trim_start();
    let run = |pred: &dyn Fn(&str) -> bool| {
        let mut j = k + 1;
        while lines.get(j).is_some_and(|l| pred(l)) {
            j += 1;
        }
        j - 1
    };
    let fixed = |l: &str| {
        let t = l.trim_start();
        t == ":" || t.starts_with(": ")
    };
    if fixed(first) {
        return run(&fixed);
    }
    if t.starts_with('|') {
        let end = run(&|l: &str| l.trim_start().starts_with('|'));
        let mut j = end + 1;
        while lines
            .get(j)
            .is_some_and(|l| keyword_value(l, "tblfm").is_some())
        {
            j += 1;
        }
        return j - 1;
    }
    if t.eq_ignore_ascii_case(":results:") {
        return (k + 2..lines.len())
            .find(|&j| lines[j].trim().eq_ignore_ascii_case(":end:"))
            .unwrap_or(k);
    }
    if let Some(kind) = t
        .get(..8)
        .filter(|h| h.eq_ignore_ascii_case("#+begin_"))
        .map(|_| {
            t[8..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
        })
    {
        let end = format!("#+end_{kind}");
        return (k + 2..lines.len())
            .find(|&j| lines[j].trim().eq_ignore_ascii_case(&end))
            .unwrap_or(k);
    }
    if t.starts_with("- ") {
        return run(&|l: &str| l.trim_start().starts_with("- "));
    }
    if t.starts_with("[[") {
        return k + 1;
    }
    k
}

/// The block's `#+RESULTS:`: the one after `#+end_src` (blank lines between
/// allowed) unless it names another block, else — for a named block — a
/// `#+RESULTS: <name>` anywhere in the file.
fn find_results(lines: &[&str], end_line: usize, name: Option<&str>) -> Option<Results> {
    let mut i = end_line + 1;
    while lines.get(i).is_some_and(|l| l.trim().is_empty()) {
        i += 1;
    }
    let at = |k: usize, hash: Option<String>| Results {
        keyword_line: k as u32,
        end_line: results_extent(lines, k) as u32,
        hash,
    };
    if let Some((hash, kw_name)) = lines.get(i).and_then(|l| results_keyword(l)) {
        if kw_name.is_empty() || Some(kw_name.as_str()) == name {
            return Some(at(i, hash));
        }
    }
    let name = name?;
    lines.iter().enumerate().find_map(|(k, l)| {
        let (hash, kw_name) = results_keyword(l)?;
        (kw_name == name).then(|| at(k, hash))
    })
}

/// Read the block whose markers are on `begin` and `end`.
fn read_block(lines: &[&str], begin: usize, end: usize) -> SrcBlock {
    let header_text = begin_src_header(lines[begin]).unwrap_or("");
    let lang = header_text
        .split_whitespace()
        .next()
        .filter(|w| !w.starts_with(':'))
        .unwrap_or("")
        .to_string();
    // Affiliated keywords sit directly above: `#+NAME:` and `#+HEADER:`
    // lines, in any order.
    let mut first = begin;
    let mut name = None;
    let mut headers = Vec::new();
    while first > 0 {
        let l = lines[first - 1];
        if let Some(v) = keyword_value(l, "name") {
            name = Some(v.to_string());
        } else if let Some(v) = keyword_value(l, "header").or_else(|| keyword_value(l, "headers")) {
            headers.push(v);
        } else {
            break;
        }
        first -= 1;
    }
    let mut header = HeaderArgs::default();
    for h in headers.iter().rev() {
        header.merge(&HeaderArgs::parse(h));
    }
    header.merge(&HeaderArgs::parse(header_text));
    let body = lines[begin + 1..end]
        .iter()
        .map(|l| unescape(l))
        .collect::<Vec<_>>()
        .join("\n");
    let results = find_results(lines, end, name.as_deref());
    SrcBlock {
        lang,
        name,
        header,
        body,
        first_line: first as u32,
        begin_line: begin as u32,
        end_line: end as u32,
        results,
    }
}

/// The block `at` is in, read whole. `lines` is the whole buffer.
pub(crate) fn src_block_at(lines: &[&str], at: u32) -> Option<SrcBlock> {
    let line = |n: u32| lines.get(n as usize).map(|l| l.to_string());
    let b = src_bounds_at(&line, lines.len() as u32, at)?;
    Some(read_block(
        lines,
        b.begin_line as usize,
        b.end_line as usize,
    ))
}

/// Every source block in `lines`, in document order.
pub(crate) fn src_blocks(lines: &[&str]) -> Vec<SrcBlock> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if begin_src_header(lines[i]).is_some() {
            if let Some(end) = (i + 1..lines.len()).find(|&j| is_end_src(lines[j])) {
                out.push(read_block(lines, i, end));
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// A headline's level, or `None` for any other line.
fn headline_level(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|b| *b == b'*').count();
    (stars > 0 && line[stars..].starts_with(' ')).then_some(stars)
}

/// The `:NAME: value` properties in the drawer under the headline on `h`.
fn drawer_properties<'a>(lines: &[&'a str], h: usize) -> Vec<(String, &'a str)> {
    let mut i = h + 1;
    // Planning lines sit between a headline and its drawer.
    while lines.get(i).is_some_and(|l| {
        let t = l.trim_start();
        ["SCHEDULED:", "DEADLINE:", "CLOSED:"]
            .iter()
            .any(|p| t.starts_with(p))
    }) {
        i += 1;
    }
    if !lines
        .get(i)
        .is_some_and(|l| l.trim().eq_ignore_ascii_case(":properties:"))
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    for l in &lines[i + 1..] {
        let t = l.trim();
        if t.eq_ignore_ascii_case(":end:") || headline_level(l).is_some() {
            break;
        }
        let Some(rest) = t.strip_prefix(':') else {
            continue;
        };
        let Some(colon) = rest
            .find(": ")
            .or_else(|| rest.strip_suffix(':').map(|r| r.len()))
        else {
            continue;
        };
        out.push((rest[..colon].to_ascii_lowercase(), rest[colon + 1..].trim()));
    }
    out
}

/// One level of inheritance: org's rule for a property, where a plain
/// `name` replaces what was inherited and `name+` appends to it.
fn accumulate(acc: &mut String, props: &[(String, &str)], name: &str) {
    for (k, v) in props {
        if k == name {
            *acc = v.to_string();
        } else if k.strip_suffix('+') == Some(name) {
            if !acc.is_empty() {
                acc.push(' ');
            }
            acc.push_str(v);
        }
    }
}

/// The header arguments `block` runs with, in babel's order, later winning:
/// babel's defaults → `#+PROPERTY: header-args` → `header-args:<lang>` (each
/// inherited down the headlines above the block, org's property rule) → the
/// block's own `#+HEADER:` lines and `#+begin_src` line (design §5.1).
pub(crate) fn effective_header_args(lines: &[&str], block: &SrcBlock) -> HeaderArgs {
    let general = "header-args".to_string();
    let lang = format!("header-args:{}", block.lang.to_ascii_lowercase());

    // The file is the outermost level: `#+PROPERTY:` lines, wherever they are.
    let file_props: Vec<(String, &str)> = lines
        .iter()
        .filter_map(|l| keyword_value(l, "property"))
        .map(|v| {
            let (k, rest) = v.split_once(char::is_whitespace).unwrap_or((v, ""));
            (k.to_ascii_lowercase(), rest.trim())
        })
        .collect();

    // The headlines above the block, innermost first: each one shallower
    // than the last one found.
    let mut ancestors = Vec::new();
    let mut shallowest = usize::MAX;
    for h in (0..block.first_line as usize).rev() {
        if let Some(level) = headline_level(lines[h]) {
            if level < shallowest {
                ancestors.push(h);
                shallowest = level;
                if level == 1 {
                    break;
                }
            }
        }
    }

    let mut acc_general = String::new();
    let mut acc_lang = String::new();
    accumulate(&mut acc_general, &file_props, &general);
    accumulate(&mut acc_lang, &file_props, &lang);
    for &h in ancestors.iter().rev() {
        let props = drawer_properties(lines, h);
        accumulate(&mut acc_general, &props, &general);
        accumulate(&mut acc_lang, &props, &lang);
    }

    let mut args = HeaderArgs::parse(
        ":results replace :session none :noweb no :tangle no :cache no :exports code",
    );
    args.merge(&HeaderArgs::parse(&acc_general));
    args.merge(&HeaderArgs::parse(&acc_lang));
    args.merge(&block.header);
    args
}

/// Which block a run belongs to, so its result finds it again after the
/// buffer moved (design §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockKey {
    /// A block with a `#+NAME:` is that name.
    Named(String),
    /// An unnamed one is its language and its body as it was. Identical
    /// blocks are told apart by which of them it was (`ordinal` of `twins`),
    /// which an edit elsewhere does not move; when the number of twins
    /// changed, by the line it was on instead.
    Anonymous {
        lang: String,
        body_hash: u64,
        ordinal: u32,
        twins: u32,
        line: u32,
    },
}

/// FNV-1a: stable across runs and builds, which `std`'s hasher is not —
/// the key is stored, and outlives the instance that made it.
fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

impl BlockKey {
    /// The key of `block`, one of the blocks in `lines`.
    pub(crate) fn of(lines: &[&str], block: &SrcBlock) -> Self {
        if let Some(n) = &block.name {
            return BlockKey::Named(n.clone());
        }
        let body_hash = fnv(&block.body);
        let twins: Vec<u32> = twins(&src_blocks(lines), &block.lang, body_hash)
            .map(|b| b.begin_line)
            .collect();
        BlockKey::Anonymous {
            lang: block.lang.clone(),
            body_hash,
            ordinal: twins
                .iter()
                .position(|l| *l == block.begin_line)
                .unwrap_or(0) as u32,
            twins: twins.len() as u32,
            line: block.begin_line,
        }
    }

    /// One string, for the store and for a confirm's single argument slot
    /// (design §2): `n:<name>` or `a:<hash>:<ordinal>:<twins>:<line>:<lang>`.
    /// The free-text part goes last so it may hold anything.
    pub(crate) fn encode(&self) -> String {
        match self {
            BlockKey::Named(n) => format!("n:{n}"),
            BlockKey::Anonymous {
                lang,
                body_hash,
                ordinal,
                twins,
                line,
            } => format!("a:{body_hash:016x}:{ordinal}:{twins}:{line}:{lang}"),
        }
    }

    /// The inverse of [`encode`](Self::encode).
    pub(crate) fn decode(text: &str) -> Option<Self> {
        if let Some(n) = text.strip_prefix("n:") {
            return Some(BlockKey::Named(n.to_string()));
        }
        let mut parts = text.strip_prefix("a:")?.splitn(5, ':');
        let body_hash = u64::from_str_radix(parts.next()?, 16).ok()?;
        let ordinal = parts.next()?.parse().ok()?;
        let twins = parts.next()?.parse().ok()?;
        let line = parts.next()?.parse().ok()?;
        let lang = parts.next()?.to_string();
        Some(BlockKey::Anonymous {
            lang,
            body_hash,
            ordinal,
            twins,
            line,
        })
    }

    /// The block this key names in `lines` as they are now, or `None` when
    /// it is gone or its body changed — never a guess (design §5.3).
    pub(crate) fn find(&self, lines: &[&str]) -> Option<SrcBlock> {
        let blocks = src_blocks(lines);
        match self {
            BlockKey::Named(n) => blocks.into_iter().find(|b| b.name.as_deref() == Some(n)),
            BlockKey::Anonymous {
                lang,
                body_hash,
                ordinal,
                twins: was,
                line,
            } => {
                let found: Vec<&SrcBlock> = twins(&blocks, lang, *body_hash).collect();
                let pick = if found.len() as u32 == *was {
                    found.get(*ordinal as usize).copied()
                } else {
                    found
                        .into_iter()
                        .min_by_key(|b| b.begin_line.abs_diff(*line))
                };
                pick.cloned()
            }
        }
    }
}

/// The unnamed blocks of `lang` whose body hashes to `body_hash`, in order.
fn twins<'a>(
    blocks: &'a [SrcBlock],
    lang: &'a str,
    body_hash: u64,
) -> impl Iterator<Item = &'a SrcBlock> + 'a {
    blocks
        .iter()
        .filter(move |b| b.name.is_none() && b.lang == lang && fnv(&b.body) == body_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(text: &str, at: u32) -> Option<SrcBounds> {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let line = |n: u32| lines.get(n as usize).cloned();
        src_bounds_at(&line, lines.len() as u32, at)
    }

    const DOC: &str = "* H\n\
                       text\n\
                       #+begin_src sh :results output\n\
                       echo hi\n\
                       #+end_src\n\
                       after\n\
                       #+BEGIN_SRC Python\n\
                       print(1)\n\
                       #+END_SRC\n";

    #[test]
    fn every_line_of_a_block_is_in_it_and_nothing_around_it_is() {
        for at in 2..=4 {
            assert_eq!(
                bounds(DOC, at),
                Some(SrcBounds {
                    begin_line: 2,
                    end_line: 4,
                    lang: "sh".to_string()
                }),
                "line {at}"
            );
        }
        for at in [0, 1, 5] {
            assert_eq!(bounds(DOC, at), None, "line {at}");
        }
    }

    #[test]
    fn markers_are_case_insensitive_and_the_language_is_kept_as_written() {
        assert_eq!(bounds(DOC, 7).map(|b| b.lang), Some("Python".to_string()));
    }

    #[test]
    fn other_blocks_and_near_misses_are_not_src_blocks() {
        let text = "#+begin_quote\nq\n#+end_quote\n#+begin_srcx\nx\n#+end_src\n";
        for at in 0..6 {
            assert_eq!(bounds(text, at), None, "line {at}");
        }
    }

    #[test]
    fn an_unterminated_block_or_one_cut_by_a_headline_is_none() {
        assert_eq!(bounds("#+begin_src sh\necho\n", 1), None);
        assert_eq!(bounds("#+begin_src sh\necho\n* H\n#+end_src\n", 1), None);
        assert_eq!(bounds("#+begin_src sh\n* H\necho\n#+end_src\n", 2), None);
    }

    #[test]
    fn a_block_with_no_language_has_an_empty_one() {
        assert_eq!(
            bounds("#+begin_src\nx\n#+end_src\n", 1).map(|b| b.lang),
            Some(String::new())
        );
    }

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn a_block_reads_its_name_headers_body_and_results() {
        let text = "#+NAME: disk\n\
                    #+HEADER: :dir /tmp\n\
                    #+begin_src sh :results output\n\
                    ,* not a headline\n\
                    df -h .\n\
                    #+end_src\n\
                    \n\
                    #+RESULTS: disk\n\
                    : one\n\
                    : two\n\
                    after\n";
        let l = lines(text);
        let b = src_block_at(&l, 4).unwrap();
        assert_eq!(b.name.as_deref(), Some("disk"));
        assert_eq!(b.lang, "sh");
        assert_eq!((b.first_line, b.begin_line, b.end_line), (0, 2, 5));
        assert_eq!(b.body, "* not a headline\ndf -h .");
        assert_eq!(b.header.get("dir"), Some(&["/tmp".to_string()][..]));
        assert_eq!(b.header.results(), ["output"]);
        assert_eq!(
            b.results,
            Some(Results {
                keyword_line: 7,
                end_line: 9,
                hash: None
            })
        );
    }

    fn results_of(text: &str) -> Option<(u32, u32, Option<String>)> {
        let l = lines(text);
        src_blocks(&l)
            .into_iter()
            .next()?
            .results
            .map(|r| (r.keyword_line, r.end_line, r.hash))
    }

    #[test]
    fn every_result_shape_has_its_extent() {
        let block = "#+begin_src sh\nx\n#+end_src\n";
        let cases = [
            (": a\n:\n: b\nafter\n", 6),
            ("| a | b |\n|---+---|\n| 1 | 2 |\n#+TBLFM: $2=1\nafter\n", 7),
            (":RESULTS:\nanything\n* even this\n:END:\nafter\n", 7),
            ("#+begin_example\n* x\n#+end_example\nafter\n", 6),
            ("- one\n- two\nafter\n", 5),
            ("[[file:plot.png]]\nafter\n", 4),
            ("raw text\n", 3),
            ("", 3),
        ];
        for (after, end) in cases {
            let text = format!("{block}#+RESULTS:\n{after}");
            assert_eq!(results_of(&text), Some((3, end, None)), "{after:?}");
        }
    }

    #[test]
    fn an_emacs_cached_result_is_read_with_its_hash() {
        let text = "#+begin_src sh\nx\n#+end_src\n#+RESULTS[3f2a9c]:\n: 1\n";
        assert_eq!(results_of(text), Some((3, 4, Some("3f2a9c".to_string()))));
    }

    #[test]
    fn a_named_blocks_results_may_be_elsewhere_but_another_blocks_are_not_its() {
        let text = "#+NAME: a\n#+begin_src sh\nx\n#+end_src\n#+RESULTS: b\n: no\n\
                    text\n#+RESULTS: a\n: yes\n";
        assert_eq!(results_of(text), Some((7, 8, None)));
        let anon = "#+begin_src sh\nx\n#+end_src\n#+RESULTS: b\n: no\n";
        assert_eq!(results_of(anon), None);
    }

    #[test]
    fn the_merge_honours_babels_order() {
        let text = "#+PROPERTY: header-args :dir /file :results output\n\
                    #+PROPERTY: header-args:sh :session file-sh\n\
                    * One\n\
                    :PROPERTIES:\n\
                    :header-args: :dir /one :noweb yes\n\
                    :END:\n\
                    ** Two\n\
                    *** Three\n\
                    SCHEDULED: <2026-11-22>\n\
                    :PROPERTIES:\n\
                    :header-args+: :tangle out.sh\n\
                    :header-args:sh: :session three\n\
                    :END:\n\
                    #+begin_src sh :results table\n\
                    x\n\
                    #+end_src\n";
        let l = lines(text);
        let b = src_block_at(&l, 14).unwrap();
        let h = effective_header_args(&l, &b);
        // `* One`'s plain `header-args` REPLACES the file's (org's property
        // rule), so `/file` and `output` are gone; `*** Three`'s `+` adds.
        assert_eq!(h.get("dir"), Some(&["/one".to_string()][..]));
        assert_eq!(h.get("noweb"), Some(&["yes".to_string()][..]));
        assert_eq!(h.get("tangle"), Some(&["out.sh".to_string()][..]));
        assert_eq!(h.get("session"), Some(&["three".to_string()][..]));
        // The block's own word joins the default's group-wise.
        assert_eq!(h.results(), ["replace", "table"]);
    }

    #[test]
    fn a_sibling_headlines_drawer_is_not_inherited() {
        let text = "* A\n:PROPERTIES:\n:header-args: :dir /a\n:END:\n* B\n\
                    #+begin_src sh\nx\n#+end_src\n";
        let l = lines(text);
        let b = src_block_at(&l, 6).unwrap();
        assert_eq!(effective_header_args(&l, &b).get("dir"), None);
    }

    #[test]
    fn block_keys_round_trip_and_tell_identical_blocks_apart_by_line() {
        let text = "#+begin_src sh\necho\n#+end_src\nmid\n#+begin_src sh\necho\n#+end_src\n";
        let l = lines(text);
        let blocks = src_blocks(&l);
        let (a, b) = (BlockKey::of(&l, &blocks[0]), BlockKey::of(&l, &blocks[1]));
        assert_ne!(a, b);
        for key in [&a, &b, &BlockKey::Named("x:y".to_string())] {
            assert_eq!(BlockKey::decode(&key.encode()).as_ref(), Some(key));
        }
        assert_eq!(a.find(&l).unwrap().begin_line, 0);
        assert_eq!(b.find(&l).unwrap().begin_line, 4);
        // Two lines inserted above: each still finds its own.
        let shifted = format!("new\nnew\n{text}");
        let l2 = lines(&shifted);
        assert_eq!(a.find(&l2).unwrap().begin_line, 2);
        assert_eq!(b.find(&l2).unwrap().begin_line, 6);
        // One twin's body changed: the count moved, so the line decides —
        // and the edited block is not a candidate at all.
        let edited = text.replacen("echo", "echo changed", 1);
        let l3 = lines(&edited);
        assert_eq!(b.find(&l3).map(|b| b.begin_line), Some(4));
        let both = text.replace("echo", "other");
        assert!(a.find(&lines(&both)).is_none());
    }

    /// OB.5's measurement: `C-c C-c` reads the whole buffer and merges, so
    /// this is what one run pays before anything spawns. A 5 000-line file
    /// with the drawer six levels up — the far end of what a real org file
    /// nests. The crate is a cdylib, which `benches/` cannot link, so it is
    /// a timing test: `cargo test --release block_merge_timing -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore = "timing; run with --release --ignored --nocapture"]
    fn block_merge_timing() {
        let mut text = String::from("#+PROPERTY: header-args :results output\n");
        for level in 1..=6 {
            text.push_str(&format!("{} Level {level}\n", "*".repeat(level)));
            if level == 1 {
                text.push_str(":PROPERTIES:\n:header-args+: :dir /tmp\n:END:\n");
            }
            for i in 0..830 {
                text.push_str(&format!("prose line {i} at level {level}\n"));
            }
        }
        text.push_str("#+begin_src sh :results table\necho hi\n#+end_src\n");
        let at = text.lines().count() as u32 - 2;
        let n = 200;
        let start = std::time::Instant::now();
        for _ in 0..n {
            let l: Vec<&str> = text.lines().collect();
            let b = src_block_at(&l, at).unwrap();
            let h = effective_header_args(&l, &b);
            assert_eq!(h.get("dir"), Some(&["/tmp".to_string()][..]));
        }
        let per = start.elapsed() / n;
        println!(
            "{} lines: split + find + merge = {per:?} per run",
            text.lines().count()
        );
    }
}

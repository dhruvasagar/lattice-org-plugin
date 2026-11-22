//! Header arguments: `:results output table :dir ~/src :var x=1` (OB.5).
//!
//! Org-babel's grammar, kept as it is, because emacs reads the same lines
//! (design §1): a header is `:key` followed by the words up to the next
//! `:key`, a double-quoted word may hold spaces, and a key given twice is
//! given again — later wins — except `:var`, which accumulates, and
//! `:results`, whose words fall into groups that each keep their own latest
//! word (`output` and `table` are different questions, so `:results table`
//! below `:results output` means `output table`).

/// A parsed set of header arguments, in the order they were written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HeaderArgs {
    entries: Vec<(String, Vec<String>)>,
}

/// `:results` words, by the question each answers. Two words in one group
/// cannot both hold, so the later replaces the earlier; words in different
/// groups combine. Babel's own groups (`org-babel-common-header-args-w-values`).
const RESULTS_GROUPS: &[&[&str]] = &[
    &["output", "value"],
    &["file", "list", "vector", "table", "scalar", "verbatim"],
    &[
        "raw", "html", "latex", "org", "code", "pp", "drawer", "link", "graphics",
    ],
    &["replace", "silent", "none", "append", "prepend", "discard"],
];

fn results_group(word: &str) -> Option<usize> {
    RESULTS_GROUPS
        .iter()
        .position(|g| g.iter().any(|w| w.eq_ignore_ascii_case(word)))
}

/// Split a header into words: whitespace-separated, with a double-quoted
/// run (quotes kept, as babel keeps them for `:var s="a b"`) one word, and
/// a `=`-joined `name="a b"` one word too.
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

impl HeaderArgs {
    /// Parse `:key value… :key value…`. Words before the first `:key` are
    /// not header arguments (on a `#+begin_src` line, the language) and are
    /// skipped — the caller strips what it knows is there.
    pub(crate) fn parse(text: &str) -> Self {
        let mut args = HeaderArgs::default();
        let mut key: Option<String> = None;
        let mut values: Vec<String> = Vec::new();
        for word in words(text) {
            if let Some(name) = word.strip_prefix(':').filter(|n| !n.is_empty()) {
                if let Some(k) = key.take() {
                    args.set(&k, std::mem::take(&mut values));
                }
                key = Some(name.to_ascii_lowercase());
            } else if key.is_some() {
                values.push(word);
            }
        }
        if let Some(k) = key {
            args.set(&k, values);
        }
        args
    }

    /// Give `key` these values, with babel's rules for a key given again.
    fn set(&mut self, key: &str, values: Vec<String>) {
        match key {
            "var" => self.entries.push((key.to_string(), values)),
            "results" => {
                let Some(slot) = self.entries.iter_mut().find(|(k, _)| k == "results") else {
                    self.entries.push((key.to_string(), values));
                    return;
                };
                for word in values {
                    match results_group(&word) {
                        Some(g) => {
                            slot.1.retain(|w| results_group(w) != Some(g));
                            slot.1.push(word);
                        }
                        None => slot.1.push(word),
                    }
                }
            }
            _ => match self.entries.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = values,
                None => self.entries.push((key.to_string(), values)),
            },
        }
    }

    /// Lay `later` over this set, later winning (`:var` accumulating,
    /// `:results` by group).
    pub(crate) fn merge(&mut self, later: &HeaderArgs) {
        for (k, v) in &later.entries {
            self.set(k, v.clone());
        }
    }

    /// The words given for `key`, if it was given.
    pub(crate) fn get(&self, key: &str) -> Option<&[String]> {
        // The LAST entry: for `var` that is one of several, which `vars`
        // reads instead; for every other key there is only one.
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }

    /// `:results`' words, lowercased.
    pub(crate) fn results(&self) -> Vec<String> {
        self.get("results")
            .unwrap_or_default()
            .iter()
            .map(|w| w.to_ascii_lowercase())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_take_the_words_up_to_the_next_key() {
        let h = HeaderArgs::parse(":results output table :dir ~/src");
        assert_eq!(h.results(), ["output", "table"]);
        assert_eq!(h.get("dir"), Some(&["~/src".to_string()][..]));
        assert_eq!(h.get("cache"), None);
    }

    #[test]
    fn a_quoted_value_is_one_word_and_keeps_its_spaces() {
        let h = HeaderArgs::parse(r#":var s="a  b" :dir "/my dir""#);
        assert_eq!(h.get("var"), Some(&[r#"s="a  b""#.to_string()][..]));
        assert_eq!(h.get("dir"), Some(&[r#""/my dir""#.to_string()][..]));
    }

    #[test]
    fn var_accumulates_where_every_other_key_is_given_again() {
        let mut h = HeaderArgs::parse(":var a=1 :dir /a :tangle yes");
        h.merge(&HeaderArgs::parse(":var b=2 :dir /b"));
        let vars = h.entries.iter().filter(|(k, _)| k == "var").count();
        assert_eq!(vars, 2, "a second :var is another assignment");
        assert_eq!(h.get("dir"), Some(&["/b".to_string()][..]));
        assert_eq!(h.get("tangle"), Some(&["yes".to_string()][..]));
    }

    #[test]
    fn results_merge_by_group() {
        let mut h = HeaderArgs::parse(":results output replace");
        h.merge(&HeaderArgs::parse(":results table"));
        assert_eq!(h.results(), ["output", "replace", "table"]);
        h.merge(&HeaderArgs::parse(":results value silent"));
        assert_eq!(h.results(), ["table", "value", "silent"]);
    }

    #[test]
    fn words_before_the_first_key_are_not_arguments() {
        let h = HeaderArgs::parse("sh -n :results output");
        assert_eq!(h.results(), ["output"]);
    }
}

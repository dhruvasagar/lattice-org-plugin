//! `{{{name(args)}}}` expansion, ox's macros.
//!
//! A template is a `#+MACRO: name body` line or one of the keyword
//! built-ins (`title`, `author`, `date`, `email`, `input-file`), collected
//! by [`Options::resolve`](super::options::Options::resolve) in its one
//! precedence pass. The function built-ins (`time`, `date` with a format,
//! `property`, `keyword`, `n`) are computed here.
//!
//! ox expands macros in the text before it parses; this expands them in the
//! tree, as the render reaches them, and parses the expansion as objects.
//! The outcome is the same wherever a macro stands where an object may, which
//! is where ox's own replacement looks for them. Expanding as the render
//! walks also keeps `n` counting in document order without a separate pass.

use std::collections::{BTreeMap, HashMap};

use crate::time_format::{format_time, When};

/// What a macro turned into.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Expansion {
    /// Org text, to be parsed as objects.
    Text(String),
    /// Something ox would do that this does not; nothing is inserted and the
    /// export names it.
    Unsupported(String),
}

/// The macro table and the state the function built-ins keep.
pub(crate) struct Macros {
    templates: BTreeMap<String, String>,
    /// The file's keywords, for `keyword(NAME)`.
    keywords: Vec<(String, String)>,
    /// `n`'s counters, by name.
    counters: HashMap<String, i64>,
    /// When the export was asked for, for `time`.
    now: When,
}

impl Macros {
    pub(crate) fn new(
        templates: BTreeMap<String, String>,
        keywords: Vec<(String, String)>,
        now: When,
    ) -> Self {
        Macros {
            templates,
            keywords,
            counters: HashMap::new(),
            now,
        }
    }

    /// Expand `{{{name(args)}}}` where the headline being rendered has
    /// `properties`. `Err` names a macro that is not defined, which fails
    /// the export, as in ox.
    pub(crate) fn expand(
        &mut self,
        name: &str,
        args: &[String],
        properties: &[(String, String)],
    ) -> Result<Expansion, String> {
        let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
        let text = match name {
            "time" => format_time(arg(0), self.now),
            "date" if !arg(0).is_empty() => {
                let raw = self.templates.get("date").map_or("", String::as_str);
                match crate::timestamp::first_stamp(raw) {
                    Some(s) => {
                        let (h, m) = s.time.unwrap_or((0, 0));
                        let local_secs = crate::timestamp::epoch_day(s.year, s.month, s.day)
                            * 86_400
                            + i64::from(h) * 3_600
                            + i64::from(m) * 60;
                        format_time(arg(0), When { local_secs })
                    }
                    // Not a timestamp: ox gives the value as written.
                    None => raw.to_string(),
                }
            }
            "property" => {
                if args.len() > 1 {
                    return Ok(Expansion::Unsupported(
                        "{{{property}}} with a search".to_string(),
                    ));
                }
                properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(arg(0)))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            }
            "keyword" => self
                .keywords
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(arg(0)))
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            "n" => {
                let counter = self.counters.entry(arg(0).to_string()).or_insert(0);
                match arg(1).trim() {
                    "-" => {}
                    "" => *counter += 1,
                    n => match n.parse::<i64>() {
                        Ok(n) => *counter = n,
                        Err(_) => *counter += 1,
                    },
                }
                counter.to_string()
            }
            "modification-time" => {
                return Ok(Expansion::Unsupported(
                    "{{{modification-time}}}".to_string(),
                ))
            }
            _ => {
                let Some(body) = self.templates.get(name) else {
                    return Err(format!("org: {{{{{{{name}}}}}}} is not defined"));
                };
                if body.trim_start().starts_with("(eval") {
                    return Ok(Expansion::Unsupported(format!(
                        "{{{{{{{name}}}}}}} (an eval macro)"
                    )));
                }
                substitute(body, args)
            }
        };
        Ok(Expansion::Text(text))
    }
}

/// `body` with `$1`, `$2`… replaced by the arguments; a missing one is empty.
fn substitute(body: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        match after[..digits].parse::<usize>() {
            Ok(n) if n > 0 => {
                out.push_str(args.get(n - 1).map_or("", String::as_str));
                rest = &after[digits..];
            }
            _ => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn macros(templates: &[(&str, &str)]) -> Macros {
        Macros::new(
            templates
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            vec![
                ("SUBJECT".to_string(), "org".to_string()),
                ("SUBJECT".to_string(), "export".to_string()),
            ],
            // 2026-01-05 09:30, local.
            When {
                local_secs: crate::timestamp::epoch_day(2026, 1, 5) * 86_400 + 9 * 3_600 + 1_800,
            },
        )
    }

    fn text(m: &mut Macros, name: &str, args: &[&str]) -> String {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        match m.expand(name, &args, &[("Owner".into(), "Ada".into())]) {
            Ok(Expansion::Text(t)) => t,
            other => panic!("{name}: {other:?}"),
        }
    }

    #[test]
    fn templates_take_their_arguments() {
        let mut m = macros(&[("greet", "Hello, $1 and $2$3! ($ and $x)")]);
        assert_eq!(
            text(&mut m, "greet", &["a", "b"]),
            "Hello, a and b! ($ and $x)"
        );
    }

    #[test]
    fn the_function_built_ins() {
        let mut m = macros(&[("date", "<2026-02-03 Tue>"), ("title", "T")]);
        assert_eq!(
            text(&mut m, "time", &["%Y-%m-%d %H:%M"]),
            "2026-01-05 09:30"
        );
        assert_eq!(text(&mut m, "date", &["%d/%m"]), "03/02");
        assert_eq!(text(&mut m, "date", &[]), "<2026-02-03 Tue>");
        assert_eq!(text(&mut m, "property", &["owner"]), "Ada");
        assert_eq!(text(&mut m, "property", &["none"]), "");
        assert_eq!(text(&mut m, "keyword", &["subject"]), "org export");
        assert_eq!(text(&mut m, "title", &[]), "T");
    }

    #[test]
    fn counters_count_and_reset() {
        let mut m = macros(&[]);
        let seq: Vec<String> = [&[][..], &[], &["", "-"], &["b"], &["", "10"], &[], &["b"]]
            .iter()
            .map(|a| text(&mut m, "n", a))
            .collect();
        assert_eq!(seq, ["1", "2", "2", "1", "10", "11", "2"]);
    }

    #[test]
    fn undefined_fails_and_the_unsupported_are_named() {
        let mut m = macros(&[("calc", "(eval (+ 1 2))")]);
        assert_eq!(
            m.expand("nope", &[], &[]),
            Err("org: {{{nope}}} is not defined".to_string())
        );
        assert!(matches!(
            m.expand("calc", &[], &[]),
            Ok(Expansion::Unsupported(w)) if w.contains("eval")
        ));
        assert!(matches!(
            m.expand("modification-time", &[], &[]),
            Ok(Expansion::Unsupported(_))
        ));
    }
}

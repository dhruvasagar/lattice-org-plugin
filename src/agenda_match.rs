//! OA.9 — org's tags/todo match syntax, parsed to **data**.
//!
//! `tags-todo "-CANCELLED+WAITING|HOLD/!"` is the language a real agenda
//! configuration is written in, and it is what makes a custom command say
//! anything more interesting than "everything with a date". This module turns
//! one of those strings into a [`MatchExpr`] that [`MatchExpr::admits`] can
//! answer with.
//!
//! ## Data, not a closure
//!
//! `Filter`'s own doc already gives the reason and it applies unchanged here:
//! a filter that is data can be parsed from a config file, and a closure
//! cannot. `org.agenda-sections` and `org.agenda-custom-commands` are TOML
//! strings, so every part of a section has to survive being written down.
//!
//! ## The grammar, and where it stops
//!
//! ```text
//!   expr     := alt ( '|' alt )*          -- OR, lowest precedence
//!   alt      := term+                     -- AND
//!   term     := ('+' | '-')? atom
//!   atom     := PROP op VALUE | TAG
//!   todo     := '/' '!'? ( kw ( '|' kw )* )?
//! ```
//!
//! Scoped to what real configurations use — every shape in this list is taken
//! from one:
//!
//! ```text
//!   "NOTE"                              a tag
//!   "-CANCELLED/!"                      exclude a tag; not-done keywords only
//!   "-CANCELLED/!NEXT"                  not-done AND keyword NEXT
//!   "-REFILE-CANCELLED-WAITING-HOLD/!"
//!   "-CANCELLED+WAITING|HOLD/!"         (-CANCELLED AND +WAITING) OR (HOLD)
//!   "STYLE=\"habit\""                   property equality
//! ```
//!
//! Org's fuller syntax — regexp tags (`{^work}`), numeric property
//! comparison, `LEVEL=` — is deliberately absent until something asks for it.
//! An unsupported construct is a parse ERROR rather than a silently ignored
//! term: a match that quietly means something narrower than it says would
//! hide rows, and "you have no tasks" is the worst thing this view can say
//! incorrectly.

/// One alternative's requirement on a single tag or property.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Term {
    /// `+tag` (or a bare `tag`) — the row must carry it.
    Tag(String),
    /// `-tag` — the row must not.
    NotTag(String),
    /// `PROP="value"` — the row's own property must equal `value`.
    Property { key: String, value: String },
    /// `PROP<>"value"`.
    PropertyNot { key: String, value: String },
}

/// Which TODO keywords an expression admits, from its `/…` part.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum TodoMatch {
    /// No `/` at all: the keyword is not constrained. A row with no keyword
    /// still matches, which is what makes a plain `tags` search find plain
    /// headlines.
    #[default]
    Any,
    /// `/!` — any not-done keyword. A row with NO keyword does not match:
    /// `!` means "is in a todo state", and no state is not a state.
    NotDone,
    /// `/NEXT` or `/!NEXT` — one of these keywords. `not_done` additionally
    /// requires the keyword be a not-done one, which is what `/!KW` means and
    /// what makes it different from `/KW` on a DONE-side keyword.
    Keywords { names: Vec<String>, not_done: bool },
}

/// A parsed match expression: OR over alternatives, each an AND over terms,
/// plus one keyword constraint shared by all of them (org puts the `/…` at
/// the end and it applies to the whole match).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MatchExpr {
    pub alternatives: Vec<Vec<Term>>,
    pub todo: TodoMatch,
}

/// Why a match string could not be read. Carries the offending text, because
/// the user wrote it and the message is the only way back to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchError {
    Empty,
    /// A `+` / `-` with nothing after it.
    DanglingSign,
    /// A property test whose value is not a quoted string.
    UnquotedValue(String),
    /// Something the supported subset does not cover — a regexp tag, a
    /// numeric comparison. Named rather than ignored, per the module header.
    Unsupported(String),
}

impl MatchError {
    pub fn message(&self) -> String {
        match self {
            MatchError::Empty => "empty match".to_string(),
            MatchError::DanglingSign => "a `+` or `-` with no tag after it".to_string(),
            MatchError::UnquotedValue(t) => {
                format!("property value must be quoted: {t}")
            }
            MatchError::Unsupported(t) => format!("unsupported match syntax: {t}"),
        }
    }
}

/// Parse one org match string.
pub fn parse(source: &str) -> Result<MatchExpr, MatchError> {
    let source = source.trim();
    if source.is_empty() {
        return Err(MatchError::Empty);
    }
    // The `/…` tail is split off first: `/` cannot appear in a tag, and
    // splitting on the FIRST one keeps `/!NEXT|WAITING` whole for the todo
    // parser rather than letting the alternation split it.
    let (tags_part, todo) = match source.split_once('/') {
        Some((head, tail)) => (head.trim(), parse_todo(tail.trim())?),
        None => (source, TodoMatch::Any),
    };

    let mut alternatives = Vec::new();
    for alt in tags_part.split('|') {
        let terms = parse_alternative(alt.trim())?;
        if !terms.is_empty() {
            alternatives.push(terms);
        }
    }
    // `-REFILE/` and `/!` are both legal: a match constraining only the
    // keyword has no tag alternatives, and one empty alternative means "every
    // row" rather than "no rows".
    if alternatives.is_empty() {
        alternatives.push(Vec::new());
    }
    Ok(MatchExpr { alternatives, todo })
}

fn parse_todo(tail: &str) -> Result<TodoMatch, MatchError> {
    let (not_done, rest) = match tail.strip_prefix('!') {
        Some(rest) => (true, rest.trim()),
        None => (false, tail),
    };
    if rest.is_empty() {
        return Ok(if not_done {
            TodoMatch::NotDone
        } else {
            // A bare trailing `/` constrains nothing — `-REFILE/` is in a real
            // config and means exactly `-REFILE`.
            TodoMatch::Any
        });
    }
    let names: Vec<String> = rest
        .split('|')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if names.is_empty() {
        return Ok(TodoMatch::Any);
    }
    Ok(TodoMatch::Keywords { names, not_done })
}

fn parse_alternative(alt: &str) -> Result<Vec<Term>, MatchError> {
    let mut terms = Vec::new();
    let bytes = alt.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Leading whitespace between terms is tolerated; org writes them
        // unspaced but a config file is prose to the person writing it.
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let negated = match bytes[i] {
            b'+' => {
                i += 1;
                false
            }
            b'-' => {
                i += 1;
                true
            }
            _ => false,
        };
        // Read the atom: up to the next `+`/`-` that is not inside quotes.
        let start = i;
        let mut in_quotes = false;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => in_quotes = !in_quotes,
                b'+' | b'-' if !in_quotes => break,
                _ => {}
            }
            i += 1;
        }
        let atom = alt[start..i].trim();
        if atom.is_empty() {
            return Err(MatchError::DanglingSign);
        }
        terms.push(parse_atom(atom, negated)?);
    }
    Ok(terms)
}

fn parse_atom(atom: &str, negated: bool) -> Result<Term, MatchError> {
    if atom.starts_with('{') {
        // A regexp tag. Refused by name rather than read as a literal tag
        // called `{^work}`, which would match nothing and look like the rows
        // were simply absent.
        return Err(MatchError::Unsupported(atom.to_string()));
    }
    if let Some((key, value)) = atom.split_once("<>") {
        return property(key, value, !negated);
    }
    if let Some((key, value)) = atom.split_once('=') {
        // `<=` / `>=` / `<` / `>` are numeric comparisons this subset does
        // not do; catch them before they read as an equality on a key
        // ending in `<`.
        if key.ends_with('<') || key.ends_with('>') {
            return Err(MatchError::Unsupported(atom.to_string()));
        }
        return property(key, value, negated);
    }
    if atom.contains('<') || atom.contains('>') {
        return Err(MatchError::Unsupported(atom.to_string()));
    }
    Ok(if negated {
        Term::NotTag(atom.to_string())
    } else {
        Term::Tag(atom.to_string())
    })
}

fn property(key: &str, value: &str, negated: bool) -> Result<Term, MatchError> {
    let key = key.trim();
    let value = value.trim();
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .ok_or_else(|| MatchError::UnquotedValue(value.to_string()))?;
    Ok(if negated {
        Term::PropertyNot {
            key: key.to_string(),
            value: unquoted.to_string(),
        }
    } else {
        Term::Property {
            key: key.to_string(),
            value: unquoted.to_string(),
        }
    })
}

impl MatchExpr {
    /// Does this expression admit a row?
    ///
    /// `is_done` is passed rather than derived because what counts as done is
    /// the user's `org.todo-keywords`, which the scan already resolved once.
    pub fn admits(
        &self,
        tags: &[String],
        properties: &[(String, String)],
        keyword: Option<&str>,
        is_done: impl Fn(Option<&str>) -> bool,
    ) -> bool {
        if !self.todo_admits(keyword, &is_done) {
            return false;
        }
        self.alternatives
            .iter()
            .any(|alt| alt.iter().all(|t| term_admits(t, tags, properties)))
    }

    fn todo_admits(&self, keyword: Option<&str>, is_done: &impl Fn(Option<&str>) -> bool) -> bool {
        match &self.todo {
            TodoMatch::Any => true,
            // No keyword is not a not-done keyword: `/!` means "in a todo
            // state", and a plain headline is in none.
            TodoMatch::NotDone => keyword.is_some() && !is_done(keyword),
            TodoMatch::Keywords { names, not_done } => {
                let Some(kw) = keyword else {
                    return false;
                };
                if *not_done && is_done(Some(kw)) {
                    return false;
                }
                names.iter().any(|n| n == kw)
            }
        }
    }
}

fn term_admits(term: &Term, tags: &[String], properties: &[(String, String)]) -> bool {
    let has_prop = |key: &str, value: &str| {
        properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case(key) && v == value)
    };
    match term {
        Term::Tag(t) => tags.iter().any(|x| x == t),
        Term::NotTag(t) => !tags.iter().any(|x| x == t),
        Term::Property { key, value } => has_prop(key, value),
        Term::PropertyNot { key, value } => !has_prop(key, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(kw: Option<&str>) -> bool {
        matches!(kw, Some("DONE") | Some("CANCELLED"))
    }

    fn expr(s: &str) -> MatchExpr {
        parse(s).unwrap_or_else(|e| panic!("{s:?}: {}", e.message()))
    }

    /// Every shape in the module header, from real configurations.
    #[test]
    fn the_shapes_real_configs_use_all_parse() {
        assert_eq!(
            expr("NOTE").alternatives,
            vec![vec![Term::Tag("NOTE".into())]]
        );
        assert_eq!(expr("-CANCELLED/!").todo, TodoMatch::NotDone);
        assert_eq!(
            expr("-CANCELLED/!NEXT").todo,
            TodoMatch::Keywords {
                names: vec!["NEXT".into()],
                not_done: true
            }
        );
        assert_eq!(
            expr("-REFILE-CANCELLED-WAITING-HOLD/!").alternatives[0].len(),
            4,
            "four exclusions, not one term called `REFILE-CANCELLED-…`"
        );
        assert_eq!(
            expr("-CANCELLED+WAITING|HOLD/!").alternatives,
            vec![
                vec![
                    Term::NotTag("CANCELLED".into()),
                    Term::Tag("WAITING".into())
                ],
                vec![Term::Tag("HOLD".into())],
            ],
            "`|` is the LOWEST precedence: (-CANCELLED AND +WAITING) OR (HOLD)"
        );
        assert_eq!(
            expr(r#"STYLE="habit""#).alternatives,
            vec![vec![Term::Property {
                key: "STYLE".into(),
                value: "habit".into()
            }]]
        );
    }

    /// A trailing bare `/` constrains nothing — `-REFILE/` is in a real config
    /// and means exactly `-REFILE`.
    #[test]
    fn a_bare_trailing_slash_constrains_nothing() {
        assert_eq!(expr("-REFILE/").todo, TodoMatch::Any);
        assert_eq!(expr("-REFILE/").alternatives[0].len(), 1);
    }

    /// `/!` with no tags at all: every row in a todo state.
    #[test]
    fn a_keyword_only_match_admits_every_tag_set() {
        let e = expr("/!");
        assert!(e.admits(&[], &[], Some("TODO"), done));
        assert!(!e.admits(&[], &[], Some("DONE"), done));
        assert!(
            !e.admits(&[], &[], None, done),
            "no keyword is not a not-done keyword"
        );
    }

    #[test]
    fn alternation_is_or_and_juxtaposition_is_and() {
        let e = expr("-CANCELLED+WAITING|HOLD");
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(e.admits(&t(&["WAITING"]), &[], None, done));
        assert!(
            !e.admits(&t(&["WAITING", "CANCELLED"]), &[], None, done),
            "the first alternative is excluded by CANCELLED…"
        );
        assert!(
            e.admits(&t(&["HOLD", "CANCELLED"]), &[], None, done),
            "…but the second alternative has no such term"
        );
        assert!(!e.admits(&t(&["other"]), &[], None, done));
    }

    #[test]
    fn a_property_match_is_case_insensitive_in_its_key_only() {
        let e = expr(r#"STYLE="habit""#);
        let p = |k: &str, v: &str| vec![(k.to_string(), v.to_string())];
        assert!(e.admits(&[], &p("STYLE", "habit"), None, done));
        assert!(e.admits(&[], &p("style", "habit"), None, done), "key folds");
        assert!(
            !e.admits(&[], &p("STYLE", "Habit"), None, done),
            "the value does not — a property value is data"
        );
    }

    #[test]
    fn a_negated_property_admits_a_row_without_it() {
        let e = expr(r#"STYLE<>"habit""#);
        assert!(e.admits(&[], &[], None, done), "absent counts as not-equal");
        assert!(!e.admits(&[], &[("STYLE".into(), "habit".into())], None, done));
    }

    /// An unsupported construct is an ERROR, never a term that silently
    /// matches nothing — the module header's rule, and the reason is that a
    /// narrower-than-written match hides rows.
    #[test]
    fn unsupported_syntax_is_refused_by_name() {
        assert!(matches!(parse("{^work}"), Err(MatchError::Unsupported(_))));
        assert!(matches!(parse("LEVEL>2"), Err(MatchError::Unsupported(_))));
        assert!(matches!(
            parse("PRIORITY<=\"B\""),
            Err(MatchError::Unsupported(_))
        ));
    }

    #[test]
    fn a_malformed_match_is_refused_rather_than_guessed_at() {
        assert_eq!(parse(""), Err(MatchError::Empty));
        assert_eq!(parse("   "), Err(MatchError::Empty));
        assert_eq!(parse("-"), Err(MatchError::DanglingSign));
        assert!(matches!(
            parse("STYLE=habit"),
            Err(MatchError::UnquotedValue(_))
        ));
    }

    /// A quoted value containing `-` must not be split as a term boundary.
    #[test]
    fn a_sign_inside_a_quoted_value_is_not_a_term_boundary() {
        let e = expr(r#"KIND="a-b""#);
        assert_eq!(
            e.alternatives,
            vec![vec![Term::Property {
                key: "KIND".into(),
                value: "a-b".into()
            }]]
        );
    }

    /// `/!KW` differs from `/KW` exactly when the keyword is a done one.
    #[test]
    fn bang_before_a_keyword_still_requires_not_done() {
        let strict = expr("/!DONE");
        assert!(
            !strict.admits(&[], &[], Some("DONE"), done),
            "`/!DONE` asks for a not-done keyword named DONE, which is nothing"
        );
        let loose = expr("/DONE");
        assert!(loose.admits(&[], &[], Some("DONE"), done));
    }
}

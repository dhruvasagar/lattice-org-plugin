//! OR.11a — `${field}`: what a roam template knows about the node it is making.
//!
//! Design: `docs/dev/architecture/org-roam.md` §6.3.
//!
//! ## Two placeholder syntaxes, because they answer different questions
//!
//! `%` interpolates the **capture context** — when you pressed the key, what
//! buffer you were in, what you typed ([`crate::capture`] owns all of it).
//! `${}` interpolates the **node being created**: its title, its slug, its id.
//! Neither can express the other. `%^{Title}` would prompt for a title the
//! picker already asked for, and `${a}` has no meaning in a plain org capture
//! that creates no node.
//!
//! So they coexist rather than compete, and a template uses both — the
//! reference corpus's `pkos-concept.org` opens
//!
//! ```org
//! #+Title: ${title}
//! …
//! :QUESTION: %^{What one question does this answer?}
//! ```
//!
//! ## This is a correctness fix, not a nicety
//!
//! Ten of the eleven templates in the reference corpus contain `${title}`.
//! Without expansion they do not degrade — they write the characters
//! `${title}` into the user's new note, as the first line, as the title. Every
//! note made from a template is then wrong in the one field that names it.
//!
//! ## Unknown fields survive verbatim
//!
//! [`crate::capture`]'s rule for unknown `%x`, for its reason: a template is
//! user text, and a `${autor}` that did not expand can be found and fixed,
//! while one that vanished cannot. A typo must not silently eat a line.

/// What a roam template may interpolate about the node being created.
#[derive(Debug, Clone, Default)]
pub struct Node<'a> {
    /// The title the user typed into the picker.
    pub title: &'a str,
    /// The filename slug derived from it — `roam_find::slug`'s output.
    pub slug: &'a str,
    /// The freshly-minted `:ID:`.
    pub id: &'a str,
}

/// Expand `${title}` / `${slug}` / `${id}` in `body`.
///
/// Runs **before** [`crate::capture::expand_with`], never instead of it: a
/// template carrying both syntaxes needs both passes, and doing `${}` first
/// means a title containing a literal `%U` is inserted as text rather than
/// being re-read as a capture placeholder. That ordering is the safe one —
/// user data must not become template syntax.
///
/// `$` that does not open a `${` is literal, so a template mentioning a shell
/// variable or a price is untouched.
pub fn expand_fields(body: &str, node: &Node<'_>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(close) = after.find('}') else {
            // An unterminated `${` is the rest of the template, verbatim.
            // Scanning on would silently swallow everything after a typo.
            out.push_str(&rest[at..]);
            return out;
        };
        let name = &after[..close];
        match name {
            "title" => out.push_str(node.title),
            "slug" => out.push_str(node.slug),
            "id" => out.push_str(node.id),
            // Verbatim, braces included — see the module note.
            _ => {
                out.push_str("${");
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node<'a>(title: &'a str, slug: &'a str, id: &'a str) -> Node<'a> {
        Node { title, slug, id }
    }

    #[test]
    fn the_three_fields_expand() {
        let n = node("Rust Async", "rust_async", "ABC-123");
        assert_eq!(
            expand_fields("#+title: ${title}", &n),
            "#+title: Rust Async"
        );
        assert_eq!(expand_fields("${slug}.org", &n), "rust_async.org");
        assert_eq!(expand_fields(":ID: ${id}", &n), ":ID: ABC-123");
    }

    #[test]
    fn several_fields_in_one_body_all_expand() {
        let n = node("Rust Async", "rust_async", "ABC-123");
        assert_eq!(
            expand_fields(":ID: ${id}\n#+title: ${title}\n# ${slug}", &n),
            ":ID: ABC-123\n#+title: Rust Async\n# rust_async"
        );
    }

    /// An unknown field survives with its braces. A typo must be findable.
    #[test]
    fn an_unknown_field_is_left_verbatim() {
        let n = node("T", "t", "I");
        assert_eq!(expand_fields("${autor}", &n), "${autor}");
        assert_eq!(expand_fields("a ${x} b ${title}", &n), "a ${x} b T");
    }

    /// An unterminated `${` takes the rest of the template with it, verbatim —
    /// rather than scanning to a `}` on some later line and eating everything
    /// between, which is what a typo would otherwise cost.
    #[test]
    fn an_unterminated_field_does_not_eat_the_template() {
        let n = node("T", "t", "I");
        assert_eq!(
            expand_fields("#+title: ${title\nbody stays", &n),
            "#+title: ${title\nbody stays"
        );
    }

    /// A bare `$` is not a placeholder. Templates mention prices and shell
    /// variables.
    #[test]
    fn a_dollar_that_opens_nothing_is_literal() {
        let n = node("T", "t", "I");
        assert_eq!(
            expand_fields("costs $5 and $HOME", &n),
            "costs $5 and $HOME"
        );
        assert_eq!(expand_fields("$", &n), "$");
    }

    #[test]
    fn a_body_with_no_fields_is_unchanged() {
        let n = node("T", "t", "I");
        assert_eq!(expand_fields("* plain\nbody\n", &n), "* plain\nbody\n");
        assert_eq!(expand_fields("", &n), "");
    }

    /// `%` placeholders are NOT this pass's business — they must survive it
    /// untouched so `capture::expand_with` can do its half afterwards.
    #[test]
    fn capture_placeholders_pass_through_untouched() {
        let n = node("Rust", "rust", "I");
        assert_eq!(
            expand_fields("#+title: ${title}\n:Q: %^{Question}\n%U\n%?", &n),
            "#+title: Rust\n:Q: %^{Question}\n%U\n%?"
        );
    }

    /// A title containing template syntax is inserted as TEXT. This is why
    /// `${}` runs before `%`: user data must not become template syntax.
    #[test]
    fn a_title_is_data_not_syntax() {
        let n = node("100% done", "100_done", "I");
        assert_eq!(expand_fields("#+title: ${title}", &n), "#+title: 100% done");
    }
}

//! OC.4 — what a capture template asks the user for.
//!
//! A template like
//!
//! ```text
//! * %^{Word} :fc:
//! - Context: %^{Context sentence}
//! - Translation: %^{Translation}
//! ```
//!
//! needs three answers before a single write. That is the only way a template
//! of this shape can exist at all — a vocabulary entry is not one line of typed
//! text, it is several named fields.
//!
//! This module only *finds* the questions. Collecting the answers is the
//! transient `Argument` mechanism (`PendingTransientArgument` → the menu is
//! parked, the value lands in `TransientState`, the menu comes back), which is
//! what lattice already uses for magit's argument rows. An earlier draft
//! encoded the answers into the prompt buffer's name instead; that was a second
//! spelling of a mechanism the editor already has, and it is gone.
//!
//! Design: `docs/dev/architecture/org-capture.md` §5.

/// The `%^{…}` questions in `body`, in order, duplicates included.
///
/// A template that asks the same question twice gets two fields and two
/// independent answers — the second is not assumed to repeat the first, since
/// `%^{Line}` twice in a list template plainly means two different lines.
pub fn questions(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'%' && bytes[i + 1] == b'^' && bytes[i + 2] == b'{' {
            if let Some(end) = body[i + 3..].find('}') {
                let q = &body[i + 3..i + 3 + end];
                if !q.trim().is_empty() {
                    out.push(q.trim().to_string());
                }
                i += 3 + end + 1;
                continue;
            }
            // An unclosed `%^{` is left alone — `capture::expand` writes it
            // through verbatim, so the user can see what they mistyped.
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_question_is_asked_twice() {
        assert_eq!(
            questions("- %^{Line}\n- %^{Line}"),
            vec!["Line".to_string(), "Line".to_string()]
        );
    }

    /// A question with no text tells the user nothing about what to type, so
    /// it is not asked. `capture::expand` leaves it in the output, where it is
    /// visible and fixable.
    #[test]
    fn an_empty_question_is_not_asked() {
        assert!(questions("* %^{} and %^{   }").is_empty());
    }

    /// An unclosed `%^{` is a typo, and the template is user text: it survives
    /// to the output rather than swallowing the rest of the template.
    #[test]
    fn an_unclosed_question_is_not_a_prompt() {
        assert!(questions("* %^{Word").is_empty());
        assert_eq!(questions("* %^{A} %^{B"), vec!["A".to_string()]);
    }
}

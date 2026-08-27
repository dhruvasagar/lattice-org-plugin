//! Capture: get a thought into org without leaving what you were doing (OM.11).
//!
//! Emacs's `org-capture` is a template system with a selection menu, `%^{}`
//! interactive prompts, `%[file]` includes and per-template targets. This is
//! the spine of it: one template, one prompt, one target — because the thing
//! capture is FOR is not losing the thought, and every extra hop between the
//! chord and the text landing is a chance to lose it.
//!
//! The template placeholders are org's own spelling, so what a user already
//! knows transfers:
//!
//! | | |
//! |---|---|
//! | `%?` | what you typed. Absent ⇒ appended on its own line. |
//! | `%U` | today, inactive: `[2026-08-26 Wed]` |
//! | `%T` | today, active: `<2026-08-26 Wed>` — the agenda sees this one |
//! | `%%` | a literal `%` |
//!
//! An unknown `%x` is left **verbatim** rather than dropped. A template is
//! user text; silently eating part of it is worse than writing a `%d` that
//! did not expand, which is visible and fixable.

use crate::timestamp;

/// Expand `template` with `entered` and today's date, no `%^{}` answers.
pub fn expand(template: &str, entered: &str, today: i64, annotation: &str) -> String {
    expand_with(template, entered, &[], today, annotation)
}

/// Expand `template`, substituting `%^{…}` from `answers` in template order.
///
/// `today` is passed in rather than read here so the expansion is a pure
/// function — the same reason the agenda's `begin` captures its anchor once.
///
/// **Answers are consumed positionally**, which is why a question that was
/// never asked (an empty `%^{}`) is left verbatim rather than eating an
/// answer: shifting the sequence would substitute every later answer one slot
/// early, and the result would look plausible while being wrong.
pub fn expand_with(
    template: &str,
    entered: &str,
    answers: &[String],
    today: i64,
    annotation: &str,
) -> String {
    let (y, m, d) = crate::agenda::civil_from_epoch_day(today);
    let stamp = format!(
        "{y:04}-{m:02}-{d:02} {}",
        timestamp::DAY_NAMES[timestamp::weekday(y, m, d)]
    );

    let mut out = String::with_capacity(template.len() + entered.len());
    let mut saw_placeholder = false;
    let mut next_answer = 0usize;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // OC.4: `%^{Question}` — the menu collected these as fields, in this
        // order, so they are consumed in this order.
        if chars.peek() == Some(&'^') {
            let mut lookahead = chars.clone();
            lookahead.next();
            if lookahead.peek() == Some(&'{') {
                lookahead.next();
                let mut question = String::new();
                let mut closed = false;
                for qc in lookahead.by_ref() {
                    if qc == '}' {
                        closed = true;
                        break;
                    }
                    question.push(qc);
                }
                if closed {
                    chars = lookahead;
                    if question.trim().is_empty() {
                        // Never asked (`capture_flow::questions` skips it), so
                        // it consumes no answer and stays visible.
                        out.push_str("%^{");
                        out.push_str(&question);
                        out.push('}');
                    } else {
                        if let Some(a) = answers.get(next_answer) {
                            out.push_str(a);
                        }
                        next_answer += 1;
                    }
                    continue;
                }
                // Unclosed: verbatim, like any other unknown placeholder.
            }
        }
        match chars.next() {
            Some('?') => {
                out.push_str(entered);
                saw_placeholder = true;
            }
            Some('U') => out.push_str(&format!("[{stamp}]")),
            Some('T') => out.push_str(&format!("<{stamp}>")),
            // OC.5b: `%t` is the ACTIVE date-only stamp — an entry due on a day
            // without claiming a time.
            //
            // **It renders identically to `%T` today, and that is a gap in `%T`
            // rather than in `%t`.** In org proper `%T` carries a time of day;
            // here it does not, because the only clock this plugin reads is
            // `today_epoch_day` — whole days since the epoch. So both forms
            // currently emit `<date Day>`. `%t` is still worth having: it is
            // what a user writes when they mean a date, and it is already
            // CORRECT — the day `%T` grows a time, templates using `%t` keep
            // meaning what they meant, and only `%T` changes.
            Some('t') => out.push_str(&format!("<{stamp}>")),
            // OC.5b: `%a` — a link back to where the capture fired. Empty when
            // the capture came from a buffer with no path (a scratch buffer, or
            // the capture menu itself): an org link to nothing is worse than no
            // link, because it looks followable and is not.
            Some('a') => out.push_str(annotation),
            Some('%') => out.push('%'),
            // Unknown, or a trailing `%`: verbatim.
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }

    // A template with no `%?` still has to carry the text somewhere, or the
    // chord silently discards what the user typed — the one outcome capture
    // must never have.
    if !saw_placeholder && !entered.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(entered);
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-08-26 is a Wednesday.
    fn today() -> i64 {
        timestamp::epoch_day(2026, 8, 26)
    }

    #[test]
    fn the_entered_text_lands_where_the_template_says() {
        assert_eq!(
            expand("* TODO %?", "call the bank", today(), ""),
            "* TODO call the bank\n"
        );
    }

    #[test]
    fn both_timestamp_forms_carry_the_weekday() {
        assert_eq!(
            expand("%U %T", "", today(), ""),
            "[2026-08-26 Wed] <2026-08-26 Wed>\n"
        );
    }

    /// A template that forgot `%?` must not eat the text. Losing what the user
    /// typed is the one failure capture cannot have.
    #[test]
    fn a_template_without_a_placeholder_still_keeps_the_text() {
        assert_eq!(
            expand("* Note", "the thought", today(), ""),
            "* Note\nthe thought\n"
        );
    }

    /// An unknown placeholder is left visible rather than dropped: a template
    /// is user text, and a `%d` that did not expand can be fixed, whereas one
    /// that vanished cannot be found.
    /// OC.5b: `%t` is the active date-only stamp.
    ///
    /// It renders the same as `%T` today because `%T` carries no time of day
    /// yet — a gap in `%T`, not in `%t`. Asserted against the literal form
    /// rather than against `%T` so that when `%T` grows a time this test keeps
    /// describing what `%t` means instead of quietly following it.
    #[test]
    fn the_active_date_stamp_is_bracketed_and_dated() {
        let out = expand("%t", "", today(), "");
        assert!(
            out.starts_with('<') && out.trim_end().ends_with('>'),
            "ACTIVE, so angle brackets — an inactive `[…]` stamp would never \
             reach the agenda, and that is the whole difference: {out}"
        );
        assert!(out.contains("2026-08-26 Wed"), "{out}");
        assert!(!out.contains(':'), "no time of day: {out}");
    }

    /// OC.5b: `%a` is the annotation the caller computed — expanded verbatim,
    /// because building the link is the caller's job (it needs the buffer) and
    /// re-deriving it here would need state this function deliberately has none
    /// of.
    #[test]
    fn the_annotation_expands_where_it_is_asked_for() {
        assert_eq!(
            expand(
                "* %?\n  from %a",
                "note",
                today(),
                "[[file:/n.org::7][n.org]]"
            ),
            "* note\n  from [[file:/n.org::7][n.org]]\n"
        );
    }

    /// A capture from a buffer with no path expands `%a` to nothing rather than
    /// to a broken link. An org link to nowhere looks followable and is not,
    /// which is worse than an absent one.
    #[test]
    fn an_empty_annotation_leaves_no_broken_link() {
        assert_eq!(expand("* %?\n  %a", "note", today(), ""), "* note\n  \n");
    }

    /// `%a` and the timestamps coexist, and `%a` does not consume a `%^{}`
    /// answer — it is not a question, it is a fact about the capture.
    #[test]
    fn the_annotation_does_not_consume_a_question_answer() {
        assert_eq!(
            expand_with(
                "%^{Kind}: %? (%a)",
                "buy milk",
                &["TODO".to_string()],
                today(),
                "[[file:/n.org::1][n.org]]",
            ),
            "TODO: buy milk ([[file:/n.org::1][n.org]])\n"
        );
    }

    #[test]
    fn an_unknown_placeholder_survives_verbatim() {
        assert_eq!(expand("%d %% %?", "x", today(), ""), "%d % x\n");
    }

    /// OC.4: the questions are substituted in template order, each at its own
    /// position — the property the whole fields menu exists to deliver.
    #[test]
    fn questions_substitute_in_order_at_their_own_positions() {
        let answers = vec![
            "chat".to_string(),
            "le chat noir".to_string(),
            "cat".to_string(),
        ];
        assert_eq!(
            expand_with(
                "* %^{Word} :fc:\n- Context: %^{Context}\n- T: %^{Translation}",
                "",
                &answers,
                today(),
                "",
            ),
            "* chat :fc:\n- Context: le chat noir\n- T: cat\n"
        );
    }

    /// Questions and `%?` are independent: the body goes where `%?` is, the
    /// answers where their own questions are.
    #[test]
    fn a_question_and_the_body_coexist() {
        assert_eq!(
            expand_with(
                "* %^{Kind}: %?",
                "buy milk",
                &["TODO".to_string()],
                today(),
                ""
            ),
            "* TODO: buy milk\n"
        );
    }

    /// A missing answer leaves an empty slot rather than shifting every later
    /// one up — a substitution that silently slid would look plausible and be
    /// wrong.
    #[test]
    fn a_missing_answer_leaves_its_slot_empty_without_shifting_the_rest() {
        assert_eq!(
            expand_with("[%^{A}][%^{B}]", "", &["only".to_string()], today(), ""),
            "[only][]\n"
        );
    }

    /// An empty question is never ASKED (`capture_flow::questions` skips it),
    /// so it must consume no answer either — otherwise every answer after it
    /// would land one slot early.
    #[test]
    fn an_empty_question_consumes_no_answer_and_stays_visible() {
        assert_eq!(
            expand_with("[%^{}][%^{Real}]", "", &["x".to_string()], today(), ""),
            "[%^{}][x]\n"
        );
    }

    /// An unclosed `%^{` is a typo in user text: verbatim, like any other
    /// unknown placeholder, and it consumes no answer.
    #[test]
    fn an_unclosed_question_survives_verbatim() {
        assert_eq!(
            expand_with("%^{Real} then %^{oops", "", &["a".to_string()], today(), ""),
            "a then %^{oops\n"
        );
    }

    #[test]
    fn a_trailing_percent_is_not_an_error() {
        assert_eq!(expand("* %", "", today(), ""), "* %\n");
    }

    #[test]
    fn the_result_always_ends_a_line() {
        assert_eq!(expand("* %?", "a", today(), ""), "* a\n");
        assert_eq!(expand("* %?\n", "a", today(), ""), "* a\n");
    }
}

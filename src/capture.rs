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

/// Expand `template` with `entered` and today's date.
///
/// `today` is passed in rather than read here so the expansion is a pure
/// function — the same reason the agenda's `begin` captures its anchor once.
pub fn expand(template: &str, entered: &str, today: i64) -> String {
    let (y, m, d) = crate::agenda::civil_from_epoch_day(today);
    let stamp = format!(
        "{y:04}-{m:02}-{d:02} {}",
        timestamp::DAY_NAMES[timestamp::weekday(y, m, d)]
    );

    let mut out = String::with_capacity(template.len() + entered.len());
    let mut saw_placeholder = false;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('?') => {
                out.push_str(entered);
                saw_placeholder = true;
            }
            Some('U') => out.push_str(&format!("[{stamp}]")),
            Some('T') => out.push_str(&format!("<{stamp}>")),
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
            expand("* TODO %?", "call the bank", today()),
            "* TODO call the bank\n"
        );
    }

    #[test]
    fn both_timestamp_forms_carry_the_weekday() {
        assert_eq!(
            expand("%U %T", "", today()),
            "[2026-08-26 Wed] <2026-08-26 Wed>\n"
        );
    }

    /// A template that forgot `%?` must not eat the text. Losing what the user
    /// typed is the one failure capture cannot have.
    #[test]
    fn a_template_without_a_placeholder_still_keeps_the_text() {
        assert_eq!(
            expand("* Note", "the thought", today()),
            "* Note\nthe thought\n"
        );
    }

    /// An unknown placeholder is left visible rather than dropped: a template
    /// is user text, and a `%d` that did not expand can be fixed, whereas one
    /// that vanished cannot be found.
    #[test]
    fn an_unknown_placeholder_survives_verbatim() {
        assert_eq!(expand("%d %% %?", "x", today()), "%d % x\n");
    }

    #[test]
    fn a_trailing_percent_is_not_an_error() {
        assert_eq!(expand("* %", "", today()), "* %\n");
    }

    #[test]
    fn the_result_always_ends_a_line() {
        assert_eq!(expand("* %?", "a", today()), "* a\n");
        assert_eq!(expand("* %?\n", "a", today()), "* a\n");
    }
}

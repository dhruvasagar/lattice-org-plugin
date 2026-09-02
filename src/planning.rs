//! OA.25 — the planning line under a headline.
//!
//! Slice plan: `lattice/docs/dev/operations/slice-plans/org-agenda.md` phase 7.
//!
//! ```text
//!   * TODO write the thing
//!     DEADLINE: <2026-09-10 Thu> SCHEDULED: <2026-09-03 Thu>
//!     the body starts here
//! ```
//!
//! **One line holds all of them**, and that is the whole reason this module
//! exists. `SCHEDULED:` and `DEADLINE:` look like two independent things to
//! set, and an implementation that inserts each one separately produces
//!
//! ```text
//!   * TODO write the thing
//!     SCHEDULED: <2026-09-03 Thu>
//!     DEADLINE: <2026-09-10 Thu>
//! ```
//!
//! which org does not read back: only the FIRST line after a headline is
//! planning, so the deadline silently becomes body text. The file still opens,
//! the agenda simply stops showing a deadline that is right there in the file —
//! the worst failure shape available, because nothing reports it.
//!
//! So every write is: parse the whole line, change one field, render the whole
//! line. Removing the last field removes the line, because a blank planning
//! line is not a thing org writes.
//!
//! ## Field order
//!
//! `CLOSED DEADLINE SCHEDULED`, which is `org-element-planning-interpreter`'s
//! order. Org itself does not care about the order when reading, but a file
//! that round-trips through emacs comes back in this one, and a plugin that
//! wrote a different order would show up as a spurious diff the first time the
//! user touched the headline in emacs.
//!
//! `CLOSED` is parsed and PRESERVED but never set here — it is what
//! `org-todo` writes when a task moves to a done state, and dropping it on an
//! unrelated `s` would lose the completion time.

/// The three stamps a planning line can carry, each as the raw text between
/// its keyword and the next — `<2026-09-03 Thu>`, `[2026-09-01 Tue 10:11]`.
///
/// Raw rather than parsed, deliberately. A repeater (`.+1d/3d`), a time
/// (`<2026-09-03 Thu 10:00>`), a range — org owns what those mean, and a
/// module whose job is to put a stamp back where it found it does not need to
/// understand one to avoid corrupting it. See `org_date`'s note on repeaters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Planning {
    pub closed: Option<String>,
    pub deadline: Option<String>,
    pub scheduled: Option<String>,
}

/// Which field a key sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Scheduled,
    Deadline,
}

/// What a write to the planning line turns into.
///
/// A separate type from the effect so the decision is testable without a host:
/// "does scheduling something already scheduled REPLACE or stack" is a
/// question about this enum, and answering it in a `Vec<Effect>` would mean
/// asserting against edit ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanEdit {
    /// Rewrite the planning line at `line`, whose current length is `len`.
    Replace { line: u32, len: u32, text: String },
    /// There is no planning line; add one below the headline at `line`.
    Insert { line: u32, text: String },
    /// The last field went away, so the line goes with it. `len` is its
    /// current length; the delete spans to the start of the next line.
    Delete { line: u32, len: u32 },
    /// Nothing to do — clearing a field that was not set.
    Nothing,
}

impl Planning {
    pub fn get(&self, field: Field) -> Option<&String> {
        match field {
            Field::Scheduled => self.scheduled.as_ref(),
            Field::Deadline => self.deadline.as_ref(),
        }
    }

    fn set(&mut self, field: Field, value: Option<String>) {
        match field {
            Field::Scheduled => self.scheduled = value,
            Field::Deadline => self.deadline = value,
        }
    }

    fn is_empty(&self) -> bool {
        self.closed.is_none() && self.deadline.is_none() && self.scheduled.is_none()
    }

    /// The line this planning renders to, at `indent` spaces. `None` when
    /// there is nothing left to write — the caller deletes the line.
    pub fn render(&self, indent: usize) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts: Vec<String> = Vec::with_capacity(3);
        for (kw, value) in [
            ("CLOSED:", &self.closed),
            ("DEADLINE:", &self.deadline),
            ("SCHEDULED:", &self.scheduled),
        ] {
            if let Some(v) = value {
                parts.push(format!("{kw} {v}"));
            }
        }
        Some(format!("{}{}", " ".repeat(indent), parts.join(" ")))
    }
}

/// Read a planning line, or `None` if `line` is not one.
///
/// A planning line is one whose content is nothing but planning keywords and
/// their stamps. The strictness matters in one direction only: a body line
/// that merely MENTIONS `SCHEDULED:` must not be mistaken for planning and
/// rewritten, because that would destroy the user's prose. Being too strict
/// costs an insert of a fresh line, which is recoverable; being too loose eats
/// text.
pub fn parse(line: &str) -> Option<Planning> {
    let body = line.trim();
    if body.is_empty() {
        return None;
    }
    let mut out = Planning::default();
    let mut rest = body;
    let mut saw_one = false;
    while !rest.is_empty() {
        let (field, kw): (&mut Option<String>, &str) = if let Some(r) = rest.strip_prefix("CLOSED:")
        {
            rest = r;
            (&mut out.closed, "CLOSED:")
        } else if let Some(r) = rest.strip_prefix("DEADLINE:") {
            rest = r;
            (&mut out.deadline, "DEADLINE:")
        } else if let Some(r) = rest.strip_prefix("SCHEDULED:") {
            rest = r;
            (&mut out.scheduled, "SCHEDULED:")
        } else {
            // Anything that is not a keyword means this is not a planning
            // line. Not "the rest is ignored" — see the note above.
            return None;
        };
        let _ = kw;
        let stamp = take_stamp(&mut rest)?;
        // A repeated keyword is malformed org; the first wins and the line is
        // still a planning line, which is the reading that loses least.
        if field.is_none() {
            *field = Some(stamp);
        }
        saw_one = true;
        rest = rest.trim_start();
    }
    saw_one.then_some(out)
}

/// Take one `<...>` or `[...]` stamp off the front of `rest`, leaving the
/// remainder. `None` for a keyword with no stamp after it — malformed, and
/// treating the line as planning would then rewrite it into something else.
fn take_stamp(rest: &mut &str) -> Option<String> {
    let trimmed = rest.trim_start();
    let close = match trimmed.as_bytes().first()? {
        b'<' => b'>',
        b'[' => b']',
        _ => return None,
    };
    let end = trimmed.bytes().position(|b| b == close)?;
    let (stamp, tail) = trimmed.split_at(end + 1);
    *rest = tail;
    Some(stamp.to_string())
}

/// The edit that sets (or clears) `field` on the headline at `headline`.
///
/// `next` is the line below the headline — `None` at end of file. `value` is
/// the rendered stamp, or `None` to remove the field, which is what an empty
/// prompt answer means (and the only spelling of "unschedule" that does not
/// need a second key).
pub fn plan_edit(
    headline: u32,
    next: Option<&str>,
    field: Field,
    value: Option<String>,
) -> PlanEdit {
    match next.and_then(|l| parse(l).map(|p| (l, p))) {
        Some((line_text, mut existing)) => {
            if existing.get(field).map(String::as_str) == value.as_deref() {
                // Already says that. Skip the edit rather than push a no-op
                // onto the undo stack — `u` after a key that did nothing must
                // not "undo" it.
                return PlanEdit::Nothing;
            }
            existing.set(field, value);
            let indent = line_text.len() - line_text.trim_start().len();
            let len = line_text.len() as u32;
            match existing.render(indent) {
                // Keeps whatever the OTHER fields were: adding a SCHEDULED to
                // a headline that already has a DEADLINE must not drop it.
                Some(text) => PlanEdit::Replace {
                    line: headline + 1,
                    len,
                    text,
                },
                None => PlanEdit::Delete {
                    line: headline + 1,
                    len,
                },
            }
        }
        // No planning line. Clearing a field it does not have is not an edit.
        None => match value {
            None => PlanEdit::Nothing,
            Some(v) => {
                let mut fresh = Planning::default();
                fresh.set(field, Some(v));
                match fresh.render(0) {
                    Some(text) => PlanEdit::Insert {
                        line: headline + 1,
                        text,
                    },
                    None => PlanEdit::Nothing,
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    const SCHED: &str = "<2026-09-03 Thu>";
    const DEAD: &str = "<2026-09-10 Thu>";

    #[test]
    fn a_planning_line_reads_every_field() {
        let p = parse("  CLOSED: [2026-09-01 Tue 10:11] DEADLINE: <2026-09-10 Thu> SCHEDULED: <2026-09-03 Thu>")
            .expect("a planning line");
        assert_eq!(p.closed.as_deref(), Some("[2026-09-01 Tue 10:11]"));
        assert_eq!(p.deadline.as_deref(), Some(DEAD));
        assert_eq!(p.scheduled.as_deref(), Some(SCHED));
    }

    #[test]
    fn a_stamp_with_a_repeater_survives_whole() {
        // The repeater is org's to interpret; this module's job is to put it
        // back where it found it.
        let p = parse("SCHEDULED: <2026-09-03 Thu .+1d/3d>").expect("a planning line");
        assert_eq!(p.scheduled.as_deref(), Some("<2026-09-03 Thu .+1d/3d>"));
        assert_eq!(
            p.render(0).as_deref(),
            Some("SCHEDULED: <2026-09-03 Thu .+1d/3d>")
        );
    }

    #[test]
    fn prose_that_mentions_a_keyword_is_not_a_planning_line() {
        // The direction that matters: mistaking body text for planning would
        // REWRITE the user's prose. Being too strict only costs an insert.
        assert_eq!(parse("I should note the SCHEDULED: field here"), None);
        assert_eq!(parse("the body starts here"), None);
        assert_eq!(parse(""), None);
        // A keyword with no stamp is malformed, not planning.
        assert_eq!(parse("SCHEDULED:"), None);
        assert_eq!(parse("SCHEDULED: soon"), None);
    }

    #[test]
    fn scheduling_an_unplanned_headline_inserts_a_line() {
        assert_eq!(
            plan_edit(
                4,
                Some("the body starts here"),
                Field::Scheduled,
                Some(SCHED.into())
            ),
            PlanEdit::Insert {
                line: 5,
                text: "SCHEDULED: <2026-09-03 Thu>".into(),
            }
        );
    }

    #[test]
    fn a_headline_at_end_of_file_still_gets_one() {
        assert_eq!(
            plan_edit(4, None, Field::Deadline, Some(DEAD.into())),
            PlanEdit::Insert {
                line: 5,
                text: "DEADLINE: <2026-09-10 Thu>".into(),
            }
        );
    }

    /// The test the slice plan named: scheduling something already scheduled
    /// REPLACES rather than stacking a second `SCHEDULED:`.
    #[test]
    fn rescheduling_replaces_rather_than_stacking() {
        let existing = "  SCHEDULED: <2026-09-01 Tue>";
        let edit = plan_edit(4, Some(existing), Field::Scheduled, Some(SCHED.into()));
        assert_eq!(
            edit,
            PlanEdit::Replace {
                line: 5,
                len: existing.len() as u32,
                text: "  SCHEDULED: <2026-09-03 Thu>".into(),
            }
        );
    }

    /// The other half, and the one an implementation treating the two fields
    /// as independent inserts gets wrong: adding a SCHEDULED must KEEP a
    /// DEADLINE that is already on the line.
    #[test]
    fn adding_a_schedule_keeps_an_existing_deadline() {
        let existing = "  DEADLINE: <2026-09-10 Thu>";
        let PlanEdit::Replace { text, .. } =
            plan_edit(4, Some(existing), Field::Scheduled, Some(SCHED.into()))
        else {
            panic!("expected a replace");
        };
        assert_eq!(
            text,
            "  DEADLINE: <2026-09-10 Thu> SCHEDULED: <2026-09-03 Thu>"
        );
    }

    #[test]
    fn a_closed_stamp_is_never_collateral() {
        // `org-todo` wrote it when the task completed; an unrelated `s` must
        // not lose the completion time.
        let existing = "CLOSED: [2026-09-01 Tue 10:11]";
        let PlanEdit::Replace { text, .. } =
            plan_edit(0, Some(existing), Field::Scheduled, Some(SCHED.into()))
        else {
            panic!("expected a replace");
        };
        assert_eq!(
            text,
            "CLOSED: [2026-09-01 Tue 10:11] SCHEDULED: <2026-09-03 Thu>"
        );
    }

    #[test]
    fn clearing_the_last_field_removes_the_line() {
        let existing = "  SCHEDULED: <2026-09-01 Tue>";
        assert_eq!(
            plan_edit(4, Some(existing), Field::Scheduled, None),
            PlanEdit::Delete {
                line: 5,
                len: existing.len() as u32,
            }
        );
    }

    #[test]
    fn clearing_one_of_two_fields_keeps_the_line() {
        let existing = "  DEADLINE: <2026-09-10 Thu> SCHEDULED: <2026-09-03 Thu>";
        let PlanEdit::Replace { text, .. } = plan_edit(4, Some(existing), Field::Scheduled, None)
        else {
            panic!("expected a replace");
        };
        assert_eq!(text, "  DEADLINE: <2026-09-10 Thu>");
    }

    #[test]
    fn clearing_a_field_that_was_never_set_is_not_an_edit() {
        assert_eq!(
            plan_edit(4, Some("the body"), Field::Scheduled, None),
            PlanEdit::Nothing
        );
        assert_eq!(
            plan_edit(
                4,
                Some("  DEADLINE: <2026-09-10 Thu>"),
                Field::Scheduled,
                None
            ),
            PlanEdit::Nothing
        );
    }

    #[test]
    fn setting_the_same_date_twice_is_not_an_edit() {
        assert_eq!(
            plan_edit(
                4,
                Some("  SCHEDULED: <2026-09-03 Thu>"),
                Field::Scheduled,
                Some(SCHED.into())
            ),
            PlanEdit::Nothing
        );
    }

    #[test]
    fn an_existing_lines_indentation_is_kept() {
        // Files in the wild carry both — `org-adapt-indentation` has flipped
        // default across org versions. Rewriting one to the other spelling
        // makes a diff out of a date change.
        let PlanEdit::Replace { text, .. } = plan_edit(
            0,
            Some("      SCHEDULED: <2026-09-01 Tue>"),
            Field::Scheduled,
            Some(SCHED.into()),
        ) else {
            panic!("expected a replace");
        };
        assert_eq!(text, "      SCHEDULED: <2026-09-03 Thu>");
    }
}

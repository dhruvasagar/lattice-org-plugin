//! HB.2 — what completing a *repeating* task does.
//!
//! Design: `docs/dev/architecture/org-habits.md` §3 (lattice repo).
//!
//! Org's `org-auto-repeat-maybe`, reproduced: shift the planning stamps by
//! their repeaters, reset the keyword instead of leaving it DONE, log the
//! completion, and stamp `:LAST_REPEAT:`.
//!
//! ## The headline never stays DONE
//!
//! This is the part that reads as a bug until you know it. A repeating task's
//! completion is recorded in the **log**, not in the keyword — the keyword
//! goes back to `NEXT` (or whatever `:REPEAT_TO_STATE:` says) and the
//! timestamp moves forward. So the log line is not bookkeeping: it is the only
//! record that the thing was ever done, and it is what the consistency graph
//! is drawn from.
//!
//! Which means a half-applied completion is worse than none. Shifting without
//! logging loses the history; logging without shifting leaves the task due in
//! the past forever. The whole rewrite is one edit so `u` takes it back as one
//! action, and so neither half can land alone.
//!
//! ## What this module is not
//!
//! It does not decide *whether* a task repeats — it answers `None` when
//! nothing on the planning line carries a repeater, and the caller does the
//! ordinary DONE. `:STYLE: habit` is not consulted at all: a repeater is what
//! makes a task repeat, and org treats the style as a *display* hint for the
//! agenda's graph. Requiring it here would silently break every repeating task
//! that is not a habit.

use crate::org_date::Date;
use crate::repeat;

/// A completion, as lines to write back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    /// The subtree region's replacement lines, `first`..=`last` inclusive.
    pub lines: Vec<String>,
    /// The keyword the headline was reset to — for the echo, so the user is
    /// told the task repeated rather than left guessing why DONE vanished.
    pub reset_to: String,
    /// Where the timestamp moved to.
    pub next: Date,
}

/// Everything the rewrite needs that is not in the lines.
pub struct Context<'a> {
    /// `today`, for `.+` repeaters and the log stamp.
    pub today: Date,
    /// `[2026-09-03 Wed 09:14]`, pre-rendered — the guest has no clock of its
    /// own and the caller already resolved one.
    pub now_stamp: &'a str,
    /// The configured keyword sequence, in order, done-states last.
    pub keywords: &'a [String],
    /// Which keywords are terminal (`| DONE`).
    pub done_keywords: &'a [String],
    /// `org-log-into-drawer`. When false the log line sits loose under the
    /// planning line, which is org's other spelling and one emacs re-files
    /// differently — so it is honoured rather than assumed.
    pub log_into_drawer: bool,
}

/// Complete the repeating task whose subtree is `lines` (headline first).
///
/// `None` when the task does not repeat, which is the caller's signal to do
/// an ordinary DONE.
pub fn complete_repeating(
    lines: &[String],
    from_keyword: &str,
    ctx: &Context,
) -> Option<Completed> {
    let headline = lines.first()?;
    let plan_index = lines
        .iter()
        .position(|l| crate::planning::parse(l).is_some());
    let plan_line = plan_index.map(|i| lines[i].clone())?;

    // Shift every stamp that carries a repeater; a task can have both a
    // repeating SCHEDULED and a fixed DEADLINE, and only the repeating ones
    // move.
    let (shifted, next) = shift_planning(&plan_line, ctx.today)?;

    let reset_to = reset_keyword(lines, ctx)?;
    let new_headline = crate::todo::set_keyword(headline, ctx.keywords, &reset_to)?;

    let log = format!(
        "- State \"{}\" from \"{}\" {}",
        done_label(ctx),
        from_keyword,
        ctx.now_stamp
    );

    let mut out: Vec<String> = lines.to_vec();
    out[0] = new_headline;
    if let Some(i) = plan_index {
        out[i] = shifted;
    }
    set_property(&mut out, "LAST_REPEAT", ctx.now_stamp);
    insert_log(&mut out, &log, ctx.log_into_drawer);

    Some(Completed {
        lines: out,
        reset_to,
        next,
    })
}

/// The done keyword to name in the log line — the one the user actually
/// entered. Falls back to `DONE`, which is the only sensible guess and the
/// name org uses when a sequence declares no explicit done state.
fn done_label(ctx: &Context) -> String {
    ctx.done_keywords
        .first()
        .cloned()
        .unwrap_or_else(|| "DONE".to_string())
}

/// What the headline goes back to: `:REPEAT_TO_STATE:` if the subtree
/// declares one, else the first non-done keyword of the sequence.
///
/// The property wins because it is per-task and explicit — the user's habits
/// set `NEXT` while their sequence starts at `TODO`, so ignoring it would
/// quietly re-file every habit into the wrong state.
fn reset_keyword(lines: &[String], ctx: &Context) -> Option<String> {
    if let Some(v) = property(lines, "REPEAT_TO_STATE") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    ctx.keywords
        .iter()
        .find(|k| !ctx.done_keywords.contains(k))
        .cloned()
}

/// Read `:KEY:` out of the subtree's `:PROPERTIES:` drawer.
fn property(lines: &[String], key: &str) -> Option<String> {
    let start = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(":PROPERTIES:"))?;
    for line in &lines[start + 1..] {
        let t = line.trim();
        if t.eq_ignore_ascii_case(":END:") {
            break;
        }
        if let Some(rest) = t.strip_prefix(':') {
            if let Some((k, v)) = rest.split_once(':') {
                if k.eq_ignore_ascii_case(key) {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// Set `:KEY: value` in the `:PROPERTIES:` drawer, replacing any existing
/// entry. No drawer, no property — org does not create one for `LAST_REPEAT`
/// alone, and manufacturing a drawer on a plain repeating task would add
/// three lines the user never asked for.
fn set_property(lines: &mut Vec<String>, key: &str, value: &str) {
    let Some(start) = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(":PROPERTIES:"))
    else {
        return;
    };
    let indent = leading_ws(&lines[start]);
    let entry = format!("{indent}:{key}: {value}");
    for i in start + 1..lines.len() {
        let t = lines[i].trim();
        if t.eq_ignore_ascii_case(":END:") {
            lines.insert(i, entry);
            return;
        }
        if let Some(rest) = t.strip_prefix(':') {
            if let Some((k, _)) = rest.split_once(':') {
                if k.eq_ignore_ascii_case(key) {
                    lines[i] = entry;
                    return;
                }
            }
        }
    }
}

/// Put the state-change line where `org-log-into-drawer` says it goes.
///
/// Into `:LOGBOOK:` when the option is on, creating the drawer if the task has
/// none — unlike `LAST_REPEAT`'s drawer, this one MUST exist, because the log
/// line is the completion record and dropping it loses the history the graph
/// is built from.
///
/// New entries go at the TOP of the drawer, which is org's order: most recent
/// first, so the newest completion is the one you see without scrolling.
fn insert_log(lines: &mut Vec<String>, log: &str, into_drawer: bool) {
    let after_plan = lines
        .iter()
        .position(|l| crate::planning::parse(l).is_some())
        .map(|i| i + 1)
        .unwrap_or(1);
    let indent = lines
        .get(after_plan)
        .map(|l| leading_ws(l))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "  ".to_string());

    if !into_drawer {
        lines.insert(after_plan, format!("{indent}{log}"));
        return;
    }
    if let Some(start) = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(":LOGBOOK:"))
    {
        lines.insert(start + 1, format!("{indent}{log}"));
        return;
    }
    // No drawer yet. It goes after the PROPERTIES drawer when there is one —
    // org's order is properties, then logbook — and after the planning line
    // otherwise.
    let at = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(":END:"))
        .map(|i| i + 1)
        .unwrap_or(after_plan);
    lines.insert(at, format!("{indent}:END:"));
    lines.insert(at, format!("{indent}{log}"));
    lines.insert(at, format!("{indent}:LOGBOOK:"));
}

fn leading_ws(line: &str) -> String {
    line[..line.len() - line.trim_start().len()].to_string()
}

/// Shift every repeating stamp on a planning line. `None` when none repeats.
///
/// Returns the rewritten line and the SCHEDULED date it moved to (falling back
/// to the deadline's, for a task that carries only one) — the caller echoes it
/// so the user learns where the task went.
fn shift_planning(line: &str, today: Date) -> Option<(String, Date)> {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    let mut moved: Option<Date> = None;
    while let Some(open) = rest.find(['<', '[']) {
        let close = if rest.as_bytes()[open] == b'<' {
            '>'
        } else {
            ']'
        };
        let Some(end) = rest[open..].find(close).map(|i| open + i) else {
            break;
        };
        let stamp = &rest[open..=end];
        out.push_str(&rest[..open]);
        match shift_stamp(stamp, today) {
            Some((next_stamp, date)) => {
                out.push_str(&next_stamp);
                // The FIRST shifted stamp is the one reported: a planning line
                // is written SCHEDULED-then-DEADLINE, and the schedule is what
                // "when is it next" means.
                moved.get_or_insert(date);
            }
            None => out.push_str(stamp),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    moved.map(|d| (out, d))
}

/// Shift one `<date … +1d>` stamp. `None` if it carries no repeater.
fn shift_stamp(stamp: &str, today: Date) -> Option<(String, Date)> {
    let open = stamp.chars().next()?;
    let close = if open == '<' { '>' } else { ']' };
    let inner = stamp.strip_prefix(open)?.strip_suffix(close)?;
    let mut parts: Vec<&str> = inner.split_whitespace().collect();
    let rep_index = parts.iter().position(|p| repeat::parse(p).is_some())?;
    let rep = repeat::parse(parts[rep_index])?;
    let date = parse_date(parts.first()?)?;
    let next = repeat::next_date(&rep, date, today);
    // Rebuild: date, weekday, then whatever else rode along (a time, a second
    // repeater, a delay cookie) with the repeater re-rendered in place.
    parts[0] = "";
    let tail: Vec<&str> = parts
        .iter()
        .enumerate()
        .filter(|(i, p)| *i != 0 && *i != rep_index && !p.is_empty() && !is_weekday(p))
        .map(|(_, p)| *p)
        .collect();
    let mut inner_out = format!(
        "{:04}-{:02}-{:02} {}",
        next.year,
        next.month,
        next.day,
        weekday(next)
    );
    for t in tail {
        inner_out.push(' ');
        inner_out.push_str(t);
    }
    inner_out.push(' ');
    inner_out.push_str(&rep.render());
    Some((format!("{open}{inner_out}{close}"), next))
}

fn parse_date(s: &str) -> Option<Date> {
    let mut it = s.split('-');
    let year: i32 = it.next()?.parse().ok()?;
    let month: u32 = it.next()?.parse().ok()?;
    let day: u32 = it.next()?.parse().ok()?;
    (1..=12).contains(&month).then_some(())?;
    (1..=31).contains(&day).then_some(())?;
    Some(Date { year, month, day })
}

fn is_weekday(s: &str) -> bool {
    matches!(s, "Mon" | "Tue" | "Wed" | "Thu" | "Fri" | "Sat" | "Sun")
}

/// The day name org writes. Derived from the day number rather than carried
/// over from the old stamp — the whole point is that the date moved, and a
/// stale weekday is the kind of wrong that survives review.
fn weekday(d: Date) -> &'static str {
    // 1970-01-01 was a Thursday.
    const NAMES: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    NAMES[repeat::to_days(d).rem_euclid(7) as usize]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> Date {
        Date {
            year: y,
            month: m,
            day,
        }
    }

    fn kw() -> Vec<String> {
        ["TODO", "NEXT", "DONE"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
    fn done_kw() -> Vec<String> {
        vec!["DONE".to_string()]
    }

    fn ctx<'a>(now: &'a str, into_drawer: bool, k: &'a [String], dk: &'a [String]) -> Context<'a> {
        Context {
            today: d(2026, 9, 3),
            now_stamp: now,
            keywords: k,
            done_keywords: dk,
            log_into_drawer: into_drawer,
        }
    }

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    /// The user's own habit, end to end.
    #[test]
    fn a_habit_repeats_resets_and_logs() {
        let k = kw();
        let dk = done_kw();
        let src = lines(
            "* NEXT Meditate\n\
             SCHEDULED: <2026-09-02 Wed .+1d/3d>\n\
             :PROPERTIES:\n\
             :STYLE: habit\n\
             :REPEAT_TO_STATE: NEXT\n\
             :END:\n",
        );
        let got = complete_repeating(&src, "NEXT", &ctx("[2026-09-03 Thu 09:14]", true, &k, &dk))
            .expect("a repeating task");

        // 1. the keyword is RESET, not left DONE
        assert_eq!(got.reset_to, "NEXT");
        assert_eq!(got.lines[0], "* NEXT Meditate");
        // 2. `.+1d` from TODAY, not from the old stamp
        assert_eq!(got.next, d(2026, 9, 4));
        assert!(
            got.lines[1].contains("<2026-09-04 Fri .+1d/3d>"),
            "shifted, weekday re-derived, repeater preserved: {:?}",
            got.lines[1]
        );
        // 3. the completion is LOGGED — the only record it happened
        let joined = got.lines.join("\n");
        assert!(
            joined.contains(r#"- State "DONE" from "NEXT" [2026-09-03 Thu 09:14]"#),
            "{joined}"
        );
        assert!(joined.contains(":LOGBOOK:"), "into the drawer: {joined}");
        // 4. and stamped
        assert!(
            joined.contains(":LAST_REPEAT: [2026-09-03 Thu 09:14]"),
            "{joined}"
        );
    }

    /// `:REPEAT_TO_STATE:` beats the sequence's first keyword. The user's
    /// habits say NEXT while their sequence starts at TODO — ignoring the
    /// property would quietly re-file every habit into the wrong state.
    #[test]
    fn the_repeat_to_state_property_wins() {
        let k = kw();
        let dk = done_kw();
        let with = lines("* NEXT X\nSCHEDULED: <2026-09-02 Wed +1d>\n:PROPERTIES:\n:REPEAT_TO_STATE: NEXT\n:END:\n");
        assert_eq!(
            complete_repeating(&with, "NEXT", &ctx("[t]", true, &k, &dk))
                .unwrap()
                .reset_to,
            "NEXT"
        );
        let without = lines("* NEXT X\nSCHEDULED: <2026-09-02 Wed +1d>\n");
        assert_eq!(
            complete_repeating(&without, "NEXT", &ctx("[t]", true, &k, &dk))
                .unwrap()
                .reset_to,
            "TODO",
            "no property: the sequence's first non-done keyword"
        );
    }

    /// A task with no repeater is not this module's business — the caller
    /// does an ordinary DONE. Answering `Some` here would turn every
    /// completed task into a resurrected one.
    #[test]
    fn a_non_repeating_task_is_declined() {
        let k = kw();
        let dk = done_kw();
        for src in [
            "* NEXT X\nSCHEDULED: <2026-09-02 Wed>\n",
            "* NEXT X\n",
            "* NEXT X\nnot a planning line\n",
        ] {
            assert!(
                complete_repeating(&lines(src), "NEXT", &ctx("[t]", true, &k, &dk)).is_none(),
                "{src:?}"
            );
        }
    }

    /// `:STYLE: habit` is deliberately NOT required. A repeater is what makes
    /// a task repeat; the style is a display hint for the graph. Requiring it
    /// would break every repeating task that is not a habit.
    #[test]
    fn a_repeating_task_need_not_be_a_habit() {
        let k = kw();
        let dk = done_kw();
        let src = lines("* NEXT Pay rent\nSCHEDULED: <2026-09-01 Tue +1m>\n");
        let got = complete_repeating(&src, "NEXT", &ctx("[t]", true, &k, &dk)).unwrap();
        assert_eq!(got.next, d(2026, 10, 1), "`+1m` from the STAMP, not today");
    }

    /// `org-log-into-drawer` off puts the line loose under the planning line.
    /// Honoured rather than assumed: emacs re-files the two spellings
    /// differently, so guessing would scatter log lines through the file.
    #[test]
    fn the_drawer_option_is_honoured() {
        let k = kw();
        let dk = done_kw();
        let src = lines("* NEXT X\nSCHEDULED: <2026-09-02 Wed .+1d>\n");
        let got = complete_repeating(&src, "NEXT", &ctx("[ts]", false, &k, &dk)).unwrap();
        let joined = got.lines.join("\n");
        assert!(!joined.contains(":LOGBOOK:"), "{joined}");
        assert!(joined.contains(r#"- State "DONE""#), "{joined}");
    }

    /// A second completion appends to the drawer that is already there, at
    /// the TOP — org's order, newest first.
    #[test]
    fn a_second_completion_joins_the_existing_drawer_newest_first() {
        let k = kw();
        let dk = done_kw();
        let src = lines(
            "* NEXT X\n\
             SCHEDULED: <2026-09-02 Wed .+1d>\n\
             :LOGBOOK:\n\
             - State \"DONE\" from \"NEXT\" [2026-09-01 Tue 08:00]\n\
             :END:\n",
        );
        let got = complete_repeating(&src, "NEXT", &ctx("[2026-09-03 Thu 09:14]", true, &k, &dk))
            .unwrap();
        let joined = got.lines.join("\n");
        assert_eq!(
            joined.matches(":LOGBOOK:").count(),
            1,
            "one drawer: {joined}"
        );
        let newest = joined.find("[2026-09-03").unwrap();
        let older = joined.find("[2026-09-01").unwrap();
        assert!(newest < older, "newest first: {joined}");
    }

    /// A deadline that repeats moves too; one that does not is left alone.
    #[test]
    fn only_repeating_stamps_move() {
        let k = kw();
        let dk = done_kw();
        let src = lines("* NEXT X\nSCHEDULED: <2026-09-02 Wed .+1d> DEADLINE: <2026-12-25 Fri>\n");
        let got = complete_repeating(&src, "NEXT", &ctx("[t]", true, &k, &dk)).unwrap();
        assert!(
            got.lines[1].contains("<2026-09-04 Fri .+1d>"),
            "{:?}",
            got.lines[1]
        );
        assert!(
            got.lines[1].contains("<2026-12-25 Fri>"),
            "the fixed deadline is untouched: {:?}",
            got.lines[1]
        );
    }

    /// The weekday is re-derived from the new date. Carrying the old one over
    /// is the kind of wrong that survives review — it looks like a date.
    #[test]
    fn the_weekday_is_recomputed() {
        assert_eq!(weekday(d(2026, 9, 3)), "Thu");
        assert_eq!(weekday(d(1970, 1, 1)), "Thu");
        assert_eq!(weekday(d(2026, 9, 6)), "Sun");
    }

    /// A time on the stamp survives the shift — `<2026-09-02 Wed 09:00 .+1d>`
    /// is a habit you do at nine, and dropping the time would quietly
    /// reschedule it to midnight.
    #[test]
    fn a_time_of_day_survives() {
        let k = kw();
        let dk = done_kw();
        let src = lines("* NEXT X\nSCHEDULED: <2026-09-02 Wed 09:00 .+1d>\n");
        let got = complete_repeating(&src, "NEXT", &ctx("[t]", true, &k, &dk)).unwrap();
        assert!(
            got.lines[1].contains("<2026-09-04 Fri 09:00 .+1d>"),
            "{:?}",
            got.lines[1]
        );
    }
}

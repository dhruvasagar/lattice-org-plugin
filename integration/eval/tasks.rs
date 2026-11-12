//! The tasks, each graded without a model (design §9), and the scripted
//! answers that let the harness itself be tested without one.
//!
//! A grader reads only what a user could check: the answer's text, the files
//! after the turn, and what the tools returned. It never reads the model's
//! reasoning, so a model is not graded on how it talks, only on what it got
//! right.

use std::path::PathBuf;

use crate::corpus::{CLOCKED, MIGRATION_NOTES};

/// One tool call the model made, as the chat recorded it.
#[derive(Debug, Clone)]
pub struct Call {
    /// The tool's wire name, `org__agenda`.
    pub name: String,
    pub input: serde_json::Value,
    pub output: String,
    pub ok: bool,
}

/// What a turn left behind, for the grader.
pub struct Outcome {
    pub answer: String,
    pub calls: Vec<Call>,
    /// Every corpus file before the turn and after it, sorted by path.
    pub before: Vec<(PathBuf, String)>,
    pub after: Vec<(PathBuf, String)>,
    /// The file the change task expects, as `org.change` writes it, when the
    /// task has one: its path and text.
    pub expected: Option<(PathBuf, String)>,
}

/// What a scripted model does next, given the tool results so far.
pub enum Step {
    Call(&'static str, serde_json::Value),
    Answer(String),
}

pub struct Task {
    pub name: &'static str,
    /// The org profile the chat opens on: the one the matching workflow uses.
    pub profile: &'static str,
    pub prompt: &'static str,
    pub grade: fn(&Outcome) -> Result<(), String>,
    /// A correct run, as a scripted model: the tools a careful model would
    /// call, and an answer made of what they returned.
    pub oracle: fn(&[String]) -> Step,
}

pub const TASKS: &[Task] = &[
    Task {
        name: "today",
        profile: "org-plan",
        prompt: "What's on today?",
        grade: grade_today,
        oracle: oracle_today,
    },
    Task {
        name: "mark-done",
        profile: "org-plan",
        prompt: "Mark the review task done.",
        grade: grade_mark_done,
        oracle: oracle_mark_done,
    },
    Task {
        name: "clock-week",
        profile: "org-report",
        prompt:
            "How much time did I clock on each project over the last seven days, today included?",
        grade: grade_clock,
        oracle: oracle_clock,
    },
    Task {
        name: "notes",
        profile: "org-research",
        prompt: "Find my notes on the database migration and summarise them.",
        grade: grade_notes,
        oracle: oracle_notes,
    },
];

/// The ref a listing gave `heading`: `[ref <ref>]` on its line.
pub fn ref_of(listing: &str, heading: &str) -> Option<String> {
    let line = listing.lines().find(|l| l.contains(heading))?;
    let (_, rest) = line.split_once("[ref ")?;
    Some(rest.split(']').next()?.to_string())
}

/// The turn changed no file.
fn unchanged(o: &Outcome) -> Result<(), String> {
    if o.before == o.after {
        return Ok(());
    }
    let changed: Vec<String> = o
        .after
        .iter()
        .filter(|f| !o.before.contains(f))
        .map(|(p, _)| p.display().to_string())
        .collect();
    Err(format!("a read-only task changed {}", changed.join(", ")))
}

fn mentions(answer: &str, what: &str) -> bool {
    answer.to_lowercase().contains(&what.to_lowercase())
}

// --- today ------------------------------------------------------------------

/// Today's entries: a scheduled task, a deadline, a timed event, a repeat and
/// a habit. Not today: tomorrow's task, an event in ten days, yesterday's
/// finished one.
const TODAY: &[&str] = &["Standup", "Q3 review", "plumber", "Dentist", "Exercise"];
const NOT_TODAY: &[&str] = &["passport", "offsite", "Ship the beta"];

fn grade_today(o: &Outcome) -> Result<(), String> {
    unchanged(o)?;
    let missing: Vec<&str> = TODAY
        .iter()
        .copied()
        .filter(|w| !mentions(&o.answer, w))
        .collect();
    let wrong: Vec<&str> = NOT_TODAY
        .iter()
        .copied()
        .filter(|w| mentions(&o.answer, w))
        .collect();
    match (missing.is_empty(), wrong.is_empty()) {
        (true, true) => Ok(()),
        _ => Err(format!(
            "missing {missing:?}, named though not today {wrong:?}"
        )),
    }
}

fn oracle_today(results: &[String]) -> Step {
    match results {
        [] => Step::Call("org__agenda", serde_json::json!({})),
        [agenda, ..] => Step::Answer(agenda.clone()),
    }
}

// --- mark-done --------------------------------------------------------------

/// The heading the change task is about: the only open task with "review" in
/// it (the vendor contract's review is already DONE).
pub const REVIEW: &str = "Write the Q3 review";

/// Clock times dropped, so an expected file made a minute earlier matches.
fn without_times(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match tail.find(']') {
            // `[2026-11-12 Thu 09:30]`: keep the day, drop the time.
            Some(j) if j > 11 && tail.as_bytes()[5] == b'-' => {
                let inner = &tail[1..j];
                let day: Vec<&str> = inner.split(' ').take(2).collect();
                out.push('[');
                out.push_str(&day.join(" "));
                out.push(']');
                rest = &tail[j + 1..];
            }
            _ => {
                out.push('[');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn grade_mark_done(o: &Outcome) -> Result<(), String> {
    let Some((path, want)) = &o.expected else {
        return Err("the harness made no expected file".to_string());
    };
    for (p, before) in &o.before {
        let after = o
            .after
            .iter()
            .find(|(q, _)| q == p)
            .map(|(_, t)| t.as_str())
            .unwrap_or("");
        if p == path {
            if without_times(after) != without_times(want) {
                return Err(if after == before {
                    "the review task was not changed".to_string()
                } else {
                    format!("{} is not as org.change writes it:\n{after}", p.display())
                });
            }
        } else if after != before {
            return Err(format!("{} changed too", p.display()));
        }
    }
    Ok(())
}

fn oracle_mark_done(results: &[String]) -> Step {
    match results {
        [] => Step::Call("org__query", serde_json::json!({"match": "/TODO"})),
        [listing] => match ref_of(listing, REVIEW) {
            Some(r) => Step::Call(
                "org__change",
                serde_json::json!({"op": "set-state", "ref": r, "state": "DONE"}),
            ),
            None => Step::Answer(format!("no review task in: {listing}")),
        },
        [.., said] => Step::Answer(said.clone()),
    }
}

// --- clock-week -------------------------------------------------------------

/// Ways an answer may write `minutes`: `3:45`, `3h 45m`, `3 hours and 45
/// minutes`, `3.75 hours`, `225 minutes`, in any case.
fn spellings(minutes: u32) -> Vec<String> {
    let (h, m) = (minutes / 60, minutes % 60);
    let mut out = vec![format!("{h}:{m:02}"), format!("{minutes} min")];
    let hours = ["h", "hr", "hrs", "hour", "hours"];
    let mins = ["m", "min", "mins", "minute", "minutes"];
    for hw in hours {
        for gap in ["", " "] {
            if m == 0 {
                out.push(format!("{h}{gap}{hw}"));
                continue;
            }
            for join in [" ", " and ", ", ", ""] {
                for mw in mins {
                    out.push(format!("{h}{gap}{hw}{join}{m}{gap}{mw}"));
                }
            }
        }
    }
    let decimal = f64::from(minutes) / 60.0;
    for hw in ["h", " h", " hours", " hrs", "hrs", " hr"] {
        out.push(format!("{decimal}{hw}"));
        out.push(format!("{decimal:.2}{hw}"));
    }
    out
}

fn grade_clock(o: &Outcome) -> Result<(), String> {
    unchanged(o)?;
    let answer = o
        .answer
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut wrong = Vec::new();
    for (project, minutes) in CLOCKED {
        let named = answer.contains(&project.to_lowercase());
        let timed = spellings(*minutes).iter().any(|s| answer.contains(s));
        if !(named && timed) {
            wrong.push(format!("{project} {}:{:02}", minutes / 60, minutes % 60));
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!("the answer does not give {}", wrong.join(", ")))
    }
}

fn oracle_clock(results: &[String]) -> Step {
    match results {
        [] => Step::Call("org__clock", serde_json::json!({})),
        [report, ..] => Step::Answer(report.clone()),
    }
}

// --- notes ------------------------------------------------------------------

fn grade_notes(o: &Outcome) -> Result<(), String> {
    unchanged(o)?;
    if o.answer.trim().is_empty() {
        return Err("no summary".to_string());
    }
    let unread: Vec<&str> = MIGRATION_NOTES
        .iter()
        .filter(|(_, phrase)| !o.calls.iter().any(|c| c.ok && c.output.contains(phrase)))
        .map(|(id, _)| *id)
        .collect();
    if unread.is_empty() {
        Ok(())
    } else {
        Err(format!("did not read {unread:?}"))
    }
}

/// The files a notes listing names: `[ref id] in <path>`.
fn listed_files(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|l| l.split_once("] in ").map(|(_, p)| p.trim().to_string()))
        .collect()
}

fn oracle_notes(results: &[String]) -> Step {
    let Some(listing) = results.first() else {
        return Step::Call(
            "org__notes",
            serde_json::json!({"query": "database migration"}),
        );
    };
    let files = listed_files(listing);
    let read = results.len() - 1;
    match files.get(read) {
        Some(path) => Step::Call("lattice__read_file", serde_json::json!({"path": path})),
        None => Step::Answer(results[1..].join("\n\n")),
    }
}

/// A model that answers at once, from nothing: every grader must fail it, or
/// the grader is not checking anything.
pub fn lazy(_: &[String]) -> Step {
    Step::Answer("You have a few things on; nothing needs doing.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_recognised_however_written() {
        let s = spellings(225);
        for w in [
            "3:45",
            "3h 45m",
            "3 hours and 45 minutes",
            "3.75 hours",
            "225 min",
        ] {
            assert!(s.iter().any(|x| x == w), "{w}");
        }
        assert!(spellings(120).iter().any(|x| x == "2 hours"));
    }

    #[test]
    fn times_are_dropped_from_inactive_stamps_only() {
        assert_eq!(
            without_times("CLOSED: [2026-11-12 Thu 09:30] [[id:x][y]]"),
            "CLOSED: [2026-11-12 Thu] [[id:x][y]]"
        );
    }
}

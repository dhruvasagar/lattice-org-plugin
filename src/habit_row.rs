//! HB.5c — the consistency graph, rendered for one agenda row.
//!
//! Design: `org-habits.md` §4–§5, and `org-agenda.md` §5b for the seam this
//! feeds. The three pieces below it already exist: [`history`](crate::history)
//! reads the completions, [`habit_graph`](crate::habit_graph) decides each
//! day's state, and this turns the result into the one line the host hangs
//! under the row.
//!
//! ## Only `:STYLE: habit` gets a graph
//!
//! Org's rule (`org-is-habit-p`), and this is the one place org's `STYLE`
//! property matters. [`complete`](crate::complete) deliberately ignores it —
//! a repeater is what makes a task repeat, and requiring the style there would
//! silently break every repeating task that is not a habit. The graph is the
//! opposite case: it is a *display* for tasks you are tracking as habits, and
//! drawing one under every repeating TODO would add a row to agendas nobody
//! asked to change.
//!
//! ## Computed during the scan
//!
//! The guest is already reading this file and already walking this subtree, so
//! the graph costs a completion parse and some arithmetic on work that is
//! happening anyway — off the UI and actor threads, on a call that is already
//! budgeted. It is also correct to compute here: the graph changes when the
//! file changes or the day rolls over, and both re-run the scan.

use crate::habit_graph;
use crate::org_date::Date;
use crate::{headline, history, repeat};

/// The rendered graph: the line's text, and the runs of it that share a colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub text: String,
    /// `(start_byte, end_byte, element_name)`, coalesced — see
    /// [`render`](self::render).
    pub spans: Vec<(u32, u32, String)>,
}

/// The graph for the headline at `line`, or `None` when there is nothing to
/// draw.
///
/// `None` for: a headline that is not a habit, one whose planning line carries
/// no repeater, and one whose `SCHEDULED:` stamp will not parse. Each is an
/// ordinary answer — most rows are not habits — so they are indistinguishable
/// to the caller on purpose.
pub fn annotation_for(
    lines: &[&str],
    line: u32,
    today: Date,
    done_keywords: &[String],
    nerd_fonts: bool,
) -> Option<Annotation> {
    let read = |n: u32| lines.get(n as usize).map(|s| s.to_string());
    let end = headline::subtree_end(read, line, lines.len() as u32);
    let subtree: Vec<String> = (line..=end).filter_map(&read).collect();

    if !is_habit(&subtree) {
        return None;
    }
    let (scheduled, repeater) = scheduled_repeat(&subtree)?;
    let completions = history::completions(&subtree, done_keywords);
    let days = habit_graph::build(
        &habit_graph::Habit {
            completions: &completions,
            scheduled,
            repeater,
        },
        today,
    );
    Some(render(&days, nerd_fonts))
}

/// `:STYLE: habit` in the subtree's `:PROPERTIES:` drawer, org's own test.
///
/// Case-insensitive on both halves: org writes `:STYLE: habit`, but a file
/// edited by hand carries whatever the hand typed, and refusing to draw a graph
/// because the value was capitalised would be a bug the user could only find by
/// reading this function.
fn is_habit(subtree: &[String]) -> bool {
    let Some(start) = subtree
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(":PROPERTIES:"))
    else {
        return false;
    };
    for line in &subtree[start + 1..] {
        let t = line.trim();
        if t.eq_ignore_ascii_case(":END:") {
            break;
        }
        if let Some(rest) = t.strip_prefix(':') {
            if let Some((k, v)) = rest.split_once(':') {
                if k.eq_ignore_ascii_case("STYLE") {
                    return v.trim().eq_ignore_ascii_case("habit");
                }
            }
        }
    }
    false
}

/// The `SCHEDULED:` date and its repeater.
///
/// A habit's graph is drawn against the SCHEDULED stamp specifically — a
/// `DEADLINE:` repeater says when the task is late, not when it recurs, and
/// org's own parse errors out on a habit with no scheduled repeat.
fn scheduled_repeat(subtree: &[String]) -> Option<(Date, repeat::Repeater)> {
    let plan = subtree
        .iter()
        .find_map(|l| crate::planning::parse(l))?
        .get(crate::planning::Field::Scheduled)
        .cloned()?;
    // The stamp's date through the shared parser rather than a fourth
    // hand-rolled split — `timestamp` is what the schedule prompt and the
    // clock scan already read stamps with.
    let stamp = crate::timestamp::first_stamp(&plan)?;
    let date = Date {
        year: stamp.year,
        month: stamp.month,
        day: stamp.day,
    };
    let inner = plan.trim_matches(|c| c == '<' || c == '>' || c == '[' || c == ']');
    let repeater = inner.split_whitespace().find_map(repeat::parse)?;
    Some((date, repeater))
}

/// One cell per day, and the runs of equal colour over them.
///
/// **Spans are coalesced.** A 29-day window with one span per cell would cross
/// the boundary as 29 records to say what a handful say, and every one of them
/// resolves a theme element name host-side. Consecutive days sharing an element
/// are one run, which is also how the cells worker treats them at paint.
///
/// Byte offsets, not character offsets: the seam's contract, and the glyphs are
/// multi-byte in both palettes.
fn render(days: &[habit_graph::Day], nerd_fonts: bool) -> Annotation {
    let mut text = String::with_capacity(days.len() * 3);
    let mut spans: Vec<(u32, u32, String)> = Vec::new();
    for day in days {
        let start = text.len() as u32;
        text.push(habit_graph::glyph(day, nerd_fonts));
        let end = text.len() as u32;
        let element = day.state.element(day.muted).to_string();
        match spans.last_mut() {
            Some(last) if last.2 == element && last.1 == start => last.1 = end,
            _ => spans.push((start, end, element)),
        }
    }
    Annotation { text, spans }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> Date {
        Date {
            year: 2026,
            month: 9,
            day: 3,
        }
    }

    fn done() -> Vec<String> {
        vec!["DONE".to_string()]
    }

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    const HABIT: &str = "* NEXT Water the plants\n  \
        SCHEDULED: <2026-09-05 Sat .+2d/4d>\n  \
        :PROPERTIES:\n  \
        :STYLE: habit\n  \
        :END:\n  \
        :LOGBOOK:\n  \
        - State \"DONE\" from \"NEXT\" [2026-09-03 Thu 09:14]\n  \
        :END:\n";

    #[test]
    fn a_habit_gets_a_graph() {
        let l = lines(HABIT);
        let a = annotation_for(&l, 0, today(), &done(), false).expect("a habit draws a graph");
        assert_eq!(
            a.text.chars().count() as i64,
            habit_graph::PRECEDING_DAYS + habit_graph::FOLLOWING_DAYS + 1,
            "one cell per day of the window"
        );
        assert!(
            a.text.contains('✓'),
            "the day it was completed carries the done glyph: {}",
            a.text
        );
    }

    /// The gate. A repeating task that is not marked as a habit keeps the
    /// agenda it always had — no second row.
    #[test]
    fn a_repeating_task_without_the_style_gets_nothing() {
        let text = "* NEXT Pay rent\n  SCHEDULED: <2026-09-05 Sat .+1m>\n";
        assert!(annotation_for(&lines(text), 0, today(), &done(), false).is_none());
    }

    /// And a habit with no repeater has no cadence to draw against.
    #[test]
    fn a_habit_without_a_repeater_gets_nothing() {
        let text = "* NEXT Odd one\n  \
            SCHEDULED: <2026-09-05 Sat>\n  \
            :PROPERTIES:\n  :STYLE: habit\n  :END:\n";
        assert!(annotation_for(&lines(text), 0, today(), &done(), false).is_none());
    }

    /// A plain headline is the overwhelmingly common row, and it must cost the
    /// scan nothing beyond the drawer check.
    #[test]
    fn a_plain_headline_gets_nothing() {
        assert!(annotation_for(&lines("* TODO Ship it\n"), 0, today(), &done(), false).is_none());
    }

    /// `:STYLE:` is matched case-insensitively — a hand-edited file is not a
    /// broken one.
    #[test]
    fn the_style_value_is_matched_case_insensitively() {
        let text = "* NEXT X\n  \
            SCHEDULED: <2026-09-05 Sat .+2d/4d>\n  \
            :PROPERTIES:\n  :Style: Habit\n  :END:\n";
        assert!(annotation_for(&lines(text), 0, today(), &done(), false).is_some());
    }

    /// The spans coalesce. A window of 29 cells that emitted one span each
    /// would cross the boundary as 29 records saying what a handful say, and
    /// each one resolves a theme element host-side.
    #[test]
    fn equal_neighbours_become_one_span() {
        let l = lines(HABIT);
        let a = annotation_for(&l, 0, today(), &done(), false).expect("a graph");
        assert!(
            a.spans.len() < a.text.chars().count(),
            "runs must coalesce, got {} spans for {} cells",
            a.spans.len(),
            a.text.chars().count()
        );
        // Contiguous and complete: every byte of the text is covered exactly
        // once, or a cell would render in the renderer's default and look like
        // a hole in the bar.
        assert_eq!(a.spans.first().map(|s| s.0), Some(0));
        assert_eq!(a.spans.last().map(|s| s.1), Some(a.text.len() as u32));
        for pair in a.spans.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "no gap between runs");
        }
    }

    /// Every span names an element the mode registers, in the `habit.*`
    /// namespace the host auto-prefixes with `org.`.
    #[test]
    fn spans_name_registered_habit_elements() {
        let l = lines(HABIT);
        let a = annotation_for(&l, 0, today(), &done(), false).expect("a graph");
        for (_, _, slot) in &a.spans {
            assert!(slot.starts_with("habit."), "unexpected element name {slot}");
        }
    }

    /// Both palettes produce the same number of cells, so toggling
    /// `ui.nerd_fonts` cannot shift the bar's width.
    #[test]
    fn both_palettes_render_the_same_width() {
        let l = lines(HABIT);
        let plain = annotation_for(&l, 0, today(), &done(), false).expect("a graph");
        let nerd = annotation_for(&l, 0, today(), &done(), true).expect("a graph");
        assert_eq!(
            plain.text.chars().count(),
            nerd.text.chars().count(),
            "the icon-degradation rule: same cell count in both palettes"
        );
    }

    /// A run's byte offsets have to survive multi-byte glyphs — both palettes
    /// are multi-byte throughout, so an implementation counting characters
    /// would colour the wrong cells.
    #[test]
    fn span_offsets_are_bytes_not_characters() {
        let l = lines(HABIT);
        let a = annotation_for(&l, 0, today(), &done(), false).expect("a graph");
        assert!(
            a.text.len() > a.text.chars().count(),
            "the premise: the glyphs are multi-byte"
        );
        assert_eq!(
            a.spans.last().map(|s| s.1),
            Some(a.text.len() as u32),
            "the last run ends at the text's BYTE length"
        );
    }

    /// The nested-headline rule `history` enforces, seen from here: a child's
    /// completions are not the parent's, so a parent habit with an untouched
    /// child does not inherit the child's graph.
    #[test]
    fn a_childs_completions_do_not_reach_the_parent() {
        let text = "* NEXT Parent\n  \
            SCHEDULED: <2026-09-05 Sat .+2d/4d>\n  \
            :PROPERTIES:\n  :STYLE: habit\n  :END:\n\
            ** NEXT Child\n  \
            :LOGBOOK:\n  \
            - State \"DONE\" from \"NEXT\" [2026-09-03 Thu 09:14]\n  \
            :END:\n";
        let a = annotation_for(&lines(text), 0, today(), &done(), false).expect("a graph");
        assert!(
            !a.text.contains('✓'),
            "the child's completion must not mark the parent's graph: {}",
            a.text
        );
    }
}

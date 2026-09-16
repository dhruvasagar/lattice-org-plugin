//! OC.5a — resolving a capture target to the line the note is inserted at.
//!
//! `Target::FileHeadline` has parsed since OC.2 and been ignored ever since:
//! both submit paths called `Target::file()` and handed the host
//! `FileAnchor::End`, so a template that said "under Vocabulary" appended at the
//! bottom of the file instead. It looked configured and behaved as if it were
//! not, which is the worst of both — the menu row even printed the target.
//!
//! ## The insertion point is the subtree's end, not the headline's line
//!
//! Inserting immediately *after* the headline would put the new entry in front
//! of everything already filed under it, so a capture list reads
//! newest-to-oldest inside the subtree while the file around it reads
//! oldest-to-newest. Emacs files at the end of the subtree and so does refile
//! (`refile::targets_in` → `subtree_end(...) + 1`); this is the same
//! computation against a file read from disk rather than the open buffer, and
//! deliberately shares [`crate::headline::subtree_end`] so the two cannot drift.
//!
//! ## Matching a headline by name
//!
//! `refile::title_of` keeps the TODO keyword and the tags on purpose — its
//! consumer is a picker, and typing `TODO` there is useful. A *template* names
//! its target in a config file, so the opposite is true: a user writing
//! `headline = "Vocabulary"` must not have their capture silently start
//! appending the day someone adds `:drill:` to that headline. So the comparison
//! here strips both and compares what is left.
//!
//! ## An absent headline appends and says so
//!
//! Not "creates it", and not "refuses". Creating invents structure the user did
//! not ask for, in a file they may not have looked at in months. Refusing loses
//! a note they have already typed — the one outcome capture must never produce.
//! Appending keeps the note and moves it somewhere findable; the echo is what
//! tells them the target moved.

/// Where a capture should be written, resolved against the file's current text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insertion {
    /// Append at the end of the file — a plain `file` target, a file that does
    /// not exist yet, or a headline that could not be found.
    Append,
    /// Insert before this 0-based line: the line after the target subtree's
    /// last. Past-the-end is fine and the host clamps it (`file-anchor.line`).
    AtLine(u32),
}

/// Resolve `headline` against `text`, or [`Insertion::Append`] when it is absent.
///
/// Pure over the file's text so the interesting behaviour — which line, and what
/// happens when the headline is missing — is testable without a filesystem, a
/// capability grant or a running editor.
/// `#[cfg(test)]`: production resolves the outline at the call site, because
/// only there is it known whether `parse-file` answered. This keeps the text
/// path's own behaviour — which line, and what happens when the headline is
/// missing — testable without a filesystem, a capability grant or an editor.
#[cfg(test)]
pub fn resolve(text: &str, headline: &str) -> Insertion {
    let lines: Vec<&str> = text.lines().collect();
    let outline = crate::headline::outline_text(&lines);
    resolve_in(&lines, &outline, headline)
}

/// [`resolve`] over an outline someone else produced (OT.8).
///
/// The capture action calls this with `tree-sitter.parse-file`'s walk, so a
/// template naming `headline = "Vocabulary"` cannot match a `* Vocabulary` line
/// written as an example inside a `#+BEGIN_SRC` block — which would file the
/// note into the middle of a code block and report success.
///
/// Shares [`crate::headline::Entry`] with `refile::targets_from`, one level up
/// from the `subtree_end` sharing this module's header describes, and for the
/// same reason: the two insertion points must not drift.
/// Where an `entry` files under `entry` — the line after its whole subtree.
///
/// One named home for the rule, because this module's header commits to it not
/// drifting from `refile::targets_in`'s `subtree_end(...) + 1`. Production and
/// the test wrappers below both go through here rather than each adding one.
pub fn after_subtree(entry: Option<&crate::headline::Entry>) -> Insertion {
    match entry {
        Some(entry) => Insertion::AtLine(entry.end_line + 1),
        None => Insertion::Append,
    }
}

/// [`find_in`] as an insertion point.
#[cfg(test)]
pub fn resolve_in(lines: &[&str], outline: &[crate::headline::Entry], headline: &str) -> Insertion {
    after_subtree(find_in(lines, outline, headline))
}

/// CT.4: the ENTRY a headline target resolves to, not just its insertion line.
///
/// `table-line` needs the subtree's whole span to scope its table search, and
/// CT.6 / CT.7 will need the node itself to descend from. Splitting the find
/// from the insertion-point arithmetic keeps one answer to "which headline is
/// this" rather than two that can drift.
pub fn find_in<'a>(
    lines: &[&str],
    outline: &'a [crate::headline::Entry],
    headline: &str,
) -> Option<&'a crate::headline::Entry> {
    let wanted = normalise(headline);
    if wanted.is_empty() {
        return None;
    }
    // First match wins. A file with two headlines of the same name is already
    // ambiguous to a human reading it; picking the first is the answer that
    // matches how the user's eye finds it, and it is stable across edits below.
    outline.iter().find(|entry| {
        lines
            .get(entry.line as usize)
            .and_then(|l| l.get(entry.level..))
            .is_some_and(|rest| normalise(&strip_tags(rest)) == wanted)
    })
}

/// CT.3: resolve a full outline PATH — `file+olp`'s target.
///
/// ## Why a path, when a headline already works
///
/// `resolve_in` takes the first headline anywhere in the file with a matching
/// name. That is right for a unique name and wrong for a repeated one: a file
/// with `Inbox` under both `Work` and `Home` files every capture under
/// whichever comes first, silently. An outline path says which one.
///
/// It is also the machinery CT.7's `sub-olp` needs — descending from a resolved
/// node into a named child is this walk with a different starting scope — which
/// is why it lands here rather than waiting for the slice that needs it most.
///
/// ## Descent, not search
///
/// Each segment after the first is looked for **inside the previous segment's
/// subtree and strictly deeper than it**, so `["Work", "Inbox"]` cannot match a
/// top-level `Inbox` that happens to appear after `Work` ends. Within that
/// scope the first match wins, for the same reason `resolve_in` takes the first:
/// a file with two identical siblings is already ambiguous to the human reading
/// it.
///
/// A segment that is not found returns [`Insertion::Append`] — the whole point
/// of this module's "an absent headline appends and says so" rule, applied to
/// the first segment that breaks the chain rather than only to the last.
/// [`find_olp_in`] as an insertion point.
#[cfg(test)]
pub fn resolve_olp_in(
    lines: &[&str],
    outline: &[crate::headline::Entry],
    olp: &[String],
) -> Insertion {
    after_subtree(find_olp_in(lines, outline, olp))
}

/// CT.4: the ENTRY an outline path resolves to — [`find_in`]'s peer.
pub fn find_olp_in<'a>(
    lines: &[&str],
    outline: &'a [crate::headline::Entry],
    olp: &[String],
) -> Option<&'a crate::headline::Entry> {
    // The whole file, any level — the widest seed of the same descent.
    find_olp_within(lines, outline, (0, u32::MAX), 0, olp)
}

/// CT.7: [`find_olp_in`] starting from a SCOPE rather than the whole file.
///
/// The same descent, seeded differently — which is why CT.3 wrote the walk in
/// terms of `(lo, hi, min_level)` instead of hard-coding the file. A `sub-olp`
/// descends from the node a datetree resolved to; nothing about the walk
/// changes, only where it begins.
pub fn find_olp_within<'a>(
    lines: &[&str],
    outline: &'a [crate::headline::Entry],
    scope: (u32, u32),
    base_level: usize,
    olp: &[String],
) -> Option<&'a crate::headline::Entry> {
    if olp.is_empty() || olp.iter().all(|s| normalise(s).is_empty()) {
        return None;
    }
    let (mut lo, mut hi) = scope;
    let mut min_level: usize = base_level;
    let mut found: Option<&crate::headline::Entry> = None;

    for segment in olp {
        let wanted = normalise(segment);
        if wanted.is_empty() {
            return None;
        }
        let hit = outline.iter().find(|entry| {
            entry.line >= lo
                && entry.line <= hi
                && entry.level > min_level
                && lines
                    .get(entry.line as usize)
                    .and_then(|l| l.get(entry.level..))
                    .is_some_and(|rest| normalise(&strip_tags(rest)) == wanted)
        });
        let Some(entry) = hit else {
            return None;
        };
        // Descend: the next segment must live inside THIS subtree and below it.
        lo = entry.line + 1;
        hi = entry.end_line;
        min_level = entry.level;
        found = Some(entry);
    }

    found
}

/// CT.5: re-level a captured body so it becomes a CHILD of its target.
///
/// ## Why this is not optional
///
/// Lattice inserts a capture body verbatim at a line boundary. Emacs does not:
/// its `entry` type files the captured entry as a child of the target and
/// re-levels it to fit. The difference was invisible while targets were
/// top-level headlines, and the user init works around it in prose — its vocab
/// template is written with `**` instead of `*`, with a comment explaining
/// that a `*` heading "would close the `Vocabulary` subtree and land as its
/// sibling instead of inside it".
///
/// Against a datetree it stops being a wart and becomes corruption.
/// `org-datetree` puts the day node at LEVEL 3 (`* 2026` / `** 2026-09
/// September` / `*** 2026-09-16 Wednesday`), so a template whose first line is
/// `* How to use this` terminates the day node, the month and the year, and
/// lands as a sibling of `* 2026` — silently reorganising a file the user was
/// only adding a row to. That is why CT.5 lands before CT.6 rather than after.
///
/// ## The shift
///
/// The body's SHALLOWEST heading becomes a child of the target, and every
/// deeper heading shifts by the same delta, so the body's internal structure
/// survives. A body with no headings is returned untouched — there is nothing
/// to re-level, and prose must not acquire stars.
///
/// The shift is signed: a body already deeper than its target is raised, not
/// only lowered. A one-directional version would leave a `***` body under a
/// `*` headline two levels too deep, which reads as nesting the user did not
/// write.
///
/// Only headings move. A `*` inside a body line, a `#+BEGIN_SRC` block or a
/// bold marker is not a heading and is left alone — [`heading_level`] is what
/// decides, and it requires the stars to start the line and be followed by a
/// space.
pub fn relevel(body: &str, target_level: usize) -> String {
    let shallowest = body.lines().filter_map(heading_level).min();
    let Some(shallowest) = shallowest else {
        return body.to_string();
    };
    // The body's top heading should sit one level below the target.
    let wanted = target_level + 1;
    if shallowest == wanted {
        return body.to_string();
    }
    let delta = wanted as isize - shallowest as isize;

    let mut out = String::with_capacity(body.len() + 8);
    for (i, line) in body.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match heading_level(line) {
            Some(level) => {
                // `max(1)` because a heading cannot have zero stars; a body
                // raised past the top of the outline clamps rather than
                // becoming prose.
                let new_level = (level as isize + delta).max(1) as usize;
                out.push_str(&"*".repeat(new_level));
                out.push_str(&line[level..]);
            }
            None => out.push_str(line),
        }
    }
    if body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The star count of an org heading, or `None` for any other line.
///
/// Requires the stars to START the line and be followed by a space, which is
/// org's own rule. Without the trailing-space check a `**bold**` line at column
/// zero would be read as a level-2 heading and re-levelled, corrupting a body
/// that contained emphasis.
fn heading_level(line: &str) -> Option<usize> {
    let stars = line.chars().take_while(|c| *c == '*').count();
    if stars == 0 {
        return None;
    }
    match line[stars..].chars().next() {
        Some(' ') | Some('\t') => Some(stars),
        _ => None,
    }
}

/// CT.4: where a `table-line` row goes, and what has to be created first.
///
/// [`Insertion`] cannot express "make a table, then put the row in it", so
/// this carries both: the line to insert before, and any table scaffolding
/// that must precede the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInsertion {
    /// The 0-based line the text goes before.
    pub line: u32,
    /// Lines to write ahead of the row — `|   |` and `|---|` when the target
    /// had no table. Empty in the ordinary case.
    pub create: Vec<String>,
}

/// Where in the table a row lands, when the caller has a preference.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TablePlacement {
    /// After the last row. Org's default.
    #[default]
    End,
    /// The first data line after the first hline — org's `:prepend`.
    Prepend,
    /// Org's `:table-line-pos`, e.g. `"II-1"`: relative to the Nth hline.
    Pos(String),
}

/// CT.4: resolve a `table-line` target — org's `org-capture-place-table-line`.
///
/// ## Scope
///
/// A heading-bearing target scopes the search to that heading's BODY, stopping
/// at the next headline — org's rule, stated in `org-capture.el`'s own
/// docstring: "table-line will be inserted into the nearest table, if any,
/// searching from point to the end of current heading body". A whole-file
/// target takes the first table in the file.
///
/// `scope` is the half-open line range to search; the caller derives it from
/// whichever target shape it resolved, which is what lets CT.7 point this at a
/// datetree node's descendant rather than at a top-level headline.
///
/// ## An absent table is CREATED
///
/// Org inserts `|   |` / `|---|` and uses that. Doing nothing because the
/// section has no table yet would lose the row the user just typed — the one
/// outcome capture must never produce — and refusing would lose it just as
/// surely.
///
/// ## No alignment
///
/// Org realigns the table after inserting; this does not, and will not. Column
/// alignment is table EDITING, and doing it here would rewrite lines the user
/// did not capture. A ragged row is correct org and reads fine.
pub fn resolve_table_in(
    lines: &[&str],
    scope: (u32, u32),
    placement: &TablePlacement,
) -> TableInsertion {
    let (lo, hi) = scope;
    let hi = hi.min(lines.len() as u32);

    // The first run of table lines inside the scope.
    let mut start: Option<u32> = None;
    let mut end: u32 = lo;
    for i in lo..hi {
        let is_row = lines
            .get(i as usize)
            .is_some_and(|l| l.trim_start().starts_with('|'));
        match (is_row, start) {
            (true, None) => {
                start = Some(i);
                end = i + 1;
            }
            (true, Some(_)) => end = i + 1,
            // A blank line inside a table ends it; so does any non-row line.
            (false, Some(_)) => break,
            (false, None) => {}
        }
    }

    let Some(start) = start else {
        // No table in scope: create one at the end of the scope, so the row
        // lands under whatever prose the section already has rather than above
        // it.
        return TableInsertion {
            line: hi,
            create: vec!["|   |".to_string(), "|---|".to_string()],
        };
    };

    let line = match placement {
        TablePlacement::End => end,
        TablePlacement::Prepend => first_data_line_after_hline(lines, start, end).unwrap_or(end),
        TablePlacement::Pos(spec) => table_line_pos(lines, start, end, spec).unwrap_or(end),
    };
    TableInsertion {
        line,
        create: Vec::new(),
    }
}

/// Org's `:prepend` — the first data line after the first hline, so a row goes
/// at the TOP of the data rather than above the header.
fn first_data_line_after_hline(lines: &[&str], start: u32, end: u32) -> Option<u32> {
    let mut seen_hline = false;
    for i in start..end {
        let l = lines.get(i as usize)?.trim_start();
        if is_hline(l) {
            seen_hline = true;
        } else if seen_hline {
            return Some(i);
        }
    }
    None
}

/// Org's `:table-line-pos`, e.g. `"II-1"` or `"I+2"`.
///
/// The `I`s count which hline group, the signed number is the offset from it.
/// Org's own regex is `\(I+\)\([-+][0-9]+\)`; the semantics are kept rather
/// than re-derived, so a spec ported from an emacs config means the same thing.
fn table_line_pos(lines: &[&str], start: u32, end: u32, spec: &str) -> Option<u32> {
    let spec = spec.trim();
    let bars = spec.chars().take_while(|c| *c == 'I').count();
    if bars == 0 {
        return None;
    }
    let rest = &spec[bars..];
    let (sign, digits) = rest.split_at(rest.find(|c: char| c.is_ascii_digit())?);
    let delta: i64 = digits.parse().ok()?;
    let delta = if sign.starts_with('-') { -delta } else { delta };

    // The `bars`-th hline, 1-based.
    let hline = (start..end)
        .filter(|i| {
            lines
                .get(*i as usize)
                .is_some_and(|l| is_hline(l.trim_start()))
        })
        .nth(bars - 1)?;

    let target = hline as i64 + delta;
    // Clamped into the table: a spec pointing outside it is a config error the
    // caller reports, not a write outside the table.
    if target < start as i64 || target > end as i64 {
        return None;
    }
    Some(target as u32)
}

/// `|---+---|` and friends — a table rule rather than a data row.
///
/// Org's own rule, and it is simpler than it looks: `org-table-hline-regexp` is
/// `"^[ \t]*|-"`, with the dataline peer `"^[ \t]*|[^-]"`. The character
/// immediately after the bar decides it, and nothing else is examined.
///
/// A first attempt here checked "every character after the bar is one of
/// `-+| `, and there is at least one `-`", which READS as more careful and is
/// wrong: `| - |` is a data cell containing a dash, and that spelling classed
/// it as a rule. The consequence would have been silent — `prepend` landing
/// above the header, and `End` off by one on any table whose last line is a
/// rule. The test caught it; org's actual regexp is what fixed it.
fn is_hline(line: &str) -> bool {
    line.trim_start().starts_with("|-")
}

/// CT.4: the row text org would insert.
///
/// Trimmed, and prefixed with `"| "` only when it is not already a row — org's
/// rule, so a template written as `| %^{a} | %^{b} |` is used as-is while a
/// bare `%^{a}` still becomes a cell.
pub fn table_row_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with('|') {
        trimmed.to_string()
    } else {
        format!("| {trimmed}")
    }
}

/// A headline's text with its TODO keyword and tags removed, lowercased and
/// whitespace-collapsed.
///
/// Case- and spacing-insensitive because this compares a hand-written config
/// string against a hand-written file, and `"vocabulary"` vs `"Vocabulary"` is
/// not a distinction a user means to make when they disagree with themselves
/// across two files.
fn normalise(s: &str) -> String {
    strip_todo(s.trim())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Drop a leading TODO-ish keyword — a leading all-caps word.
///
/// Deliberately shape-based rather than read from `org.todo-keywords`: the
/// keyword set is per-file (`#+TODO:`) and per-user, and a capture target that
/// resolved differently depending on an unrelated option would be a very
/// confusing thing to debug. An all-caps first word is what every org TODO
/// keyword looks like, and a headline whose first word is genuinely an
/// all-caps ordinary word can be matched by writing it out in full.
fn strip_todo(s: &str) -> &str {
    let Some((first, rest)) = s.split_once(char::is_whitespace) else {
        return s;
    };
    let keywordish = !first.is_empty()
        && first
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '-' || c == '_');
    if keywordish {
        rest.trim_start()
    } else {
        s
    }
}

/// Drop a trailing `:tag:tag:` cluster.
fn strip_tags(s: &str) -> String {
    let trimmed = s.trim_end();
    let Some(open) = trimmed.rfind(" :") else {
        return trimmed.to_string();
    };
    let tail = &trimmed[open + 1..];
    let is_tags = tail.len() >= 2
        && tail.starts_with(':')
        && tail.ends_with(':')
        && tail[1..tail.len() - 1].split(':').all(|t| {
            !t.is_empty()
                && t.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '@')
        });
    if is_tags {
        trimmed[..open].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OT.8's twin: what the TEXT outline answers for a headline that exists
    /// only as an example inside a `#+BEGIN_SRC` block.
    ///
    /// Not "it files into the block" — it does not. `subtree_end` stops at the
    /// next real headline, so the note lands just after `#+END_SRC`, attributed
    /// to a heading that is not there. Wrong in a quieter way than filing into
    /// the block would be, and quieter is worse: the note is somewhere the user
    /// has no reason to look, and capture reported success.
    ///
    /// The tree answers `Append` for the same input, which is OC.5a's
    /// missing-target contract and comes with the warning that says so.
    /// `a_capture_target_inside_a_block_is_not_a_target` in
    /// `tests/org_structure.rs` is the other half.
    #[test]
    fn the_text_outline_matches_a_headline_written_inside_a_block() {
        let text =
            "* Inbox\n#+BEGIN_SRC org\n* Vocabulary\nexample content\n#+END_SRC\n* Later\ntail\n";
        assert_eq!(
            resolve(text, "Vocabulary"),
            Insertion::AtLine(5),
            "the text scan takes the block's example as a real headline and \
             files just past the block's end"
        );
    }

    const FILE: &str = "\
#+TITLE: Notes
* Inbox
captured stuff
* Vocabulary
** French
un mot
** German
ein Wort
* Archive
old things
";

    /// The headline the template names decides the line, and the note lands
    /// AFTER the whole subtree — not in front of its children.
    #[test]
    fn a_headline_target_inserts_after_its_whole_subtree() {
        // `* Vocabulary` is line 3; its subtree swallows both `**` children and
        // runs through `ein Wort` (line 7), so the insertion point is line 8 —
        // the `* Archive` line, which the new entry is inserted in front of.
        assert_eq!(resolve(FILE, "Vocabulary"), Insertion::AtLine(8));
    }

    /// A nested headline is targetable, and its subtree ends at its SIBLING,
    /// not at the next top-level headline.
    #[test]
    fn a_nested_headline_stops_at_its_sibling() {
        // `** French` is line 4, its subtree is `un mot` (5), so 6 — `** German`.
        assert_eq!(resolve(FILE, "French"), Insertion::AtLine(6));
    }

    /// The last headline in the file targets past the end; the host clamps it.
    #[test]
    fn the_last_headline_targets_past_the_end() {
        assert_eq!(resolve(FILE, "Archive"), Insertion::AtLine(10));
    }

    /// **The failure mode this slice exists to make honest.** A headline that
    /// is not there appends — the note survives — rather than being refused or
    /// causing the headline to be invented.
    #[test]
    fn a_missing_headline_appends_rather_than_losing_the_note() {
        assert_eq!(resolve(FILE, "Nowhere"), Insertion::Append);
    }

    /// A file that does not exist yet reads as empty; capture into it appends
    /// and thereby creates it.
    #[test]
    fn an_empty_file_appends() {
        assert_eq!(resolve("", "Vocabulary"), Insertion::Append);
    }

    /// Tags must not break a target. A user writes `headline = "Vocabulary"`
    /// once; adding `:drill:` to that headline months later must not silently
    /// send every future capture to the bottom of the file.
    #[test]
    fn tags_on_the_headline_do_not_break_the_match() {
        let text = "* Vocabulary :drill:fc:\nun mot\n* Next\n";
        assert_eq!(resolve(text, "Vocabulary"), Insertion::AtLine(2));
    }

    /// Same for a TODO keyword appearing on the target headline.
    #[test]
    fn a_todo_keyword_on_the_headline_does_not_break_the_match() {
        let text = "* TODO Vocabulary\nun mot\n* Next\n";
        assert_eq!(resolve(text, "Vocabulary"), Insertion::AtLine(2));
    }

    /// Case and inner spacing are not distinctions a user means to make when
    /// their config and their file disagree.
    #[test]
    fn matching_ignores_case_and_extra_spacing() {
        let text = "* My   Notes\nbody\n* Next\n";
        assert_eq!(resolve(text, "my notes"), Insertion::AtLine(2));
    }

    /// Body text that merely mentions the name is not a headline.
    #[test]
    fn a_line_that_is_not_a_headline_never_matches() {
        let text = "* Real\nVocabulary is mentioned here\n";
        assert_eq!(resolve(text, "Vocabulary"), Insertion::Append);
    }

    /// A blank headline in the template is the plain-file target. The parser
    /// already degrades this shape, so this is belt-and-braces at the second
    /// place that could get it wrong.
    #[test]
    fn a_blank_headline_appends() {
        assert_eq!(resolve(FILE, "   "), Insertion::Append);
    }

    /// Two headlines with the same name: the first wins, and it is the one the
    /// user's eye finds first too.
    #[test]
    fn a_duplicate_headline_resolves_to_the_first() {
        let text = "* Dup\na\n* Other\nb\n* Dup\nc\n";
        assert_eq!(resolve(text, "Dup"), Insertion::AtLine(2));
    }

    /// A trailing `:` that is not a tag cluster is part of the title.
    #[test]
    fn a_colon_that_is_not_a_tag_cluster_stays_in_the_title() {
        let text = "* See also: refs\nbody\n* Next\n";
        assert_eq!(resolve(text, "See also: refs"), Insertion::AtLine(2));
    }

    /// CT.3 helper: resolve an outline path over text, the way [`resolve`] does
    /// for a single headline.
    fn resolve_olp(text: &str, olp: &[&str]) -> Insertion {
        let lines: Vec<&str> = text.lines().collect();
        let outline = crate::headline::outline_text(&lines);
        let olp: Vec<String> = olp.iter().map(|s| s.to_string()).collect();
        resolve_olp_in(&lines, &outline, &olp)
    }

    /// The case `file+olp` exists for, and the one `file+headline` gets wrong.
    ///
    /// Two `Inbox` headlines, under `Work` and under `Home`. A bare headline
    /// target takes the first and files everything there silently; the path
    /// picks the one that was asked for.
    #[test]
    fn an_outline_path_picks_the_right_one_of_two_same_named_headlines() {
        let text = "\
* Work
** Inbox
work note
** Done
* Home
** Inbox
home note
* End
";
        // `Work/Inbox` is lines 1..2, so the insert lands on line 3 (`** Done`).
        assert_eq!(resolve_olp(text, &["Work", "Inbox"]), Insertion::AtLine(3));
        // `Home/Inbox` is lines 5..6, so the insert lands on line 7 (`* End`).
        assert_eq!(resolve_olp(text, &["Home", "Inbox"]), Insertion::AtLine(7));

        // And this is what the bare headline does with the same file — it can
        // only ever reach the first. Not a bug in `resolve_in`; the reason the
        // path resolver exists.
        assert_eq!(resolve(text, "Inbox"), Insertion::AtLine(3));
    }

    /// A segment must be found INSIDE the previous segment's subtree, not
    /// merely after it. `Work/Later` must not match the top-level `Later` that
    /// follows `Work` — that is the difference between descent and search, and
    /// a resolver that searched would pass every other test in this file.
    #[test]
    fn a_path_descends_rather_than_searching_forward() {
        let text = "\
* Work
** Notes
* Later
body
";
        assert_eq!(resolve_olp(text, &["Work", "Later"]), Insertion::Append);
        // The same name IS reachable as a one-segment path, so the failure
        // above is about scope and not about the name.
        assert_eq!(resolve_olp(text, &["Later"]), Insertion::AtLine(4));
    }

    /// And strictly deeper: a sibling at the same level is not a child.
    #[test]
    fn a_path_segment_must_be_deeper_than_its_parent() {
        let text = "\
* Work
* Inbox
body
";
        assert_eq!(resolve_olp(text, &["Work", "Inbox"]), Insertion::Append);
    }

    /// A path can be deeper than two, and lands after the LAST segment's
    /// subtree rather than after any ancestor's.
    #[test]
    fn a_three_segment_path_lands_after_its_own_subtree() {
        let text = "\
* A
** B
*** C
c body
*** D
** E
* F
";
        // `A/B/C` is lines 2..3, so the insert lands on line 4 (`*** D`).
        assert_eq!(resolve_olp(text, &["A", "B", "C"]), Insertion::AtLine(4));
    }

    /// A broken chain appends, like an absent headline — the note is kept and
    /// the caller says where it went.
    #[test]
    fn a_missing_segment_appends() {
        let text = "* Work\n** Notes\n";
        assert_eq!(resolve_olp(text, &["Work", "Nope"]), Insertion::Append);
        assert_eq!(resolve_olp(text, &["Nope", "Notes"]), Insertion::Append);
    }

    /// An empty or all-blank path has nothing to resolve. It cannot arrive from
    /// a config (`from_declared` filters blanks and refuses an empty `olp`),
    /// so this pins the resolver's own contract rather than a reachable state.
    #[test]
    fn an_empty_path_appends() {
        let text = "* Work\n";
        assert_eq!(resolve_olp(text, &[]), Insertion::Append);
        assert_eq!(resolve_olp(text, &["  "]), Insertion::Append);
    }

    /// CT.5: a `*` body under a level-1 headline becomes `**` — a child, not a
    /// sibling. Without this the body TERMINATES the target's subtree.
    #[test]
    fn a_body_becomes_a_child_of_its_target() {
        assert_eq!(relevel("* note\nbody\n", 1), "** note\nbody\n");
    }

    /// Relative structure survives: every heading shifts by the same delta.
    ///
    /// Target level 2 means the body's shallowest heading becomes level 3, so
    /// the delta is +2 and `*** c` goes to `***** c`. The gaps are what this
    /// asserts: `c` stays two levels below `a`, `b` one.
    #[test]
    fn the_bodys_internal_nesting_is_preserved() {
        assert_eq!(
            relevel("* a\n*** c\n** b\n", 2),
            "*** a\n***** c\n**** b\n",
            "each shifted by +2, so `c` stays two below `a` and `b` one"
        );
    }

    /// The shift is SIGNED — a body already too deep is raised. A
    /// one-directional version would leave a `***` body under a `*` headline
    /// two levels too deep, which reads as nesting the user did not write.
    #[test]
    fn a_body_deeper_than_its_target_is_raised() {
        assert_eq!(relevel("*** deep\ntext\n", 1), "** deep\ntext\n");
    }

    /// Already correct is left exactly alone — no rewrite, no churn.
    #[test]
    fn a_body_at_the_right_level_is_untouched() {
        let body = "** note\nbody\n";
        assert_eq!(relevel(body, 1), body);
    }

    /// Prose must not acquire stars. A body with no headings is returned
    /// verbatim — this is the common capture, one line of text.
    #[test]
    fn a_body_with_no_headings_is_untouched() {
        let body = "just a note\nwith two lines\n";
        assert_eq!(relevel(body, 3), body);
        assert_eq!(relevel("", 1), "");
    }

    /// Only HEADINGS move. `**bold**` at column zero is not a heading — org
    /// requires a space after the stars — and re-levelling it would corrupt a
    /// body containing emphasis.
    #[test]
    fn emphasis_is_not_a_heading() {
        assert_eq!(heading_level("**bold** text"), None);
        assert_eq!(heading_level("* heading"), Some(1));
        assert_eq!(heading_level("*"), None, "bare stars are not a heading");
        assert_eq!(heading_level("no stars"), None);
        assert_eq!(
            relevel("* h\n**bold** line\n", 2),
            "*** h\n**bold** line\n",
            "the heading shifted, the emphasis did not"
        );
    }

    /// The datetree case CT.6 depends on: a day node at level 3 takes a
    /// `*`-rooted template body to `****`, so it lands INSIDE the day rather
    /// than terminating the year.
    #[test]
    fn a_template_body_fits_under_a_level_three_datetree_node() {
        let body = "* How to use this\n- stop\n* Daily Overview\n| a |\n";
        assert_eq!(
            relevel(body, 3),
            "**** How to use this\n- stop\n**** Daily Overview\n| a |\n",
            "both sections become children of the day node"
        );
    }

    /// A trailing newline is preserved, and its absence is too — the body is
    /// spliced into a file, so gaining or losing one shifts everything after.
    #[test]
    fn the_trailing_newline_is_preserved_either_way() {
        assert_eq!(relevel("* a\n", 1), "** a\n");
        assert_eq!(relevel("* a", 1), "** a");
    }

    /// CT.4 helper: resolve a table insertion over the whole of `text`.
    fn table(text: &str, placement: TablePlacement) -> TableInsertion {
        let lines: Vec<&str> = text.lines().collect();
        let n = lines.len() as u32;
        resolve_table_in(&lines, (0, n), &placement)
    }

    /// Org's default: after the last row.
    #[test]
    fn a_row_lands_after_the_last_row_by_default() {
        let text = "| a | b |\n|---+---|\n| 1 | 2 |\n";
        assert_eq!(
            table(text, TablePlacement::End),
            TableInsertion {
                line: 3,
                create: Vec::new()
            }
        );
    }

    /// Org's `:prepend`: the first data line AFTER the first hline, so a row
    /// goes at the top of the data rather than above the header.
    #[test]
    fn prepend_lands_after_the_first_hline_not_above_the_header() {
        let text = "| a | b |\n|---+---|\n| 1 | 2 |\n| 3 | 4 |\n";
        assert_eq!(
            table(text, TablePlacement::Prepend),
            TableInsertion {
                line: 2,
                create: Vec::new()
            },
            "above `| 1 | 2 |`, below the rule — NOT line 0, which would put \
             the row above the header"
        );
    }

    /// Org's `:table-line-pos`. `II-1` is the second hline group, one line up.
    #[test]
    fn a_table_line_pos_counts_hline_groups() {
        // hlines at 1 and 4.
        let text = "| h |\n|---|\n| 1 |\n| 2 |\n|---|\n| 3 |\n";
        assert_eq!(
            table(text, TablePlacement::Pos("II-1".to_string())).line,
            3,
            "one line above the second hline"
        );
        assert_eq!(
            table(text, TablePlacement::Pos("I+1".to_string())).line,
            2,
            "one line below the first hline"
        );
    }

    /// A spec naming an hline group that does not exist falls back to the end
    /// rather than writing outside the table.
    #[test]
    fn an_out_of_range_table_line_pos_falls_back_to_the_end() {
        let text = "| h |\n|---|\n| 1 |\n";
        assert_eq!(
            table(text, TablePlacement::Pos("III-1".to_string())).line,
            3
        );
        assert_eq!(
            table(text, TablePlacement::Pos("nonsense".to_string())).line,
            3
        );
    }

    /// An absent table is CREATED. Doing nothing would lose the row the user
    /// just typed, which is the outcome capture must never produce.
    #[test]
    fn an_absent_table_is_created() {
        let text = "some prose\nmore prose\n";
        let got = table(text, TablePlacement::End);
        assert_eq!(got.line, 2, "at the end of the scope, under the prose");
        assert_eq!(got.create, vec!["|   |".to_string(), "|---|".to_string()]);
    }

    /// The table search stops at the first non-row line, so a second table
    /// later in the scope is not mistaken for a continuation of the first.
    #[test]
    fn the_first_table_in_scope_wins() {
        let text = "| a |\n|---|\n| 1 |\nprose\n| b |\n| 2 |\n";
        assert_eq!(
            table(text, TablePlacement::End).line,
            3,
            "the end of the FIRST table, not of the second"
        );
    }

    /// A row already written as a row is used as-is; a bare value becomes a
    /// cell. Org's rule, so `| %^{a} | %^{b} |` is not double-prefixed.
    #[test]
    fn a_row_is_prefixed_only_when_it_is_not_already_one() {
        assert_eq!(table_row_text("| a | b |"), "| a | b |");
        assert_eq!(table_row_text("  | a |  "), "| a |");
        assert_eq!(table_row_text("bare"), "| bare");
    }

    /// An hline is a rule, not data — `|---+---|` must not be mistaken for a
    /// row, or `prepend` would land above the header and `End` would be off by
    /// one on a table ending in a rule.
    #[test]
    fn an_hline_is_not_a_data_row() {
        assert!(is_hline("|---|"));
        assert!(is_hline("|---+---|"));
        assert!(is_hline("|--- | ---|"));
        assert!(!is_hline("| a | b |"));
        assert!(
            !is_hline("| - |"),
            "a single dash is a CELL containing a dash"
        );
    }

    /// Segments normalise like a headline does — TODO keyword, tags, case and
    /// spacing — because both sides are hand-written and a user does not mean
    /// `Work` and `work` to differ.
    #[test]
    fn path_segments_normalise_like_headlines() {
        let text = "* TODO Work   Stuff :proj:\n** Inbox\n";
        assert_eq!(
            resolve_olp(text, &["work stuff", "INBOX"]),
            Insertion::AtLine(2)
        );
    }
}

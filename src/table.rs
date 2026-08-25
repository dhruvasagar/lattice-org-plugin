//! OM.12 — org tables: finding them, and aligning their columns.
//!
//! ```org
//! | Name  | Qty |
//! |-------+-----|
//! | bread |   1 |
//! ```
//!
//! ## Alignment is a whole-table operation
//!
//! A column's width is the widest cell in it, so touching one cell can change
//! every row. That is why this works on the contiguous block of table lines
//! rather than a line at a time, and why the result lands as ONE edit — a
//! half-aligned table is a worse state than either end.
//!
//! ## Width is measured in characters, not bytes
//!
//! A table of names with accents lines up only if `é` counts as one column.
//! Full width-aware measurement (CJK, emoji) belongs to the renderer; this
//! uses `chars().count()`, which is right for the Latin-plus-accents case that
//! covers most org tables and is honestly wrong for CJK — recorded rather than
//! silently approximated.

/// A row's cells, or a separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// `| a | b |` — the trimmed cell texts.
    Cells(Vec<String>),
    /// `|---+---|` — a rule between sections.
    Separator,
}

/// True when `line` is part of a table.
pub fn is_table_line(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// Parse a table line into cells or a separator.
pub fn parse_row(line: &str) -> Option<Row> {
    let t = line.trim();
    if !t.starts_with('|') {
        return None;
    }
    let inner = t.trim_start_matches('|').trim_end_matches('|');
    // A separator is dashes and pluses only — `|---+---|`.
    if !inner.is_empty()
        && inner
            .chars()
            .all(|c| c == '-' || c == '+' || c == '|' || c.is_whitespace())
        && inner.contains('-')
    {
        return Some(Row::Separator);
    }
    Some(Row::Cells(
        inner.split('|').map(|c| c.trim().to_string()).collect(),
    ))
}

/// The contiguous run of table lines containing `at`, as `(first, last)`.
pub fn table_bounds(
    line: impl Fn(u32) -> Option<String>,
    at: u32,
    line_count: u32,
) -> Option<(u32, u32)> {
    if !line(at).is_some_and(|t| is_table_line(&t)) {
        return None;
    }
    let mut first = at;
    while first > 0 && line(first - 1).is_some_and(|t| is_table_line(&t)) {
        first -= 1;
    }
    let mut last = at;
    while last + 1 < line_count && line(last + 1).is_some_and(|t| is_table_line(&t)) {
        last += 1;
    }
    Some((first, last))
}

/// Align a table's rows, returning one rendered line per input row.
///
/// Every row is padded to the same column widths, and a separator is redrawn
/// to match. Ragged rows are fine: a row with fewer cells than its widest
/// sibling is padded out, because refusing to align a table mid-edit — the
/// exact moment a row IS ragged — would make the key useless when it is most
/// wanted.
pub fn align(rows: &[Row]) -> Vec<String> {
    let columns = rows
        .iter()
        .filter_map(|r| match r {
            Row::Cells(c) => Some(c.len()),
            Row::Separator => None,
        })
        .max()
        .unwrap_or(0);
    if columns == 0 {
        return rows.iter().map(|_| "|".to_string()).collect();
    }
    // Floor of 1: a column whose cells are all empty still has to be visible
    // and wide enough to put the caret in — a zero-width column renders as
    // `||` and there is nowhere to type.
    let mut widths = vec![1usize; columns];
    for r in rows {
        if let Row::Cells(cells) = r {
            for (i, c) in cells.iter().enumerate() {
                widths[i] = widths[i].max(c.chars().count());
            }
        }
    }
    rows.iter()
        .map(|r| match r {
            Row::Separator => {
                let mut s = String::from("|");
                for (i, w) in widths.iter().enumerate() {
                    if i > 0 {
                        s.push('+');
                    }
                    s.push_str(&"-".repeat(w + 2));
                }
                s.push('|');
                s
            }
            Row::Cells(cells) => {
                let mut s = String::from("|");
                for (i, w) in widths.iter().enumerate() {
                    let cell = cells.get(i).map(String::as_str).unwrap_or("");
                    let pad = w - cell.chars().count();
                    s.push(' ');
                    s.push_str(cell);
                    s.push_str(&" ".repeat(pad));
                    s.push(' ');
                    s.push('|');
                }
                s
            }
        })
        .collect()
}

/// Which cell (0-based) `byte` falls in, on a table row.
pub fn cell_at(line: &str, byte: usize) -> usize {
    let upto = &line[..byte.min(line.len())];
    // Cells are separated by `|`; the leading one opens the row, so the count
    // of pipes before the cursor minus that opener is the index.
    upto.matches('|').count().saturating_sub(1)
}

/// Byte offset where cell `index`'s text starts, on an aligned row.
///
/// Used to place the caret after `<Tab>`. Returns the end of the line when
/// the index is past the last cell, so a `<Tab>` off the end parks sensibly
/// rather than at column zero.
pub fn cell_start(line: &str, index: usize) -> usize {
    let mut seen = 0usize;
    for (i, ch) in line.char_indices() {
        if ch == '|' {
            // The pipe that OPENS cell `index` is the index-th one — the
            // leading `|` opens cell 0. Skip it and the single padding space.
            if seen == index {
                return (i + 2).min(line.len());
            }
            seen += 1;
        }
    }
    line.len()
}

// ── OM.13: moving and inserting rows and columns ──

/// Swap rows `a` and `b`. Out-of-range indices leave the table untouched.
pub fn swap_rows(rows: &mut [Row], a: usize, b: usize) -> bool {
    if a == b || a >= rows.len() || b >= rows.len() {
        return false;
    }
    // Refuse to move a separator: a rule marks a section boundary, and
    // dragging it through the body would silently re-section the table.
    if matches!(rows[a], Row::Separator) || matches!(rows[b], Row::Separator) {
        return false;
    }
    rows.swap(a, b);
    true
}

/// Swap column `a` with `b` across every row.
pub fn swap_columns(rows: &mut [Row], a: usize, b: usize) -> bool {
    if a == b {
        return false;
    }
    let width = column_count(rows);
    if a >= width || b >= width {
        return false;
    }
    for row in rows.iter_mut() {
        if let Row::Cells(cells) = row {
            // Pad first: a ragged row would otherwise lose the swap silently,
            // leaving one row's columns transposed against the rest.
            while cells.len() <= a.max(b) {
                cells.push(String::new());
            }
            cells.swap(a, b);
        }
    }
    true
}

/// Insert an empty row below `at`.
pub fn insert_row(rows: &mut Vec<Row>, at: usize) {
    let width = column_count(rows).max(1);
    let index = (at + 1).min(rows.len());
    rows.insert(index, Row::Cells(vec![String::new(); width]));
}

/// Insert an empty column after `at` in every row.
pub fn insert_column(rows: &mut [Row], at: usize) {
    for row in rows.iter_mut() {
        if let Row::Cells(cells) = row {
            let index = (at + 1).min(cells.len());
            cells.insert(index, String::new());
        }
    }
}

/// Delete row `at`, unless it is the table's only content row — a table with
/// no rows is not a table, and the key would silently destroy it.
pub fn delete_row(rows: &mut Vec<Row>, at: usize) -> bool {
    if at >= rows.len() {
        return false;
    }
    let content = rows.iter().filter(|r| matches!(r, Row::Cells(_))).count();
    if content <= 1 && matches!(rows[at], Row::Cells(_)) {
        return false;
    }
    rows.remove(at);
    true
}

/// Delete column `at`, unless it is the last one.
pub fn delete_column(rows: &mut [Row], at: usize) -> bool {
    if column_count(rows) <= 1 {
        return false;
    }
    for row in rows.iter_mut() {
        if let Row::Cells(cells) = row {
            if at < cells.len() {
                cells.remove(at);
            }
        }
    }
    true
}

/// The widest row's cell count.
pub fn column_count(rows: &[Row]) -> usize {
    rows.iter()
        .filter_map(|r| match r {
            Row::Cells(c) => Some(c.len()),
            Row::Separator => None,
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_table_lines_and_separators() {
        assert!(is_table_line("| a | b |"));
        assert!(is_table_line("  |---+---|"));
        assert!(!is_table_line("not a table"));
        assert_eq!(parse_row("|---+---|"), Some(Row::Separator));
        assert_eq!(
            parse_row("| a | b |"),
            Some(Row::Cells(vec!["a".into(), "b".into()]))
        );
    }

    /// A row of empty cells is NOT a separator — `| | |` is a blank row, and
    /// redrawing it as dashes would destroy content the user just made room
    /// for.
    #[test]
    fn an_empty_row_is_not_a_separator() {
        assert_eq!(
            parse_row("|  |  |"),
            Some(Row::Cells(vec!["".into(), "".into()]))
        );
    }

    #[test]
    fn aligns_columns_to_their_widest_cell() {
        let rows = vec![
            Row::Cells(vec!["Name".into(), "Qty".into()]),
            Row::Separator,
            Row::Cells(vec!["bread".into(), "1".into()]),
        ];
        assert_eq!(
            align(&rows),
            vec![
                "| Name  | Qty |".to_string(),
                "|-------+-----|".to_string(),
                "| bread | 1   |".to_string(),
            ]
        );
    }

    /// A ragged row is exactly the state a table is in mid-edit, so refusing
    /// to align one would make the key useless when it is most wanted.
    #[test]
    fn a_ragged_row_is_padded_rather_than_refused() {
        let rows = vec![
            Row::Cells(vec!["a".into(), "b".into(), "c".into()]),
            Row::Cells(vec!["x".into()]),
        ];
        assert_eq!(
            align(&rows),
            vec!["| a | b | c |".to_string(), "| x |   |   |".to_string()]
        );
    }

    /// Accented Latin lines up only if `é` counts as one column.
    #[test]
    fn width_is_measured_in_characters_not_bytes() {
        let rows = vec![
            Row::Cells(vec!["café".into()]),
            Row::Cells(vec!["ab".into()]),
        ];
        assert_eq!(
            align(&rows),
            vec!["| café |".to_string(), "| ab   |".to_string()]
        );
    }

    fn buf(text: &str) -> (impl Fn(u32) -> Option<String> + use<>, u32) {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let n = lines.len() as u32;
        (move |i: u32| lines.get(i as usize).cloned(), n)
    }

    #[test]
    fn bounds_cover_the_contiguous_run_only() {
        let (l, n) = buf("prose\n| a |\n|---|\n| b |\nmore prose\n| other |\n");
        assert_eq!(table_bounds(&l, 2, n), Some((1, 3)));
        assert_eq!(table_bounds(&l, 5, n), Some((5, 5)), "a separate table");
        assert_eq!(table_bounds(&l, 0, n), None, "not on a table");
    }

    fn table(rows: &[&str]) -> Vec<Row> {
        rows.iter().filter_map(|r| parse_row(r)).collect()
    }

    #[test]
    fn rows_swap_and_columns_swap_across_every_row() {
        let mut t = table(&["| a | b |", "| c | d |"]);
        assert!(swap_rows(&mut t, 0, 1));
        assert_eq!(align(&t)[0], "| c | d |");

        let mut t = table(&["| a | b |", "| c | d |"]);
        assert!(swap_columns(&mut t, 0, 1));
        assert_eq!(
            align(&t),
            vec!["| b | a |".to_string(), "| d | c |".to_string()]
        );
    }

    /// A rule marks a section boundary; dragging it through the body would
    /// silently re-section the table.
    #[test]
    fn a_separator_refuses_to_be_moved() {
        let mut t = table(&["| a |", "|---|", "| b |"]);
        assert!(!swap_rows(&mut t, 1, 2));
        assert_eq!(t[1], Row::Separator);
    }

    /// A ragged row must be padded before a column swap, or it silently
    /// keeps its columns transposed against every other row.
    #[test]
    fn a_column_swap_pads_ragged_rows_first() {
        let mut t = table(&["| a | b | c |", "| x |"]);
        assert!(swap_columns(&mut t, 0, 2));
        assert_eq!(
            align(&t),
            vec!["| c | b | a |".to_string(), "|   |   | x |".to_string()]
        );
    }

    #[test]
    fn inserting_a_row_matches_the_tables_width() {
        let mut t = table(&["| a | b |", "| c | d |"]);
        insert_row(&mut t, 0);
        assert_eq!(t.len(), 3);
        assert_eq!(align(&t)[1], "|   |   |", "as wide as the table");
    }

    #[test]
    fn inserting_a_column_widens_every_row() {
        let mut t = table(&["| a | b |", "|---+---|", "| c | d |"]);
        insert_column(&mut t, 0);
        assert_eq!(align(&t)[0], "| a |   | b |");
        assert_eq!(align(&t)[2], "| c |   | d |");
    }

    /// A table with no rows is not a table — deleting the last one would
    /// silently destroy it.
    #[test]
    fn the_last_row_and_column_refuse_deletion() {
        let mut t = table(&["| a |"]);
        assert!(!delete_row(&mut t, 0));
        assert!(!delete_column(&mut t, 0));
        assert_eq!(align(&t), vec!["| a |".to_string()]);

        let mut t = table(&["| a | b |", "| c | d |"]);
        assert!(delete_row(&mut t, 0));
        assert!(delete_column(&mut t, 0));
        assert_eq!(align(&t), vec!["| d |".to_string()]);
    }

    #[test]
    fn cell_index_follows_the_cursor() {
        let l = "| a | b | c |";
        assert_eq!(cell_at(l, 2), 0);
        assert_eq!(cell_at(l, 6), 1);
        assert_eq!(cell_at(l, 10), 2);
    }

    #[test]
    fn cell_start_places_the_caret_on_the_cells_text() {
        let l = "| a | bb | c |";
        assert_eq!(&l[cell_start(l, 0)..cell_start(l, 0) + 1], "a");
        assert_eq!(&l[cell_start(l, 1)..cell_start(l, 1) + 2], "bb");
        // Past the last cell parks at the end rather than column zero.
        assert_eq!(cell_start(l, 9), l.len());
    }
}

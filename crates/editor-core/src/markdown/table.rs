//! Pipe tables: cell splitting, separator rows, column alignment and the
//! greedy scan that groups header, separator and data rows into tables.

use std::ops::Range;

/// Identifies one table within a `Blocks` value. Stable between updates that
/// do not touch the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableId(pub u32);

/// Column alignment from the separator row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    /// `---`: no colon, renders as left aligned.
    None,
    /// `:---`
    Left,
    /// `:---:`
    Centre,
    /// `---:`
    Right,
}

/// Where a table sits: header, separator and last row are line indices.
/// `last == sep` when the table has no data rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub header: usize,
    pub sep: usize,
    pub last: usize,
    pub aligns: Vec<Align>,
}

/// Splits a table row into trimmed cells. The trimmed line must start and end
/// with `|`; cells split on `|` not preceded by a backslash. Returns `None`
/// when the line is not a table row.
pub fn parse_cells(line: &str) -> Option<Vec<&str>> {
    let t = line.trim();
    if t.len() < 2 || !t.starts_with('|') || !t.ends_with('|') {
        return None;
    }
    let inner = &t[1..t.len() - 1];
    let bytes = inner.as_bytes();
    let mut cells = Vec::new();
    let mut start = 0;
    for i in 0..bytes.len() {
        if bytes[i] == b'|' && (i == 0 || bytes[i - 1] != b'\\') {
            cells.push(inner[start..i].trim());
            start = i + 1;
        }
    }
    cells.push(inner[start..].trim());
    Some(cells)
}

/// A separator cell: `^:?-+:?$`.
fn is_separator_cell(cell: &str) -> bool {
    let c = cell.strip_prefix(':').unwrap_or(cell);
    let c = c.strip_suffix(':').unwrap_or(c);
    !c.is_empty() && c.bytes().all(|b| b == b'-')
}

/// Whether every cell of the row is a separator cell (`|---|:---:|`).
pub fn is_separator(line: &str) -> bool {
    parse_cells(line).is_some_and(|cells| cells.iter().all(|c| is_separator_cell(c)))
}

/// Column alignments of a separator row, or `None` if it is not one.
pub fn alignments(line: &str) -> Option<Vec<Align>> {
    if !is_separator(line) {
        return None;
    }
    let cells = parse_cells(line)?;
    Some(
        cells
            .iter()
            .map(|c| match (c.starts_with(':'), c.ends_with(':')) {
                (true, true) => Align::Centre,
                (false, true) => Align::Right,
                (true, false) => Align::Left,
                (false, false) => Align::None,
            })
            .collect(),
    )
}

/// A table found by `find_tables`, before an id is assigned.
pub(crate) struct Found {
    pub header: usize,
    pub sep: usize,
    pub last: usize,
    pub aligns: Vec<Align>,
}

/// Scans `range` top to bottom for tables. `row(i)` gives the line text, or
/// `None` for lines that cannot take part (inside code). A header needs the
/// next line to be a separator with the same cell count; data rows follow
/// while their cell count matches. A line used as a data row is never a header.
pub(crate) fn find_tables<'a>(
    range: Range<usize>,
    row: impl Fn(usize) -> Option<&'a str>,
) -> Vec<Found> {
    let cells = |i: usize| row(i).and_then(parse_cells).map(|c| c.len());
    let mut out = Vec::new();
    let mut i = range.start;
    while i < range.end {
        let Some(cols) = cells(i) else {
            i += 1;
            continue;
        };
        let sep = i + 1;
        let sep_text = if sep < range.end { row(sep) } else { None };
        let aligns = match sep_text.and_then(alignments) {
            Some(a) if a.len() == cols => a,
            _ => {
                i += 1;
                continue;
            }
        };
        let mut j = sep + 1;
        while j < range.end && cells(j) == Some(cols) {
            j += 1;
        }
        out.push(Found {
            header: i,
            sep,
            last: j - 1,
            aligns,
        });
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from moon_edit document_line_test.dart: parseTableCells.
    #[test]
    fn parses_simple_row_into_cells() {
        assert_eq!(parse_cells("| a | b | c |"), Some(vec!["a", "b", "c"]));
    }

    #[test]
    fn trims_whitespace_from_cells() {
        assert_eq!(
            parse_cells("|  hello  |  world  |"),
            Some(vec!["hello", "world"])
        );
    }

    #[test]
    fn handles_empty_cells() {
        assert_eq!(parse_cells("| | data |"), Some(vec!["", "data"]));
    }

    #[test]
    fn handles_escaped_pipes() {
        assert_eq!(parse_cells(r"| a \| b | c |"), Some(vec![r"a \| b", "c"]));
    }

    #[test]
    fn non_table_line_has_no_cells() {
        assert_eq!(parse_cells("not a table"), None);
        assert_eq!(parse_cells("no pipe |"), None);
        assert_eq!(parse_cells("| no end pipe"), None);
        assert_eq!(parse_cells("|"), None);
    }

    #[test]
    fn handles_single_column() {
        assert_eq!(parse_cells("| only |"), Some(vec!["only"]));
    }

    // isTableSeparatorLine.
    #[test]
    fn detects_separators() {
        for s in [
            "|---|---|",
            "| --- | --- |",
            "|:---|:---|",
            "|---:|---:|",
            "|:---:|:---:|",
            "|:---|:---:|---:|",
        ] {
            assert!(is_separator(s), "{s}");
        }
    }

    #[test]
    fn separator_requires_a_dash_and_pipes() {
        assert!(!is_separator("|:|:|"));
        assert!(!is_separator("| text | more |"));
        assert!(!is_separator("---"));
    }

    // parseTableAlignments. moon_edit maps `---` to left; this port keeps the
    // distinction as `Align::None`, which renders as left.
    #[test]
    fn parses_alignments() {
        assert_eq!(
            alignments("|---|---|"),
            Some(vec![Align::None, Align::None])
        );
        assert_eq!(alignments("|:---|"), Some(vec![Align::Left]));
        assert_eq!(alignments("|---:|"), Some(vec![Align::Right]));
        assert_eq!(alignments("|:---:|"), Some(vec![Align::Centre]));
        assert_eq!(
            alignments("|:---|:---:|---:|"),
            Some(vec![Align::Left, Align::Centre, Align::Right])
        );
        assert_eq!(alignments("| text |"), None);
    }
}

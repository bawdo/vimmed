//! Table editing: inserting a new table, moving between cells (Tab,
//! Shift+Tab, Insert-mode Enter), the `t` commands that add, delete, move and
//! align rows and columns, and the grid picker's state.
//!
//! Each function is pure: it reads the buffer (and the table's `TableInfo`
//! from `Blocks`) and returns where the cursor goes, or an `EditPlan`. Plans
//! follow `lists::EditPlan`'s convention: edits apply in order through
//! `Buffer::replace`, each edit's range refers to the buffer as left by the
//! edits before it, and `cursor` refers to the buffer after the last edit and
//! sits on a grapheme boundary there. (No plan here has two edits that move
//! lines, so every range also holds in the buffer the plan was made from.)
//!
//! Rows and columns: a cell is `(row, col)`, where `row` is the line's offset
//! from the header (header 0, separator 1, data rows from 2) and `col` counts
//! from 0. The cursor's column comes from counting unescaped pipes up to and
//! including it, so a cursor on a pipe is in the cell that pipe opens.

use std::ops::Range;

use crate::buffer::{Buffer, Edit, Pos};
use crate::grapheme;
use crate::lists::EditPlan;
use crate::markdown::table::parse_cells;
use crate::markdown::{Blocks, LineKind, TableInfo};
use crate::strings;

/// The generated header cell text before its number: `Column 1`, `Column 2`.
/// Document content, not a user-visible string.
const HEADER_CELL: &str = "Column";

/// The separator cell of a new column, and of a column cycled back to the
/// default alignment.
const SEP_CELL: &str = "---";

/// The table line `line` belongs to, or `None` off a table (code included).
/// `blocks` must describe the buffer.
pub fn table_at(blocks: &Blocks, line: usize) -> Option<&TableInfo> {
    match *blocks.kind(line) {
        LineKind::TableHeader { table }
        | LineKind::TableSep { table }
        | LineKind::TableRow { table } => Some(blocks.table(table)),
        _ => None,
    }
}

/// Inserts a `cols` x `rows` table at `cursor`'s line: a header
/// `| Column 1 | Column 2 |`, a separator `| --- | --- |` and `rows` empty
/// rows `|  |  |`. On an empty line the table replaces it; otherwise it goes
/// below the line with a blank line above and below it. The cursor goes to
/// the first data cell (the first header cell when `rows` is 0). `cursor` is
/// clamped first, and `cols` of 0 makes one column.
pub fn insert(buf: &Buffer, cursor: Pos, cols: u32, rows: u32) -> EditPlan {
    let line = buf.clamp(cursor).line;
    let text = buf.line(line);
    let table = generate(cols.max(1) as usize, rows as usize);
    let first_row = if rows == 0 { 0 } else { 2 };
    let col = cell_start(&table[first_row], 0);
    let body = table.join("\n");
    if text.is_empty() {
        let at = Pos { line, col: 0 };
        return one(
            at..at,
            body,
            Pos {
                line: line + first_row,
                col,
            },
        );
    }
    let at = Pos {
        line,
        col: text.len(),
    };
    one(
        at..at,
        format!("\n\n{body}\n"),
        Pos {
            line: line + 2 + first_row,
            col,
        },
    )
}

/// The cell `pos` is in, as `(row, col)` (see the module docs). `pos` is
/// clamped to the buffer, and then to the table's lines.
pub fn cell_at(info: &TableInfo, buf: &Buffer, pos: Pos) -> (usize, usize) {
    let pos = buf.clamp(pos);
    let line = pos.line.clamp(info.header, info.last);
    let col = if line == pos.line {
        column_of(buf.line(line), pos.col, ncols(info))
    } else {
        0
    };
    (line - info.header, col)
}

/// Where a cell move goes: either the cursor only moves (no undo entry), or
/// the table grows a row (one undo step) and the cursor lands in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellMove {
    Move(Pos),
    Edit(EditPlan),
}

/// Tab: the start of the next cell, right along the row, then the first cell
/// of the next row, skipping the separator. From the last cell of the last
/// row an empty row is appended and the cursor goes to its first cell.
///
/// Divergence: on the separator this moves as if from the header (moon_edit
/// finds no row for the separator and stays on it, or appends a row).
pub fn next_cell(info: &TableInfo, buf: &Buffer, pos: Pos) -> CellMove {
    let (line, col) = nav_cell(info, buf, pos);
    if col + 1 < ncols(info) {
        return CellMove::Move(Pos {
            line,
            col: cell_start(buf.line(line), col + 1),
        });
    }
    match next_line(info, line) {
        Some(next) => CellMove::Move(Pos {
            line: next,
            col: cell_start(buf.line(next), 0),
        }),
        None => CellMove::Edit(append_row(info, buf, 0)),
    }
}

/// Shift+Tab: the start of the previous cell, left along the row, then the
/// last cell of the previous row, skipping the separator. `None` from the
/// first header cell (and the first separator cell, which moves as the
/// header's; see `next_cell`).
pub fn prev_cell(info: &TableInfo, buf: &Buffer, pos: Pos) -> Option<Pos> {
    let (line, col) = nav_cell(info, buf, pos);
    if col > 0 {
        return Some(Pos {
            line,
            col: cell_start(buf.line(line), col - 1),
        });
    }
    let prev = if line > info.sep {
        line - 1
    } else {
        return None;
    };
    let prev = if prev == info.sep { info.header } else { prev };
    Some(Pos {
        line: prev,
        col: cell_start(buf.line(prev), ncols(info) - 1),
    })
}

/// Insert-mode Enter: the same column's cell on the next row, skipping the
/// separator. From the last row an empty row is appended and the cursor goes
/// to that column's cell in it. The separator moves as the header (see
/// `next_cell`).
pub fn next_row(info: &TableInfo, buf: &Buffer, pos: Pos) -> CellMove {
    let (line, col) = nav_cell(info, buf, pos);
    match next_line(info, line) {
        Some(next) => CellMove::Move(Pos {
            line: next,
            col: cell_start(buf.line(next), col),
        }),
        None => CellMove::Edit(append_row(info, buf, col)),
    }
}

/// The `t` command `key` with the cursor at `cursor` in the table `info`:
///
/// - `a` adds an empty row below the cursor's row, `A` above it; on the
///   header or separator both add it after the separator (for `a` a
///   divergence: moon_edit puts it between the header and the separator).
///   The cursor goes to the new row's first cell.
/// - `d` deletes the cursor's row (the whole table on the header or
///   separator), `D` the whole table. The cursor goes to col 0 of the line
///   now at the deleted one, or of the line above when nothing follows.
/// - `c` adds a column right of the cursor's, `C` left of it, headed
///   `Column <n+1>`. `x` deletes the cursor's column (the whole table when it
///   is the only one). `>` and `<` swap it with its right or left neighbour.
///   `=` cycles its alignment: default or left, then centre, then right.
///   These rewrite each changed row as `| a | b |` with trimmed cells. The
///   cursor stays on its line, at the start of the cell with its column
///   index (where the column went, for `>` and `<`); on a line left as it was
///   it stays where it is.
///
/// `None` (nothing to record) for any other key, a cursor off the table, `>`
/// or `<` at the edge, or an `info` that visibly lags `buf`. `cursor` is
/// clamped first.
pub fn command(info: &TableInfo, buf: &Buffer, cursor: Pos, key: char) -> Option<EditPlan> {
    let cursor = buf.clamp(cursor);
    if info.last >= buf.line_count() || !(info.header..=info.last).contains(&cursor.line) {
        return None;
    }
    let line = cursor.line;
    let on_data = line > info.sep;
    let n = ncols(info);
    let col = column_of(buf.line(line), cursor.col, n);
    match key {
        'a' if on_data => Some(insert_row_after(info, buf, line)),
        'a' | 'A' if !on_data => Some(insert_row_after(info, buf, info.sep)),
        'A' => {
            let row = empty_row(n);
            let at = Pos { line, col: 0 };
            let cursor = Pos {
                line,
                col: cell_start(&row, 0),
            };
            Some(one(at..at, format!("{row}\n"), cursor))
        }
        'd' if on_data => Some(delete_lines(buf, line, line)),
        'd' | 'D' => Some(delete_lines(buf, info.header, info.last)),
        'x' if n == 1 => Some(delete_lines(buf, info.header, info.last)),
        'c' | 'C' | 'x' | '>' | '<' | '=' => {
            let rows = rows(info, buf)?;
            let (new_rows, target) = column_op(info, rows, key, col)?;
            let plan = rewrite(info, buf, cursor, new_rows, target);
            (!plan.edits.is_empty()).then_some(plan)
        }
        _ => None,
    }
}

/// The `:table` grid picker's state: the grid's size (`cols` x `rows`) and
/// the selected cell. The grid starts at 5x5 with the top-left cell selected
/// and grows by one column (row) each time the selection reaches its last
/// column (row), up to 10x10. It never shrinks back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TablePicker {
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
}

/// The picker grid's starting and largest size, per side.
const PICKER_MIN: u32 = 5;
const PICKER_MAX: u32 = 10;

impl Default for TablePicker {
    fn default() -> TablePicker {
        TablePicker::new()
    }
}

impl TablePicker {
    pub fn new() -> TablePicker {
        TablePicker {
            cols: PICKER_MIN,
            rows: PICKER_MIN,
            col: 0,
            row: 0,
        }
    }

    /// The grid's width in cells.
    pub fn cols(&self) -> u32 {
        self.cols
    }

    /// The grid's height in cells.
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// The selected cell, `(col, row)` from 0.
    pub fn selected(&self) -> (u32, u32) {
        (self.col, self.row)
    }

    /// Selects a cell (mouse hover or click), clamped to the grid.
    pub fn select(&mut self, col: u32, row: u32) {
        self.col = col.min(self.cols - 1);
        self.row = row.min(self.rows - 1);
        if self.col + 1 >= self.cols && self.cols < PICKER_MAX {
            self.cols += 1;
        }
        if self.row + 1 >= self.rows && self.rows < PICKER_MAX {
            self.rows += 1;
        }
    }

    /// `h` and Left: one column fewer, down to one.
    pub fn left(&mut self) {
        self.select(self.col.saturating_sub(1), self.row);
    }

    /// `l` and Right: one column more.
    pub fn right(&mut self) {
        self.select(self.col + 1, self.row);
    }

    /// `k` and Up: one row fewer, down to one.
    pub fn up(&mut self) {
        self.select(self.col, self.row.saturating_sub(1));
    }

    /// `j` and Down: one row more.
    pub fn down(&mut self) {
        self.select(self.col, self.row + 1);
    }

    /// What Enter inserts: `(cols, rows)`, the selected cell's column and
    /// row plus 1.
    pub fn size(&self) -> (u32, u32) {
        (self.col + 1, self.row + 1)
    }

    /// The label under the grid: `<cols> x <rows>` of `size`.
    pub fn label(&self) -> String {
        let (c, r) = self.size();
        strings::table_size(c, r)
    }
}

fn one(range: Range<Pos>, text: impl Into<String>, cursor: Pos) -> EditPlan {
    EditPlan {
        edits: vec![Edit {
            range,
            text: text.into(),
        }],
        cursor,
    }
}

/// The lines of a new table.
fn generate(cols: usize, rows: usize) -> Vec<String> {
    let header: Vec<String> = (1..=cols).map(|i| format!("{HEADER_CELL} {i}")).collect();
    let mut out = vec![build_row(&header), build_row(&vec![SEP_CELL; cols])];
    out.extend(std::iter::repeat_n(empty_row(cols), rows));
    out
}

/// `| a | b |`.
fn build_row<S: AsRef<str>>(cells: &[S]) -> String {
    let cells: Vec<&str> = cells.iter().map(AsRef::as_ref).collect();
    format!("| {} |", cells.join(" | "))
}

/// A row of `n` empty cells: `|  |  |`.
fn empty_row(n: usize) -> String {
    build_row(&vec![""; n])
}

/// The column count; a table always has at least one.
fn ncols(info: &TableInfo) -> usize {
    info.aligns.len().max(1)
}

fn is_pipe(bytes: &[u8], i: usize) -> bool {
    bytes[i] == b'|' && (i == 0 || bytes[i - 1] != b'\\')
}

/// The column at byte `col` of `text`: unescaped pipes up to and including
/// `col`, less the leading one, clamped to `0..n`.
fn column_of(text: &str, col: usize, n: usize) -> usize {
    let bytes = text.as_bytes();
    let limit = col.saturating_add(1).min(bytes.len());
    let pipes = (0..limit).filter(|&i| is_pipe(bytes, i)).count();
    pipes.saturating_sub(1).min(n - 1)
}

/// Where cell `column`'s content starts: after its opening pipe and one
/// space, backed off to a grapheme boundary (a lone combining mark after
/// the space joins it). 0 when the row has no such cell. The result always
/// maps back to `column` through `column_of`.
fn cell_start(text: &str, column: usize) -> usize {
    let bytes = text.as_bytes();
    let Some(pipe) = (0..bytes.len()).filter(|&i| is_pipe(bytes, i)).nth(column) else {
        return 0;
    };
    let mut at = pipe + 1;
    if bytes.get(at) == Some(&b' ') {
        at += 1;
    }
    grapheme::floor(text, at)
}

/// The cursor's line and column for navigation: clamped to the table, with
/// the separator standing in for the header.
fn nav_cell(info: &TableInfo, buf: &Buffer, pos: Pos) -> (usize, usize) {
    let (row, col) = cell_at(info, buf, pos);
    let line = info.header + row;
    (if line == info.sep { info.header } else { line }, col)
}

/// The next row's line after `line` (header or data row), or `None` on the
/// last row.
fn next_line(info: &TableInfo, line: usize) -> Option<usize> {
    let next = if line == info.header {
        info.sep + 1
    } else {
        line + 1
    };
    (next <= info.last).then_some(next)
}

/// Appends an empty row below the table, cursor in its cell `col`.
fn append_row(info: &TableInfo, buf: &Buffer, col: usize) -> EditPlan {
    let row = empty_row(ncols(info));
    let at = Pos {
        line: info.last,
        col: buf.line(info.last).len(),
    };
    let cursor = Pos {
        line: info.last + 1,
        col: cell_start(&row, col),
    };
    one(at..at, format!("\n{row}"), cursor)
}

/// Inserts an empty row below `line`, cursor in its first cell.
fn insert_row_after(info: &TableInfo, buf: &Buffer, line: usize) -> EditPlan {
    let row = empty_row(ncols(info));
    let at = Pos {
        line,
        col: buf.line(line).len(),
    };
    let cursor = Pos {
        line: line + 1,
        col: cell_start(&row, 0),
    };
    one(at..at, format!("\n{row}"), cursor)
}

/// Deletes lines `first..=last` whole. The cursor goes to col 0 of the line
/// that takes their place, or of the line above when nothing follows (an
/// empty buffer keeps one empty line).
fn delete_lines(buf: &Buffer, first: usize, last: usize) -> EditPlan {
    if last + 1 < buf.line_count() {
        let at = Pos {
            line: first,
            col: 0,
        };
        return one(
            at..Pos {
                line: last + 1,
                col: 0,
            },
            "",
            at,
        );
    }
    if first > 0 {
        let above = first - 1;
        let start = Pos {
            line: above,
            col: buf.line(above).len(),
        };
        let end = Pos {
            line: last,
            col: buf.line(last).len(),
        };
        return one(
            start..end,
            "",
            Pos {
                line: above,
                col: 0,
            },
        );
    }
    let origin = Pos::default();
    let end = Pos {
        line: last,
        col: buf.line(last).len(),
    };
    one(origin..end, "", origin)
}

/// Every table line's trimmed cells, header first, or `None` when a line no
/// longer has the table's column count (`info` lags `buf`).
fn rows<'a>(info: &TableInfo, buf: &'a Buffer) -> Option<Vec<Vec<&'a str>>> {
    let n = ncols(info);
    (info.header..=info.last)
        .map(|l| parse_cells(buf.line(l)).filter(|c| c.len() == n))
        .collect()
}

/// Applies column operation `key` at column `col` to `rows` (header first,
/// then the separator). Returns the new cells of every line and the column
/// the cursor goes to, or `None` for `>` and `<` at the edge.
fn column_op(
    info: &TableInfo,
    rows: Vec<Vec<&str>>,
    key: char,
    col: usize,
) -> Option<(Vec<Vec<String>>, usize)> {
    let n = ncols(info);
    let new_header = format!("{HEADER_CELL} {}", n + 1);
    // Shortened to `new_header`'s lifetime so its cell can go in.
    let mut rows: Vec<Vec<&str>> = rows;
    let target = match key {
        'c' | 'C' => {
            let at = if key == 'c' { col + 1 } else { col };
            for (i, row) in rows.iter_mut().enumerate() {
                let cell = match i {
                    0 => new_header.as_str(),
                    1 => SEP_CELL,
                    _ => "",
                };
                row.insert(at, cell);
            }
            col
        }
        'x' => {
            for row in &mut rows {
                row.remove(col);
            }
            col.min(n - 2)
        }
        '>' | '<' => {
            let to = if key == '>' {
                (col + 1 < n).then_some(col + 1)?
            } else {
                col.checked_sub(1)?
            };
            for row in &mut rows {
                row.swap(col, to);
            }
            to
        }
        _ => {
            let sep = &mut rows[info.sep - info.header][col];
            *sep = match (sep.starts_with(':'), sep.ends_with(':')) {
                (true, true) => "---:",
                (false, true) => SEP_CELL,
                _ => ":---:",
            };
            col
        }
    };
    let rows = rows
        .into_iter()
        .map(|r| r.into_iter().map(str::to_owned).collect())
        .collect();
    Some((rows, target))
}

/// Replaces each table line whose text changes with its rebuilt row. The
/// cursor goes to cell `target` on its line when that line changed, else it
/// stays.
fn rewrite(
    info: &TableInfo,
    buf: &Buffer,
    cursor: Pos,
    rows: Vec<Vec<String>>,
    target: usize,
) -> EditPlan {
    let mut edits = Vec::new();
    let mut new_cursor = cursor;
    for (i, cells) in rows.iter().enumerate() {
        let line = info.header + i;
        let old = buf.line(line);
        let new = build_row(cells);
        if new == old {
            continue;
        }
        if line == cursor.line {
            new_cursor = Pos {
                line,
                col: cell_start(&new, target),
            };
        }
        edits.push(Edit {
            range: Pos { line, col: 0 }..Pos {
                line,
                col: old.len(),
            },
            text: new,
        });
    }
    EditPlan {
        edits,
        cursor: new_cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::table::is_separator;
    use crate::markdown::{Align, LineKind};

    fn pos(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    fn lines(buf: &Buffer) -> Vec<&str> {
        (0..buf.line_count()).map(|i| buf.line(i)).collect()
    }

    /// Applies `plan` edit by edit, then checks the cursor is in range and on
    /// a grapheme boundary of the result (the lists.rs helper).
    fn apply(buf: &mut Buffer, plan: &EditPlan) {
        for e in &plan.edits {
            buf.replace(e.range.clone(), &e.text);
        }
        assert_on_boundary(buf, plan.cursor);
    }

    fn assert_on_boundary(buf: &Buffer, c: Pos) {
        assert!(c.line < buf.line_count(), "cursor line {c:?} out of range");
        let l = buf.line(c.line);
        assert!(c.col <= l.len(), "cursor col {c:?} past {l:?}");
        assert_eq!(
            grapheme::floor(l, c.col),
            c.col,
            "cursor {c:?} splits {l:?}"
        );
    }

    /// The table on `line` of `buf`.
    fn info(buf: &Buffer, line: usize) -> TableInfo {
        table_at(&Blocks::new(buf), line)
            .cloned()
            .unwrap_or_else(|| panic!("no table on line {line}"))
    }

    /// Runs the `t` command `key` with the cursor at `cursor` and applies the
    /// plan, returning the new buffer and cursor, or `None` when it declines.
    fn cmd(text: &str, cursor: Pos, key: char) -> Option<(Buffer, Pos)> {
        let mut buf = Buffer::from_text(text);
        let t = info(&buf, cursor.line);
        let plan = command(&t, &buf, cursor, key)?;
        apply(&mut buf, &plan);
        Some((buf, plan.cursor))
    }

    fn insert_at(text: &str, cursor: Pos, cols: u32, rows: u32) -> (Buffer, Pos) {
        let mut buf = Buffer::from_text(text);
        let plan = insert(&buf, cursor, cols, rows);
        apply(&mut buf, &plan);
        (buf, plan.cursor)
    }

    fn cells(buf: &Buffer, line: usize) -> Vec<&str> {
        parse_cells(buf.line(line)).expect("table row")
    }

    /// Follows a cell move: applies the edit when there is one.
    fn follow(buf: &mut Buffer, m: CellMove) -> Pos {
        match m {
            CellMove::Move(p) => {
                assert_on_boundary(buf, p);
                p
            }
            CellMove::Edit(plan) => {
                apply(buf, &plan);
                plan.cursor
            }
        }
    }

    /// The 3-column, 2-data-row table the moon_edit command and navigation
    /// tests share.
    const T: &str = "| A | B | C |\n| --- | --- | --- |\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |";

    // generateTable (table_operations_test.dart).

    #[test]
    fn generates_two_by_one() {
        assert_eq!(
            generate(2, 1),
            ["| Column 1 | Column 2 |", "| --- | --- |", "|  |  |"]
        );
    }

    #[test]
    fn generates_three_by_two() {
        assert_eq!(
            generate(3, 2),
            [
                "| Column 1 | Column 2 | Column 3 |",
                "| --- | --- | --- |",
                "|  |  |  |",
                "|  |  |  |"
            ]
        );
    }

    #[test]
    fn generates_one_by_one() {
        assert_eq!(generate(1, 1), ["| Column 1 |", "| --- |", "|  |"]);
    }

    #[test]
    fn generates_header_only_for_zero_rows() {
        assert_eq!(generate(2, 0), ["| Column 1 | Column 2 |", "| --- | --- |"]);
    }

    #[test]
    fn generates_five_by_three() {
        let g = generate(5, 3);
        assert_eq!(g.len(), 5);
        assert_eq!(
            g[0],
            "| Column 1 | Column 2 | Column 3 | Column 4 | Column 5 |"
        );
        assert_eq!(g[1], "| --- | --- | --- | --- | --- |");
        for row in &g[2..] {
            assert_eq!(row, "|  |  |  |  |  |");
        }
    }

    #[test]
    fn generated_table_parses_as_a_table() {
        let g = generate(3, 2);
        for line in &g {
            assert_eq!(parse_cells(line).map(|c| c.len()), Some(3), "{line}");
        }
        assert!(is_separator(&g[1]));
    }

    // insertTable (document_controller.dart).

    #[test]
    fn insert_replaces_an_empty_line() {
        let (buf, c) = insert_at("a\n\nb", pos(1, 0), 2, 1);
        assert_eq!(
            lines(&buf),
            [
                "a",
                "| Column 1 | Column 2 |",
                "| --- | --- |",
                "|  |  |",
                "b"
            ]
        );
        assert_eq!(c, pos(3, 2));
        let (buf, c) = insert_at("", pos(0, 0), 1, 2);
        assert_eq!(lines(&buf), ["| Column 1 |", "| --- |", "|  |", "|  |"]);
        assert_eq!(c, pos(2, 2));
    }

    #[test]
    fn insert_on_a_non_empty_line_adds_blank_lines_around() {
        let (buf, c) = insert_at("text\nnext", pos(0, 2), 2, 1);
        assert_eq!(
            lines(&buf),
            [
                "text",
                "",
                "| Column 1 | Column 2 |",
                "| --- | --- |",
                "|  |  |",
                "",
                "next"
            ]
        );
        assert_eq!(c, pos(4, 2));
        let (buf, c) = insert_at("text", pos(0, 4), 1, 1);
        assert_eq!(
            lines(&buf),
            ["text", "", "| Column 1 |", "| --- |", "|  |", ""]
        );
        assert_eq!(c, pos(4, 2));
    }

    #[test]
    fn inserted_table_is_a_table() {
        let (buf, c) = insert_at("text", pos(0, 0), 3, 2);
        let t = info(&buf, c.line);
        assert_eq!((t.header, t.sep, t.last), (2, 3, 5));
        assert_eq!(cell_at(&t, &buf, c), (2, 0));
    }

    #[test]
    fn insert_with_zero_rows_puts_the_cursor_in_the_header() {
        let (buf, c) = insert_at("", pos(0, 0), 2, 0);
        assert_eq!(lines(&buf), ["| Column 1 | Column 2 |", "| --- | --- |"]);
        assert_eq!(c, pos(0, 2));
    }

    #[test]
    fn insert_with_zero_columns_makes_one() {
        let (buf, _) = insert_at("", pos(0, 0), 0, 1);
        assert_eq!(lines(&buf), ["| Column 1 |", "| --- |", "|  |"]);
    }

    #[test]
    fn insert_keeps_crlf() {
        let (buf, _) = insert_at("a\r\n", pos(0, 1), 1, 1);
        assert_eq!(
            buf.to_text(),
            "a\r\n\r\n| Column 1 |\r\n| --- |\r\n|  |\r\n\r\n"
        );
    }

    // cellStartOffset (table_operations_test.dart).

    #[test]
    fn cell_start_offsets() {
        assert_eq!(cell_start("| hello | world |", 0), 2);
        assert_eq!(cell_start("| hello | world |", 1), 10);
        assert_eq!(cell_start("| A | B | C |", 2), 10);
        assert_eq!(cell_start("|  |  |", 0), 2);
        assert_eq!(cell_start("|  |  |", 1), 5);
        assert_eq!(cell_start("| A | B |", 5), 0);
        assert_eq!(cell_start("| hello |", 0), 2);
    }

    // TableContext.fromCursor (table_context_test.dart): finding the table.

    #[test]
    fn no_table_off_a_table_line() {
        for (text, line) in [
            ("Just a paragraph", 0),
            ("", 0),
            ("Hello", 5),
            (
                "| A | B |\n|---|---|\n| 1 | 2 |\n\nNot a table\n\n| C | D |\n|---|---|\n| 3 | 4 |",
                4,
            ),
            ("# Title\n\n| A | B |\n|---|---|\n| 1 | 2 |", 0),
        ] {
            let buf = Buffer::from_text(text);
            assert!(table_at(&Blocks::new(&buf), line).is_none(), "{text:?}");
        }
    }

    #[test]
    fn finds_the_table_from_any_of_its_lines() {
        let buf = Buffer::from_text("| Name | Age |\n|------|-----|\n| Alice | 30 |");
        for line in 0..3 {
            let t = info(&buf, line);
            assert_eq!((t.header, t.sep, t.last), (0, 1, 2));
            assert_eq!(t.aligns.len(), 2);
        }
    }

    #[test]
    fn multi_row_table_tracks_every_data_row() {
        let buf = Buffer::from_text(
            "| Name | Age | City |\n|------|-----|------|\n| Alice | 30 | NYC |\n| Bob | 25 | LA |\n| Carol | 35 | SF |",
        );
        let t = info(&buf, 0);
        assert_eq!((t.header, t.sep, t.last), (0, 1, 4));
        assert_eq!(t.aligns.len(), 3);
        assert_eq!(info(&buf, 4), t);
    }

    #[test]
    fn table_detects_mixed_alignments() {
        let buf = Buffer::from_text(
            "| Left | Center | Right |\n|:-----|:------:|------:|\n| a | b | c |",
        );
        assert_eq!(
            info(&buf, 0).aligns,
            [Align::Left, Align::Centre, Align::Right]
        );
    }

    #[test]
    fn table_after_text() {
        let buf =
            Buffer::from_text("# Title\n\nSome paragraph text.\n\n| A | B |\n|---|---|\n| 1 | 2 |");
        let t = info(&buf, 4);
        assert_eq!((t.header, t.sep, t.last), (4, 5, 6));
    }

    #[test]
    fn header_only_table() {
        let buf = Buffer::from_text("| A | B |\n|---|---|");
        let t = info(&buf, 0);
        assert_eq!((t.header, t.sep, t.last), (0, 1, 1));
    }

    // table_integration_test.dart (the logic parts).

    #[test]
    fn table_between_paragraphs_and_two_tables_apart() {
        let buf = Buffer::from_text("Before\n| A | B |\n|---|---|\n| 1 | 2 |\nAfter");
        let blocks = Blocks::new(&buf);
        assert!(table_at(&blocks, 0).is_none());
        assert!(table_at(&blocks, 4).is_none());
        assert_eq!(table_at(&blocks, 3).map(|t| t.header), Some(1));

        let buf = Buffer::from_text("| A |\n|---|\n| 1 |\nMiddle\n| B |\n|---|\n| 2 |");
        let blocks = Blocks::new(&buf);
        assert_eq!(table_at(&blocks, 2).map(|t| t.header), Some(0));
        assert!(table_at(&blocks, 3).is_none());
        assert_eq!(table_at(&blocks, 6).map(|t| t.header), Some(4));
    }

    #[test]
    fn table_inside_code_is_not_a_table() {
        let buf = Buffer::from_text("```\n| A |\n|---|\n```");
        assert!(table_at(&Blocks::new(&buf), 1).is_none());
    }

    // currentCellRow (table_context_test.dart). Changed: moon_edit numbers
    // the separator -1 and data rows from 1; here the row is the line's
    // offset from the header (separator 1, data rows from 2), so it never
    // goes negative.

    #[test]
    fn cell_row_is_the_offset_from_the_header() {
        let buf = Buffer::from_text("| Name | Age |\n|------|-----|\n| Alice | 30 |\n| Bob | 25 |");
        let t = info(&buf, 0);
        assert_eq!(cell_at(&t, &buf, pos(0, 3)).0, 0);
        assert_eq!(cell_at(&t, &buf, pos(1, 2)).0, 1);
        assert_eq!(cell_at(&t, &buf, pos(2, 3)).0, 2);
        assert_eq!(cell_at(&t, &buf, pos(3, 3)).0, 3);
    }

    // currentCellColumn (table_context_test.dart).

    #[test]
    fn cell_column_by_pipe_counting() {
        let buf = Buffer::from_text("| Name | Age |\n|------|-----|\n| Alice | 30 |");
        let t = info(&buf, 0);
        let col = |c| cell_at(&t, &buf, pos(0, c)).1;
        assert_eq!(col(3), 0, "inside Name");
        assert_eq!(col(9), 1, "inside Age");
        assert_eq!(col(7), 1, "on a pipe enters the next column");
        assert_eq!(col(0), 0, "on the leading pipe");
        assert_eq!(col(14), 1, "at the end of the line");

        let buf = Buffer::from_text("| A | B | C |\n|---|---|---|\n| 1 | 2 | 3 |");
        let t = info(&buf, 0);
        assert_eq!(cell_at(&t, &buf, pos(0, 10)).1, 2);
    }

    #[test]
    fn escaped_pipe_does_not_split_a_cell() {
        let buf = Buffer::from_text("| A | B |\n|---|---|\n| a \\| b | c |");
        let t = info(&buf, 2);
        assert_eq!(cell_at(&t, &buf, pos(2, 7)).1, 0);
        assert_eq!(cell_at(&t, &buf, pos(2, 11)).1, 1);
    }

    // nextCell (table_navigation_test.dart).

    #[test]
    fn next_cell_moves_right_within_a_row() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        assert_eq!(next_cell(&t, &buf, pos(0, 2)), CellMove::Move(pos(0, 6)));
    }

    #[test]
    fn next_cell_wraps_past_the_separator() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        assert_eq!(next_cell(&t, &buf, pos(0, 10)), CellMove::Move(pos(2, 2)));
        assert_eq!(next_cell(&t, &buf, pos(2, 10)), CellMove::Move(pos(3, 2)));
    }

    #[test]
    fn next_cell_from_the_last_cell_appends_a_row() {
        let mut buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        let m = next_cell(&t, &buf, pos(3, 10));
        assert!(matches!(m, CellMove::Edit(_)));
        let c = follow(&mut buf, m);
        assert_eq!(buf.line_count(), 5);
        assert_eq!(buf.line(4), "|  |  |  |");
        assert_eq!(c, pos(4, 2));
        // The new row belongs to the table.
        assert_eq!(info(&buf, 4).last, 4);
    }

    #[test]
    fn next_cell_from_a_header_only_table_appends_the_first_row() {
        let mut buf = Buffer::from_text("| A | B |\n|---|---|\nafter");
        let t = info(&buf, 0);
        let m = next_cell(&t, &buf, pos(0, 6));
        let c = follow(&mut buf, m);
        assert_eq!(lines(&buf), ["| A | B |", "|---|---|", "|  |  |", "after"]);
        assert_eq!(c, pos(2, 2));
    }

    // previousCell (table_navigation_test.dart).

    #[test]
    fn prev_cell_moves_left_within_a_row() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        assert_eq!(prev_cell(&t, &buf, pos(2, 6)), Some(pos(2, 2)));
    }

    #[test]
    fn prev_cell_wraps_to_the_previous_row() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        assert_eq!(prev_cell(&t, &buf, pos(3, 2)), Some(pos(2, 10)));
        // From the first data row, past the separator to the header.
        assert_eq!(prev_cell(&t, &buf, pos(2, 2)), Some(pos(0, 10)));
    }

    #[test]
    fn prev_cell_stops_at_the_first_header_cell() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        assert_eq!(prev_cell(&t, &buf, pos(0, 2)), None);
    }

    // moveDown (table_navigation_test.dart): Insert-mode Enter.

    #[test]
    fn next_row_keeps_the_column() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        assert_eq!(next_row(&t, &buf, pos(2, 6)), CellMove::Move(pos(3, 6)));
    }

    #[test]
    fn next_row_skips_the_separator() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        assert_eq!(next_row(&t, &buf, pos(0, 2)), CellMove::Move(pos(2, 2)));
    }

    #[test]
    fn next_row_from_the_last_row_creates_one() {
        let mut buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        let m = next_row(&t, &buf, pos(3, 6));
        assert!(matches!(m, CellMove::Edit(_)));
        let c = follow(&mut buf, m);
        assert_eq!(buf.line_count(), 5);
        assert_eq!(c, pos(4, 5));
    }

    // Divergence: moon_edit's navigation finds no row for the separator, so
    // Enter there appends a row and Tab stays on the separator. Here the
    // separator navigates as if it were the header.
    #[test]
    fn separator_navigates_as_the_header() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        assert_eq!(next_row(&t, &buf, pos(1, 8)), CellMove::Move(pos(2, 6)));
        assert_eq!(next_cell(&t, &buf, pos(1, 2)), CellMove::Move(pos(0, 6)));
        assert_eq!(prev_cell(&t, &buf, pos(1, 2)), None);
    }

    // tn / tp (table_commands_test.dart) are Tab and Shift+Tab here.

    #[test]
    fn tab_and_shift_tab_from_cell_starts() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        let CellMove::Move(p) = next_cell(&t, &buf, pos(2, 2)) else {
            panic!("moved");
        };
        assert_eq!(p.line, 2);
        assert!(p.col > 2);
        assert_eq!(prev_cell(&t, &buf, pos(2, 6)), Some(pos(2, 2)));
    }

    // ta (table_commands_test.dart).

    #[test]
    fn ta_adds_a_row_below() {
        let (buf, c) = cmd(T, pos(2, 2), 'a').unwrap();
        assert_eq!(buf.line_count(), 5);
        assert_eq!(buf.line(3), "|  |  |  |");
        assert_eq!(buf.line(4), "| 4 | 5 | 6 |");
        assert_eq!(c, pos(3, 2));
    }

    #[test]
    fn ta_on_last_row_extends_the_table() {
        let (buf, c) = cmd("| A | B |\n| --- | --- |\n| 1 | 2 |", pos(2, 2), 'a').unwrap();
        assert_eq!(buf.line(3), "|  |  |");
        assert_eq!(c, pos(3, 2));
        let t = info(&buf, 3);
        assert_eq!(t.last, 3);
        assert_eq!(
            *Blocks::new(&buf).kind(3),
            LineKind::TableRow {
                table: match Blocks::new(&buf).kind(0) {
                    LineKind::TableHeader { table } => *table,
                    k => panic!("{k:?}"),
                }
            }
        );
    }

    // Divergence (Task 37): moon_edit's `ta` on the header inserts the row
    // between the header and the separator, which breaks the table. It
    // inserts after the separator instead, as `tA` does.
    #[test]
    fn ta_on_header_inserts_after_separator() {
        for line in [0, 1] {
            let (buf, c) = cmd(T, pos(line, 2), 'a').unwrap();
            assert_eq!(buf.line_count(), 5, "line {line}");
            assert_eq!(buf.line(0), "| A | B | C |");
            assert_eq!(buf.line(1), "| --- | --- | --- |");
            assert_eq!(buf.line(2), "|  |  |  |");
            assert_eq!(buf.line(3), "| 1 | 2 | 3 |");
            assert_eq!(c, pos(2, 2));
            assert_eq!(info(&buf, 0).last, 4);
        }
    }

    // tA (table_commands_test.dart).

    #[test]
    fn ta_upper_adds_a_row_above() {
        let (buf, c) = cmd(T, pos(2, 2), 'A').unwrap();
        assert_eq!(buf.line_count(), 5);
        assert_eq!(buf.line(2), "|  |  |  |");
        assert_eq!(buf.line(3), "| 1 | 2 | 3 |");
        assert_eq!(c, pos(2, 2));
    }

    #[test]
    fn ta_upper_on_header_or_separator_inserts_after_separator() {
        for line in [0, 1] {
            let (buf, c) = cmd(T, pos(line, 2), 'A').unwrap();
            assert_eq!(buf.line_count(), 5);
            assert_eq!(buf.line(0), "| A | B | C |");
            assert_eq!(buf.line(1), "| --- | --- | --- |");
            assert!(cells(&buf, 2).iter().all(|c| c.is_empty()));
            assert_eq!(c, pos(2, 2));
        }
    }

    #[test]
    fn inserted_row_below_separator_is_first_data_row() {
        let (buf, _) = cmd("| A | B |\n| --- | --- |\n| 1 | 2 |", pos(2, 2), 'A').unwrap();
        assert_eq!(buf.line(2), "|  |  |");
        assert_eq!(buf.line(3), "| 1 | 2 |");
        assert_eq!(cells(&buf, 2), ["", ""]);
    }

    // td (table_commands_test.dart, table_operations_test.dart).

    #[test]
    fn td_deletes_a_data_row() {
        let (buf, c) = cmd(T, pos(2, 2), 'd').unwrap();
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line(2), "| 4 | 5 | 6 |");
        assert_eq!(c, pos(2, 0));
    }

    #[test]
    fn td_deletes_the_last_data_row() {
        let (buf, c) = cmd("| A | B |\n| --- | --- |\n| 1 | 2 |", pos(2, 2), 'd').unwrap();
        assert_eq!(lines(&buf), ["| A | B |", "| --- | --- |"]);
        assert_eq!(c, pos(1, 0));
    }

    #[test]
    fn td_on_header_or_separator_deletes_the_table() {
        for line in [0, 1] {
            let (buf, c) = cmd(T, pos(line, 2), 'd').unwrap();
            assert_eq!(lines(&buf), [""]);
            assert_eq!(c, pos(0, 0));
        }
    }

    // tD (table_commands_test.dart, deleteTable in table_operations_test.dart).

    #[test]
    fn td_upper_deletes_the_table_from_any_row() {
        for line in 0..4 {
            let (buf, _) = cmd(T, pos(line, 2), 'D').unwrap();
            assert_eq!(lines(&buf), [""]);
        }
    }

    #[test]
    fn deleting_a_table_keeps_the_text_around_it() {
        let (buf, c) = cmd(
            "before\n| A | B |\n| --- | --- |\n| 1 | 2 |\nafter",
            pos(2, 0),
            'D',
        )
        .unwrap();
        assert_eq!(lines(&buf), ["before", "after"]);
        assert_eq!(c, pos(1, 0));
        let (buf, _) = cmd(
            "line1\nline2\n| A | B |\n| --- | --- |\n| 1 | 2 |\nline3\nline4",
            pos(4, 2),
            'D',
        )
        .unwrap();
        assert_eq!(lines(&buf), ["line1", "line2", "line3", "line4"]);
        // A table at the end of the document.
        let (buf, c) = cmd("before\n| A |\n| --- |", pos(1, 2), 'D').unwrap();
        assert_eq!(lines(&buf), ["before"]);
        assert_eq!(c, pos(0, 0));
    }

    // tc (table_commands_test.dart, insertColumn in table_operations_test.dart).

    #[test]
    fn tc_adds_a_column_right() {
        let (buf, c) = cmd(T, pos(2, 2), 'c').unwrap();
        assert_eq!(cells(&buf, 0), ["A", "Column 4", "B", "C"]);
        assert_eq!(buf.line(1), "| --- | --- | --- | --- |");
        assert_eq!(buf.line(2), "| 1 |  | 2 | 3 |");
        assert_eq!(info(&buf, 0).aligns.len(), 4);
        assert_eq!(c, pos(2, 2));
    }

    #[test]
    fn tc_on_the_last_column_appends_one() {
        let (buf, _) = cmd("| A | B |\n| --- | --- |\n| 1 | 2 |", pos(2, 6), 'c').unwrap();
        assert_eq!(
            lines(&buf),
            [
                "| A | B | Column 3 |",
                "| --- | --- | --- |",
                "| 1 | 2 |  |"
            ]
        );
    }

    #[test]
    fn tc_upper_adds_a_column_left() {
        let (buf, c) = cmd(T, pos(2, 2), 'C').unwrap();
        assert_eq!(cells(&buf, 0), ["Column 4", "A", "B", "C"]);
        assert_eq!(info(&buf, 0).aligns.len(), 4);
        // The cursor keeps its column index, so it sits in the new column.
        assert_eq!(c, pos(2, 2));
        let (buf, _) = cmd("| A | B |\n| --- | --- |\n| 1 | 2 |", pos(0, 2), 'C').unwrap();
        assert_eq!(
            lines(&buf),
            [
                "| Column 3 | A | B |",
                "| --- | --- | --- |",
                "|  | 1 | 2 |"
            ]
        );
    }

    #[test]
    fn added_column_keeps_every_row_parseable() {
        let (buf, _) = cmd(
            "| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |",
            pos(0, 2),
            'c',
        )
        .unwrap();
        for i in 0..4 {
            assert_eq!(cells(&buf, i).len(), 3, "line {i}");
        }
        assert_eq!(info(&buf, 0).last, 3);
    }

    // tx (table_commands_test.dart, deleteColumn in table_operations_test.dart).

    #[test]
    fn tx_deletes_the_current_column() {
        let (buf, c) = cmd(T, pos(2, 2), 'x').unwrap();
        assert_eq!(cells(&buf, 0), ["B", "C"]);
        assert_eq!(cells(&buf, 2), ["2", "3"]);
        assert_eq!(info(&buf, 0).aligns.len(), 2);
        assert_eq!(c, pos(2, 2));
    }

    #[test]
    fn tx_removes_middle_and_last_columns() {
        let t3 = "| A | B | C |\n| --- | --- | --- |\n| 1 | 2 | 3 |";
        let (buf, c) = cmd(t3, pos(2, 6), 'x').unwrap();
        assert_eq!(lines(&buf), ["| A | C |", "| --- | --- |", "| 1 | 3 |"]);
        assert_eq!(c, pos(2, 6));
        let (buf, c) = cmd(t3, pos(2, 10), 'x').unwrap();
        assert_eq!(lines(&buf), ["| A | B |", "| --- | --- |", "| 1 | 2 |"]);
        // The rightmost went: the cursor goes to the new last column.
        assert_eq!(c, pos(2, 6));
        let (buf, _) = cmd(T, pos(3, 6), 'x').unwrap();
        assert_eq!(buf.line(2), "| 1 | 3 |");
        assert_eq!(buf.line(3), "| 4 | 6 |");
    }

    #[test]
    fn tx_on_the_only_column_deletes_the_table() {
        let (buf, c) = cmd("| A |\n| --- |\n| 1 |", pos(2, 2), 'x').unwrap();
        assert_eq!(lines(&buf), [""]);
        assert_eq!(c, pos(0, 0));
    }

    // t> and t< (table_commands_test.dart, moveColumn in
    // table_operations_test.dart).

    #[test]
    fn t_gt_swaps_with_the_right_neighbour() {
        let (buf, c) = cmd(T, pos(2, 2), '>').unwrap();
        assert_eq!(cells(&buf, 0), ["B", "A", "C"]);
        assert_eq!(cells(&buf, 2), ["2", "1", "3"]);
        assert_eq!(c, pos(2, 6));
    }

    #[test]
    fn t_lt_swaps_with_the_left_neighbour() {
        let (buf, c) = cmd(T, pos(2, 6), '<').unwrap();
        assert_eq!(cells(&buf, 0), ["B", "A", "C"]);
        assert_eq!(c, pos(2, 2));
        let t3 = "| A | B | C |\n| --- | --- | --- |\n| 1 | 2 | 3 |";
        let (buf, _) = cmd(t3, pos(2, 10), '<').unwrap();
        assert_eq!(buf.line(0), "| A | C | B |");
        assert_eq!(buf.line(2), "| 1 | 3 | 2 |");
    }

    #[test]
    fn swap_carries_the_alignment() {
        let (buf, _) = cmd(
            "| A | B | C |\n| :---: | --- | ---: |\n| 1 | 2 | 3 |",
            pos(2, 10),
            '<',
        )
        .unwrap();
        assert_eq!(cells(&buf, 1), [":---:", "---:", "---"]);
    }

    // moon_edit's no-op at the edges returns the cursor unchanged; here the
    // command declines with None, so 37b records no undo entry.
    #[test]
    fn edge_swap_returns_none() {
        let buf = Buffer::from_text(T);
        let t = info(&buf, 0);
        assert_eq!(command(&t, &buf, pos(2, 10), '>'), None);
        assert_eq!(command(&t, &buf, pos(2, 2), '<'), None);
        assert_eq!(command(&t, &buf, pos(0, 0), '<'), None);
    }

    // t= (table_commands_test.dart, cycleAlignment in
    // table_operations_test.dart).

    #[test]
    fn t_eq_cycles_left_centre_right() {
        let two = "| A | B |\n| --- | --- |\n| 1 | 2 |";
        let (buf, c) = cmd(two, pos(2, 2), '=').unwrap();
        assert_eq!(buf.line(1), "| :---: | --- |");
        assert_eq!(c, pos(2, 2));
        let (buf, _) = cmd("| A | B |\n| :---: | --- |\n| 1 | 2 |", pos(2, 2), '=').unwrap();
        assert_eq!(buf.line(1), "| ---: | --- |");
        let (buf, _) = cmd("| A | B |\n| ---: | --- |\n| 1 | 2 |", pos(2, 2), '=').unwrap();
        assert_eq!(buf.line(1), "| --- | --- |");
        let (buf, _) = cmd("| A | B |\n| :--- | --- |\n| 1 | 2 |", pos(2, 2), '=').unwrap();
        assert_eq!(buf.line(1), "| :---: | --- |");
    }

    #[test]
    fn t_eq_full_cycle_from_the_separator() {
        let mut text = T.to_owned();
        for want in [":---:", "---:", "---"] {
            let (buf, c) = cmd(&text, pos(1, 2), '=').unwrap();
            assert_eq!(cells(&buf, 1)[0], want);
            assert_eq!(c, pos(1, 2));
            text = buf.to_text();
        }
    }

    #[test]
    fn t_eq_cycles_only_the_current_column() {
        let (buf, _) = cmd(T, pos(2, 6), '=').unwrap();
        assert_eq!(cells(&buf, 1), ["---", ":---:", "---"]);
        assert_eq!(buf.line(0), "| A | B | C |");
    }

    #[test]
    fn column_operations_rewrite_rows_trimmed() {
        let (buf, _) = cmd("|A|  B   |\n|---|:-:|\n|1|2|", pos(2, 1), '>').unwrap();
        assert_eq!(lines(&buf), ["| B | A |", "| :-: | --- |", "| 2 | 1 |"]);
    }

    #[test]
    fn unknown_key_and_cursor_off_the_table_decline() {
        let buf = Buffer::from_text("x\n| A |\n| --- |\n| 1 |");
        let t = info(&buf, 1);
        assert_eq!(command(&t, &buf, pos(2, 2), 'z'), None);
        assert_eq!(command(&t, &buf, pos(2, 2), 'n'), None);
        assert_eq!(command(&t, &buf, pos(0, 0), 'a'), None);
    }

    #[test]
    fn stale_info_declines_without_panic() {
        let t = info(&Buffer::from_text(T), 0);
        for text in [
            "| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |",
            "x",
            "| A |",
        ] {
            let buf = Buffer::from_text(text);
            for key in ['c', 'C', 'x', '>', '<', '='] {
                assert_eq!(command(&t, &buf, pos(0, 0), key), None, "{text:?} {key}");
            }
        }
    }

    // R12: positions in tables with clusters.

    #[test]
    fn graphemes_stay_whole() {
        let fam = "👩\u{200d}👩\u{200d}👧";
        let text = format!("| {fam} | e\u{301} |\n| --- | --- |\n| \u{301}x | {fam}y |");
        let mut buf = Buffer::from_text(&text);
        let t = info(&buf, 0);
        // The data row's first cell opens with a lone combining mark, which
        // joins the space before it: the cell start backs off to that space.
        let start = next_cell(&t, &buf, pos(0, fam.len() + 4));
        assert_eq!(start, CellMove::Move(pos(2, 1)));
        assert_eq!(cell_at(&t, &buf, pos(2, 1)), (2, 0));
        let CellMove::Move(p) = next_cell(&t, &buf, pos(2, 1)) else {
            panic!("moved");
        };
        assert_on_boundary(&buf, p);
        assert_eq!(cell_at(&t, &buf, p), (2, 1));
        assert_eq!(prev_cell(&t, &buf, p), Some(pos(2, 1)));
        // A column swap keeps every cluster and lands on a boundary.
        let plan = command(&t, &buf, p, '<').unwrap();
        apply(&mut buf, &plan);
        assert_eq!(
            lines(&buf),
            [
                format!("| e\u{301} | {fam} |"),
                "| --- | --- |".to_owned(),
                format!("| {fam}y | \u{301}x |"),
            ]
        );
        // Every command plans to boundaries.
        let buf = Buffer::from_text(&text);
        for key in ['a', 'A', 'd', 'D', 'c', 'C', 'x', '>', '='] {
            for line in 0..3 {
                for col in [0, 1, 2, 5, 9] {
                    let mut b = buf.clone();
                    if let Some(plan) = command(&t, &b, pos(line, col), key) {
                        apply(&mut b, &plan);
                    }
                }
            }
        }
    }

    #[test]
    fn incoming_cursor_is_clamped() {
        let buf = Buffer::from_text("| e\u{301} | b |\n| --- | --- |\n| c | d |");
        let t = info(&buf, 0);
        // Inside the cluster and past the end of the line.
        assert_eq!(cell_at(&t, &buf, pos(0, 3)), (0, 0));
        assert_eq!(cell_at(&t, &buf, pos(2, 99)), (2, 1));
        let p = insert(&buf, pos(9, 99), 1, 1);
        let mut b = buf.clone();
        apply(&mut b, &p);
    }

    type Planner = Box<dyn Fn(&TableInfo, &Buffer) -> Option<EditPlan>>;

    #[test]
    fn plans_apply_cleanly_and_invert() {
        let edit = |m: CellMove| match m {
            CellMove::Edit(p) => Some(p),
            CellMove::Move(_) => None,
        };
        let mut cases: Vec<(&str, Planner)> = vec![
            (
                "| A |\n| --- |\nz",
                Box::new(move |t, b| edit(next_cell(t, b, pos(0, 2)))),
            ),
            (T, Box::new(move |t, b| edit(next_row(t, b, pos(3, 2))))),
        ];
        for key in ['a', 'A', 'd', 'D', 'c', 'C', 'x', '>', '<', '='] {
            cases.push((T, Box::new(move |t, b| command(t, b, pos(2, 6), key))));
            cases.push((
                "x\n| A | B |\n|---|---|\n| 1 | 2 |\ny",
                // The last column, except for `>`, which declines there.
                Box::new(move |t, b| {
                    let col = if key == '>' { 2 } else { 7 };
                    command(t, b, pos(1, col), key)
                }),
            ));
        }
        for (text, f) in cases {
            let mut buf = Buffer::from_text(text);
            let t = info(&buf, buf.line_count().min(2) - 1);
            let plan = f(&t, &buf).expect(text);
            let mut inverses: Vec<Edit> = plan
                .edits
                .iter()
                .map(|e| buf.replace(e.range.clone(), &e.text))
                .collect();
            assert_ne!(buf.to_text(), text);
            assert_on_boundary(&buf, plan.cursor);
            while let Some(inv) = inverses.pop() {
                buf.replace(inv.range, &inv.text);
            }
            assert_eq!(buf.to_text(), text);
        }
        // insert inverts too.
        for (text, cursor) in [("", pos(0, 0)), ("a\nb", pos(0, 1))] {
            let mut buf = Buffer::from_text(text);
            let plan = insert(&buf, cursor, 2, 2);
            let inv: Vec<Edit> = plan
                .edits
                .iter()
                .map(|e| buf.replace(e.range.clone(), &e.text))
                .collect();
            for e in inv.into_iter().rev() {
                buf.replace(e.range, &e.text);
            }
            assert_eq!(buf.to_text(), text);
        }
    }

    // The grid picker (table_grid_picker.dart's hover logic).

    #[test]
    fn picker_starts_five_by_five_at_one_by_one() {
        let p = TablePicker::new();
        assert_eq!((p.cols(), p.rows()), (5, 5));
        assert_eq!(p.selected(), (0, 0));
        assert_eq!(p.size(), (1, 1));
        assert_eq!(p.label(), "1 x 1");
    }

    #[test]
    fn picker_grows_when_the_selection_reaches_an_edge() {
        let mut p = TablePicker::new();
        for _ in 0..3 {
            p.right();
        }
        assert_eq!(p.cols(), 5);
        p.right();
        assert_eq!(p.selected(), (4, 0));
        assert_eq!(p.cols(), 6);
        assert_eq!(p.rows(), 5);
        p.select(2, 4);
        assert_eq!((p.cols(), p.rows()), (6, 6));
        assert_eq!(p.label(), "3 x 5");
    }

    #[test]
    fn picker_grows_to_ten_and_inserts_c_plus_1() {
        let mut p = TablePicker::new();
        for _ in 0..20 {
            p.right();
            p.down();
        }
        assert_eq!((p.cols(), p.rows()), (10, 10));
        assert_eq!(p.selected(), (9, 9));
        assert_eq!(p.size(), (10, 10));
        assert_eq!(p.label(), "10 x 10");
        p.left();
        p.up();
        p.up();
        assert_eq!(p.size(), (9, 8));
        // A hover past the grid clamps to it.
        p.select(40, 3);
        assert_eq!(p.size(), (10, 4));
    }

    #[test]
    fn picker_shrinks_to_one_but_never_below() {
        let mut p = TablePicker::new();
        p.right();
        p.left();
        p.left();
        p.up();
        assert_eq!(p.size(), (1, 1));
        // The grid does not shrink back.
        for _ in 0..6 {
            p.right();
        }
        for _ in 0..6 {
            p.left();
        }
        assert_eq!(p.cols(), 8);
        assert_eq!(p.size(), (1, 1));
    }
}

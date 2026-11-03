//! What a renderer reads from the editor. `editor-render` depends on this
//! module and never the reverse: the editor implements `ViewSource` (Task 12)
//! and the renderer only reads through it.

use std::ops::Range;

use crate::buffer::Pos;
use crate::markdown::LineDisplay;
use crate::tokens::{Theme, Tokens};

/// The status bar's content. Task 26 extends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusModel {
    /// The mode name, for example `INSERT`.
    pub mode: &'static str,
    /// The file name as shown.
    pub file: String,
    /// True when there are changes not saved.
    pub dirty: bool,
    /// The cursor position as shown, for example `12:4`.
    pub position: String,
}

/// A selection between two source positions. Task 22 adds behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
    /// True when whole lines are selected, whatever the columns.
    pub linewise: bool,
}

impl Selection {
    /// The selection's start and end, in document order.
    pub fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The source byte range selected on `line`, or `None` when the line is
    /// outside the selection. `line_len` is the line's length in bytes.
    pub fn on_line(&self, line: usize, line_len: usize) -> Option<Range<usize>> {
        let (start, end) = self.ordered();
        if line < start.line || line > end.line {
            return None;
        }
        if self.linewise {
            return Some(0..line_len);
        }
        let from = if line == start.line { start.col } else { 0 };
        let to = if line == end.line { end.col } else { line_len };
        let (from, to) = (from.min(line_len), to.min(line_len));
        (from < to).then_some(from..to)
    }
}

/// The renderer's view of the editor.
pub trait ViewSource {
    fn line_count(&self) -> usize;
    /// Source line `i`, without its terminator.
    fn line(&self, i: usize) -> &str;
    /// How line `i` displays, given the cursor.
    fn display(&self, i: usize) -> LineDisplay;
    fn cursor(&self) -> Pos;
    fn status(&self) -> StatusModel;
    fn tokens(&self) -> &Tokens;
    fn theme(&self) -> Theme;
    /// True in reader mode (no caret).
    fn reader(&self) -> bool;
    fn selection(&self) -> Option<Selection>;
    /// Search match ranges on line `i`, as source byte ranges.
    fn matches(&self, i: usize) -> &[Range<usize>];
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn selection_on_line_covers_the_middle_lines_whole() {
        let s = Selection {
            anchor: pos(3, 2),
            head: pos(1, 4),
            linewise: false,
        };
        assert_eq!(s.on_line(0, 10), None);
        assert_eq!(s.on_line(1, 10), Some(4..10));
        assert_eq!(s.on_line(2, 10), Some(0..10));
        assert_eq!(s.on_line(3, 10), Some(0..2));
        assert_eq!(s.on_line(4, 10), None);
    }

    #[test]
    fn linewise_selection_takes_whole_lines() {
        let s = Selection {
            anchor: pos(1, 4),
            head: pos(1, 4),
            linewise: true,
        };
        assert_eq!(s.on_line(1, 7), Some(0..7));
    }

    #[test]
    fn empty_selection_selects_nothing() {
        let s = Selection {
            anchor: pos(1, 4),
            head: pos(1, 4),
            linewise: false,
        };
        assert_eq!(s.on_line(1, 7), None);
    }
}

//! What a renderer reads from the editor. `editor-render` depends on this
//! module and never the reverse: the editor implements `ViewSource` (Task 12)
//! and the renderer only reads through it.

use std::ops::Range;

use crate::buffer::{Buffer, LineShift, Pos, ShiftMark};
use crate::grapheme;
use crate::markdown::LineDisplay;
use crate::tables::TablePicker;
use crate::tokens::{Theme, Tokens};

/// How a status message reads: as information, a warning or an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Info,
    Warning,
    Error,
}

impl MsgKind {
    /// The kind of `text`: an Error in vim's error form (`E` and a digit,
    /// as in `E486: Pattern not found`), else Info. A word that merely
    /// starts with `E` is no error.
    pub fn of(text: &str) -> MsgKind {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some('E'), Some(d)) if d.is_ascii_digit() => MsgKind::Error,
            _ => MsgKind::Info,
        }
    }
}

/// What the status bar shows on its left. The editor picks one: a
/// message, then a prompt, then the line being typed, then the mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusLeft {
    /// The mode name, for example `INSERT`.
    Mode(&'static str),
    /// A line being typed in the status bar: `:` commands, `/` and `?`
    /// searches, the link prompt's `link: `. `cursor` is a byte offset
    /// into `text`.
    Input {
        prefix: &'static str,
        text: String,
        cursor: usize,
    },
    /// A question waiting for an answer, as `:s///c` asks.
    Prompt(String),
    Message {
        text: String,
        kind: MsgKind,
    },
}

/// The status bar's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusModel {
    pub left: StatusLeft,
    /// The file name as shown.
    pub file: String,
    /// True when there are changes not saved.
    pub dirty: bool,
    /// Warnings about the file on disk, shown after the file name.
    pub warnings: Vec<&'static str>,
    /// The pending count and keys, for example `2d`.
    pub showcmd: String,
    /// The cursor position as shown, `line:col`: the line from 1 and the
    /// col the grapheme display column from 1.
    pub position: String,
}

/// How the document's caret is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaretShape {
    /// A bar `size.caret` wide before the cursor (Insert).
    Bar,
    /// A block over the grapheme under the cursor (Normal, Visual).
    Block,
    /// None, while the caret is in the status bar (or in reader mode).
    Hidden,
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

    /// The source byte range selected on `line`, whose text is `text`, or
    /// `None` when the line is outside the selection. Linewise, the line is
    /// whole. Charwise, the selection takes the whole grapheme under its
    /// end (as Visual mode does), and its ends snap to grapheme boundaries;
    /// an empty line gives an empty range.
    pub fn on_line(&self, line: usize, text: &str) -> Option<Range<usize>> {
        let (start, end) = self.ordered();
        if line < start.line || line > end.line {
            return None;
        }
        let len = text.len();
        if self.linewise {
            return Some(0..len);
        }
        let from = if line == start.line {
            grapheme::floor(text, start.col)
        } else {
            0
        };
        let to = if line == end.line {
            grapheme::next(text, grapheme::floor(text, end.col))
        } else {
            len
        };
        Some(from..to.max(from))
    }

    /// The source byte range selected on each line, for paint, in line
    /// order: `on_line` for every line from the start to the end (clamped
    /// to the buffer).
    pub fn ranges(&self, buf: &Buffer) -> Vec<(usize, Range<usize>)> {
        let (start, end) = self.ordered();
        let (start, end) = (buf.clamp(start), buf.clamp(end));
        let sel = Selection {
            anchor: start,
            head: end,
            linewise: self.linewise,
        };
        (start.line..=end.line)
            .filter_map(|i| sel.on_line(i, buf.line(i)).map(|r| (i, r)))
            .collect()
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
    /// True in reader mode: no caret, nothing revealed.
    fn reader(&self) -> bool;
    /// While the help overlay shows, the first of `help::rows` it shows.
    fn help(&self) -> Option<usize> {
        None
    }
    /// The grid picker `:table` opens, while it shows, drawn under the
    /// cursor's line.
    fn picker(&self) -> Option<&TablePicker> {
        None
    }
    fn selection(&self) -> Option<Selection>;
    /// Search match ranges on line `i`, as source byte ranges.
    fn matches(&self, i: usize) -> &[Range<usize>];
    /// The match the cursor is on (the search's, or the one `:s///c` asks
    /// about), as a line and its source byte range.
    fn current_match(&self) -> Option<(usize, Range<usize>)>;
    /// How the caret is drawn.
    fn caret_shape(&self) -> CaretShape;
    /// Text an input method is composing at the cursor, in the document,
    /// and its cursor (a byte offset into it). It is not in the buffer: the
    /// renderer draws it at the caret, underlined.
    fn preedit(&self) -> Option<(&str, usize)> {
        None
    }
    /// The line shifts since `since` (with `None`, only the current mark),
    /// so the renderer can move its per-line state with the lines. `None`
    /// when the source does not report shifts: the renderer then resizes
    /// at the end, as it always has.
    fn line_shifts_since(&self, since: Option<ShiftMark>) -> Option<LineShifts> {
        let _ = since;
        None
    }
}

/// What `ViewSource::line_shifts_since` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineShifts {
    /// Where the source's edit history is now.
    pub mark: ShiftMark,
    /// The shifts since the mark asked about, oldest first. `None` when
    /// they are unknown: no mark, a mark from another buffer, or one older
    /// than the buffer's log.
    pub shifts: Option<Vec<LineShift>>,
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
        let text = "0123456789";
        assert_eq!(s.on_line(0, text), None);
        assert_eq!(s.on_line(1, text), Some(4..10));
        assert_eq!(s.on_line(2, text), Some(0..10));
        // The end takes the grapheme under it.
        assert_eq!(s.on_line(3, text), Some(0..3));
        assert_eq!(s.on_line(4, text), None);
    }

    #[test]
    fn linewise_selection_takes_whole_lines() {
        let s = Selection {
            anchor: pos(1, 4),
            head: pos(1, 4),
            linewise: true,
        };
        assert_eq!(s.on_line(1, "abcdefg"), Some(0..7));
    }

    #[test]
    fn a_fresh_selection_selects_the_grapheme_under_it() {
        let s = Selection {
            anchor: pos(1, 1),
            head: pos(1, 1),
            linewise: false,
        };
        assert_eq!(s.on_line(1, "ae\u{301}x"), Some(1..4));
        assert_eq!(s.on_line(1, ""), Some(0..0));
    }

    #[test]
    fn only_vim_error_forms_are_errors() {
        assert_eq!(MsgKind::of("E486: Pattern not found: x"), MsgKind::Error);
        assert_eq!(MsgKind::of("E37: No write"), MsgKind::Error);
        assert_eq!(MsgKind::of("Esc pressed"), MsgKind::Info);
        assert_eq!(MsgKind::of("E"), MsgKind::Info);
        assert_eq!(MsgKind::of("3 substitutions on 1 line"), MsgKind::Info);
    }
}

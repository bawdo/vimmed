//! The editing mode and how the status bar shows it and the cursor.

use crate::buffer::{Buffer, Pos};
use crate::grapheme;
use crate::strings;

/// The editing mode. The vim engine (Task 19 onward) adds the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
}

impl Mode {
    /// The name shown in the status bar.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Normal => strings::MODE_NORMAL,
            Mode::Insert => strings::MODE_INSERT,
        }
    }
}

/// The cursor as shown, `line:column`, both 1-based, counting graphemes.
pub fn position(buf: &Buffer, cursor: Pos) -> String {
    let col = grapheme::display_col(buf.line(cursor.line), cursor.col);
    format!("{}:{}", cursor.line + 1, col + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_are_verbatim() {
        assert_eq!(Mode::Normal.name(), "NORMAL");
        assert_eq!(Mode::Insert.name(), "INSERT");
    }

    #[test]
    fn position_counts_graphemes_from_one() {
        let buf = Buffer::from_text("a\ne\u{301}x");
        assert_eq!(position(&buf, Pos { line: 1, col: 3 }), "2:2");
        assert_eq!(position(&buf, Pos { line: 0, col: 0 }), "1:1");
    }
}

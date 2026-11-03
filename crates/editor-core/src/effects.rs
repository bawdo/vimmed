//! What handling one input asks of the shell.

use std::ops::Range;

/// What to repaint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Redraw {
    #[default]
    None,
    /// These source lines (in the buffer after the input). The renderer's
    /// `paint_lines` also repaints the status bar when its model changed, so
    /// `Lines` covers `Status`.
    Lines(Range<usize>),
    /// Only the status bar.
    Status,
    All,
}

impl Redraw {
    /// Both repaints as one: line ranges join into the range spanning both,
    /// `All` covers everything and `Lines` covers `Status`.
    pub fn merge(self, other: Redraw) -> Redraw {
        match (self, other) {
            (Redraw::All, _) | (_, Redraw::All) => Redraw::All,
            (Redraw::None, r) | (r, Redraw::None) => r,
            (Redraw::Lines(a), Redraw::Lines(b)) => {
                Redraw::Lines(a.start.min(b.start)..a.end.max(b.end))
            }
            (Redraw::Lines(r), Redraw::Status) | (Redraw::Status, Redraw::Lines(r)) => {
                Redraw::Lines(r)
            }
            (Redraw::Status, Redraw::Status) => Redraw::Status,
        }
    }
}

/// The result of `Editor::handle`. Every field left at its default asks
/// nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    pub redraw: Redraw,
    /// A new window title.
    pub title: Option<String>,
    /// The dirty state, when it changed.
    pub dirty: Option<bool>,
    /// Close the window and exit with this code.
    pub close: Option<i32>,
    /// Scroll so the cursor shows.
    pub scroll_to_cursor: bool,
    /// Reader mode, when it changed.
    pub reader: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_none_yields_the_other() {
        assert_eq!(Redraw::None.merge(Redraw::Status), Redraw::Status);
        assert_eq!(Redraw::Lines(2..3).merge(Redraw::None), Redraw::Lines(2..3));
        assert_eq!(Redraw::None.merge(Redraw::None), Redraw::None);
    }

    #[test]
    fn merge_lines_spans_both_ranges() {
        assert_eq!(
            Redraw::Lines(5..6).merge(Redraw::Lines(1..2)),
            Redraw::Lines(1..6)
        );
    }

    #[test]
    fn merge_all_wins_and_lines_cover_status() {
        assert_eq!(Redraw::Lines(1..2).merge(Redraw::All), Redraw::All);
        assert_eq!(Redraw::All.merge(Redraw::Status), Redraw::All);
        assert_eq!(
            Redraw::Status.merge(Redraw::Lines(1..2)),
            Redraw::Lines(1..2)
        );
        assert_eq!(Redraw::Status.merge(Redraw::Status), Redraw::Status);
    }
}

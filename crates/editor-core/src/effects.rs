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

/// A scroll of the view that leaves the cursor where it is (reader view's
/// keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// This many source lines down (up when negative).
    Lines(i32),
    /// This many heights of the text area down (up when negative).
    Pages(i32),
    /// To the document's start.
    Top,
    /// To the document's end.
    Bottom,
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
    /// Scroll the view, leaving the cursor where it is.
    pub scroll: Option<Scroll>,
    /// Wake me at this time (`marks::now_ns` clock): the shell arms a timer
    /// that feeds `Input::Tick` then, so a timed message can go.
    pub wake_at: Option<u64>,
}

impl Effects {
    /// Two results as one, `other` after `self`: the redraws merge, the
    /// later value of each option wins, the scroll requests combine and
    /// the earlier wake time stands.
    pub fn merge(self, other: Effects) -> Effects {
        Effects {
            redraw: self.redraw.merge(other.redraw),
            title: other.title.or(self.title),
            dirty: other.dirty.or(self.dirty),
            close: other.close.or(self.close),
            scroll_to_cursor: self.scroll_to_cursor || other.scroll_to_cursor,
            reader: other.reader.or(self.reader),
            scroll: other.scroll.or(self.scroll),
            wake_at: match (self.wake_at, other.wake_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
        }
    }
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

    #[test]
    fn effects_merge_keeps_the_later_values() {
        let a = Effects {
            redraw: Redraw::Lines(1..2),
            scroll: Some(Scroll::Lines(2)),
            title: Some("a".into()),
            dirty: Some(true),
            scroll_to_cursor: true,
            wake_at: Some(9),
            ..Effects::default()
        };
        let b = Effects {
            redraw: Redraw::Status,
            title: Some("b".into()),
            close: Some(0),
            reader: Some(false),
            scroll: Some(Scroll::Top),
            wake_at: Some(4),
            ..Effects::default()
        };
        assert_eq!(
            a.merge(b),
            Effects {
                redraw: Redraw::Lines(1..2),
                title: Some("b".into()),
                dirty: Some(true),
                close: Some(0),
                scroll_to_cursor: true,
                reader: Some(false),
                scroll: Some(Scroll::Top),
                wake_at: Some(4),
            }
        );
    }
}

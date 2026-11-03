//! Start-up decisions that need no window: when `first_frame` is marked,
//! and how big the first window is.

/// `first_frame` means: the first display-link tick that committed and
/// flushed a frame, with the run loop live, at or after the window being
/// key. Commits made outside a tick (a resize) neither mark nor suppress
/// the mark.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FirstFrame {
    key: bool,
    marked: bool,
}

impl FirstFrame {
    /// The window became key. Returns true when the shell must force one
    /// more tick that commits (the mark is still owed).
    pub fn became_key(&mut self) -> bool {
        self.key = true;
        !self.marked
    }

    /// A tick committed and flushed. Returns true exactly once: when
    /// `first_frame` should be marked now.
    pub fn tick_committed(&mut self) -> bool {
        if self.marked || !self.key {
            return false;
        }
        self.marked = true;
        true
    }

    pub fn is_marked(&self) -> bool {
        self.marked
    }
}

/// The content size to open at: `want`, clamped to `max` (the screen's
/// visible frame less the title bar), so AppKit never has to constrain the
/// window as it goes on screen.
pub fn fit_content(want: (f64, f64), max: (f64, f64)) -> (f64, f64) {
    (want.0.min(max.0), want.1.min(max.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_before_any_tick_marks_on_the_first_tick() {
        let mut f = FirstFrame::default();
        assert!(f.became_key());
        assert!(f.tick_committed());
        assert!(f.is_marked());
        assert!(!f.tick_committed(), "marks exactly once");
        assert!(!f.became_key(), "no tick owed once marked");
    }

    #[test]
    fn geometry_commit_before_key_does_not_suppress_the_mark() {
        let mut f = FirstFrame::default();
        // A resize commits outside a tick: the shell tells the machine
        // nothing. Then the first tick runs before the window is key.
        assert!(!f.tick_committed());
        assert!(f.became_key(), "a tick is owed");
        assert!(f.tick_committed());
    }

    #[test]
    fn key_before_geometry_commit_still_marks_on_the_tick() {
        let mut f = FirstFrame::default();
        assert!(f.became_key());
        // Resize commit here: nothing reported. The next tick marks.
        assert!(f.tick_committed());
    }

    #[test]
    fn key_late_after_several_ticks() {
        let mut f = FirstFrame::default();
        for _ in 0..3 {
            assert!(!f.tick_committed());
        }
        assert!(f.became_key());
        assert!(f.tick_committed());
        assert!(!f.tick_committed());
    }

    #[test]
    fn never_key_never_marks() {
        let mut f = FirstFrame::default();
        for _ in 0..10 {
            assert!(!f.tick_committed());
        }
        assert!(!f.is_marked());
    }

    #[test]
    fn window_fits_the_visible_frame() {
        assert_eq!(
            fit_content((900.0, 1000.0), (1440.0, 840.0)),
            (900.0, 840.0)
        );
        assert_eq!(
            fit_content((900.0, 1000.0), (2560.0, 1400.0)),
            (900.0, 1000.0)
        );
        assert_eq!(fit_content((900.0, 1000.0), (800.0, 600.0)), (800.0, 600.0));
    }
}

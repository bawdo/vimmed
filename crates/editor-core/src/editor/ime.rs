//! Input methods: committed text (`Input::Text`) and the text an input
//! method is still composing (`Input::Preedit`).
//!
//! An input method is active only where text is typed: Insert (with vim
//! off, always), the command line and a search. Never in Normal, Visual,
//! Confirm, Prompt or reader mode, so `j` there is a key with no input
//! method in the way. The shells ask `ime_active` before every key.
//!
//! Preedit never touches the buffer: it is held here and drawn at the
//! caret (underlined, by the renderer) until the committed `Text` arrives,
//! or until the input method or a mode change clears it.

use crate::effects::{Effects, Redraw};
use crate::input::Input;
use crate::status::Mode;

use super::Editor;

/// `text` with every `\r\n` and lone `\r` as `\n`. Input method commits
/// and pastes can carry either.
pub(super) fn normalise_newlines(text: String) -> String {
    if !text.contains('\r') {
        return text;
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

impl Editor {
    /// True while an input method may compose: Insert (or vim off), the
    /// command line and a search. Never in reader mode, and never while a
    /// prompt asks: its answers are single keys.
    pub fn ime_active(&self) -> bool {
        if self.reader || self.mode == Mode::Prompt {
            return false;
        }
        !self.cfg.vim || matches!(self.mode, Mode::Insert | Mode::Command | Mode::Search)
    }

    /// The text being composed and its cursor (a byte offset into it).
    pub fn preedit(&self) -> Option<(&str, usize)> {
        self.preedit.as_ref().map(|(t, c)| (t.as_str(), *c))
    }

    /// `Input::Preedit`: held (not typed) while an input method is active,
    /// and dropped when `text` is empty. The cursor snaps back to a char
    /// boundary inside `text`.
    pub(super) fn set_preedit(&mut self, text: String, cursor: usize) -> Effects {
        if !self.ime_active() {
            return self.clear_preedit();
        }
        if text.is_empty() {
            return self.clear_preedit();
        }
        let mut cursor = cursor.min(text.len());
        while !text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        let next = Some((text, cursor));
        if self.preedit == next {
            return Effects::default();
        }
        self.preedit = next;
        self.preedit_redraw()
    }

    /// Accepts the composition as if it had been typed: `Input::Text` with
    /// its text (newlines normalised, the same mode guard). AppKit ends a
    /// composition this way (`unmarkText` on an input source switch or
    /// focus loss). Nothing without a composition; outside an input mode a
    /// stray one is dropped, never typed.
    pub fn commit_preedit(&mut self) -> Effects {
        let Some((text, _)) = self.preedit.clone() else {
            return Effects::default();
        };
        if !self.ime_active() {
            return self.clear_preedit();
        }
        self.handle(Input::Text(text))
    }

    /// Drops the preedit, repainting where it showed.
    pub(super) fn clear_preedit(&mut self) -> Effects {
        if self.preedit.take().is_none() {
            return Effects::default();
        }
        self.preedit_redraw()
    }

    /// Where a preedit shows: the cursor's line in the document, or the
    /// status bar for the command line or a search.
    fn preedit_redraw(&self) -> Effects {
        let redraw = match self.mode {
            Mode::Command | Mode::Search if self.cfg.vim || self.linking() => Redraw::Status,
            _ => {
                let line = self
                    .cursor
                    .line
                    .min(self.buf.line_count().saturating_sub(1));
                Redraw::Lines(line..line + 1)
            }
        };
        Effects {
            redraw,
            ..Effects::default()
        }
    }

    /// After any input: a preedit left over once the input method is no
    /// longer active (Esc out of Insert, Enter on the command line) goes,
    /// repainting the line it showed on.
    pub(super) fn drop_stale_preedit(&mut self, line: usize) -> Effects {
        if self.ime_active() || self.preedit.take().is_none() {
            return Effects::default();
        }
        let line = line.min(self.buf.line_count().saturating_sub(1));
        Effects {
            redraw: Redraw::Lines(line..line + 1),
            ..Effects::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::effects::Redraw;
    use crate::input::Input;
    use crate::status::Mode;
    use crate::testkit::{ViewSource, build_reader, ed, keys, pos};
    use crate::view_model::StatusLeft;

    fn preedit(text: &str, cursor: usize) -> Input {
        Input::Preedit {
            text: text.into(),
            cursor,
        }
    }

    #[test]
    fn commit_preedit_types_the_composition() {
        let mut e = ed("ab");
        keys(&mut e, "li");
        e.handle(preedit("にほ", 3));
        let fx = e.commit_preedit();
        assert_eq!(e.text(), "aにほb");
        assert_eq!(e.preedit(), None);
        assert_eq!(e.cursor(), pos(0, 7));
        assert!(e.dirty());
        assert_eq!(fx.dirty, Some(true));
        assert!(matches!(fx.redraw, Redraw::Lines(r) if r.contains(&0)));
        // Already committed: a second call does nothing.
        assert_eq!(e.commit_preedit(), crate::effects::Effects::default());
        assert_eq!(e.text(), "aにほb");
    }

    #[test]
    fn commit_preedit_normalises_carriage_returns() {
        let mut e = ed("");
        keys(&mut e, "i");
        e.handle(preedit("a\r\nb\rc", 1));
        e.commit_preedit();
        assert_eq!(e.text(), "a\nb\nc");
    }

    #[test]
    fn commit_preedit_on_the_command_line_appends_to_it() {
        let mut e = ed("abc");
        keys(&mut e, ":s");
        e.handle(preedit("ü", 2));
        e.commit_preedit();
        assert_eq!(e.cmdline(), "sü");
        assert_eq!(e.preedit(), None);
        assert_eq!(e.text(), "abc");
    }

    #[test]
    fn commit_preedit_without_one_or_outside_an_input_mode_types_nothing() {
        let mut e = ed("abc");
        assert_eq!(e.commit_preedit(), crate::effects::Effects::default());
        // A stray composition in Normal (it cannot get there through
        // `handle`) is dropped, not run as keys.
        e.preedit = Some(("x".into(), 1));
        e.commit_preedit();
        assert_eq!(e.text(), "abc");
        assert_eq!(e.preedit(), None);
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn text_input_inserts_in_insert_mode() {
        let mut e = ed("ab");
        keys(&mut e, "li");
        e.handle(Input::Text("日本".into()));
        assert_eq!(e.text(), "a日本b");
        assert_eq!(e.cursor(), pos(0, 7));
        assert_eq!(e.mode(), Mode::Insert);
    }

    #[test]
    fn text_input_normalises_carriage_returns() {
        let mut e = ed("");
        keys(&mut e, "i");
        e.handle(Input::Text("a\r\nb\rc\nd".into()));
        assert_eq!(e.text(), "a\nb\nc\nd");
        assert_eq!(e.cursor(), pos(3, 1));
    }

    #[test]
    fn text_input_ignored_in_normal() {
        // Committed text in Normal is a run of keys, never typing: `é` and
        // `zz` mean nothing there, so nothing changes.
        let mut e = ed("abc");
        let fx = e.handle(Input::Text("é".into()));
        assert_eq!(e.text(), "abc");
        assert_eq!(e.mode(), Mode::Normal);
        assert!(!e.dirty());
        assert_eq!(fx.dirty, None);
        e.handle(Input::Text("zz".into()));
        assert_eq!(e.text(), "abc");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn text_input_ignored_in_reader_mode() {
        let mut e = build_reader("abc");
        e.handle(Input::Text("ixyz".into()));
        assert_eq!(e.text(), "abc");
        assert!(!e.dirty());
    }

    #[test]
    fn preedit_shows_without_editing_buffer() {
        let mut e = ed("ab");
        keys(&mut e, "li");
        let fx = e.handle(preedit("にほ", 3));
        assert_eq!(e.text(), "ab");
        assert!(!e.dirty());
        assert_eq!(e.preedit(), Some(("にほ", 3)));
        assert_eq!(ViewSource::preedit(&e), Some(("にほ", 3)));
        assert_eq!(e.cursor(), pos(0, 1));
        assert_eq!(fx.redraw, Redraw::Lines(0..1));
        assert_eq!(fx.dirty, None);
        // The commit types and ends the composition.
        let fx = e.handle(Input::Text("日本".into()));
        assert_eq!(e.text(), "a日本b");
        assert_eq!(e.preedit(), None);
        assert_eq!(fx.redraw, Redraw::Lines(0..1));
    }

    #[test]
    fn empty_preedit_clears_it() {
        let mut e = ed("ab");
        keys(&mut e, "i");
        e.handle(preedit("x", 1));
        let fx = e.handle(preedit("", 0));
        assert_eq!(e.preedit(), None);
        assert_eq!(fx.redraw, Redraw::Lines(0..1));
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn preedit_cursor_snaps_to_a_char_boundary() {
        let mut e = ed("");
        keys(&mut e, "i");
        e.handle(preedit("日本", 4));
        assert_eq!(e.preedit(), Some(("日本", 3)));
        e.handle(preedit("日本", 99));
        assert_eq!(e.preedit(), Some(("日本", 6)));
    }

    #[test]
    fn preedit_ignored_outside_input_modes() {
        let mut e = ed("abc");
        let fx = e.handle(preedit("x", 1));
        assert_eq!(e.preedit(), None);
        assert_eq!(fx.redraw, Redraw::None);
        keys(&mut e, "v");
        e.handle(preedit("x", 1));
        assert_eq!(e.preedit(), None);
        let mut r = build_reader("abc");
        r.handle(preedit("x", 1));
        assert_eq!(r.preedit(), None);
    }

    #[test]
    fn leaving_insert_drops_the_preedit() {
        let mut e = ed("ab\ncd");
        keys(&mut e, "ji");
        e.handle(preedit("x", 1));
        let fx = keys(&mut e, "<Esc>");
        assert_eq!(e.preedit(), None);
        assert_eq!(e.mode(), Mode::Normal);
        assert!(matches!(fx.redraw, Redraw::Lines(r) if r.contains(&1)));
    }

    #[test]
    fn ime_is_active_only_where_text_is_typed() {
        let mut e = ed("abc");
        assert!(!e.ime_active());
        keys(&mut e, "i");
        assert!(e.ime_active());
        keys(&mut e, "<Esc>v");
        assert!(!e.ime_active());
        keys(&mut e, "<Esc>:");
        assert!(e.ime_active());
        keys(&mut e, "<Esc>/");
        assert!(e.ime_active());
        keys(&mut e, "<Esc>");
        assert!(!e.ime_active());
        assert!(!build_reader("abc").ime_active());
    }

    #[test]
    fn preedit_on_the_command_line_shows_in_the_status_bar() {
        let mut e = ed("abc");
        keys(&mut e, ":s");
        let fx = e.handle(preedit("ü", 2));
        assert_eq!(fx.redraw, Redraw::Status);
        assert_eq!(e.cmdline(), "s");
        // Not in the document: the status bar shows it.
        assert_eq!(ViewSource::preedit(&e), None);
        assert_eq!(
            e.status().left,
            StatusLeft::Input {
                prefix: ":",
                text: "sü".into(),
                cursor: 3,
            }
        );
        // Leaving the command line drops it.
        keys(&mut e, "<Esc>");
        assert_eq!(e.preedit(), None);
    }
}

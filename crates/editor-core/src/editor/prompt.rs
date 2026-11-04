//! Prompt mode: a question in the status bar answered with one key, such
//! as `unsaved changes: [s]ave [d]iscard [c]ancel` when the window is
//! closed with unsaved changes. Esc is the same as cancel; any other key
//! waits for an answer. The input method is off while a prompt asks, so
//! the answers arrive as plain keys (committed text is taken a char at a
//! time all the same).
//!
//! What is being asked is an `Ask`; each kind maps its answer keys to what
//! they do. Cancelling goes back to the mode the prompt was asked from.

use crate::effects::{Effects, Redraw};
use crate::input::{Key, KeyEvent};
use crate::status::Mode;
use crate::strings;

use super::Editor;

/// What a prompt asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ask {
    /// The window was asked to close with unsaved changes.
    Close,
}

impl Ask {
    fn question(self) -> &'static str {
        match self {
            Ask::Close => strings::UNSAVED_CHANGES_PROMPT,
        }
    }
}

/// A prompt being asked, and the mode to go back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Prompt {
    ask: Ask,
    ret: Mode,
}

impl Prompt {
    pub(super) fn question(&self) -> &'static str {
        self.ask.question()
    }
}

impl Editor {
    /// Asks `ask` in the status bar. A message showing makes way for it,
    /// and a composition in progress is dropped.
    pub(super) fn ask(&mut self, ask: Ask) -> Effects {
        if self.asking.is_some() {
            return Effects::default();
        }
        let fx = self.clear_preedit();
        self.unsay();
        self.asking = Some(Prompt {
            ask,
            ret: self.mode,
        });
        self.mode = Mode::Prompt;
        fx.merge(Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        })
    }

    /// A key while a prompt asks: an answer (either case), or Esc to
    /// cancel. Other keys wait.
    pub(super) fn prompt_key(&mut self, k: KeyEvent) -> Effects {
        match k.key {
            Key::Esc => self.answer('c'),
            Key::Char(c) => self.answer(c.to_ascii_lowercase()),
            _ => Effects::default(),
        }
    }

    /// Committed text while a prompt asks: each char is a key.
    pub(super) fn prompt_text(&mut self, text: &str) -> Effects {
        let mut fx = Effects::default();
        for c in text.chars() {
            if self.asking.is_none() {
                break;
            }
            fx = fx.merge(self.answer(c.to_ascii_lowercase()));
        }
        fx
    }

    fn answer(&mut self, c: char) -> Effects {
        let Some(p) = self.asking else {
            return Effects::default();
        };
        match (p.ask, c) {
            (Ask::Close, 's') => {
                let fx = self.end_prompt(p);
                match self.write_step(false) {
                    (true, w) => fx.merge(w).merge(self.close(0)),
                    (false, w) => fx.merge(w),
                }
            }
            (Ask::Close, 'd') => {
                let fx = self.end_prompt(p);
                fx.merge(self.close(0))
            }
            (Ask::Close, 'c') => self.end_prompt(p),
            _ => Effects::default(),
        }
    }

    /// The prompt is answered: back to the mode it was asked from.
    fn end_prompt(&mut self, p: Prompt) -> Effects {
        self.asking = None;
        self.mode = p.ret;
        Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::input::Input;
    use crate::status::Mode;
    use crate::strings;
    use crate::testkit::{FileCall, ViewSource, ed_with_files, keys, plain_with_host};
    use crate::view_model::StatusLeft;

    fn asks(e: &crate::editor::Editor) -> bool {
        e.status().left == StatusLeft::Prompt(strings::UNSAVED_CHANGES_PROMPT.into())
    }

    #[test]
    fn close_request_clean_closes_with_0() {
        let (mut e, files) = ed_with_files("hello");
        assert_eq!(e.handle(Input::CloseRequested).close, Some(0));
        assert_eq!(files.calls(), [FileCall::Close { code: 0 }]);
    }

    #[test]
    fn close_request_dirty_prompts() {
        let (mut e, files) = ed_with_files("hello");
        keys(&mut e, "x");
        let fx = e.handle(Input::CloseRequested);
        assert_eq!(fx.close, None);
        assert_ne!(fx.redraw, crate::effects::Redraw::None);
        assert_eq!(e.mode(), Mode::Prompt);
        assert!(asks(&e));
        assert!(!e.ime_active());
        // Asking again changes nothing; other keys wait for an answer.
        assert_eq!(e.handle(Input::CloseRequested).close, None);
        keys(&mut e, "xj<CR>");
        assert!(asks(&e));
        assert_eq!(e.text(), "ello");
        assert!(files.calls().is_empty());
    }

    #[test]
    fn c_and_esc_cancel_back_to_the_mode_before() {
        let (mut e, files) = ed_with_files("hello");
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        keys(&mut e, "c");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.prompt(), None);
        assert!(e.dirty());
        keys(&mut e, "i");
        e.handle(Input::CloseRequested);
        keys(&mut e, "<Esc>");
        assert_eq!(e.mode(), Mode::Insert);
        assert!(files.calls().is_empty());
    }

    #[test]
    fn d_discards_and_closes() {
        let (mut e, files) = ed_with_files("hello");
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        assert_eq!(keys(&mut e, "D").close, Some(0));
        assert_eq!(files.calls(), [FileCall::Close { code: 0 }]);
        assert_eq!(files.disk(), "hello");
    }

    #[test]
    fn s_saves_then_closes() {
        let (mut e, files) = ed_with_files("hello");
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        assert_eq!(keys(&mut e, "s").close, Some(0));
        let wrote_and_closed = [
            FileCall::Write { force: false },
            FileCall::Close { code: 0 },
        ];
        assert_eq!(files.calls(), wrote_and_closed);
        assert_eq!(files.disk(), "ello");
    }

    #[test]
    fn a_failed_save_stays_open_and_says_why() {
        let (mut e, files) = ed_with_files("hello");
        files.fail_writes();
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        assert_eq!(keys(&mut e, "s").close, None);
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.message(), Some("write failed: No space left on device"));
        assert!(e.dirty());
    }

    #[test]
    fn committed_text_answers() {
        let (mut e, files) = ed_with_files("hello");
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        assert_eq!(e.handle(Input::Text("d".into())).close, Some(0));
        assert_eq!(files.calls(), [FileCall::Close { code: 0 }]);
    }

    #[test]
    fn vim_off_prompts_too() {
        let (mut e, _host) = plain_with_host("hello");
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        assert!(asks(&e));
        assert!(!e.ime_active());
        keys(&mut e, "c");
        assert_eq!(e.text(), "xhello");
        assert_eq!(e.handle(Input::Text("y".into())).close, None);
        assert_eq!(e.text(), "xyhello");
    }
}

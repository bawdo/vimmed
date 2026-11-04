//! Prompt mode: a question in the status bar answered with one key, such
//! as `unsaved changes: [s]ave [d]iscard [c]ancel` when the window is
//! closed with unsaved changes, the already-open prompt (`locking`)
//! and the recovery prompt (`recovering`) at open, or, with vim off,
//! `file changed on disk: [o]verwrite [r]eload [c]ancel` when Primary+S
//! would write over a file changed on disk (`nonvim_keys`). Esc is the
//! close and changed-on-disk prompts' cancel; the prompts at open have no cancel, so Esc does
//! nothing there. Any other key waits for an
//! answer. The input method is off while a prompt asks, so the answers
//! arrive as plain keys; committed text of one char answers like the key,
//! and longer text is no answer.
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
    /// At open, another live window (process `pid`) has the file open.
    AlreadyOpen { pid: u32 },
    /// At open, a window that is gone left a recovery copy newer than the
    /// file (`recovering`).
    Recovery,
    /// With vim off, Primary+S found the file changed on disk: `o` is
    /// `:w!`, `r` is `:e!`.
    ChangedOnDisk,
}

impl Ask {
    fn question(self) -> String {
        match self {
            Ask::Close => strings::UNSAVED_CHANGES_PROMPT.to_owned(),
            Ask::AlreadyOpen { pid } => strings::already_open(pid),
            Ask::Recovery => strings::RECOVERY_PROMPT.to_owned(),
            Ask::ChangedOnDisk => strings::CHANGED_ON_DISK_PROMPT.to_owned(),
        }
    }

    /// The answer Esc gives, if any. The prompts at open have none, so
    /// they cannot be dismissed into editing without a choice.
    fn esc(self) -> Option<char> {
        match self {
            Ask::Close | Ask::ChangedOnDisk => Some('c'),
            Ask::AlreadyOpen { .. } | Ask::Recovery => None,
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
    pub(super) fn question(&self) -> String {
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

    /// A key while a prompt asks: an answer (either case), or Esc (which
    /// cancels, where the prompt has a cancel). Other keys, and keys with
    /// Ctrl, Alt or Cmd, wait.
    pub(super) fn prompt_key(&mut self, k: KeyEvent) -> Effects {
        // Only a plain key (Shift allowed) answers: Primary+D must not
        // discard.
        if k.mods.ctrl || k.mods.alt || k.mods.cmd {
            return Effects::default();
        }
        match k.key {
            Key::Esc => match self.asking.and_then(|p| p.ask.esc()) {
                Some(c) => self.answer(c),
                None => Effects::default(),
            },
            Key::Char(c) => self.answer(c.to_ascii_lowercase()),
            _ => Effects::default(),
        }
    }

    /// Committed text while a prompt asks (an input method's commit, or
    /// a composition `commit_preedit` committed): one char answers like
    /// the key; anything longer is no answer, and is dropped.
    pub(super) fn prompt_text(&mut self, text: &str) -> Effects {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => self.answer(c.to_ascii_lowercase()),
            _ => Effects::default(),
        }
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
            (Ask::Close | Ask::ChangedOnDisk, 'c') => self.end_prompt(p),
            (Ask::ChangedOnDisk, 'o') => {
                let fx = self.end_prompt(p);
                fx.merge(self.write_step(true).1)
            }
            (Ask::ChangedOnDisk, 'r') => {
                let fx = self.end_prompt(p);
                fx.merge(self.reload())
            }
            // Then the recovery prompt, if a copy was found (`recovering`).
            (Ask::AlreadyOpen { .. }, 'o') => {
                let fx = self.end_prompt(p);
                self.open_read_only();
                self.offer_recovery();
                fx
            }
            (Ask::AlreadyOpen { .. }, 'e') => {
                let fx = self.end_prompt(p);
                self.offer_recovery();
                fx
            }
            (Ask::AlreadyOpen { .. }, 'q') => {
                let fx = self.end_prompt(p);
                fx.merge(self.close(0))
            }
            (Ask::Recovery, 'r') => {
                let fx = self.end_prompt(p);
                let fx = fx.merge(self.restore_recovery());
                self.say_held_lock_warning();
                fx
            }
            (Ask::Recovery, 'd') => {
                let fx = self.end_prompt(p);
                self.discard_found();
                self.say_held_lock_warning();
                fx
            }
            // The prompt stays until `r` or `d`.
            (Ask::Recovery, 'c') => {
                self.compare_recovery();
                Effects::default()
            }
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
    fn modified_keys_do_not_answer() {
        let (mut e, files) = ed_with_files("hello");
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        assert_eq!(keys(&mut e, "<D-d><D-s><C-s><C-c>").close, None);
        assert!(asks(&e));
        assert!(files.calls().is_empty());
        // Shift alone is still an answer.
        let shift_d = crate::input::Mods {
            shift: true,
            ..crate::input::Mods::default()
        };
        let fx = e.handle(crate::testkit::key_with(
            crate::input::Key::Char('D'),
            shift_d,
        ));
        assert_eq!(fx.close, Some(0));
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
    fn a_committed_composition_answers_only_as_one_char() {
        let (mut e, files) = ed_with_files("hello");
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        // More than one char is no answer: `dc` neither discards nor
        // cancels.
        assert_eq!(e.handle(Input::Text("dc".into())).close, None);
        assert!(asks(&e));
        // A composition left over when the prompt came up is committed as
        // one char, which answers like the key.
        e.preedit = Some(("d".into(), 1));
        assert_eq!(e.commit_preedit().close, Some(0));
        assert_eq!(e.preedit(), None);
        assert_eq!(files.calls(), [FileCall::Close { code: 0 }]);
        // A longer one is dropped, never typed.
        let (mut e, files) = ed_with_files("hello");
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        e.preedit = Some(("にほ".into(), 6));
        assert_eq!(e.commit_preedit().close, None);
        assert_eq!(e.preedit(), None);
        assert!(asks(&e));
        assert_eq!(e.text(), "ello");
        assert!(files.calls().is_empty());
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

//! The command line (`:`) and what it runs: typing it, `crate::ex`
//! parsing it on Enter, `:w`, `:q` and `:e!` through the `Saver`, and
//! `:s` (with `crate::substitute` finding the replacements), including the
//! Confirm mode `:s///c` asks in.

use crate::buffer::{Buffer, Pos};
use crate::effects::{Effects, Redraw};
use crate::ex::{self, ExCmd, Sub};
use crate::grapheme;
use crate::input::{Key, KeyEvent};
use crate::search::{Hit, Search};
use crate::status::{self, Mode};
use crate::strings;
use crate::substitute::{self, Answer, ConfirmSession};
use crate::view_model::MsgKind;
use crate::vim;
use crate::vim::operator::OpOutcome;

use super::Editor;

impl Editor {
    /// A key on the command line. Typing adds to it and Backspace takes
    /// its last grapheme off; on an empty line Backspace leaves, as Esc
    /// does (vim 9.1). Enter runs the command. Other keys do nothing.
    pub(super) fn command_key(&mut self, k: KeyEvent) -> Effects {
        let status = Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        };
        match k.key {
            Key::Esc => self.leave_cmdline(),
            Key::Backspace if self.cmdline.is_empty() => self.leave_cmdline(),
            Key::Backspace => {
                let line = &mut self.cmdline;
                line.truncate(grapheme::prev(line, line.len()));
                status
            }
            Key::Enter => {
                let input = std::mem::take(&mut self.cmdline);
                self.mode = Mode::Normal;
                if input.trim().is_empty() {
                    return status;
                }
                let fx = match ex::parse(&input) {
                    Ok(cmd) => self.ex(cmd),
                    Err(text) => {
                        self.say(text);
                        Effects::default()
                    }
                };
                status.merge(fx)
            }
            Key::Char(c) => self.cmdline_input(c.encode_utf8(&mut [0; 4])),
            Key::Space => self.cmdline_input(" "),
            _ => Effects::default(),
        }
    }

    /// Text typed on the command line. Enter runs it, so a line break in
    /// committed text is dropped.
    pub(super) fn cmdline_input(&mut self, text: &str) -> Effects {
        self.cmdline
            .extend(text.chars().filter(|c| !matches!(c, '\n' | '\r')));
        Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        }
    }

    /// Esc on the command line: back to Normal, the line dropped.
    fn leave_cmdline(&mut self) -> Effects {
        self.mode = Mode::Normal;
        self.cmdline.clear();
        Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        }
    }

    /// Carries out a command from the command line.
    fn ex(&mut self, cmd: ExCmd) -> Effects {
        match cmd {
            ExCmd::Write { force } => self.write_step(force).1,
            ExCmd::Quit { force } if !force && self.dirty => {
                self.say(strings::NO_WRITE_SINCE_CHANGE.to_owned());
                Effects::default()
            }
            ExCmd::Quit { .. } => self.close(0),
            ExCmd::Exit if !self.dirty => self.close(0),
            ExCmd::WriteQuit | ExCmd::Exit => match self.write_step(false) {
                (true, fx) => fx.merge(self.close(0)),
                (false, fx) => fx,
            },
            ExCmd::CQuit => self.close(1),
            ExCmd::ReloadForce => self.reload(),
            // TODO(Task 40): `:help` shows the help overlay.
            // TODO(Task 37): `:table` inserts a table or opens the picker.
            ExCmd::Help | ExCmd::Table(_) => Effects::default(),
            ExCmd::Substitute(sub) => self.substitute(sub),
        }
    }

    /// Writes the document through the `Saver`: true when it was written,
    /// which leaves the buffer clean. A failed write says why, as an Error.
    pub fn write(&mut self, force: bool) -> bool {
        match self.saver.write(&self.buf.to_text(), force) {
            Ok(()) => {
                self.dirty = false;
                true
            }
            Err(e) => {
                let text = strings::write_failed(&e.to_string());
                self.say_as(text, MsgKind::Error, status::MESSAGE_FOR_NS);
                false
            }
        }
    }

    /// Reloads the document from disk through the `Saver`, dropping the
    /// changes, as one undo step. A failed reload says why, as an Error,
    /// and changes nothing.
    /// TODO(Task 31): take the file's line endings and BOM from disk too.
    pub fn reload(&mut self) -> Effects {
        let fresh = match self.saver.reload() {
            Ok(text) => Buffer::from_text(&text),
            Err(e) => {
                let text = strings::reload_failed(&e.to_string());
                self.say_as(text, MsgKind::Error, status::MESSAGE_FOR_NS);
                return Effects::default();
            }
        };
        let lines = (0..fresh.line_count()).map(|i| fresh.line(i));
        let text = lines.collect::<Vec<_>>().join("\n");
        let same = (0..self.buf.line_count())
            .map(|i| self.buf.line(i))
            .eq((0..fresh.line_count()).map(|i| fresh.line(i)));
        let was_dirty = self.dirty;
        let fx = if same {
            Effects::default()
        } else {
            let cursor = self.cursor;
            let last = self.buf.line_count() - 1;
            let end = Pos {
                line: last,
                col: self.buf.line(last).len(),
            };
            let fx = self.run(|ctx| {
                ctx.replace(Pos::default()..end, &text);
                OpOutcome {
                    cursor,
                    insert: false,
                }
            });
            self.cursor = vim::rest(&self.buf, self.cursor);
            fx
        };
        self.dirty = false;
        fx.merge(Effects {
            redraw: Redraw::All,
            dirty: was_dirty.then_some(false),
            ..Effects::default()
        })
    }

    /// Closes the window, exiting with `code`, and tells the `Saver`.
    pub fn close(&mut self, code: i32) -> Effects {
        self.saver.close(code);
        Effects {
            close: Some(code),
            ..Effects::default()
        }
    }

    /// `write`, with the effects of the dirty state it changed.
    fn write_step(&mut self, force: bool) -> (bool, Effects) {
        let was_dirty = self.dirty;
        let written = self.write(force);
        let fx = Effects {
            dirty: (was_dirty && !self.dirty).then_some(false),
            ..Effects::default()
        };
        (written, fx)
    }

    /// `:s`. An empty pattern is the last search's. The pattern becomes
    /// the last search, as in vim, so `n` finds it and it is highlighted.
    /// `n` only counts and takes priority over `c`; reader mode only
    /// counts.
    fn substitute(&mut self, mut sub: Sub) -> Effects {
        if sub.pattern.is_empty() {
            match self.search.as_ref().filter(|s| !s.query.is_empty()) {
                Some(s) => sub.pattern.clone_from(&s.query),
                None => {
                    self.say(strings::NO_PREVIOUS_PATTERN.to_owned());
                    return Effects::default();
                }
            }
        }
        let visual = self.last_visual.map(|s| {
            let (a, b) = s.ordered();
            (a.line, b.line)
        });
        let n = self.buf.line_count();
        let lines = match substitute::lines(sub.range, self.cursor.line, visual, n) {
            Ok(lines) => lines,
            Err(text) => {
                self.say(text);
                return Effects::default();
            }
        };
        let fx = self.set_last_search(&sub.pattern);
        fx.merge(if sub.flags.n {
            self.count_matches(&sub, lines)
        } else if self.reader {
            Effects::default()
        } else if sub.flags.c {
            self.start_confirm(&sub, lines)
        } else {
            self.substitute_all(&sub, lines)
        })
    }

    /// Makes `pattern` the last search, going the way the last one went.
    fn set_last_search(&mut self, pattern: &str) -> Effects {
        let forward = self.search.as_ref().is_none_or(|s| s.forward);
        let mut s = Search::new(forward, self.vim_cursor());
        s.query = pattern.to_owned();
        s.refresh(&self.buf);
        self.search = Some(s);
        Effects {
            redraw: Redraw::All,
            ..Effects::default()
        }
    }

    /// `:s///n`: says how many matches there are. Vim 9.1 then puts the
    /// cursor on its line's first non-blank.
    fn count_matches(&mut self, sub: &Sub, lines: std::ops::RangeInclusive<usize>) -> Effects {
        let r = substitute::run(&self.buf, sub, lines);
        if r.count == 0 {
            self.say(strings::pattern_not_found(&sub.pattern));
            return Effects::default();
        }
        self.say(strings::matches(r.count, r.lines));
        let line = self.cursor.line;
        self.move_across(Pos {
            line,
            col: vim::motion::caret_col(self.buf.line(line)),
        })
    }

    /// `:s` without asking, as one undo step. The cursor ends on the first
    /// non-blank of the last line changed and, undone or redone, at the
    /// start of the first (vim 9.1).
    fn substitute_all(&mut self, sub: &Sub, lines: std::ops::RangeInclusive<usize>) -> Effects {
        let r = substitute::run(&self.buf, sub, lines);
        let (Some(first), Some(last)) = (r.edits.first(), r.edits.last()) else {
            self.say(strings::pattern_not_found(&sub.pattern));
            return Effects::default();
        };
        let (first, last) = (first.range.start.line, last.range.start.line);
        let fx = self.run(|ctx| {
            ctx.place(Pos {
                line: first,
                col: 0,
            });
            for e in &r.edits {
                ctx.replace(e.range.clone(), &e.text);
            }
            OpOutcome {
                cursor: Pos {
                    line: last,
                    col: vim::motion::caret_col(ctx.buf.line(last)),
                },
                insert: false,
            }
        });
        self.say(strings::substitutions(r.count, r.lines));
        fx
    }

    /// `:s///c`: Confirm mode on the first match, or `E486` when there is
    /// none. The whole session is one undo step, opened here and closed by
    /// `end_confirm`.
    fn start_confirm(&mut self, sub: &Sub, lines: std::ops::RangeInclusive<usize>) -> Effects {
        let session = substitute::confirm_session(&self.buf, sub, lines);
        if session.done() {
            self.say(strings::pattern_not_found(&sub.pattern));
            return Effects::default();
        }
        self.unsay();
        self.history.begin();
        self.confirm = Some(session);
        self.mode = Mode::Confirm;
        self.show_confirm()
    }

    /// A key in Confirm mode: `y n a q l` in either case, or Esc to quit.
    /// Other keys wait for an answer.
    pub(super) fn confirm_key(&mut self, k: KeyEvent) -> Effects {
        let answer = match k.key {
            Key::Esc => Answer::Quit,
            Key::Char(c) => match Answer::from_key(c) {
                Some(a) => a,
                None => return Effects::default(),
            },
            _ => return Effects::default(),
        };
        let Some(session) = self.confirm.as_mut() else {
            return Effects::default();
        };
        let edits = session.answer(answer);
        let done = session.done();
        let fx = if edits.is_empty() {
            Effects::default()
        } else {
            self.run(|ctx| {
                for e in &edits {
                    ctx.replace(e.range.clone(), &e.text);
                }
                OpOutcome {
                    cursor: ctx.cursor,
                    insert: false,
                }
            })
        };
        fx.merge(if done {
            self.end_confirm()
        } else {
            self.show_confirm()
        })
    }

    /// Puts the cursor on the match being asked about, highlighted as the
    /// current one.
    fn show_confirm(&mut self) -> Effects {
        let Some((line, range)) = self.confirm.as_ref().and_then(ConfirmSession::current) else {
            return Effects::default();
        };
        let at = Pos {
            line,
            col: range.start,
        };
        if let Some(s) = self.search.as_mut() {
            s.set_current(Some(&Hit {
                line,
                range,
                wrapped: false,
            }));
        }
        self.move_across(at).merge(Effects {
            redraw: Redraw::All,
            ..Effects::default()
        })
    }

    /// The session is over: its undo step closes (with the cursor vim's
    /// undo goes back to), the cursor goes where vim leaves it, and what
    /// was replaced is said (nothing, when nothing was).
    fn end_confirm(&mut self) -> Effects {
        let Some(session) = self.confirm.take() else {
            return Effects::default();
        };
        if let Some(at) = session.undo_cursor() {
            self.history.set_cursor(at);
        }
        self.history.end();
        self.mode = Mode::Normal;
        if let Some(s) = self.search.as_mut() {
            s.set_current(None);
        }
        if session.count() > 0 {
            let text = strings::substitutions(session.count(), session.lines());
            self.say(text);
        }
        let to = vim::rest(&self.buf, session.end_cursor(&self.buf));
        self.move_across(to).merge(Effects {
            redraw: Redraw::All,
            ..Effects::default()
        })
    }
}

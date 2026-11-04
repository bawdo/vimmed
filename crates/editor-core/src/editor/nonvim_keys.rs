//! Carrying out the keys with vim off (`crate::nonvim` says what each one
//! means): motions and the selection, typing in runs that are one undo
//! step each, the formatting, undo and clipboard chords, Primary+S with
//! its changed-on-disk prompt, and the find bar.
//!
//! The selection is `anchor` to the cursor (the cursor's side open) in
//! Insert, the mode the editor rests in with vim off. Every edit drops it
//! (`finish`); a formatting chord then selects the text it wrapped.

use std::ops::Range;

use crate::buffer::Pos;
use crate::effects::{Effects, Redraw};
use crate::file::{DiskEvent, WriteError};
use crate::grapheme;
use crate::input::{Key, KeyEvent};
use crate::nonvim::{self, Action, Motion, RunKind};
use crate::search::{self, Hit, Search};
use crate::status::{self, Mode};
use crate::strings;
use crate::view_model::{MsgKind, Selection};
use crate::vim::operator::{ListKey, OpOutcome};

use super::Editor;
use super::prompt::Ask;

impl Editor {
    /// A key with vim off, outside the find bar, a prompt and the link
    /// prompt.
    pub(super) fn plain_key(&mut self, k: KeyEvent) -> Effects {
        match nonvim::action(&k, self.platform) {
            Action::Move { motion, select } => self.plain_move(motion, select),
            Action::Type(text) => self.plain_type(&text),
            Action::Backspace => self.plain_delete(false),
            Action::Delete => self.plain_delete(true),
            Action::List(key) => self.plain_list_key(key),
            Action::Bold => self.plain_wrap(2),
            Action::Italic => self.plain_wrap(1),
            Action::Link => self.start_link(),
            Action::Undo => self.plain_undo(false),
            Action::Redo => self.plain_undo(true),
            Action::Copy => self.plain_copy(),
            Action::Cut => self.plain_cut(),
            Action::Paste => self.plain_paste(),
            Action::Find => self.start_find(),
            Action::Esc => self.drop_selection(),
            Action::Nothing => Effects::default(),
        }
    }

    /// The selection, in document order, the cursor's side open; `None`
    /// when nothing is selected.
    pub(super) fn plain_range(&self) -> Option<Range<Pos>> {
        let a = self.buf.clamp(self.anchor?);
        let c = self.buf.clamp(self.cursor);
        if a == c {
            None
        } else if a < c {
            Some(a..c)
        } else {
            Some(c..a)
        }
    }

    /// The selection as the renderer draws it: its head is the last
    /// grapheme selected, as Visual's is.
    pub(super) fn plain_selection(&self) -> Option<Selection> {
        let r = self.plain_range()?;
        let head = nonvim::left(&self.buf, r.end).unwrap_or(r.end);
        Some(Selection {
            anchor: r.start,
            head,
            linewise: false,
        })
    }

    /// Esc: the selection goes, the cursor stays.
    fn drop_selection(&mut self) -> Effects {
        let before = self.selection_lines();
        self.anchor = None;
        self.selection_redraw(before)
    }

    /// A motion. With `select` it moves the selection's head, anchoring
    /// it where the cursor was. Without, it drops the selection, and Left
    /// and Right go to its start and end.
    fn plain_move(&mut self, motion: Motion, select: bool) -> Effects {
        let before = self.selection_lines();
        let range = self.plain_range();
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
            let end = match (range, motion) {
                (Some(r), Motion::Left) => Some(r.start),
                (Some(r), Motion::Right) => Some(r.end),
                _ => None,
            };
            if let Some(to) = end {
                return self.move_across(to).merge(self.selection_redraw(before));
            }
        }
        let to = match motion {
            Motion::Up => return self.vertical(false).merge(self.selection_redraw(before)),
            Motion::Down => return self.vertical(true).merge(self.selection_redraw(before)),
            Motion::Left => nonvim::left(&self.buf, self.cursor),
            Motion::Right => nonvim::right(&self.buf, self.cursor),
            Motion::LineStart => Some(self.at(0)),
            Motion::LineEnd => Some(self.line_end(self.cursor.line)),
            Motion::WordLeft => Some(nonvim::word_left(&self.buf, self.cursor)),
            Motion::WordRight => Some(nonvim::word_right(&self.buf, self.cursor)),
            Motion::DocStart => Some(Pos::default()),
            Motion::DocEnd => Some(nonvim::doc_end(&self.buf)),
        };
        let fx = match to {
            Some(to) => self.move_across(to),
            None => Effects::default(),
        };
        fx.merge(self.selection_redraw(before))
    }

    /// Runs `f`, a typed key of `kind`, as part of the run before it when
    /// `join` allows and nothing else came between (`crate::nonvim`), and
    /// makes it the run the next key may join.
    fn typed(
        &mut self,
        kind: RunKind,
        join: bool,
        f: impl FnOnce(&mut Editor) -> Effects,
    ) -> Effects {
        let group = self.plain.joins(kind).filter(|_| join);
        let reopened = group.is_some_and(|g| self.history.reopen(g));
        let mark = self.buf.shift_mark();
        let fx = f(self);
        if reopened {
            self.history.end();
        }
        if (reopened || self.buf.shift_mark() != mark)
            && let Some(g) = self.history.last_group()
        {
            self.plain.ran(kind, g);
        }
        fx
    }

    /// Typed text (a key, or an input method's commit): over a selection,
    /// a step of its own that the typing after joins.
    pub(super) fn plain_type(&mut self, text: &str) -> Effects {
        if let Some(r) = self.plain_range() {
            let before = self.selection_lines();
            let fx = self.typed(RunKind::Typing, false, |e| e.edit(r, text));
            return fx.merge(self.selection_redraw(before));
        }
        self.typed(RunKind::Typing, true, |e| e.insert(text))
    }

    /// Backspace, or (`forward`) Delete: takes the selection, else a
    /// grapheme.
    fn plain_delete(&mut self, forward: bool) -> Effects {
        if let Some(r) = self.plain_range() {
            let before = self.selection_lines();
            let fx = self.typed(RunKind::Deleting, false, |e| e.edit(r, ""));
            return fx.merge(self.selection_redraw(before));
        }
        self.typed(RunKind::Deleting, true, |e| {
            if forward { e.delete() } else { e.backspace() }
        })
    }

    /// Enter, Tab or Shift+Tab (`list_keys`), over a selection taking it
    /// first, in the same step. In a table, where the key moves between
    /// cells, it only drops the selection and moves.
    fn plain_list_key(&mut self, key: ListKey) -> Effects {
        let Some(r) = self.plain_range() else {
            return self.insert_list_key(key);
        };
        let before = self.selection_lines();
        self.anchor = None;
        if let Some(fx) = self.table_nav(key) {
            return fx.merge(self.selection_redraw(before));
        }
        if self.refuses_edits() {
            return self.selection_redraw(before);
        }
        self.history.begin();
        let fx = self.edit(r, "");
        let fx = fx.merge(self.insert_list_key(key));
        self.history.end();
        fx.merge(self.selection_redraw(before))
    }

    /// Primary+B (`width` 2) or Primary+I (`width` 1): `toggle_wrap` over
    /// the selection or at the cursor, one undo step, selecting the text
    /// the markers wrap.
    fn plain_wrap(&mut self, width: usize) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        let before = self.selection_lines();
        let sel = self.plain_range().unwrap_or_else(|| {
            let at = self.buf.clamp(self.cursor);
            at..at
        });
        let w = nonvim::toggle_wrap(&self.buf, sel, width);
        let select = w.select.clone();
        let fx = self.run(|ctx| {
            ctx.replace(w.range, &w.text);
            OpOutcome {
                cursor: select.end,
                insert: false,
            }
        });
        if select.start != select.end {
            self.anchor = Some(select.start);
        }
        fx.merge(self.selection_redraw(before))
    }

    /// Primary+Z, or (`redo`) Primary+Shift+Z: one step.
    fn plain_undo(&mut self, redo: bool) -> Effects {
        let before = self.selection_lines();
        self.anchor = None;
        self.undo(redo, 1).merge(self.selection_redraw(before))
    }

    /// Primary+C: the selection's text to the clipboard. Read-only
    /// documents copy too.
    fn plain_copy(&mut self) -> Effects {
        if let Some(r) = self.plain_range() {
            let text = self.buf.text_range(r);
            self.host.clipboard_set(&text);
        }
        Effects::default()
    }

    /// Primary+X: the selection's text to the clipboard, then out of the
    /// document, as one step.
    fn plain_cut(&mut self) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        let Some(r) = self.plain_range() else {
            return Effects::default();
        };
        let text = self.buf.text_range(r.clone());
        self.host.clipboard_set(&text);
        let before = self.selection_lines();
        self.edit(r, "").merge(self.selection_redraw(before))
    }

    /// Primary+V: the clipboard's text in literally, over the selection,
    /// as one step, the cursor after it.
    fn plain_paste(&mut self) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        let Some(text) = self
            .host
            .clipboard_get()
            .filter(|t| !t.is_empty())
            .map(|t| t.replace("\r\n", "\n"))
        else {
            return Effects::default();
        };
        let before = self.selection_lines();
        let r = self.plain_range().unwrap_or_else(|| {
            let at = self.buf.clamp(self.cursor);
            at..at
        });
        self.edit(r, &text).merge(self.selection_redraw(before))
    }

    /// Primary+S with vim off (`plain_write`).
    pub(super) fn plain_save(&mut self) -> Effects {
        self.plain_write(false).1
    }

    /// A write with vim off: `:w`, except that a file changed on disk
    /// asks `file changed on disk: [o]verwrite [r]eload [c]ancel` rather
    /// than refusing (`closing`: for the close prompt's `s`, whose close
    /// goes ahead once `o` has written). The file watch's warning asks at
    /// once; a change the watch has not seen yet asks once the write finds
    /// it. True when written, with the effects of the dirty state it
    /// changed.
    pub(super) fn plain_write(&mut self, closing: bool) -> (bool, Effects) {
        if self.refuse_write() {
            return (false, Effects::default());
        }
        let ask = Ask::ChangedOnDisk { closing };
        if self.disk == Some(DiskEvent::Changed) {
            return (false, self.ask(ask));
        }
        let was_dirty = self.dirty;
        match self.write_checked(false) {
            Ok(()) => self.say(strings::WRITTEN.to_owned()),
            Err(WriteError::ChangedOnDisk) => return (false, self.ask(ask)),
            Err(e) => {
                self.say_as(e.message(), MsgKind::Error, status::MESSAGE_FOR_NS);
                return (false, Effects::default());
            }
        }
        let fx = Effects {
            dirty: was_dirty.then_some(false),
            ..Effects::default()
        };
        (true, fx)
    }

    /// Primary+F: the find bar, searching from the cursor. The selection
    /// goes.
    fn start_find(&mut self) -> Effects {
        let before = self.selection_lines();
        self.anchor = None;
        let fx = self.clear_preedit();
        self.unsay();
        self.plain.open_find(self.buf.clamp(self.cursor));
        self.search = Some(Search::new(true, self.vim_cursor()));
        fx.merge(self.selection_redraw(before)).merge(Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        })
    }

    /// A key while the find bar shows: typing and Backspace edit the
    /// query, Enter and Shift+Enter go to the next and previous match, Esc
    /// closes it, and Primary+V pastes into it. Other keys do nothing.
    pub(super) fn find_key(&mut self, k: KeyEvent) -> Effects {
        let m = k.mods;
        if self.platform.primary(m) {
            if matches!(k.key, Key::Char('v' | 'V')) && !m.alt && !m.shift {
                let text = self.host.clipboard_get().unwrap_or_default();
                return self.find_input(&text);
            }
            return Effects::default();
        }
        if m.ctrl || m.cmd {
            return Effects::default();
        }
        match k.key {
            Key::Esc => self.close_find(),
            Key::Enter => self.find_step(!m.shift),
            Key::Backspace => {
                let Some(s) = self.search.as_mut() else {
                    return Effects::default();
                };
                let q = &mut s.query;
                q.truncate(grapheme::prev(q, q.len()));
                self.find_again()
            }
            Key::Char(c) => self.find_input(c.encode_utf8(&mut [0; 4])),
            Key::Space => self.find_input(" "),
            _ => Effects::default(),
        }
    }

    /// Text typed into the find bar; a line break cannot be in a match, so
    /// it is dropped.
    pub(super) fn find_input(&mut self, text: &str) -> Effects {
        let Some(s) = self.search.as_mut() else {
            return Effects::default();
        };
        s.query
            .extend(text.chars().filter(|c| !matches!(c, '\n' | '\r')));
        self.find_again()
    }

    /// Searches for the query as it is now from where the bar opened (a
    /// match there counts), moving the cursor to the match, or back there
    /// when there is none.
    fn find_again(&mut self) -> Effects {
        let Some(origin) = self.plain.find().map(|f| self.buf.clamp(f.origin)) else {
            return Effects::default();
        };
        let Some(s) = self.search.as_mut() else {
            return Effects::default();
        };
        s.refresh(&self.buf);
        let here = search::line_matches(self.buf.line(origin.line), &s.query)
            .into_iter()
            .find(|r| r.start == origin.col)
            .map(|range| Hit {
                line: origin.line,
                range,
                wrapped: false,
            });
        let hit = here.or_else(|| search::find(&self.buf, &s.query, origin, true, 1));
        s.set_current(hit.as_ref());
        self.unsay();
        let to = hit.map_or(origin, |h| h.pos());
        self.count_finds();
        let fx = self.move_across(to);
        fx.merge(Effects {
            redraw: Redraw::All,
            ..Effects::default()
        })
    }

    /// Enter (`forward`) or Shift+Enter in the find bar: the next or
    /// previous match from the cursor, saying `search wrapped` for a
    /// second when it went round the end.
    fn find_step(&mut self, forward: bool) -> Effects {
        let Some(s) = self.search.as_mut() else {
            return Effects::default();
        };
        let Some(hit) = search::find(&self.buf, &s.query, self.cursor, forward, 1) else {
            return Effects::default();
        };
        s.set_current(Some(&hit));
        if hit.wrapped {
            self.say_as(
                strings::SEARCH_WRAPPED.to_owned(),
                MsgKind::Info,
                search::WRAPPED_FOR_NS,
            );
        } else {
            self.unsay();
        }
        self.count_finds();
        self.move_across(hit.pos()).merge(Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        })
    }

    /// Brings the find bar's counter up to date.
    fn count_finds(&mut self) {
        let (at, of) = match &self.search {
            Some(s) => {
                let all = s.matches(&self.buf);
                let at = s
                    .current()
                    .and_then(|c| all.iter().position(|m| *m == c))
                    .map_or(0, |i| i + 1);
                (at, all.len())
            }
            None => (0, 0),
        };
        if let Some(f) = self.plain.find_mut() {
            f.at = at;
            f.of = of;
        }
    }

    /// Esc in the find bar: it closes, the cursor stays on the match, and
    /// the highlights go.
    fn close_find(&mut self) -> Effects {
        self.plain.close_find();
        self.search = None;
        self.unsay();
        self.clear_preedit().merge(Effects {
            redraw: Redraw::All,
            scroll_to_cursor: true,
            ..Effects::default()
        })
    }

    /// The find bar's counter, with a message showing (`search wrapped`)
    /// before it: what the status bar shows on the right while the bar is
    /// open.
    pub(super) fn find_counter(&self) -> Option<String> {
        let f = self.plain.find()?;
        let counter = strings::find_counter(f.at, f.of);
        Some(match &self.message {
            Some((text, _)) => format!("{text}  {counter}"),
            None => counter,
        })
    }

    /// True while the find bar is open (vim off).
    pub(super) fn finding(&self) -> bool {
        self.plain.find().is_some()
    }

    /// True in the mode non-vim editing rests in.
    pub(super) fn plain_editing(&self) -> bool {
        !self.cfg.vim && self.mode == Mode::Insert
    }
}

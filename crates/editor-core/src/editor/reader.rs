//! Reader view and the help overlay.
//!
//! Reader view (`--view`, or Primary+Shift+R from Normal, Insert or
//! Visual) hides the caret and the status bar and reveals nothing. It is
//! read-only: edits, undo, `.`, paste and Primary+S do nothing, and `:` is
//! disabled. `j k`, the arrows, PageUp and PageDown, `gg` and `G` scroll
//! the view (`Effects::scroll`) and leave the cursor where it is, so
//! toggling back finds the cursor, mode and selection as they were. Other
//! keys still reach the vim engine: Visual selects and `y` yanks, `/` and
//! `n` search, `ZZ` and `ZQ` close.
//!
//! The help overlay (`:help`) shows `crate::help::rows` from a first row
//! that `j`/`k` (and Down/Up) move; Esc dismisses it. While it shows it
//! takes every key.

use crate::buffer::Pos;
use crate::effects::{Effects, Redraw, Scroll};
use crate::help;
use crate::input::{Key, KeyEvent, Mods};
use crate::status::Mode;
use crate::vim::Want;

use super::Editor;

/// What toggling reader view off goes back to.
pub(super) struct Return {
    cursor: Pos,
    goal: Option<Want>,
    mode: Mode,
    anchor: Option<Pos>,
}

/// A full repaint: reader view changes the geometry, and the overlay
/// covers the text.
fn all() -> Effects {
    Effects {
        redraw: Redraw::All,
        ..Effects::default()
    }
}

impl Editor {
    /// Primary+Shift+R: reader view on, or off again.
    pub(super) fn toggle_reader(&mut self) -> Effects {
        if self.reader {
            self.leave_reader()
        } else {
            self.enter_reader()
        }
    }

    /// Into reader view, from Normal, Insert or Visual (a line being typed
    /// in the status bar keeps the key). An Insert session ends here, as
    /// Esc would end it, for undo and `.`.
    fn enter_reader(&mut self) -> Effects {
        let from = self.mode;
        if !matches!(
            from,
            Mode::Normal | Mode::Insert | Mode::Visual | Mode::VisualLine
        ) {
            return Effects::default();
        }
        self.vim.cancel();
        if from == Mode::Insert && self.cfg.vim {
            self.history.end();
            self.vim.commit();
        }
        let fx = self.clear_preedit();
        self.reader_return = Some(Return {
            cursor: self.cursor,
            goal: self.goal,
            mode: from,
            anchor: self.anchor,
        });
        self.anchor = None;
        self.mode = Mode::Normal;
        self.reader = true;
        self.reader_g = false;
        fx.merge(Effects {
            reader: Some(true),
            ..all()
        })
    }

    /// Out of reader view, to the cursor, mode and selection it was
    /// entered from (opened with `--view`, the mode it opened in), with
    /// the view back on the cursor. An Insert session starts again where
    /// it stopped.
    fn leave_reader(&mut self) -> Effects {
        self.vim.cancel();
        self.reader = false;
        self.reader_g = false;
        if let Some(back) = self.reader_return.take() {
            self.cursor = self.buf.clamp(back.cursor);
            self.goal = back.goal;
            self.mode = back.mode;
            self.anchor = back.anchor.map(|a| self.buf.clamp(a));
        }
        if self.mode == Mode::Insert && self.cfg.vim {
            self.history.begin();
            self.vim.resume_insert();
        }
        Effects {
            reader: Some(false),
            scroll_to_cursor: true,
            ..all()
        }
    }

    /// A key in reader view outside Visual: the scrolling keys scroll,
    /// with a count multiplying them, and `[N]G` or `[N]gg` bring line N
    /// to the top. `None` passes any other key on to the vim engine. A
    /// scrolling key while an operator, register or other prefix waits
    /// does nothing and drops what waited, since the engine would only
    /// move the hidden cursor. A `g` is held (with any count left
    /// waiting in the engine) to see whether `gg` follows; if not, it goes
    /// to the engine before the key does.
    pub(super) fn reader_key(&mut self, k: KeyEvent) -> Option<Effects> {
        let pending = self.vim.showcmd();
        // A count alone (`3`), and nothing else, may come before a scroll.
        let count: Option<i32> = if pending.is_empty() {
            None
        } else {
            pending.parse().ok()
        };
        let counted = (pending.is_empty() || count.is_some()) && !self.vim.register_waiting();
        // Line N at the top; the view clamps it to the document's end.
        let to_line = |n: i32| Scroll::Line(usize::try_from(n.max(1) - 1).unwrap_or(0));
        if std::mem::take(&mut self.reader_g) {
            if k.key == Key::Char('g') {
                return Some(self.reader_scroll(count.map_or(Scroll::Top, to_line)));
            }
            let mut fx = Effects::default();
            if self.cfg.vim {
                fx = self.normal_key(KeyEvent {
                    key: Key::Char('g'),
                    mods: Mods::default(),
                    ..k
                });
            }
            // Re-entrant: with the held `g` cleared above, `key` runs this
            // key the normal way, back through here only if it scrolls.
            // It cannot hold another `g` on the strength of this one, so
            // this recurses at most once.
            return Some(fx.merge(self.key(k)));
        }
        let n = count.unwrap_or(1);
        let scroll = match k.key {
            Key::Char('j') | Key::Down => Scroll::Lines(n),
            Key::Char('k') | Key::Up => Scroll::Lines(-n),
            Key::PageDown => Scroll::Pages(n),
            Key::PageUp => Scroll::Pages(-n),
            Key::Char('G') => count.map_or(Scroll::Bottom, to_line),
            Key::Char('g') if counted => {
                self.reader_g = true;
                return Some(Effects::default());
            }
            Key::Char('g') => Scroll::Top,
            _ => return None,
        };
        if !counted {
            self.vim.cancel();
            return Some(Effects {
                redraw: Redraw::Status,
                ..Effects::default()
            });
        }
        Some(self.reader_scroll(scroll))
    }

    /// Scrolls the view; any count typed for it is used up.
    fn reader_scroll(&mut self, scroll: Scroll) -> Effects {
        self.vim.cancel();
        Effects {
            scroll: Some(scroll),
            redraw: Redraw::Status,
            ..Effects::default()
        }
    }

    /// `:help`: the overlay, from its first row.
    pub(super) fn open_help(&mut self) -> Effects {
        self.help = Some(0);
        all()
    }

    /// A key while the help shows: `j`/Down and `k`/Up scroll it a row
    /// (no further than its last row), Esc dismisses it, and every other
    /// key does nothing.
    pub(super) fn help_key(&mut self, k: KeyEvent) -> Effects {
        let Some(top) = self.help else {
            return Effects::default();
        };
        let plain = !(k.mods.ctrl || k.mods.alt || self.platform.primary(k.mods));
        let last = help::rows().len().saturating_sub(1);
        let to = match k.key {
            Key::Esc => {
                self.help = None;
                return all();
            }
            Key::Char('j') | Key::Down if plain => (top + 1).min(last),
            Key::Char('k') | Key::Up if plain => top.saturating_sub(1),
            _ => return Effects::default(),
        };
        if to == top {
            return Effects::default();
        }
        self.help = Some(to);
        all()
    }
}

#[cfg(test)]
mod tests {
    use crate::effects::{Effects, Redraw, Scroll};
    use crate::input::{Key, Mods};
    use crate::status::Mode;
    use crate::testkit::{ViewSource, build_reader, ed, ed_with_files, key, key_with, keys, pos};
    use crate::view_model::{CaretShape, Selection};

    use super::super::Editor;

    /// Primary+Shift+R, as the Mac shell sends it.
    fn toggle(e: &mut Editor) -> Effects {
        e.handle(key_with(
            Key::Char('R'),
            Mods {
                cmd: true,
                shift: true,
                ..Mods::default()
            },
        ))
    }

    #[test]
    fn view_flag_opens_in_reader() {
        let e = build_reader("a");
        assert!(e.reader());
        assert_eq!(e.caret_shape(), CaretShape::Hidden);
    }

    #[test]
    fn toggle_restores_cursor_and_mode() {
        let mut e = ed("one\ntwo\nthree");
        keys(&mut e, "jli");
        assert_eq!((e.mode(), e.cursor()), (Mode::Insert, pos(1, 1)));
        let fx = toggle(&mut e);
        assert_eq!(fx.reader, Some(true));
        assert_eq!(fx.redraw, Redraw::All);
        assert!(e.reader());
        assert_eq!(e.caret_shape(), CaretShape::Hidden);
        // Reading scrolls the view; the cursor waits.
        keys(&mut e, "jjGgg<Down>");
        assert_eq!(e.cursor(), pos(1, 1));
        let fx = toggle(&mut e);
        assert_eq!(fx.reader, Some(false));
        assert!(fx.scroll_to_cursor);
        assert!(!e.reader());
        assert_eq!((e.mode(), e.cursor()), (Mode::Insert, pos(1, 1)));
        // The Insert session goes on, as one undo step that `.` repeats.
        keys(&mut e, "x<Esc>");
        assert_eq!(e.text(), "one\ntxwo\nthree");
        keys(&mut e, "j.");
        assert_eq!(e.text(), "one\ntxwo\ntxhree");
        keys(&mut e, "uu");
        assert_eq!(e.text(), "one\ntwo\nthree");

        // From Visual, the selection comes back.
        let mut e = ed("hello world");
        keys(&mut e, "lvll");
        toggle(&mut e);
        assert_eq!(e.selection(), None);
        toggle(&mut e);
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(
            e.selection(),
            Some(Selection {
                anchor: pos(0, 1),
                head: pos(0, 3),
                linewise: false
            })
        );
    }

    #[test]
    fn toggle_is_ignored_while_a_line_is_typed() {
        let mut e = ed("abc");
        keys(&mut e, ":");
        assert_eq!(toggle(&mut e).reader, None);
        assert!(!e.reader());
        assert_eq!(e.mode(), Mode::Command);
    }

    #[test]
    fn reader_refuses_edits() {
        let (mut e, files) = ed_with_files("abc\ndef");
        toggle(&mut e);
        keys(&mut e, "xddihello<Esc>oZ<Esc>pP<D-v>vjd");
        keys(&mut e, "<Esc>");
        assert_eq!(e.text(), "abc\ndef");
        assert!(!e.dirty());
        assert_eq!(e.mode(), Mode::Normal);
        // `:` is disabled, so neither `:w` nor `:q` can run.
        let fx = keys(&mut e, ":q<CR>");
        assert_eq!(fx.close, None);
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cmdline(), "");
        // Primary+S does nothing in reader view.
        keys(&mut e, "<D-s>");
        assert!(files.calls().is_empty());
        assert_eq!(e.message(), None);
    }

    #[test]
    fn reader_keys_scroll_the_view_not_the_cursor() {
        let mut e = build_reader("a\nb\nc\nd");
        let scroll = |e: &mut Editor, seq: &str| keys(e, seq).scroll;
        assert_eq!(scroll(&mut e, "j"), Some(Scroll::Lines(1)));
        assert_eq!(scroll(&mut e, "<Down>"), Some(Scroll::Lines(1)));
        assert_eq!(scroll(&mut e, "k"), Some(Scroll::Lines(-1)));
        assert_eq!(scroll(&mut e, "<Up>"), Some(Scroll::Lines(-1)));
        assert_eq!(scroll(&mut e, "3j"), Some(Scroll::Lines(3)));
        assert_eq!(scroll(&mut e, "G"), Some(Scroll::Bottom));
        assert_eq!(scroll(&mut e, "gg"), Some(Scroll::Top));
        assert_eq!(e.handle(key(Key::PageDown)).scroll, Some(Scroll::Pages(1)));
        assert_eq!(e.handle(key(Key::PageUp)).scroll, Some(Scroll::Pages(-1)));
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.showcmd(), "");
        // Visual still moves the (hidden) cursor, so a selection can grow
        // and be yanked.
        keys(&mut e, "vjy");
        assert_eq!(e.register(None).unwrap().text, "a\nb");
        assert_eq!(e.text(), "a\nb\nc\nd");
    }

    #[test]
    fn reader_ignores_search_keys() {
        let mut e = build_reader("foo\nbar foo\nbaz");
        for k in ["/", "?", "n", "N"] {
            let fx = keys(&mut e, k);
            assert_eq!(e.mode(), Mode::Normal, "{k} opened a line");
            assert_eq!(fx.scroll, None);
            // The next key scrolls, not types into a hidden query.
            assert_eq!(keys(&mut e, "j").scroll, Some(Scroll::Lines(1)));
            assert_eq!(e.mode(), Mode::Normal);
        }
        assert_eq!(e.cursor(), pos(0, 0));
        // An operator waiting for a search is dropped too.
        keys(&mut e, "y/bar<CR>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.register(None), None);
        // So is Visual's.
        keys(&mut e, "v/");
        assert_eq!(e.mode(), Mode::Visual);
        keys(&mut e, "<Esc>");
        // The toggle still works.
        assert_eq!(toggle(&mut e).reader, Some(false));
    }

    #[test]
    fn reader_zz_refuses_when_dirty() {
        let (mut e, files) = ed_with_files("abc");
        keys(&mut e, "x");
        assert!(e.dirty());
        toggle(&mut e);
        let fx = keys(&mut e, "ZZ");
        assert_eq!(fx.close, None);
        assert!(files.calls().is_empty(), "wrote: {:?}", files.calls());
        assert_eq!(e.message(), Some(crate::strings::READ_ONLY_ZQ));
        assert!(e.dirty());
        // ZQ still discards and closes.
        assert_eq!(keys(&mut e, "ZQ").close, Some(0));
    }

    #[test]
    fn reader_zz_closes_when_clean() {
        let (mut e, files) = ed_with_files("abc");
        toggle(&mut e);
        assert_eq!(keys(&mut e, "ZZ").close, Some(0));
        assert!(
            !files
                .calls()
                .iter()
                .any(|c| matches!(c, crate::testkit::FileCall::Write { .. }))
        );
    }

    #[test]
    fn counted_g_scrolls_that_line_to_the_top() {
        let mut e = build_reader("a\nb\nc\nd\ne");
        assert_eq!(keys(&mut e, "3G").scroll, Some(Scroll::Line(2)));
        assert_eq!(keys(&mut e, "4gg").scroll, Some(Scroll::Line(3)));
        // Line 0 is line 1, and a count past the end is the end's to clamp.
        assert_eq!(keys(&mut e, "0G").scroll, Some(Scroll::Bottom));
        assert_eq!(keys(&mut e, "1gg").scroll, Some(Scroll::Line(0)));
        assert_eq!(keys(&mut e, "99G").scroll, Some(Scroll::Line(98)));
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.showcmd(), "");
    }

    #[test]
    fn scroll_keys_after_an_operator_or_register_do_nothing() {
        let mut e = build_reader("a\nb\nc\nd");
        for seq in ["yj", "\"aj", "\"a3k", "dG", "yG", "\"<Down>", "cj"] {
            let fx = keys(&mut e, seq);
            assert_eq!(fx.scroll, None, "{seq} scrolled");
            assert_eq!(e.cursor(), pos(0, 0), "{seq} moved the cursor");
            assert_eq!(e.showcmd(), "", "{seq} left keys pending");
        }
        keys(&mut e, "c");
        assert_eq!(e.handle(key(Key::PageDown)).scroll, None);
        assert_eq!(e.showcmd(), "");
        assert_eq!(e.register(None), None);
        assert_eq!(e.register(Some('a')), None);
        // Afterwards the keys scroll as ever.
        assert_eq!(keys(&mut e, "j").scroll, Some(Scroll::Lines(1)));
    }

    #[test]
    fn help_opens_scrolls_and_ignores_other_keys() {
        let mut e = ed("abc");
        let fx = keys(&mut e, ":help<CR>");
        assert_eq!(e.help(), Some(0));
        assert_eq!(fx.redraw, Redraw::All);
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "jj");
        assert_eq!(e.help(), Some(2));
        keys(&mut e, "k<Up>");
        assert_eq!(e.help(), Some(0));
        keys(&mut e, "k");
        assert_eq!(e.help(), Some(0));
        keys(&mut e, "xdd<D-v>");
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), pos(0, 0));
        // It never scrolls past its last row.
        for _ in 0..crate::help::rows().len() + 5 {
            keys(&mut e, "j");
        }
        assert_eq!(e.help(), Some(crate::help::rows().len() - 1));
    }

    #[test]
    fn esc_dismisses_help() {
        let mut e = ed("abc");
        keys(&mut e, ":help<CR>j");
        let fx = keys(&mut e, "<Esc>");
        assert_eq!(e.help(), None);
        assert_eq!(fx.redraw, Redraw::All);
        keys(&mut e, "x");
        assert_eq!(e.text(), "bc");
    }
}

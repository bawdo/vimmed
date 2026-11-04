//! The vim engine: modes, counts, motions, operators and the keys that
//! enter Insert.
//!
//! `Editor` routes Normal-mode keys here, and Visual-mode keys to
//! `Vim::visual_key` (`visual`). The engine keeps the pending command
//! (count, operator, `g` or `i` prefix), decides what each key means, and
//! hands back a `Step` for the editor to carry out on its buffer (operators
//! through `operator::operate` and `operator::put`). Insert-mode typing
//! stays on the editor's own path, which tells the engine what was typed so
//! `.` can repeat it (`repeat`).

pub mod count;
pub mod motion;
pub mod operator;
pub mod register;
pub mod repeat;
pub mod textobj;
pub mod visual;

use crate::buffer::{Buffer, Pos};
use crate::grapheme;
use crate::input::{Key, KeyEvent};
use crate::vim::operator::{EditCtx, OpOutcome};
use crate::vim::repeat::Change;

pub use count::Count;
pub use motion::{Motion, MotionKind, MotionResult};
pub use operator::{OpRange, Operator};

/// The editing mode. Normal, Insert, Visual, Visual Line and Search have
/// their behaviour; Command has only its entry from Visual (Task 25 builds
/// it), and later tasks build the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    Search,
    Command,
    Confirm,
    Prompt,
}

/// The column vertical motions aim for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// A display column (graphemes from the line start).
    Col(usize),
    /// The end of the line, after `$`.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub pos: Pos,
    pub want: Want,
}

impl Cursor {
    /// A cursor at `pos` that wants its own display column.
    pub fn at(buf: &Buffer, pos: Pos) -> Cursor {
        Cursor {
            pos,
            want: Want::Col(grapheme::display_col(buf.line(pos.line), pos.col)),
        }
    }
}

/// What a Normal-mode key asks the editor to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing moves; the pending command may have changed.
    Pending,
    /// Move the cursor.
    Move(Cursor),
    /// Enter Insert with the cursor here.
    Insert(Pos),
    /// Open a plain empty line below (`o`) or above (`O`) the cursor's line
    /// and enter Insert on it.
    Open { above: bool },
    /// Apply an operator (`operator::operate`).
    Operate { op: Operator, range: OpRange },
    /// Put the clipboard `count` times (`operator::put`): `P` before.
    Put { before: bool, count: usize },
    /// Undo `count` steps (`u`).
    Undo { count: usize },
    /// Redo `count` steps (Ctrl+R).
    Redo { count: usize },
    /// Repeat the last change (`.`), with a new count if one was typed
    /// (`Vim::repeat`).
    Repeat { count: Option<usize> },
    /// Enter Visual (or, with `linewise`, Visual Line), or switch to it
    /// from the other one, keeping the anchor.
    Visual { linewise: bool },
    /// Leave Visual for Normal.
    Leave,
    /// `:` from Visual: the command line, pre-filled with the selection's
    /// range.
    Cmdline,
    /// `/` (`forward`) or `?`: type a search, from Normal, Visual (to come
    /// back to) or after an operator (`d/foo`), which then acts up to the
    /// match. The `count`th match is the one.
    Search {
        forward: bool,
        op: Option<Operator>,
        count: usize,
    },
    /// `n`, or `N` (`reverse`): the `count`th match of the last search from
    /// the cursor, as a motion or, after an operator, its exclusive target.
    SearchNext {
        reverse: bool,
        op: Option<Operator>,
        count: usize,
    },
    /// An operator over the inner code block (`yic dic cic`).
    InnerCode { op: Operator },
}

/// The Normal-mode command being typed.
#[derive(Debug, Default)]
pub struct Vim {
    count: Count,
    /// `d`, `c` or `y`, waiting for its motion.
    op: Option<Operator>,
    /// `g`, or `i` after an operator, waiting for its second key.
    prefix: Option<char>,
    /// The change the latest command made, until it is done: an Insert
    /// session's change is done at Esc.
    pending: Option<Change>,
    /// The change `.` repeats.
    last: Option<Change>,
}

impl Vim {
    pub fn new() -> Vim {
        Vim::default()
    }

    /// The pending count and keys, for the status line.
    pub fn showcmd(&self) -> String {
        let mut s = String::new();
        if let Some(n) = self.count.pre {
            s.push_str(&n.to_string());
        }
        s.extend(self.op.map(Operator::key));
        if let Some(n) = self.count.post {
            s.push_str(&n.to_string());
        }
        s.extend(self.prefix);
        s
    }

    /// Forgets the pending command (not the change `.` repeats).
    pub fn reset(&mut self) {
        self.count = Count::default();
        self.op = None;
        self.prefix = None;
    }

    /// The change `.` repeats.
    pub fn last_change(&self) -> Option<&Change> {
        self.last.as_ref()
    }

    /// The command that made the pending change is done: it becomes the
    /// change `.` repeats.
    pub fn commit(&mut self) {
        if let Some(c) = self.pending.take() {
            self.last = Some(c);
        }
    }

    /// Forgets the pending change (the command made none, as in reader
    /// mode).
    pub fn discard(&mut self) {
        self.pending = None;
    }

    /// The query and direction an operator's `/` or `?` searched for, so
    /// `.` searches for them again.
    pub fn searched(&mut self, query: &str, forward: bool) {
        if let Some(Change::Search { pattern, .. }) = self.pending.as_mut() {
            *pattern = Some((query.to_owned(), forward));
        }
    }

    /// Text typed in the Insert session of the pending change.
    pub fn typed(&mut self, text: &str) {
        if let Some(t) = self.pending.as_mut().and_then(Change::text_mut) {
            t.push_str(&text.replace("\r\n", "\n"));
        }
    }

    /// Backspace in the Insert session: the last grapheme typed goes. Text
    /// from before the session is not the change's, so past its start
    /// nothing changes.
    pub fn backspaced(&mut self) {
        if let Some(t) = self.pending.as_mut().and_then(Change::text_mut) {
            t.truncate(grapheme::prev(t, t.len()));
        }
    }

    /// An arrow key in the Insert session: as in vim, what is typed from
    /// here is a fresh insert at the new position.
    pub fn arrowed(&mut self) {
        if self.pending.is_some() {
            self.pending = Some(Change::Insert {
                entry: 'i',
                text: String::new(),
            });
        }
    }

    /// Replays the last change at `ctx.cursor`. A `count` replaces the
    /// recorded one, for this and later repeats. With no change recorded
    /// it does nothing. `search` is the last search's query and direction,
    /// which a repeated `dn` (or `d/foo`) looks for.
    pub fn repeat(
        &mut self,
        ctx: &mut EditCtx,
        count: Option<usize>,
        search: Option<(&str, bool)>,
    ) -> OpOutcome {
        let Some(change) = self.last.as_mut() else {
            return OpOutcome {
                cursor: ctx.cursor,
                insert: false,
            };
        };
        if let Some(n) = count {
            change.set_count(n);
        }
        repeat::replay(ctx, change, search)
    }

    /// Handles one Normal-mode key for a cursor at `cur` in `buf`. Esc (and
    /// Ctrl+C, which the editor turns into Esc) clears the pending command.
    /// A key that completes nothing clears it too, count included.
    pub fn key(&mut self, k: &KeyEvent, buf: &Buffer, cur: Cursor) -> Step {
        let m = match k.key {
            Key::Char(c) if k.mods.ctrl => return self.ctrl(c),
            Key::Char(c) => return self.char(c, buf, cur),
            Key::Left => Motion::Left,
            Key::Right => Motion::Right,
            Key::Up => Motion::Up,
            Key::Down => Motion::Down,
            Key::Home => Motion::LineStart,
            Key::End => Motion::LineEnd,
            // TODO(later tasks): Enter, Backspace, Space, Delete, Tab and
            // paging in Normal mode.
            Key::Esc
            | Key::Enter
            | Key::Backspace
            | Key::Delete
            | Key::Tab
            | Key::PageUp
            | Key::PageDown
            | Key::Space => {
                self.reset();
                return Step::Pending;
            }
        };
        self.motion(m, buf, cur)
    }

    /// Ctrl and a char: Ctrl+R redoes; nothing else means anything yet.
    fn ctrl(&mut self, c: char) -> Step {
        let plain = self.op.is_none() && self.prefix.is_none();
        let count = self.count.effective();
        self.reset();
        if c == 'r' && plain {
            Step::Redo { count }
        } else {
            Step::Pending
        }
    }

    /// The keys Normal and Visual share: counts, the `g` prefix and the
    /// motions. `None` when `c` is none of them.
    fn counted(&mut self, c: char, buf: &Buffer, cur: Cursor) -> Option<Step> {
        if let Some(prefix) = self.prefix.take() {
            if prefix == 'g' && c == 'g' {
                let m = if self.count.given() {
                    Motion::GotoLine(self.count.effective())
                } else {
                    Motion::DocStart
                };
                return Some(self.motion(m, buf, cur));
            }
            self.reset();
            return Some(Step::Pending);
        }
        let after_op = self.op.is_some();
        if let Some(d) = c.to_digit(10)
            && (d != 0 || self.count.pending(after_op))
        {
            self.count.push(d as usize, after_op);
            return Some(Step::Pending);
        }
        let m = match c {
            'h' => Motion::Left,
            'l' => Motion::Right,
            'j' => Motion::Down,
            'k' => Motion::Up,
            '0' => Motion::LineStart,
            '^' => Motion::FirstNonBlank,
            '$' => Motion::LineEnd,
            'w' => Motion::WordFwd,
            'b' => Motion::WordBack,
            'e' => Motion::WordEnd,
            'G' if self.count.given() => Motion::GotoLine(self.count.effective()),
            'G' => Motion::DocEnd,
            'g' => {
                self.prefix = Some('g');
                return Some(Step::Pending);
            }
            _ => return None,
        };
        Some(self.motion(m, buf, cur))
    }

    fn char(&mut self, c: char, buf: &Buffer, cur: Cursor) -> Step {
        if self.prefix == Some('i') {
            // `ic` after an operator: the inner code block. Counts do not
            // apply.
            let op = self.op;
            self.reset();
            return match op {
                Some(op) if c == 'c' => {
                    if op != Operator::Yank {
                        self.pending = Some(Change::InnerCode {
                            op,
                            inserted: (op == Operator::Change).then(String::new),
                        });
                    }
                    Step::InnerCode { op }
                }
                _ => Step::Pending,
            };
        }
        if let Some(step) = self.counted(c, buf, cur) {
            return step;
        }
        let after_op = self.op.is_some();
        match c {
            'i' if after_op => {
                self.prefix = Some('i');
                Step::Pending
            }
            'v' | 'V' if !after_op => {
                self.reset();
                Step::Visual { linewise: c == 'V' }
            }
            // `D` is `d$` and `C` is `c$`, counts and all. Without a count
            // they record as themselves.
            'D' | 'C' if !after_op => {
                let given = self.count.given();
                self.op = Some(if c == 'D' {
                    Operator::Delete
                } else {
                    Operator::Change
                });
                let step = self.motion(Motion::LineEnd, buf, cur);
                if !given && self.pending.is_some() {
                    self.pending = Some(if c == 'D' {
                        Change::CapD
                    } else {
                        Change::CapC(String::new())
                    });
                }
                step
            }
            'd' | 'c' | 'y' => self.operator_key(c, buf, cur),
            '/' | '?' | 'n' | 'N' => self.search_key(c),
            'x' if !after_op => {
                let count = self.count.effective();
                self.reset();
                OpRange::chars(buf, cur.pos, count).map_or(Step::Pending, |range| {
                    self.pending = Some(Change::X(count));
                    Step::Operate {
                        op: Operator::Delete,
                        range,
                    }
                })
            }
            'p' | 'P' if !after_op => {
                let count = self.count.effective();
                self.reset();
                self.pending = Some(Change::Put {
                    after: c == 'p',
                    count,
                });
                Step::Put {
                    before: c == 'P',
                    count,
                }
            }
            'i' | 'a' | 'A' | 'I' | 'o' | 'O' if !after_op => {
                // The count is not used: inserting it N times is not built.
                self.reset();
                self.pending = Some(Change::Insert {
                    entry: c,
                    text: String::new(),
                });
                insert_entry(c, buf, cur.pos)
            }
            'u' if !after_op => {
                let count = self.count.effective();
                self.reset();
                Step::Undo { count }
            }
            '.' if !after_op => {
                let count = self.count.given().then(|| self.count.effective());
                self.reset();
                Step::Repeat { count }
            }
            _ => {
                self.reset();
                Step::Pending
            }
        }
    }

    /// `d`, `c` or `y`: starts an operator, or doubled (`dd cc yy`) acts
    /// on `count` lines. Another operator's key cancels.
    fn operator_key(&mut self, c: char, buf: &Buffer, cur: Cursor) -> Step {
        let Some(op) = Operator::from_key(c) else {
            self.reset();
            return Step::Pending;
        };
        match self.op {
            None => {
                self.op = Some(op);
                Step::Pending
            }
            Some(pending) if pending == op => {
                let count = self.count.effective();
                self.reset();
                if op != Operator::Yank {
                    self.pending = Some(Change::Line {
                        op,
                        count,
                        inserted: (op == Operator::Change).then(String::new),
                    });
                }
                Step::Operate {
                    op,
                    range: OpRange::lines(buf, cur.pos.line, cur.pos.col, count),
                }
            }
            Some(_) => {
                self.reset();
                Step::Pending
            }
        }
    }

    /// `/ ? n N`, as motions or after an operator. Under an operator they
    /// record a change that `.` repeats: `dn` as `n` would, and `d/foo`
    /// with its own query once the editor fills it in (`searched`). The
    /// editor discards it when the search finds nothing or is cancelled.
    fn search_key(&mut self, c: char) -> Step {
        let count = self.count.effective();
        let op = self.op;
        self.reset();
        if let Some(op) = op.filter(|&op| op != Operator::Yank) {
            self.pending = Some(Change::Search {
                op,
                reverse: c == 'N',
                count,
                pattern: None,
                inserted: (op == Operator::Change).then(String::new),
            });
        }
        match c {
            '/' | '?' => Step::Search {
                forward: c == '/',
                op,
                count,
            },
            _ => Step::SearchNext {
                reverse: c == 'N',
                op,
                count,
            },
        }
    }

    fn motion(&mut self, m: Motion, buf: &Buffer, cur: Cursor) -> Step {
        let count = self.count.effective();
        let op = self.op;
        self.reset();
        if let Some(op) = op {
            // A failed motion leaves the operator nothing to act on.
            let Some(range) = OpRange::from_motion(op, m, buf, cur, count) else {
                return Step::Pending;
            };
            if op != Operator::Yank {
                self.pending = Some(Change::Op {
                    op,
                    motion: m,
                    count,
                    inserted: (op == Operator::Change).then(String::new),
                });
            }
            return Step::Operate { op, range };
        }
        let r = motion::apply(m, buf, cur, count);
        let pos = rest(buf, r.cursor.pos);
        Step::Move(if pos == r.cursor.pos {
            r.cursor
        } else {
            Cursor::at(buf, pos)
        })
    }
}

/// Where the Normal-mode cursor may rest: on a grapheme start, the line's
/// last one at most.
pub fn rest(buf: &Buffer, pos: Pos) -> Pos {
    let p = buf.clamp(pos);
    Pos {
        col: p.col.min(motion::last_col(buf.line(p.line))),
        ..p
    }
}

/// Where the cursor goes when Insert ends: one grapheme left, never past
/// the line start.
pub fn leave_insert(buf: &Buffer, pos: Pos) -> Pos {
    let p = buf.clamp(pos);
    Pos {
        col: grapheme::prev(buf.line(p.line), p.col),
        ..p
    }
}

/// `i a A I o O`: where Insert starts.
fn insert_entry(c: char, buf: &Buffer, pos: Pos) -> Step {
    match c {
        'o' => Step::Open { above: false },
        'O' => Step::Open { above: true },
        _ => Step::Insert(insert_at(c, buf, pos)),
    }
}

/// Where `i`, `a`, `A` or `I` (any other char) starts Insert for a cursor
/// at `pos`.
pub fn insert_at(c: char, buf: &Buffer, pos: Pos) -> Pos {
    let p = buf.clamp(pos);
    let line = buf.line(p.line);
    let col = match c {
        'a' => grapheme::next(line, p.col),
        'A' => line.len(),
        // On a blank line, after the blanks.
        'I' => motion::first_non_blank(line).unwrap_or(line.len()),
        _ => p.col,
    };
    Pos { col, ..p }
}

#[cfg(test)]
mod tests {
    use crate::editor::Config;
    use crate::effects::Redraw;
    use crate::input::{Input, Key, Mods};
    use crate::testkit::*;
    use crate::vim::Mode;

    const DOC: &str = "hello world\nline two\nline three";

    // The vim-correct cases from the brief.

    #[test]
    fn w_stops_at_empty_line() {
        let mut e = ed("foo\n\nbar");
        keys(&mut e, "w");
        assert_eq!(e.cursor(), pos(1, 0));
    }

    #[test]
    fn unicode_letters_are_word_chars() {
        let mut e = ed("café bar");
        keys(&mut e, "w");
        assert_eq!(e.cursor(), pos(0, 6));
    }

    #[test]
    fn hl_step_by_grapheme() {
        let mut e = ed("a👩‍👩‍👧b");
        keys(&mut e, "l");
        assert_eq!(e.cursor(), pos(0, 1));
        keys(&mut e, "l");
        assert_eq!(e.cursor(), pos(0, 19));
    }

    #[test]
    fn j_keeps_wanted_column_and_dollar_sticks() {
        let mut e = ed("abcdef\nab\nabcdef");
        keys(&mut e, "$jj");
        assert_eq!(e.cursor(), pos(2, 5));
    }

    #[test]
    fn esc_from_insert_steps_left() {
        let mut e = ed("abc");
        keys(&mut e, "A<Esc>");
        assert_eq!(e.cursor(), pos(0, 2));
    }

    #[test]
    fn count_multiplies_and_unused_count_clears() {
        let mut e = ed(&"x\n".repeat(20));
        keys(&mut e, "2j");
        assert_eq!(e.cursor().line, 2);
        keys(&mut e, "3i<Esc>j");
        assert_eq!(e.cursor().line, 3);
    }

    #[test]
    fn showcmd_shows_pending() {
        let mut e = ed("a");
        keys(&mut e, "2d");
        assert_eq!(e.showcmd(), "2d");
    }

    // vim_engine_test.dart: initial state.

    #[test]
    fn starts_in_normal_at_the_document_start() {
        let e = ed(DOC);
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.status().mode, "NORMAL");
    }

    #[test]
    fn bench_mode_still_starts_in_insert() {
        let e = build("some **bold**", Config::default(), true);
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(0, 7));
    }

    #[test]
    fn without_vim_the_editor_stays_insert_only() {
        let mut e = build("ab", Config { vim: false }, false);
        assert_eq!(e.mode(), Mode::Insert);
        keys(&mut e, "j<Esc>");
        assert_eq!(e.text(), "jab");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(0, 1));
    }

    // vim_engine_test.dart: motions in normal mode.

    #[test]
    fn h_and_l_move_along_the_line() {
        let mut e = ed(DOC);
        keys(&mut e, "ll");
        assert_eq!(e.cursor().col, 2);
        keys(&mut e, "h");
        assert_eq!(e.cursor().col, 1);
    }

    #[test]
    fn j_and_k_move_between_lines() {
        let mut e = ed(DOC);
        keys(&mut e, "j");
        assert_eq!(e.cursor().line, 1);
        keys(&mut e, "k");
        assert_eq!(e.cursor().line, 0);
    }

    #[test]
    fn w_zero_and_dollar() {
        let mut e = ed(DOC);
        keys(&mut e, "w");
        assert_eq!(e.cursor().col, 6);
        keys(&mut e, "0");
        assert_eq!(e.cursor().col, 0);
        keys(&mut e, "lll0");
        assert_eq!(e.cursor().col, 0);
        keys(&mut e, "$");
        assert_eq!(e.cursor().col, 10);
    }

    #[test]
    fn gg_and_big_g() {
        let mut e = ed(DOC);
        keys(&mut e, "jj");
        assert_eq!(e.cursor().line, 2);
        keys(&mut e, "gg");
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "G");
        assert_eq!(e.cursor().line, 2);
    }

    // motion_registry_test.dart: each key names its motion.

    #[test]
    fn every_motion_key_moves() {
        for (seq, at) in [
            ("w", pos(0, 2)),
            ("ww", pos(0, 5)),
            ("we", pos(0, 3)),
            ("wwb", pos(0, 2)),
            ("$^", pos(0, 2)),
            ("$0", pos(0, 0)),
            ("j", pos(1, 0)),
            ("jk", pos(0, 0)),
            ("ll", pos(0, 2)),
            ("llh", pos(0, 1)),
            ("G", pos(1, 0)),
            ("Ggg", pos(0, 2)),
        ] {
            let mut e = ed("  ab cd\nx");
            keys(&mut e, seq);
            assert_eq!(e.cursor(), at, "{seq}");
        }
    }

    #[test]
    fn w_on_the_last_word_rests_on_the_last_char() {
        let mut e = ed("hello");
        keys(&mut e, "w");
        assert_eq!(e.cursor(), pos(0, 4));
        keys(&mut e, "b");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn arrows_home_and_end_move_in_normal_mode() {
        let mut e = ed(DOC);
        keys(&mut e, "<Right><Right><Down>");
        assert_eq!(e.cursor(), pos(1, 2));
        keys(&mut e, "<End>");
        assert_eq!(e.cursor(), pos(1, 7));
        keys(&mut e, "<Up><Home>");
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "<Left>");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.text(), DOC);
    }

    // vim_engine_test.dart: mode transitions.

    #[test]
    fn i_enters_insert_and_esc_returns_to_normal() {
        let mut e = ed(DOC);
        keys(&mut e, "i");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.status().mode, "INSERT");
        keys(&mut e, "<Esc>");
        assert_eq!(e.mode(), Mode::Normal);
        // At the line start Esc stays put (moon_edit could go past it).
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn a_enters_insert_after_the_cursor() {
        let mut e = ed(DOC);
        keys(&mut e, "a");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor().col, 1);
    }

    #[test]
    fn typing_in_insert_goes_through_the_editor() {
        let mut e = ed("abc");
        let fx = keys(&mut e, "Ad<Esc>");
        assert_eq!(e.text(), "abcd");
        assert_eq!(fx.dirty, Some(true));
        assert_eq!(e.cursor(), pos(0, 3));
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn esc_from_insert_steps_left_a_whole_grapheme() {
        let mut e = ed("ab👩\u{200d}👩\u{200d}👧");
        keys(&mut e, "A<Esc>");
        assert_eq!(e.cursor(), pos(0, 2));
    }

    #[test]
    fn ctrl_c_acts_as_esc() {
        let mut e = ed("abc");
        keys(&mut e, "A<C-c>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 2));
        keys(&mut e, "3<C-c>");
        assert_eq!(e.showcmd(), "");
        assert_eq!(e.text(), "abc");
    }

    // insert_mode_actions_test.dart and vim_engine_test.dart: insert entries.

    #[test]
    fn i_keeps_the_position() {
        let mut e = ed("hello world");
        keys(&mut e, "lll");
        keys(&mut e, "i");
        assert_eq!(e.cursor(), pos(0, 3));
        let mut e = ed("hello world");
        keys(&mut e, "$i");
        assert_eq!(e.cursor(), pos(0, 10));
    }

    #[test]
    fn a_steps_right_one_grapheme() {
        let mut e = ed("hello");
        keys(&mut e, "lla");
        assert_eq!(e.cursor(), pos(0, 3));
        let mut e = ed("hello");
        keys(&mut e, "$a");
        assert_eq!(e.cursor(), pos(0, 5));
        let mut e = ed("");
        keys(&mut e, "a");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.mode(), Mode::Insert);
        let mut e = ed("👩\u{200d}👩\u{200d}👧x");
        keys(&mut e, "a");
        assert_eq!(e.cursor(), pos(0, 18));
    }

    #[test]
    fn big_a_goes_to_the_line_end() {
        let mut e = ed("hello world");
        keys(&mut e, "lllA");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(0, 11));
        let mut e = ed("first\nsecond");
        keys(&mut e, "jllA");
        assert_eq!(e.cursor(), pos(1, 6));
        let mut e = ed("");
        keys(&mut e, "A");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn big_i_goes_to_the_first_non_blank() {
        for (text, col) in [
            ("  hello", 2),
            ("   hello world", 3),
            ("hello", 0),
            ("  \t  hello", 5),
            ("", 0),
            ("\t\thello", 2),
        ] {
            let mut e = ed(text);
            keys(&mut e, "$I");
            assert_eq!(e.mode(), Mode::Insert, "{text:?}");
            assert_eq!(e.cursor(), pos(0, col), "{text:?}");
        }
    }

    #[test]
    fn o_opens_a_line_below() {
        let mut e = ed(DOC);
        let fx = keys(&mut e, "o");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(1, 0));
        assert_eq!(e.text(), "hello world\n\nline two\nline three");
        assert_eq!(fx.dirty, Some(true));

        let mut e = ed("line one\nline two");
        keys(&mut e, "jllo");
        assert_eq!(e.cursor(), pos(2, 0));
        assert_eq!(e.text(), "line one\nline two\n");

        let mut e = ed("");
        keys(&mut e, "o");
        assert_eq!(e.cursor(), pos(1, 0));
        assert_eq!(e.text(), "\n");
    }

    #[test]
    fn big_o_opens_a_line_above() {
        let mut e = ed(DOC);
        keys(&mut e, "jlllO");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(1, 0));
        assert_eq!(e.text(), "hello world\n\nline two\nline three");

        let mut e = ed("line one\nline two");
        keys(&mut e, "lllO");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.text(), "\nline one\nline two");

        let mut e = ed("");
        keys(&mut e, "O");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.text(), "\n");
    }

    #[test]
    fn o_and_big_o_open_plain_lines_in_blocks() {
        for (text, below, above) in [
            (
                "# Header\nParagraph",
                "# Header\n\nParagraph",
                "\n# Header\nParagraph",
            ),
            ("> Quote\nText", "> Quote\n\nText", "\n> Quote\nText"),
            ("- Item\nText", "- Item\n\nText", "\n- Item\nText"),
        ] {
            let mut e = ed(text);
            keys(&mut e, "llo");
            assert_eq!(e.text(), below);
            assert_eq!(e.cursor(), pos(1, 0));
            let mut e = ed(text);
            keys(&mut e, "llO");
            assert_eq!(e.text(), above);
            assert_eq!(e.cursor(), pos(0, 0));
        }
    }

    #[test]
    fn typing_after_o_fills_the_new_line() {
        let mut e = ed("- item");
        keys(&mut e, "onext<Esc>");
        assert_eq!(e.text(), "- item\nnext");
        assert_eq!(e.cursor(), pos(1, 3));
    }

    // vim_engine_test.dart: pending state.

    #[test]
    fn g_then_another_key_clears_without_moving() {
        let mut e = ed(DOC);
        keys(&mut e, "gx");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.showcmd(), "");
        // The g is gone: a following g alone does not complete gg.
        keys(&mut e, "jg");
        assert_eq!(e.cursor().line, 1);
        assert_eq!(e.showcmd(), "g");
    }

    #[test]
    fn esc_clears_a_pending_operator() {
        let mut e = ed(DOC);
        keys(&mut e, "d<Esc>w");
        assert_eq!(e.cursor(), pos(0, 6));
        assert_eq!(e.text(), DOC);
    }

    #[test]
    fn pending_keys_redraw_only_the_status() {
        let mut e = ed(DOC);
        assert_eq!(keys(&mut e, "2").redraw, Redraw::Status);
        assert_eq!(keys(&mut e, "<Esc>").redraw, Redraw::Status);
    }

    // vim_engine_test.dart: count prefix.

    #[test]
    fn showcmd_shows_counts_and_pending_keys() {
        let e = ed(DOC);
        assert_eq!(e.showcmd(), "");
        for (seq, shown) in [
            ("3", "3"),
            ("12", "12"),
            ("30", "30"),
            ("3d", "3d"),
            ("g", "g"),
            ("5g", "5g"),
            ("2d3", "2d3"),
        ] {
            let mut e = ed(DOC);
            keys(&mut e, seq);
            assert_eq!(e.showcmd(), shown, "{seq}");
        }
    }

    #[test]
    fn zero_without_a_count_is_a_motion() {
        let mut e = ed(DOC);
        keys(&mut e, "ll0");
        assert_eq!(e.cursor().col, 0);
        assert_eq!(e.showcmd(), "");
    }

    #[test]
    fn esc_clears_a_pending_count() {
        let mut e = ed(DOC);
        keys(&mut e, "3");
        assert_eq!(e.showcmd(), "3");
        keys(&mut e, "<Esc>");
        assert_eq!(e.showcmd(), "");
        keys(&mut e, "j");
        assert_eq!(e.cursor().line, 1);
    }

    #[test]
    fn counts_repeat_motions_and_then_clear() {
        let five = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5";
        let mut e = ed(five);
        keys(&mut e, "3j");
        assert_eq!(e.cursor().line, 3);
        assert_eq!(e.showcmd(), "");

        let mut e = ed("one two three four");
        keys(&mut e, "2w");
        assert_eq!(e.cursor().col, 8);

        let mut e = ed(five);
        keys(&mut e, "5G");
        assert_eq!(e.cursor().line, 4);

        let mut e = ed(five);
        keys(&mut e, "3gg");
        assert_eq!(e.cursor().line, 2);
        keys(&mut e, "gg");
        assert_eq!(e.cursor().line, 0);
    }

    #[test]
    fn an_unknown_key_clears_the_count() {
        let mut e = ed(&"x\n".repeat(10));
        keys(&mut e, "3zj");
        assert_eq!(e.cursor().line, 1);
    }

    // vim_engine_test.dart: insert mode key handling.

    #[test]
    fn keys_in_insert_mode_type_text() {
        let mut e = ed("x");
        keys(&mut e, "ia b<CR>c<BS>");
        assert_eq!(e.text(), "a b\nx");
        assert_eq!(e.mode(), Mode::Insert);
    }

    #[test]
    fn committed_text_in_normal_mode_acts_as_keys() {
        let mut e = ed("ab");
        e.handle(Input::Text("lix".into()));
        assert_eq!(e.text(), "axb");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(0, 2));
    }

    #[test]
    fn primary_shortcuts_do_nothing_in_normal_mode() {
        let mut e = ed(DOC);
        let cmd = Mods {
            cmd: true,
            ..Mods::default()
        };
        e.handle(key_with(Key::Char('j'), cmd));
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.text(), DOC);
    }
}

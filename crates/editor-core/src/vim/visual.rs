//! Visual and Visual Line modes: the keys they take, what a selection
//! covers, and repeating a visual change.
//!
//! The editor keeps the anchor; the cursor is the selection's head. A
//! charwise selection takes the grapheme under its end, so every edge sits
//! on a grapheme boundary (R12). An operator leaves Visual for Normal (`c`
//! for Insert). `.` after a visual operator acts on a region the same size
//! from the cursor, as vim does: as many lines (Visual Line), as many
//! graphemes (within one line), or as many lines and the same end column
//! (across lines).

use std::ops::Range;

use crate::buffer::{Buffer, Pos};
use crate::grapheme;
use crate::input::{Key, KeyEvent};
use crate::view_model::Selection;
use crate::vim::motion::{self, Motion};
use crate::vim::operator::{OpRange, Operator};
use crate::vim::register::Register;
use crate::vim::repeat::{Change, Typed};
use crate::vim::{Cursor, Step, Vim, Want};

/// The size of a selection, for `.` to repeat a visual operator over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Visual Line: this many lines.
    Lines(usize),
    /// Charwise within one line: this many graphemes.
    Chars(usize),
    /// Charwise across lines: `lines` lines down, ending on the grapheme in
    /// display column `end`.
    Span { lines: usize, end: usize },
}

/// What an operator acts on for `sel`. Charwise, from the start to just
/// past the grapheme under the end; an empty last line gives its line
/// break, as in vim. Linewise, the whole lines.
pub fn op_range(buf: &Buffer, sel: Selection) -> OpRange {
    let (a, b) = sel.ordered();
    let (a, b) = (buf.clamp(a), buf.clamp(b));
    if sel.linewise {
        // `start` is also the cursor undo restores: the head where it
        // moved up, else column 0 of the first line.
        let backward = sel.head < sel.anchor;
        return OpRange {
            start: Pos {
                line: a.line,
                col: if backward { a.col } else { 0 },
            },
            end: Pos {
                line: b.line,
                col: 0,
            },
            linewise: true,
        };
    }
    let line = buf.line(b.line);
    let end = if b.col >= line.len() && b.line + 1 < buf.line_count() {
        Pos {
            line: b.line + 1,
            col: 0,
        }
    } else {
        Pos {
            col: grapheme::next(line, b.col),
            ..b
        }
    };
    OpRange {
        start: a,
        end,
        linewise: false,
    }
}

/// The text `sel` covers, as a yank of it would store it: the raw source,
/// and for Visual Line the lines joined with a trailing `\n`.
pub fn register(buf: &Buffer, sel: Selection) -> Register {
    let r = op_range(buf, sel);
    if r.linewise {
        let end = Pos {
            line: r.end.line,
            col: buf.line(r.end.line).len(),
        };
        let from = Pos {
            line: r.start.line,
            col: 0,
        };
        let mut text = buf.text_range(from..end);
        text.push('\n');
        Register::lines(text)
    } else {
        Register::chars(buf.text_range(r.start..r.end))
    }
}

/// What a paste over `sel` replaces. A Visual Line selection keeps its
/// line structure: lines from the clipboard (`text` ending in `\n`)
/// replace the selected lines and their breaks, other text replaces just
/// the lines' content. At the end of the buffer the trailing `\n` is
/// dropped from `text`, so no empty line is left behind.
pub fn paste_range(buf: &Buffer, sel: Selection, text: &mut String) -> Range<Pos> {
    let r = op_range(buf, sel);
    if !r.linewise {
        return r.start..r.end;
    }
    let (a, b) = (r.start.line, r.end.line);
    if text.ends_with('\n') {
        if b + 1 < buf.line_count() {
            return Pos { line: a, col: 0 }..Pos {
                line: b + 1,
                col: 0,
            };
        }
        text.pop();
    }
    Pos { line: a, col: 0 }..Pos {
        line: b,
        col: buf.line(b).len(),
    }
}

/// The size of `sel`, for `.`.
pub fn shape(buf: &Buffer, sel: Selection) -> Shape {
    let (a, b) = sel.ordered();
    let (a, b) = (buf.clamp(a), buf.clamp(b));
    if sel.linewise {
        return Shape::Lines(b.line - a.line + 1);
    }
    let end = buf.display_col(b);
    if a.line == b.line {
        Shape::Chars(end - buf.display_col(a) + 1)
    } else {
        Shape::Span {
            lines: b.line - a.line,
            end,
        }
    }
}

/// The region of `shape` starting at `at`, as far as the buffer goes.
pub fn region(buf: &Buffer, at: Pos, shape: Shape) -> OpRange {
    let at = buf.clamp(at);
    let head = match shape {
        Shape::Lines(n) => return OpRange::lines(buf, at.line, at.col, n),
        Shape::Chars(n) => {
            let line = buf.line(at.line);
            let mut col = at.col;
            for _ in 1..n {
                let next = grapheme::next(line, col);
                if next >= line.len() {
                    break;
                }
                col = next;
            }
            Pos { col, ..at }
        }
        Shape::Span { lines, end } => {
            let line = at.line.saturating_add(lines).min(buf.line_count() - 1);
            Pos {
                line,
                col: motion::col_for(buf, line, Want::Col(end)),
            }
        }
    };
    op_range(
        buf,
        Selection {
            anchor: at,
            head,
            linewise: false,
        },
    )
}

impl Vim {
    /// Handles one key in Visual or Visual Line mode, for the selection
    /// `sel` (its head is `cur`). Motions, with counts, move the head.
    pub fn visual_key(&mut self, k: &KeyEvent, buf: &Buffer, cur: Cursor, sel: Selection) -> Step {
        if let Some(step) = self.register_key(k) {
            return step;
        }
        let step = self.visual_step(k, buf, cur, sel);
        self.settle(step)
    }

    fn visual_step(&mut self, k: &KeyEvent, buf: &Buffer, cur: Cursor, sel: Selection) -> Step {
        let m = match k.key {
            Key::Char(c) if !k.mods.ctrl => return self.visual_char(c, buf, cur, sel),
            Key::Left => Motion::Left,
            Key::Right => Motion::Right,
            Key::Up => Motion::Up,
            Key::Down => Motion::Down,
            Key::Home => Motion::LineStart,
            Key::End => Motion::LineEnd,
            Key::Esc => {
                self.reset();
                return Step::Leave;
            }
            // TODO(later tasks): Ctrl keys, Enter, Backspace, Space,
            // Delete, Tab and paging in Visual mode.
            _ => {
                self.reset();
                return Step::Pending;
            }
        };
        self.motion(m, buf, cur)
    }

    fn visual_char(&mut self, c: char, buf: &Buffer, cur: Cursor, sel: Selection) -> Step {
        if let Some(step) = self.counted(c, buf, cur) {
            return step;
        }
        let given = self.count.given().then(|| self.count.effective());
        let count = self.count.effective();
        self.reset();
        match c {
            'v' | 'V' => {
                let linewise = c == 'V';
                if linewise == sel.linewise {
                    Step::Leave
                } else {
                    Step::Visual { linewise }
                }
            }
            'd' | 'x' | 'y' | 'c' => {
                let op = match c {
                    'd' | 'x' => Operator::Delete,
                    'c' => Operator::Change,
                    _ => Operator::Yank,
                };
                if op != Operator::Yank {
                    self.pending = Some(Change::Visual {
                        op,
                        shape: shape(buf, sel),
                        inserted: (op == Operator::Change).then(Typed::default),
                    });
                }
                Step::Operate {
                    op,
                    range: op_range(buf, sel),
                }
            }
            ':' => Step::Cmdline,
            '/' | '?' => Step::Search {
                forward: c == '/',
                op: None,
                count,
            },
            'n' | 'N' => Step::SearchNext {
                reverse: c == 'N',
                op: None,
                count,
            },
            '.' => Step::Repeat { count: given },
            _ => Step::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::input::Input;
    use crate::testkit::*;
    use crate::view_model::Selection;
    use crate::vim::Mode;

    const DOC: &str = "hello world\nline two\nline three";
    const ZWJ: &str = "👩\u{200d}👩\u{200d}👧";

    fn sel(e: &crate::editor::Editor) -> Option<Selection> {
        e.selection()
    }

    // vim_engine_test.dart: mode transitions and visual mode.

    #[test]
    fn v_and_big_v_enter_the_visual_modes_and_esc_leaves() {
        let mut e = ed(DOC);
        keys(&mut e, "v");
        assert_eq!(e.mode(), Mode::Visual);
        keys(&mut e, "<Esc>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(sel(&e), None);
        keys(&mut e, "V");
        assert_eq!(e.mode(), Mode::VisualLine);
        keys(&mut e, "<Esc>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(sel(&e), None);
    }

    #[test]
    fn entering_visual_anchors_at_the_cursor() {
        let mut e = ed(DOC);
        keys(&mut e, "lv");
        let s = sel(&e).unwrap();
        assert_eq!(s.anchor, pos(0, 1));
        assert_eq!(s.head, pos(0, 1));
        assert!(!s.linewise);
    }

    #[test]
    fn motions_extend_the_selection_and_keep_the_anchor() {
        let mut e = ed(DOC);
        keys(&mut e, "vll");
        let s = sel(&e).unwrap();
        assert_eq!(s.anchor, pos(0, 0));
        assert_eq!(s.head, pos(0, 2));
        assert_eq!(e.cursor(), pos(0, 2));
    }

    #[test]
    fn motions_take_counts() {
        let mut e = ed(DOC);
        keys(&mut e, "v2w");
        assert_eq!(e.cursor(), pos(1, 0));
        keys(&mut e, "<Esc>gg");
        keys(&mut e, "v3l");
        assert_eq!(e.cursor(), pos(0, 3));
        keys(&mut e, "2j");
        assert_eq!(e.cursor(), pos(2, 3));
        keys(&mut e, "gg");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(sel(&e).unwrap().anchor, pos(0, 0));
        assert_eq!(e.mode(), Mode::Visual);
    }

    #[test]
    fn v_and_big_v_toggle_off() {
        let mut e = ed(DOC);
        keys(&mut e, "vv");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(sel(&e), None);
        keys(&mut e, "VV");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(sel(&e), None);
    }

    #[test]
    fn v_and_big_v_switch_between_the_modes() {
        let mut e = ed(DOC);
        keys(&mut e, "lVl");
        assert_eq!(e.mode(), Mode::VisualLine);
        assert!(sel(&e).unwrap().linewise);
        keys(&mut e, "v");
        assert_eq!(e.mode(), Mode::Visual);
        let s = sel(&e).unwrap();
        assert_eq!(s.anchor, pos(0, 1));
        assert!(!s.linewise);
        keys(&mut e, "V");
        assert_eq!(e.mode(), Mode::VisualLine);
        assert_eq!(sel(&e).unwrap().anchor, pos(0, 1));
    }

    #[test]
    fn leaving_visual_keeps_the_cursor() {
        let mut e = ed(DOC);
        keys(&mut e, "vjl<Esc>");
        assert_eq!(e.cursor(), pos(1, 1));
    }

    // The operators over a selection (clipboard_test.dart's selection
    // cases and the brief's).

    #[test]
    fn visual_d_takes_the_grapheme_under_the_cursor() {
        let mut e = ed("Hello world");
        keys(&mut e, "lvld");
        assert_eq!(e.text(), "Hlo world");
        assert_eq!(e.register(None).unwrap().text, "el");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 1));
    }

    #[test]
    fn visual_d_then_typing_replaces_the_word() {
        let mut e = ed("Hello world");
        keys(&mut e, "vlllld");
        assert_eq!(e.text(), " world");
        keys(&mut e, "iGoodbye<Esc>");
        assert_eq!(e.text(), "Goodbye world");
    }

    #[test]
    fn visual_multiline_d_removes_exactly_selection() {
        let (mut e, host) = ed_with_host("Line one\nLine two");
        keys(&mut e, "vjd");
        assert_eq!(e.text(), "ine two");
        let r = e.register(None).unwrap();
        assert_eq!(r.text, "Line one\nL");
        assert!(!r.linewise);
        assert_eq!(host.clipboard(), "Line one\nL");
        assert_eq!(e.line_count(), 1);
    }

    #[test]
    fn a_backwards_selection_covers_the_same_text() {
        let mut e = ed("Hello world");
        keys(&mut e, "$vbd");
        assert_eq!(e.text(), "Hello ");
    }

    #[test]
    fn visual_x_deletes_like_d() {
        let mut e = ed("abcdef");
        keys(&mut e, "lvlx");
        assert_eq!(e.text(), "adef");
    }

    #[test]
    fn visual_y_yanks_and_goes_to_the_start() {
        let (mut e, host) = ed_with_host("Hello world");
        keys(&mut e, "wvey");
        assert_eq!(e.text(), "Hello world");
        assert_eq!(host.clipboard(), "world");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 6));
        assert_eq!(sel(&e), None);
        let mut e = ed("Hello world");
        keys(&mut e, "$vby");
        assert_eq!(e.register(None).unwrap().text, "world");
        assert_eq!(e.cursor(), pos(0, 6));
    }

    #[test]
    fn the_selection_is_the_raw_markdown() {
        let mut e = ed("**bold** text");
        keys(&mut e, "v7ly");
        assert_eq!(e.register(None).unwrap().text, "**bold**");
    }

    #[test]
    fn visual_c_enters_insert() {
        let mut e = ed("Hello world");
        keys(&mut e, "vec");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.text(), " world");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.register(None).unwrap().text, "Hello");
        keys(&mut e, "Bye<Esc>");
        assert_eq!(e.text(), "Bye world");
        assert_eq!(e.mode(), Mode::Normal);
        // The delete and the typing are one step.
        keys(&mut e, "u");
        assert_eq!(e.text(), "Hello world");
    }

    #[test]
    fn an_empty_line_in_a_charwise_selection_takes_its_line_break() {
        let mut e = ed("a\n\nb");
        keys(&mut e, "jvd");
        assert_eq!(e.text(), "a\nb");
    }

    #[test]
    fn visual_line_d_y_and_c_take_whole_lines() {
        let mut e = ed("a\nb\nc");
        keys(&mut e, "Vjd");
        assert_eq!(e.text(), "c");
        let r = e.register(None).unwrap();
        assert_eq!(r.text, "a\nb\n");
        assert!(r.linewise);

        let mut e = ed("a\nb\nc");
        keys(&mut e, "jVy");
        assert_eq!(e.register(None).unwrap().text, "b\n");
        assert_eq!(e.text(), "a\nb\nc");
        assert_eq!(e.mode(), Mode::Normal);

        let mut e = ed("a\nb\nc");
        keys(&mut e, "Vjc");
        assert_eq!(e.text(), "\nc");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn each_visual_command_is_one_undo_step() {
        let mut e = ed("Line one\nLine two\nLine three");
        keys(&mut e, "vjd");
        keys(&mut e, "Vjd");
        assert_eq!(e.history().undo_len(), 2);
        keys(&mut e, "u");
        assert_eq!(e.text(), "ine two\nLine three");
        keys(&mut e, "u");
        assert_eq!(e.text(), "Line one\nLine two\nLine three");
        // Moving the selection made no step of its own.
        assert!(!e.history().can_undo());
    }

    #[test]
    fn selections_never_split_graphemes() {
        // A ZWJ family at the selection's end, then at its start.
        let mut e = ed(&format!("a{ZWJ}b"));
        keys(&mut e, "vly");
        assert_eq!(e.register(None).unwrap().text, format!("a{ZWJ}"));
        let mut e = ed(&format!("a{ZWJ}b"));
        keys(&mut e, "lvd");
        assert_eq!(e.text(), "ab");
        assert_eq!(e.register(None).unwrap().text, ZWJ);
        let mut e = ed(&format!("a{ZWJ}b"));
        keys(&mut e, "$vhd");
        assert_eq!(e.text(), "a");
    }

    #[test]
    fn reader_mode_selects_and_yanks_but_does_not_delete() {
        let mut e = build_reader("Hello world");
        keys(&mut e, "vly");
        assert_eq!(e.register(None).unwrap().text, "He");
        keys(&mut e, "vld");
        assert_eq!(e.text(), "Hello world");
        assert_eq!(e.mode(), Mode::Normal);
    }

    // Dot repeat: dot_command_test.dart and vim's rule that a visual
    // operator repeats over a region the same size.

    #[test]
    fn dot_in_visual_leaves_visual_then_repeats() {
        let mut e = ed("hello world");
        keys(&mut e, "x");
        assert_eq!(e.text(), "ello world");
        keys(&mut e, "v");
        keys(&mut e, ".");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.text(), "llo world");
    }

    #[test]
    fn a_visual_delete_repeats_over_the_same_number_of_graphemes() {
        let mut e = ed(&format!("ab{ZWJ}cdef"));
        keys(&mut e, "vld");
        assert_eq!(e.text(), format!("{ZWJ}cdef"));
        keys(&mut e, ".");
        assert_eq!(e.text(), "def");
    }

    #[test]
    fn a_visual_line_delete_repeats_over_the_same_number_of_lines() {
        let mut e = ed("1\n2\n3\n4\n5");
        keys(&mut e, "Vjd.");
        assert_eq!(e.text(), "5");
    }

    #[test]
    fn a_multiline_visual_delete_repeats_to_the_same_end_column() {
        let mut e = ed("abc\ndef\nghi\njkl");
        keys(&mut e, "lvjd");
        assert_eq!(e.text(), "af\nghi\njkl");
        // From the start of "ghi": down a line, to the second column.
        keys(&mut e, "j0.");
        assert_eq!(e.text(), "af\nl");
    }

    #[test]
    fn a_visual_change_repeats_with_its_text() {
        let mut e = ed("aaa bbb");
        keys(&mut e, "vecX<Esc>w.");
        assert_eq!(e.text(), "X X");
    }

    #[test]
    fn a_visual_yank_is_not_a_change() {
        let mut e = ed("abc");
        keys(&mut e, "x");
        keys(&mut e, "vy.");
        assert_eq!(e.text(), "c");
    }

    // Text the input method commits while Visual is up is keys, not typing.

    #[test]
    fn committed_text_in_visual_does_not_type() {
        let mut e = ed("abc");
        keys(&mut e, "v");
        e.handle(Input::Text("zz".into()));
        assert_eq!(e.text(), "abc");
        assert_eq!(e.mode(), Mode::Visual);
        // As keys: `l` extends the selection.
        e.handle(Input::Text("l".into()));
        assert_eq!(e.cursor(), pos(0, 1));
        assert_eq!(e.mode(), Mode::Visual);
    }

    #[test]
    fn committed_text_on_the_command_line_does_not_type() {
        let mut e = ed("abc");
        keys(&mut e, "v:");
        e.handle(Input::Text("zz".into()));
        assert_eq!(e.text(), "abc");
        assert_eq!(e.mode(), Mode::Command);
    }

    // `:` and `/` from Visual: the entry hooks Tasks 24 and 25 build on.

    #[test]
    fn colon_from_visual_prefills_the_range() {
        let mut e = ed("line one\nline two\nline three");
        keys(&mut e, "Vj:");
        assert_eq!(e.mode(), Mode::Command);
        assert_eq!(e.cmdline(), "'<,'>");
        let s = e.last_visual().unwrap();
        assert_eq!(s.ordered().0.line, 0);
        assert_eq!(s.ordered().1.line, 1);
        assert!(s.linewise);
        assert_eq!(sel(&e), None);
        keys(&mut e, "<Esc>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cmdline(), "");
    }

    #[test]
    fn slash_from_visual_returns_to_visual() {
        let mut e = ed(DOC);
        keys(&mut e, "vl/");
        assert_eq!(e.mode(), Mode::Search);
        assert!(sel(&e).is_some());
        keys(&mut e, "<Esc>");
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(sel(&e).unwrap().anchor, pos(0, 0));
        keys(&mut e, "/<CR>");
        assert_eq!(e.mode(), Mode::Visual);
        let mut e = ed(DOC);
        keys(&mut e, "V/<Esc>");
        assert_eq!(e.mode(), Mode::VisualLine);
    }

    // Selection::ranges, for paint.

    #[test]
    fn ranges_give_each_line_its_selected_bytes() {
        let mut e = ed("abc\n\ndef");
        keys(&mut e, "lvjj");
        let s = sel(&e).unwrap();
        assert_eq!(s.ranges(e.buffer()), vec![(0, 1..3), (1, 0..0), (2, 0..2)]);
        keys(&mut e, "V");
        let s = sel(&e).unwrap();
        assert_eq!(s.ranges(e.buffer()), vec![(0, 0..3), (1, 0..0), (2, 0..3)]);
    }

    #[test]
    fn ranges_take_the_whole_last_grapheme() {
        let mut e = ed(&format!("a{ZWJ}b"));
        keys(&mut e, "vl");
        let s = sel(&e).unwrap();
        assert_eq!(s.ranges(e.buffer()), vec![(0, 0..1 + ZWJ.len())]);
    }

    #[test]
    fn the_selection_shows_only_in_visual() {
        let mut e = ed(DOC);
        assert_eq!(sel(&e), None);
        keys(&mut e, "vl");
        assert!(sel(&e).is_some());
        keys(&mut e, "y");
        assert_eq!(sel(&e), None);
    }

    // Clipboard keys: clipboard_test.dart and the owner's decisions.

    #[test]
    fn primary_c_copies_the_raw_selection_and_stays_in_visual() {
        let (mut e, host) = ed_with_host("Hello **world**");
        keys(&mut e, "lvlll<D-c>");
        assert_eq!(host.clipboard(), "ello");
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(e.text(), "Hello **world**");
        keys(&mut e, "<Esc>wv$<D-c>");
        assert_eq!(host.clipboard(), "**world**");
    }

    #[test]
    fn primary_c_in_visual_line_copies_the_lines_and_a_newline() {
        let (mut e, host) = ed_with_host("First line\nSecond line\nThird");
        keys(&mut e, "Vj<D-c>");
        assert_eq!(host.clipboard(), "First line\nSecond line\n");
        // A put uses it as lines.
        keys(&mut e, "<Esc>Gp");
        assert_eq!(
            e.text(),
            "First line\nSecond line\nThird\nFirst line\nSecond line"
        );
    }

    #[test]
    fn copy_and_cut_without_a_selection_do_nothing() {
        let (mut e, host) = ed_with_host("Hello world");
        host.set_clipboard("before");
        let fx = keys(&mut e, "<D-c><D-x>");
        assert_eq!(host.clipboard(), "before");
        assert_eq!(e.text(), "Hello world");
        assert_eq!(fx.dirty, None);
        keys(&mut e, "i<D-c><D-x>");
        assert_eq!(host.clipboard(), "before");
        assert_eq!(e.text(), "Hello world");
    }

    #[test]
    fn primary_x_copies_then_deletes() {
        let (mut e, host) = ed_with_host("Hello world");
        keys(&mut e, "vllll<D-x>");
        assert_eq!(host.clipboard(), "Hello");
        assert_eq!(e.text(), " world");
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "u");
        assert_eq!(e.text(), "Hello world");

        let (mut e, host) = ed_with_host("a\nb\nc");
        keys(&mut e, "jV<D-x>");
        assert_eq!(host.clipboard(), "b\n");
        assert_eq!(e.text(), "a\nc");
    }

    #[test]
    fn primary_v_pastes_at_the_cursor() {
        let (mut e, host) = ed_with_host("Hello");
        host.set_clipboard("World ");
        keys(&mut e, "<D-v>");
        assert_eq!(e.text(), "World Hello");
        assert_eq!(e.mode(), Mode::Normal);

        let (mut e, host) = ed_with_host("Hello");
        host.set_clipboard("World\nGoodbye");
        keys(&mut e, "A<D-v>");
        assert_eq!(e.text(), "HelloWorld\nGoodbye");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(1, 7));
    }

    #[test]
    fn paste_is_literal_and_one_undo_step() {
        let (mut e, host) = ed_with_host("- item");
        host.set_clipboard("- a\n- b");
        keys(&mut e, "A<CR>");
        let before = e.text();
        // Typing Enter in a list continues it...
        assert_eq!(before, "- item\n- ");
        keys(&mut e, "<D-v>");
        // ...but a paste's newlines never do.
        assert_eq!(e.text(), "- item\n- - a\n- b");
        assert_eq!(e.cursor(), pos(2, 3));
        keys(&mut e, "<Esc>u");
        assert_eq!(e.text(), before);

        let (mut e, host) = ed_with_host("- item");
        host.set_clipboard("- a\n- b");
        keys(&mut e, "$<D-v>");
        assert_eq!(e.text(), "- ite- a\n- bm");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), "- item");
    }

    #[test]
    fn paste_replaces_the_selection_and_keeps_the_clipboard() {
        let (mut e, host) = ed_with_host("Hello world");
        host.set_clipboard("Goodbye");
        keys(&mut e, "vllll<D-v>");
        assert_eq!(e.text(), "Goodbye world");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(host.clipboard(), "Goodbye");
        keys(&mut e, "u");
        assert_eq!(e.text(), "Hello world");

        let (mut e, host) = ed_with_host("a\nb\nc");
        host.set_clipboard("x\ny\n");
        keys(&mut e, "jV<D-v>");
        assert_eq!(e.text(), "a\nx\ny\nc");
    }

    #[test]
    fn paste_with_an_empty_clipboard_or_in_reader_mode_does_nothing() {
        let (mut e, _host) = ed_with_host("abc");
        keys(&mut e, "<D-v>");
        assert_eq!(e.text(), "abc");
        assert!(!e.history().can_undo());
        let mut e = build_reader("abc");
        e.host().clipboard_set("x");
        keys(&mut e, "<D-v>");
        assert_eq!(e.text(), "abc");
    }

    #[test]
    fn ctrl_c_without_selection_is_esc() {
        let (mut e, host) = ed_with_host("abc");
        host.set_clipboard("before");
        keys(&mut e, "A<C-c>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 2));
        assert_eq!(host.clipboard(), "before");
        keys(&mut e, "2<C-c>");
        assert_eq!(e.showcmd(), "");
    }

    #[test]
    fn ctrl_c_with_a_selection_copies() {
        let (mut e, host) = ed_with_host("abc");
        keys(&mut e, "vl<C-c>");
        assert_eq!(host.clipboard(), "ab");
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(e.text(), "abc");
    }

    #[test]
    fn on_linux_ctrl_is_primary() {
        let (mut e, host) = ed_linux_with_host("abc");
        keys(&mut e, "vl<C-c>");
        assert_eq!(host.clipboard(), "ab");
        assert_eq!(e.mode(), Mode::Visual);
        keys(&mut e, "<Esc>A<C-c>");
        assert_eq!(e.mode(), Mode::Normal);
        host.set_clipboard("Z");
        keys(&mut e, "0<C-v>");
        assert_eq!(e.text(), "Zabc");
        // Cmd is not primary on Linux.
        keys(&mut e, "<D-v>");
        assert_eq!(e.text(), "Zabc");
    }

    #[test]
    fn without_vim_primary_v_pastes_and_copy_does_nothing() {
        let (mut e, host) = plain_with_host("ab");
        host.set_clipboard("XY");
        keys(&mut e, "<D-v>");
        assert_eq!(e.text(), "XYab");
        assert_eq!(e.cursor(), pos(0, 2));
        keys(&mut e, "<D-c><D-x>");
        assert_eq!(host.clipboard(), "XY");
        assert_eq!(e.text(), "XYab");
    }
}

//! Cursor motions over the buffer, grapheme by grapheme.

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{Buffer, Pos};
use crate::grapheme;
use crate::vim::{Cursor, Want};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Down,
    Up,
    LineStart,
    FirstNonBlank,
    LineEnd,
    WordFwd,
    WordBack,
    WordEnd,
    DocStart,
    DocEnd,
    /// The 1-based line number from a count (`5G`, `5gg`).
    GotoLine(usize),
}

/// How an operator treats the text a motion covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionKind {
    Exclusive,
    Inclusive,
    Linewise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionResult {
    pub cursor: Cursor,
    pub kind: MotionKind,
    /// False when the motion failed: a relative motion (`h l j k w b e`)
    /// that could not move at all, as `j` on the last line. Vim beeps and
    /// an operator over a failed motion does nothing. Absolute motions
    /// (`0 ^ $ gg G`) never fail, even when they land where they started.
    pub moved: bool,
}

/// Where `m`, repeated `count` times (0 counts as 1), takes `cur`.
///
/// Every position comes from `Buffer::clamp` and the grapheme helpers, so
/// none splits a cluster. Columns rest on a grapheme start, at most the
/// line's last one, with one exception: `w` past the last word of the
/// buffer lands on the line end, so an operator covers that word. The
/// engine pulls the Normal-mode cursor back onto the last grapheme.
pub fn apply(m: Motion, buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    use Motion::*;
    let count = count.max(1);
    let p = buf.clamp(cur.pos);
    let line = buf.line(p.line);
    let last_line = buf.line_count() - 1;
    let at = |pos| Cursor::at(buf, pos);
    let (cursor, kind) = match m {
        Left => (
            at(repeat(count, p, |q| Pos {
                col: grapheme::prev(line, q.col),
                ..q
            })),
            MotionKind::Exclusive,
        ),
        Right => (
            at(repeat(count, p, |q| Pos {
                col: grapheme::next(line, q.col).min(last_col(line)),
                ..q
            })),
            MotionKind::Exclusive,
        ),
        Down | Up => {
            let target = if m == Down {
                p.line.saturating_add(count).min(last_line)
            } else {
                p.line.saturating_sub(count)
            };
            let cursor = if target == p.line {
                Cursor {
                    pos: p,
                    want: cur.want,
                }
            } else {
                Cursor {
                    pos: Pos {
                        line: target,
                        col: col_for(buf, target, cur.want),
                    },
                    want: cur.want,
                }
            };
            (cursor, MotionKind::Linewise)
        }
        LineStart => (at(Pos { col: 0, ..p }), MotionKind::Exclusive),
        FirstNonBlank => (
            at(Pos {
                col: caret_col(line),
                ..p
            }),
            MotionKind::Exclusive,
        ),
        LineEnd => {
            let target = p.line.saturating_add(count - 1).min(last_line);
            let pos = Pos {
                line: target,
                col: last_col(buf.line(target)),
            };
            (
                Cursor {
                    pos,
                    want: Want::End,
                },
                MotionKind::Inclusive,
            )
        }
        WordFwd => (
            at(repeat(count, p, |q| word_fwd(buf, q))),
            MotionKind::Exclusive,
        ),
        WordBack => (
            at(repeat(count, p, |q| word_back(buf, q))),
            MotionKind::Exclusive,
        ),
        WordEnd => (
            at(repeat(count, p, |q| word_end(buf, q))),
            MotionKind::Inclusive,
        ),
        DocStart => (at(line_start(buf, 0)), MotionKind::Linewise),
        DocEnd => (at(line_start(buf, last_line)), MotionKind::Linewise),
        GotoLine(n) => (
            at(line_start(buf, n.saturating_sub(1).min(last_line))),
            MotionKind::Linewise,
        ),
    };
    let moved = match m {
        Left | Right | Down | Up | WordFwd | WordBack | WordEnd => cursor.pos != p,
        LineStart | FirstNonBlank | LineEnd | DocStart | DocEnd | GotoLine(_) => true,
    };
    MotionResult {
        cursor,
        kind,
        moved,
    }
}

/// True when `p` is on the last grapheme of a word (as `e` stops), so `cw`
/// there changes only that grapheme.
pub fn at_word_end(buf: &Buffer, p: Pos) -> bool {
    let line = buf.line(p.line);
    let c = class_at(line, p.col);
    c != Class::Blank && class_at(line, grapheme::next(line, p.col)) != c
}

/// True when `p` is on a blank (whitespace, or the end of its line).
pub fn on_blank(buf: &Buffer, p: Pos) -> bool {
    class(buf, p) == Class::Blank
}

/// One `w` step from `p`, for an operator: the step does not cross a line
/// end unless it starts on an empty line. Vim stops the operated text at
/// the end of the line the last word moved over is on.
pub fn word_fwd_for_operator(buf: &Buffer, p: Pos) -> Pos {
    let q = word_fwd(buf, p);
    if q.line == p.line {
        return q;
    }
    if buf.line(p.line).is_empty() {
        // Onto the next line's start; the operator's exclusive rule then
        // makes it linewise.
        return Pos {
            line: p.line + 1,
            col: 0,
        };
    }
    Pos {
        line: p.line,
        col: buf.line(p.line).len(),
    }
}

/// Applies `step` up to `count` times, stopping early once it stops moving
/// (so a huge count costs no more than the text it crosses).
fn repeat(count: usize, mut p: Pos, step: impl Fn(Pos) -> Pos) -> Pos {
    for _ in 0..count {
        let q = step(p);
        if q == p {
            break;
        }
        p = q;
    }
    p
}

/// The start of the line's last grapheme, where Normal mode rests at the
/// line end. 0 on an empty line.
pub fn last_col(line: &str) -> usize {
    grapheme::prev(line, line.len())
}

/// The start of the first grapheme that is not a space or a tab.
pub fn first_non_blank(line: &str) -> Option<usize> {
    line.grapheme_indices(true)
        .find(|(_, g)| !g.starts_with([' ', '\t']))
        .map(|(i, _)| i)
}

/// Where `^` lands: the first non-blank, or the last char of a blank line.
pub fn caret_col(line: &str) -> usize {
    first_non_blank(line).unwrap_or_else(|| last_col(line))
}

/// The first non-blank of `line` (as `gg` and `G` land).
fn line_start(buf: &Buffer, line: usize) -> Pos {
    Pos {
        line,
        col: caret_col(buf.line(line)),
    }
}

/// The byte column on `line` for `want`, on the last grapheme at most.
pub fn col_for(buf: &Buffer, line: usize, want: Want) -> usize {
    match want {
        Want::End => last_col(buf.line(line)),
        Want::Col(g) => buf
            .grapheme_start(line, g)
            .unwrap_or_else(|| last_col(buf.line(line))),
    }
}

/// What a word motion sees at a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Whitespace, and the end of a line.
    Blank,
    /// `char::is_alphanumeric` or `_`.
    Word,
    /// Anything else. A run of it is a word of its own.
    Punct,
}

fn class_at(line: &str, col: usize) -> Class {
    match line.get(col..).and_then(|s| s.chars().next()) {
        None => Class::Blank,
        Some(c) if c.is_whitespace() => Class::Blank,
        Some(c) if c.is_alphanumeric() || c == '_' => Class::Word,
        Some(_) => Class::Punct,
    }
}

fn class(buf: &Buffer, p: Pos) -> Class {
    class_at(buf.line(p.line), p.col)
}

/// One grapheme on, stepping from a line's end to the next line's start.
/// `None` at the end of the buffer.
fn inc(buf: &Buffer, p: Pos) -> Option<Pos> {
    let line = buf.line(p.line);
    if p.col < line.len() {
        Some(Pos {
            col: grapheme::next(line, p.col),
            ..p
        })
    } else if p.line + 1 < buf.line_count() {
        Some(Pos {
            line: p.line + 1,
            col: 0,
        })
    } else {
        None
    }
}

/// One grapheme back, stepping from a line's start to the previous line's
/// end. `None` at the start of the buffer.
fn dec(buf: &Buffer, p: Pos) -> Option<Pos> {
    if p.col > 0 {
        Some(Pos {
            col: grapheme::prev(buf.line(p.line), p.col),
            ..p
        })
    } else if p.line > 0 {
        Some(Pos {
            line: p.line - 1,
            col: buf.line(p.line - 1).len(),
        })
    } else {
        None
    }
}

/// `w`: past the rest of this word, then past blanks and line ends to the
/// next word or empty line. At the end of the buffer: its last line's end.
fn word_fwd(buf: &Buffer, mut p: Pos) -> Pos {
    let start = class(buf, p);
    if start != Class::Blank {
        // A line end is blank, so this never leaves the line.
        while class(buf, p) == start {
            p.col = grapheme::next(buf.line(p.line), p.col);
        }
    }
    loop {
        if class(buf, p) != Class::Blank {
            return p;
        }
        let line = buf.line(p.line);
        if p.col < line.len() {
            p.col = grapheme::next(line, p.col);
            continue;
        }
        if p.line + 1 >= buf.line_count() {
            return p;
        }
        p = Pos {
            line: p.line + 1,
            col: 0,
        };
        if buf.line(p.line).is_empty() {
            return p;
        }
    }
}

/// `b`: back past blanks and line ends (stopping on an empty line), then to
/// the start of the word there.
fn word_back(buf: &Buffer, p: Pos) -> Pos {
    let Some(mut p) = dec(buf, p) else {
        return p;
    };
    loop {
        if buf.line(p.line).is_empty() {
            return p;
        }
        if class(buf, p) != Class::Blank {
            break;
        }
        match dec(buf, p) {
            Some(q) => p = q,
            None => return p,
        }
    }
    let c = class(buf, p);
    let line = buf.line(p.line);
    while p.col > 0 {
        let q = grapheme::prev(line, p.col);
        if class_at(line, q) != c {
            break;
        }
        p.col = q;
    }
    p
}

/// `e`: to the last grapheme of this word, or of the next one when already
/// there. Empty lines are not stops. Stays put when no word end follows.
fn word_end(buf: &Buffer, from: Pos) -> Pos {
    let start = class(buf, from);
    let Some(mut p) = inc(buf, from) else {
        return from;
    };
    if start == Class::Blank || class(buf, p) != start {
        while class(buf, p) == Class::Blank {
            match inc(buf, p) {
                Some(q) => p = q,
                None => return from,
            }
        }
    }
    let c = class(buf, p);
    let line = buf.line(p.line);
    while p.col < line.len() && class_at(line, p.col) == c {
        p.col = grapheme::next(line, p.col);
    }
    Pos {
        col: grapheme::prev(line, p.col),
        ..p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(buf: &Buffer, line: usize, col: usize) -> Cursor {
        Cursor {
            pos: Pos { line, col },
            want: Want::Col(grapheme::display_col(buf.line(line), col)),
        }
    }

    /// Where `m` (with `count`) takes a cursor at `line`, `col` in `text`.
    fn go(m: Motion, text: &str, line: usize, col: usize, count: usize) -> MotionResult {
        let buf = Buffer::from_text(text);
        apply(m, &buf, cursor(&buf, line, col), count)
    }

    fn to(m: Motion, text: &str, line: usize, col: usize) -> Pos {
        go(m, text, line, col, 1).cursor.pos
    }

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    // character_motions_test.dart

    #[test]
    fn h_moves_left_and_stops_at_line_start() {
        assert_eq!(to(Motion::Left, "Hello world", 0, 5), p(0, 4));
        assert_eq!(to(Motion::Left, "Hello world", 0, 0), p(0, 0));
        assert_eq!(to(Motion::Left, "First line\nSecond line", 1, 3), p(1, 2));
        let r = go(Motion::Left, "Hello world", 0, 5, 1);
        assert_eq!(r.cursor.want, Want::Col(4));
    }

    #[test]
    fn l_moves_right_and_stops_on_the_last_char() {
        assert_eq!(to(Motion::Right, "Hello world", 0, 4), p(0, 5));
        assert_eq!(to(Motion::Right, "Hello", 0, 4), p(0, 4));
        assert_eq!(to(Motion::Right, "", 0, 0), p(0, 0));
        assert_eq!(to(Motion::Right, "First line\nSecond line", 1, 3), p(1, 4));
        assert_eq!(
            go(Motion::Right, "Hello", 0, 4, 1).cursor.want,
            Want::Col(4)
        );
    }

    #[test]
    fn h_and_l_take_counts_and_clamp() {
        assert_eq!(go(Motion::Left, "Hello World", 0, 8, 3).cursor.pos, p(0, 5));
        assert_eq!(go(Motion::Left, "Hello", 0, 2, 10).cursor.pos, p(0, 0));
        assert_eq!(
            go(Motion::Right, "Hello World", 0, 0, 3).cursor.pos,
            p(0, 3)
        );
        assert_eq!(go(Motion::Right, "Hello", 0, 2, 10).cursor.pos, p(0, 4));
        // A huge count finishes without stepping a billion times.
        assert_eq!(
            go(Motion::Right, "Hello", 0, 2, usize::MAX).cursor.pos,
            p(0, 4)
        );
    }

    #[test]
    fn h_and_l_step_whole_graphemes() {
        let s = "e\u{301}👩\u{200d}👩\u{200d}👧x";
        assert_eq!(to(Motion::Right, s, 0, 0), p(0, 3));
        assert_eq!(to(Motion::Right, s, 0, 3), p(0, 21));
        assert_eq!(to(Motion::Left, s, 0, 21), p(0, 3));
        // A position inside a cluster snaps back first.
        assert_eq!(to(Motion::Right, s, 0, 7), p(0, 21));
    }

    // document_motions_test.dart

    #[test]
    fn gg_goes_to_the_first_line() {
        let r = go(Motion::DocStart, "Hello\nWorld\nTest", 2, 2, 1);
        assert_eq!(r.cursor.pos, p(0, 0));
        assert_eq!(r.cursor.want, Want::Col(0));
        assert_eq!(to(Motion::DocStart, "Hello\nWorld", 0, 0), p(0, 0));
    }

    #[test]
    fn big_g_goes_to_the_last_line() {
        let r = go(Motion::DocEnd, "Hello\nWorld\nTest", 0, 2, 1);
        assert_eq!(r.cursor.pos, p(2, 0));
        assert_eq!(r.cursor.want, Want::Col(0));
        assert_eq!(to(Motion::DocEnd, "Hello", 0, 3), p(0, 0));
        assert_eq!(to(Motion::DocEnd, "Hello\nWorld", 1, 0), p(1, 0));
    }

    #[test]
    fn goto_line_is_one_based_and_clamps() {
        let five = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5";
        assert_eq!(to(Motion::DocEnd, five, 0, 0).line, 4);
        assert_eq!(to(Motion::GotoLine(5), five, 0, 0).line, 4);
        assert_eq!(
            to(Motion::GotoLine(1), "Line 1\nLine 2\nLine 3", 2, 0).line,
            0
        );
        assert_eq!(
            to(Motion::GotoLine(100), "Line 1\nLine 2\nLine 3", 0, 0).line,
            2
        );
        assert_eq!(to(Motion::GotoLine(0), "a\nb", 1, 0).line, 0);
    }

    #[test]
    fn gg_and_big_g_land_on_the_first_non_blank() {
        assert_eq!(to(Motion::DocEnd, "a\n  b", 0, 0), p(1, 2));
        assert_eq!(to(Motion::DocStart, "\t x\nb", 1, 0), p(0, 2));
        assert_eq!(to(Motion::GotoLine(2), "a\n   b\nc", 0, 0), p(1, 3));
    }

    // line_motions_test.dart

    #[test]
    fn j_moves_down_at_the_wanted_column() {
        assert_eq!(to(Motion::Down, "Hello\nWorld", 0, 2), p(1, 2));
        // Clamps to the last char of a shorter line.
        assert_eq!(to(Motion::Down, "Hello World\nHi", 0, 8), p(1, 1));
        assert_eq!(to(Motion::Down, "Hello\nX", 0, 3), p(1, 0));
        // Stays on the last line.
        assert_eq!(to(Motion::Down, "Hello\nWorld", 1, 2), p(1, 2));
    }

    #[test]
    fn j_and_k_keep_the_want() {
        let buf = Buffer::from_text("Hi\nHello World");
        let cur = Cursor {
            pos: p(0, 1),
            want: Want::Col(100),
        };
        let r = apply(Motion::Down, &buf, cur, 1);
        assert_eq!(r.cursor.pos, p(1, 10));
        assert_eq!(r.cursor.want, Want::Col(100));

        let buf = Buffer::from_text("Hello World\nHi");
        let cur = Cursor {
            pos: p(1, 1),
            want: Want::Col(100),
        };
        let r = apply(Motion::Up, &buf, cur, 1);
        assert_eq!(r.cursor.pos, p(0, 10));
        assert_eq!(r.cursor.want, Want::Col(100));
    }

    #[test]
    fn j_and_k_take_counts_and_clamp() {
        let five = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5";
        assert_eq!(go(Motion::Down, five, 0, 0, 3).cursor.pos.line, 3);
        assert_eq!(
            go(Motion::Down, "Line 1\nLine 2\nLine 3", 0, 0, 10)
                .cursor
                .pos
                .line,
            2
        );
        assert_eq!(
            go(Motion::Down, "Line 1\nLine 2", 0, 0, 1).cursor.pos.line,
            1
        );
        assert_eq!(go(Motion::Up, five, 4, 0, 3).cursor.pos.line, 1);
        assert_eq!(
            go(Motion::Up, "Line 1\nLine 2\nLine 3", 2, 0, 10)
                .cursor
                .pos
                .line,
            0
        );
        assert_eq!(go(Motion::Down, five, 0, 0, usize::MAX).cursor.pos.line, 4);
    }

    #[test]
    fn k_moves_up_at_the_wanted_column() {
        assert_eq!(to(Motion::Up, "Hello\nWorld", 1, 3), p(0, 3));
        assert_eq!(to(Motion::Up, "Hello\nWorld", 0, 2), p(0, 2));
        assert_eq!(to(Motion::Up, "Hi\nHello World", 1, 8), p(0, 1));
        assert_eq!(to(Motion::Up, "X\nHello", 1, 3), p(0, 0));
    }

    #[test]
    fn j_keeps_the_display_column_across_graphemes() {
        // e-acute is one grapheme of three bytes.
        assert_eq!(to(Motion::Down, "e\u{301}bcd\nxyz", 0, 4), p(1, 2));
        assert_eq!(to(Motion::Up, "e\u{301}bcd\nxyz", 1, 2), p(0, 4));
    }

    // line_position_motions_test.dart

    #[test]
    fn zero_goes_to_the_line_start() {
        let r = go(Motion::LineStart, "Hello World", 0, 5, 1);
        assert_eq!(r.cursor.pos, p(0, 0));
        assert_eq!(r.cursor.want, Want::Col(0));
        assert_eq!(to(Motion::LineStart, "Hello\nWorld", 1, 3), p(1, 0));
    }

    #[test]
    fn caret_goes_to_the_first_non_blank() {
        assert_eq!(to(Motion::FirstNonBlank, "   Hello", 0, 6), p(0, 3));
        assert_eq!(to(Motion::FirstNonBlank, "Hello", 0, 3), p(0, 0));
        assert_eq!(to(Motion::FirstNonBlank, "\tHello", 0, 4), p(0, 1));
        assert_eq!(to(Motion::FirstNonBlank, " \t  Hello", 0, 7), p(0, 4));
        assert_eq!(to(Motion::FirstNonBlank, "", 0, 0), p(0, 0));
        let r = go(Motion::FirstNonBlank, "   Hello", 0, 6, 1);
        assert_eq!(r.cursor.want, Want::Col(3));
    }

    #[test]
    fn caret_on_a_blank_line_lands_on_its_last_char() {
        // Vim. moon_edit's markdown parser trimmed the line, so it said 0.
        assert_eq!(to(Motion::FirstNonBlank, "   ", 0, 0), p(0, 2));
    }

    #[test]
    fn dollar_goes_to_the_last_char_and_sticks() {
        let r = go(Motion::LineEnd, "Hello", 0, 1, 1);
        assert_eq!(r.cursor.pos, p(0, 4));
        assert_eq!(r.cursor.want, Want::End);
        let r = go(Motion::LineEnd, "", 0, 0, 1);
        assert_eq!(r.cursor.pos, p(0, 0));
        assert_eq!(r.cursor.want, Want::End);
        assert_eq!(to(Motion::LineEnd, "Hello\nWorld", 1, 0), p(1, 4));

        // Sticky end: after $, j goes to the end of a longer line.
        let buf = Buffer::from_text("Hi\nHello World");
        let end = apply(Motion::LineEnd, &buf, cursor(&buf, 0, 0), 1).cursor;
        assert_eq!(end.pos, p(0, 1));
        assert_eq!(apply(Motion::Down, &buf, end, 1).cursor.pos, p(1, 10));
    }

    #[test]
    fn dollar_with_a_count_goes_down_count_minus_one_lines() {
        assert_eq!(
            go(Motion::LineEnd, "ab\ncde\nf", 0, 0, 2).cursor.pos,
            p(1, 2)
        );
        assert_eq!(
            go(Motion::LineEnd, "ab\ncde\nf", 0, 0, 9).cursor.pos,
            p(2, 0)
        );
    }

    #[test]
    fn dollar_lands_on_the_last_grapheme() {
        assert_eq!(
            to(Motion::LineEnd, "ab👩\u{200d}👩\u{200d}👧", 0, 0),
            p(0, 2)
        );
    }

    // word_motions_test.dart

    #[test]
    fn w_moves_to_the_next_word() {
        let r = go(Motion::WordFwd, "hello world", 0, 0, 1);
        assert_eq!(r.cursor.pos, p(0, 6));
        assert_eq!(r.cursor.want, Want::Col(6));
        assert_eq!(to(Motion::WordFwd, "hello   world", 0, 0), p(0, 8));
        assert_eq!(to(Motion::WordFwd, "hello world", 0, 2), p(0, 6));
        assert_eq!(to(Motion::WordFwd, "hello,world", 0, 0), p(0, 5));
        assert_eq!(to(Motion::WordFwd, "hello\nworld", 0, 4), p(1, 0));
        assert_eq!(to(Motion::WordFwd, "", 0, 0), p(0, 0));
    }

    #[test]
    fn w_at_the_last_word_goes_past_the_end_for_operators() {
        // The engine clamps the cursor back onto the last char.
        assert_eq!(to(Motion::WordFwd, "hello", 0, 0), p(0, 5));
    }

    #[test]
    fn b_moves_to_the_previous_word_start() {
        let r = go(Motion::WordBack, "hello world", 0, 6, 1);
        assert_eq!(r.cursor.pos, p(0, 0));
        assert_eq!(r.cursor.want, Want::Col(0));
        assert_eq!(to(Motion::WordBack, "hello world", 0, 3), p(0, 0));
        assert_eq!(to(Motion::WordBack, "hello", 0, 0), p(0, 0));
        assert_eq!(to(Motion::WordBack, "hello   world", 0, 8), p(0, 0));
        assert_eq!(to(Motion::WordBack, "hello,world", 0, 6), p(0, 5));
        assert_eq!(to(Motion::WordBack, "hello\nworld", 1, 0), p(0, 0));
    }

    #[test]
    fn e_moves_to_the_word_end() {
        let r = go(Motion::WordEnd, "hello world", 0, 0, 1);
        assert_eq!(r.cursor.pos, p(0, 4));
        assert_eq!(r.cursor.want, Want::Col(4));
        assert_eq!(to(Motion::WordEnd, "hello world", 0, 4), p(0, 10));
        assert_eq!(to(Motion::WordEnd, "hello world", 0, 2), p(0, 4));
        assert_eq!(to(Motion::WordEnd, "hello   world", 0, 5), p(0, 12));
        assert_eq!(to(Motion::WordEnd, "hello,world", 0, 0), p(0, 4));
        assert_eq!(to(Motion::WordEnd, "hello", 0, 3), p(0, 4));
        assert_eq!(to(Motion::WordEnd, "hi\nworld", 0, 1), p(1, 4));
        // No word end after the last: stays.
        assert_eq!(to(Motion::WordEnd, "hello", 0, 4), p(0, 4));
    }

    #[test]
    fn word_motions_take_counts() {
        let s = "one two three four five";
        assert_eq!(go(Motion::WordFwd, s, 0, 0, 3).cursor.pos, p(0, 14));
        assert_eq!(go(Motion::WordBack, s, 0, 19, 2).cursor.pos, p(0, 8));
        assert_eq!(
            go(Motion::WordEnd, "one two three four", 0, 0, 2)
                .cursor
                .pos,
            p(0, 6)
        );
        assert_eq!(
            go(Motion::WordFwd, s, 0, 0, usize::MAX).cursor.pos,
            p(0, 23)
        );
        assert_eq!(
            go(Motion::WordBack, s, 0, 19, usize::MAX).cursor.pos,
            p(0, 0)
        );
    }

    #[test]
    fn empty_lines_stop_w_and_b_but_not_e() {
        let s = "foo\n\nbar";
        assert_eq!(to(Motion::WordFwd, s, 0, 0), p(1, 0));
        assert_eq!(to(Motion::WordFwd, s, 1, 0), p(2, 0));
        assert_eq!(to(Motion::WordBack, s, 2, 0), p(1, 0));
        assert_eq!(to(Motion::WordBack, s, 1, 0), p(0, 0));
        assert_eq!(to(Motion::WordEnd, s, 0, 2), p(2, 2));
        // A line of blanks is not a stop.
        assert_eq!(to(Motion::WordFwd, "foo\n  \nbar", 0, 0), p(2, 0));
    }

    #[test]
    fn unicode_alphanumerics_and_underscore_are_word_chars() {
        assert_eq!(to(Motion::WordFwd, "café bar", 0, 0), p(0, 6));
        assert_eq!(to(Motion::WordFwd, "snake_case x", 0, 0), p(0, 11));
        assert_eq!(to(Motion::WordFwd, "日本語 x", 0, 0), p(0, 10));
        assert_eq!(to(Motion::WordEnd, "naïve.", 0, 0), p(0, 5));
        assert_eq!(to(Motion::WordBack, "x Ωμέγα", 0, 10), p(0, 2));
        // A punctuation run is its own word.
        assert_eq!(to(Motion::WordFwd, "a->b", 0, 0), p(0, 1));
        assert_eq!(to(Motion::WordFwd, "a->b", 0, 1), p(0, 3));
    }

    #[test]
    fn relative_motions_that_cannot_move_fail() {
        use Motion::*;
        for (m, text, line, col) in [
            (Down, "a\nb", 1, 0),
            (Up, "a\nb", 0, 0),
            (Left, "ab", 0, 0),
            (Right, "ab", 0, 1),
            (WordFwd, "", 0, 0),
            (WordBack, "ab", 0, 0),
            (WordEnd, "ab", 0, 1),
        ] {
            let r = go(m, text, line, col, 1);
            assert!(!r.moved, "{m:?} on {text:?}");
            assert_eq!(r.cursor.pos, p(line, col), "{m:?} on {text:?}");
        }
        assert!(go(Down, "a\nb", 0, 0, 1).moved);
        assert!(go(WordFwd, "hello", 0, 4, 1).moved);
    }

    #[test]
    fn absolute_motions_never_fail() {
        use Motion::*;
        for m in [
            LineStart,
            FirstNonBlank,
            LineEnd,
            DocStart,
            DocEnd,
            GotoLine(1),
        ] {
            assert!(go(m, "a", 0, 0, 1).moved, "{m:?}");
        }
    }

    #[test]
    fn a_failed_vertical_motion_returns_the_clamped_position() {
        // It used to hand back the cursor as given, column and all.
        let buf = Buffer::from_text("ab");
        let cur = Cursor {
            pos: p(0, 99),
            want: Want::Col(7),
        };
        let r = apply(Motion::Down, &buf, cur, 1);
        assert_eq!(r.cursor.pos, p(0, 2));
        assert_eq!(r.cursor.want, Want::Col(7));
        assert!(!r.moved);
    }

    #[test]
    fn word_helpers_for_operators() {
        let buf = Buffer::from_text("foo bar\n\nbaz");
        assert!(at_word_end(&buf, p(0, 2)));
        assert!(!at_word_end(&buf, p(0, 1)));
        assert!(!at_word_end(&buf, p(0, 3)));
        assert!(on_blank(&buf, p(0, 3)));
        assert!(on_blank(&buf, p(1, 0)));
        assert!(!on_blank(&buf, p(0, 0)));
        assert_eq!(word_fwd_for_operator(&buf, p(0, 0)), p(0, 4));
        assert_eq!(word_fwd_for_operator(&buf, p(0, 4)), p(0, 7));
        assert_eq!(word_fwd_for_operator(&buf, p(1, 0)), p(2, 0));
        assert_eq!(word_fwd_for_operator(&buf, p(2, 0)), p(2, 3));
    }

    #[test]
    fn kinds_follow_vim() {
        use Motion::*;
        let buf = Buffer::from_text("ab cd\nef");
        let cur = cursor(&buf, 0, 1);
        for (m, kind) in [
            (Left, MotionKind::Exclusive),
            (Right, MotionKind::Exclusive),
            (WordFwd, MotionKind::Exclusive),
            (WordBack, MotionKind::Exclusive),
            (LineStart, MotionKind::Exclusive),
            (FirstNonBlank, MotionKind::Exclusive),
            (WordEnd, MotionKind::Inclusive),
            (LineEnd, MotionKind::Inclusive),
            (Down, MotionKind::Linewise),
            (Up, MotionKind::Linewise),
            (DocStart, MotionKind::Linewise),
            (DocEnd, MotionKind::Linewise),
            (GotoLine(2), MotionKind::Linewise),
        ] {
            assert_eq!(apply(m, &buf, cur, 1).kind, kind, "{m:?}");
        }
    }
}

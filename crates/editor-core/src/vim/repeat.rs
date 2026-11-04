//! Dot repeat: the last change, recorded as the keys that made it, and
//! replaying it at the cursor.
//!
//! The engine records a `Change` as each command starts (`Vim::key`), the
//! editor tells it what is typed in the Insert session that follows, and
//! the change becomes the one `.` repeats when the command is done. `.`
//! replays it through the same operators, so the repeat lands where the
//! command itself would at the new cursor.

use crate::markdown::Blocks;
use crate::search;
use crate::vim::motion::Motion;
use crate::vim::operator::{self, EditCtx, OpOutcome, OpRange, Operator};
use crate::vim::textobj;
use crate::vim::visual::{self, Shape};
use crate::vim::{Cursor, insert_at, leave_insert};

/// A repeatable change, as `.` replays it. Counts are the effective count
/// (`2d3w` records 6). `inserted` is the text typed in the Insert session
/// a change (`c`) opened: `None` for a delete, `Some` for a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// `x`.
    X(usize),
    /// An operator over a motion (`dw`, `cw`, `d$`).
    Op {
        op: Operator,
        motion: Motion,
        count: usize,
        inserted: Option<String>,
    },
    /// A doubled operator over whole lines (`dd`, `cc`).
    Line {
        op: Operator,
        count: usize,
        inserted: Option<String>,
    },
    /// An Insert session from `i a A I o O`, with the text typed in it.
    Insert { entry: char, text: String },
    /// `p` (`after`) or `P`.
    Put { after: bool, count: usize },
    /// `C` without a count, with the text typed after it.
    CapC(String),
    /// `D` without a count.
    CapD,
    /// An operator over a Visual selection, repeated over a region the
    /// same size from the cursor.
    Visual {
        op: Operator,
        shape: Shape,
        inserted: Option<String>,
    },
    /// An operator over the inner code block (`dic`, `cic`).
    InnerCode {
        op: Operator,
        inserted: Option<String>,
    },
    /// An operator up to a match: of the last search as it is at the
    /// repeat (`dn`, `cN`), or of the query and direction it typed
    /// (`pattern`, from `d/foo` or `d?foo`), which `.` makes the last
    /// search again, as vim does.
    Search {
        op: Operator,
        reverse: bool,
        count: usize,
        pattern: Option<(String, bool)>,
        inserted: Option<String>,
    },
}

impl Change {
    /// The text typed in the change's Insert session, when it has one.
    pub fn text_mut(&mut self) -> Option<&mut String> {
        match self {
            Change::Op { inserted, .. }
            | Change::Line { inserted, .. }
            | Change::Visual { inserted, .. }
            | Change::InnerCode { inserted, .. }
            | Change::Search { inserted, .. } => inserted.as_mut(),
            Change::Insert { text, .. } | Change::CapC(text) => Some(text),
            Change::X(_) | Change::Put { .. } | Change::CapD => None,
        }
    }

    /// Replaces the count. An Insert has none to replace. `C` and `D` with
    /// a count are `c$` and `d$` with it.
    pub fn set_count(&mut self, n: usize) {
        match self {
            Change::X(count)
            | Change::Op { count, .. }
            | Change::Line { count, .. }
            | Change::Put { count, .. }
            | Change::Search { count, .. } => *count = n,
            // The region's size is the selection's, and `ic` takes no
            // count.
            Change::Insert { .. } | Change::Visual { .. } | Change::InnerCode { .. } => {}
            Change::CapC(text) => {
                *self = Change::Op {
                    op: Operator::Change,
                    motion: Motion::LineEnd,
                    count: n,
                    inserted: Some(std::mem::take(text)),
                };
            }
            Change::CapD => {
                *self = Change::Op {
                    op: Operator::Delete,
                    motion: Motion::LineEnd,
                    count: n,
                    inserted: None,
                };
            }
        }
    }
}

/// Carries `change` out at `ctx.cursor`, ending in Normal mode: a change
/// that opened an Insert session types its text and steps left, as Esc
/// would. `search` is the last search's query and direction.
pub fn replay(ctx: &mut EditCtx, change: &Change, search: Option<(&str, bool)>) -> OpOutcome {
    let stay = OpOutcome {
        cursor: ctx.cursor,
        insert: false,
    };
    let cur = Cursor::at(ctx.buf, ctx.buf.clamp(ctx.cursor));
    match change {
        Change::X(count) => OpRange::chars(ctx.buf, cur.pos, *count)
            .map_or(stay, |r| operator::operate(ctx, Operator::Delete, r)),
        Change::Op {
            op,
            motion,
            count,
            inserted,
        } => match OpRange::from_motion(*op, *motion, ctx.buf, cur, *count) {
            Some(r) => {
                let out = operator::operate(ctx, *op, r);
                type_in(ctx, out, inserted.as_deref())
            }
            None => stay,
        },
        Change::Line {
            op,
            count,
            inserted,
        } => {
            let r = OpRange::lines(ctx.buf, cur.pos.line, cur.pos.col, *count);
            let out = operator::operate(ctx, *op, r);
            type_in(ctx, out, inserted.as_deref())
        }
        Change::CapC(text) => {
            match OpRange::from_motion(Operator::Change, Motion::LineEnd, ctx.buf, cur, 1) {
                Some(r) => {
                    let out = operator::operate(ctx, Operator::Change, r);
                    type_in(ctx, out, Some(text))
                }
                None => stay,
            }
        }
        Change::CapD => {
            match OpRange::from_motion(Operator::Delete, Motion::LineEnd, ctx.buf, cur, 1) {
                Some(r) => operator::operate(ctx, Operator::Delete, r),
                None => stay,
            }
        }
        Change::Put { after, count } => operator::put(ctx, !after, *count),
        Change::Visual {
            op,
            shape,
            inserted,
        } => {
            let r = visual::region(ctx.buf, cur.pos, *shape);
            let out = operator::operate(ctx, *op, r);
            type_in(ctx, out, inserted.as_deref())
        }
        Change::InnerCode { op, inserted } => {
            // `.` is rare enough that rescanning the blocks is fine.
            match textobj::code_block(&Blocks::new(ctx.buf), cur.pos.line) {
                Some(block) if textobj::acts(*op, Some(&block)) => {
                    let out = textobj::operate(ctx, *op, block);
                    type_in(ctx, out, inserted.as_deref())
                }
                _ => stay,
            }
        }
        Change::Search {
            op,
            reverse,
            count,
            pattern,
            inserted,
        } => {
            let typed = pattern.as_ref().map(|(q, f)| (q.as_str(), *f));
            let target = match typed {
                Some((query, forward)) => search::find(ctx.buf, query, cur.pos, forward, *count),
                None => search.and_then(|(query, forward)| {
                    search::find(ctx.buf, query, cur.pos, forward != *reverse, *count)
                }),
            };
            match target.and_then(|hit| OpRange::exclusive(ctx.buf, cur.pos, hit.pos())) {
                Some(r) => {
                    let out = operator::operate(ctx, *op, r);
                    type_in(ctx, out, inserted.as_deref())
                }
                None => stay,
            }
        }
        Change::Insert { entry, text } => {
            let out = match entry {
                'o' => operator::open_line(ctx, false),
                'O' => operator::open_line(ctx, true),
                c => {
                    let at = insert_at(*c, ctx.buf, cur.pos);
                    ctx.place(at);
                    OpOutcome {
                        cursor: at,
                        insert: true,
                    }
                }
            };
            type_in(ctx, out, Some(text))
        }
    }
}

/// After a command: types `text` where it left the cursor and leaves
/// Insert, when it entered Insert. `None` leaves the outcome as it is.
fn type_in(ctx: &mut EditCtx, out: OpOutcome, text: Option<&str>) -> OpOutcome {
    let Some(text) = text else {
        return out;
    };
    let at = ctx.buf.clamp(out.cursor);
    ctx.place(at);
    if !text.is_empty() {
        ctx.replace(at..at, text);
    }
    let cursor = leave_insert(ctx.buf, ctx.cursor);
    ctx.place(cursor);
    OpOutcome {
        cursor,
        insert: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::vim::Mode;

    // repeatable_change_test.dart: what each command records.

    #[test]
    fn each_command_records_its_change() {
        for (text, seq, want) in [
            (
                "one two three",
                "2dw",
                Change::Op {
                    op: Operator::Delete,
                    motion: Motion::WordFwd,
                    count: 2,
                    inserted: None,
                },
            ),
            (
                "one two",
                "cwhello<Esc>",
                Change::Op {
                    op: Operator::Change,
                    motion: Motion::WordFwd,
                    count: 1,
                    inserted: Some("hello".into()),
                },
            ),
            (
                "",
                "ihello world<Esc>",
                Change::Insert {
                    entry: 'i',
                    text: "hello world".into(),
                },
            ),
            (
                "a\nb\nc",
                "2dd",
                Change::Line {
                    op: Operator::Delete,
                    count: 2,
                    inserted: None,
                },
            ),
            (
                "a\nb",
                "ccx<Esc>",
                Change::Line {
                    op: Operator::Change,
                    count: 1,
                    inserted: Some("x".into()),
                },
            ),
            ("abcd", "3x", Change::X(3)),
            (
                "ab",
                "yl2p",
                Change::Put {
                    after: true,
                    count: 2,
                },
            ),
            (
                "ab",
                "ylP",
                Change::Put {
                    after: false,
                    count: 1,
                },
            ),
            ("abc", "lD", Change::CapD),
            ("abc", "lCxy<Esc>", Change::CapC("xy".into())),
            (
                "abc\ndef",
                "2D",
                Change::Op {
                    op: Operator::Delete,
                    motion: Motion::LineEnd,
                    count: 2,
                    inserted: None,
                },
            ),
            (
                "a",
                "ox<CR>y<Esc>",
                Change::Insert {
                    entry: 'o',
                    text: "x\ny".into(),
                },
            ),
        ] {
            let mut e = ed(text);
            keys(&mut e, seq);
            assert_eq!(e.last_change(), Some(&want), "{seq}");
        }
    }

    #[test]
    fn yanks_and_motions_are_not_changes() {
        let mut e = ed("abc def");
        keys(&mut e, "x");
        keys(&mut e, "ywyyw$p.");
        // `.` after `p` repeats the put, not a yank.
        assert_eq!(
            e.last_change(),
            Some(&Change::Put {
                after: true,
                count: 1
            })
        );
        let mut e = ed("abc def");
        keys(&mut e, "xywyywj");
        assert_eq!(e.last_change(), Some(&Change::X(1)));
    }

    // dot_command_test.dart.

    #[test]
    fn dot_repeats_x() {
        let mut e = ed("hello");
        keys(&mut e, "x");
        assert_eq!(e.text(), "ello");
        keys(&mut e, ".");
        assert_eq!(e.text(), "llo");
    }

    #[test]
    fn dot_repeats_dw() {
        let mut e = ed("one two three");
        keys(&mut e, "dw");
        assert_eq!(e.text(), "two three");
        keys(&mut e, ".");
        assert_eq!(e.text(), "three");
    }

    #[test]
    fn dot_repeats_dd() {
        let mut e = ed("line1\nline2\nline3");
        keys(&mut e, "dd");
        assert_eq!(e.text(), "line2\nline3");
        keys(&mut e, ".");
        assert_eq!(e.text(), "line3");
    }

    #[test]
    fn dot_repeats_an_insert_with_its_text() {
        // moon_edit typed with `i` then pressed `A<Esc>` before `.`; in vim
        // that empty `A` session is itself the last change, so this enters
        // with `A` and repeats it.
        let mut e = ed("");
        keys(&mut e, "Ahello<Esc>");
        assert_eq!(e.text(), "hello");
        keys(&mut e, ".");
        assert_eq!(e.text(), "hellohello");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 9));
    }

    #[test]
    fn count_replaces_recorded_count() {
        let mut e = ed("abcdefgh");
        keys(&mut e, "2x");
        assert_eq!(e.text(), "cdefgh");
        keys(&mut e, "3.");
        assert_eq!(e.text(), "fgh");
        // The new count sticks for the next `.`.
        let mut e = ed("abcdefgh");
        keys(&mut e, "2x3..");
        assert_eq!(e.text(), "");
    }

    #[test]
    fn dot_with_nothing_recorded_does_nothing() {
        let mut e = ed("hello");
        let fx = keys(&mut e, ".");
        assert_eq!(e.text(), "hello");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(fx.dirty, None);
        assert!(!e.history().can_undo());
    }

    // The brief's cases.

    #[test]
    fn cw_typing_is_one_undo_step() {
        let mut e = ed("hello world");
        keys(&mut e, "cwxyz<Esc>");
        assert_eq!(e.text(), "xyz world");
        keys(&mut e, "u");
        assert_eq!(e.text(), "hello world");
        assert_eq!(e.cursor(), pos(0, 0));
        assert!(!e.history().can_undo());
    }

    #[test]
    fn o_insert_is_one_undo_step() {
        let mut e = ed("abc\ndef");
        keys(&mut e, "ofoo<Esc>");
        assert_eq!(e.text(), "abc\nfoo\ndef");
        keys(&mut e, "u");
        assert_eq!(e.text(), "abc\ndef");
        assert_eq!(e.cursor(), pos(0, 0));
        assert!(!e.history().can_undo());
    }

    #[test]
    fn dot_repeats_o_with_new_line() {
        let mut e = ed("a\nb");
        keys(&mut e, "ofoo<Esc>j.");
        assert_eq!(e.text(), "a\nfoo\nb\nfoo");
        assert_eq!(e.cursor(), pos(3, 2));
        keys(&mut e, "gg");
        keys(&mut e, "OX<Esc>j.");
        assert_eq!(e.text(), "X\nX\na\nfoo\nb\nfoo");
        assert_eq!(e.cursor(), pos(1, 0));
    }

    #[test]
    fn backspace_edits_recorded_insert() {
        let mut e = ed("");
        keys(&mut e, "ifoo<BS>x<Esc>");
        assert_eq!(e.text(), "fox");
        assert_eq!(
            e.last_change(),
            Some(&Change::Insert {
                entry: 'i',
                text: "fox".into(),
            })
        );
        keys(&mut e, ".");
        assert_eq!(e.text(), "fofoxx");
        // Backspace over a line break takes the break out of the text too,
        // and over a whole cluster takes the cluster.
        let mut e = ed("");
        keys(&mut e, "ia<CR><BS>👩\u{200d}👩\u{200d}👧<BS>b<Esc>");
        assert_eq!(
            e.last_change(),
            Some(&Change::Insert {
                entry: 'i',
                text: "ab".into(),
            })
        );
    }

    #[test]
    fn arrow_in_insert_splits_undo() {
        let mut e = ed("");
        keys(&mut e, "iaa<Left>bb<Esc>");
        assert_eq!(e.text(), "abba");
        assert_eq!(e.history().undo_len(), 2);
        keys(&mut e, "u");
        assert_eq!(e.text(), "aa");
        keys(&mut e, "u");
        assert_eq!(e.text(), "");
        // `.` repeats only what was typed after the arrow.
        let mut e = ed("");
        keys(&mut e, "iaa<Left>bb<Esc>0.");
        assert_eq!(e.text(), "bbabba");
    }

    // Owned carry-overs.

    #[test]
    fn big_o_redo_lands_on_the_opened_line_at_the_saved_column() {
        let mut e = ed("abc");
        keys(&mut e, "lOfoo<Esc>");
        assert_eq!(e.text(), "foo\nabc");
        assert_eq!(e.cursor(), pos(0, 2));
        keys(&mut e, "u");
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), pos(0, 1));
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "foo\nabc");
        assert_eq!(e.cursor(), pos(0, 1));
        // With nothing typed, redo lands on the opened line, not after it.
        let mut e = ed("abc");
        keys(&mut e, "O<Esc>u<C-r>");
        assert_eq!(e.text(), "\nabc");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn undo_and_redo_keep_the_cursor_on_grapheme_boundaries() {
        let fam = "👩\u{200d}👩\u{200d}👧";
        let line = format!("a{fam}b");
        let on_boundary = |e: &crate::editor::Editor| {
            let p = e.cursor();
            let text = e.text();
            let l = text.lines().nth(p.line).unwrap_or("");
            assert_eq!(crate::grapheme::floor(l, p.col), p.col, "{p:?} in {l:?}");
        };
        // Delete the cluster, then undo and redo it.
        let mut e = ed(&line);
        keys(&mut e, "lx");
        assert_eq!(e.text(), "ab");
        keys(&mut e, "u");
        assert_eq!(e.text(), line);
        assert_eq!(e.cursor(), pos(0, 1));
        on_boundary(&e);
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "ab");
        assert_eq!(e.cursor(), pos(0, 1));
        on_boundary(&e);
        // Type after the cluster, then undo: back before the session.
        let mut e = ed(&line);
        keys(&mut e, "$izz<Esc>");
        assert_eq!(e.text(), format!("a{fam}zzb"));
        assert_eq!(e.cursor(), pos(0, 20));
        keys(&mut e, "u");
        assert_eq!(e.cursor(), pos(0, 19));
        on_boundary(&e);
        // Delete the last char so the cluster ends the line: Normal mode
        // rests on its start after redo.
        let mut e = ed(&line);
        keys(&mut e, "$xu<C-r>");
        assert_eq!(e.text(), format!("a{fam}"));
        assert_eq!(e.cursor(), pos(0, 1));
        on_boundary(&e);
    }

    // undo_redo_integration_test.dart.

    #[test]
    fn a_full_session_undoes_and_redoes() {
        let mut e = ed("hello world");
        keys(&mut e, "dw");
        assert_eq!(e.text(), "world");
        keys(&mut e, "igoodbye <Esc>");
        assert_eq!(e.text(), "goodbye world");
        keys(&mut e, "dd");
        assert_eq!(e.text(), "");
        keys(&mut e, "u");
        assert_eq!(e.text(), "goodbye world");
        keys(&mut e, "u");
        assert_eq!(e.text(), "world");
        keys(&mut e, "u");
        assert_eq!(e.text(), "hello world");
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "world");
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "goodbye world");
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "");
    }

    #[test]
    fn dot_is_one_undo_step() {
        let mut e = ed("aaa bbb ccc");
        keys(&mut e, "dw");
        assert_eq!(e.text(), "bbb ccc");
        keys(&mut e, ".");
        assert_eq!(e.text(), "ccc");
        keys(&mut e, "u");
        assert_eq!(e.text(), "bbb ccc");
        keys(&mut e, "u");
        assert_eq!(e.text(), "aaa bbb ccc");
    }

    #[test]
    fn undo_across_lines() {
        let mut e = ed("line1\nline2\nline3");
        keys(&mut e, "dddd");
        assert_eq!(e.text(), "line3");
        keys(&mut e, "u");
        assert_eq!(e.text(), "line2\nline3");
        keys(&mut e, "u");
        assert_eq!(e.text(), "line1\nline2\nline3");
    }

    #[test]
    fn cw_and_its_typing_undo_together() {
        // moon_edit undid `cw` in two steps; owner decision 3 makes the
        // change and its typing one.
        let mut e = ed("hello world");
        keys(&mut e, "cwgoodbye<Esc>");
        assert_eq!(e.text(), "goodbye world");
        keys(&mut e, "u");
        assert_eq!(e.text(), "hello world");
    }

    #[test]
    fn counted_x_undoes_in_one_step() {
        let mut e = ed("abcdef");
        keys(&mut e, "3x");
        assert_eq!(e.text(), "def");
        keys(&mut e, "u");
        assert_eq!(e.text(), "abcdef");
    }

    #[test]
    fn put_undoes() {
        let mut e = ed("hello world");
        keys(&mut e, "yw$p");
        assert_eq!(e.text(), "hello worldhello ");
        keys(&mut e, "u");
        assert_eq!(e.text(), "hello world");
    }

    // Undo and redo.

    #[test]
    fn undo_and_redo_land_on_the_top_changed_line_as_vim_does() {
        // Decision D12 (vim's `u_undoredo`): the saved column is the
        // cursor at the group's first change.
        let mut e = ed("one two three");
        keys(&mut e, "wdw");
        assert_eq!(e.cursor(), pos(0, 4));
        keys(&mut e, "0u");
        assert_eq!(e.cursor(), pos(0, 4));
        keys(&mut e, "$<C-r>");
        assert_eq!(e.cursor(), pos(0, 4));
        // `A` moves to the end before it changes anything: column 3, which
        // Normal mode clamps to 2 on `abc`, and 3 on `abcxyz` after redo.
        let mut e = ed("abc");
        keys(&mut e, "Axyz<Esc>");
        keys(&mut e, "0u");
        assert_eq!(e.cursor(), pos(0, 2));
        keys(&mut e, "<C-r>");
        assert_eq!(e.cursor(), pos(0, 3));
    }

    #[test]
    fn undo_of_dk_and_dgg_land_where_the_motion_put_the_cursor() {
        let mut e = ed("x\n  yy\nzzzz");
        keys(&mut e, "jj$dk");
        assert_eq!(e.text(), "x");
        keys(&mut e, "u");
        assert_eq!(e.text(), "x\n  yy\nzzzz");
        assert_eq!(e.cursor(), pos(1, 3));
        keys(&mut e, "<C-r>");
        assert_eq!(e.cursor(), pos(0, 0));
        let mut e = ed("  a\nbb\ncc");
        keys(&mut e, "jj$dgg");
        assert_eq!(e.text(), "");
        keys(&mut e, "u");
        assert_eq!(e.cursor(), pos(0, 2));
    }

    #[test]
    fn undo_of_dj_keeps_the_saved_column() {
        let mut e = ed("abcdef\nghijkl\nmno");
        keys(&mut e, "lllllldj");
        assert_eq!(e.text(), "mno");
        keys(&mut e, "u");
        assert_eq!(e.cursor(), pos(0, 5));
        keys(&mut e, "<C-r>");
        // Vim reports [1,6], past the end of "mno"; ours clamps.
        assert_eq!(e.cursor(), pos(0, 2));
    }

    #[test]
    fn undo_and_redo_of_o_land_on_the_line_it_was_opened_from() {
        let mut e = ed("abc\ndef");
        keys(&mut e, "lofoo<Esc>u");
        assert_eq!(e.text(), "abc\ndef");
        assert_eq!(e.cursor(), pos(0, 1));
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "abc\nfoo\ndef");
        assert_eq!(e.cursor(), pos(0, 1));
    }

    #[test]
    fn undo_of_dd_on_the_last_line_lands_on_it() {
        let mut e = ed("ab\n  cd");
        keys(&mut e, "jdd");
        assert_eq!(e.text(), "ab");
        keys(&mut e, "u");
        assert_eq!(e.text(), "ab\n  cd");
        assert_eq!(e.cursor(), pos(1, 0));
        keys(&mut e, "<C-r>");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn undo_clamps_the_saved_column_to_a_grapheme_boundary() {
        // `A` saves the column past the line end; Normal mode rests on the
        // start of the last cluster, never inside it.
        let fam = "👩\u{200d}👩\u{200d}👧";
        let mut e = ed(&format!("a{fam}"));
        keys(&mut e, "Axyz<Esc>0u");
        assert_eq!(e.text(), format!("a{fam}"));
        assert_eq!(e.cursor(), pos(0, 1));
        keys(&mut e, "<C-r>");
        assert_eq!(e.cursor(), pos(0, 19));
        // `d0` moves the cursor to the range start, which vim restores.
        let mut e = ed(&format!("abcde{fam}z"));
        keys(&mut e, "llllld0u");
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), format!("{fam}z"));
        assert_eq!(e.cursor(), pos(0, 0));
    }

    // Golden cases from real vim (VIM 9.1, headless), owner ruling R29.
    // Positions here are 0-based; each test notes vim's 1-based ones.

    const GOLDEN: &str = "  alpha one\n    bravo two\n      charlie three\n delta four";

    fn golden(start: &str, typed: &str, want: crate::buffer::Pos) {
        let mut e = ed(GOLDEN);
        keys(&mut e, start);
        keys(&mut e, typed);
        assert_eq!(e.cursor(), want, "{typed:?} from {start:?}");
    }

    #[test]
    fn vim_golden_a_xyz_then_undo() {
        // Vim: start (2,9), result [2,13].
        golden("j8l", "Axyz<Esc>0u", pos(1, 12));
    }

    #[test]
    fn vim_golden_dd_undo() {
        // Vim: start (2,9), result [2,5].
        golden("j8l", "ddu", pos(1, 4));
    }

    #[test]
    fn vim_golden_dd_undo_redo() {
        // Vim: start (2,9), result [2,5] (each step its own undo block).
        golden("j8l", "ddu<C-r>", pos(1, 4));
    }

    #[test]
    fn vim_golden_dk_undo() {
        // Vim: start (3,10), result [2,10].
        golden("jj9l", "dku", pos(1, 9));
    }

    #[test]
    fn vim_golden_dk_undo_redo() {
        // Vim: start (3,10), result [2,10] (each step its own undo block).
        golden("jj9l", "dku<C-r>", pos(1, 9));
    }

    #[test]
    fn vim_golden_dj_undo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "dju", pos(1, 8));
    }

    #[test]
    fn vim_golden_dgg_undo() {
        // Vim: start (4,8), result [1,3].
        golden("jjj7l", "dggu", pos(0, 2));
    }

    #[test]
    fn vim_golden_big_g_undo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "dGu", pos(1, 8));
    }

    #[test]
    fn vim_golden_x_undo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "xu", pos(1, 8));
    }

    #[test]
    fn vim_golden_dw_undo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "dwu", pos(1, 8));
    }

    #[test]
    fn vim_golden_dw_undo_redo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "dwu<C-r>", pos(1, 8));
    }

    #[test]
    fn vim_golden_cw_undo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "cwZZ<Esc>u", pos(1, 8));
    }

    #[test]
    fn vim_golden_o_undo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "oNEW<Esc>u", pos(1, 8));
    }

    #[test]
    fn vim_golden_big_o_undo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "ONEW<Esc>u", pos(1, 8));
    }

    #[test]
    fn vim_golden_o_undo_redo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "oNEW<Esc>u<C-r>", pos(1, 8));
    }

    #[test]
    fn vim_golden_yyjp_undo() {
        // Vim: start (2,9), result [3,9].
        golden("j8l", "yyjpu", pos(2, 8));
    }

    #[test]
    fn vim_golden_yykp_undo() {
        // Vim: start (2,9), result [1,9].
        golden("j8l", "yykPu", pos(0, 8));
    }

    // `J` is not implemented in vim/mod.rs yet, so this passes without
    // testing anything: ignored until a task adds `J`, then enable it.
    #[test]
    #[ignore = "J is not implemented yet"]
    fn vim_golden_j_join_undo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "Ju", pos(1, 8));
    }

    #[test]
    fn vim_golden_cc_undo() {
        // Vim: start (2,9), result [2,5].
        golden("j8l", "ccZZ<Esc>u", pos(1, 4));
    }

    #[test]
    fn vim_golden_big_v_d_undo() {
        // Vim: start (2,9), result [2,1].
        golden("j8l", "Vdu", pos(1, 0));
    }

    #[test]
    fn vim_golden_big_v_j_d_undo() {
        // Vim: start (2,9), result [2,1].
        golden("j8l", "Vjdu", pos(1, 0));
    }

    #[test]
    fn vim_golden_big_v_k_d_undo() {
        // Vim: start (3,10), result [2,10].
        golden("jj9l", "Vkdu", pos(1, 9));
    }

    #[test]
    fn vim_golden_big_v_j_c_undo() {
        // Vim: start (2,9), result [3,1].
        golden("j8l", "VjcZZ<Esc>u", pos(2, 0));
    }

    #[test]
    fn vim_golden_v_j_d_undo() {
        // Vim: start (2,9), result [2,9].
        golden("j8l", "vjdu", pos(1, 8));
    }

    #[test]
    fn vim_golden_v_k_d_undo() {
        // Vim: start (3,10), result [2,10].
        golden("jj9l", "vkdu", pos(1, 9));
    }

    #[test]
    fn vim_golden_cj_undo() {
        // Vim: start (2,9), result [3,9].
        golden("j8l", "cjZZ<Esc>u", pos(2, 8));
    }

    #[test]
    fn vim_golden_two_cc_undo() {
        // Vim: start (2,9), result [3,9].
        golden("j8l", "2ccZZ<Esc>u", pos(2, 8));
    }

    #[test]
    fn vim_golden_ck_undo() {
        // Vim: start (3,10), result [3,10].
        golden("jj9l", "ckZZ<Esc>u", pos(2, 9));
    }

    #[test]
    fn vim_golden_cc_undo_redo() {
        // Vim: start (2,9), result [2,2] (each step its own undo block).
        golden("j8l", "ccZZ<Esc>u<C-r>", pos(1, 1));
    }

    #[test]
    fn vim_golden_big_v_d_undo_redo() {
        // Vim: start (2,9), result [2,1] (each step its own undo block).
        golden("j8l", "Vdu<C-r>", pos(1, 0));
    }

    #[test]
    fn vim_golden_dd_undo_redo_on_line_3() {
        // Vim: start (3,10), result [3,7] (each step its own undo block).
        golden("jj9l", "ddu<C-r>", pos(2, 6));
    }

    #[test]
    fn vim_golden_big_v_j_d_undo_redo() {
        // Vim: start (2,9), result [2,1] (each step its own undo block).
        golden("j8l", "Vjdu<C-r>", pos(1, 0));
    }

    #[test]
    fn vim_golden_big_v_k_c_undo_redo() {
        // Vim: start (3,10), result [3,10] (each step its own undo block).
        golden("jj9l", "VkcZZ<Esc>u<C-r>", pos(2, 9));
    }

    #[test]
    fn vim_golden_cj_undo_redo() {
        // Vim: start (2,9), result [3,9] (each step its own undo block).
        golden("j8l", "cjZZ<Esc>u<C-r>", pos(2, 8));
    }

    #[test]
    fn vim_golden_two_cc_undo_redo() {
        // Vim: start (2,9), result [3,9] (each step its own undo block).
        golden("j8l", "2ccZZ<Esc>u<C-r>", pos(2, 8));
    }

    #[test]
    fn vim_golden_a_xyz_undo_redo() {
        // Vim: start (2,9), result [2,14] (each step its own undo block).
        golden("j8l", "Axyz<Esc>0u<C-r>", pos(1, 13));
    }

    #[test]
    fn undo_lands_on_a_cluster_start_when_the_saved_column_is_inside_one() {
        // R12. `cj` from (3,10) saves column 9 of line 4, which is inside
        // the cluster. Vim works on codepoints and lands on byte column 10;
        // we land on the cluster start.
        let fam = "👩\u{200d}👩\u{200d}👧";
        let mut e = ed(&format!(
            "  alpha one\n    bravo two\n      charlie three\nab{fam}cd"
        ));
        keys(&mut e, "jj9lcjZZ<Esc>u");
        assert_eq!(e.cursor(), pos(3, 2));
        keys(&mut e, "<C-r>");
        let text = e.text();
        let line = text.lines().nth(e.cursor().line).unwrap_or("");
        assert_eq!(crate::grapheme::floor(line, e.cursor().col), e.cursor().col);
    }

    #[test]
    fn undo_and_redo_with_nothing_are_silent() {
        let mut e = ed("abc");
        keys(&mut e, "l");
        let fx = keys(&mut e, "u");
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), pos(0, 1));
        assert_eq!(fx.dirty, None);
        assert_eq!(e.message(), None);
        let fx = keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "abc");
        assert_eq!(fx.dirty, None);
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn counts_undo_and_redo_several_steps() {
        let mut e = ed("abcdef");
        keys(&mut e, "xxx");
        keys(&mut e, "2u");
        assert_eq!(e.text(), "bcdef");
        keys(&mut e, "9u");
        assert_eq!(e.text(), "abcdef");
        keys(&mut e, "2<C-r>");
        assert_eq!(e.text(), "cdef");
    }

    #[test]
    fn a_new_change_clears_redo() {
        let mut e = ed("abc");
        keys(&mut e, "xu");
        assert!(e.history().can_redo());
        keys(&mut e, "l");
        assert!(e.history().can_redo(), "a motion keeps redo");
        keys(&mut e, "x<C-r>");
        assert_eq!(e.text(), "ac");
        assert!(!e.history().can_redo());
    }

    #[test]
    fn an_insert_session_with_no_typing_adds_no_step() {
        let mut e = ed("abc");
        keys(&mut e, "i<Esc>a<Esc>");
        assert!(!e.history().can_undo());
    }

    #[test]
    fn undo_redraws_and_marks_dirty() {
        let mut e = ed("a\n```\ncode\n```");
        keys(&mut e, "jjdd");
        let fx = keys(&mut e, "u");
        assert_eq!(e.text(), "a\n```\ncode\n```");
        assert_eq!(fx.redraw, crate::effects::Redraw::All);
        assert!(fx.scroll_to_cursor);
        assert!(e.dirty());
        assert_eq!(e.status().mode, "NORMAL");
    }

    #[test]
    fn reader_mode_does_not_undo_or_repeat() {
        let mut e = crate::testkit::build_reader("abc");
        keys(&mut e, "x.u<C-r>");
        assert_eq!(e.text(), "abc");
        assert_eq!(e.last_change(), None);
    }

    // Dot repeat over the other changes.

    #[test]
    fn dot_repeats_changes_with_their_text() {
        let mut e = ed("aaa bbb ccc");
        keys(&mut e, "cwxx<Esc>w.");
        assert_eq!(e.text(), "xx xx ccc");
        assert_eq!(e.cursor(), pos(0, 4));
        let mut e = ed("one\ntwo\nthree");
        keys(&mut e, "ccX<Esc>j.");
        assert_eq!(e.text(), "X\nX\nthree");
        let mut e = ed("abcd\nefgh");
        keys(&mut e, "llCZ<Esc>j.");
        assert_eq!(e.text(), "abZ\nefZ");
        let mut e = ed("abcd\nefgh");
        keys(&mut e, "llDj.");
        assert_eq!(e.text(), "ab\ne");
        let mut e = ed("ab");
        keys(&mut e, "ylp.");
        assert_eq!(e.text(), "aaab");
        let mut e = ed("a\nb");
        keys(&mut e, "Ix<Esc>j.");
        assert_eq!(e.text(), "xa\nxb");
    }

    #[test]
    fn dot_after_an_undo_still_repeats() {
        let mut e = ed("abc");
        keys(&mut e, "xu.");
        assert_eq!(e.text(), "bc");
        // The repeat is a new step, and it clears redo.
        assert_eq!(e.history().undo_len(), 1);
        assert!(!e.history().can_redo());
    }
}

//! The edit history: grouped undo and redo.
//!
//! Every change to the buffer, from an operator or from typing, goes
//! through `EditCtx::replace`, which records it here as an inverse edit
//! (what undo applies) and a forward edit (what redo applies). Edits are
//! gathered into groups, one per undo step: `begin` opens a group and
//! `end` closes it, and the pairs nest, so an insert session opened by its
//! entry key stays one group while each keystroke inside it opens and
//! closes its own.

use std::collections::VecDeque;

use crate::buffer::{Buffer, Edit, Pos};
use crate::vim;

/// The most undo steps kept. Past it, the oldest group is dropped.
pub const MAX_UNDO_GROUPS: usize = 1_000;

/// One undo step: the edits it made, oldest first, as (inverse, forward)
/// pairs, and where the cursor was when the first of them was made.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    edits: Vec<(Edit, Edit)>,
    first_cursor: Pos,
    /// True when the group's first change was a linewise delete or change.
    linewise: bool,
}

#[derive(Debug, Default)]
pub struct History {
    undo: VecDeque<Group>,
    redo: Vec<Group>,
    open: Option<Group>,
    /// How many `begin`s are waiting for their `end`.
    depth: usize,
}

/// The forward edit for a replacement `Buffer::replace` carried out:
/// `inverse` is what it returned and `inserted` the text it inserted. Its
/// range is what `inverse.text` covered before the edit.
pub fn forward(inverse: &Edit, inserted: &str) -> Edit {
    let start = inverse.range.start;
    let removed = &inverse.text;
    let end = match removed.rfind('\n') {
        None => Pos {
            col: start.col + removed.len(),
            ..start
        },
        Some(i) => Pos {
            line: start.line + removed.matches('\n').count(),
            col: removed.len() - i - 1,
        },
    };
    Edit {
        range: start..end,
        text: inserted.replace("\r\n", "\n"),
    }
}

/// Applies `edits` to `buf` and returns the cursor vim leaves after
/// undoing or redoing `g` (`u_undoredo`, checked against vim 9.1 in the
/// golden tests in `vim/repeat.rs`).
///
/// The saved cursor is the one vim has at the group's first change, after
/// the operator's motion has moved it: `dd` and `cc` are on the first
/// non-blank, `dk` and `dgg` on the motion's landing, `dj` and `dG` where
/// they started, `A` at the line end. The cursor goes to the top line the
/// edits changed. On it, if it is the saved cursor's line, the saved
/// column (clamped to a grapheme Normal mode rests on), else the first
/// non-blank. A change that only adds or takes whole lines below a line
/// (an `o`, a linewise `dd` on the last line) counts from the line below.
///
/// Vim's rule for using the saved cursor is that it lies in the changed
/// block or the line either side of it (the "gwap" comment in
/// `u_undoredo`), which also covers `o` and the multi-line linewise
/// change (`Vjc`, `cj`, `ck`, `2cc`) first deletes the lines after the
/// first with the cursor moved down onto them, so the saved cursor is on
/// the second line and undo lands there, unlike `cc`. Its column is the
/// range start's: the cursor's own for `cj`, `2cc` and `ck` (the landing
/// of `k`), column 0 for Visual Line. That is the one thing telling
/// `2cc` (column kept) from `Vjc` (column 0): vim's range start is the
/// lesser of the cursor and where the command left it, which is the
/// cursor for a count of lines but the Visual start (column 0) in
/// Visual Line. Redo of a
/// linewise delete or change always goes to the first non-blank of the top
/// line.
fn apply<'a>(
    buf: &mut Buffer,
    edits: impl Iterator<Item = &'a Edit>,
    g: &Group,
    redo: bool,
) -> Pos {
    let mut top = usize::MAX;
    // How many lines the edit that set `top` leaves in its place.
    let mut size = 0;
    for e in edits {
        let start = e.range.start;
        let at_end = start.line < buf.line_count() && buf.line(start.line).len() == start.col;
        let breaks = e.text.starts_with('\n') || buf.text_range(e.range.clone()).starts_with('\n');
        let bumped = at_end && breaks;
        let first = start.line + usize::from(bumped);
        if first < top {
            top = first;
            let breaks_in = e.text.matches('\n').count();
            size = match (bumped, e.text.starts_with('\n')) {
                (false, _) => breaks_in + 1,
                (true, true) => breaks_in,
                (true, false) => 0,
            };
        }
        buf.replace(e.range.clone(), &e.text);
    }
    let saved = g.first_cursor;
    let line = top.min(buf.line_count() - 1);
    // Vim: a saved cursor within the changed block, or the line either
    // side of it, is where the cursor goes back to.
    let near = saved.line + 1 >= top && saved.line <= top + size;
    let at = if redo && g.linewise {
        Pos {
            line,
            col: vim::motion::caret_col(buf.line(line)),
        }
    } else if near {
        saved
    } else {
        Pos {
            line,
            col: vim::motion::caret_col(buf.line(line)),
        }
    };
    vim::rest(buf, at)
}

impl History {
    pub fn new() -> History {
        History::default()
    }

    /// Opens a group, or nests in the open one. The group's cursor is the
    /// one given to its first `record`.
    pub fn begin(&mut self) {
        if self.depth == 0 {
            self.open = Some(Group {
                edits: Vec::new(),
                first_cursor: Pos::default(),
                linewise: false,
            });
        }
        self.depth += 1;
    }

    /// Closes the group `begin` opened. The outermost `end` files it as
    /// one undo step, unless it is empty. With no group open it does
    /// nothing.
    pub fn end(&mut self) {
        if self.depth == 0 {
            return;
        }
        self.depth -= 1;
        if self.depth > 0 {
            return;
        }
        if let Some(g) = self.open.take()
            && !g.edits.is_empty()
        {
            self.push(g);
        }
    }

    /// Records one replacement made with the cursor at `cursor`: `inverse`
    /// undoes it and `forward` redoes it. Outside an open group it is an
    /// undo step of its own.
    pub fn record(&mut self, cursor: Pos, inverse: Edit, forward: Edit) {
        match &mut self.open {
            Some(g) => {
                if g.edits.is_empty() {
                    g.first_cursor = cursor;
                }
                g.edits.push((inverse, forward));
            }
            None => {
                let g = Group {
                    first_cursor: cursor,
                    linewise: false,
                    edits: vec![(inverse, forward)],
                };
                self.push(g);
            }
        }
    }

    /// Marks the open group as a linewise change, so that redo goes to the
    /// first non-blank of its top line.
    pub fn mark_linewise(&mut self) {
        if let Some(g) = &mut self.open {
            g.linewise = true;
        }
    }

    /// Files a finished group: a new step clears redo, and the oldest step
    /// goes once there are more than `MAX_UNDO_GROUPS`.
    fn push(&mut self, g: Group) {
        self.redo.clear();
        self.undo.push_back(g);
        if self.undo.len() > MAX_UNDO_GROUPS {
            self.undo.pop_front();
        }
    }

    /// Undoes the latest group in `buf` and returns where the cursor goes
    /// (see `apply`), or `None` when there is nothing to undo.
    pub fn undo(&mut self, buf: &mut Buffer) -> Option<Pos> {
        let g = self.undo.pop_back()?;
        let at = apply(
            buf,
            g.edits.iter().rev().map(|(inverse, _)| inverse),
            &g,
            false,
        );
        self.redo.push(g);
        Some(at)
    }

    /// Redoes the latest undone group in `buf` and returns where the
    /// cursor goes, by the same rule as `undo`, or `None` when there is
    /// nothing to redo.
    pub fn redo(&mut self, buf: &mut Buffer) -> Option<Pos> {
        let g = self.redo.pop()?;
        let at = apply(buf, g.edits.iter().map(|(_, forward)| forward), &g, true);
        self.undo.push_back(g);
        Some(at)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// How many undo steps there are.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    /// Replaces `r` in `buf` and records it, as `EditCtx::replace` does.
    fn edit(h: &mut History, buf: &mut Buffer, r: std::ops::Range<Pos>, text: &str) {
        let at = r.start;
        let inverse = buf.replace(r, text);
        let fwd = forward(&inverse, text);
        h.record(at, inverse, fwd);
    }

    // operation_log_test.dart: OperationLog.

    #[test]
    fn starts_with_nothing_to_undo_or_redo() {
        let h = History::new();
        assert!(!h.can_undo());
        assert!(!h.can_redo());
    }

    #[test]
    fn a_recorded_edit_can_be_undone() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "hello");
        assert!(h.can_undo());
        assert!(!h.can_redo());
    }

    #[test]
    fn undo_reverts_the_edit_and_allows_redo() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "hello");
        assert!(h.undo(&mut buf).is_some());
        assert_eq!(buf.to_text(), "");
        assert!(!h.can_undo());
        assert!(h.can_redo());
    }

    #[test]
    fn undo_and_redo_with_nothing_return_none() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("abc");
        assert_eq!(h.undo(&mut buf), None);
        assert_eq!(h.redo(&mut buf), None);
        assert_eq!(buf.to_text(), "abc");
    }

    #[test]
    fn redo_reapplies_the_undone_edit() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "hello");
        h.undo(&mut buf);
        assert!(h.redo(&mut buf).is_some());
        assert_eq!(buf.to_text(), "hello");
        assert!(h.can_undo());
        assert!(!h.can_redo());
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "first");
        h.undo(&mut buf);
        assert!(h.can_redo());
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "second");
        assert!(!h.can_redo());
        assert_eq!(h.redo(&mut buf), None);
        assert_eq!(buf.to_text(), "second");
    }

    #[test]
    fn several_undos_and_redos_walk_the_history() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        for (i, s) in ["one", "two", "three"].into_iter().enumerate() {
            let at = buf.clamp(pos(0, usize::MAX));
            edit(&mut h, &mut buf, at..at, s);
            assert_eq!(h.undo_len(), i + 1);
        }
        assert_eq!(buf.to_text(), "onetwothree");
        h.undo(&mut buf);
        h.undo(&mut buf);
        assert_eq!(buf.to_text(), "one");
        assert!(h.can_undo() && h.can_redo());
        h.redo(&mut buf);
        assert_eq!(buf.to_text(), "onetwo");
        assert!(h.can_undo() && h.can_redo());
        h.undo(&mut buf);
        h.undo(&mut buf);
        assert_eq!(buf.to_text(), "");
        assert!(!h.can_undo() && h.can_redo());
        assert_eq!(h.undo(&mut buf), None);
    }

    // operation_log_test.dart: OperationLog with cursor positions.

    #[test]
    fn undo_and_redo_land_where_vim_does() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        h.begin();
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "hello");
        h.end();
        assert!(h.can_undo());
        assert_eq!(h.undo(&mut buf), Some(pos(0, 0)));
        assert_eq!(h.redo(&mut buf), Some(pos(0, 0)));
    }

    #[test]
    fn an_empty_group_is_discarded() {
        let mut h = History::new();
        h.begin();
        h.end();
        assert!(!h.can_undo());
    }

    #[test]
    fn an_empty_group_keeps_redo() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "a");
        h.undo(&mut buf);
        h.begin();
        h.end();
        assert!(h.can_redo());
    }

    #[test]
    fn an_edit_outside_a_group_gets_a_group_of_its_own() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("ab");
        edit(&mut h, &mut buf, pos(0, 1)..pos(0, 1), "x");
        edit(&mut h, &mut buf, pos(0, 2)..pos(0, 2), "y");
        assert_eq!(h.undo_len(), 2);
        assert_eq!(h.undo(&mut buf), Some(pos(0, 2)));
        assert_eq!(buf.to_text(), "axb");
        assert_eq!(h.redo(&mut buf), Some(pos(0, 2)));
    }

    // Grouping.

    #[test]
    fn a_group_undoes_all_its_edits_in_one_step() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("hello world");
        h.begin();
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 5), "");
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "x\ny");
        edit(&mut h, &mut buf, pos(1, 1)..pos(1, 1), "z");
        h.end();
        assert_eq!(buf.to_text(), "x\nyz world");
        assert_eq!(h.undo_len(), 1);
        assert_eq!(h.undo(&mut buf), Some(pos(0, 0)));
        assert_eq!(buf.to_text(), "hello world");
        assert_eq!(h.redo(&mut buf), Some(pos(0, 0)));
        assert_eq!(buf.to_text(), "x\nyz world");
    }

    #[test]
    fn nested_groups_make_one_step() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        h.begin();
        for (i, c) in ["a", "b", "c"].into_iter().enumerate() {
            h.begin();
            edit(&mut h, &mut buf, pos(0, i)..pos(0, i), c);
            h.end();
            assert!(!h.can_undo(), "still open");
        }
        h.end();
        assert_eq!(h.undo_len(), 1);
        assert_eq!(h.undo(&mut buf), Some(pos(0, 0)));
        assert_eq!(buf.to_text(), "");
    }

    #[test]
    fn end_without_begin_does_nothing() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        h.end();
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "a");
        assert_eq!(h.undo_len(), 1);
    }

    #[test]
    fn multiline_edits_round_trip() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("line1\nline2\nline3");
        h.begin();
        edit(&mut h, &mut buf, pos(0, 0)..pos(1, 0), "");
        h.end();
        h.begin();
        edit(&mut h, &mut buf, pos(0, 3)..pos(1, 2), "A\nB\nC");
        h.end();
        assert_eq!(buf.to_text(), "linA\nB\nCne3");
        h.undo(&mut buf);
        assert_eq!(buf.to_text(), "line2\nline3");
        h.undo(&mut buf);
        assert_eq!(buf.to_text(), "line1\nline2\nline3");
        h.redo(&mut buf);
        h.redo(&mut buf);
        assert_eq!(buf.to_text(), "linA\nB\nCne3");
    }

    #[test]
    fn the_stack_holds_at_most_max_groups_and_drops_the_oldest() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        let extra = 5;
        for i in 0..MAX_UNDO_GROUPS + extra {
            h.begin();
            edit(&mut h, &mut buf, pos(0, i)..pos(0, i), "x");
            h.end();
            assert!(h.undo_len() <= MAX_UNDO_GROUPS);
        }
        assert_eq!(h.undo_len(), MAX_UNDO_GROUPS);
        for _ in 0..MAX_UNDO_GROUPS - 1 {
            assert!(h.undo(&mut buf).is_some());
        }
        // The last group kept: the one after the dropped ones.
        assert_eq!(h.undo(&mut buf), Some(pos(0, extra - 1)));
        assert_eq!(buf.to_text(), "x".repeat(extra));
        assert_eq!(h.undo(&mut buf), None);
        assert_eq!(buf.to_text(), "x".repeat(extra));
        // And it all comes back.
        let mut n = 0;
        while h.redo(&mut buf).is_some() {
            n += 1;
        }
        assert_eq!(n, MAX_UNDO_GROUPS);
        assert_eq!(buf.to_text(), "x".repeat(MAX_UNDO_GROUPS + extra));
    }
}

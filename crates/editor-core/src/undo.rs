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

/// The most undo steps kept. Past it, the oldest group is dropped.
pub const MAX_UNDO_GROUPS: usize = 1_000;

/// One undo step: the edits it made, oldest first, as (inverse, forward)
/// pairs, and where the cursor was before and after.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    edits: Vec<(Edit, Edit)>,
    cursor_before: Pos,
    cursor_after: Pos,
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

impl History {
    pub fn new() -> History {
        History::default()
    }

    /// Opens a group with the cursor at `cursor`, or nests in the open one.
    pub fn begin(&mut self, cursor: Pos) {
        if self.depth == 0 {
            self.open = Some(Group {
                edits: Vec::new(),
                cursor_before: cursor,
                cursor_after: cursor,
            });
        }
        self.depth += 1;
    }

    /// Closes the group `begin` opened, leaving the cursor at `cursor`. The
    /// outermost `end` files it as one undo step, unless it is empty. With
    /// no group open it does nothing.
    pub fn end(&mut self, cursor: Pos) {
        if self.depth == 0 {
            return;
        }
        self.depth -= 1;
        if self.depth > 0 {
            return;
        }
        if let Some(mut g) = self.open.take()
            && !g.edits.is_empty()
        {
            g.cursor_after = cursor;
            self.push(g);
        }
    }

    /// Records one replacement: `inverse` undoes it and `forward` redoes it.
    /// Outside an open group it is an undo step of its own, with the cursor
    /// at the edit's start before it and after the inserted text after.
    pub fn record(&mut self, inverse: Edit, forward: Edit) {
        match &mut self.open {
            Some(g) => g.edits.push((inverse, forward)),
            None => {
                let g = Group {
                    cursor_before: forward.range.start,
                    cursor_after: inverse.range.end,
                    edits: vec![(inverse, forward)],
                };
                self.push(g);
            }
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

    /// Undoes the latest group in `buf` and returns the cursor from before
    /// it, or `None` when there is nothing to undo.
    pub fn undo(&mut self, buf: &mut Buffer) -> Option<Pos> {
        let g = self.undo.pop_back()?;
        for (inverse, _) in g.edits.iter().rev() {
            buf.replace(inverse.range.clone(), &inverse.text);
        }
        let at = g.cursor_before;
        self.redo.push(g);
        Some(at)
    }

    /// Redoes the latest undone group in `buf` and returns the cursor from
    /// after it, or `None` when there is nothing to redo.
    pub fn redo(&mut self, buf: &mut Buffer) -> Option<Pos> {
        let g = self.redo.pop()?;
        for (_, forward) in &g.edits {
            buf.replace(forward.range.clone(), &forward.text);
        }
        let at = g.cursor_after;
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
        let inverse = buf.replace(r, text);
        let fwd = forward(&inverse, text);
        h.record(inverse, fwd);
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
    fn undo_returns_the_cursor_before_and_redo_the_cursor_after() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        h.begin(pos(0, 0));
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "hello");
        h.end(pos(0, 5));
        assert!(h.can_undo());
        assert_eq!(h.undo(&mut buf), Some(pos(0, 0)));
        assert_eq!(h.redo(&mut buf), Some(pos(0, 5)));
    }

    #[test]
    fn an_empty_group_is_discarded() {
        let mut h = History::new();
        h.begin(pos(0, 0));
        h.end(pos(0, 0));
        assert!(!h.can_undo());
    }

    #[test]
    fn an_empty_group_keeps_redo() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "a");
        h.undo(&mut buf);
        h.begin(pos(0, 0));
        h.end(pos(0, 0));
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
        assert_eq!(h.redo(&mut buf), Some(pos(0, 3)));
    }

    // Grouping.

    #[test]
    fn a_group_undoes_all_its_edits_in_one_step() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("hello world");
        h.begin(pos(0, 0));
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 5), "");
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "x\ny");
        edit(&mut h, &mut buf, pos(1, 1)..pos(1, 1), "z");
        h.end(pos(1, 1));
        assert_eq!(buf.to_text(), "x\nyz world");
        assert_eq!(h.undo_len(), 1);
        assert_eq!(h.undo(&mut buf), Some(pos(0, 0)));
        assert_eq!(buf.to_text(), "hello world");
        assert_eq!(h.redo(&mut buf), Some(pos(1, 1)));
        assert_eq!(buf.to_text(), "x\nyz world");
    }

    #[test]
    fn nested_groups_make_one_step() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        h.begin(pos(0, 0));
        for (i, c) in ["a", "b", "c"].into_iter().enumerate() {
            h.begin(pos(0, i));
            edit(&mut h, &mut buf, pos(0, i)..pos(0, i), c);
            h.end(pos(0, i + 1));
            assert!(!h.can_undo(), "still open");
        }
        h.end(pos(0, 2));
        assert_eq!(h.undo_len(), 1);
        assert_eq!(h.undo(&mut buf), Some(pos(0, 0)));
        assert_eq!(buf.to_text(), "");
    }

    #[test]
    fn end_without_begin_does_nothing() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        h.end(pos(0, 0));
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "a");
        assert_eq!(h.undo_len(), 1);
    }

    #[test]
    fn multiline_edits_round_trip() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("line1\nline2\nline3");
        h.begin(pos(0, 0));
        edit(&mut h, &mut buf, pos(0, 0)..pos(1, 0), "");
        h.end(pos(0, 0));
        h.begin(pos(0, 0));
        edit(&mut h, &mut buf, pos(0, 3)..pos(1, 2), "A\nB\nC");
        h.end(pos(2, 1));
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
            h.begin(pos(0, i));
            edit(&mut h, &mut buf, pos(0, i)..pos(0, i), "x");
            h.end(pos(0, i + 1));
            assert!(h.undo_len() <= MAX_UNDO_GROUPS);
        }
        assert_eq!(h.undo_len(), MAX_UNDO_GROUPS);
        for _ in 0..MAX_UNDO_GROUPS - 1 {
            assert!(h.undo(&mut buf).is_some());
        }
        // The last group kept: the one after the dropped ones.
        assert_eq!(h.undo(&mut buf), Some(pos(0, extra)));
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

//! The edit history: grouped undo and redo.
//!
//! Every change to the buffer, from an operator or from typing, goes
//! through `EditCtx::replace`, which records it here as an inverse edit
//! (what undo applies) and a forward edit (what redo applies). Edits are
//! gathered into groups, one per undo step: `begin` opens a group and
//! `end` closes it, and the pairs nest, so an insert session opened by its
//! entry key stays one group while each keystroke inside it opens and
//! closes its own.
//!
//! The save point: every group has an id, and the history's state is the
//! id of the latest group undo would take back (or of the last one dropped
//! off the front, or 0 at the start). A write records that state
//! (`mark_saved`), and undo or redo that lands on it is back at the text
//! on disk (`at_saved`), as vim resets 'modified'.

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
    /// True when undo and redo go to `first_cursor` itself (`pin_cursor`).
    pinned: bool,
    /// Unique in this history; never 0.
    id: u64,
}

#[derive(Debug, Default)]
pub struct History {
    undo: VecDeque<Group>,
    redo: Vec<Group>,
    open: Option<Group>,
    /// How many `begin`s are waiting for their `end`.
    depth: usize,
    /// The last group id handed out.
    last_id: u64,
    /// The state with nothing left to undo: 0, or the id of the last group
    /// dropped off the front.
    base: u64,
    /// The state the text on disk matches. 0 to begin with: a history
    /// starts at the text that was read.
    saved: u64,
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

/// Where an undo or redo leaves things (`History::undo_step`,
/// `History::redo_step`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landing {
    /// The cursor by vim's rule (`apply`), before it is put on a grapheme
    /// Normal mode rests on: `History::undo` and `History::redo` rest it;
    /// with vim off it is clamped instead, so a cursor at a line end stays
    /// there.
    pub at: Pos,
    /// Where the last edit applied ends: after the text a redo put back
    /// (where the cursor goes on a redo with vim off).
    pub end: Pos,
}

/// Applies `edits` to `buf` and returns where the cursor goes after
/// undoing or redoing `g`: the cursor vim leaves (`u_undoredo`, checked
/// against vim 9.1 in the golden tests in `vim/repeat.rs`), not yet rested
/// on a grapheme, and the end of the last edit. Undo and redo use the
/// same rule.
///
/// The saved cursor is the one vim has at the group's first change, after
/// the operator's motion has moved it. Vim's operator range starts at the
/// lesser of the cursor before the command and where the command leaves it:
/// - `dd` and `cc` leave it on the first non-blank of the last line of the
///   count, so one line starts at the first non-blank (or the cursor's
///   column if that is less) and a count of lines starts at the cursor.
/// - `dk` and `dgg` start at the motion's landing; `dj` and `dG` where they
///   began; `A` at the line end.
/// - Visual Line starts at column 0 of its first line, or at the head where
///   it moved up; charwise operators at the start of their range.
/// - A linewise change over more than one line (`Vjc`, `cj`, `ck`, `2cc`)
///   first deletes the lines after the first with the cursor moved down
///   onto them, so the saved cursor is the range start's column on the
///   second line, which is where undo lands (unlike `cc`).
///
/// The cursor goes to the saved cursor if it lies in the block of lines the
/// edits changed or on the line either side of it (the "gwap" comment in
/// `u_undoredo`; this also covers `o`), clamped to a grapheme Normal mode
/// rests on. Otherwise it goes to the first non-blank of the top changed
/// line. A change that only adds or takes whole lines below a line (an `o`,
/// a linewise `dd` on the last line) counts from the line below.
///
/// A group whose cursor was pinned (`History::pin_cursor`) goes to that
/// cursor instead, wherever the edits were.
fn apply<'a>(buf: &mut Buffer, edits: impl Iterator<Item = &'a Edit>, g: &Group) -> Landing {
    let mut top = usize::MAX;
    let mut end = Pos::default();
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
        let inverse = buf.replace(e.range.clone(), &e.text);
        end = inverse.range.end;
    }
    let saved = g.first_cursor;
    if g.pinned {
        return Landing { at: saved, end };
    }
    let line = top.min(buf.line_count() - 1);
    // Vim: a saved cursor within the changed block, or the line either
    // side of it, is where the cursor goes back to.
    let near = saved.line + 1 >= top && saved.line <= top + size;
    let at = if near {
        saved
    } else {
        Pos {
            line,
            col: vim::motion::caret_col(buf.line(line)),
        }
    };
    Landing { at, end }
}

impl History {
    pub fn new() -> History {
        History::default()
    }

    /// Opens a group, or nests in the open one. The group's cursor is the
    /// one given to its first `record`.
    pub fn begin(&mut self) {
        if self.depth == 0 {
            self.open = Some(self.group(Pos::default()));
        }
        self.depth += 1;
    }

    /// A new empty group with the next id.
    fn group(&mut self, first_cursor: Pos) -> Group {
        self.last_id += 1;
        Group {
            edits: Vec::new(),
            first_cursor,
            pinned: false,
            id: self.last_id,
        }
    }

    /// Where the history is: the id of the latest group undo would take
    /// back, or `base` when there is none.
    fn state(&self) -> u64 {
        self.undo.back().map_or(self.base, |g| g.id)
    }

    /// The buffer now matches the file on disk. An open group with edits
    /// (a write in the middle of an Insert session) is filed as a step of
    /// its own and a fresh one opened in its place, so undo can come back
    /// to exactly this text.
    pub fn mark_saved(&mut self) {
        if self.open.as_ref().is_some_and(|g| !g.edits.is_empty()) {
            // Its first record sets the cursor, as for any group.
            let fresh = self.group(Pos::default());
            if let Some(g) = self.open.replace(fresh) {
                self.push(g);
            }
        }
        self.saved = self.state();
    }

    /// True when the buffer is at the text last saved (or read), as far
    /// as the history knows.
    pub fn at_saved(&self) -> bool {
        self.open.as_ref().is_none_or(|g| g.edits.is_empty()) && self.state() == self.saved
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
                let mut g = self.group(cursor);
                g.edits.push((inverse, forward));
                self.push(g);
            }
        }
    }

    /// Sets the cursor the open group keeps (see `apply`), in place of the
    /// one its first `record` gave, for a command that edits before it
    /// knows where vim's undo would put the cursor (`:s///c`). With no
    /// group open, or an empty one, it does nothing.
    pub fn set_cursor(&mut self, cursor: Pos) {
        if let Some(g) = self.open.as_mut().filter(|g| !g.edits.is_empty()) {
            g.first_cursor = cursor;
        }
    }

    /// Like `set_cursor`, but undo and redo of the group go to `cursor`
    /// itself (on a grapheme Normal mode rests on) rather than by vim's
    /// rule in `apply`: for a command whose undo puts the cursor back
    /// where it was (the table commands). With no group open, or an empty
    /// one, it does nothing.
    pub fn pin_cursor(&mut self, cursor: Pos) {
        if let Some(g) = self.open.as_mut().filter(|g| !g.edits.is_empty()) {
            g.first_cursor = cursor;
            g.pinned = true;
        }
    }

    /// Files a finished group: a new step clears redo, and the oldest step
    /// goes once there are more than `MAX_UNDO_GROUPS`.
    fn push(&mut self, g: Group) {
        self.redo.clear();
        self.undo.push_back(g);
        if self.undo.len() > MAX_UNDO_GROUPS
            && let Some(dropped) = self.undo.pop_front()
        {
            self.base = dropped.id;
        }
    }

    /// Undoes the latest group in `buf` and returns where the cursor goes
    /// (see `apply`), or `None` when there is nothing to undo.
    pub fn undo(&mut self, buf: &mut Buffer) -> Option<Pos> {
        let l = self.undo_step(buf)?;
        Some(vim::rest(buf, l.at))
    }

    /// `undo`, handing back the `Landing` unrested.
    pub fn undo_step(&mut self, buf: &mut Buffer) -> Option<Landing> {
        let g = self.undo.pop_back()?;
        let l = apply(buf, g.edits.iter().rev().map(|(inverse, _)| inverse), &g);
        self.redo.push(g);
        Some(l)
    }

    /// Redoes the latest undone group in `buf` and returns where the
    /// cursor goes, by the same rule as `undo`, or `None` when there is
    /// nothing to redo.
    pub fn redo(&mut self, buf: &mut Buffer) -> Option<Pos> {
        let l = self.redo_step(buf)?;
        Some(vim::rest(buf, l.at))
    }

    /// `redo`, handing back the `Landing` unrested.
    pub fn redo_step(&mut self, buf: &mut Buffer) -> Option<Landing> {
        let g = self.redo.pop()?;
        let l = apply(buf, g.edits.iter().map(|(_, forward)| forward), &g);
        self.undo.push_back(g);
        Some(l)
    }

    /// The id of the latest undo step, if there is one.
    pub fn last_group(&self) -> Option<u64> {
        self.undo.back().map(|g| g.id)
    }

    /// Opens the latest undo step again, for more edits to join it (a run
    /// of typing with vim off: `crate::nonvim`), as `begin` opens a new
    /// one; `end` files it again, under the same id. Only when it is step
    /// `id`, nothing is open, nothing is waiting to be redone and it is not
    /// the save point (a write ends a run). True when it reopened.
    pub fn reopen(&mut self, id: u64) -> bool {
        let latest = self.last_group() == Some(id);
        if !latest || self.depth != 0 || !self.redo.is_empty() || self.saved == id {
            return false;
        }
        self.open = self.undo.pop_back();
        self.depth = 1;
        true
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
    fn reopen_extends_only_the_latest_unsaved_step() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("");
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 0), "a");
        let id = h.last_group().unwrap();
        assert!(h.reopen(id));
        edit(&mut h, &mut buf, pos(0, 1)..pos(0, 1), "b");
        h.end();
        assert_eq!(h.undo_len(), 1);
        assert_eq!(h.last_group(), Some(id));
        // Not a step that is not the latest, nor the save point, nor with
        // something to redo.
        edit(&mut h, &mut buf, pos(0, 2)..pos(0, 2), "c");
        assert!(!h.reopen(id));
        let c = h.last_group().unwrap();
        h.mark_saved();
        assert!(!h.reopen(c));
        edit(&mut h, &mut buf, pos(0, 3)..pos(0, 3), "d");
        let d = h.last_group().unwrap();
        h.undo(&mut buf);
        h.redo(&mut buf);
        assert!(h.reopen(d));
        h.end();
        h.undo(&mut buf);
        assert!(!h.reopen(c));
        assert_eq!(buf.to_text(), "abc");
        h.undo(&mut buf);
        h.undo(&mut buf);
        assert_eq!(buf.to_text(), "");
    }

    #[test]
    fn set_cursor_replaces_the_open_groups_cursor() {
        let mut h = History::new();
        let mut buf = Buffer::from_text("abc def\nxyz");
        // Nothing open, or nothing recorded yet: no effect.
        h.set_cursor(pos(0, 5));
        h.begin();
        h.set_cursor(pos(0, 5));
        edit(&mut h, &mut buf, pos(0, 0)..pos(0, 1), "A");
        h.set_cursor(pos(0, 4));
        h.end();
        assert_eq!(h.undo(&mut buf), Some(pos(0, 4)));
    }

    #[test]
    fn a_pinned_cursor_is_where_undo_and_redo_go() {
        // Line 3's edit is far from the cursor on line 0, so vim's rule
        // would go to line 3's first non-blank; a pinned cursor stays.
        let mut h = History::new();
        let mut buf = Buffer::from_text("abc\nd\ne\n  fgh");
        // Nothing open, or nothing recorded yet: no effect.
        h.pin_cursor(pos(0, 2));
        h.begin();
        h.pin_cursor(pos(0, 2));
        edit(&mut h, &mut buf, pos(3, 2)..pos(3, 3), "F");
        h.pin_cursor(pos(0, 2));
        h.end();
        assert_eq!(h.undo(&mut buf), Some(pos(0, 2)));
        assert_eq!(buf.to_text(), "abc\nd\ne\n  fgh");
        assert_eq!(h.redo(&mut buf), Some(pos(0, 2)));
        // Unpinned, the same edit goes by vim's rule.
        let mut h = History::new();
        let mut buf = Buffer::from_text("abc\nd\ne\n  fgh");
        h.begin();
        edit(&mut h, &mut buf, pos(3, 2)..pos(3, 3), "F");
        h.set_cursor(pos(0, 2));
        h.end();
        assert_eq!(h.undo(&mut buf), Some(pos(3, 2)));
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

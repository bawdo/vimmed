//! The edit history. Every change to the buffer, from an operator or from
//! typing, goes through `EditCtx::replace`, which records it here.
//!
//! This is the minimal record: no undo, redo, grouping or dot repeat yet.
//! TODO(Task 21): those, built on the changes recorded here.

use std::ops::Range;

use crate::buffer::Pos;

/// One replacement, as `Buffer::replace` carried it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Where the inserted text sits after the edit (the inverse edit's
    /// range). Before the edit, `removed` started at `range.start`.
    pub range: Range<Pos>,
    pub removed: String,
    /// The inserted text with `\r\n` normalised to `\n`.
    pub inserted: String,
    pub cursor_before: Pos,
    pub cursor_after: Pos,
}

#[derive(Debug, Default)]
pub struct History {
    changes: Vec<Change>,
}

impl History {
    pub fn new() -> History {
        History::default()
    }

    pub fn record(&mut self, change: Change) {
        self.changes.push(change);
    }

    /// Every change so far, oldest first.
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// Sets where the cursor ended up after the latest change.
    pub fn set_cursor_after(&mut self, pos: Pos) {
        if let Some(c) = self.changes.last_mut() {
            c.cursor_after = pos;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn records_changes_in_order_and_updates_the_last_cursor() {
        let mut h = History::new();
        h.set_cursor_after(pos(9, 9));
        assert!(h.changes().is_empty());
        let c = Change {
            range: pos(0, 0)..pos(0, 1),
            removed: String::new(),
            inserted: "a".into(),
            cursor_before: pos(0, 0),
            cursor_after: pos(0, 1),
        };
        h.record(c.clone());
        h.record(Change {
            inserted: "b".into(),
            ..c.clone()
        });
        h.set_cursor_after(pos(0, 0));
        assert_eq!(h.changes().len(), 2);
        assert_eq!(h.changes()[0], c);
        assert_eq!(h.changes()[1].cursor_after, pos(0, 0));
    }
}

//! Product budgets: the ceilings that keep one key, or one document, from
//! costing unbounded time or memory.
//!
//! Each constant has a normal value and a lower one under `cfg(fuzzing)` (set
//! by cargo-fuzz and ClusterFuzzLite). Fuzz builds lower the numbers only;
//! the same limit-handling code runs in both.

use std::ops::Range;

use crate::buffer::{Buffer, Pos};

/// The largest value a count part may reach. Matches vim's own ceiling, so a
/// count never means more than a billion operations. Fuzz builds use 1,000 so
/// a few input bytes cannot mean tens of millions of operations.
#[cfg(not(fuzzing))]
pub const MAX_COUNT: usize = 999_999_999;
#[cfg(fuzzing)]
pub const MAX_COUNT: usize = 1_000;

/// The most lines a document may hold after an edit grows it. Bounds the
/// per-line bookkeeping (line index, render rows) a single edit can force.
#[cfg(not(fuzzing))]
pub const MAX_DOC_LINES: usize = 1 << 22; // 4,194,304
#[cfg(fuzzing)]
pub const MAX_DOC_LINES: usize = 1 << 16; // 65,536

/// The most bytes a document may hold after an edit grows it (256 MiB
/// normally). Bounds memory so a repeated put or substitute cannot exhaust
/// it. Loading is never refused, only growth. Counts line contents only,
/// not line breaks.
#[cfg(not(fuzzing))]
pub const MAX_DOC_BYTES: usize = 1 << 28; // 268,435,456
#[cfg(fuzzing)]
pub const MAX_DOC_BYTES: usize = 1 << 20; // 1,048,576

/// The most bytes of text the undo history may retain (64 MiB normally).
/// Bounds memory held by saved edits, alongside the group-count limit.
#[cfg(not(fuzzing))]
pub const MAX_UNDO_BYTES: usize = 1 << 26; // 67,108,864
#[cfg(fuzzing)]
pub const MAX_UNDO_BYTES: usize = 1 << 20; // 1,048,576

/// The most changes one `:s` command may make. Bounds the work and the undo
/// record of a single substitution over a large range.
#[cfg(not(fuzzing))]
pub const MAX_SUB_CHANGES: usize = 1 << 18; // 262,144
#[cfg(fuzzing)]
pub const MAX_SUB_CHANGES: usize = 1 << 12; // 4,096

/// Whether `buf` stays within the document budgets after growing by
/// `added_lines` lines and `added_bytes` bytes (line breaks not counted).
/// A sum that overflows does not fit.
pub fn fits(buf: &Buffer, added_lines: usize, added_bytes: usize) -> bool {
    let lines = buf.line_count().checked_add(added_lines);
    let bytes = buf.byte_count().checked_add(added_bytes);
    lines.is_some_and(|n| n <= MAX_DOC_LINES) && bytes.is_some_and(|n| n <= MAX_DOC_BYTES)
}

/// Whether `buf` stays within the document budgets after `r` is replaced
/// with `text`: only what the edit adds beyond what it removes counts, so
/// a replacement no bigger than its range always fits.
pub fn fits_replacing(buf: &Buffer, r: Range<Pos>, text: &str) -> bool {
    match growth(buf, r, text) {
        (0, 0) => true,
        (lines, bytes) => fits(buf, lines, bytes),
    }
}

/// The lines and content bytes replacing `r` with `text` adds beyond what
/// it removes, each at least zero.
fn growth(buf: &Buffer, r: Range<Pos>, text: &str) -> (usize, usize) {
    let (a, b) = (buf.clamp(r.start), buf.clamp(r.end));
    let removed_lines = b.line - a.line;
    let removed_bytes = if a.line == b.line {
        b.col - a.col
    } else {
        let middle: usize = (a.line + 1..b.line).map(|l| buf.line(l).len()).sum();
        buf.line(a.line).len() - a.col + middle + b.col
    };
    let breaks = text.matches('\n').count();
    (
        breaks.saturating_sub(removed_lines),
        (text.len() - breaks).saturating_sub(removed_bytes),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_holds_both_budgets_and_overflow_never_fits() {
        let buf = Buffer::from_text("ab\ncd\n");
        let lines = MAX_DOC_LINES - buf.line_count();
        let bytes = MAX_DOC_BYTES - buf.byte_count();
        assert!(fits(&buf, 0, 0));
        assert!(fits(&buf, lines, bytes));
        assert!(!fits(&buf, lines + 1, 0));
        assert!(!fits(&buf, 0, bytes + 1));
        assert!(!fits(&buf, usize::MAX, 0));
        assert!(!fits(&buf, 0, usize::MAX));
    }

    #[test]
    fn growth_counts_what_a_replacement_adds_beyond_its_range() {
        let buf = Buffer::from_text("abcd\nef\nghij");
        let p = |line, col| Pos { line, col };
        // An insert adds all of its text, line breaks as lines.
        assert_eq!(growth(&buf, p(0, 1)..p(0, 1), "xy\nz"), (1, 3));
        // Within a line: "bc" goes, "xyz" comes.
        assert_eq!(growth(&buf, p(0, 1)..p(0, 3), "xyz"), (0, 1));
        assert_eq!(growth(&buf, p(0, 1)..p(0, 3), "x"), (0, 0));
        // Across lines: "bcd", "ef" and "gh" go with two breaks.
        assert_eq!(growth(&buf, p(0, 1)..p(2, 2), "1234567\n\n\n"), (1, 0));
        assert_eq!(growth(&buf, p(0, 1)..p(2, 2), "12345678"), (0, 1));
        // A position past the text is clamped.
        assert_eq!(growth(&buf, p(2, 2)..p(2, 99), "xyz"), (0, 1));
    }

    #[test]
    fn a_replacement_that_adds_nothing_always_fits() {
        let buf = Buffer::from_text("abcd");
        let p = |col| Pos { line: 0, col };
        assert!(fits_replacing(&buf, p(0)..p(4), "wxyz"));
        assert!(fits_replacing(&buf, p(0)..p(4), ""));
        assert!(fits_replacing(&buf, p(0)..p(0), "a"));
    }
}

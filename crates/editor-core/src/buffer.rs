//! The document model: lines of text with the file's BOM, EOL and trailing
//! newline remembered so saving restores them.
//!
//! Lines are held without line terminators. Text crossing the API (`text_range`,
//! `replace`) always uses `\n`; `to_text` applies the buffer's EOL.
//!
//! Every edit is logged as a `LineShift` (which lines it replaced with how
//! many), so a renderer can move its per-line state with the lines. The log
//! keeps the last `SHIFT_LOG_LEN` shifts; asking from further back, or with
//! a mark from another buffer, gives "unknown".

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::grapheme;

/// A position: `line` index and `col` byte offset into that line.
///
/// Positions produced by user motions (via `Buffer::clamp`, `grapheme::next`
/// and `grapheme::prev`) sit on grapheme boundaries. `Edit` ranges need not:
/// an inverse edit can end inside a cluster formed by the edit itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
}

impl Eol {
    fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::CrLf => "\r\n",
        }
    }
}

/// A replacement of `range` (in the text it applies to) with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<Pos>,
    pub text: String,
}

/// One edit's effect on the lines: lines `at..at + removed` were replaced
/// by `inserted` lines. A same-line edit is `removed: 1, inserted: 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineShift {
    pub at: usize,
    pub removed: usize,
    pub inserted: usize,
}

/// A point in one buffer's edit history: which buffer, and how many edits
/// it had seen. Marks from different buffers never match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShiftMark {
    buffer: u64,
    seq: u64,
}

/// How many recent shifts a buffer keeps.
pub const SHIFT_LOG_LEN: usize = 64;

/// Hands each buffer its identity.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct Buffer {
    lines: Vec<String>,
    eol: Eol,
    bom: bool,
    trailing_newline: bool,
    /// This buffer's identity, never shared with another `Buffer` value.
    id: u64,
    /// Edits since the buffer was made.
    seq: u64,
    /// The last edits' shifts, oldest first, at most `SHIFT_LOG_LEN`.
    shifts: VecDeque<LineShift>,
}

/// A clone is a new buffer: it edits on its own from here, so it takes a
/// new identity and an empty log, and no mark taken on the original applies
/// to it.
impl Clone for Buffer {
    fn clone(&self) -> Buffer {
        Buffer::new(
            self.lines.clone(),
            self.eol,
            self.bom,
            self.trailing_newline,
        )
    }
}

const BOM: char = '\u{FEFF}';

impl Buffer {
    pub fn from_text(text: &str) -> Buffer {
        let (bom, text) = match text.strip_prefix(BOM) {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let eol = match text.find('\n') {
            Some(i) if text[..i].ends_with('\r') => Eol::CrLf,
            _ => Eol::Lf,
        };
        let normalised = text.replace("\r\n", "\n");
        let trailing_newline = normalised.ends_with('\n');
        let body = normalised.strip_suffix('\n').unwrap_or(&normalised);
        let lines = body.split('\n').map(str::to_owned).collect();
        Buffer::new(lines, eol, bom, trailing_newline)
    }

    fn new(lines: Vec<String>, eol: Eol, bom: bool, trailing_newline: bool) -> Buffer {
        Buffer {
            lines,
            eol,
            bom,
            trailing_newline,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            seq: 0,
            shifts: VecDeque::new(),
        }
    }

    /// Where this buffer's edit history is now.
    pub fn shift_mark(&self) -> ShiftMark {
        ShiftMark {
            buffer: self.id,
            seq: self.seq,
        }
    }

    /// The line shifts since `since`, oldest first (empty, and allocating
    /// nothing, when there was no edit). `None` when they are unknown: the
    /// mark is from another buffer, or older than the log. Never a part.
    pub fn shifts_since(&self, since: ShiftMark) -> Option<Vec<LineShift>> {
        if since.buffer != self.id || since.seq > self.seq {
            return None;
        }
        let n = usize::try_from(self.seq - since.seq).ok()?;
        let skip = self.shifts.len().checked_sub(n)?;
        Some(self.shifts.iter().skip(skip).copied().collect())
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if self.bom {
            out.push(BOM);
        }
        out.push_str(&self.lines.join(self.eol.as_str()));
        if self.trailing_newline {
            out.push_str(self.eol.as_str());
        }
        out
    }

    pub fn eol(&self) -> Eol {
        self.eol
    }

    /// The line at `i` without its terminator, or "" when out of range.
    pub fn line(&self, i: usize) -> &str {
        self.lines.get(i).map_or("", String::as_str)
    }

    /// The number of lines, at least 1.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Clamps the line, then the column, then snaps back to a grapheme boundary.
    pub fn clamp(&self, p: Pos) -> Pos {
        let line = p.line.min(self.lines.len() - 1);
        Pos {
            line,
            col: grapheme::floor(&self.lines[line], p.col),
        }
    }

    /// Like `clamp` but only snaps to a char boundary, so a position that is
    /// valid in the post-edit text (an inverse edit's end) is preserved.
    fn clamp_to_char(&self, p: Pos) -> Pos {
        let line = p.line.min(self.lines.len() - 1);
        let s = &self.lines[line];
        let mut col = p.col.min(s.len());
        while !s.is_char_boundary(col) {
            col -= 1;
        }
        Pos { line, col }
    }

    /// The text in `r` (clamped), joined with `\n`.
    pub fn text_range(&self, r: Range<Pos>) -> String {
        let (start, end) = self.ordered(r);
        if start.line == end.line {
            return self.lines[start.line][start.col..end.col].to_owned();
        }
        let mut out = self.lines[start.line][start.col..].to_owned();
        for l in &self.lines[start.line + 1..end.line] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&self.lines[end.line][..end.col]);
        out
    }

    /// Replaces `r` with `text` (`\n` and `\r\n` both split lines) and returns the
    /// inverse edit: its range covers the inserted text in the new buffer and its
    /// text is what was removed.
    ///
    /// This is the raw primitive that undo and redo need. Positions are snapped
    /// only to char boundaries (and the line and column are bounded), so it never
    /// panics, but it does NOT keep grapheme clusters whole: a position inside a
    /// cluster is honoured as given. Positions that originate from the user must
    /// go through `clamp` / `grapheme::next` / `grapheme::prev` first. `r` must
    /// not be reversed (debug-asserted; release collapses it to empty at `start`).
    pub fn replace(&mut self, r: Range<Pos>, text: &str) -> Edit {
        let (start, end) = self.ordered(r);
        let removed = self.text_range(start..end);
        let inserted = text.replace("\r\n", "\n");
        let head = self.lines[start.line][..start.col].to_owned();
        let tail = self.lines[end.line][end.col..].to_owned();

        let mut pieces: Vec<String> = inserted.split('\n').map(str::to_owned).collect();
        let new_end = if pieces.len() == 1 {
            Pos {
                line: start.line,
                col: start.col + pieces[0].len(),
            }
        } else {
            Pos {
                line: start.line + pieces.len() - 1,
                col: pieces[pieces.len() - 1].len(),
            }
        };
        pieces[0].insert_str(0, &head);
        if let Some(last) = pieces.last_mut() {
            last.push_str(&tail);
        }
        let shift = LineShift {
            at: start.line,
            removed: end.line - start.line + 1,
            inserted: pieces.len(),
        };
        self.lines.splice(start.line..=end.line, pieces);
        self.log(shift);

        Edit {
            range: start..new_end,
            text: removed,
        }
    }

    fn log(&mut self, shift: LineShift) {
        if self.shifts.len() == SHIFT_LOG_LEN {
            self.shifts.pop_front();
        }
        self.shifts.push_back(shift);
        self.seq += 1;
    }

    fn ordered(&self, r: Range<Pos>) -> (Pos, Pos) {
        debug_assert!(r.start <= r.end, "reversed range: {r:?}");
        let start = self.clamp_to_char(r.start);
        let end = self.clamp_to_char(r.end).max(start);
        (start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn round_trips_lf_crlf_bom_and_trailing_newline() {
        for input in ["a\nb\n", "a\r\nb", "\u{FEFF}x\n", ""] {
            assert_eq!(Buffer::from_text(input).to_text(), input, "{input:?}");
        }
        let b = Buffer::from_text("a\r\nb\r\n");
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(1), "b");
        assert_eq!(Buffer::from_text("").line_count(), 1);
        assert_eq!(Buffer::from_text("a\n").line_count(), 1);
        assert_eq!(Buffer::from_text("\u{FEFF}x").line(0), "x");
    }

    #[test]
    fn replace_returns_inverse() {
        let original = "hello world\nsecond\n";
        let mut b = Buffer::from_text(original);
        let inv = b.replace(pos(0, 6)..pos(0, 11), "there");
        assert_eq!(b.to_text(), "hello there\nsecond\n");
        assert_eq!(inv.range, pos(0, 6)..pos(0, 11));
        assert_eq!(inv.text, "world");
        let inv2 = b.replace(inv.range, &inv.text);
        assert_eq!(b.to_text(), original);
        assert_eq!(inv2.text, "there");
    }

    #[test]
    fn multiline_replace_splits_and_joins() {
        let mut b = Buffer::from_text("abcd\nefgh");
        let inv = b.replace(pos(0, 2)..pos(0, 2), "X\nY\nZ");
        assert_eq!(b.to_text(), "abX\nY\nZcd\nefgh");
        assert_eq!(inv.range, pos(0, 2)..pos(2, 1));
        assert_eq!(inv.text, "");

        // Join lines by deleting across a line break.
        let inv = b.replace(pos(0, 1)..pos(3, 2), "");
        assert_eq!(b.to_text(), "agh");
        assert_eq!(inv.text, "bX\nY\nZcd\nef");
        b.replace(inv.range, &inv.text);
        assert_eq!(b.to_text(), "abX\nY\nZcd\nefgh");
    }

    #[test]
    fn crlf_buffer_normalises_inserted_line_endings() {
        let mut b = Buffer::from_text("a\r\nb");
        b.replace(pos(1, 1)..pos(1, 1), "\r\nc");
        assert_eq!(b.to_text(), "a\r\nb\r\nc");
    }

    #[test]
    fn inverse_survives_joining_into_a_cluster() {
        let mut b = Buffer::from_text("\u{301}");
        let inv = b.replace(pos(0, 0)..pos(0, 0), "e");
        assert_eq!(b.to_text(), "e\u{301}");
        b.replace(inv.range, &inv.text);
        assert_eq!(b.to_text(), "\u{301}");
    }

    #[test]
    fn edits_never_split_graphemes() {
        let s = "e\u{301}👩‍👩‍👧x";
        let b = Buffer::from_text(s);
        assert_eq!(b.clamp(pos(0, 2)), pos(0, 0));
        assert_eq!(b.clamp(pos(0, 3)), pos(0, 3));
        assert_eq!(b.clamp(pos(0, 10)), pos(0, 3));
        assert_eq!(b.clamp(pos(0, 99)), pos(0, 22));
        assert_eq!(b.clamp(pos(9, 99)), pos(0, 22));
    }

    #[test]
    fn bad_offsets_never_panic() {
        let mut b = Buffer::from_text("e\u{301}x\nlé");
        // Mid-char and out-of-range offsets are tolerated.
        b.replace(pos(0, 2)..pos(7, 400), "Z");
        assert_eq!(b.line_count(), 1);
        // Raw-primitive behaviour: col 2 is mid-char, so it snaps to the char
        // boundary at 1, which is inside the "e\u{301}" cluster. `replace` does
        // not protect grapheme clusters; callers go through `clamp` first.
        assert_eq!(b.to_text(), "eZ");
        let _ = b.text_range(pos(1, 1)..pos(5, 5));
    }

    /// The shifts `b` logged since `m`, which must be known.
    fn since(b: &Buffer, m: ShiftMark) -> Vec<LineShift> {
        b.shifts_since(m).expect("shifts known")
    }

    fn shift(at: usize, removed: usize, inserted: usize) -> LineShift {
        LineShift {
            at,
            removed,
            inserted,
        }
    }

    #[test]
    fn replace_logs_its_line_shift() {
        let mut b = Buffer::from_text("abc\ndef\nghi\njkl");
        let first = b.shift_mark();
        assert_eq!(since(&b, first), Vec::new(), "nothing yet");

        // Enter mid-line: line 0 becomes two lines.
        let m = b.shift_mark();
        b.replace(pos(0, 1)..pos(0, 1), "\n");
        assert_eq!(b.to_text(), "a\nbc\ndef\nghi\njkl");
        assert_eq!(since(&b, m), vec![shift(0, 1, 2)]);

        // A join: lines 0 and 1 become one.
        let m = b.shift_mark();
        b.replace(pos(0, 1)..pos(1, 0), "");
        assert_eq!(b.to_text(), "abc\ndef\nghi\njkl");
        assert_eq!(since(&b, m), vec![shift(0, 2, 1)]);

        // A multi-line delete: lines 1 to 3 become one.
        let m = b.shift_mark();
        b.replace(pos(1, 1)..pos(3, 1), "");
        assert_eq!(b.to_text(), "abc\ndkl");
        assert_eq!(since(&b, m), vec![shift(1, 3, 1)]);

        // A multi-line paste of whole lines above line 1.
        let m = b.shift_mark();
        b.replace(pos(1, 0)..pos(1, 0), "x\ny\nz\n");
        assert_eq!(b.to_text(), "abc\nx\ny\nz\ndkl");
        assert_eq!(since(&b, m), vec![shift(1, 1, 4)]);

        // A same-line edit.
        let m = b.shift_mark();
        b.replace(pos(4, 0)..pos(4, 1), "D");
        assert_eq!(since(&b, m), vec![shift(4, 1, 1)]);

        // Every shift since the first mark, in order.
        let all = vec![
            shift(0, 1, 2),
            shift(0, 2, 1),
            shift(1, 3, 1),
            shift(1, 1, 4),
            shift(4, 1, 1),
        ];
        assert_eq!(since(&b, first), all);
        assert_eq!(since(&b, b.shift_mark()), Vec::new());
    }

    #[test]
    fn shift_log_is_bounded() {
        let mut b = Buffer::from_text("a");
        let old = b.shift_mark();
        b.replace(pos(0, 0)..pos(0, 0), "x");
        let kept = b.shift_mark();
        for _ in 0..SHIFT_LOG_LEN {
            b.replace(pos(0, 0)..pos(0, 0), "x");
        }
        assert_eq!(b.shifts_since(old), None, "older than the log: unknown");
        assert_eq!(since(&b, kept).len(), SHIFT_LOG_LEN, "exactly the log");
        assert_eq!(b.shifts.len(), SHIFT_LOG_LEN, "the log stays bounded");
    }

    #[test]
    fn a_new_buffer_never_matches_an_old_mark() {
        let a = Buffer::from_text("same");
        let m = a.shift_mark();
        assert_eq!(a.shifts_since(m), Some(Vec::new()));
        let b = Buffer::from_text("same");
        assert_ne!(b.shift_mark(), m);
        assert_eq!(b.shifts_since(m), None);
        // A clone edits on its own, so it is a new buffer too.
        let c = a.clone();
        assert_eq!(c.shifts_since(m), None);
        assert_eq!(c.text_range(pos(0, 0)..pos(0, 4)), "same");
    }

    #[test]
    fn text_range_spans_lines() {
        let b = Buffer::from_text("abc\ndef\nghi");
        assert_eq!(b.text_range(pos(0, 1)..pos(2, 2)), "bc\ndef\ngh");
        assert_eq!(b.text_range(pos(1, 1)..pos(1, 3)), "ef");
    }
}

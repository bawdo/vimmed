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

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::colindex::LineCols;
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
    /// Column indexes for up to two lines, `(line, index)`, most recently
    /// used first. `replace` keeps them true to the text.
    cols: RefCell<[Option<(usize, LineCols)>; 2]>,
}

/// A clone is a new buffer: it edits on its own from here, so it takes a
/// new identity, an empty log and no column index, and no mark taken on the
/// original applies to it.
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
            cols: RefCell::default(),
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

    /// The number of graphemes before `pos.col` on `pos.line`; always equal
    /// to `grapheme::display_col(self.line(pos.line), pos.col)`.
    pub fn display_col(&self, pos: Pos) -> usize {
        self.with_cols(pos.line, |cols, line| cols.display_col(line, pos.col))
    }

    /// The byte where grapheme `g` (0-based) of `line` starts, or `None` when
    /// the line has `g` or fewer graphemes.
    pub fn grapheme_start(&self, line: usize, g: usize) -> Option<usize> {
        self.with_cols(line, |cols, text| cols.grapheme_start(text, g))
    }

    /// Runs `f` on the column index for `line`, making it the most recently
    /// used entry (or, for a line not cached, a fresh entry in place of the
    /// least recently used one). A line past the end gets a throwaway index.
    fn with_cols<R>(&self, line: usize, f: impl FnOnce(&mut LineCols, &str) -> R) -> R {
        let Some(text) = self.lines.get(line) else {
            return f(&mut LineCols::default(), "");
        };
        let mut cols = self.cols.borrow_mut();
        let found = cols
            .iter()
            .position(|e| matches!(e, Some((l, _)) if *l == line));
        let mut index = found
            .and_then(|i| cols[i].take())
            .map_or_else(LineCols::default, |(_, c)| c);
        let answer = f(&mut index, text);
        if found != Some(0) {
            cols[1] = cols[0].take();
        }
        cols[0] = Some((line, index));
        answer
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
        self.update_cols(start, end, shift);

        Edit {
            range: start..new_end,
            text: removed,
        }
    }

    /// Keeps the column indexes true after `start..end` (old coordinates)
    /// was replaced as `shift` describes. An edit within one line, with no
    /// line break inserted, keeps that line's checkpoints before `start.col`:
    /// a boundary there depends only on the unchanged text up to and
    /// including the character at it. An edit wholly above a line moves its
    /// index by the change in line count; one wholly below leaves it alone.
    /// Any other edit touching the line drops its index.
    fn update_cols(&mut self, start: Pos, end: Pos, shift: LineShift) {
        let same_line = start.line == end.line && shift.inserted == 1;
        for entry in self.cols.get_mut().iter_mut() {
            let Some((line, index)) = entry else {
                continue;
            };
            if *line < start.line {
                continue;
            }
            if *line > end.line {
                *line = *line + shift.inserted - shift.removed;
            } else if same_line {
                index.keep_before(start.col);
            } else {
                *entry = None;
            }
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
    use crate::colindex;
    use unicode_segmentation::UnicodeSegmentation;

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
    /// `unit` repeated until the result is at least `bytes` long.
    fn fill(unit: &str, bytes: usize) -> String {
        unit.repeat(bytes.div_ceil(unit.len()))
    }

    /// The mixed string from the column index tests: ASCII, a precomposed
    /// letter, a flag pair, a ZWJ family and a lone `\r`.
    fn mixed(bytes: usize) -> String {
        fill("aé🇦🇺👩‍👩‍👧\r", bytes)
    }

    /// The reference answer for `grapheme_start`.
    fn nth_start(line: &str, g: usize) -> Option<usize> {
        line.grapheme_indices(true).nth(g).map(|(i, _)| i)
    }

    /// The line each cached entry names, most recently used first.
    fn cached_lines(b: &Buffer) -> Vec<Option<usize>> {
        b.cols
            .borrow()
            .iter()
            .map(|e| e.as_ref().map(|(l, _)| *l))
            .collect()
    }

    /// The cached entry for `line`, printed, so a test can see whether it
    /// changed without reaching into the index's own fields.
    fn cached(b: &Buffer, line: usize) -> Option<String> {
        b.cols
            .borrow()
            .iter()
            .flatten()
            .find(|(l, _)| *l == line)
            .map(|(_, c)| format!("{c:?}"))
    }

    /// A seeded LCG, so the random order needs no new dependency.
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((self.0 >> 33) as usize) % n
        }
    }

    /// A random char boundary in `line`, the end included.
    fn boundary(rng: &mut Lcg, line: &str) -> usize {
        floor_char(line, rng.below(line.len() + 1))
    }

    fn assert_cols(b: &Buffer, line: usize, col: usize) {
        assert_eq!(
            b.display_col(pos(line, col)),
            grapheme::display_col(b.line(line), col),
            "display_col @ {line}:{col}"
        );
    }

    /// The char boundary at or before `col`, within `line`.
    fn floor_char(line: &str, col: usize) -> usize {
        let mut c = col.min(line.len());
        while !line.is_char_boundary(c) {
            c -= 1;
        }
        c
    }

    #[test]
    fn display_col_matches_after_random_edits() {
        let text = [mixed(5_000), mixed(3_000), mixed(4_000)].join("\n");
        let mut b = Buffer::from_text(&text);
        let mut rng = Lcg(0xb0ff);
        let inserts = ["", "x", "é", "🇦", "🇺", "\u{301}", "\u{200d}", "👩", "\r"];
        let (mut edits, mut deep) = (0, 0);
        for step in 0..2_000 {
            let line = rng.below(b.line_count());
            let s = b.line(line).to_owned();
            let col = boundary(&mut rng, &s);
            // Deletions stay short, so lines stay long enough to hold
            // checkpoints.
            let near = floor_char(&s, col + rng.below(64));
            match rng.below(10) {
                0..=3 => {
                    assert_cols(&b, line, col);
                    assert_cols(&b, line, s.len());
                    deep += usize::from(col > 2 * colindex::COL_STEP);
                }
                4 => {
                    let n = s.graphemes(true).count();
                    let g = rng.below(n + 2);
                    assert_eq!(
                        b.grapheme_start(line, g),
                        nth_start(&s, g),
                        "grapheme_start {line} {g} at step {step}"
                    );
                }
                5 => {
                    // Inside one line.
                    let t = inserts[rng.below(inserts.len())].repeat(1 + rng.below(3));
                    b.replace(pos(line, col)..pos(line, near), &t);
                    edits += 1;
                }
                6 if line + 1 < b.line_count() => {
                    // Across lines: from near this line's end to near the
                    // next one's start, or the one after.
                    let to = (line + 1 + rng.below(2)).min(b.line_count() - 1);
                    let from = floor_char(&s, s.len().saturating_sub(rng.below(64)));
                    let till = floor_char(b.line(to), rng.below(64));
                    let t = inserts[rng.below(inserts.len())];
                    b.replace(pos(line, from)..pos(to, till), t);
                    edits += 1;
                }
                7 if b.line_count() < 5 => {
                    // Inserting a line break, alone or among other text.
                    let t = ["\n", "é\n🇦", "\r\n", "a\nb\nc"][rng.below(4)];
                    b.replace(pos(line, col)..pos(line, col), t);
                    edits += 1;
                }
                8 if s.len() < 8_000 => {
                    // Growing the line, so splits do not wear it down.
                    b.replace(pos(line, col)..pos(line, col), &mixed(rng.below(3_000)));
                    edits += 1;
                }
                _ => {
                    // A combining mark, which joins the grapheme before it.
                    b.replace(pos(line, col)..pos(line, near.min(col + 4)), "\u{301}");
                    edits += 1;
                }
            }
        }
        assert!(edits > 500, "too few edits: {edits}");
        assert!(deep > 100, "too few queries past a checkpoint: {deep}");
        let long = (0..b.line_count())
            .filter(|&l| b.line(l).len() > 2 * colindex::COL_STEP)
            .count();
        assert!(long >= 2, "the lines wore down: {long} long");
        for line in 0..b.line_count() {
            let s = b.line(line).to_owned();
            for col in (0..=s.len()).filter(|&c| s.is_char_boundary(c)).step_by(7) {
                assert_cols(&b, line, col);
            }
        }
    }

    #[test]
    fn combining_mark_at_a_checkpoint() {
        let mut b = Buffer::from_text(&"é".repeat(2_048));
        assert_eq!(b.line(0).len(), 4_096);
        assert_cols(&b, 0, 4_096);
        let before = cached(&b, 0).unwrap_or_default();
        assert!(before.contains("(2048, 1024)"), "no checkpoint: {before}");
        b.replace(pos(0, 2_048)..pos(0, 2_048), "\u{301}");
        assert_cols(&b, 0, b.line(0).len());
        assert_eq!(b.display_col(pos(0, b.line(0).len())), 2_048);
        assert_eq!(b.grapheme_start(0, 1_024), Some(2_050));
        assert_eq!(b.grapheme_start(0, 1_023), Some(2_046));
    }

    #[test]
    fn undo_restores_then_queries() {
        let mut b = Buffer::from_text(&"é".repeat(5_000));
        let end = |b: &Buffer| pos(0, b.line(0).len());
        assert_cols(&b, 0, end(&b).col);
        let inv = b.replace(pos(0, 0)..end(&b), &"a".repeat(7_000));
        assert_cols(&b, 0, end(&b).col);
        assert_eq!(b.display_col(end(&b)), 7_000);
        b.replace(inv.range, &inv.text);
        assert_eq!(b.line(0), "é".repeat(5_000));
        assert_cols(&b, 0, end(&b).col);
        assert_eq!(b.display_col(end(&b)), 5_000);
        assert_eq!(b.grapheme_start(0, 4_999), Some(9_998));
    }

    #[test]
    fn line_shift_moves_the_index() {
        let long = "é".repeat(3_000);
        let text = ["a", "b", "c", "d", "e", long.as_str(), "g"].join("\n");
        let mut b = Buffer::from_text(&text);
        assert_cols(&b, 5, long.len());
        let before = cached(&b, 5).expect("line 5 is cached");
        b.replace(pos(1, 0)..pos(1, 0), "x\ny\n");
        assert_eq!(b.line(7), long);
        assert_eq!(cached_lines(&b), vec![Some(7), None]);
        assert_eq!(cached(&b, 7), Some(before), "the checkpoints moved whole");
        assert_cols(&b, 7, long.len());
        assert_eq!(b.grapheme_start(7, 2_999), Some(5_998));

        // A join above moves it back up.
        b.replace(pos(0, 1)..pos(1, 0), "");
        assert_eq!(cached_lines(&b), vec![Some(6), None]);
        assert_cols(&b, 6, long.len());
    }

    #[test]
    fn edit_below_keeps_the_index() {
        let long = "é".repeat(3_000);
        let text = [long.as_str(), "b", "c"].join("\n");
        let mut b = Buffer::from_text(&text);
        assert_cols(&b, 0, long.len());
        let before = cached(&b, 0).expect("line 0 is cached");
        assert!(!before.contains("checkpoints: []"), "{before}");
        b.replace(pos(1, 0)..pos(2, 1), "x\ny\nz");
        assert_eq!(cached(&b, 0), Some(before.clone()), "an edit below");
        b.replace(pos(2, 0)..pos(2, 0), "\u{301}");
        assert_eq!(cached(&b, 0), Some(before), "a same-line edit below");
        assert_cols(&b, 0, long.len());
    }

    #[test]
    fn edits_touching_the_line_drop_or_trim_it() {
        let long = "é".repeat(3_000);
        let text = ["a", long.as_str(), "c"].join("\n");

        // A same-line edit keeps only the checkpoints before it.
        let mut b = Buffer::from_text(&text);
        assert_cols(&b, 1, long.len());
        b.replace(pos(1, 3_000)..pos(1, 3_002), "🇦");
        let kept = cached(&b, 1).expect("still cached");
        assert!(
            kept.contains("(2048, 1024)") && !kept.contains("(3072"),
            "{kept}"
        );
        assert_cols(&b, 1, b.line(1).len());

        // A line break inside the line, or an edit spanning into it from
        // above, drops it.
        for (r, t) in [
            (pos(1, 10)..pos(1, 10), "\n"),
            (pos(0, 1)..pos(1, 4), ""),
            (pos(1, 4)..pos(2, 0), "x"),
        ] {
            let mut b = Buffer::from_text(&text);
            assert_cols(&b, 1, long.len());
            b.replace(r.clone(), t);
            assert_eq!(cached_lines(&b), vec![None, None], "{r:?}");
            for l in 0..b.line_count() {
                assert_cols(&b, l, b.line(l).len());
            }
        }
    }

    #[test]
    fn a_third_line_replaces_the_least_recent() {
        let long = "é".repeat(2_000);
        let text = [long.as_str(), long.as_str(), long.as_str()].join("\n");
        let b = Buffer::from_text(&text);
        assert_cols(&b, 0, 100);
        assert_cols(&b, 1, 100);
        assert_eq!(cached_lines(&b), vec![Some(1), Some(0)]);
        assert_cols(&b, 0, 200);
        assert_eq!(cached_lines(&b), vec![Some(0), Some(1)]);
        assert_cols(&b, 2, 300);
        assert_eq!(cached_lines(&b), vec![Some(2), Some(0)]);
        assert_eq!(b.grapheme_start(1, 5), Some(10));
        assert_eq!(cached_lines(&b), vec![Some(1), Some(2)]);
        // A line past the end is answered, not cached.
        assert_eq!(b.display_col(pos(9, 4)), 0);
        assert_eq!(b.grapheme_start(9, 0), None);
        assert_eq!(cached_lines(&b), vec![Some(1), Some(2)]);
    }

    #[test]
    fn clone_has_no_index() {
        let b = Buffer::from_text(&"é".repeat(3_000));
        assert_cols(&b, 0, 6_000);
        assert_eq!(cached_lines(&b), vec![Some(0), None]);
        let c = b.clone();
        assert_eq!(cached_lines(&c), vec![None, None]);
        assert_cols(&c, 0, 6_000);
    }
}

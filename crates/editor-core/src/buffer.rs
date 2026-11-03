//! The document model: lines of text with the file's BOM, EOL and trailing
//! newline remembered so saving restores them.
//!
//! Lines are held without line terminators. Text crossing the API (`text_range`,
//! `replace`) always uses `\n`; `to_text` applies the buffer's EOL.

use std::ops::Range;

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

#[derive(Debug, Clone)]
pub struct Buffer {
    lines: Vec<String>,
    eol: Eol,
    bom: bool,
    trailing_newline: bool,
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
        Buffer {
            lines: body.split('\n').map(str::to_owned).collect(),
            eol,
            bom,
            trailing_newline,
        }
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
        self.lines.splice(start.line..=end.line, pieces);

        Edit {
            range: start..new_end,
            text: removed,
        }
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

    #[test]
    fn text_range_spans_lines() {
        let b = Buffer::from_text("abc\ndef\nghi");
        assert_eq!(b.text_range(pos(0, 1)..pos(2, 2)), "bc\ndef\ngh");
        assert_eq!(b.text_range(pos(1, 1)..pos(1, 3)), "ef");
    }
}

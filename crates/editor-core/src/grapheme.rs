//! Grapheme cluster stepping over a single line. Columns are byte offsets.
//!
//! Every function tolerates out-of-range and mid-character offsets: they
//! saturate rather than panic.

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

// Each search runs locally from `col` with a GraphemeCursor, so a call costs
// O(cluster plus any preceding regional-indicator run) rather than a rescan
// from byte 0. (A fresh cursor counts back to the start of an RI run to decide
// flag pairing, so walking a long run of RIs is still quadratic.) The whole
// line is passed as the one chunk, starting at 0, so the cursor never needs
// pre-context or another chunk: its errors are unreachable. Even so, any `Err`
// falls back to the saturating value instead of panicking.

/// Clamp `col` to the line and snap a mid-character offset down to the start
/// of that character, so it is a valid cursor position.
fn char_floor(line: &str, col: usize) -> usize {
    let mut c = col.min(line.len());
    while !line.is_char_boundary(c) {
        c -= 1;
    }
    c
}

/// Whether the char boundary `c` is a grapheme boundary.
fn is_boundary(line: &str, c: usize) -> bool {
    GraphemeCursor::new(c, line.len(), true)
        .is_boundary(line, 0)
        .unwrap_or(true)
}

/// The greatest grapheme boundary strictly before the char boundary `c`, or 0.
fn boundary_before(line: &str, c: usize) -> usize {
    GraphemeCursor::new(c, line.len(), true)
        .prev_boundary(line, 0)
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// The start of the next grapheme cluster after `col`, or the line length.
pub fn next(line: &str, col: usize) -> usize {
    // Snapping a mid-char `col` down is safe: no boundary lies strictly
    // between the snapped offset and `col`, so the first boundary after
    // either is the same.
    let c = char_floor(line, col);
    GraphemeCursor::new(c, line.len(), true)
        .next_boundary(line, 0)
        .ok()
        .flatten()
        .unwrap_or(line.len())
}

/// The start of the grapheme cluster before `col`, or 0.
pub fn prev(line: &str, col: usize) -> usize {
    let c = char_floor(line, col);
    if c < col && c < line.len() && is_boundary(line, c) {
        // `col` was mid-char, so `c` itself lies strictly before it and is the
        // answer when it is a cluster start. The line end is excluded: it
        // starts no cluster, so a past-the-end `col` steps back from it.
        return c;
    }
    boundary_before(line, c)
}

/// The greatest grapheme boundary at or before `col` (saturating at the line length).
pub fn floor(line: &str, col: usize) -> usize {
    if col >= line.len() {
        return line.len();
    }
    let c = char_floor(line, col);
    if is_boundary(line, c) {
        c
    } else {
        boundary_before(line, c)
    }
}

/// The number of graphemes before byte offset `col`, for `line:column` display (0-based).
pub fn display_col(line: &str, col: usize) -> usize {
    // Lines never contain `\n`, so an ASCII prefix holds no `\r\n` pair and
    // each of its bytes starts a grapheme.
    let end = col.min(line.len());
    if line.as_bytes()[..end].is_ascii() {
        return end;
    }
    line.grapheme_indices(true)
        .take_while(|&(i, _)| i < col)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "e\u{301}👩‍👩‍👧x";

    #[test]
    fn edits_never_split_graphemes_stepping() {
        let mut fwd = vec![0];
        let mut c = 0;
        while c < S.len() {
            c = next(S, c);
            fwd.push(c);
        }
        assert_eq!(fwd, [0, 3, 21, 22]);

        let mut back = vec![S.len()];
        let mut c = S.len();
        while c > 0 {
            c = prev(S, c);
            back.push(c);
        }
        assert_eq!(back, [22, 21, 3, 0]);
    }

    #[test]
    fn stepping_saturates_and_survives_bad_offsets() {
        assert_eq!(next(S, 22), 22);
        assert_eq!(next(S, 999), 22);
        assert_eq!(prev(S, 0), 0);
        assert_eq!(prev(S, 999), 21);
        // Mid-char offset (inside the 2-byte U+0301): must not panic.
        assert_eq!(next(S, 2), 3);
        assert_eq!(prev(S, 2), 0);
        assert_eq!(floor(S, 2), 0);
        assert_eq!(floor(S, 999), 22);
        assert_eq!(floor(S, 3), 3);
    }

    // The original whole-line scans, kept as a reference so the local
    // GraphemeCursor search can be checked against them on every offset.
    fn old_next(line: &str, col: usize) -> usize {
        line.grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|&i| i > col)
            .unwrap_or(line.len())
    }

    fn old_prev(line: &str, col: usize) -> usize {
        line.grapheme_indices(true)
            .map(|(i, _)| i)
            .take_while(|&i| i < col)
            .last()
            .unwrap_or(0)
    }

    fn old_floor(line: &str, col: usize) -> usize {
        if col >= line.len() {
            return line.len();
        }
        line.grapheme_indices(true)
            .map(|(i, _)| i)
            .take_while(|&i| i <= col)
            .last()
            .unwrap_or(0)
    }

    #[test]
    fn local_search_matches_whole_line_scan_on_every_offset() {
        let long: String = "ab\u{301}c 🇦🇺🇳🇿 한\u{1100}\u{1161}\u{11A8} 👨‍👩‍👧‍👦\t\r\nz".repeat(8);
        let lines: Vec<&str> = vec![
            "",
            "plain ascii",
            S,
            "a\r\nb",
            "\r\n",
            "a\rb\r",
            "e\u{301}\u{302}\u{303}\u{304}x",
            "\u{301}\u{302}lead",
            "👨‍👩‍👧‍👦👩‍👩‍👧x👨‍👩‍👦",
            "🇦🇺",
            "🇦🇺🇳",
            "🇦🇺🇳🇿",
            "x🇦🇺🇳🇿🇬",
            "\u{1100}\u{1161}\u{11A8}\u{1100}\u{1161}",
            "한국어",
            "a\tb\t",
            "क्षत्रिय",
            "\u{1F3F4}\u{E0067}\u{E0062}\u{E007F}",
            "\u{0600}1",
            "\u{1F44D}\u{1F3FD}\u{2764}\u{FE0F}",
            &long,
        ];
        for line in lines {
            for col in 0..=line.len() + 2 {
                assert_eq!(
                    next(line, col),
                    old_next(line, col),
                    "next {line:?} @ {col}"
                );
                assert_eq!(
                    prev(line, col),
                    old_prev(line, col),
                    "prev {line:?} @ {col}"
                );
                assert_eq!(
                    floor(line, col),
                    old_floor(line, col),
                    "floor {line:?} @ {col}"
                );
            }
        }
    }

    #[test]
    fn display_col_counts_graphemes() {
        assert_eq!(display_col("👩‍👩‍👧x", 18), 1);
        assert_eq!(display_col("👩‍👩‍👧x", 0), 0);
        assert_eq!(display_col("👩‍👩‍👧x", 19), 2);
        assert_eq!(display_col(S, 22), 3);
    }
}

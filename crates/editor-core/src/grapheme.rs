//! Grapheme cluster stepping over a single line. Columns are byte offsets.
//!
//! Every function tolerates out-of-range and mid-character offsets: they
//! saturate rather than panic.

use unicode_segmentation::UnicodeSegmentation;

/// The start of the next grapheme cluster after `col`, or the line length.
pub fn next(line: &str, col: usize) -> usize {
    line.grapheme_indices(true)
        .map(|(i, _)| i)
        .find(|&i| i > col)
        .unwrap_or(line.len())
}

/// The start of the grapheme cluster before `col`, or 0.
pub fn prev(line: &str, col: usize) -> usize {
    line.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i < col)
        .last()
        .unwrap_or(0)
}

/// The greatest grapheme boundary at or before `col` (saturating at the line length).
pub fn floor(line: &str, col: usize) -> usize {
    if col >= line.len() {
        return line.len();
    }
    line.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i <= col)
        .last()
        .unwrap_or(0)
}

/// The number of graphemes before byte offset `col`, for `line:column` display (0-based).
pub fn display_col(line: &str, col: usize) -> usize {
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

    #[test]
    fn display_col_counts_graphemes() {
        assert_eq!(display_col("👩‍👩‍👧x", 18), 1);
        assert_eq!(display_col("👩‍👩‍👧x", 0), 0);
        assert_eq!(display_col("👩‍👩‍👧x", 19), 2);
        assert_eq!(display_col(S, 22), 3);
    }
}

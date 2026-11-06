//! A checkpointed grapheme column index for one line, so a column query costs
//! O(`COL_STEP`) after the first walk rather than O(line length).
//!
//! A checkpoint is a grapheme boundary reached by a segmentation run that
//! started at byte 0 or at an earlier checkpoint. UAX #29 rules may be
//! evaluated from any known boundary, so a run restarted at a checkpoint
//! finds the same boundaries as a run from the start of the line. A byte
//! picked any other way could sit inside a regional-indicator pair, where a
//! restart pairs the flags wrongly.
//!
//! Lines never contain `\n`, so an ASCII prefix holds no `\r\n` pair and
//! every byte in it starts a grapheme.

use unicode_segmentation::UnicodeSegmentation;

/// The least distance in bytes between one checkpoint and the next.
pub(crate) const COL_STEP: usize = 1024;

/// Grapheme counts at known boundaries of one line.
#[derive(Debug, Default, Clone)]
pub(crate) struct LineCols {
    /// `(byte, graphemes_before)`, ascending by byte with no duplicates;
    /// `(0, 0)` is implied and never stored.
    checkpoints: Vec<(usize, usize)>,
}

impl LineCols {
    /// The number of graphemes before byte offset `col`; always equal to
    /// `grapheme::display_col(line, col)`.
    pub(crate) fn display_col(&mut self, line: &str, col: usize) -> usize {
        let end = col.min(line.len());
        if line.as_bytes()[..end].is_ascii() {
            return end;
        }
        let at = self.checkpoints.partition_point(|&(b, _)| b <= col);
        let (start, before) = at.checked_sub(1).map_or((0, 0), |i| self.checkpoints[i]);
        let mut n = before;
        for (i, _) in line[start..].grapheme_indices(true) {
            let b = start + i;
            self.note(b, n);
            if b >= col {
                break;
            }
            n += 1;
        }
        n
    }

    /// The byte where grapheme `g` (0-based) starts, or `None` when the line
    /// has `g` or fewer graphemes.
    pub(crate) fn grapheme_start(&mut self, line: &str, g: usize) -> Option<usize> {
        if line.is_ascii() {
            return (g < line.len()).then_some(g);
        }
        let at = self.checkpoints.partition_point(|&(_, n)| n <= g);
        let (start, before) = at.checked_sub(1).map_or((0, 0), |i| self.checkpoints[i]);
        for (n, (i, _)) in (before..).zip(line[start..].grapheme_indices(true)) {
            let b = start + i;
            self.note(b, n);
            if n == g {
                return Some(b);
            }
        }
        None
    }

    /// Drops every checkpoint at or after byte `col`, as an edit starting at
    /// `col` may move those boundaries. Earlier ones depend only on text
    /// before `col` and survive.
    pub(crate) fn keep_before(&mut self, col: usize) {
        let keep = self.checkpoints.partition_point(|&(b, _)| b < col);
        self.checkpoints.truncate(keep);
    }

    /// Records the boundary `b`, with `n` graphemes before it, when it lies
    /// at least `COL_STEP` beyond the last checkpoint. Only boundaries past
    /// the last one are stored, which keeps the list ascending.
    fn note(&mut self, b: usize, n: usize) {
        let last = self.checkpoints.last().map_or(0, |&(b, _)| b);
        if b >= last + COL_STEP {
            self.checkpoints.push((b, n));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grapheme;
    use unicode_segmentation::UnicodeSegmentation;

    /// `unit` repeated until the result is at least `bytes` long.
    fn fill(unit: &str, bytes: usize) -> String {
        unit.repeat(bytes.div_ceil(unit.len()))
    }

    fn strings() -> Vec<String> {
        vec![
            fill("é", 5_000),
            fill("e\u{301}", 5_000),
            fill("🇦🇺", 5_000),
            fill("👩‍👩‍👧", 5_000),
            fill("\u{1100}\u{1161}\u{11A8}", 5_000),
            fill("aé🇦🇺👩‍👩‍👧\r", 5_000),
            // An ASCII prefix longer than `COL_STEP`, then non-ASCII.
            "x".repeat(3_000) + &fill("é", 2_000),
        ]
    }

    fn char_boundaries(s: &str) -> Vec<usize> {
        (0..=s.len()).filter(|&c| s.is_char_boundary(c)).collect()
    }

    /// The number of graphemes before every byte offset, from one whole-line
    /// segmentation run, independent of any fast path.
    fn reference(s: &str) -> Vec<usize> {
        let starts: Vec<usize> = s.grapheme_indices(true).map(|(i, _)| i).collect();
        (0..=s.len())
            .map(|c| starts.partition_point(|&i| i < c))
            .collect()
    }

    /// A seeded LCG, so the random order needs no new dependency.
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((self.0 >> 33) as usize) % n
        }
    }

    fn assert_ascending(cols: &LineCols) {
        assert!(
            cols.checkpoints.windows(2).all(|w| w[0].0 < w[1].0),
            "checkpoints not ascending: {:?}",
            cols.checkpoints
        );
    }

    #[test]
    fn ascii_is_byte_offset() {
        let mut cols = LineCols::default();
        assert_eq!(cols.display_col("abc", 2), 2);
        assert_eq!(cols.grapheme_start("abc", 3), None);
        assert_eq!(cols.grapheme_start("abc", 2), Some(2));

        let long = "e".repeat(10_000);
        assert_eq!(cols.display_col(&long, 9_999), 9_999);
        assert_eq!(cols.display_col(&long, 10_000), 10_000);
        assert_eq!(cols.display_col(&long, 20_000), 10_000);
        assert_eq!(cols.grapheme_start(&long, 9_999), Some(9_999));
        assert_eq!(cols.grapheme_start(&long, 10_000), None);
        assert!(cols.checkpoints.is_empty());
    }

    #[test]
    fn matches_display_col_everywhere() {
        let mut rng = Lcg(0x5eed);
        for s in strings() {
            let want = reference(&s);
            let bounds = char_boundaries(&s);
            for &c in &bounds {
                assert_eq!(grapheme::display_col(&s, c), want[c], "grapheme @ {c}");
                assert_eq!(
                    LineCols::default().display_col(&s, c),
                    want[c],
                    "fresh @ {c}"
                );
            }
            let mut shared = LineCols::default();
            for _ in 0..bounds.len() * 2 {
                let c = bounds[rng.below(bounds.len())];
                assert_eq!(shared.display_col(&s, c), want[c], "shared @ {c}");
            }
            assert_ascending(&shared);
            assert!(!shared.checkpoints.is_empty());
        }
    }

    #[test]
    fn grapheme_start_inverts() {
        let mut rng = Lcg(0xc0105);
        for s in strings() {
            let starts: Vec<usize> = s.grapheme_indices(true).map(|(i, _)| i).collect();
            let want = |g: usize| starts.get(g).copied();
            for g in 0..starts.len() + 2 {
                assert_eq!(
                    LineCols::default().grapheme_start(&s, g),
                    want(g),
                    "fresh {g}"
                );
            }
            let mut shared = LineCols::default();
            for _ in 0..starts.len() * 2 {
                let g = rng.below(starts.len() + 2);
                assert_eq!(shared.grapheme_start(&s, g), want(g), "shared {g}");
                // Interleave the forward query on the same index.
                let c = rng.below(s.len() + 1);
                if s.is_char_boundary(c) {
                    assert_eq!(
                        shared.display_col(&s, c),
                        grapheme::display_col(&s, c),
                        "mixed @ {c}"
                    );
                }
            }
            assert_ascending(&shared);
        }
    }

    #[test]
    fn flags_across_checkpoints() {
        let s = "🇦🇺".repeat(2_000);
        let mut cols = LineCols::default();
        for c in (0..=s.len()).step_by(8) {
            assert_eq!(
                cols.display_col(&s, c),
                grapheme::display_col(&s, c),
                "@ {c}"
            );
        }
        assert!(cols.checkpoints.len() > 1);
        assert_ascending(&cols);
        // Every checkpoint sits between whole flags, never inside a pair.
        assert!(
            cols.checkpoints
                .iter()
                .all(|&(b, n)| b % 8 == 0 && n == b / 8)
        );
        // Asking backwards, from a cold index, agrees too.
        let mut cold = LineCols::default();
        for c in (0..s.len() + 1).step_by(8).rev() {
            assert_eq!(cold.display_col(&s, c), c / 8, "cold @ {c}");
        }
    }

    #[test]
    fn keep_before_keeps_only_earlier() {
        let s = fill("é", 5_000);
        let mut cols = LineCols::default();
        assert_eq!(cols.display_col(&s, s.len()), s.len() / 2);
        assert!(cols.checkpoints.iter().any(|&(b, _)| b >= 3_000));

        cols.keep_before(3_000);
        assert!(!cols.checkpoints.is_empty());
        assert!(cols.checkpoints.iter().all(|&(b, _)| b < 3_000));

        for c in (0..=s.len()).step_by(2) {
            assert_eq!(cols.display_col(&s, c), c / 2, "@ {c}");
        }
        assert_eq!(cols.grapheme_start(&s, 2_400), Some(4_800));
        assert_ascending(&cols);
    }
}

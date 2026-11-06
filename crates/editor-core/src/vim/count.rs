//! The count typed before a command, and the one typed between an operator
//! and its motion.

use crate::limits::MAX_COUNT;

/// Clamps `n` to `MAX_COUNT`.
fn ceiling(n: usize) -> usize {
    n.min(MAX_COUNT)
}

/// `pre` is the count before the command (`3` in `3dw`), `post` the count
/// after an operator (`3` in `d3w`). Both default to 1 and they multiply,
/// so `2d3w` acts on 6 words.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Count {
    pub pre: Option<usize>,
    pub post: Option<usize>,
}

impl Count {
    /// The count the command uses: both parts multiplied, 1 when neither was
    /// typed. Saturates rather than overflowing.
    pub fn effective(&self) -> usize {
        ceiling(self.pre.unwrap_or(1).saturating_mul(self.post.unwrap_or(1)))
    }

    /// True when either part was typed.
    pub fn given(&self) -> bool {
        self.pre.is_some() || self.post.is_some()
    }

    /// Appends a digit to `pre`, or to `post` when `after_operator`.
    pub fn push(&mut self, digit: usize, after_operator: bool) {
        let part = if after_operator {
            &mut self.post
        } else {
            &mut self.pre
        };
        *part = Some(ceiling(
            part.unwrap_or(0).saturating_mul(10).saturating_add(digit),
        ));
    }

    /// True when a digit has been typed for the part being entered, so a `0`
    /// extends the count instead of moving to the line start.
    pub fn pending(&self, after_operator: bool) -> bool {
        if after_operator {
            self.post.is_some()
        } else {
            self.pre.is_some()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_defaults_to_one() {
        assert_eq!(Count::default().effective(), 1);
        assert!(!Count::default().given());
    }

    #[test]
    fn effective_multiplies_pre_and_post() {
        let c = Count {
            pre: Some(2),
            post: Some(3),
        };
        assert_eq!(c.effective(), 6);
        assert_eq!(
            Count {
                pre: None,
                post: Some(4)
            }
            .effective(),
            4
        );
    }

    #[test]
    fn digits_accumulate_into_the_active_part() {
        let mut c = Count::default();
        c.push(1, false);
        c.push(2, false);
        assert_eq!(c.pre, Some(12));
        assert!(!c.pending(true));
        c.push(3, true);
        c.push(0, true);
        assert_eq!(c.post, Some(30));
        assert_eq!(c.effective(), 360);
    }

    #[cfg(fuzzing)]
    #[test]
    fn fuzz_builds_clamp_counts_to_one_thousand() {
        let mut c = Count::default();
        for _ in 0..5 {
            c.push(9, false);
        }
        assert_eq!(c.pre, Some(1_000));
        assert_eq!(c.effective(), 1_000);
        for _ in 0..5 {
            c.push(9, true);
        }
        assert_eq!(c.post, Some(1_000));
        assert_eq!(c.effective(), 1_000);
    }

    #[cfg(not(fuzzing))]
    #[test]
    fn count_clamps_at_vim_ceiling() {
        let mut c = Count::default();
        for d in [9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9] {
            c.push(d, false);
        }
        assert_eq!(c.effective(), 999_999_999);
        let c = Count {
            pre: Some(2),
            post: Some(999_999_999),
        };
        assert_eq!(c.effective(), 999_999_999);
        let c = Count {
            pre: Some(999_999_999),
            post: Some(2),
        };
        assert_eq!(c.effective(), 999_999_999);
    }
}

//! Order statistics for benchmark samples.

/// Nearest-rank percentile of an ascending slice. `p` is in 0..=1.
/// An empty slice yields NaN.
pub fn nearest_rank(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Sorted copy of the samples (NaN-safe total order).
pub fn sorted(samples: &[f64]) -> Vec<f64> {
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

pub fn median(samples: &[f64]) -> f64 {
    nearest_rank(&sorted(samples), 0.5)
}

pub fn max(samples: &[f64]) -> f64 {
    nearest_rank(&sorted(samples), 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_matches_spike() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(nearest_rank(&v, 0.5), 5.0);
        assert_eq!(nearest_rank(&v, 0.95), 10.0);
    }

    #[test]
    fn edges() {
        assert!(nearest_rank(&[], 0.5).is_nan());
        assert_eq!(nearest_rank(&[7.0], 0.0), 7.0);
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(max(&[3.0, 9.0, 2.0]), 9.0);
    }
}

//! Damage: the frame's pixel rows that changed since the shell last copied.

use std::ops::Range;

/// Sorted, disjoint row ranges. Ranges that overlap or touch merge; ranges
/// with even one row between them stay separate, so a shell copies only the
/// rows that changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Damage(pub Vec<Range<u32>>);

impl Damage {
    pub fn new() -> Damage {
        Damage::default()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Adds `r`, merging it with every range it overlaps or touches. Empty
    /// ranges are ignored.
    pub fn add(&mut self, r: Range<u32>) {
        if r.start >= r.end {
            return;
        }
        let (mut start, mut end) = (r.start, r.end);
        let mut out = Vec::with_capacity(self.0.len() + 1);
        let mut placed = false;
        for x in self.0.drain(..) {
            if x.end < start {
                out.push(x);
            } else if x.start > end {
                if !placed {
                    out.push(start..end);
                    placed = true;
                }
                out.push(x);
            } else {
                start = start.min(x.start);
                end = end.max(x.end);
            }
        }
        if !placed {
            out.push(start..end);
        }
        self.0 = out;
    }

    /// Both sets of rows, merged by the same rule as `add`.
    pub fn union(&self, other: &Damage) -> Damage {
        let mut d = self.clone();
        for r in &other.0 {
            d.add(r.clone());
        }
        d
    }

    /// Drops rows at or past `height`.
    pub fn clip(&mut self, height: u32) {
        self.0.retain_mut(|r| {
            r.end = r.end.min(height);
            r.start < r.end
        });
    }
}

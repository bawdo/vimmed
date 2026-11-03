//! The frame as horizontal strips, one texture each.
//!
//! GTK copies a whole texture whenever it changes, so the frame is cut into
//! strips `strip_px` rows tall and only the strips a damage range touches are
//! uploaded again. The others keep their texture object, which lets GSK
//! reuse what it already holds on the GPU. A change of size or scale
//! rebuilds every strip.
//!
//! The bookkeeping is generic over the texture so it is tested without GTK.

use editor_render::damage::Damage;
use editor_render::frame::Frame;

/// Strip height in device pixels unless `<P>STRIP_PX` says otherwise.
pub const DEFAULT_STRIP_PX: u32 = 128;

/// The strip height for a `<P>STRIP_PX` value (tuning only): a positive
/// integer, or the default.
pub fn strip_px(var: Option<&str>) -> u32 {
    var.and_then(|v| v.trim().parse().ok())
        .filter(|&n: &u32| n > 0)
        .unwrap_or(DEFAULT_STRIP_PX)
}

/// A texture made from a band of the frame's rows.
pub trait StripTexture {
    fn upload(frame: &Frame, top: u32, rows: u32) -> Self;
}

/// One strip: rows `top..top + rows` of the frame.
pub struct Strip<T> {
    pub top: u32,
    pub rows: u32,
    pub texture: T,
}

pub struct StripSet<T> {
    strip_px: u32,
    /// Width, height and scale bits of the frame the strips were cut from.
    geometry: Option<(u32, u32, u32)>,
    strips: Vec<Strip<T>>,
}

impl<T: StripTexture> StripSet<T> {
    pub fn new(strip_px: u32) -> StripSet<T> {
        StripSet {
            strip_px: strip_px.max(1),
            geometry: None,
            strips: Vec::new(),
        }
    }

    pub fn strips(&self) -> &[Strip<T>] {
        &self.strips
    }

    /// Brings the strips up to date with `frame`, given the rows `d` that
    /// changed since the last call. Returns the strips rebuilt.
    pub fn sync(&mut self, frame: &Frame, d: &Damage) -> Vec<usize> {
        let g = (frame.width_px, frame.height_px, frame.scale.to_bits());
        if self.geometry != Some(g) {
            self.geometry = Some(g);
            let h = frame.height_px;
            self.strips = (0..h)
                .step_by(self.strip_px as usize)
                .map(|top| {
                    let rows = self.strip_px.min(h - top);
                    Strip {
                        top,
                        rows,
                        texture: T::upload(frame, top, rows),
                    }
                })
                .collect();
            return (0..self.strips.len()).collect();
        }
        let mut rebuilt = Vec::new();
        for (i, s) in self.strips.iter_mut().enumerate() {
            let end = s.top + s.rows;
            if d.0.iter().any(|r| r.start < end && r.end > s.top) {
                s.texture = T::upload(frame, s.top, s.rows);
                rebuilt.push(i);
            }
        }
        rebuilt
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    thread_local! {
        static NEXT: Cell<u64> = const { Cell::new(0) };
    }

    /// A texture stand-in: a fresh id per upload, plus what it was cut from.
    #[derive(Debug, PartialEq)]
    struct Fake {
        id: u64,
        top: u32,
        rows: u32,
        scale: f32,
    }

    impl StripTexture for Fake {
        fn upload(frame: &Frame, top: u32, rows: u32) -> Fake {
            let id = NEXT.with(|n| {
                n.set(n.get() + 1);
                n.get()
            });
            Fake {
                id,
                top,
                rows,
                scale: frame.scale,
            }
        }
    }

    fn ids(s: &StripSet<Fake>) -> Vec<u64> {
        s.strips().iter().map(|s| s.texture.id).collect()
    }

    fn dmg(ranges: &[(u32, u32)]) -> Damage {
        let mut d = Damage::new();
        for &(a, b) in ranges {
            d.add(a..b);
        }
        d
    }

    #[test]
    fn unchanged_strips_keep_texture_identity() {
        let frame = Frame::new(100, 600, 1.0);
        let mut s = StripSet::<Fake>::new(128);
        assert_eq!(s.sync(&frame, &Damage::new()), vec![0, 1, 2, 3, 4]);
        let bands: Vec<(u32, u32)> = s.strips().iter().map(|s| (s.top, s.rows)).collect();
        assert_eq!(
            bands,
            vec![(0, 128), (128, 128), (256, 128), (384, 128), (512, 88)]
        );
        let before = ids(&s);

        assert_eq!(s.sync(&frame, &dmg(&[(130, 140)])), vec![1]);
        let after = ids(&s);
        assert_ne!(after[1], before[1]);
        for i in [0, 2, 3, 4] {
            assert_eq!(after[i], before[i], "strip {i} kept its texture");
        }
        assert_eq!(s.strips()[1].texture.top, 128);
        assert_eq!(s.strips()[1].texture.rows, 128);

        // A range across a boundary rebuilds both sides; two ranges rebuild
        // only their own strips.
        assert_eq!(s.sync(&frame, &dmg(&[(250, 260)])), vec![1, 2]);
        assert_eq!(s.sync(&frame, &dmg(&[(0, 1), (599, 600)])), vec![0, 4]);
        // Nothing changed: nothing rebuilt, no texture replaced.
        let still = ids(&s);
        assert!(s.sync(&frame, &Damage::new()).is_empty());
        assert_eq!(ids(&s), still);
        // Rows exactly at a boundary belong to the next strip only.
        assert_eq!(s.sync(&frame, &dmg(&[(256, 257)])), vec![2]);
    }

    #[test]
    fn scale_change_rebuilds_all_strips() {
        let mut s = StripSet::<Fake>::new(128);
        s.sync(&Frame::new(100, 300, 1.0), &Damage::new());
        let before = ids(&s);
        // Same pixel size at a new scale: every strip is new.
        let rescaled = Frame::new(100, 300, 1.5);
        assert_eq!(s.sync(&rescaled, &Damage::new()), vec![0, 1, 2]);
        let after = ids(&s);
        assert!(after.iter().all(|id| !before.contains(id)));
        assert!(s.strips().iter().all(|s| s.texture.scale == 1.5));
        // A new size does the same, and the strips follow the new height.
        assert_eq!(
            s.sync(&Frame::new(100, 130, 1.5), &dmg(&[(0, 1)])),
            vec![0, 1]
        );
        assert_eq!(s.strips()[1].rows, 2);
    }

    #[test]
    fn strip_height_override_is_for_tuning_only() {
        assert_eq!(strip_px(None), DEFAULT_STRIP_PX);
        assert_eq!(strip_px(Some("64")), 64);
        assert_eq!(strip_px(Some("0")), DEFAULT_STRIP_PX);
        assert_eq!(strip_px(Some("lots")), DEFAULT_STRIP_PX);
    }
}

//! Double-buffered presentation.
//!
//! Two surfaces take turns as the layer's contents. Each one is a frame
//! stale when it comes back as the back buffer, so staging copies this
//! frame's damage plus whatever the previous presented frame changed.

use editor_render::damage::Damage;
use editor_render::frame::{Frame, copy_rows_bgra};

/// A BGRA pixel buffer the presenter can write into.
pub trait Surface {
    /// Calls `f` with the surface's bytes and its row stride in bytes.
    fn with_bytes(&mut self, f: &mut dyn FnMut(&mut [u8], usize));
}

/// The surface bookkeeping, generic so it is tested without a window.
pub struct Presenter<S: Surface> {
    surfaces: [S; 2],
    back: usize,
    /// Damage staged into the back surface since the last commit.
    staged: Damage,
    /// What the previous commit changed: the back surface lacks it.
    prev: Damage,
    dirty: bool,
}

impl<S: Surface> Presenter<S> {
    pub fn with_surfaces(surfaces: [S; 2]) -> Presenter<S> {
        Presenter {
            surfaces,
            back: 0,
            staged: Damage::new(),
            prev: Damage::new(),
            dirty: false,
        }
    }

    /// Copies `d ∪ prev` into the back surface, where `prev` is what the
    /// last commit changed (the back surface was last written a frame
    /// before that).
    pub fn stage(&mut self, frame: &Frame, d: &Damage) {
        let rows = d.union(&self.prev);
        if rows.is_empty() {
            return;
        }
        self.surfaces[self.back]
            .with_bytes(&mut |buf, stride| copy_rows_bgra(frame, buf, stride, &rows));
        self.staged = self.staged.union(d);
        self.dirty = true;
    }

    /// True when something is staged and not yet committed.
    pub fn has_staged(&self) -> bool {
        self.dirty
    }

    /// Hands back the surface to show and makes the other one the back.
    pub fn swap(&mut self) -> Option<&S> {
        if !self.dirty {
            return None;
        }
        let shown = self.back;
        self.back ^= 1;
        self.prev = std::mem::take(&mut self.staged);
        self.dirty = false;
        Some(&self.surfaces[shown])
    }
}

#[cfg(target_os = "macos")]
pub use mac::IoSurface;

#[cfg(target_os = "macos")]
mod mac {
    use objc2::runtime::AnyObject;
    use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString};
    use objc2_core_graphics::kCGColorSpaceSRGB;
    use objc2_io_surface::{
        IOSurfaceLockOptions, IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceColorSpace,
        kIOSurfaceHeight, kIOSurfacePixelFormat, kIOSurfaceWidth,
    };
    use objc2_quartz_core::{CALayer, CATransaction};

    use super::{Presenter, Surface};

    /// A BGRA IOSurface tagged sRGB.
    pub struct IoSurface(CFRetained<IOSurfaceRef>);

    impl IoSurface {
        pub fn new(w: u32, h: u32) -> IoSurface {
            let n = |v: i32| CFNumber::new_i32(v);
            let (nw, nh, nb) = (n(w.max(1) as i32), n(h.max(1) as i32), n(4));
            let nf = n(i32::from_be_bytes(*b"BGRA"));
            // SAFETY: reading immutable framework constants.
            let keys: [&CFString; 4] = unsafe {
                [
                    kIOSurfaceWidth,
                    kIOSurfaceHeight,
                    kIOSurfaceBytesPerElement,
                    kIOSurfacePixelFormat,
                ]
            };
            let vals: [&CFNumber; 4] = [&nw, &nh, &nb, &nf];
            let props = CFDictionary::from_slices(&keys, &vals);
            // SAFETY: `props` holds the documented keys with CFNumber values.
            let surf = unsafe { IOSurfaceRef::new(props.as_opaque()) }.expect("IOSurface");
            // SAFETY: the colour space key takes a colour space name string.
            unsafe { surf.set_value(kIOSurfaceColorSpace, kCGColorSpaceSRGB) };
            IoSurface(surf)
        }
    }

    impl Surface for IoSurface {
        fn with_bytes(&mut self, f: &mut dyn FnMut(&mut [u8], usize)) {
            let s = &self.0;
            let none = IOSurfaceLockOptions(0);
            // SAFETY: the surface is locked for the whole time its memory is
            // borrowed, and the slice covers exactly `bytes_per_row * height`
            // bytes of its single plane.
            unsafe {
                s.lock(none, std::ptr::null_mut());
                let len = s.bytes_per_row() * s.height();
                let bytes = std::slice::from_raw_parts_mut(s.base_address().as_ptr().cast(), len);
                f(bytes, s.bytes_per_row());
                s.unlock(none, std::ptr::null_mut());
            }
        }
    }

    impl Presenter<IoSurface> {
        /// Two BGRA sRGB IOSurfaces of `w_px` by `h_px`.
        pub fn new(w_px: u32, h_px: u32) -> Presenter<IoSurface> {
            Presenter::with_surfaces([IoSurface::new(w_px, h_px), IoSurface::new(w_px, h_px)])
        }

        /// Makes the staged surface the layer's contents in a transaction
        /// with actions disabled. Returns false when nothing was staged.
        pub fn commit(&mut self, layer: &CALayer) -> bool {
            let Some(s) = self.swap() else {
                return false;
            };
            let obj: &AnyObject = s.0.as_ref();
            CATransaction::begin();
            CATransaction::setDisableActions(true);
            // SAFETY: an IOSurface is a documented type for `contents`.
            unsafe { layer.setContents(Some(obj)) };
            CATransaction::commit();
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 2;
    const H: u32 = 6;

    struct Fake {
        bytes: Vec<u8>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                bytes: vec![0; (W * H * 4) as usize],
            }
        }
        /// The rows whose first pixel's blue byte is `v`.
        fn rows_with(&self, v: u8) -> Vec<u32> {
            (0..H)
                .filter(|y| self.bytes[(y * W * 4) as usize] == v)
                .collect()
        }
    }

    impl Surface for Fake {
        fn with_bytes(&mut self, f: &mut dyn FnMut(&mut [u8], usize)) {
            f(&mut self.bytes, (W * 4) as usize);
        }
    }

    /// A frame whose every pixel is RGBA `(0, 0, v, 255)`, so BGRA byte 0
    /// holds `v`.
    fn frame(v: u8) -> Frame {
        let mut f = Frame::new(W, H, 1.0);
        for px in f.pixmap.data_mut().as_chunks_mut::<4>().0 {
            *px = [0, 0, v, 255];
        }
        f
    }

    fn dmg(r: std::ops::Range<u32>) -> Damage {
        let mut d = Damage::new();
        d.add(r);
        d
    }

    #[test]
    fn stage_copies_union_of_this_and_previous_damage() {
        let mut p = Presenter::with_surfaces([Fake::new(), Fake::new()]);
        // Frame 1 changes rows 0..2 and goes to surface 0.
        p.stage(&frame(1), &dmg(0..2));
        assert!(p.swap().is_some());
        // Frame 2 changes rows 4..5 and goes to surface 1, which never saw
        // frame 1: it must get rows 0..2 as well.
        p.stage(&frame(2), &dmg(4..5));
        let shown = p.swap().expect("staged");
        assert_eq!(shown.rows_with(2), vec![0, 1, 4]);
        assert_eq!(shown.rows_with(0), vec![2, 3, 5]);
    }

    #[test]
    fn stages_between_commits_all_reach_the_other_surface() {
        let mut p = Presenter::with_surfaces([Fake::new(), Fake::new()]);
        // Two keys land before one tick: both go to surface 0.
        p.stage(&frame(1), &dmg(0..1));
        p.stage(&frame(1), &dmg(3..4));
        assert!(p.swap().is_some());
        // Surface 1 must catch up on both.
        p.stage(&frame(2), &dmg(5..6));
        let shown = p.swap().expect("staged");
        assert_eq!(shown.rows_with(2), vec![0, 3, 5]);
    }

    #[test]
    fn swap_without_staging_is_none() {
        let mut p = Presenter::with_surfaces([Fake::new(), Fake::new()]);
        assert!(p.swap().is_none());
        assert!(!p.has_staged());
        p.stage(&frame(1), &dmg(0..1));
        assert!(p.has_staged());
    }
}

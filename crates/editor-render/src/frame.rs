//! The frame: a premultiplied RGBA pixmap at device pixels, and the copy of
//! damaged rows into a shell's BGRA buffer.

use tiny_skia::Pixmap;

use crate::damage::Damage;

/// The painted frame. `pixmap` is premultiplied RGBA, `width_px` by
/// `height_px` (at least one pixel each way), at `scale` device pixels per
/// point.
pub struct Frame {
    pub width_px: u32,
    pub height_px: u32,
    pub scale: f32,
    pub pixmap: Pixmap,
}

impl Frame {
    /// A transparent frame. A zero size is raised to one pixel, because a
    /// pixmap can't be empty.
    pub fn new(width_px: u32, height_px: u32, scale: f32) -> Frame {
        let (w, h) = (width_px.max(1), height_px.max(1));
        let pixmap = Pixmap::new(w, h).expect("a pixmap of at least one pixel");
        Frame {
            width_px: w,
            height_px: h,
            scale,
            pixmap,
        }
    }
}

/// Copies the damaged rows of `src` into `dst` as premultiplied BGRA (the
/// native order for CoreAnimation, cairo and wl_shm ARGB8888), `dst_stride`
/// bytes per row. Rows outside the damage are left alone.
///
/// A destination from before a resize may be smaller than the frame: rows
/// and columns that don't fit are skipped rather than written out of bounds.
pub fn copy_rows_bgra(src: &Frame, dst: &mut [u8], dst_stride: usize, d: &Damage) {
    let w = src.pixmap.width() as usize;
    let h = src.pixmap.height() as usize;
    let data = src.pixmap.data();
    let row_bytes = (w * 4).min(dst_stride / 4 * 4);
    if row_bytes == 0 {
        return;
    }
    let dst_rows =
        dst.len().saturating_sub(row_bytes) / dst_stride + usize::from(dst.len() >= row_bytes);
    let rows = h.min(dst_rows);
    for r in &d.0 {
        for y in (r.start as usize).min(rows)..(r.end as usize).min(rows) {
            let s_row = &data[y * w * 4..y * w * 4 + row_bytes];
            let d_row = &mut dst[y * dst_stride..y * dst_stride + row_bytes];
            for (d, s) in d_row
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(s_row.as_chunks::<4>().0)
            {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
            }
        }
    }
}

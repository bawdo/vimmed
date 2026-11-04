//! The text layout interface. One source line is one layout object. The
//! interface is library-neutral: `layout_cosmic` implements it over
//! cosmic-text, and caret and hit geometry live in `LaidLine` itself.

use std::any::Any;
use std::ops::Range;
use std::sync::Arc;

use editor_core::tokens::Rgba;

/// Which face a span uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontRole {
    Body,
    Mono,
}

/// A run of a line's display text in one style. `range` is in display bytes;
/// sizes are device pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledSpan {
    pub range: Range<usize>,
    pub role: FontRole,
    pub size_px: f32,
    pub weight: u16,
    pub italic: bool,
    pub color: Rgba,
    pub strike: bool,
    /// A single underline in the span's colour (links).
    pub underline: bool,
}

/// A caret position within a laid line, in pixels from the line's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caret {
    pub x: f32,
    pub top: f32,
    pub height: f32,
}

/// A glyph's coverage, ready to blend.
#[derive(Debug, Clone)]
pub enum MaskData {
    /// One coverage byte per pixel, tinted with the mask's colour.
    Alpha(Arc<[u8]>),
    /// Straight-alpha RGBA per pixel (colour glyphs such as emoji).
    Color(Arc<[u8]>),
    /// Full coverage over the whole rectangle (decoration lines).
    Solid,
}

/// One glyph (or decoration) to blend at `x, y` (its top left, in pixels from
/// the line's top left), `w` by `h` pixels.
#[derive(Debug, Clone)]
pub struct GlyphMask {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub color: Rgba,
    pub data: MaskData,
}

/// A rectangle in pixels from the line's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One cluster's place on a row: its display bytes and horizontal extent.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphGeom {
    pub bytes: Range<usize>,
    pub x: f32,
    pub w: f32,
}

/// One visual row of a laid line.
#[derive(Debug, Clone, PartialEq)]
pub struct RowGeom {
    pub top: f32,
    pub height: f32,
    pub width: f32,
    /// The display bytes on this row.
    pub text: Range<usize>,
    /// Clusters in visual order.
    pub glyphs: Vec<GlyphGeom>,
}

/// A laid-out source line: `height` pixels over `rows` visual rows. The
/// implementation that made it keeps its own state in `inner`.
pub struct LaidLine {
    pub height: f32,
    pub rows: u32,
    geom: Vec<RowGeom>,
    inner: Option<Box<dyn Any>>,
}

impl LaidLine {
    /// A line from its rows. Row ranges must cover the text in order.
    pub fn new(geom: Vec<RowGeom>, inner: Option<Box<dyn Any>>) -> LaidLine {
        LaidLine {
            height: geom.iter().map(|r| r.height).sum(),
            rows: geom.len() as u32,
            geom,
            inner,
        }
    }

    pub fn row_geometry(&self) -> &[RowGeom] {
        &self.geom
    }

    /// The implementation's own state, if it is a `T`.
    pub fn inner<T: 'static>(&self) -> Option<&T> {
        self.inner.as_ref()?.downcast_ref()
    }

    /// The widest row's width.
    pub fn width(&self) -> f32 {
        self.geom.iter().map(|r| r.width).fold(0.0, f32::max)
    }

    /// The row holding display byte `byte`. A byte at a wrap point belongs to
    /// the row it starts; a byte past the end belongs to the last row.
    fn row_of(&self, byte: usize) -> Option<&RowGeom> {
        let last = self.geom.len().checked_sub(1)?;
        self.geom
            .iter()
            .enumerate()
            .find(|(k, r)| *k == last || byte < r.text.end)
            .map(|(_, r)| r)
    }

    /// Where a caret before display byte `byte` sits.
    pub fn caret(&self, byte: usize) -> Caret {
        let Some(row) = self.row_of(byte) else {
            return Caret {
                x: 0.0,
                top: 0.0,
                height: self.height,
            };
        };
        let x = row
            .glyphs
            .iter()
            .find(|g| g.bytes.start <= byte && byte < g.bytes.end)
            .map(|g| g.x)
            .or_else(|| {
                // Before the first cluster: its left edge. Past every cluster: the row's end.
                let first = row.glyphs.first()?;
                if byte < first.bytes.start {
                    Some(first.x)
                } else {
                    row.glyphs.iter().map(|g| g.x + g.w).reduce(f32::max)
                }
            })
            .unwrap_or(0.0);
        Caret {
            x,
            top: row.top,
            height: row.height,
        }
    }

    /// The display byte nearest the point `x, y` (pixels from the line's top
    /// left): a cluster's start when the point is on its left half, its end
    /// when on its right half.
    pub fn hit(&self, x: f32, y: f32) -> usize {
        let Some(row) = self
            .geom
            .iter()
            .find(|r| y < r.top + r.height)
            .or(self.geom.last())
        else {
            return 0;
        };
        for g in &row.glyphs {
            if x < g.x + g.w / 2.0 {
                return g.bytes.start;
            }
            if x < g.x + g.w {
                return g.bytes.end;
            }
        }
        row.glyphs
            .iter()
            .map(|g| g.bytes.end)
            .max()
            .unwrap_or(row.text.start)
            .max(row.text.start)
    }

    /// One rectangle per row covering the clusters that overlap display
    /// bytes `r`. Empty ranges give nothing.
    pub fn range_rects(&self, r: Range<usize>) -> Vec<RectF> {
        if r.start >= r.end {
            return Vec::new();
        }
        self.geom
            .iter()
            .filter_map(|row| {
                let (x0, x1) = row
                    .glyphs
                    .iter()
                    .filter(|g| g.bytes.start < r.end && r.start < g.bytes.end)
                    .fold(None, |acc: Option<(f32, f32)>, g| {
                        let (a, b) = acc.unwrap_or((g.x, g.x + g.w));
                        Some((a.min(g.x), b.max(g.x + g.w)))
                    })?;
                Some(RectF {
                    x: x0,
                    y: row.top,
                    w: x1 - x0,
                    h: row.height,
                })
            })
            .collect()
    }
}

/// Lays text out and rasterises it.
pub trait TextLayout {
    /// Lays out one line's display text, wrapped to `width_px`. `spans` cover
    /// the text in order; give at least one (an empty one for an empty line),
    /// because the first span sets the line's metrics.
    fn layout_line(&mut self, text: &str, spans: &[StyledSpan], width_px: f32) -> LaidLine;

    /// Hands every glyph mask of `line` to `sink`.
    fn draw(&mut self, line: &LaidLine, sink: &mut dyn FnMut(GlyphMask));

    /// Called once after each painted frame. The cosmic-text implementation
    /// trims its shape-run cache here; other implementations may do nothing.
    fn end_frame(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(top: f32, text: Range<usize>, xs: &[(Range<usize>, f32, f32)]) -> RowGeom {
        RowGeom {
            top,
            height: 10.0,
            width: xs.iter().map(|g| g.1 + g.2).fold(0.0, f32::max),
            text,
            glyphs: xs
                .iter()
                .map(|(b, x, w)| GlyphGeom {
                    bytes: b.clone(),
                    x: *x,
                    w: *w,
                })
                .collect(),
        }
    }

    /// "ab cd" wrapped after the space: "ab " on row 0, "cd" on row 1.
    fn wrapped() -> LaidLine {
        LaidLine::new(
            vec![
                row(
                    0.0,
                    0..3,
                    &[(0..1, 0.0, 5.0), (1..2, 5.0, 5.0), (2..3, 10.0, 3.0)],
                ),
                row(10.0, 3..5, &[(3..4, 0.0, 5.0), (4..5, 5.0, 5.0)]),
            ],
            None,
        )
    }

    #[test]
    fn height_and_rows_sum_the_rows() {
        let l = wrapped();
        assert_eq!((l.height, l.rows), (20.0, 2));
        assert_eq!(l.width(), 13.0);
    }

    #[test]
    fn caret_at_a_wrap_point_starts_the_next_row() {
        let l = wrapped();
        assert_eq!(l.caret(1).x, 5.0);
        let c = l.caret(3);
        assert_eq!((c.x, c.top), (0.0, 10.0));
        let end = l.caret(5);
        assert_eq!((end.x, end.top), (10.0, 10.0));
    }

    #[test]
    fn hit_picks_the_nearer_edge() {
        let l = wrapped();
        assert_eq!(l.hit(1.0, 1.0), 0);
        assert_eq!(l.hit(4.0, 1.0), 1);
        assert_eq!(l.hit(6.0, 12.0), 4);
        assert_eq!(l.hit(99.0, 12.0), 5);
        assert_eq!(l.hit(99.0, 99.0), 5);
    }

    #[test]
    fn range_rects_split_by_row() {
        let l = wrapped();
        let r = l.range_rects(1..4);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].x, r[0].w, r[0].y), (5.0, 8.0, 0.0));
        assert_eq!((r[1].x, r[1].w, r[1].y), (0.0, 5.0, 10.0));
        assert!(l.range_rects(2..2).is_empty());
    }

    #[test]
    fn an_empty_line_carets_at_zero() {
        let l = LaidLine::new(vec![row(0.0, 0..0, &[])], None);
        assert_eq!(l.caret(0).x, 0.0);
        assert_eq!(l.hit(50.0, 5.0), 0);
    }
}

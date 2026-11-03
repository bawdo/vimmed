//! `TextLayout` over cosmic-text 0.19, used the way `crates/layout-bench`
//! measured it (Task 7 verdict): one `Buffer` per source line,
//! `Wrap::WordOrGlyph`, `Shaping::Advanced`, fonts loaded by path, and the
//! `shape-run-cache` trimmed once per painted frame.

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, CacheKey, Color, Family, Metrics, Shaping, Style, SwashCache, SwashContent,
    Weight, Wrap,
};
use editor_core::tokens::{Rgba, TokenId, Tokens};

use crate::fonts::FontSet;
use crate::layout::{
    FontRole, GlyphGeom, GlyphMask, LaidLine, MaskData, RowGeom, StyledSpan, TextLayout,
};

/// How many frames an unused shaped run survives: runs used in the current or
/// the previous frame are kept (the benchmarked setting). `DocView` reuses an
/// unchanged line's layout without reshaping it, so the cache only has to
/// carry runs from one frame to the next on lines that really changed.
pub const RUN_CACHE_KEEP_AGES: u64 = 1;

/// A rasterised glyph, shared between draws.
struct Raster {
    left: i32,
    top: i32,
    w: u32,
    h: u32,
    data: MaskData,
}

/// What a `LaidLine` made here carries: the line's buffer and the colour of
/// each span (glyphs find theirs through their attrs' metadata).
struct CosmicLine {
    buffer: Buffer,
    colors: Vec<Rgba>,
}

pub struct CosmicLayout {
    fonts: FontSet,
    swash: SwashCache,
    rasters: HashMap<CacheKey, Option<Raster>>,
    line_height_body: f32,
    line_height_code: f32,
}

impl CosmicLayout {
    /// Line heights come from `line.height.body` (body spans) and
    /// `line.height.code` (mono spans).
    pub fn new(fonts: FontSet, tokens: &Tokens) -> CosmicLayout {
        CosmicLayout {
            fonts,
            swash: SwashCache::new(),
            rasters: HashMap::new(),
            line_height_body: tokens.ratio(TokenId::LineHeightBody),
            line_height_code: tokens.ratio(TokenId::LineHeightCode),
        }
    }

    fn metrics(&self, s: &StyledSpan) -> Metrics {
        let ratio = match s.role {
            FontRole::Body => self.line_height_body,
            FontRole::Mono => self.line_height_code,
        };
        Metrics::new(s.size_px, s.size_px * ratio)
    }

    fn attrs<'a>(&'a self, s: &StyledSpan, index: usize) -> Attrs<'a> {
        let family = match (s.role, &self.fonts.sans, &self.fonts.mono) {
            (FontRole::Body, Some(name), _) | (FontRole::Mono, _, Some(name)) => Family::Name(name),
            (FontRole::Body, None, _) => Family::SansSerif,
            (FontRole::Mono, _, None) => Family::Monospace,
        };
        let mut a = Attrs::new()
            .family(family)
            .weight(Weight(s.weight))
            .style(if s.italic {
                Style::Italic
            } else {
                Style::Normal
            })
            .color(Color::rgba(s.color.r, s.color.g, s.color.b, s.color.a))
            .metadata(index)
            .metrics(self.metrics(s));
        a.text_decoration.strikethrough = s.strike;
        a
    }

    fn raster(&mut self, key: CacheKey) -> Option<&Raster> {
        let Self {
            fonts,
            swash,
            rasters,
            ..
        } = self;
        rasters
            .entry(key)
            .or_insert_with(|| {
                let img = swash.get_image_uncached(&mut fonts.system, key)?;
                let data = match img.content {
                    SwashContent::Mask => MaskData::Alpha(Arc::from(img.data)),
                    SwashContent::Color => MaskData::Color(Arc::from(img.data)),
                    // Not produced by the default rendering; treat as no glyph.
                    SwashContent::SubpixelMask => return None,
                };
                Some(Raster {
                    left: img.placement.left,
                    top: img.placement.top,
                    w: img.placement.width,
                    h: img.placement.height,
                    data,
                })
            })
            .as_ref()
    }
}

/// `spans` as (text, span index) pieces covering all of `text`: gaps take the
/// span before them (or the first span), and ranges are clamped to the text.
fn pieces(text: &str, spans: &[StyledSpan]) -> Vec<(usize, std::ops::Range<usize>)> {
    let mut out = Vec::with_capacity(spans.len() + 1);
    let mut at = 0;
    for (i, s) in spans.iter().enumerate() {
        let start = s.range.start.clamp(at, text.len());
        let end = s.range.end.clamp(start, text.len());
        if start > at {
            out.push((i.saturating_sub(1), at..start));
        }
        if end > start {
            out.push((i, start..end));
        }
        at = end;
    }
    if at < text.len() {
        out.push((spans.len().saturating_sub(1), at..text.len()));
    }
    out.retain(|(_, r)| text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
    out
}

impl TextLayout for CosmicLayout {
    fn layout_line(&mut self, text: &str, spans: &[StyledSpan], width_px: f32) -> LaidLine {
        let Some(first) = spans.first() else {
            return LaidLine::new(Vec::new(), None);
        };
        let mut buffer = Buffer::new_empty(self.metrics(first));
        buffer.set_size(Some(width_px.max(1.0)), None);
        buffer.set_wrap(Wrap::WordOrGlyph);
        let default = self.attrs(first, 0);
        let rich: Vec<(&str, Attrs)> = pieces(text, spans)
            .into_iter()
            .map(|(i, r)| (&text[r], self.attrs(&spans[i], i)))
            .collect();
        buffer.set_rich_text(rich, &default, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts.system, false);

        let mut geom: Vec<RowGeom> = Vec::new();
        for run in buffer.layout_runs() {
            let glyphs: Vec<GlyphGeom> = run
                .glyphs
                .iter()
                .map(|g| GlyphGeom {
                    bytes: g.start..g.end,
                    x: g.x,
                    w: g.w,
                })
                .collect();
            let prev_end = geom.last().map_or(0, |r| r.text.end);
            let start = glyphs
                .iter()
                .map(|g| g.bytes.start)
                .min()
                .unwrap_or(prev_end);
            let end = glyphs.iter().map(|g| g.bytes.end).max().unwrap_or(start);
            if let Some(prev) = geom.last_mut() {
                prev.text.end = start.max(prev.text.start);
            }
            geom.push(RowGeom {
                top: run.line_top,
                height: run.line_height,
                width: run.line_w,
                text: start.max(prev_end)..end.max(prev_end),
                glyphs,
            });
        }
        if let Some(last) = geom.last_mut() {
            last.text.end = text.len().max(last.text.start);
        }
        let colors = spans.iter().map(|s| s.color).collect();
        LaidLine::new(geom, Some(Box::new(CosmicLine { buffer, colors })))
    }

    fn draw(&mut self, line: &LaidLine, sink: &mut dyn FnMut(GlyphMask)) {
        let Some(cl) = line.inner::<CosmicLine>() else {
            return;
        };
        let color_of = |i: usize| cl.colors.get(i).or(cl.colors.first()).copied();
        for run in cl.buffer.layout_runs() {
            for g in run.glyphs {
                let Some(color) = color_of(g.metadata) else {
                    continue;
                };
                let p = g.physical((0.0, run.line_y), 1.0);
                if let Some(r) = self.raster(p.cache_key) {
                    sink(GlyphMask {
                        x: p.x + r.left,
                        y: p.y - r.top,
                        w: r.w,
                        h: r.h,
                        color,
                        data: r.data.clone(),
                    });
                }
            }
            for d in run.decorations {
                if !d.data.text_decoration.strikethrough {
                    continue;
                }
                let glyphs = &run.glyphs[d.glyph_range.clone()];
                let Some(first) = glyphs.first() else {
                    continue;
                };
                let Some(color) = color_of(first.metadata) else {
                    continue;
                };
                let x0 = glyphs.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
                let x1 = glyphs
                    .iter()
                    .map(|g| g.x + g.w)
                    .fold(f32::NEG_INFINITY, f32::max);
                // The font's own strikeout metrics, in ems, at the span's size.
                let m = d.data.strikethrough_metrics;
                let thickness = (m.thickness * d.font_size).max(1.0).ceil();
                let y = run.line_y - m.offset * d.font_size;
                if x1 > x0 {
                    sink(GlyphMask {
                        x: x0 as i32,
                        y: y as i32,
                        w: (x1 - x0).ceil() as u32,
                        h: thickness as u32,
                        color,
                        data: MaskData::Solid,
                    });
                }
            }
        }
    }

    fn end_frame(&mut self) {
        self.fonts.system.shape_run_cache.trim(RUN_CACHE_KEEP_AGES);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(range: std::ops::Range<usize>) -> StyledSpan {
        let t = Tokens::defaults(editor_core::tokens::Theme::Light);
        StyledSpan {
            range,
            role: FontRole::Body,
            size_px: 16.0,
            weight: 400,
            italic: false,
            color: t.color(TokenId::ColorText),
            strike: false,
        }
    }

    #[test]
    fn pieces_cover_the_text_and_fill_gaps() {
        let p = pieces("abcdef", &[span(0..2), span(3..4)]);
        assert_eq!(p, vec![(0, 0..2), (0, 2..3), (1, 3..4), (1, 4..6)]);
        assert_eq!(pieces("", &[span(0..0)]), vec![]);
        assert_eq!(pieces("ab", &[span(0..9)]), vec![(0, 0..2)]);
    }
}

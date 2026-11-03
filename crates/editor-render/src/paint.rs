//! Line painting: mapping a line's display model to styled spans (every size,
//! weight and colour from tokens), filling rectangles through tiny-skia and
//! blending glyph masks into the frame.

use std::ops::Range;

use editor_core::markdown::{LineDisplay, Prefix, SpanBase, SpanStyle};
use editor_core::tokens::{Rgba, TokenId, Tokens};
use tiny_skia::{FilterQuality, Paint, Pattern, Pixmap, Rect, SpreadMode, Transform};

use crate::layout::{FontRole, GlyphMask, MaskData, StyledSpan};

/// The heading size token for level `n` (levels past 6 use h6).
fn heading_size(n: u8) -> TokenId {
    match n {
        0 | 1 => TokenId::FontSizeH1,
        2 => TokenId::FontSizeH2,
        3 => TokenId::FontSizeH3,
        4 => TokenId::FontSizeH4,
        5 => TokenId::FontSizeH5,
        _ => TokenId::FontSizeH6,
    }
}

/// Copies the premultiplied `src` onto `p` at whole pixels `x, y` (source-over),
/// only inside the rows `clip` and the pixmap. `opaque` says every source pixel
/// has full alpha, which makes each row a plain memory copy.
pub fn blit(p: &mut Pixmap, x: i32, y: i32, src: &Pixmap, opaque: bool, clip: &Range<f32>) {
    let (pw, ph) = (p.width() as i32, p.height() as i32);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let ys = y.max(clip.start.ceil() as i32).max(0)..(y + sh).min(clip.end as i32).min(ph);
    let xs = x.max(0)..(x + sw).min(pw);
    if ys.is_empty() || xs.is_empty() {
        return;
    }
    let n = (xs.end - xs.start) as usize;
    let (src_data, dst) = (src.data(), p.data_mut());
    for yy in ys {
        let s0 = (((yy - y) * sw + (xs.start - x)) * 4) as usize;
        let d0 = ((yy * pw + xs.start) * 4) as usize;
        let s_row = &src_data[s0..s0 + n * 4];
        let d_row = &mut dst[d0..d0 + n * 4];
        if opaque {
            d_row.copy_from_slice(s_row);
            continue;
        }
        for (d, s) in d_row
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(s_row.as_chunks::<4>().0)
        {
            let inv = 255 - u16::from(s[3]);
            for k in 0..4 {
                let v = u16::from(s[k]) + (u16::from(d[k]) * inv + 127) / 255;
                d[k] = v.min(255) as u8;
            }
        }
    }
}

/// Draws `src` scaled to the `w` by `h` pixel box at `x, y`, only inside the
/// rows `clip`. The source is premultiplied, so transparency blends correctly.
pub fn draw_scaled(
    p: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    src: &Pixmap,
    clip: &Range<f32>,
) {
    let (y0, y1) = (y.max(clip.start), (y + h).min(clip.end));
    let (x0, x1) = (x.max(0.0), (x + w).min(p.width() as f32));
    if y1 <= y0 || x1 <= x0 || src.width() == 0 || src.height() == 0 {
        return;
    }
    let Some(rect) = Rect::from_ltrb(x0, y0, x1, y1) else {
        return;
    };
    // Fill just the clipped rect with the image as a shader: no allocation
    // proportional to the frame, and only the rect's pixels are sampled.
    let (sx, sy) = (w / src.width() as f32, h / src.height() as f32);
    // A whole-pixel copy needs no filtering.
    let copy = sx == 1.0 && sy == 1.0 && x.fract() == 0.0 && y.fract() == 0.0;
    let quality = if copy {
        FilterQuality::Nearest
    } else {
        FilterQuality::Bilinear
    };
    let paint = Paint {
        shader: Pattern::new(
            src.as_ref(),
            SpreadMode::Pad,
            quality,
            1.0,
            Transform::from_row(sx, 0.0, 0.0, sy, x, y),
        ),
        anti_alias: false,
        ..Paint::default()
    };
    p.fill_rect(rect, &paint, Transform::identity(), None);
}

/// The styled span for `style` over display bytes `range`, at `scale`.
///
/// Code, code blocks and revealed syntax are mono; headings take their level's
/// size and the heading weight; `strong` takes the strong weight; `em` is
/// italic; revealed syntax takes `color.syntax.reveal` and links `color.link`.
pub fn span_for(style: &SpanStyle, range: Range<usize>, t: &Tokens, scale: f32) -> StyledSpan {
    let role = match style.base {
        SpanBase::Code | SpanBase::CodeBlock { .. } | SpanBase::Syntax => FontRole::Mono,
        SpanBase::Body | SpanBase::Heading(_) => FontRole::Body,
    };
    let size = match style.base {
        SpanBase::Heading(n) => t.px(heading_size(n)),
        _ => t.px(TokenId::FontSize),
    };
    let weight = if style.strong {
        TokenId::FontWeightStrong
    } else if matches!(style.base, SpanBase::Heading(_)) {
        TokenId::FontWeightHeading
    } else {
        TokenId::FontWeightBody
    };
    let color = if style.base == SpanBase::Syntax {
        TokenId::ColorSyntaxReveal
    } else if style.link {
        TokenId::ColorLink
    } else {
        TokenId::ColorText
    };
    StyledSpan {
        range,
        role,
        size_px: size * scale,
        weight: t.weight(weight),
        italic: style.em,
        color: t.color(color),
        strike: style.strike,
    }
}

/// A line's display text and its styled spans. An empty display still gets
/// one empty span in the line's base style, so it keeps its line height.
pub fn styled_line(
    src: &str,
    d: &LineDisplay,
    t: &Tokens,
    scale: f32,
) -> (String, Vec<StyledSpan>) {
    let mut text = String::new();
    let mut spans = Vec::with_capacity(d.spans.len().max(1));
    for s in &d.spans {
        let piece = src.get(s.src.clone()).unwrap_or_default();
        let start = text.len();
        text.push_str(piece);
        spans.push(span_for(&s.style, start..text.len(), t, scale));
    }
    if spans.is_empty() {
        let base = match &d.deco {
            editor_core::markdown::BlockDeco::CodeBg { .. } => {
                SpanStyle::plain(SpanBase::CodeBlock { lang: None })
            }
            _ => SpanStyle::plain(SpanBase::Body),
        };
        spans.push(span_for(&base, 0..0, t, scale));
    }
    (text, spans)
}

/// The text drawn for a hidden list prefix, if any.
pub fn prefix_text(p: &Prefix) -> Option<String> {
    match p {
        Prefix::Bullet(c) => Some(c.to_string()),
        Prefix::Number(n) => Some(n.clone()),
        Prefix::Task(done) => Some(if *done { '\u{2611}' } else { '\u{2610}' }.to_string()),
        Prefix::None | Prefix::QuoteBars(_) => None,
    }
}

/// How far a line's text sits right of the column's left edge, in pixels.
pub fn content_offset(p: &Prefix, t: &Tokens, scale: f32) -> f32 {
    match p {
        Prefix::None => 0.0,
        Prefix::Bullet(_) | Prefix::Number(_) | Prefix::Task(_) => {
            t.px(TokenId::SpaceListIndent) * scale
        }
        Prefix::QuoteBars(n) => {
            f32::from(*n) * (t.px(TokenId::SizeQuoteBar) + t.px(TokenId::SpaceQuoteGap)) * scale
        }
    }
}

/// Source byte `col` as a display byte. A byte inside hidden syntax maps to
/// the display boundary where the syntax was removed.
pub fn src_to_display_clamped(d: &LineDisplay, col: usize) -> usize {
    let mut at = 0;
    for s in &d.spans {
        if col < s.src.start {
            return at;
        }
        if col <= s.src.end {
            return at + (col - s.src.start);
        }
        at += s.src.len();
    }
    at
}

/// Fills `x, y, w, h` (pixels) with `c`, limited to rows `clip`. Edges are
/// not anti-aliased, so a fill never strays outside the rows it names.
pub fn fill(p: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Rgba, clip: &Range<f32>) {
    let (y0, y1) = (y.max(clip.start), (y + h).min(clip.end));
    let (x0, x1) = (x.max(0.0), (x + w).min(p.width() as f32));
    if y1 <= y0 || x1 <= x0 {
        return;
    }
    let Some(rect) = Rect::from_ltrb(x0, y0, x1, y1) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(c.r, c.g, c.b, c.a);
    paint.anti_alias = false;
    p.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Blends `m` (placed at `ox, oy` plus its own offset) over the frame,
/// source-over in premultiplied RGBA, limited to rows `clip`.
pub fn blend(p: &mut Pixmap, ox: i32, oy: i32, m: &GlyphMask, clip: &Range<u32>) {
    let (pw, ph) = (p.width() as i32, p.height() as i32);
    let (x, y) = (ox + m.x, oy + m.y);
    let ys = y.max(clip.start as i32).max(0)..(y + m.h as i32).min(clip.end as i32).min(ph);
    let xs = x.max(0)..(x + m.w as i32).min(pw);
    if ys.is_empty() || xs.is_empty() {
        return;
    }
    let c = m.color;
    let data = p.data_mut();
    for yy in ys {
        for xx in xs.clone() {
            let i = ((yy - y) * m.w as i32 + (xx - x)) as usize;
            let (r, g, b, a) = match &m.data {
                MaskData::Alpha(cov) => {
                    let a = mul(u32::from(cov[i]), u32::from(c.a));
                    (c.r, c.g, c.b, a)
                }
                MaskData::Color(px) => {
                    let o = i * 4;
                    (
                        px[o],
                        px[o + 1],
                        px[o + 2],
                        mul(u32::from(px[o + 3]), u32::from(c.a)),
                    )
                }
                MaskData::Solid => (c.r, c.g, c.b, u32::from(c.a)),
            };
            if a == 0 {
                continue;
            }
            let o = ((yy * pw + xx) * 4) as usize;
            let ia = 255 - a;
            for (k, s) in [r, g, b].into_iter().enumerate() {
                data[o + k] = (mul(u32::from(s), a) + mul(u32::from(data[o + k]), ia)) as u8;
            }
            data[o + 3] = (a + mul(u32::from(data[o + 3]), ia)) as u8;
        }
    }
}

/// `a * b / 255`, rounded.
fn mul(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    (t + (t >> 8)) >> 8
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::markdown::DisplaySpan;
    use editor_core::tokens::Theme;

    #[test]
    fn styles_map_to_tokens() {
        let t = Tokens::defaults(Theme::Light);
        let mut st = SpanStyle::plain(SpanBase::Heading(2));
        let h = span_for(&st, 0..3, &t, 2.0);
        assert_eq!(h.size_px, t.px(TokenId::FontSizeH2) * 2.0);
        assert_eq!(h.weight, t.weight(TokenId::FontWeightHeading));
        assert_eq!(h.role, FontRole::Body);
        st.strong = true;
        st.link = true;
        st.em = true;
        st.strike = true;
        let s = span_for(&st, 0..3, &t, 1.0);
        assert_eq!(s.weight, t.weight(TokenId::FontWeightStrong));
        assert_eq!(s.color, t.color(TokenId::ColorLink));
        assert!(s.italic && s.strike);
        let syn = span_for(&SpanStyle::plain(SpanBase::Syntax), 0..1, &t, 1.0);
        assert_eq!(syn.role, FontRole::Mono);
        assert_eq!(syn.color, t.color(TokenId::ColorSyntaxReveal));
        let code = span_for(&SpanStyle::plain(SpanBase::Code), 0..1, &t, 1.0);
        assert_eq!(code.role, FontRole::Mono);
        assert_eq!(code.size_px, t.px(TokenId::FontSize));
    }

    #[test]
    fn styled_line_concatenates_shown_source() {
        let t = Tokens::defaults(Theme::Light);
        let d = LineDisplay {
            prefix: Prefix::None,
            spans: vec![
                DisplaySpan {
                    src: 2..6,
                    style: SpanStyle::plain(SpanBase::Body),
                },
                DisplaySpan {
                    src: 8..13,
                    style: SpanStyle::plain(SpanBase::Body),
                },
            ],
            deco: editor_core::markdown::BlockDeco::None,
            src_len: 13,
        };
        let (text, spans) = styled_line("**bold** tail", &d, &t, 1.0);
        assert_eq!(text, "bold tail");
        assert_eq!(spans[1].range, 4..9);
        assert_eq!(src_to_display_clamped(&d, 0), 0);
        assert_eq!(src_to_display_clamped(&d, 7), 4);
        assert_eq!(src_to_display_clamped(&d, 13), 9);
    }

    #[test]
    fn blend_is_source_over() {
        let mut p = Pixmap::new(2, 1).unwrap();
        let t = Tokens::defaults(Theme::Light);
        let c = t.color(TokenId::ColorText);
        fill(
            &mut p,
            0.0,
            0.0,
            2.0,
            1.0,
            t.color(TokenId::ColorBg),
            &(0.0..1.0),
        );
        let m = GlyphMask {
            x: 0,
            y: 0,
            w: 2,
            h: 1,
            color: c,
            data: MaskData::Alpha(vec![255, 0].into()),
        };
        blend(&mut p, 0, 0, &m, &(0..1));
        let full = p.pixel(0, 0).unwrap();
        assert_eq!((full.red(), full.green(), full.blue()), (c.r, c.g, c.b));
        let none = p.pixel(1, 0).unwrap();
        let bg = t.color(TokenId::ColorBg);
        assert_eq!((none.red(), none.green(), none.blue()), (bg.r, bg.g, bg.b));
    }
}

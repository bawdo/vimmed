//! Line painting: mapping a line's display model to styled spans (every size,
//! weight and colour from tokens), filling rectangles through tiny-skia and
//! blending glyph masks into the frame.

use std::ops::Range;

use editor_core::markdown::{LineDisplay, Prefix, SpanBase, SpanStyle};
use editor_core::tokens::{Rgba, TokenId, Tokens};
use tiny_skia::{FilterQuality, Paint, Pattern, Pixmap, Rect, SpreadMode, Transform};

use crate::layout::{FontRole, GlyphMask, MaskData, RectF, StyledSpan};

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

thread_local! {
    /// `draw_scaled` calls made on this thread.
    static RESAMPLES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many times `draw_scaled` has run on the calling thread. A resample
/// is tens of milliseconds for a large image, so the UI thread must never
/// add to this (tests and diagnostics read it).
pub fn resamples_on_this_thread() -> u64 {
    RESAMPLES.with(|n| n.get())
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
    RESAMPLES.with(|n| n.set(n.get() + 1));
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
/// italic; revealed syntax takes `color.syntax.reveal` and links `color.link`,
/// underlined.
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
        underline: style.link && style.base != SpanBase::Syntax,
    }
}

/// The ATX heading level of source line `src` (`#` to `######` after at most
/// three spaces, then a space or the end), or `None`.
pub fn atx_level(src: &str) -> Option<u8> {
    let t = src.trim_start_matches(' ');
    if src.len() - t.len() > 3 {
        return None;
    }
    let n = t.bytes().take_while(|&b| b == b'#').count();
    let rest = &t[n..];
    ((1..=6).contains(&n) && (rest.is_empty() || rest.starts_with([' ', '\t']))).then_some(n as u8)
}

/// A line's display text and its styled spans. An empty display still gets
/// one empty span in the line's base style, so it keeps its line height.
///
/// Inline code on a heading line keeps the heading's size (it stays mono):
/// `Code` replaces the line's base role, so the level comes from the line's
/// heading spans, or from its source when the `#` prefix is all that says so.
pub fn styled_line(
    src: &str,
    d: &LineDisplay,
    t: &Tokens,
    scale: f32,
) -> (String, Vec<StyledSpan>) {
    let mut text = String::new();
    let mut spans = Vec::with_capacity(d.spans.len().max(1));
    let has_code = d.spans.iter().any(|s| s.style.base == SpanBase::Code);
    let heading = has_code
        .then(|| {
            d.spans
                .iter()
                .find_map(|s| match s.style.base {
                    SpanBase::Heading(n) => Some(n),
                    _ => None,
                })
                .or_else(|| atx_level(src))
        })
        .flatten();
    for s in &d.spans {
        let piece = src.get(s.src.clone()).unwrap_or_default();
        let start = text.len();
        text.push_str(piece);
        let mut span = span_for(&s.style, start..text.len(), t, scale);
        if let (SpanBase::Code, Some(n)) = (&s.style.base, heading) {
            span.size_px = t.px(heading_size(n)) * scale;
        }
        spans.push(span);
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

/// Puts `pre`, an input method's composition, into a line's display `text`
/// at display byte `at`, in the style of the span it lands in (or the one
/// that ends there), underlined and in `color`. The spans after it move
/// along. Returns the composition's display range.
pub fn splice_preedit(
    text: &mut String,
    spans: &mut Vec<StyledSpan>,
    at: usize,
    pre: &str,
    color: Rgba,
) -> Range<usize> {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    text.insert_str(at, pre);
    let n = pre.len();
    // The span it lands in, else the last one ending at it, else the first.
    let donor = spans
        .iter()
        .position(|s| s.range.start <= at && at < s.range.end)
        .or_else(|| spans.iter().rposition(|s| s.range.end == at))
        .unwrap_or(0);
    let piece = spans.get(donor).map(|d| StyledSpan {
        range: at..at + n,
        color,
        underline: true,
        ..d.clone()
    });
    let mut out = Vec::with_capacity(spans.len() + 2);
    for (k, s) in std::mem::take(spans).into_iter().enumerate() {
        let r = s.range.clone();
        if r.end <= at {
            out.push(s);
            if k == donor {
                out.extend(piece.clone());
            }
        } else if r.start >= at {
            if k == donor {
                out.extend(piece.clone());
            }
            out.push(StyledSpan {
                range: r.start + n..r.end + n,
                ..s
            });
        } else {
            out.push(StyledSpan {
                range: r.start..at,
                ..s.clone()
            });
            out.extend(piece.clone());
            out.push(StyledSpan {
                range: at + n..r.end + n,
                ..s
            });
        }
    }
    *spans = out;
    at..at + n
}

/// The display byte ranges of a line's inline code spans, merged where they
/// touch.
pub fn code_ranges(d: &LineDisplay) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut at = 0;
    for s in &d.spans {
        let r = at..at + s.src.len();
        at = r.end;
        if s.style.base != SpanBase::Code || r.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == r.start => last.end = r.end,
            _ => out.push(r),
        }
    }
    out
}

/// Splits a code line's spans by highlight classes (byte ranges into the
/// display text, which on a code line is the source line), colouring each
/// piece with its class's token. Ranges off a char boundary or past the text
/// are skipped.
pub fn apply_classes(
    text: &str,
    spans: Vec<StyledSpan>,
    classes: &[(Range<usize>, crate::highlight::SyntaxClass)],
    t: &Tokens,
) -> Vec<StyledSpan> {
    if classes.is_empty() {
        return spans;
    }
    let ok = |r: &Range<usize>| {
        r.start < r.end
            && r.end <= text.len()
            && text.is_char_boundary(r.start)
            && text.is_char_boundary(r.end)
    };
    let mut out = Vec::with_capacity(spans.len() + 2 * classes.len());
    for base in spans {
        let mut at = base.range.start;
        for (r, class) in classes.iter().filter(|(r, _)| ok(r)) {
            let (a, b) = (r.start.max(at), r.end.min(base.range.end));
            if a >= b {
                continue;
            }
            if a > at {
                out.push(StyledSpan {
                    range: at..a,
                    ..base.clone()
                });
            }
            out.push(StyledSpan {
                range: a..b,
                color: t.color(class.token()),
                ..base.clone()
            });
            at = b;
        }
        if at < base.range.end || base.range.is_empty() {
            out.push(StyledSpan {
                range: at..base.range.end,
                ..base
            });
        }
    }
    out
}

/// The language tag of a fence line: the first word after the backticks or
/// tildes, or an empty string.
pub fn fence_lang(line: &str) -> &str {
    let t = line.trim_start();
    let t = t.trim_start_matches(['`', '~']).trim_start();
    t.split(|c: char| c.is_whitespace() || c == '{')
        .next()
        .unwrap_or("")
}

/// A revealed block prefix that can hang in the left margin: where its glyphs
/// start (after leading whitespace), where it ends (after the space that
/// follows it), and its quote depth (0 unless it is a quote prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HangPrefix {
    pub start: usize,
    pub end: usize,
    pub quote_depth: u8,
}

/// The block prefix of source line `src` that hangs: a heading's `#`s, a
/// quote's `>`s, or a list marker (with its task box). `None` for other lines.
pub fn hang_prefix(src: &str) -> Option<HangPrefix> {
    let b = src.as_bytes();
    let start = b.iter().take_while(|&&c| c == b' ' || c == b'\t').count();
    let skip_spaces = |mut i: usize| {
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        i
    };
    let rest = &b[start..];
    if let Some(n) = atx_level(src) {
        let end = skip_spaces(start + usize::from(n));
        return Some(HangPrefix {
            start,
            end,
            quote_depth: 0,
        });
    }
    if rest.first() == Some(&b'>') {
        let (mut i, mut depth) = (start, 0u8);
        while i < b.len() && b[i] == b'>' {
            depth = depth.saturating_add(1);
            i = skip_spaces(i + 1);
        }
        return Some(HangPrefix {
            start,
            end: i,
            quote_depth: depth,
        });
    }
    let marker = match rest {
        [b'-' | b'*' | b'+', b' ', ..] => 1,
        _ => {
            let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
            match rest.get(digits) {
                Some(b'.' | b')')
                    if (1..=9).contains(&digits) && rest.get(digits + 1) == Some(&b' ') =>
                {
                    digits + 1
                }
                _ => return None,
            }
        }
    };
    let mut end = skip_spaces(start + marker);
    for task in [b"[ ] ", b"[x] ", b"[X] "] {
        if b[end..].starts_with(task) {
            end = skip_spaces(end + task.len());
        }
    }
    Some(HangPrefix {
        start,
        end,
        quote_depth: 0,
    })
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

/// Which corners of a box `fill_rounded` rounds, and by how much (pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corners {
    pub radius: f32,
    pub top: bool,
    pub bottom: bool,
}

/// Fills the box `b` like `fill`, with its top and/or bottom corners rounded.
/// A corner's curved edge is anti-aliased by coverage; nothing is drawn
/// outside the box or the rows `clip`.
pub fn fill_rounded(p: &mut Pixmap, b: RectF, corners: Corners, c: Rgba, clip: &Range<f32>) {
    let r = corners.radius.min(b.w / 2.0).min(b.h / 2.0).max(0.0);
    let band_top = if corners.top { r.ceil() } else { 0.0 };
    let band_bottom = if corners.bottom { r.ceil() } else { 0.0 };
    let mid_h = b.h - band_top - band_bottom;
    fill(p, b.x, b.y + band_top, b.w, mid_h, c, clip);
    let mut row = |yy: f32, d: f32| {
        // `d`: how far this row's centre is inside the corner's circle centre line.
        let inset = r - (r * r - d * d).max(0.0).sqrt();
        let (xl, xr) = (b.x + inset, b.x + b.w - inset);
        let (full_l, full_r) = (xl.ceil(), xr.floor());
        fill(p, full_l, yy, full_r - full_l, 1.0, c, clip);
        let ry = yy as u32..yy as u32 + 1;
        for (px, cover) in [(full_l - 1.0, full_l - xl), (full_r, xr - full_r)] {
            if cover <= 0.0 || !(clip.start..clip.end).contains(&yy) {
                continue;
            }
            let mut color = c;
            color.a = (f32::from(c.a) * cover).round() as u8;
            let m = GlyphMask {
                x: px as i32,
                y: yy as i32,
                w: 1,
                h: 1,
                color,
                data: MaskData::Solid,
            };
            blend(p, 0, 0, &m, &ry);
        }
    };
    for k in 0..band_top as u32 {
        let yy = b.y + k as f32;
        row(yy, r - (k as f32 + 0.5));
    }
    for k in 0..band_bottom as u32 {
        let yy = b.y + b.h - 1.0 - k as f32;
        row(yy, r - (k as f32 + 0.5));
    }
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

    fn span(range: Range<usize>, t: &Tokens) -> StyledSpan {
        span_for(&SpanStyle::plain(SpanBase::Body), range, t, 1.0)
    }

    #[test]
    fn preedit_splits_the_span_it_lands_in() {
        let t = Tokens::defaults(Theme::Light);
        let green = t.color(TokenId::ColorPreeditUnderline);
        let mut text = "abcd".to_owned();
        let mut bold = span(2..4, &t);
        bold.weight = t.weight(TokenId::FontWeightStrong);
        let mut spans = vec![span(0..2, &t), bold.clone()];
        let r = splice_preedit(&mut text, &mut spans, 3, "日本", green);
        assert_eq!(text, "abc日本d");
        assert_eq!(r, 3..9);
        let ranges: Vec<_> = spans.iter().map(|s| s.range.clone()).collect();
        assert_eq!(ranges, vec![0..2, 2..3, 3..9, 9..10]);
        // In the style of the span it landed in, underlined in its colour.
        assert_eq!(spans[2].weight, bold.weight);
        assert!(spans[2].underline);
        assert_eq!(spans[2].color, green);
        assert!(!spans[1].underline && !spans[3].underline);
        assert_eq!(spans[3].weight, bold.weight);
    }

    #[test]
    fn preedit_at_a_boundary_or_on_an_empty_line() {
        let t = Tokens::defaults(Theme::Light);
        let c = t.color(TokenId::ColorPreeditUnderline);
        // Between two spans: after the first, the second moves along.
        let mut text = "abcd".to_owned();
        let mut spans = vec![span(0..2, &t), span(2..4, &t)];
        assert_eq!(splice_preedit(&mut text, &mut spans, 2, "x", c), 2..3);
        let ranges: Vec<_> = spans.iter().map(|s| s.range.clone()).collect();
        assert_eq!(ranges, vec![0..2, 2..3, 3..5]);
        // At the end.
        let mut text = "ab".to_owned();
        let mut spans = vec![span(0..2, &t)];
        assert_eq!(splice_preedit(&mut text, &mut spans, 9, "x", c), 2..3);
        assert_eq!(text, "abx");
        // An empty line keeps one span per piece, in order.
        let mut text = String::new();
        let mut spans = vec![span(0..0, &t)];
        assert_eq!(splice_preedit(&mut text, &mut spans, 0, "x", c), 0..1);
        assert_eq!(text, "x");
        assert_eq!(spans.last().unwrap().range, 0..1);
        assert!(spans.last().unwrap().underline);
        // Inside a char: before it.
        let mut text = "é".to_owned();
        let mut spans = vec![span(0..2, &t)];
        assert_eq!(splice_preedit(&mut text, &mut spans, 1, "x", c), 0..1);
        assert_eq!(text, "xé");
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

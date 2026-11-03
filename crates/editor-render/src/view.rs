//! `DocView`: the document painted into a frame, one layout object per
//! source line, with row-range damage.
//!
//! Geometry, top to bottom: `space.page.top`, then the lines in a column
//! `space.column` wide (narrower when the window is, keeping `space.page.side`
//! each side) and centred, then the status bar (`size.status.height`) at the
//! bottom. Line heights are whole pixels, so every damaged row belongs to
//! exactly one line or the status bar.
//!
//! Only on-screen lines are shaped. A line not shaped yet counts as
//! `font.size * line.height.body` tall. An unchanged line keeps its layout
//! across frames, so a full repaint after a few keystrokes reshapes nothing
//! (see `layout_cosmic::RUN_CACHE_KEEP_AGES`).

use std::ops::Range;
use std::path::Path;

use editor_core::buffer::Pos;
use editor_core::host::Waker;
use editor_core::markdown::{BlockDeco, LineDisplay, SpanBase, SpanStyle, display_to_src};
use editor_core::strings::IMAGE_NOT_FOUND;
use editor_core::tokens::{TokenId, Tokens};
use editor_core::view_model::{StatusModel, ViewSource};

use crate::damage::Damage;
use crate::frame::Frame;
use crate::images::{ImageSlot, ImageState, Images};
use crate::layout::{LaidLine, RectF, StyledSpan, TextLayout};
use crate::paint::{
    blend, blit, content_offset, draw_scaled, fill, prefix_text, span_for, src_to_display_clamped,
    styled_line,
};

/// What an image line draws: its reserved box in pixels and the state to
/// draw it in. The box never changes once the header is read.
struct ImageLine {
    src: String,
    /// Zero when there is no known size (missing file, remote URL).
    w_px: f32,
    h_px: f32,
    state: ImageState,
    /// The decoded image resampled once to the box, so a redraw is a plain
    /// copy (a keystroke that moves the line redraws it).
    scaled: Option<tiny_skia::Pixmap>,
    /// Every pixel of `scaled` has full alpha.
    opaque: bool,
}

impl ImageLine {
    /// Resamples a decoded image to its box if that has not been done.
    fn ensure_scaled(&mut self) {
        let ImageState::Ready(px) = &self.state else {
            return;
        };
        if self.scaled.is_some() {
            return;
        }
        let (w, h) = (self.w_px as u32, self.h_px as u32);
        if let Some(mut boxed) = tiny_skia::Pixmap::new(w, h) {
            draw_scaled(
                &mut boxed,
                0.0,
                0.0,
                self.w_px,
                self.h_px,
                px,
                &(0.0..self.h_px),
            );
            self.opaque = boxed.data().as_chunks::<4>().0.iter().all(|p| p[3] == 255);
            self.scaled = Some(boxed);
        }
    }
}

/// Paints an image line's box (the page is already filled behind it): the
/// pixels when decoded, else the placeholder fill. Remote links have no box.
fn paint_image(
    p: &mut tiny_skia::Pixmap,
    t: &Tokens,
    x: f32,
    top: f32,
    img: &ImageLine,
    clip: &Range<f32>,
) {
    match &img.state {
        ImageState::Ready(px) => match &img.scaled {
            Some(s) => blit(p, x as i32, top as i32, s, img.opaque, clip),
            None => draw_scaled(p, x, top, img.w_px, img.h_px, px, clip),
        },
        ImageState::Pending | ImageState::Missing => {
            let c = t.color(TokenId::ColorImagePlaceholder);
            fill(p, x, top, img.w_px, img.h_px, c, clip);
        }
        ImageState::Remote(_) => {}
    }
}

/// A rectangle in points, relative to the frame's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectPt {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// What a layout was made from. Equal keys give equal layouts.
#[derive(PartialEq)]
pub(crate) struct LayoutKey {
    text: String,
    spans: Vec<StyledSpan>,
    width: f32,
}

pub(crate) struct Cached {
    key: LayoutKey,
    pub(crate) laid: LaidLine,
}

impl Cached {
    /// True when this layout was made from exactly these inputs.
    pub(crate) fn key_matches(&self, text: &str, spans: &[StyledSpan], width: f32) -> bool {
        self.key.text == text && self.key.spans == spans && self.key.width == width
    }
}

/// `slot`'s layout, made again only when its key changed.
pub(crate) fn lay<'a>(
    layout: &mut dyn TextLayout,
    slot: &'a mut Option<Cached>,
    text: String,
    spans: Vec<StyledSpan>,
    width: f32,
) -> &'a LaidLine {
    let key = LayoutKey { text, spans, width };
    if slot.as_ref().is_none_or(|c| c.key != key) {
        let laid = layout.layout_line(&key.text, &key.spans, width);
        *slot = Some(Cached { key, laid });
    }
    &slot.as_ref().expect("just filled").laid
}

/// Per source line: its height in pixels (estimated until shaped), and once
/// shaped, its layouts and the display they came from.
struct LineSlot {
    height: f32,
    text: Option<Cached>,
    prefix: Option<Cached>,
    display: Option<LineDisplay>,
    /// The text's offset right of the column's left edge, in pixels.
    offset: f32,
    /// Set while the line is an image line and images are on. Kept when the
    /// layouts are forgotten, so a decode can still paint a line scrolled
    /// back into view.
    image: Option<ImageLine>,
    /// The placeholder or link text of an image line that has no pixels yet.
    label: Option<Cached>,
}

impl LineSlot {
    fn estimated(height: f32) -> LineSlot {
        LineSlot {
            height,
            text: None,
            prefix: None,
            display: None,
            offset: 0.0,
            image: None,
            label: None,
        }
    }

    fn forget(&mut self) {
        self.text = None;
        self.prefix = None;
        self.display = None;
        self.label = None;
    }
}

/// Frame geometry in pixels.
#[derive(Clone, Copy)]
pub(crate) struct Geo {
    pub(crate) width: f32,
    pub(crate) height: f32,
    col_x: f32,
    col_w: f32,
    page_top: f32,
    /// The first row of the status bar.
    pub(crate) text_bottom: f32,
}

pub struct DocView {
    pub(crate) layout: Box<dyn TextLayout>,
    pub(crate) tokens: Tokens,
    pub(crate) frame: Frame,
    /// Scroll offset in pixels: how far the document has moved up.
    scroll: f32,
    lines: Vec<LineSlot>,
    pub(crate) status_cache: Vec<Cached>,
    pub(crate) last_status: Option<StatusModel>,
    pub(crate) damage: Damage,
    /// The caret's line and its rectangle in frame pixels, as last painted.
    caret: Option<(usize, RectF)>,
    /// Present once `set_images` is called.
    images: Option<Images>,
    /// True once the first decoded image has been painted (and marked).
    image_marked: bool,
}

impl DocView {
    pub fn new(
        layout: Box<dyn TextLayout>,
        tokens: &Tokens,
        w_pt: f32,
        h_pt: f32,
        scale: f32,
    ) -> DocView {
        let mut v = DocView {
            layout,
            tokens: tokens.clone(),
            frame: Frame::new(to_px(w_pt, scale), to_px(h_pt, scale), scale),
            scroll: 0.0,
            lines: Vec::new(),
            status_cache: Vec::new(),
            last_status: None,
            damage: Damage::new(),
            caret: None,
            images: None,
            image_marked: false,
        };
        v.clear_frame();
        v
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// The rows changed since the last call.
    pub fn take_damage(&mut self) -> Damage {
        let mut d = std::mem::take(&mut self.damage);
        d.clip(self.frame.height_px);
        d
    }

    /// Changes the frame to `w_pt` by `h_pt` points at `scale`. The pixmap is
    /// reallocated when its pixel size or scale changes, and the whole frame
    /// is damaged: call `paint_all` next.
    pub fn resize(&mut self, w_pt: f32, h_pt: f32, scale: f32) {
        let old_scale = self.frame.scale;
        let (w, h) = (to_px(w_pt, scale), to_px(h_pt, scale));
        let rescaled = scale != old_scale;
        if rescaled || w != self.frame.width_px || h != self.frame.height_px {
            self.frame = Frame::new(w, h, scale);
        }
        if rescaled {
            let est = self.estimate();
            for s in &mut self.lines {
                *s = LineSlot::estimated(est);
            }
            self.status_cache.clear();
            if old_scale > 0.0 {
                self.scroll = (self.scroll * scale / old_scale).round();
            }
        }
        self.last_status = None;
        self.caret = None;
        self.clear_frame();
    }

    /// Repaints everything: background, every on-screen line and the status
    /// bar. Unchanged lines reuse their layouts.
    pub fn paint_all(&mut self, src: &dyn ViewSource) {
        self.sync(src);
        let g = self.geo();
        self.caret = None;
        let bg = self.tokens.color(TokenId::ColorBg);
        fill(
            &mut self.frame.pixmap,
            0.0,
            0.0,
            g.width,
            g.text_bottom,
            bg,
            &(0.0..g.text_bottom),
        );
        self.paint_down_from(src, 0, g.page_top - self.scroll);
        self.last_status = None;
        self.paint_status(src);
        self.damage.add(0..self.frame.height_px);
        self.layout.end_frame();
    }

    /// Repaints source lines `lines` after an edit, and the status bar when its
    /// model changed. A line whose height is unchanged damages only its own
    /// rows; a height change (or a change in the line count) repaints and
    /// damages from that line to the bottom of the text. The caret's old and
    /// new lines are repainted when the cursor changed line.
    pub fn paint_lines(&mut self, src: &dyn ViewSource, lines: Range<usize>) {
        let count_changed = self.sync(src);
        let count = self.lines.len();
        let first_edit = lines.start;
        let mut todo: Vec<usize> = lines.collect();
        let cursor = src.cursor().line;
        let old = self.caret.map(|(l, _)| l);
        if old != Some(cursor) {
            todo.extend(old);
            todo.push(cursor);
        }
        todo.retain(|&i| i < count);
        todo.sort_unstable();
        todo.dedup();

        let g = self.geo();
        let mut from = count_changed.then_some(first_edit.min(count));
        for i in todo {
            if from.is_some_and(|f| i >= f) {
                break;
            }
            let top = self.line_top(i);
            if top >= g.text_bottom || top + self.lines[i].height <= 0.0 {
                self.lines[i].forget();
                continue;
            }
            let before = self.lines[i].height;
            if self.relayout(src, i) != before {
                from = Some(i);
                break;
            }
            self.draw_line(src, i, top);
            self.damage.add(rows(top, before, g.text_bottom));
        }
        if let Some(f) = from {
            self.repaint_to_bottom(src, f);
        }
        self.paint_status(src);
        self.layout.end_frame();
    }

    /// The source position under the point `x_pt, y_pt`, or `None` outside
    /// the text area or over a line not painted yet.
    pub fn hit_test(&self, x_pt: f32, y_pt: f32) -> Option<Pos> {
        let s = self.frame.scale;
        let (x, y) = (x_pt * s, y_pt * s);
        let g = self.geo();
        if !(0.0..g.text_bottom).contains(&y) || self.lines.is_empty() {
            return None;
        }
        let last = self.lines.len() - 1;
        let mut top = g.page_top - self.scroll;
        for (i, slot) in self.lines.iter().enumerate() {
            if y < top + slot.height || i == last {
                let laid = &slot.text.as_ref()?.laid;
                let d = slot.display.as_ref()?;
                let b = laid.hit(x - g.col_x - slot.offset, (y - top).max(0.0));
                return Some(Pos {
                    line: i,
                    col: display_to_src(d, b),
                });
            }
            top += slot.height;
        }
        None
    }

    /// Scrolls so the cursor's line is in view. Returns true when the view
    /// moved; the caller then calls `paint_all`.
    pub fn scroll_to_cursor(&mut self, cursor: Pos) -> bool {
        let Some(last) = self.lines.len().checked_sub(1) else {
            return false;
        };
        let i = cursor.line.min(last);
        let g = self.geo();
        let top: f32 = g.page_top + self.lines[..i].iter().map(|s| s.height).sum::<f32>();
        let bottom = top + self.lines[i].height;
        let mut s = self.scroll;
        if top - s < 0.0 || (i == 0 && s > 0.0) {
            s = (top - g.page_top).max(0.0);
        } else if bottom - s > g.text_bottom {
            s = bottom - g.text_bottom;
        }
        let s = s.round();
        let moved = s != self.scroll;
        self.scroll = s;
        moved
    }

    /// The caret as last painted, in points (for IME placement).
    pub fn caret_rect(&self) -> Option<RectPt> {
        let (_, r) = self.caret?;
        let s = self.frame.scale;
        Some(RectPt {
            x: r.x / s,
            y: r.y / s,
            w: r.w / s,
            h: r.h / s,
        })
    }

    /// True when line `i` holds a layout (it was on screen at the last paint).
    pub fn is_shaped(&self, i: usize) -> bool {
        self.lines.get(i).is_some_and(|s| s.text.is_some())
    }

    /// Line `i`'s height in pixels: measured once shaped, estimated before.
    pub fn line_height_px(&self, i: usize) -> Option<f32> {
        self.lines.get(i).map(|s| s.height)
    }

    /// Turns images on: local paths resolve against `base_dir`, and `waker`
    /// is called (from the worker thread) after each decode. Call before the
    /// first `paint_all`; lines painted earlier pick images up when next laid out.
    pub fn set_images(&mut self, base_dir: &Path, waker: Waker) {
        self.images = Some(Images::new(base_dir, waker));
    }

    /// Takes finished image decodes and paints them into the space their
    /// lines already reserved: no relayout. Returns true when rows were
    /// damaged; the shell then copies `take_damage()` to the screen.
    pub fn poll_async(&mut self) -> bool {
        let changed = match self.images.as_mut() {
            Some(images) => images.poll(),
            None => return false,
        };
        let g = self.geo();
        let mut any = false;
        for i in changed {
            let Some(src) = self
                .lines
                .get(i)
                .and_then(|s| s.image.as_ref())
                .map(|m| m.src.clone())
            else {
                continue;
            };
            let slot = self.images.as_ref().and_then(|im| im.slot_of(&src));
            let Some(ImageSlot { state, .. }) = slot else {
                continue;
            };
            if let Some(img) = self.lines[i].image.as_mut() {
                img.state = state;
                img.ensure_scaled();
            }
            let h = self.lines[i].height;
            let top = self.line_top(i);
            if top >= g.text_bottom || top + h <= 0.0 {
                continue;
            }
            let clip = top.max(0.0)..(top + h).min(g.text_bottom);
            let bg = self.tokens.color(TokenId::ColorBg);
            fill(&mut self.frame.pixmap, g.col_x, top, g.col_w, h, bg, &clip);
            if let Some(img) = &self.lines[i].image {
                let (pix, t) = (&mut self.frame.pixmap, &self.tokens);
                paint_image(pix, t, g.col_x, top, img, &clip);
                if matches!(img.state, ImageState::Ready(_)) && !self.image_marked {
                    self.image_marked = true;
                    marks::mark(marks::names::IMAGES_PAINTED);
                }
            }
            self.damage.add(rows(top, h, g.text_bottom));
            any = true;
        }
        any
    }

    pub(crate) fn geo(&self) -> Geo {
        let t = &self.tokens;
        let s = self.frame.scale;
        let width = self.frame.width_px as f32;
        let height = self.frame.height_px as f32;
        let side = t.px(TokenId::SpacePageSide) * s;
        let col_w = (t.px(TokenId::SpaceColumn) * s)
            .min(width - 2.0 * side)
            .max(1.0)
            .round();
        let status = (t.px(TokenId::SizeStatusHeight) * s).round().min(height);
        Geo {
            width,
            height,
            col_x: ((width - col_w) / 2.0).max(0.0).floor(),
            col_w,
            page_top: (t.px(TokenId::SpacePageTop) * s).round(),
            text_bottom: height - status,
        }
    }

    fn estimate(&self) -> f32 {
        let t = &self.tokens;
        (t.px(TokenId::FontSize) * t.ratio(TokenId::LineHeightBody) * self.frame.scale).ceil()
    }

    /// Takes the source's tokens and line count. Returns true when the line
    /// count changed.
    fn sync(&mut self, src: &dyn ViewSource) -> bool {
        self.tokens = src.tokens().clone();
        let n = src.line_count();
        if n == self.lines.len() {
            return false;
        }
        let est = self.estimate();
        self.lines.truncate(n);
        self.lines.resize_with(n, || LineSlot::estimated(est));
        true
    }

    /// Fills the frame with the page and status backgrounds and damages it.
    fn clear_frame(&mut self) {
        let g = self.geo();
        let bg = self.tokens.color(TokenId::ColorBg);
        let status = self.tokens.color(TokenId::ColorStatusBg);
        let all = 0.0..g.height;
        fill(
            &mut self.frame.pixmap,
            0.0,
            0.0,
            g.width,
            g.text_bottom,
            bg,
            &all,
        );
        let h = g.height - g.text_bottom;
        fill(
            &mut self.frame.pixmap,
            0.0,
            g.text_bottom,
            g.width,
            h,
            status,
            &all,
        );
        self.damage = Damage::new();
        self.damage.add(0..self.frame.height_px);
    }

    /// Line `i`'s top in frame pixels.
    fn line_top(&self, i: usize) -> f32 {
        let above: f32 = self.lines[..i].iter().map(|s| s.height).sum();
        self.geo().page_top - self.scroll + above
    }

    /// Lays line `i` out from the source and returns its height.
    fn relayout(&mut self, src: &dyn ViewSource, i: usize) -> f32 {
        let g = self.geo();
        let t = &self.tokens;
        let s = self.frame.scale;
        let d = src.display(i);
        let (text, spans) = styled_line(src.line(i), &d, t, s);
        let offset = content_offset(&d.prefix, t, s).round();
        let width = (g.col_w - offset).max(1.0);
        let slot = &mut self.lines[i];
        let layout = self.layout.as_mut();
        let laid = lay(layout, &mut slot.text, text, spans, width);
        slot.height = laid.height.ceil();
        slot.offset = offset;
        match prefix_text(&d.prefix) {
            Some(p) => {
                let style =
                    editor_core::markdown::SpanStyle::plain(editor_core::markdown::SpanBase::Body);
                let span = span_for(&style, 0..p.len(), t, s);
                lay(layout, &mut slot.prefix, p, vec![span], offset.max(1.0));
            }
            None => slot.prefix = None,
        }
        let old_image = slot.image.take();
        match (&d.deco, self.images.as_mut()) {
            (BlockDeco::Image { src, alt }, Some(images)) => {
                images.set_limits(
                    t.px(TokenId::SpaceColumn),
                    t.px(TokenId::SizeImageMaxHeight),
                );
                let r = images.reserve(i, src);
                let known = r.w_pt > 0.0;
                let mut link = false;
                let label = match &r.state {
                    ImageState::Ready(_) => None,
                    ImageState::Pending => (!alt.is_empty()).then(|| alt.clone()),
                    ImageState::Missing if alt.is_empty() => Some(IMAGE_NOT_FOUND.to_owned()),
                    ImageState::Missing => Some(alt.clone()),
                    ImageState::Remote(url) => {
                        link = true;
                        Some(if alt.is_empty() {
                            url.clone()
                        } else {
                            format!("{alt} {url}")
                        })
                    }
                };
                match label {
                    Some(text) => {
                        let mut style = SpanStyle::plain(SpanBase::Body);
                        style.link = link;
                        let span = span_for(&style, 0..text.len(), t, s);
                        lay(layout, &mut slot.label, text, vec![span], width);
                    }
                    None => slot.label = None,
                }
                let label_h = slot.label.as_ref().map_or(0.0, |c| c.laid.height.ceil());
                let (w_px, h_px) = if known {
                    ((r.w_pt * s).round(), (r.h_pt * s).ceil())
                } else {
                    (width, slot.height.max(label_h))
                };
                slot.height = h_px;
                // Keep the resampled copy while the box and the image are unchanged.
                let kept = old_image.filter(|o| o.src == *src && o.w_px == w_px && o.h_px == h_px);
                let opaque = kept.as_ref().is_some_and(|o| o.opaque);
                let mut line = ImageLine {
                    src: src.clone(),
                    w_px,
                    h_px,
                    state: r.state,
                    scaled: kept.and_then(|o| o.scaled),
                    opaque,
                };
                line.ensure_scaled();
                slot.image = Some(line);
            }
            _ => slot.label = None,
        }
        slot.display = Some(d);
        slot.height
    }

    /// Lays out and paints lines from `first` (whose top is `top`) down to the
    /// bottom of the text area, skipping lines above the top edge.
    ///
    /// Lines from `first` on that end up off screen release their layouts and
    /// keep their measured heights, so memory is bounded by the screen and
    /// scroll positions stay stable. Lines above `first` are left alone.
    fn paint_down_from(&mut self, src: &dyn ViewSource, first: usize, mut top: f32) {
        let g = self.geo();
        for i in first..self.lines.len() {
            if top >= g.text_bottom {
                for slot in &mut self.lines[i..] {
                    slot.forget();
                }
                break;
            }
            if top + self.lines[i].height > 0.0 {
                self.relayout(src, i);
                self.draw_line(src, i, top);
            } else {
                self.lines[i].forget();
            }
            top += self.lines[i].height;
        }
    }

    /// Repaints from line `first` to the bottom of the text and damages it.
    fn repaint_to_bottom(&mut self, src: &dyn ViewSource, first: usize) {
        let g = self.geo();
        let top = if first < self.lines.len() {
            self.line_top(first)
        } else {
            self.line_top(self.lines.len())
        };
        let y0 = top.clamp(0.0, g.text_bottom);
        if self.caret.is_some_and(|(l, _)| l >= first) {
            self.caret = None;
        }
        let bg = self.tokens.color(TokenId::ColorBg);
        let clip = y0..g.text_bottom;
        fill(
            &mut self.frame.pixmap,
            0.0,
            y0,
            g.width,
            g.text_bottom - y0,
            bg,
            &clip,
        );
        self.paint_down_from(src, first, top);
        self.damage.add(y0 as u32..g.text_bottom as u32);
    }

    /// Paints line `i` (already laid out) with its top at `top`: background,
    /// block decoration, search matches, selection, prefix, text and caret.
    fn draw_line(&mut self, src: &dyn ViewSource, i: usize, top: f32) {
        let g = self.geo();
        let t = &self.tokens;
        let s = self.frame.scale;
        let slot = &self.lines[i];
        let (Some(text), Some(d)) = (slot.text.as_ref(), slot.display.as_ref()) else {
            return;
        };
        let h = slot.height;
        let clip = top.max(0.0)..(top + h).min(g.text_bottom);
        if clip.is_empty() {
            return;
        }
        let rows = clip.start as u32..clip.end.ceil() as u32;
        let pix = &mut self.frame.pixmap;
        fill(pix, 0.0, top, g.width, h, t.color(TokenId::ColorBg), &clip);

        let (cx, cw) = (g.col_x, g.col_w);
        match &d.deco {
            BlockDeco::CodeBg { unclosed, .. } => {
                fill(pix, cx, top, cw, h, t.color(TokenId::ColorCodeBg), &clip);
                if *unclosed {
                    let w = (t.px(TokenId::SizeUnclosedBar) * s).round();
                    let c = t.color(TokenId::ColorCodeUnclosed);
                    fill(pix, cx, top, w, h, c, &clip);
                }
            }
            BlockDeco::QuoteBars(n) => {
                let bar = (t.px(TokenId::SizeQuoteBar) * s).round();
                let step = bar + (t.px(TokenId::SpaceQuoteGap) * s).round();
                for k in 0..*n {
                    let x = cx + f32::from(k) * step;
                    fill(pix, x, top, bar, h, t.color(TokenId::ColorQuoteBar), &clip);
                }
            }
            BlockDeco::Rule => {
                let w = (t.px(TokenId::SizeRule) * s).round().max(1.0);
                let y = (top + (h - w) / 2.0).round();
                fill(pix, cx, y, cw, w, t.color(TokenId::ColorRule), &clip);
            }
            BlockDeco::Image { .. } => match &slot.image {
                Some(img) => {
                    paint_image(pix, t, cx, top, img, &clip);
                    if matches!(img.state, ImageState::Ready(_)) && !self.image_marked {
                        self.image_marked = true;
                        marks::mark(marks::names::IMAGES_PAINTED);
                    }
                }
                None => {
                    let c = t.color(TokenId::ColorImagePlaceholder);
                    fill(pix, cx, top, cw, h, c, &clip);
                }
            },
            BlockDeco::None | BlockDeco::TableRow { .. } => {}
        }

        let x0 = cx + slot.offset;
        let laid = &text.laid;
        let mut highlight = |r: Range<usize>, token: TokenId| {
            let r = src_to_display_clamped(d, r.start)..src_to_display_clamped(d, r.end);
            for rect in laid.range_rects(r) {
                let c = t.color(token);
                fill(pix, x0 + rect.x, top + rect.y, rect.w, rect.h, c, &clip);
            }
        };
        for m in src.matches(i) {
            highlight(m.clone(), TokenId::ColorSearchMatch);
        }
        if let Some(sel) = src.selection().and_then(|sel| sel.on_line(i, d.src_len)) {
            highlight(sel, TokenId::ColorSelection);
        }

        let (ox, oy) = (x0 as i32, top as i32);
        if let Some(p) = &slot.prefix {
            let px = cx as i32;
            self.layout
                .draw(&p.laid, &mut |m| blend(pix, px, oy, &m, &rows));
        }
        self.layout
            .draw(laid, &mut |m| blend(pix, ox, oy, &m, &rows));
        if let Some(l) = &slot.label {
            let lx = cx as i32;
            self.layout
                .draw(&l.laid, &mut |m| blend(pix, lx, oy, &m, &rows));
        }

        if self.caret.is_some_and(|(l, _)| l == i) {
            self.caret = None;
        }
        let cursor = src.cursor();
        if !src.reader() && cursor.line == i {
            let c = laid.caret(src_to_display_clamped(d, cursor.col));
            let rect = RectF {
                x: (x0 + c.x).round(),
                y: top + c.top,
                w: (t.px(TokenId::SizeCaret) * s).round().max(1.0),
                h: c.height,
            };
            let color = t.color(TokenId::ColorCaret);
            fill(pix, rect.x, rect.y, rect.w, rect.h, color, &clip);
            self.caret = Some((i, rect));
        }
    }
}

/// Pixels for `pt` points at `scale`.
fn to_px(pt: f32, scale: f32) -> u32 {
    (pt * scale).round().max(0.0) as u32
}

/// The rows of a line at `top`, `h` tall, inside the text area.
fn rows(top: f32, h: f32, text_bottom: f32) -> Range<u32> {
    let a = top.clamp(0.0, text_bottom);
    let b = (top + h).clamp(0.0, text_bottom);
    a as u32..b.ceil() as u32
}

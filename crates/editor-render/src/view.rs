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
//!
//! Blocks: code blocks sit on a `color.code.bg` box with `radius.code`
//! corners, their text `space.code.pad` in, the language label top right on
//! the opening fence and, when the block is unclosed, a `size.unclosed.bar`
//! accent on its last line; with highlighting on (`set_highlighting`) their
//! content is coloured as results land. Tables are a grid laid out a whole
//! table at a time (`table_paint`). Quotes draw one bar per depth. List
//! markers sit in the indent, which grows to fit a wide number. With
//! `size.hanging_prefix` above zero, list markers and revealed block
//! prefixes (`#`, `>`, list markers) hang in the left margin instead, so
//! revealing them does not move the text.
//!
//! Over the text: search matches in `color.search.match` (the current one in
//! `color.search.current`), the selection in `color.selection` (taking the
//! grapheme under its end, as Visual does), and the caret in `color.caret`,
//! a block one grapheme wide or a `size.caret` bar as `caret_shape` says.
//!
//! Fonts: a line that shaped a glyph as `.notdef` is laid out again when lazy
//! fallback lands a face for it (`poll_fonts`).

use std::ops::Range;
use std::path::Path;

use editor_core::buffer::Pos;
use editor_core::grapheme;
use editor_core::host::Waker;
use editor_core::markdown::{
    BlockDeco, BlockId, LineDisplay, Prefix, SpanBase, SpanStyle, display_to_src,
};
use editor_core::strings::IMAGE_NOT_FOUND;
use editor_core::tokens::{TokenId, Tokens};
use editor_core::view_model::{CaretShape, StatusModel, ViewSource};

use crate::code_track::CodeTracks;
use crate::damage::Damage;
use crate::frame::Frame;
use crate::images::{ImageSlot, ImageState, Images};
use crate::layout::{LaidLine, RectF, StyledSpan, TextLayout};
use crate::paint::{
    Corners, HangPrefix, apply_classes, blend, blit, code_ranges, content_offset, draw_scaled,
    fence_lang, fill, fill_rounded, hang_prefix, prefix_text, span_for, src_to_display_clamped,
    styled_line,
};
use crate::table_paint::{self, TableGeom};

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

/// A highlighted code line: its block, its line within the block's content,
/// and its spans before colouring (so landed colour can be applied without
/// the source).
struct CodeLine {
    id: BlockId,
    k: usize,
    base: Vec<StyledSpan>,
}

/// What `draw_line` last painted over a line's text, in display bytes (the
/// caret as source columns), so a recolour can repaint the line as it was.
#[derive(Default)]
struct Overlay {
    matches: Vec<Range<usize>>,
    /// The current match, over the others.
    current: Option<Range<usize>>,
    selection: Option<Range<usize>>,
    caret: Option<OverlayCaret>,
}

/// The caret as `draw_line` took it: the cursor's source column, the end
/// of the grapheme under it (what a block covers), and its shape.
#[derive(Clone, Copy)]
struct OverlayCaret {
    col: usize,
    end: usize,
    shape: CaretShape,
}

/// Per source line: its height in pixels (estimated until shaped), and once
/// shaped, its layouts and the display they came from.
struct LineSlot {
    height: f32,
    text: Option<Cached>,
    prefix: Option<Cached>,
    /// Where the prefix glyph draws, right of the column's left edge
    /// (negative when it hangs in the margin).
    prefix_dx: f32,
    /// A revealed block prefix hung in the left margin, and where it sits in
    /// the source. The display no longer holds it.
    hung: Option<(Cached, HangPrefix)>,
    display: Option<LineDisplay>,
    /// The text's offset right of the column's left edge, in pixels.
    offset: f32,
    /// Quote bars drawn left of the text.
    bars: u8,
    /// Set while the line is an image line and images are on. Kept when the
    /// layouts are forgotten, so a decode can still paint a line scrolled
    /// back into view.
    image: Option<ImageLine>,
    /// The placeholder or link text of an image line that has no pixels yet.
    label: Option<Cached>,
    /// The language label of an opening fence.
    code_label: Option<Cached>,
    code: Option<CodeLine>,
    /// For a rendered table row: the table's first line.
    table: Option<usize>,
    overlay: Overlay,
}

impl LineSlot {
    fn estimated(height: f32) -> LineSlot {
        LineSlot {
            height,
            text: None,
            prefix: None,
            prefix_dx: 0.0,
            hung: None,
            display: None,
            offset: 0.0,
            bars: 0,
            image: None,
            label: None,
            code_label: None,
            code: None,
            table: None,
            overlay: Overlay::default(),
        }
    }

    fn forget(&mut self) {
        self.text = None;
        self.prefix = None;
        self.hung = None;
        self.display = None;
        self.label = None;
        self.code_label = None;
        self.code = None;
        self.table = None;
        self.overlay = Overlay::default();
        // The resampled copy is box-sized: keep memory bounded by the screen.
        // The image line itself stays, so its height and a later decode survive.
        if let Some(img) = self.image.as_mut() {
            img.scaled = None;
            img.opaque = false;
        }
    }

    /// The line's own layouts (not a table's cells).
    fn layouts(&self) -> impl Iterator<Item = &Cached> {
        let hung = self.hung.as_ref().map(|(c, _)| c);
        [&self.text, &self.prefix, &self.label, &self.code_label]
            .into_iter()
            .flatten()
            .chain(hung)
    }

    /// Drops the line's layouts so the next relayout shapes them again,
    /// keeping its height, display and overlays.
    fn drop_layouts(&mut self) {
        self.text = None;
        self.prefix = None;
        self.hung = None;
        self.label = None;
        self.code_label = None;
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
    /// The line the cursor was on and the caret's shape when that line was
    /// last drawn (`None` until then), so a caret that moves or changes
    /// shape repaints its line, and one that stays put repaints nothing.
    cursor_drawn: Option<(usize, CaretShape)>,
    /// Present once `set_images` is called.
    images: Option<Images>,
    /// True once the first decoded image has been painted (and marked).
    image_marked: bool,
    /// Code block highlighting (off until `set_highlighting`).
    code: CodeTracks,
    /// Table geometry for the tables painted recently.
    tables: Vec<TableGeom>,
    /// Counts paints, so per-frame caches know when they are stale.
    frame_no: u64,
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
            cursor_drawn: None,
            images: None,
            image_marked: false,
            code: CodeTracks::new(),
            tables: Vec::new(),
            frame_no: 0,
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
            self.tables.clear();
            if old_scale > 0.0 {
                self.scroll = (self.scroll * scale / old_scale).round();
            }
        }
        self.last_status = None;
        self.caret = None;
        self.cursor_drawn = None;
        self.clear_frame();
    }

    /// Repaints everything: background, every on-screen line and the status
    /// bar. Unchanged lines reuse their layouts.
    pub fn paint_all(&mut self, src: &dyn ViewSource) {
        self.begin_frame();
        self.sync(src);
        let g = self.geo();
        self.caret = None;
        self.cursor_drawn = None;
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
        // Only the tables on screen keep their geometry.
        let f = self.frame_no;
        self.tables.retain(|t| t.frame == f);
        self.last_status = None;
        self.paint_status(src);
        self.damage.add(0..self.frame.height_px);
        self.layout.end_frame();
    }

    /// Repaints source lines `lines` after an edit, and the status bar when its
    /// model changed. A line whose height is unchanged damages only its own
    /// rows; a height change (or a change in the line count) repaints and
    /// damages from that line to the bottom of the text. The caret's old and
    /// new lines are repainted when the cursor changed line or the caret its
    /// shape.
    ///
    /// An edit in or next to a rendered table repaints the whole table: its
    /// column widths come from every row, so any row can move them all.
    pub fn paint_lines(&mut self, src: &dyn ViewSource, lines: Range<usize>) {
        self.begin_frame();
        let near_table: Vec<usize> = lines.clone().filter(|&i| self.was_near_table(i)).collect();
        let count_changed = self.sync(src);
        let count = self.lines.len();
        let first_edit = lines.start;
        let mut todo: Vec<usize> = lines.collect();
        for i in near_table {
            for j in i.saturating_sub(1)..(i + 2).min(count) {
                let d = src.display(j);
                if let Some((r, _)) = table_paint::extent(src, j, &d) {
                    todo.extend(r);
                }
            }
        }
        let cursor = src.cursor().line;
        let drawn = self.cursor_drawn;
        if drawn != Some((cursor, src.caret_shape())) {
            todo.extend(drawn.map(|(l, _)| l));
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
    /// the text area or over a line not painted yet. Over a rendered table
    /// row, the position is in the cell under the point.
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
                if let Some(col) = self.hit_table(i, x - g.col_x, (y - top).max(0.0)) {
                    return Some(Pos { line: i, col });
                }
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

    /// True when image line `i` holds its resampled copy of the image (it is
    /// on screen and decoded). Off-screen lines release it.
    pub fn has_scaled_image(&self, i: usize) -> bool {
        self.lines
            .get(i)
            .and_then(|s| s.image.as_ref())
            .is_some_and(|m| m.scaled.is_some())
    }

    /// The styled spans line `i` was last laid out with, while it is shaped
    /// (for tests and diagnostics).
    pub fn styled_spans(&self, i: usize) -> Option<&[StyledSpan]> {
        self.lines
            .get(i)?
            .text
            .as_ref()
            .map(|c| c.key.spans.as_slice())
    }

    /// Where line `i`'s text starts, in frame pixels from the left, while it
    /// is shaped.
    pub fn text_left_px(&self, i: usize) -> Option<f32> {
        let slot = self.lines.get(i)?;
        slot.text.as_ref()?;
        Some(self.geo().col_x + slot.offset)
    }

    /// Line `i`'s height in pixels: measured once shaped, estimated before.
    pub fn line_height_px(&self, i: usize) -> Option<f32> {
        self.lines.get(i).map(|s| s.height)
    }

    /// True when line `i` is shaped and some glyph on it (its text, prefix,
    /// labels or table cells) is `.notdef`.
    pub fn has_missing_glyphs(&self, i: usize) -> bool {
        let Some(slot) = self.lines.get(i) else {
            return false;
        };
        let own = slot.layouts().any(|c| c.laid.missing_glyphs);
        own || slot.table.is_some_and(|first| self.table_missing(first))
    }

    fn table_missing(&self, first: usize) -> bool {
        let mut t = self.tables.iter().filter(|g| g.lines.start == first);
        t.any(|g| g.missing_glyphs())
    }

    /// Lets lazy font fallback start looking up the codepoints no loaded face
    /// covers. Shells call it once the first frame is out, so no lookup (and
    /// no Core Text warm-up) lands before it.
    pub fn start_font_fallback(&mut self) {
        self.layout.start_font_fallback();
    }

    /// Loads faces that lazy fallback found, then lays out again and paints
    /// every on-screen line that showed `.notdef`. Each such line damages
    /// only its own rows; one whose height changed repaints from there to
    /// the bottom. Returns true when faces were loaded; the shell then copies
    /// `take_damage()` to the screen.
    pub fn poll_fonts(&mut self, src: &dyn ViewSource) -> bool {
        if !self.layout.poll_fonts() {
            return false;
        }
        self.begin_frame();
        if self.sync(src) {
            // The source changed shape without a paint: paint all of it.
            self.paint_all(src);
            return true;
        }
        let stale: Vec<usize> = self
            .tables
            .iter()
            .filter(|g| g.missing_glyphs())
            .map(|g| g.lines.start)
            .collect();
        self.tables.retain(|g| !stale.contains(&g.lines.start));
        let todo: Vec<usize> = (0..self.lines.len())
            .filter(|&i| {
                let s = &self.lines[i];
                s.layouts().any(|c| c.laid.missing_glyphs)
                    || s.table.is_some_and(|f| stale.contains(&f))
            })
            .collect();
        for &i in &todo {
            self.lines[i].drop_layouts();
        }
        let g = self.geo();
        for i in todo {
            let top = self.line_top(i);
            let before = self.lines[i].height;
            if top >= g.text_bottom || top + before <= 0.0 {
                self.lines[i].forget();
                continue;
            }
            if self.relayout(src, i) != before {
                self.repaint_to_bottom(src, i);
                break;
            }
            self.draw_line(src, i, top);
            self.damage.add(rows(top, before, g.text_bottom));
        }
        // The status bar keeps its layouts until its model changes: shape
        // and paint it again (its own rows) when one of them missed a glyph.
        if self.status_cache.iter().any(|c| c.laid.missing_glyphs) {
            self.status_cache.clear();
            self.last_status = None;
            self.paint_status(src);
        }
        self.layout.end_frame();
        true
    }

    /// Turns images on: local paths resolve against `base_dir`, and `waker`
    /// is called (from the worker thread) after each decode. Call before the
    /// first `paint_all`; lines painted earlier pick images up when next laid out.
    pub fn set_images(&mut self, base_dir: &Path, waker: Waker) {
        self.images = Some(Images::new(base_dir, waker));
    }

    /// Turns code block highlighting on: `waker` is called (from the worker
    /// thread) when a block's colour is ready, and the shell then calls
    /// `poll_async`. Call before the first `paint_all`.
    pub fn set_highlighting(&mut self, waker: Waker) {
        self.code.enable(waker);
    }

    /// Takes finished work from the workers and paints it without the
    /// source: image decodes into the space their lines already reserved, and
    /// code colour onto lines that keep their metrics. No relayout of other
    /// lines. Returns true when rows were damaged; the shell then copies
    /// `take_damage()` to the screen.
    pub fn poll_async(&mut self) -> bool {
        let images = self.poll_images();
        let code = self.poll_highlight();
        images || code
    }

    fn poll_images(&mut self) -> bool {
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
            }
            let h = self.lines[i].height;
            let top = self.line_top(i);
            if top >= g.text_bottom || top + h <= 0.0 {
                continue;
            }
            if let Some(img) = self.lines[i].image.as_mut() {
                img.ensure_scaled();
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

    /// Colours the on-screen lines of blocks whose highlight landed.
    fn poll_highlight(&mut self) -> bool {
        let g = self.geo();
        let mut any = false;
        for (id, range) in self.code.poll() {
            for i in range {
                let Some(slot) = self.lines.get_mut(i) else {
                    break;
                };
                let (Some(cl), Some(cached)) = (&slot.code, &slot.text) else {
                    continue;
                };
                if cl.id != id {
                    continue;
                }
                let classes = self.code.line_classes(id, cl.k);
                let (text, width) = (cached.key.text.clone(), cached.key.width);
                let spans = apply_classes(&text, cl.base.clone(), &classes, &self.tokens);
                lay(self.layout.as_mut(), &mut slot.text, text, spans, width);
                let h = slot.height;
                let top = self.line_top(i);
                if top >= g.text_bottom || top + h <= 0.0 {
                    continue;
                }
                self.paint_line(i, top);
                self.damage.add(rows(top, h, g.text_bottom));
                any = true;
            }
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

    fn begin_frame(&mut self) {
        self.frame_no += 1;
        self.code.begin_frame();
    }

    /// True when line `i`, or a line next to it, was a rendered table row at
    /// the last paint (from the displays kept for on-screen lines).
    fn was_near_table(&self, i: usize) -> bool {
        (i.saturating_sub(1)..i + 2).any(|j| {
            self.lines
                .get(j)
                .and_then(|s| s.display.as_ref())
                .is_some_and(|d| matches!(d.deco, BlockDeco::TableRow { .. }))
        })
    }

    /// Takes the source's tokens and line count. Returns true when the line
    /// count changed.
    fn sync(&mut self, src: &dyn ViewSource) -> bool {
        if *src.tokens() != self.tokens {
            // Table geometry bakes cell colours, fonts and widths in.
            self.tables.clear();
            self.tokens = src.tokens().clone();
        }
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

    /// The table geometry holding line `i` (displayed as `d`), checked
    /// against the source once per frame and laid out again when any row,
    /// or the column, changed.
    fn ensure_table(&mut self, src: &dyn ViewSource, i: usize, d: &LineDisplay) -> Option<usize> {
        let f = self.frame_no;
        if let Some(k) = self
            .tables
            .iter()
            .position(|g| g.frame == f && g.lines.contains(&i))
        {
            return Some(k);
        }
        let (lines, displays) = table_paint::extent(src, i, d)?;
        let (t, s, avail) = (&self.tokens, self.frame.scale, self.geo().col_w);
        let key = table_paint::key_of(src, &lines, avail, t, s);
        if let Some(k) = self
            .tables
            .iter()
            .position(|g| g.lines == lines && g.matches(&key))
        {
            self.tables[k].frame = f;
            return Some(k);
        }
        let overlaps = |g: &TableGeom| g.lines.start < lines.end && lines.start < g.lines.end;
        let old = self.tables.iter().position(overlaps);
        let old = old.map(|k| self.tables.swap_remove(k));
        self.tables.retain(|g| !overlaps(g));
        let layout = self.layout.as_mut();
        let mut geom = table_paint::build(layout, src, lines, &displays, avail, t, s, old);
        geom.frame = f;
        self.tables.push(geom);
        Some(self.tables.len() - 1)
    }

    /// The source column under `x, y` (pixels from the column's left edge and
    /// the line's top) when line `i` is a rendered table row.
    fn hit_table(&self, i: usize, x: f32, y: f32) -> Option<usize> {
        let first = self.lines.get(i)?.table?;
        let geom = self
            .tables
            .iter()
            .find(|g| g.lines.start == first && g.lines.contains(&i))?;
        let row = geom.row(i)?;
        let (t, s) = (&self.tokens, self.frame.scale);
        let k = geom
            .verticals
            .windows(2)
            .position(|w| x < w[1])
            .unwrap_or(geom.verticals.len().saturating_sub(2))
            .min(row.cells.len().checked_sub(1)?);
        let cell = &row.cells[k];
        let (cx, cy) = geom.cell_origin(i, k, t, s);
        let b = cell.laid()?.hit(x - cx, (y - cy).max(0.0));
        Some(display_to_src(&cell.display, b))
    }

    /// Lays line `i` out from the source and returns its height.
    fn relayout(&mut self, src: &dyn ViewSource, i: usize) -> f32 {
        let mut d = src.display(i);
        let table = match d.deco {
            BlockDeco::TableRow { .. } => self.ensure_table(src, i, &d),
            _ => None,
        };
        let g = self.geo();
        let t = &self.tokens;
        let s = self.frame.scale;
        let line = src.line(i);
        let layout = self.layout.as_mut();
        let slot = &mut self.lines[i];
        slot.code = None;
        slot.table = None;
        slot.bars = 0;
        slot.prefix_dx = 0.0;

        if let Some(geom) = table.map(|k| &self.tables[k]) {
            let style = SpanStyle::plain(SpanBase::Body);
            let empty = vec![span_for(&style, 0..0, t, s)];
            lay(layout, &mut slot.text, String::new(), empty, g.col_w);
            slot.height = geom.row(i).map_or(0.0, |r| r.height);
            slot.offset = 0.0;
            slot.prefix = None;
            slot.hung = None;
            slot.label = None;
            slot.code_label = None;
            slot.image = None;
            slot.table = Some(geom.lines.start);
            slot.display = Some(d);
            return slot.height;
        }

        // A revealed block prefix hangs in the margin: take it out of the display.
        let hanging = t.px(TokenId::SizeHangingPrefix) > 0.0;
        let gap = (t.px(TokenId::SpacePrefixGap) * s).round();
        let mut hung = None;
        if hanging && d.deco == BlockDeco::None && d.prefix == Prefix::None {
            let first = d.spans.first().cloned();
            if let (Some(hp), Some(first)) = (hang_prefix(line), first)
                && first.style.base == SpanBase::Syntax
                && first.src.start == 0
                && first.src.end >= hp.end
                && hp.end > hp.start
            {
                d.spans[0].src.start = hp.end;
                if d.spans[0].src.is_empty() {
                    d.spans.remove(0);
                }
                hung = Some(hp);
            }
        }
        let old_hung = slot.hung.take().map(|(c, _)| c);
        slot.hung = hung.map(|hp| {
            let text = line[hp.start..hp.end].trim_end().to_owned();
            let span = span_for(&SpanStyle::plain(SpanBase::Syntax), 0..text.len(), t, s);
            let mut c = old_hung;
            lay(layout, &mut c, text, vec![span], g.col_w);
            (c.expect("just laid"), hp)
        });

        let is_code = matches!(d.deco, BlockDeco::CodeBg { .. });
        let code_pad = (t.px(TokenId::SpaceCodePad) * s).round();
        slot.bars = match (&d.deco, hung) {
            (BlockDeco::QuoteBars(n), _) => *n,
            (_, Some(hp)) => hp.quote_depth,
            _ => 0,
        };
        let mut offset = if is_code {
            code_pad
        } else {
            content_offset(&Prefix::QuoteBars(slot.bars), t, s).round()
        };
        match prefix_text(&d.prefix) {
            Some(p) => {
                let style = SpanStyle::plain(SpanBase::Body);
                let span = span_for(&style, 0..p.len(), t, s);
                // Laid out the column's width, so a wide number never wraps.
                let pw = lay(layout, &mut slot.prefix, p, vec![span], g.col_w)
                    .width()
                    .ceil();
                if hanging {
                    slot.prefix_dx = -(gap + pw);
                } else {
                    let indent = content_offset(&d.prefix, t, s).round();
                    offset = indent.max(pw + gap);
                }
            }
            None => slot.prefix = None,
        }
        let inset = if is_code { 2.0 * code_pad } else { offset };
        let width = (g.col_w - inset).max(1.0);

        let (text, mut spans) = styled_line(line, &d, t, s);
        let content = !d.spans.is_empty()
            && d.spans
                .iter()
                .all(|sp| matches!(sp.style.base, SpanBase::CodeBlock { .. }));
        if is_code
            && content
            && let Some((id, k, classes)) = self.code.classes(src, i, &d)
        {
            let base = spans.clone();
            spans = apply_classes(&text, spans, &classes, t);
            slot.code = Some(CodeLine { id, k, base });
        }
        let laid = lay(layout, &mut slot.text, text, spans, width);
        slot.height = laid.height.ceil();
        slot.offset = offset;

        let lang = match d.deco {
            BlockDeco::CodeBg { first: true, .. } if d.spans.is_empty() => fence_lang(line),
            _ => "",
        };
        if lang.is_empty() {
            slot.code_label = None;
        } else {
            let mut span = span_for(&SpanStyle::plain(SpanBase::Syntax), 0..lang.len(), t, s);
            span.size_px = t.px(TokenId::FontSizeCodeLabel) * s;
            span.color = t.color(TokenId::ColorCodeLabel);
            lay(
                layout,
                &mut slot.code_label,
                lang.to_owned(),
                vec![span],
                g.col_w,
            );
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
        if self.cursor_drawn.is_some_and(|(l, _)| l >= first) {
            self.cursor_drawn = None;
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

    /// Paints line `i` (already laid out) with its top at `top`, taking the
    /// search matches, selection and caret from the source.
    fn draw_line(&mut self, src: &dyn ViewSource, i: usize, top: f32) {
        let Some(d) = self.lines[i].display.as_ref() else {
            return;
        };
        let map =
            |r: &Range<usize>| src_to_display_clamped(d, r.start)..src_to_display_clamped(d, r.end);
        let cursor = src.cursor();
        let shape = src.caret_shape();
        let line = src.line(i);
        let caret = (!src.reader() && cursor.line == i && shape != CaretShape::Hidden).then(|| {
            let col = grapheme::floor(line, cursor.col);
            OverlayCaret {
                col,
                end: grapheme::next(line, col),
                shape,
            }
        });
        let overlay = Overlay {
            matches: src.matches(i).iter().map(map).collect(),
            current: src
                .current_match()
                .filter(|(l, _)| *l == i)
                .map(|(_, r)| map(&r)),
            selection: src
                .selection()
                .and_then(|sel| sel.on_line(i, line))
                .map(|r| map(&r)),
            caret,
        };
        if cursor.line == i {
            self.cursor_drawn = Some((i, shape));
        }
        self.lines[i].overlay = overlay;
        self.paint_line(i, top);
    }

    /// Paints line `i` with its top at `top`: background, block decoration,
    /// inline code backgrounds, search matches (the current one in
    /// `color.search.current`), selection, a block caret (under the text, so
    /// the grapheme shows through), prefixes, text and a bar caret (the
    /// overlays as `draw_line` last took them).
    fn paint_line(&mut self, i: usize, top: f32) {
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
        if self.caret.is_some_and(|(l, _)| l == i) {
            self.caret = None;
        }

        let (cx, cw) = (g.col_x, g.col_w);
        if let Some(first) = slot.table {
            if let Some(geom) = self
                .tables
                .iter()
                .find(|g| g.lines.start == first && g.lines.contains(&i))
            {
                let layout = self.layout.as_mut();
                table_paint::paint_row(pix, layout, geom, i, cx, top, t, s, &clip);
            }
            return;
        }
        for k in 0..slot.bars {
            let bar = (t.px(TokenId::SizeQuoteBar) * s).round();
            let step = bar + (t.px(TokenId::SpaceQuoteGap) * s).round();
            let x = cx + f32::from(k) * step;
            fill(pix, x, top, bar, h, t.color(TokenId::ColorQuoteBar), &clip);
        }
        match &d.deco {
            BlockDeco::CodeBg {
                first,
                last,
                unclosed,
            } => {
                let r = (t.px(TokenId::RadiusCode) * s).round();
                let corners = Corners {
                    radius: r,
                    top: *first,
                    bottom: *last,
                };
                let b = RectF {
                    x: cx,
                    y: top,
                    w: cw,
                    h,
                };
                fill_rounded(pix, b, corners, t.color(TokenId::ColorCodeBg), &clip);
                if *unclosed && *last {
                    // Clear of the rounded corner below it.
                    let w = (t.px(TokenId::SizeUnclosedBar) * s).round();
                    let c = t.color(TokenId::ColorCodeUnclosed);
                    let y = if *first { top + r } else { top };
                    fill(pix, cx, y, w, top + h - r - y, c, &clip);
                }
                if let Some(l) = &slot.code_label {
                    let pad = (t.px(TokenId::SpaceCodePad) * s).round();
                    let lx = (cx + cw - pad - l.laid.width()).round() as i32;
                    let ly = (top + ((h - l.laid.height) / 2.0).floor()) as i32;
                    self.layout
                        .draw(&l.laid, &mut |m| blend(pix, lx, ly, &m, &rows));
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
            BlockDeco::None | BlockDeco::QuoteBars(_) | BlockDeco::TableRow { .. } => {}
        }

        let x0 = cx + slot.offset;
        let laid = &text.laid;
        let mut highlight = |r: &Range<usize>, token: TokenId| {
            for rect in laid.range_rects(r.clone()) {
                let c = t.color(token);
                fill(pix, x0 + rect.x, top + rect.y, rect.w, rect.h, c, &clip);
            }
        };
        for r in code_ranges(d) {
            highlight(&r, TokenId::ColorCodeBg);
        }
        for m in &slot.overlay.matches {
            highlight(m, TokenId::ColorSearchMatch);
        }
        if let Some(m) = &slot.overlay.current {
            highlight(m, TokenId::ColorSearchCurrent);
        }
        if let Some(sel) = &slot.overlay.selection {
            highlight(sel, TokenId::ColorSelection);
        }

        let gap = (t.px(TokenId::SpacePrefixGap) * s).round();
        let hung_x = slot
            .hung
            .as_ref()
            .map(|(c, _)| (cx - gap - c.laid.width().ceil()).max(0.0));
        let caret_rect = slot.overlay.caret.map(|c| {
            // In the hung prefix, or the main text.
            let (laid_c, x, from, to) = match (&slot.hung, hung_x) {
                (Some((h, hp)), Some(hx)) if c.col < hp.end => (
                    &h.laid,
                    hx,
                    c.col.saturating_sub(hp.start),
                    c.end.min(hp.end).saturating_sub(hp.start),
                ),
                _ => (
                    laid,
                    x0,
                    src_to_display_clamped(d, c.col),
                    src_to_display_clamped(d, c.end),
                ),
            };
            // On the main text's row, wherever it sits across.
            let main = laid.caret(src_to_display_clamped(d, c.col));
            let at = laid_c.caret(from);
            let w = match c.shape {
                CaretShape::Block => laid_c
                    .range_rects(from..to)
                    .first()
                    .map(|r| r.w)
                    // Past the last grapheme (an empty line).
                    .unwrap_or_else(|| (t.px(TokenId::SizeCaretBlockEmpty) * s).round()),
                _ => (t.px(TokenId::SizeCaret) * s).round().max(1.0),
            };
            RectF {
                x: (x + at.x).round(),
                y: top + main.top,
                w: w.max(1.0),
                h: main.height,
            }
        });
        if let (Some(c), Some(r)) = (slot.overlay.caret, caret_rect)
            && c.shape == CaretShape::Block
        {
            fill(pix, r.x, r.y, r.w, r.h, t.color(TokenId::ColorCaret), &clip);
        }
        let oy = top as i32;
        if let Some(p) = &slot.prefix {
            let px = (cx + slot.prefix_dx).max(0.0) as i32;
            self.layout
                .draw(&p.laid, &mut |m| blend(pix, px, oy, &m, &rows));
        }
        if let (Some((c, _)), Some(hx)) = (&slot.hung, hung_x) {
            // On the main text's baseline when the line mixes sizes.
            let dy = laid
                .row_geometry()
                .first()
                .zip(c.laid.row_geometry().first())
                .map_or(0.0, |(a, b)| (a.height - b.height).max(0.0) / 2.0);
            let (hx, hy) = (hx as i32, (top + dy.floor()) as i32);
            self.layout
                .draw(&c.laid, &mut |m| blend(pix, hx, hy, &m, &rows));
        }
        let ox = x0 as i32;
        self.layout
            .draw(laid, &mut |m| blend(pix, ox, oy, &m, &rows));
        if let Some(l) = &slot.label {
            let lx = cx as i32;
            self.layout
                .draw(&l.laid, &mut |m| blend(pix, lx, oy, &m, &rows));
        }

        if let (Some(c), Some(rect)) = (slot.overlay.caret, caret_rect) {
            if c.shape == CaretShape::Bar {
                let color = t.color(TokenId::ColorCaret);
                fill(pix, rect.x, rect.y, rect.w, rect.h, color, &clip);
            }
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

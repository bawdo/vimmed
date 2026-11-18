//! The status bar, monospace at `font.size.status` on `color.status.bg`.
//! On the left, what `StatusModel::left` holds (the mode, a message in the
//! colour of its kind, a prompt, or the command or search line typed in
//! place with a `size.caret` caret), then the file name, `changes not
//! saved` in `color.status.dirty` and any disk warnings in
//! `color.status.warning`. On the right, the pending keys, `line:col` and
//! at the far end a page icon (an outlined page with lines of text, drawn
//! here rather than taken from a font) that a click on toggles reader view,
//! in `color.status.text`, or `color.status.reader` while reader view is
//! on.
//! A prompt or message too long for the room left of the right side keeps
//! its start and end and loses its middle to an ellipsis, so the open-link
//! question still shows the target's scheme or path head, its file name and
//! the answers.
//! It repaints only when its model changed, and damages only its own rows.
//!
//! Its layouts are cached by text and spans, and the spans carry every
//! token they use, so a theme change (which calls `paint_all`, clearing
//! `last_status`) lays out afresh.

use std::ops::Range;

use editor_core::markdown::{SpanBase, SpanStyle};
use editor_core::strings::CHANGES_NOT_SAVED;
use editor_core::tokens::{Rgba, TokenId};
use editor_core::view_model::{MsgKind, StatusLeft, ViewSource};

use tiny_skia::Pixmap;

use crate::layout::{FontRole, RectF, StyledSpan};
use crate::paint::{blend, fill, span_for};
use crate::view::{Cached, DocView, lay};

/// A wrap width no status item reaches: an elided item is laid out on one
/// row so its whole width can be measured.
const NO_WRAP: f32 = 1.0e7;

/// The ellipsis that stands for the chars an elided item leaves out.
const ELLIPSIS: char = '\u{2026}';

/// `text` cut to `keep` chars with an ellipsis in place of its middle:
/// two fifths of them from its start (a path's head, or a URL's scheme and
/// host) and the rest from its end (a file name, and a question's
/// answers). `None` when it already fits, or too little would be left.
fn elide_middle(text: &str, keep: usize) -> Option<String> {
    let n = text.chars().count();
    if keep >= n || keep < 2 {
        return None;
    }
    let head = keep * 2 / 5;
    let tail = keep - head;
    let mut out: String = text.chars().take(head).collect();
    out.push(ELLIPSIS);
    out.extend(text.chars().skip(n - tail));
    Some(out)
}

/// The colour a message of `kind` shows in.
fn message_color(kind: MsgKind) -> TokenId {
    match kind {
        MsgKind::Info => TokenId::ColorStatusText,
        MsgKind::Warning => TokenId::ColorStatusWarning,
        MsgKind::Error => TokenId::ColorStatusError,
    }
}

/// The page icon's box, in frame pixels: `font_px` (the status font's
/// size) tall and a little narrower, at the bar's right end inside its
/// padding, centred on the bar. `None` when the bar is too narrow for it.
fn page_icon_box(width: f32, top: f32, bar_h: f32, pad: f32, font_px: f32) -> Option<RectF> {
    let h = font_px.round().min(bar_h).max(5.0);
    let w = (h * 0.78).round();
    let x = width - pad - w;
    if x < pad {
        return None;
    }
    Some(RectF {
        x,
        y: (top + (bar_h - h) / 2.0).round(),
        w,
        h,
    })
}

/// Draws the page icon in `b`: its outline `stroke` thick and three lines
/// of text inside, the last one shorter.
fn draw_page_icon(pix: &mut Pixmap, b: RectF, stroke: f32, c: Rgba, clip: &Range<f32>) {
    fill(pix, b.x, b.y, b.w, stroke, c, clip);
    fill(pix, b.x, b.y + b.h - stroke, b.w, stroke, c, clip);
    fill(pix, b.x, b.y, stroke, b.h, c, clip);
    fill(pix, b.x + b.w - stroke, b.y, stroke, b.h, c, clip);
    let inset = stroke + (b.w * 0.2).round().max(1.0);
    let full = b.w - 2.0 * inset;
    if full < 1.0 {
        return;
    }
    let step = (b.h - 2.0 * stroke) / 4.0;
    for k in 1..=3 {
        let w = if k == 3 { (full * 0.6).round() } else { full };
        let y = (b.y + stroke + step * k as f32 - stroke / 2.0).round();
        fill(pix, b.x + inset, y, w, stroke, c, clip);
    }
}

impl DocView {
    /// Repaints the status bar when its model changed since the last paint
    /// (`last_status` is cleared to force it).
    pub(crate) fn paint_status(&mut self, src: &dyn ViewSource) {
        let model = src.status();
        if self.last_status.as_ref() == Some(&model) {
            return;
        }
        let g = self.geo();
        let t = &self.tokens;
        let s = self.frame.scale;
        let top = g.text_bottom;
        let bar_h = g.height - top;
        let clip = top..g.height;
        if bar_h <= 0.0 {
            self.reader_icon = None;
            self.last_status = Some(model);
            return;
        }
        fill(
            &mut self.frame.pixmap,
            0.0,
            top,
            g.width,
            bar_h,
            t.color(TokenId::ColorStatusBg),
            &clip,
        );

        let text_color = TokenId::ColorStatusText;
        // The left side, in order, and where the typed line's caret goes
        // in the first item.
        let mut caret = None;
        // A prompt or message is shortened in its middle to fit.
        let elide = matches!(
            model.left,
            StatusLeft::Prompt(_) | StatusLeft::Message { .. }
        );
        let first = match &model.left {
            StatusLeft::Mode(m) => (m.to_string(), text_color),
            StatusLeft::Input {
                prefix,
                text,
                cursor,
            } => {
                caret = Some(prefix.len() + (*cursor).min(text.len()));
                (format!("{prefix}{text}"), text_color)
            }
            StatusLeft::Prompt(p) => (p.clone(), text_color),
            StatusLeft::Message { text, kind } => (text.clone(), message_color(*kind)),
        };
        let mut left = vec![first, (model.file.clone(), text_color)];
        if model.dirty {
            left.push((CHANGES_NOT_SAVED.to_string(), TokenId::ColorStatusDirty));
        }
        left.extend(
            model
                .warnings
                .iter()
                .map(|w| (w.to_string(), TokenId::ColorStatusWarning)),
        );
        let right = [
            (model.showcmd.clone(), text_color),
            (model.position.clone(), text_color),
        ];
        let n_left = left.len();

        // Lay every item out first: the right side is placed from its width.
        let mut old = std::mem::take(&mut self.status_cache);
        let mut slots: Vec<(Option<Cached>, usize)> = Vec::new();
        let mono = |text: &str, color: TokenId| {
            let mut sp = span_for(&SpanStyle::plain(SpanBase::Body), 0..text.len(), t, s);
            sp.role = FontRole::Mono;
            sp.size_px = t.px(TokenId::FontSizeStatus) * s;
            sp.color = t.color(color);
            vec![sp]
        };
        let mut first_color = text_color;
        for (k, (text, color)) in left.into_iter().chain(right).enumerate() {
            if text.is_empty() {
                continue;
            }
            // An elided item is laid on one row, to be measured whole.
            let wrap = if k == 0 && elide { NO_WRAP } else { g.width };
            if k == 0 {
                first_color = color;
            }
            let spans: Vec<StyledSpan> = mono(&text, color);
            let mut slot = old
                .iter()
                .position(|c| c.key_matches(&text, &spans, wrap))
                .map(|p| old.swap_remove(p));
            lay(self.layout.as_mut(), &mut slot, text, spans, wrap);
            slots.push((slot, k));
        }

        let pad = (t.px(TokenId::SpaceStatusPad) * s).round();
        let width = |c: &Option<Cached>| c.as_ref().map_or(0.0, |c| c.laid.width());
        // The page icon takes the right end; the other items keep clear.
        let icon = page_icon_box(g.width, top, bar_h, pad, t.px(TokenId::FontSizeStatus) * s);
        let icon_room = icon.map_or(0.0, |b| b.w + pad);
        // The room a prompt or message has: the bar less its padding, the
        // right side and the left items after it (the file name, `changes
        // not saved`, warnings). When those would leave it less than half
        // the bar, they make way while it shows: the question comes first.
        if elide && let Some(i) = slots.iter().position(|(_, k)| *k == 0) {
            let sum = |slots: &[(Option<Cached>, usize)], f: &dyn Fn(usize) -> bool| -> f32 {
                slots
                    .iter()
                    .filter(|(_, k)| f(*k))
                    .map(|(c, _)| width(c) + pad)
                    .sum()
            };
            let right_w = sum(&slots, &|k| k >= n_left);
            let others_w = sum(&slots, &|k| k > 0 && k < n_left);
            let full = (g.width - 2.0 * pad - right_w - icon_room).max(0.0);
            let first_w = width(&slots[i].0);
            let mut room = (full - others_w).max(0.0);
            if first_w > room && room < full / 2.0 {
                for (c, k) in slots.iter_mut() {
                    if *k > 0 && *k < n_left {
                        *c = None;
                    }
                }
                room = full;
            }
            let mut slot = slots[i].0.take();
            for _ in 0..4 {
                let Some(c) = slot.as_ref() else { break };
                let (w, text) = (c.laid.width(), c.key_text().to_owned());
                if w <= room {
                    break;
                }
                let n = text.chars().count();
                let keep = ((n as f32 * room / w).floor() as usize).saturating_sub(1);
                let Some(short) = elide_middle(&text, keep) else {
                    break;
                };
                let spans = mono(&short, first_color);
                lay(self.layout.as_mut(), &mut slot, short, spans, NO_WRAP);
            }
            slots[i].0 = slot;
        }
        let mut right_x = g.width
            - pad
            - icon_room
            - slots
                .iter()
                .filter(|(_, k)| *k >= n_left)
                .map(|(c, _)| width(c) + pad)
                .sum::<f32>()
            + pad;
        let mut x = pad;
        // A left item after the first that would run into the right side
        // is not painted (nor any after it).
        let right_start = right_x;
        let mut left_full = false;
        let mut items = Vec::new();
        let rows = top as u32..self.frame.height_px;
        let caret_w = (t.px(TokenId::SizeCaret) * s).round().max(1.0);
        let caret_color = t.color(TokenId::ColorCaret);
        for (slot, k) in &slots {
            let Some(c) = slot else { continue };
            let laid = &c.laid;
            let w = laid.width();
            let px = if *k >= n_left {
                let at = right_x;
                right_x += w + pad;
                at
            } else {
                left_full |= *k > 0 && x + w > right_start - pad;
                if left_full {
                    continue;
                }
                let at = x;
                x += w + pad;
                at
            };
            items.push((c.key_text().to_owned(), px..px + w));
            let y = (top + (bar_h - laid.height) / 2.0).round();
            let pix = &mut self.frame.pixmap;
            let (ox, oy) = (px.round() as i32, y as i32);
            self.layout
                .draw(laid, &mut |m| blend(pix, ox, oy, &m, &rows));
            if *k == 0
                && let Some(byte) = caret
            {
                let cx = (px + laid.caret(byte).x).round();
                fill(pix, cx, y, caret_w, laid.height, caret_color, &clip);
            }
        }
        self.reader_icon = icon.map(|b| {
            let color = if model.reader {
                TokenId::ColorStatusReader
            } else {
                text_color
            };
            let stroke = s.round().max(1.0);
            draw_page_icon(&mut self.frame.pixmap, b, stroke, t.color(color), &clip);
            // Clicks land on the bar's height across it and half the
            // padding either side.
            RectF {
                x: b.x - pad / 2.0,
                y: top,
                w: b.w + pad,
                h: bar_h,
            }
        });
        self.status_items = items;
        self.status_cache
            .extend(slots.into_iter().filter_map(|(c, _)| c));
        self.damage.add(rows);
        self.last_status = Some(model);
    }
}

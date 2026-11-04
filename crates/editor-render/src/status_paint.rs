//! The status bar, monospace at `font.size.status` on `color.status.bg`.
//! On the left, what `StatusModel::left` holds (the mode, a message in the
//! colour of its kind, a prompt, or the command or search line typed in
//! place with a `size.caret` caret), then the file name, `changes not
//! saved` in `color.status.dirty` and any disk warnings in
//! `color.status.warning`. On the right, the pending keys and `line:col`.
//! It repaints only when its model changed, and damages only its own rows.
//!
//! Its layouts are cached by text and spans, and the spans carry every
//! token they use, so a theme change (which calls `paint_all`, clearing
//! `last_status`) lays out afresh.

use editor_core::markdown::{SpanBase, SpanStyle};
use editor_core::strings::CHANGES_NOT_SAVED;
use editor_core::tokens::TokenId;
use editor_core::view_model::{MsgKind, StatusLeft, ViewSource};

use crate::layout::{FontRole, StyledSpan};
use crate::paint::{blend, fill, span_for};
use crate::view::{Cached, DocView, lay};

/// The colour a message of `kind` shows in.
fn message_color(kind: MsgKind) -> TokenId {
    match kind {
        MsgKind::Info => TokenId::ColorStatusText,
        MsgKind::Warning => TokenId::ColorStatusWarning,
        MsgKind::Error => TokenId::ColorStatusError,
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
        let first = match &model.left {
            StatusLeft::Mode(m) => (m.to_string(), text_color),
            StatusLeft::Input {
                prefix,
                text,
                cursor,
            } => {
                caret = Some(prefix.len_utf8() + (*cursor).min(text.len()));
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
        for (k, (text, color)) in left.into_iter().chain(right).enumerate() {
            if text.is_empty() {
                continue;
            }
            let mut sp = span_for(&SpanStyle::plain(SpanBase::Body), 0..text.len(), t, s);
            sp.role = FontRole::Mono;
            sp.size_px = t.px(TokenId::FontSizeStatus) * s;
            sp.color = t.color(color);
            let spans: Vec<StyledSpan> = vec![sp];
            let mut slot = old
                .iter()
                .position(|c| c.key_matches(&text, &spans, g.width))
                .map(|p| old.swap_remove(p));
            lay(self.layout.as_mut(), &mut slot, text, spans, g.width);
            slots.push((slot, k));
        }

        let pad = (t.px(TokenId::SpaceStatusPad) * s).round();
        let width = |c: &Option<Cached>| c.as_ref().map_or(0.0, |c| c.laid.width());
        let mut right_x = g.width
            - pad
            - slots
                .iter()
                .filter(|(_, k)| *k >= n_left)
                .map(|(c, _)| width(c) + pad)
                .sum::<f32>()
            + pad;
        let mut x = pad;
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
                let at = x;
                x += w + pad;
                at
            };
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
        self.status_cache
            .extend(slots.into_iter().filter_map(|(c, _)| c));
        self.damage.add(rows);
        self.last_status = Some(model);
    }
}

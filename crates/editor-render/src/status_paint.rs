//! The status bar: the mode, the file name, `changes not saved` in
//! `color.status.dirty` when dirty, and `line:col` right-aligned. It damages
//! only its own rows. Task 26 completes it.

use editor_core::markdown::{SpanBase, SpanStyle};
use editor_core::tokens::TokenId;
use editor_core::view_model::ViewSource;

use crate::layout::StyledSpan;
use crate::paint::{blend, fill, span_for};
use crate::view::{DocView, lay};

/// Shown after the file name while there are changes not saved.
const DIRTY_TEXT: &str = "changes not saved";

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

        let span = |len: usize, color: TokenId| -> StyledSpan {
            let mut sp = span_for(&SpanStyle::plain(SpanBase::Body), 0..len, t, s);
            sp.size_px = t.px(TokenId::FontSizeStatus) * s;
            sp.color = t.color(color);
            sp
        };
        let mut left = vec![
            (model.mode.to_string(), TokenId::ColorStatusText),
            (model.file.clone(), TokenId::ColorStatusText),
        ];
        if model.dirty {
            left.push((DIRTY_TEXT.to_string(), TokenId::ColorStatusDirty));
        }
        let right = (model.position.clone(), TokenId::ColorStatusText);

        let pad = (t.px(TokenId::SpaceStatusPad) * s).round();
        let mut old = std::mem::take(&mut self.status_cache);
        let mut x = pad;
        let rows = top as u32..self.frame.height_px;
        let n_left = left.len();
        for (k, (text, color)) in left.into_iter().chain([right]).enumerate() {
            if text.is_empty() {
                continue;
            }
            let spans = vec![span(text.len(), color)];
            let reuse = old
                .iter()
                .position(|c| c.key_matches(&text, &spans, g.width))
                .map(|p| old.swap_remove(p));
            let mut slot = reuse;
            let laid = lay(self.layout.as_mut(), &mut slot, text, spans, g.width);
            let w = laid.width();
            let px = if k == n_left { g.width - pad - w } else { x };
            let y = (top + (bar_h - laid.height) / 2.0).round();
            let pix = &mut self.frame.pixmap;
            let (ox, oy) = (px.round() as i32, y as i32);
            self.layout
                .draw(laid, &mut |m| blend(pix, ox, oy, &m, &rows));
            x += w + pad;
            self.status_cache.extend(slot);
        }
        self.damage.add(rows);
        self.last_status = Some(model);
    }
}

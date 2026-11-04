//! The help overlay `:help` shows: `editor_core::help::rows`, from the
//! source's first row down, over the whole text area on `color.overlay.bg`
//! in `color.overlay.text`. The text sits in the column, `space.code.pad`
//! in: the title as an h2, section titles strong, each key in mono at
//! `font.size` with its description in the body font after the widest key
//! (`space.code.pad` apart), a `space.paragraph` gap between sections and
//! the footer in italics. The status bar stays as it is.
//!
//! It is painted after the lines, and again after any later paint of lines
//! under it, so it stays on top. Its layouts are cached by text and spans.

use editor_core::help::{self, HelpRow};
use editor_core::markdown::{SpanBase, SpanStyle};
use editor_core::tokens::TokenId;

use crate::layout::StyledSpan;
use crate::paint::{blend, fill, span_for};
use crate::view::{Cached, DocView, lay};

/// The least width a key's description wraps to, in multiples of
/// `font.size`: in a window too narrow for the column, the description runs
/// past the column's edge (clipped by the frame) rather than wrapping a
/// word or two per row.
const DESC_MIN_EMS: f32 = 10.0;

impl DocView {
    /// Paints the help overlay over the text area while the source showed
    /// it at the last paint (`help`), and damages the text area. Without
    /// it, nothing.
    pub(crate) fn paint_help(&mut self) {
        let Some(first) = self.help else {
            return;
        };
        let g = self.geo();
        let s = self.frame.scale;
        let t = &self.tokens;
        let clip = 0.0..g.text_bottom;
        fill(
            &mut self.frame.pixmap,
            0.0,
            0.0,
            g.width,
            g.text_bottom,
            t.color(TokenId::ColorOverlayBg),
            &clip,
        );
        let pad = (t.px(TokenId::SpaceCodePad) * s).round();
        let gap = (t.px(TokenId::SpaceParagraph) * s).round();
        let color = t.color(TokenId::ColorOverlayText);
        let span = |base: SpanBase, strong: bool, em: bool, len: usize| -> StyledSpan {
            let style = SpanStyle {
                strong,
                em,
                ..SpanStyle::plain(base)
            };
            let mut sp = span_for(&style, 0..len, t, s);
            sp.color = color;
            sp
        };
        let width = (g.col_w - 2.0 * pad).max(1.0);
        let left = g.col_x + pad;

        let mut old = std::mem::take(&mut self.help_cache);
        let layout = self.layout.as_mut();
        let mut laid = |text: &str, sp: StyledSpan, w: f32| -> Cached {
            let spans = vec![sp];
            let mut slot = old
                .iter()
                .position(|c| c.key_matches(text, &spans, w))
                .map(|p| old.swap_remove(p));
            lay(layout, &mut slot, text.to_string(), spans, w);
            slot.expect("just laid")
        };

        // Every key, so the descriptions line up after the widest.
        let rows = help::rows();
        let mut keys: Vec<Option<Cached>> = Vec::with_capacity(rows.len());
        let mut key_w: f32 = 0.0;
        for row in &rows {
            let key = match row {
                HelpRow::Entry(k, _) => {
                    let c = laid(k, span(SpanBase::Code, false, false, k.len()), width);
                    key_w = key_w.max(c.laid.width());
                    Some(c)
                }
                _ => None,
            };
            keys.push(key);
        }
        let desc_x = left + key_w + pad;
        let desc_min = t.px(TokenId::FontSize) * s * DESC_MIN_EMS;
        let desc_w = (left + width - desc_x).max(desc_min);

        // The rows that fit, from `first`: each piece and where it goes.
        let mut placed: Vec<(Cached, f32, f32)> = Vec::new();
        let mut y = g.page_top;
        for (i, row) in rows.iter().enumerate().skip(first) {
            if y >= g.text_bottom {
                break;
            }
            let pieces = match *row {
                HelpRow::Title(text) => {
                    let sp = span(SpanBase::Heading(2), false, false, text.len());
                    vec![(laid(text, sp, width), left)]
                }
                HelpRow::Section(text) => {
                    let sp = span(SpanBase::Body, true, false, text.len());
                    vec![(laid(text, sp, width), left)]
                }
                HelpRow::Footer(text) => {
                    let sp = span(SpanBase::Body, false, true, text.len());
                    vec![(laid(text, sp, width), left)]
                }
                HelpRow::Entry(_, d) => {
                    let sp = span(SpanBase::Body, false, false, d.len());
                    let desc = laid(d, sp, desc_w);
                    let key = keys[i].take().expect("an entry's key");
                    vec![(key, left), (desc, desc_x)]
                }
                HelpRow::Blank => Vec::new(),
            };
            let h = pieces
                .iter()
                .map(|(c, _)| c.laid.height)
                .fold(if pieces.is_empty() { gap } else { 0.0 }, f32::max);
            placed.extend(pieces.into_iter().map(|(c, x)| (c, x, y)));
            y += h.ceil();
        }

        let rows_clip = 0..g.text_bottom as u32;
        let pix = &mut self.frame.pixmap;
        for (c, x, y) in &placed {
            let (ox, oy) = (x.round() as i32, y.round() as i32);
            self.layout
                .draw(&c.laid, &mut |m| blend(pix, ox, oy, &m, &rows_clip));
        }
        self.help_cache = placed.into_iter().map(|(c, _, _)| c).collect();
        self.help_cache.extend(keys.into_iter().flatten());
        self.damage.add(rows_clip);
    }
}

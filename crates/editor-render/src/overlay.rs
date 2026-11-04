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
//!
//! The table picker `:table` opens (`editor_core::tables::TablePicker`)
//! is painted the same way, as a panel under the cursor's line
//! (`paint_picker`). `PickerLayout` places it and finds the cell under a
//! point, for hover and clicks (TODO(Task 38): mouse input).

use editor_core::help::{self, HelpRow};
use editor_core::markdown::{SpanBase, SpanStyle};
use editor_core::tokens::TokenId;

use crate::layout::{RectF, StyledSpan};
use crate::paint::{Corners, blend, fill, fill_rounded, span_for};
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

/// Where the table picker sits, in frame pixels: the panel's top left, its
/// padding, the cells' side and the gap between them, the grid's size in
/// cells, and the label's height under the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PickerLayout {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) pad: f32,
    pub(crate) cell: f32,
    pub(crate) gap: f32,
    pub(crate) cols: u32,
    pub(crate) rows: u32,
    pub(crate) label_h: f32,
}

impl PickerLayout {
    /// The grid's width: `cols` cells and the gaps between them.
    fn grid_w(&self) -> f32 {
        self.span(self.cols)
    }

    /// The grid's height: `rows` cells and the gaps between them.
    fn grid_h(&self) -> f32 {
        self.span(self.rows)
    }

    /// `n` cells and the `n - 1` gaps between them.
    fn span(&self, n: u32) -> f32 {
        let n = n.max(1) as f32;
        n * self.cell + (n - 1.0) * self.gap
    }

    /// The panel's width: the grid and its padding.
    pub(crate) fn width(&self) -> f32 {
        self.grid_w() + 2.0 * self.pad
    }

    /// The panel's height: padding, the grid, a gap, the label and padding.
    pub(crate) fn height(&self) -> f32 {
        self.pad + self.grid_h() + self.gap + self.label_h + self.pad
    }

    /// The grid's top left.
    fn grid_origin(&self) -> (f32, f32) {
        (self.x + self.pad, self.y + self.pad)
    }

    /// Cell `(col, row)`'s top left.
    pub(crate) fn cell_origin(&self, col: u32, row: u32) -> (f32, f32) {
        let (gx, gy) = self.grid_origin();
        let stride = self.cell + self.gap;
        (gx + col as f32 * stride, gy + row as f32 * stride)
    }

    /// The label's top: a gap under the grid.
    pub(crate) fn label_y(&self) -> f32 {
        self.grid_origin().1 + self.grid_h() + self.gap
    }

    /// Anchored to the cursor's line (`line_top..line_bottom`): just under
    /// it, or just above it when the text area (which ends at
    /// `text_bottom`) has no room below, or at the frame's top when it has
    /// none above either.
    pub(crate) fn place(self, line_top: f32, line_bottom: f32, text_bottom: f32) -> PickerLayout {
        let h = self.height();
        let y = if line_bottom + h <= text_bottom {
            line_bottom
        } else {
            (line_top - h).max(0.0)
        };
        PickerLayout { y, ..self }
    }

    /// The cell under the point `x, y` (pixels), as `(col, row)`: the cell,
    /// or the gap after it (so the pointer never falls between cells).
    /// `None` outside the grid: on the padding, the label or off the panel.
    pub(crate) fn cell_at(&self, x: f32, y: f32) -> Option<(u32, u32)> {
        let (gx, gy) = self.grid_origin();
        let (dx, dy) = (x - gx, y - gy);
        if !(0.0..self.grid_w()).contains(&dx) || !(0.0..self.grid_h()).contains(&dy) {
            return None;
        }
        let stride = self.cell + self.gap;
        let col = ((dx / stride) as u32).min(self.cols.saturating_sub(1));
        let row = ((dy / stride) as u32).min(self.rows.saturating_sub(1));
        Some((col, row))
    }
}

impl DocView {
    /// The table picker's cell under the point `x_pt, y_pt`, as `(col,
    /// row)`, while it shows (as last painted). `None` without it, or off
    /// its grid. For the shell to turn hover and clicks into
    /// `TablePicker::select` (TODO(Task 38): not wired yet).
    pub fn picker_cell_at(&self, x_pt: f32, y_pt: f32) -> Option<(u32, u32)> {
        let s = self.frame.scale;
        self.picker_at?.cell_at(x_pt * s, y_pt * s)
    }

    /// Paints the table picker while the source showed it at the last
    /// paint (`picker`), under the cursor's line (`picker_line`), and
    /// damages its rows. The panel is `color.overlay.bg`, `space.code.pad`
    /// in from its edges, rounded by `radius.code`. Its cells are
    /// `font.size` square, `space.table.pad.y` apart: those up to the
    /// selection (`TablePicker::selected`) filled in `color.overlay.text`,
    /// the rest outlined in it, `size.table.border` wide. The label (`<c>
    /// x <r>`) sits under the grid in the body font. Without the picker,
    /// nothing.
    pub(crate) fn paint_picker(&mut self) {
        let Some(p) = self.picker.clone() else {
            self.picker_at = None;
            return;
        };
        let g = self.geo();
        let s = self.frame.scale;
        let t = &self.tokens;
        let bg = t.color(TokenId::ColorOverlayBg);
        let color = t.color(TokenId::ColorOverlayText);
        let border = (t.px(TokenId::SizeTableBorder) * s).round().max(1.0);
        let radius = t.px(TokenId::RadiusCode) * s;
        let mut at = PickerLayout {
            x: g.col_x,
            y: 0.0,
            pad: (t.px(TokenId::SpaceCodePad) * s).round(),
            cell: (t.px(TokenId::FontSize) * s).round(),
            gap: (t.px(TokenId::SpaceTablePadY) * s).round(),
            cols: p.cols(),
            rows: p.rows(),
            label_h: 0.0,
        };

        let label = p.label();
        let mut sp = span_for(&SpanStyle::plain(SpanBase::Body), 0..label.len(), t, s);
        sp.color = color;
        let mut slot = self.picker_cache.take();
        let w = at.grid_w().max(1.0);
        at.label_h = lay(self.layout.as_mut(), &mut slot, label, vec![sp], w)
            .height
            .ceil();

        // Under the cursor's line, as it was laid out at the last paint.
        let line = self.picker_line;
        let top = self.line_top_px(line).unwrap_or(g.page_top);
        let bottom = top + self.line_height_px(line).unwrap_or(0.0);
        let at = at.place(top, bottom, g.text_bottom);

        let clip = 0.0..g.text_bottom;
        let pix = &mut self.frame.pixmap;
        let panel = RectF {
            x: at.x,
            y: at.y,
            w: at.width(),
            h: at.height(),
        };
        let corners = Corners {
            radius,
            top: true,
            bottom: true,
        };
        fill_rounded(pix, panel, corners, bg, &clip);
        let (sel_c, sel_r) = p.selected();
        for row in 0..at.rows {
            for col in 0..at.cols {
                let (x, y) = at.cell_origin(col, row);
                let c = at.cell;
                if col <= sel_c && row <= sel_r {
                    fill(pix, x, y, c, c, color, &clip);
                } else {
                    fill(pix, x, y, c, border, color, &clip);
                    fill(pix, x, y + c - border, c, border, color, &clip);
                    fill(pix, x, y, border, c, color, &clip);
                    fill(pix, x + c - border, y, border, c, color, &clip);
                }
            }
        }
        let cached = slot.expect("just laid");
        let rows_clip = 0..g.text_bottom as u32;
        let (ox, oy) = (
            at.cell_origin(0, 0).0.round() as i32,
            at.label_y().round() as i32,
        );
        self.layout
            .draw(&cached.laid, &mut |m| blend(pix, ox, oy, &m, &rows_clip));
        self.picker_cache = Some(cached);
        self.picker_at = Some(at);
        let y0 = at.y.max(0.0).floor() as u32;
        let y1 = (at.y + at.height()).min(g.text_bottom).ceil() as u32;
        self.damage.add(y0..y1.max(y0));
    }

    /// Paints what goes over the lines: the help overlay, then the table
    /// picker (only one of them shows at a time).
    pub(crate) fn paint_overlays(&mut self) {
        self.paint_help();
        self.paint_picker();
    }
}

#[cfg(test)]
mod tests {
    use super::PickerLayout;

    /// A 3 x 2 grid of 10 px cells 2 px apart, 5 px in from a panel at
    /// (10, 20): the grid runs 15..49 across and 25..47 down.
    fn grid() -> PickerLayout {
        PickerLayout {
            x: 10.0,
            y: 20.0,
            pad: 5.0,
            cell: 10.0,
            gap: 2.0,
            cols: 3,
            rows: 2,
            label_h: 8.0,
        }
    }

    #[test]
    fn hit_test_finds_the_cell_under_a_point() {
        let g = grid();
        assert_eq!(g.cell_at(15.0, 25.0), Some((0, 0)));
        assert_eq!(g.cell_at(24.9, 34.9), Some((0, 0)));
        // A gap belongs to the cell before it, so the pointer never falls
        // between cells.
        assert_eq!(g.cell_at(26.0, 25.0), Some((0, 0)));
        assert_eq!(g.cell_at(27.0, 25.0), Some((1, 0)));
        assert_eq!(g.cell_at(48.9, 46.9), Some((2, 1)));
        assert_eq!(g.cell_at(39.0, 37.0), Some((2, 1)));
    }

    #[test]
    fn hit_test_outside_the_grid_is_none() {
        let g = grid();
        // The padding, past the last cell, the label and outside the panel.
        assert_eq!(g.cell_at(14.9, 30.0), None);
        assert_eq!(g.cell_at(30.0, 24.9), None);
        assert_eq!(g.cell_at(49.0, 30.0), None);
        assert_eq!(g.cell_at(30.0, 47.0), None);
        assert_eq!(g.cell_at(30.0, 52.0), None);
        assert_eq!(g.cell_at(-5.0, -5.0), None);
        assert_eq!(g.cell_at(500.0, 500.0), None);
    }

    #[test]
    fn panel_size_covers_grid_label_and_padding() {
        let g = grid();
        assert_eq!(g.width(), 44.0);
        // pad, grid, gap, label, pad.
        assert_eq!(g.height(), 5.0 + 22.0 + 2.0 + 8.0 + 5.0);
    }

    #[test]
    fn placed_under_the_line_or_above_when_no_room() {
        // Room below: just under the line.
        let g = grid().place(100.0, 120.0, 400.0);
        assert_eq!(g.y, 120.0);
        // No room below: just above the line.
        let g = grid().place(100.0, 120.0, 150.0);
        assert_eq!(g.y, 100.0 - g.height());
        // No room either way: at the top of the frame.
        let g = grid().place(10.0, 20.0, 50.0);
        assert_eq!(g.y, 0.0);
        // Across, it stays where it was put.
        assert_eq!(g.x, 10.0);
    }
}

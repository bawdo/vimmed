//! How wide a code block's box is.
//!
//! Every line of a block shares one box, so the width comes from the whole
//! block: its longest content line laid out unwrapped, plus
//! `space.code.pad` each side. A block that fits the text column keeps the
//! column's width; a wider one grows to the right, from the column's left
//! edge, up to the breakout column (`Geo::wide_w`), and lines longer than
//! that still wrap. The fences don't count, so a fence revealed under the
//! cursor never moves the box.
//!
//! Each content line's width is kept with its text, so laying the block
//! out again after an edit shapes only the lines that changed.

use std::collections::HashMap;
use std::ops::Range;

use editor_core::markdown::{SpanBase, SpanStyle};
use editor_core::tokens::{TokenId, Tokens};
use editor_core::view_model::ViewSource;

use crate::layout::TextLayout;
use crate::paint::span_for;

/// Wide enough that no code line wraps while it is measured.
const UNWRAPPED: f32 = 1.0e7;

/// One code block's box width.
pub(crate) struct CodeWidth {
    /// Every line of the block, fences included.
    pub(crate) lines: Range<usize>,
    /// Each content line's text and its unwrapped width in pixels.
    measured: Vec<(String, f32)>,
    /// The box's width in pixels.
    pub(crate) box_w: f32,
    /// The frame this width was last checked against the source in.
    pub(crate) frame: u64,
}

/// The box width for content as wide as `widest`, inside `col..=wide`.
fn box_width(widest: f32, pad: f32, col: f32, wide: f32) -> f32 {
    (widest.ceil() + 2.0 * pad).min(wide).max(col)
}

/// Measures the block with lines `lines` and content `content`, between a
/// text column `col` and a breakout column `wide` pixels wide. Widths from
/// `old` whose line text is unchanged are reused, not shaped again: the
/// unchanged runs before and after an edit are kept as they are, so a
/// keystroke in a long block looks only at the lines between them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    layout: &mut dyn TextLayout,
    src: &dyn ViewSource,
    lines: Range<usize>,
    content: Range<usize>,
    col: f32,
    wide: f32,
    t: &Tokens,
    s: f32,
    old: Option<CodeWidth>,
) -> CodeWidth {
    let mut measured = old.map_or_else(Vec::new, |o| o.measured);
    let n = content.len();
    let line = |k: usize| src.line(content.start + k);
    let pre = measured
        .iter()
        .take(n)
        .enumerate()
        .take_while(|(k, (text, _))| line(*k) == text)
        .count();
    let room = measured.len().min(n) - pre;
    let post = (0..room)
        .take_while(|&m| measured[measured.len() - 1 - m].0 == line(n - 1 - m))
        .count();
    let after = measured.split_off(measured.len() - post);
    let between = measured.split_off(pre);
    // Lines inside the edit that only moved keep their widths too.
    let known: HashMap<&str, f32> = between.iter().map(|(s, w)| (s.as_str(), *w)).collect();
    let style = SpanStyle::plain(SpanBase::CodeBlock { lang: None });
    for k in pre..n - post {
        let text = line(k);
        let w = known.get(text).copied().unwrap_or_else(|| {
            let span = span_for(&style, 0..text.len(), t, s);
            layout.layout_line(text, &[span], UNWRAPPED).width()
        });
        measured.push((text.to_owned(), w));
    }
    measured.extend(after);
    let widest = measured.iter().map(|(_, w)| *w).fold(0.0, f32::max);
    let pad = (t.px(TokenId::SpaceCodePad) * s).round();
    CodeWidth {
        lines,
        measured,
        box_w: box_width(widest, pad, col, wide),
        frame: 0,
    }
}

impl CodeWidth {
    /// True when the block's content is these lines' text.
    pub(crate) fn same_text(&self, src: &dyn ViewSource, content: &Range<usize>) -> bool {
        self.measured.len() == content.len()
            && content
                .clone()
                .zip(&self.measured)
                .all(|(j, (text, _))| src.line(j) == text)
    }

    /// The box width again for a new column and breakout column.
    pub(crate) fn refit(&mut self, col: f32, wide: f32, t: &Tokens, s: f32) {
        let widest = self.measured.iter().map(|(_, w)| *w).fold(0.0, f32::max);
        let pad = (t.px(TokenId::SpaceCodePad) * s).round();
        self.box_w = box_width(widest, pad, col, wide);
    }
}

#[cfg(test)]
mod tests {
    use super::box_width;

    #[test]
    fn a_block_that_fits_keeps_the_column_width() {
        assert_eq!(box_width(300.0, 16.0, 900.0, 1600.0), 900.0);
    }

    #[test]
    fn a_wider_block_grows_to_its_longest_line() {
        assert_eq!(box_width(1000.4, 16.0, 900.0, 1600.0), 1033.0);
    }

    #[test]
    fn the_breakout_column_caps_it() {
        assert_eq!(box_width(5000.0, 16.0, 900.0, 1600.0), 1600.0);
    }
}

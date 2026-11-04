//! Pipe tables drawn as a grid.
//!
//! A table's geometry is shared by all its rows: a column is as wide as its
//! widest shaped cell plus `space.table.pad` each side, so an edit to any row
//! can move every row. `DocView` therefore lays a table out as a whole, keyed
//! on every row's source, and repaints the whole table when any row changes.
//!
//! Rows: the header has a top border and its cells are strong; the separator
//! row is a divider `size.table.border` tall; data rows have a bottom border.
//! Every row draws the column verticals, so the grid is continuous. Cells sit
//! `space.table.pad` in from the verticals and `space.table.pad.y` from the
//! row's edges, aligned per column (`Align::None` renders left). When the
//! natural widths overflow the column, every column shrinks in proportion and
//! the cells wrap.

use std::ops::Range;

use editor_core::markdown::table::{alignments, parse_cells};
use editor_core::markdown::{Align, BlockDeco, DisplaySpan, LineDisplay, Prefix, TablePart};
use editor_core::tokens::{TokenId, Tokens};
use editor_core::view_model::ViewSource;
use tiny_skia::Pixmap;

use crate::layout::{LaidLine, TextLayout};
use crate::paint::{blend, fill, styled_line};
use crate::view::{Cached, lay};

/// One cell: its source bytes on the line, the display of just those bytes,
/// and its layout.
pub(crate) struct Cell {
    pub(crate) display: LineDisplay,
    pub(crate) laid: Option<Cached>,
}

impl Cell {
    pub(crate) fn laid(&self) -> Option<&LaidLine> {
        self.laid.as_ref().map(|c| &c.laid)
    }
}

/// One line of a table.
pub(crate) struct Row {
    pub(crate) part: TablePart,
    pub(crate) height: f32,
    pub(crate) cells: Vec<Cell>,
}

/// The geometry of one table, in pixels relative to the column's left edge.
pub(crate) struct TableGeom {
    /// The table's lines, header to last row.
    pub(crate) lines: Range<usize>,
    /// What the geometry was made from: every row's source, then the sizes.
    key: (Vec<String>, [f32; 5]),
    /// The x of each column's left vertical, then the right edge's vertical.
    pub(crate) verticals: Vec<f32>,
    /// Each column's text width (inside the padding).
    pub(crate) inner: Vec<f32>,
    pub(crate) aligns: Vec<Align>,
    pub(crate) rows: Vec<Row>,
    /// The frame this geometry was last checked against the source in.
    pub(crate) frame: u64,
}

/// Token sizes in pixels.
struct Sizes {
    pad: f32,
    pad_y: f32,
    border: f32,
}

impl Sizes {
    fn new(t: &Tokens, s: f32) -> Sizes {
        Sizes {
            pad: (t.px(TokenId::SpaceTablePad) * s).round(),
            pad_y: (t.px(TokenId::SpaceTablePadY) * s).round(),
            border: (t.px(TokenId::SizeTableBorder) * s).round().max(1.0),
        }
    }
}

impl TableGeom {
    /// The table's full width, outer verticals included.
    pub(crate) fn width(&self, t: &Tokens, s: f32) -> f32 {
        self.verticals.last().copied().unwrap_or(0.0) + Sizes::new(t, s).border
    }

    pub(crate) fn row(&self, line: usize) -> Option<&Row> {
        line.checked_sub(self.lines.start)
            .and_then(|k| self.rows.get(k))
    }

    /// Where cell `c` of row `line` draws its text, relative to the column's
    /// left edge and the row's top.
    pub(crate) fn cell_origin(&self, line: usize, c: usize, t: &Tokens, s: f32) -> (f32, f32) {
        let z = Sizes::new(t, s);
        let Some(row) = self.row(line) else {
            return (0.0, 0.0);
        };
        let w = row
            .cells
            .get(c)
            .and_then(Cell::laid)
            .map_or(0.0, |l| l.width());
        let inner = self.inner.get(c).copied().unwrap_or(0.0);
        let shift = match self.aligns.get(c).copied().unwrap_or(Align::None) {
            Align::Right => (inner - w).max(0.0),
            Align::Centre => ((inner - w) / 2.0).max(0.0).floor(),
            Align::None | Align::Left => 0.0,
        };
        let x = self.verticals.get(c).copied().unwrap_or(0.0) + z.border + z.pad + shift;
        let y = match row.part {
            TablePart::Header => z.border + z.pad_y,
            _ => z.pad_y,
        };
        (x.round(), y)
    }
}

/// The table holding line `i` (whose display is `d`): its lines, scanning
/// neighbours for rows of the same table, and each row's display.
pub(crate) fn extent(
    src: &dyn ViewSource,
    i: usize,
    d: &LineDisplay,
) -> Option<(Range<usize>, Vec<LineDisplay>)> {
    let BlockDeco::TableRow { table, .. } = d.deco else {
        return None;
    };
    let same =
        |d: &LineDisplay| matches!(d.deco, BlockDeco::TableRow { table: t, .. } if t == table);
    let mut above = Vec::new();
    let mut start = i;
    while start > 0 {
        let up = src.display(start - 1);
        if !same(&up) {
            break;
        }
        above.push(up);
        start -= 1;
    }
    let mut rows: Vec<LineDisplay> = above.into_iter().rev().collect();
    rows.push(d.clone());
    let mut end = i + 1;
    while end < src.line_count() {
        let down = src.display(end);
        if !same(&down) {
            break;
        }
        rows.push(down);
        end += 1;
    }
    Some((start..end, rows))
}

/// The byte range of each cell's trimmed text in `line`.
fn cell_ranges(line: &str) -> Vec<Range<usize>> {
    let base = line.as_ptr() as usize;
    parse_cells(line)
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            let a = (c.as_ptr() as usize).saturating_sub(base).min(line.len());
            a..(a + c.len()).min(line.len())
        })
        .collect()
}

/// The part of `d` inside `cell`, strong when `strong`.
fn cell_display(d: &LineDisplay, cell: &Range<usize>, strong: bool) -> LineDisplay {
    let spans = d
        .spans
        .iter()
        .filter_map(|s| {
            let src = s.src.start.max(cell.start)..s.src.end.min(cell.end);
            (src.start < src.end).then(|| {
                let mut style = s.style.clone();
                style.strong |= strong;
                DisplaySpan { src, style }
            })
        })
        .collect();
    LineDisplay {
        prefix: Prefix::None,
        spans,
        deco: BlockDeco::None,
        src_len: cell.end,
    }
}

/// True when `geom` was made from these rows at these sizes.
pub(crate) fn key_of(
    src: &dyn ViewSource,
    lines: &Range<usize>,
    avail: f32,
    t: &Tokens,
    s: f32,
) -> (Vec<String>, [f32; 5]) {
    let z = Sizes::new(t, s);
    (
        lines.clone().map(|j| src.line(j).to_owned()).collect(),
        [avail, s, z.pad, z.pad_y, z.border],
    )
}

impl TableGeom {
    pub(crate) fn matches(&self, key: &(Vec<String>, [f32; 5])) -> bool {
        self.key == *key
    }
}

/// Lays a table out. Cell layouts from `old` whose inputs are unchanged are
/// reused rather than shaped again.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    layout: &mut dyn TextLayout,
    src: &dyn ViewSource,
    lines: Range<usize>,
    displays: &[LineDisplay],
    avail: f32,
    t: &Tokens,
    s: f32,
    old: Option<TableGeom>,
) -> TableGeom {
    let z = Sizes::new(t, s);
    let key = key_of(src, &lines, avail, t, s);
    let mut pool: Vec<Cached> = old
        .into_iter()
        .flat_map(|g| g.rows.into_iter())
        .flat_map(|r| r.cells.into_iter())
        .filter_map(|c| c.laid)
        .collect();
    let mut take = |text: &str, spans: &[crate::layout::StyledSpan], w: f32| {
        let k = pool.iter().position(|c| c.key_matches(text, spans, w))?;
        Some(pool.swap_remove(k))
    };

    let sep_line = lines
        .clone()
        .zip(displays)
        .find(|(_, d)| {
            matches!(
                d.deco,
                BlockDeco::TableRow {
                    part: TablePart::Separator,
                    ..
                }
            )
        })
        .map(|(j, _)| j);
    let aligns = sep_line
        .and_then(|j| alignments(src.line(j)))
        .unwrap_or_default();

    // Natural widths: every cell laid out unwrapped.
    let mut rows: Vec<Row> = Vec::with_capacity(displays.len());
    let mut natural: Vec<f32> = vec![0.0; aligns.len()];
    for (j, d) in lines.clone().zip(displays) {
        let part = match d.deco {
            BlockDeco::TableRow { part, .. } => part,
            _ => TablePart::Row,
        };
        let mut cells = Vec::new();
        if part != TablePart::Separator {
            let line = src.line(j);
            for (c, r) in cell_ranges(line).into_iter().enumerate() {
                let display = cell_display(d, &r, part == TablePart::Header);
                let (text, spans) = styled_line(line, &display, t, s);
                let mut slot = take(&text, &spans, avail);
                let w = lay(layout, &mut slot, text, spans, avail).width();
                if c >= natural.len() {
                    natural.push(0.0);
                }
                natural[c] = natural[c].max(w.ceil());
                cells.push(Cell {
                    display,
                    laid: slot,
                });
            }
        }
        rows.push(Row {
            part,
            height: 0.0,
            cells,
        });
    }

    // Column widths, shrunk in proportion when they overflow the column.
    let n = natural.len();
    let chrome = (n as f32 + 1.0) * z.border + n as f32 * 2.0 * z.pad;
    let want: f32 = natural.iter().sum();
    let room = (avail - chrome).max(n as f32);
    let inner: Vec<f32> = if want > room {
        natural
            .iter()
            .map(|w| (w * room / want).floor().max(1.0))
            .collect()
    } else {
        natural.clone()
    };
    let mut verticals = Vec::with_capacity(n + 1);
    let mut x = 0.0;
    for w in &inner {
        verticals.push(x);
        x += z.border + 2.0 * z.pad + w;
    }
    verticals.push(x);

    // Wrap the cells that no longer fit, then size each row.
    for (j, row) in lines.clone().zip(rows.iter_mut()) {
        let line = src.line(j);
        let mut content: f32 = 0.0;
        for (c, cell) in row.cells.iter_mut().enumerate() {
            let w = inner.get(c).copied().unwrap_or(1.0);
            if natural.get(c).is_some_and(|&nw| nw > w) {
                let (text, spans) = styled_line(line, &cell.display, t, s);
                let mut slot = take(&text, &spans, w).or(cell.laid.take());
                lay(layout, &mut slot, text, spans, w);
                cell.laid = slot;
            }
            content = content.max(cell.laid().map_or(0.0, |l| l.height.ceil()));
        }
        row.height = match row.part {
            TablePart::Separator => z.border,
            _ => content + 2.0 * z.pad_y + z.border,
        };
    }

    TableGeom {
        lines,
        key,
        verticals,
        inner,
        aligns,
        rows,
        frame: 0,
    }
}

/// Paints row `line` of `geom` at column x `cx`, row top `top`: borders in
/// `color.table.border`, then each cell's text. Only rows `clip` change.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_row(
    p: &mut Pixmap,
    layout: &mut dyn TextLayout,
    geom: &TableGeom,
    line: usize,
    cx: f32,
    top: f32,
    t: &Tokens,
    s: f32,
    clip: &Range<f32>,
) {
    let Some(row) = geom.row(line) else {
        return;
    };
    let z = Sizes::new(t, s);
    let c = t.color(TokenId::ColorTableBorder);
    let (h, w) = (row.height, geom.width(t, s));
    for v in &geom.verticals {
        fill(p, cx + v, top, z.border, h, c, clip);
    }
    match row.part {
        TablePart::Header => fill(p, cx, top, w, z.border, c, clip),
        TablePart::Separator => fill(p, cx, top, w, h, c, clip),
        TablePart::Row => fill(p, cx, top + h - z.border, w, z.border, c, clip),
    }
    let rows = clip.start as u32..clip.end.ceil() as u32;
    for (k, cell) in row.cells.iter().enumerate() {
        let Some(laid) = cell.laid() else {
            continue;
        };
        let (x, y) = geom.cell_origin(line, k, t, s);
        let (ox, oy) = ((cx + x) as i32, (top + y) as i32);
        layout.draw(laid, &mut |m| blend(p, ox, oy, &m, &rows));
    }
}

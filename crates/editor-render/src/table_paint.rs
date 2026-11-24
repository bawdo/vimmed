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
//! row's edges, aligned per column (`Align::None` renders left).
//!
//! Width: a table is as wide as its cells need, never stretched. It starts
//! at the text column's left edge; a table wider than the column breaks out
//! of it to the right, up to the breakout column (`Room`). A table too wide
//! even for that moves left as far as it needs, down to `space.page.side`
//! from the frame's left edge, and is never wider than `space.column.wide`.
//! When that is still too narrow, columns shrink as a browser's automatic
//! table layout does: each keeps its longest word (its min-content width)
//! and the room left over is shared in proportion to how much more each
//! column wants, so a short column such as an ID never wraps because a long
//! one is next to it. When the longest words alone overflow, body words
//! break first and header words are kept, so a one-word header never splits
//! because a long URL sits further down. Only when the header words alone
//! overflow do they shrink, in proportion, and break.

use std::ops::Range;

use editor_core::markdown::table::{alignments, parse_cells};
use editor_core::markdown::{Align, BlockDeco, DisplaySpan, LineDisplay, Prefix, TablePart};
use editor_core::tokens::{TokenId, Tokens};
use editor_core::view_model::ViewSource;
use tiny_skia::Pixmap;

use crate::layout::{LaidLine, TextLayout};
use crate::paint::{blend, fill, styled_line};
use crate::view::{Cached, Geo, lay};

/// Where a table may sit, in pixels: the text column's width, and the
/// breakout column's, both from the text column's left edge; how far left
/// of that edge an overflowing table may reach; and the widest it may be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Room {
    pub(crate) col: f32,
    pub(crate) wide: f32,
    pub(crate) left: f32,
    pub(crate) max: f32,
}

impl Room {
    pub(crate) fn of(g: &Geo) -> Room {
        Room {
            col: g.col_w,
            wide: g.wide_w,
            left: g.table_left,
            max: g.wide_max,
        }
    }

    /// The widest a table may be: the breakout column plus the reach left,
    /// up to `max`, and never narrower than the breakout column.
    fn total(&self) -> f32 {
        (self.wide + self.left).min(self.max).max(self.wide)
    }

    /// How far left of the text column's edge a table `width` wide starts:
    /// only as far as it overflows the breakout column, up to `left`.
    fn shift(&self, width: f32) -> f32 {
        (width - self.wide).clamp(0.0, self.left).ceil()
    }
}

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

/// The geometry of one table, in pixels relative to the text column's left
/// edge.
pub(crate) struct TableGeom {
    /// The table's lines, header to last row.
    pub(crate) lines: Range<usize>,
    /// What the geometry was made from: every row's source, then the sizes.
    key: (Vec<String>, [f32; 8]),
    /// The x of each column's left vertical, then the right edge's vertical.
    /// The first is the table's left edge.
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
        let (first, last) = (self.verticals.first(), self.verticals.last());
        let span = last.zip(first).map_or(0.0, |(l, f)| l - f);
        span + Sizes::new(t, s).border
    }

    /// The table's left edge, relative to the text column's.
    pub(crate) fn left(&self) -> f32 {
        self.verticals.first().copied().unwrap_or(0.0)
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
    room: Room,
    t: &Tokens,
    s: f32,
) -> (Vec<String>, [f32; 8]) {
    let z = Sizes::new(t, s);
    (
        lines.clone().map(|j| src.line(j).to_owned()).collect(),
        [
            room.col, room.wide, room.left, room.max, s, z.pad, z.pad_y, z.border,
        ],
    )
}

impl TableGeom {
    pub(crate) fn matches(&self, key: &(Vec<String>, [f32; 8])) -> bool {
        self.key == *key
    }

    /// True when some cell shaped a glyph as `.notdef`.
    pub(crate) fn missing_glyphs(&self) -> bool {
        let cells = self.rows.iter().flat_map(|r| &r.cells);
        cells.filter_map(Cell::laid).any(|l| l.missing_glyphs)
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
    room: Room,
    t: &Tokens,
    s: f32,
    old: Option<TableGeom>,
) -> TableGeom {
    let z = Sizes::new(t, s);
    let key = key_of(src, &lines, room, t, s);
    let avail = room.total();
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

    // Natural widths: every cell laid out unwrapped. Min-content widths:
    // each cell's longest word, and the header cell's on its own.
    let mut rows: Vec<Row> = Vec::with_capacity(displays.len());
    let mut natural: Vec<f32> = vec![0.0; aligns.len()];
    let mut least: Vec<f32> = vec![0.0; aligns.len()];
    let mut head: Vec<f32> = vec![0.0; aligns.len()];
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
                lay(layout, &mut slot, text, spans, avail);
                let (w, word) = slot.as_ref().map_or((0.0, 0.0), |c| {
                    (c.laid.width(), longest_word(&c.laid, c.key_text()))
                });
                if c >= natural.len() {
                    natural.push(0.0);
                    least.push(0.0);
                    head.push(0.0);
                }
                natural[c] = natural[c].max(w.ceil());
                least[c] = least[c].max(word.ceil());
                if part == TablePart::Header {
                    head[c] = head[c].max(word.ceil());
                }
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

    // Column widths, shrunk to fit the room.
    let n = natural.len();
    let chrome = (n as f32 + 1.0) * z.border + n as f32 * 2.0 * z.pad;
    let inner = column_widths(&natural, &least, &head, (avail - chrome).max(n as f32));
    // From the text column's left edge, moved left as far as the table
    // overflows the breakout column. A table shrunk to fit takes all the
    // room, so it starts at the room's left edge whatever its widths lost
    // to rounding.
    let squeezed = natural.iter().sum::<f32>() + chrome > avail;
    let width = if squeezed {
        avail
    } else {
        inner.iter().sum::<f32>() + chrome
    };
    let mut verticals = Vec::with_capacity(n + 1);
    let mut x = -room.shift(width);
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

/// The widest run of non-space clusters in `laid` (made from `text`): the
/// narrowest the line can wrap to without breaking a word.
fn longest_word(laid: &LaidLine, text: &str) -> f32 {
    let mut widest: f32 = 0.0;
    for row in laid.row_geometry() {
        let mut run: Option<(f32, f32)> = None;
        for g in &row.glyphs {
            let space = text
                .get(g.bytes.clone())
                .is_some_and(|c| c.chars().all(char::is_whitespace));
            if space {
                run = None;
                continue;
            }
            let (a, b) = (g.x, g.x + g.w);
            let (lo, hi) = run.map_or((a, b), |(lo, hi)| (lo.min(a), hi.max(b)));
            widest = widest.max(hi - lo);
            run = Some((lo, hi));
        }
    }
    widest
}

/// Text widths for columns that want `natural`, cannot go below `least`
/// without breaking a word, and whose header's longest word is `head`,
/// inside `room`: the natural widths when they fit; else each column's
/// least plus a share of what is left over, in proportion to how much more
/// it wants; else, when the least widths overflow, each column's head plus
/// a share in proportion to how far its least is past its head, so body
/// words break and header words do not; else, when the heads alone
/// overflow, those shrunk in proportion.
pub(crate) fn column_widths(natural: &[f32], least: &[f32], head: &[f32], room: f32) -> Vec<f32> {
    let least: Vec<f32> = natural.iter().zip(least).map(|(n, l)| l.min(*n)).collect();
    let head: Vec<f32> = least
        .iter()
        .zip(head.iter().chain(std::iter::repeat(&0.0)))
        .map(|(l, h)| h.min(*l))
        .collect();
    let want: f32 = natural.iter().sum();
    let floor: f32 = least.iter().sum();
    let heads: f32 = head.iter().sum();
    // Each column at `lo` plus `share` of the way to `hi`.
    let between = |lo: &[f32], hi: &[f32], share: f32| -> Vec<f32> {
        lo.iter()
            .zip(hi)
            .map(|(l, h)| (l + (h - l) * share).floor().max(1.0))
            .collect()
    };
    if want <= room {
        natural.to_vec()
    } else if floor < room {
        between(&least, natural, (room - floor) / (want - floor))
    } else if heads < room {
        between(&head, &least, (room - heads) / (floor - heads))
    } else {
        head.iter()
            .map(|w| (w * room / heads.max(1.0)).floor().max(1.0))
            .collect()
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
    let x = cx + geom.left();
    match row.part {
        TablePart::Header => fill(p, x, top, w, z.border, c, clip),
        TablePart::Separator => fill(p, x, top, w, h, c, clip),
        TablePart::Row => fill(p, x, top + h - z.border, w, z.border, c, clip),
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

#[cfg(test)]
mod tests {
    use super::{Room, column_widths};

    #[test]
    fn natural_widths_when_they_fit() {
        assert_eq!(
            column_widths(&[40.0, 300.0], &[40.0, 60.0], &[], 400.0),
            [40.0, 300.0]
        );
    }

    #[test]
    fn short_columns_keep_their_longest_word() {
        // 40 + 1000 wanted, 400 of room: the ID column wants nothing past its
        // word, so the long column takes all that is left.
        assert_eq!(
            column_widths(&[40.0, 1000.0], &[40.0, 60.0], &[], 400.0),
            [40.0, 360.0]
        );
    }

    #[test]
    fn leftover_room_is_shared_by_how_much_more_each_wants() {
        // Floors 100 + 100; wants 300 + 700; 400 of room leaves 200 to share,
        // a quarter of the 800 wanted past the floors.
        assert_eq!(
            column_widths(&[300.0, 700.0], &[100.0, 100.0], &[], 400.0),
            [150.0, 250.0]
        );
    }

    #[test]
    fn words_break_only_when_the_floors_overflow() {
        assert_eq!(
            column_widths(&[300.0, 300.0], &[200.0, 300.0], &[], 250.0),
            [100.0, 150.0]
        );
    }

    #[test]
    fn a_floor_never_exceeds_the_natural_width() {
        assert_eq!(
            column_widths(&[50.0, 500.0], &[80.0, 20.0], &[], 300.0),
            [50.0, 250.0]
        );
    }

    #[test]
    fn body_words_break_before_header_words() {
        // A one-word header (60) over short cells, beside a column whose URL
        // (500) overflows: the header keeps its word, the URL breaks.
        assert_eq!(
            column_widths(&[60.0, 500.0], &[60.0, 500.0], &[60.0, 40.0], 300.0),
            [60.0, 240.0]
        );
    }

    #[test]
    fn body_floors_shrink_towards_the_header_words() {
        // Heads 50 + 50, floors 150 + 250: 300 of room leaves 200 past the
        // heads, two thirds of the 300 the floors want past them.
        assert_eq!(
            column_widths(&[150.0, 250.0], &[150.0, 250.0], &[50.0, 50.0], 300.0),
            [116.0, 183.0]
        );
    }

    #[test]
    fn header_words_break_only_when_they_alone_overflow() {
        assert_eq!(
            column_widths(&[200.0, 300.0], &[200.0, 300.0], &[200.0, 200.0], 200.0),
            [100.0, 100.0]
        );
    }

    #[test]
    fn a_head_never_exceeds_its_floor() {
        // A head wider than the column's floor (its natural width wraps the
        // header) counts as the floor.
        assert_eq!(
            column_widths(&[100.0, 400.0], &[100.0, 400.0], &[300.0, 0.0], 300.0),
            [100.0, 200.0]
        );
    }

    fn room(wide: f32, left: f32, max: f32) -> Room {
        Room {
            col: 600.0,
            wide,
            left,
            max,
        }
    }

    #[test]
    fn a_table_that_fits_the_breakout_stays_at_the_column_edge() {
        let r = room(1000.0, 300.0, 1600.0);
        assert_eq!(r.total(), 1300.0);
        assert_eq!(r.shift(900.0), 0.0);
        assert_eq!(r.shift(1000.0), 0.0);
    }

    #[test]
    fn an_overflowing_table_moves_left_only_as_far_as_it_needs() {
        let r = room(1000.0, 300.0, 1600.0);
        assert_eq!(r.shift(1120.5), 121.0);
        assert_eq!(r.shift(5000.0), 300.0);
    }

    #[test]
    fn the_room_is_capped_at_the_widest_but_never_below_the_breakout() {
        assert_eq!(room(1400.0, 400.0, 1600.0).total(), 1600.0);
        assert_eq!(room(1000.0, 0.0, 800.0).total(), 1000.0);
    }
}

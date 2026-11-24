//! The fluid page: the text column sits `space.page.side` from the window's
//! left edge and grows with the window up to `space.column`, its right
//! margin scales with the window, and tables and code blocks wider than the
//! column break out of it to the right, keeping its left edge, up to
//! `space.column.wide` and the right margin.

mod common;

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use common::{Fake, H_PT, Laid, Recording, column_in, cosmic, line_top, wide_in};
use editor_core::buffer::Pos;
use editor_core::tokens::{Theme, TokenId, Tokens};
use editor_render::view::DocView;

const S: f32 = 1.0;

fn with(pairs: &[(&str, &str)]) -> Tokens {
    let mut t = Tokens::defaults(Theme::Light);
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert!(t.apply_overrides(&pairs).is_empty());
    t
}

fn recording(t: &Tokens, w_pt: f32) -> (DocView, Rc<RefCell<Vec<Laid>>>) {
    let laid = Rc::new(RefCell::new(Vec::new()));
    let layout = Recording {
        inner: cosmic(t),
        laid: laid.clone(),
    };
    (DocView::new(Box::new(layout), t, w_pt, H_PT, S), laid)
}

/// The width the last layout of `text` was given, in pixels.
fn laid_width(laid: &Rc<RefCell<Vec<Laid>>>, text: &str) -> f32 {
    let laid = laid.borrow();
    let l = laid.iter().rev().find(|l| l.text == text);
    l.unwrap_or_else(|| panic!("{text:?} was never laid out"))
        .width_px
}

fn is_red(v: &DocView, x: u32, y: u32) -> bool {
    let p = v.frame().pixmap.pixel(x, y).unwrap();
    p.red() > 200 && p.green() < 60 && p.blue() < 60
}

/// Runs of red pixels on row `y`, as x ranges.
fn red_runs(v: &DocView, y: u32) -> Vec<Range<u32>> {
    let mut out: Vec<Range<u32>> = Vec::new();
    for x in 0..v.frame().width_px {
        if !is_red(v, x, y) {
            continue;
        }
        match out.last_mut() {
            Some(r) if r.end == x => r.end = x + 1,
            _ => out.push(x..x + 1),
        }
    }
    out
}

/// The middle row of line `i`.
fn mid(v: &DocView, t: &Tokens, i: usize) -> u32 {
    (line_top(v, t, S, i) + v.line_height_px(i).unwrap() / 2.0) as u32
}

const PARA: &str = "A paragraph long enough to wrap in any column this test makes, \
    so its layout width is the column's width and nothing else: lorem ipsum dolor \
    sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore.";

#[test]
fn the_column_grows_with_the_window_up_to_space_column() {
    let t = Tokens::defaults(Theme::Light);
    let cap = t.px(TokenId::SpaceColumn);
    let mut last = 0.0;
    for w_pt in [300.0, 600.0, 800.0, 1000.0, 1200.0, 2400.0] {
        let src = Fake::new(&[PARA], t.clone());
        let (mut v, laid) = recording(&t, w_pt);
        v.paint_all(&src);
        let (col_x, col_w) = column_in(&t, w_pt, S);
        assert_eq!(
            laid_width(&laid, PARA),
            col_w,
            "wraps at the column at {w_pt}"
        );
        assert_eq!(v.text_left_px(0), Some(col_x), "left aligned at {w_pt}");
        assert_eq!(col_x, t.px(TokenId::SpacePageSide), "at the fixed margin");
        assert!(col_w >= last, "never narrower in a wider window ({w_pt})");
        assert!(col_w <= cap, "never past space.column ({w_pt})");
        last = col_w;
    }
    // 24 on the left; 5% of 800 is 40 on the right. At 1000 the 900 cap.
    assert_eq!(column_in(&t, 800.0, S).1, 736.0);
    assert_eq!(column_in(&t, 1000.0, S).1, 900.0);
    assert_eq!(column_in(&t, 2400.0, S).1, 900.0);
    // A narrow window keeps the fixed margin: 5% of 300 is under 24.
    assert_eq!(column_in(&t, 300.0, S).1, 300.0 - 2.0 * 24.0);
}

#[test]
fn the_margin_is_the_wider_of_the_fixed_and_the_ratio() {
    let t = with(&[("space.page.side.ratio", "0.2")]);
    let src = Fake::new(&[PARA], t.clone());
    let (mut v, laid) = recording(&t, 1000.0);
    v.paint_all(&src);
    assert_eq!(laid_width(&laid, PARA), 776.0, "24 left, 20% of 1000 right");
    assert_eq!(
        v.text_left_px(0),
        Some(24.0),
        "the ratio is the right side's"
    );

    let t = with(&[("space.page.side", "300")]);
    let src = Fake::new(&[PARA], t.clone());
    let (mut v, laid) = recording(&t, 1000.0);
    v.paint_all(&src);
    assert_eq!(laid_width(&laid, PARA), 400.0, "the fixed side beats 5%");
    assert_eq!(v.text_left_px(0), Some(300.0), "and sets the left margin");
}

#[test]
fn resizing_relays_the_text_at_the_new_width() {
    let t = Tokens::defaults(Theme::Light);
    let src = Fake::new(&[PARA], t.clone());
    let (mut v, laid) = recording(&t, 700.0);
    v.paint_all(&src);
    assert_eq!(laid_width(&laid, PARA), column_in(&t, 700.0, S).1);
    v.resize(1100.0, H_PT, S);
    v.paint_all(&src);
    assert_eq!(laid_width(&laid, PARA), 900.0, "wider window, wider text");
}

const NARROW: &[&str] = &["Intro", "| a | b |", "|---|---|", "| c | d |", "", "after"];

/// A table whose natural width is wider than the 900 column but well inside
/// the 1600 breakout cap (each cell is about 400 pixels of text).
const WIDE: &[&str] = &[
    "Intro",
    "| first column header with quite a lot of words | second column header with a good few words | third |",
    "|---|---|---|",
    "| a cell with plenty of words in it to make it wide | another cell with words, enough to be wide too | the last cell of the row |",
    "",
    "after",
];

#[test]
fn a_narrow_table_stays_at_the_column_left_edge() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let mut src = Fake::new(NARROW, t.clone());
    src.cursor = Pos { line: 5, col: 0 };
    let w_pt = 2400.0;
    let (mut v, _) = recording(&t, w_pt);
    v.paint_all(&src);
    let runs = red_runs(&v, mid(&v, &t, 3));
    let (col_x, _) = column_in(&t, w_pt, S);
    assert_eq!(runs.first().map(|r| r.start), Some(col_x as u32));
    let right = runs.last().unwrap().end;
    assert!(
        right < (col_x + 200.0) as u32,
        "not stretched: ends at {right}"
    );
}

#[test]
fn a_wide_table_breaks_out_of_the_column_to_the_right() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let mut src = Fake::new(WIDE, t.clone());
    src.cursor = Pos { line: 5, col: 0 };
    let w_pt = 2400.0;
    let (mut v, _) = recording(&t, w_pt);
    v.paint_all(&src);

    let (col_x, col_w) = column_in(&t, w_pt, S);
    let (wide_x, wide_w) = wide_in(&t, w_pt, S);
    assert_eq!(wide_x, col_x, "the breakout starts where the text does");
    let runs = red_runs(&v, mid(&v, &t, 3));
    assert_eq!(runs.len(), 4, "three columns, four verticals: {runs:?}");
    let (left, right) = (runs[0].start as f32, runs[3].end as f32);
    assert_eq!(left, col_x, "starts at the text's left edge");
    assert!(right > col_x + col_w, "ends right of the column");
    assert!(right <= wide_x + wide_w, "inside the breakout");

    // Room to spare, so no cell wraps: rows are as tall as a narrow table's.
    let mut narrow = Fake::new(NARROW, t.clone());
    narrow.cursor = Pos { line: 5, col: 0 };
    let (mut n, _) = recording(&t, w_pt);
    n.paint_all(&narrow);
    assert_eq!(v.line_height_px(3), n.line_height_px(3), "no wrapping");
}

#[test]
fn a_wide_table_in_a_narrow_window_stays_inside_the_margins() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let mut src = Fake::new(WIDE, t.clone());
    src.cursor = Pos { line: 5, col: 0 };
    let w_pt = 700.0;
    let (mut v, _) = recording(&t, w_pt);
    v.paint_all(&src);
    let (col_x, col_w) = column_in(&t, w_pt, S);
    let runs = red_runs(&v, mid(&v, &t, 3));
    assert_eq!(runs.first().map(|r| r.start as f32), Some(col_x));
    let right = runs.last().unwrap().end as f32;
    assert!(right <= col_x + col_w, "wraps inside the column: {right}");
}

#[test]
fn text_tables_and_code_share_the_left_edge_as_the_window_widens() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let side = t.px(TokenId::SpacePageSide);
    let code = long_code();
    let mut lines: Vec<&str> = WIDE.to_vec();
    lines.extend(["```", code.as_str(), "```", "", PARA]);
    let para = lines.len() - 1;
    for w_pt in [1000.0, 1400.0, 2400.0, 3200.0] {
        let mut src = Fake::new(&lines, t.clone());
        src.cursor = Pos { line: 4, col: 0 };
        let (mut v, laid) = recording(&t, w_pt);
        v.paint_all(&src);
        let (col_x, col_w) = column_in(&t, w_pt, S);
        let (_, wide_w) = wide_in(&t, w_pt, S);
        assert_eq!(col_x, side, "at the fixed margin at {w_pt}");
        assert_eq!(v.text_left_px(para), Some(side), "text at {w_pt}");
        let runs = red_runs(&v, mid(&v, &t, 3));
        assert_eq!(runs[0].start as f32, side, "table at {w_pt}");
        assert_eq!(
            v.text_left_px(7).map(|x| x < side + 40.0),
            Some(true),
            "code at {w_pt}"
        );
        // Past the column's cap the text stops; the breakout keeps going.
        assert!(col_w <= t.px(TokenId::SpaceColumn));
        if w_pt >= 1400.0 {
            assert_eq!(laid_width(&laid, PARA), col_w, "text capped at {w_pt}");
            assert!(wide_w > col_w, "breakout wider than the text at {w_pt}");
        }
    }
}

#[test]
fn a_one_word_header_keeps_its_word_beside_long_urls() {
    // Each URL is wider than the room, so the longest words overflow.
    let url = format!("https://example.com/{}", "segment".repeat(60));
    let lines = [
        "| Status | Link | Mirror |".to_string(),
        "|---|---|---|".to_string(),
        format!("| ok | {url} | {url} |"),
        String::new(),
        "after".to_string(),
    ];
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&lines, t.clone());
    src.cursor = Pos { line: 4, col: 0 };
    let (mut v, laid) = recording(&t, 800.0);
    v.paint_all(&src);
    let laid = laid.borrow();
    let head = laid.iter().rev().find(|l| l.text == "Status").unwrap();
    assert_eq!(head.rows.len(), 1, "the header does not split");
    let link = laid.iter().rev().find(|l| l.text == url).unwrap();
    assert!(link.rows.len() > 1, "the URL breaks instead");
}

#[test]
fn a_click_on_a_broken_out_cell_hits_that_cell() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let mut src = Fake::new(WIDE, t.clone());
    src.cursor = Pos { line: 5, col: 0 };
    let w_pt = 2400.0;
    let (mut v, _) = recording(&t, w_pt);
    v.paint_all(&src);
    let (col_x, col_w) = column_in(&t, w_pt, S);
    let y = mid(&v, &t, 3);
    let runs = red_runs(&v, y);
    // Right of the text column, inside the table.
    let x = col_x + col_w + 10.0;
    assert!(x < runs[3].start as f32, "the point is inside the table");
    let k = runs.iter().rposition(|r| r.end as f32 <= x).unwrap();
    let hit = v.hit_test(x / S, y as f32 / S).expect("a hit");
    assert_eq!(hit.pos.line, 3);
    // Cell `k` runs from just after the `k + 1`th pipe to the next one.
    let pipes: Vec<usize> = WIDE[3].match_indices('|').map(|(i, _)| i).collect();
    assert!(
        (pipes[k]..pipes[k + 1]).contains(&hit.pos.col),
        "in cell {k}, got {:?}",
        hit.pos
    );
}

#[test]
fn a_short_column_keeps_its_word_when_the_table_must_shrink() {
    let long = "words ".repeat(80);
    let row = format!("| A123 | {long}|");
    let lines = [
        "| ID | description |",
        "|---|---|",
        row.as_str(),
        "",
        "after",
    ];
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&lines, t.clone());
    src.cursor = Pos { line: 4, col: 0 };
    let (mut v, laid) = recording(&t, 800.0);
    v.paint_all(&src);
    let laid = laid.borrow();
    let id = laid.iter().rev().find(|l| l.text == "A123").unwrap();
    assert_eq!(id.rows.len(), 1, "the ID cell does not wrap");
    let desc = laid
        .iter()
        .rev()
        .find(|l| l.text == long.trim_end())
        .unwrap();
    assert!(desc.rows.len() > 1, "the long cell wraps instead");
}

/// A code line of about 1150 pixels of mono text: wider than the 900 column,
/// inside the 1530 breakout of a 2400 point window.
fn long_code() -> String {
    format!("let wide = {};", "x".repeat(108))
}

fn code_doc(code: &str) -> Vec<String> {
    ["Intro", "```", "short()", code, "```", "", "after"]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// The code background's run on line `i`: one box, so one run.
fn code_box(v: &DocView, t: &Tokens, i: usize) -> Range<u32> {
    let runs = red_runs(v, mid(v, t, i));
    assert!(!runs.is_empty(), "line {i} paints the code background");
    runs.first().unwrap().start..runs.last().unwrap().end
}

fn code_view(lines: &[String], w_pt: f32) -> (DocView, Tokens, Rc<RefCell<Vec<Laid>>>, Fake) {
    let t = with(&[("color.code.bg", "#FF0000")]);
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut src = Fake::new(&refs, t.clone());
    src.cursor = Pos {
        line: lines.len() - 1,
        col: 0,
    };
    let (mut v, laid) = recording(&t, w_pt);
    v.paint_all(&src);
    (v, t, laid, src)
}

#[test]
fn a_narrow_code_block_keeps_the_column_width() {
    let lines = code_doc("let a = 1;");
    let w_pt = 2400.0;
    let (v, t, _, _) = code_view(&lines, w_pt);
    let (col_x, col_w) = column_in(&t, w_pt, S);
    let b = code_box(&v, &t, 3);
    // The rounded corners only cut the fence lines, so the middle is full.
    assert_eq!(b, col_x as u32..(col_x + col_w) as u32);
}

#[test]
fn a_wide_code_block_grows_to_the_right_without_wrapping() {
    let code = long_code();
    let lines = code_doc(&code);
    let w_pt = 2400.0;
    let (v, t, laid, _) = code_view(&lines, w_pt);
    let (col_x, col_w) = column_in(&t, w_pt, S);
    let (_, wide_w) = wide_in(&t, w_pt, S);
    let b = code_box(&v, &t, 3);
    assert_eq!(b.start, col_x as u32, "starts at the text's left edge");
    assert!(b.end as f32 > col_x + col_w, "ends right of the column");
    assert!(b.end as f32 <= col_x + wide_w, "inside the breakout");
    let l = laid.borrow();
    let line = l.iter().rev().find(|l| l.text == code).unwrap();
    assert_eq!(line.rows.len(), 1, "the long line does not wrap");
    drop(l);
    // One box for the whole block: the short line's is as wide.
    assert_eq!(code_box(&v, &t, 2), b, "every line shares the box");
    let pad = (t.px(TokenId::SpaceCodePad) * S).round();
    let width = (b.end - b.start) as f32;
    assert_eq!(laid_width(&laid, "short()"), width - 2.0 * pad);
}

#[test]
fn a_very_long_code_line_wraps_at_the_breakout() {
    let code = format!("let wide = {};", "x".repeat(400));
    let lines = code_doc(&code);
    let w_pt = 2400.0;
    let (v, t, laid, _) = code_view(&lines, w_pt);
    let (col_x, wide_w) = wide_in(&t, w_pt, S);
    let b = code_box(&v, &t, 3);
    assert_eq!(
        b,
        col_x as u32..(col_x + wide_w) as u32,
        "the whole breakout"
    );
    let l = laid.borrow();
    let line = l.iter().rev().find(|l| l.text == code).unwrap();
    assert!(line.rows.len() > 1, "still too long, so it wraps");
}

#[test]
fn a_wide_code_block_in_a_narrow_window_stays_in_the_column() {
    let lines = code_doc(&long_code());
    let w_pt = 700.0;
    let (v, t, _, _) = code_view(&lines, w_pt);
    let (col_x, col_w) = column_in(&t, w_pt, S);
    assert_eq!(code_box(&v, &t, 3), col_x as u32..(col_x + col_w) as u32);
}

#[test]
fn lengthening_a_code_line_widens_the_whole_block() {
    let lines = code_doc("let a = 1;");
    let w_pt = 2400.0;
    let (mut v, t, _, mut src) = code_view(&lines, w_pt);
    let (col_x, col_w) = column_in(&t, w_pt, S);
    assert_eq!(code_box(&v, &t, 2).end, (col_x + col_w) as u32);
    src.set_line(3, &long_code());
    v.paint_lines(&src, 3..4);
    let wide = code_box(&v, &t, 3);
    assert!(wide.end as f32 > col_x + col_w, "the edited line widens");
    assert_eq!(code_box(&v, &t, 2), wide, "and so does the line above it");
    // Back to short: the box narrows again.
    src.set_line(3, "let a = 1;");
    v.paint_lines(&src, 3..4);
    assert_eq!(code_box(&v, &t, 2).end, (col_x + col_w) as u32);
}

#[test]
fn a_click_on_broken_out_code_hits_its_column() {
    let code = long_code();
    let lines = code_doc(&code);
    let w_pt = 2400.0;
    let (v, t, _, _) = code_view(&lines, w_pt);
    let (col_x, col_w) = column_in(&t, w_pt, S);
    let y = mid(&v, &t, 3) as f32;
    let x = col_x + col_w + 50.0;
    let hit = v.hit_test(x / S, y / S).expect("a hit");
    assert_eq!(hit.pos.line, 3);
    let left = v.text_left_px(3).unwrap();
    let per_char = (code_box(&v, &t, 3).end as f32 - left) / code.len() as f32;
    assert!(
        hit.pos.col > (col_w / per_char) as usize,
        "past what the column holds, got {:?}",
        hit.pos
    );
}

#[test]
fn an_edit_in_a_code_block_measures_only_the_edited_line() {
    let mut lines = vec!["Intro".to_string(), "```".to_string()];
    lines.extend((0..200).map(|k| format!("line_{k}();")));
    lines.extend(["```".to_string(), "after".to_string()]);
    let (mut v, _, laid, mut src) = code_view(&lines, 2400.0);
    // Measuring lays a line out unwrapped, far wider than any window.
    let measured = |laid: &Rc<RefCell<Vec<Laid>>>| {
        let l = laid.borrow();
        l.iter()
            .filter(|l| l.width_px > 100_000.0)
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(measured(&laid).len(), 200, "every content line once");
    laid.borrow_mut().clear();
    src.set_line(100, "line_100(edited);");
    v.paint_lines(&src, 100..101);
    assert_eq!(measured(&laid), ["line_100(edited);"]);
}

#[test]
fn lengthening_a_code_line_off_screen_widens_the_lines_on_screen() {
    let mut lines = vec!["Intro".to_string(), "```".to_string()];
    lines.extend((0..200).map(|k| format!("line_{k}();")));
    lines.extend(["```".to_string(), "after".to_string()]);
    let w_pt = 2400.0;
    let (mut v, t, _, mut src) = code_view(&lines, w_pt);
    let (col_x, col_w) = column_in(&t, w_pt, S);
    assert_eq!(code_box(&v, &t, 3).end, (col_x + col_w) as u32);
    // A substitution far down the block, as `:s` makes with the cursor up top.
    src.cursor = Pos { line: 0, col: 0 };
    src.set_line(150, &long_code());
    v.paint_lines(&src, 150..151);
    assert!(
        code_box(&v, &t, 3).end as f32 > col_x + col_w,
        "the box on screen widens"
    );
}

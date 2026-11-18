//! The fluid page: the text column grows with the window up to
//! `space.column`, its margins scale with the window, and tables wider than
//! the column break out of it, centred, up to `space.column.wide`.

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
        assert_eq!(v.text_left_px(0), Some(col_x), "centred at {w_pt}");
        assert!(col_w >= last, "never narrower in a wider window ({w_pt})");
        assert!(col_w <= cap, "never past space.column ({w_pt})");
        last = col_w;
    }
    // 5% of 800 is 40 a side; 5% of 1000 leaves exactly the 900 cap.
    assert_eq!(column_in(&t, 800.0, S).1, 720.0);
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
    assert_eq!(laid_width(&laid, PARA), 600.0, "20% a side of 1000");
    assert_eq!(v.text_left_px(0), Some(200.0));

    let t = with(&[("space.page.side", "300")]);
    let src = Fake::new(&[PARA], t.clone());
    let (mut v, laid) = recording(&t, 1000.0);
    v.paint_all(&src);
    assert_eq!(laid_width(&laid, PARA), 400.0, "the fixed side beats 5%");
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
fn a_wide_table_breaks_out_of_the_column_centred() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let mut src = Fake::new(WIDE, t.clone());
    src.cursor = Pos { line: 5, col: 0 };
    let w_pt = 2400.0;
    let (mut v, _) = recording(&t, w_pt);
    v.paint_all(&src);

    let (col_x, col_w) = column_in(&t, w_pt, S);
    let (wide_x, wide_w) = wide_in(&t, w_pt, S);
    let runs = red_runs(&v, mid(&v, &t, 3));
    assert_eq!(runs.len(), 4, "three columns, four verticals: {runs:?}");
    let (left, right) = (runs[0].start as f32, runs[3].end as f32);
    assert!(
        left < col_x,
        "starts left of the column ({left} vs {col_x})"
    );
    assert!(right > col_x + col_w, "ends right of the column");
    assert!(
        left >= wide_x && right <= wide_x + wide_w,
        "inside the breakout"
    );
    let centre = (left + right) / 2.0;
    let page = (w_pt * S) / 2.0;
    assert!((centre - page).abs() <= 1.0, "centred: {centre} vs {page}");

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
fn a_click_on_a_broken_out_cell_hits_that_cell() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let mut src = Fake::new(WIDE, t.clone());
    src.cursor = Pos { line: 5, col: 0 };
    let w_pt = 2400.0;
    let (mut v, _) = recording(&t, w_pt);
    v.paint_all(&src);
    let (col_x, _) = column_in(&t, w_pt, S);
    let y = mid(&v, &t, 3);
    let runs = red_runs(&v, y);
    // Just inside the first cell, which sits left of the text column.
    let x = runs[0].end as f32 + 14.0;
    assert!(x < col_x, "the point is in the breakout margin");
    let hit = v.hit_test(x / S, y as f32 / S).expect("a hit");
    assert_eq!(hit.pos.line, 3);
    let first_cell = WIDE[3].find("a cell").unwrap();
    assert!(
        (first_cell..first_cell + 3).contains(&hit.pos.col),
        "at the first cell's start, got {:?}",
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

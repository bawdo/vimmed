//! The table grid picker `:table` opens: its cells, the selection and the
//! label, drawn in `color.overlay.*` under the cursor's line, from the fake
//! source and from a real editor.

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use common::{Fake, H_PT, Laid, Recording, W_PT, cosmic, line_top, open_editor, press, view};
use editor_core::buffer::Pos;
use editor_core::input::Key;
use editor_core::tables::TablePicker;
use editor_core::tokens::{Rgba, Theme, TokenId, Tokens};
use editor_render::view::DocView;

fn rgba(v: &DocView, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let p = v.frame().pixmap.pixel(x, y).unwrap();
    (p.red(), p.green(), p.blue(), p.alpha())
}

fn opaque(c: Rgba) -> (u8, u8, u8, u8) {
    (c.r, c.g, c.b, 255)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("table_picker_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A view whose layout records the text of every line it lays out.
fn recording(t: &Tokens) -> (DocView, Rc<RefCell<Vec<Laid>>>) {
    let laid = Rc::new(RefCell::new(Vec::new()));
    let layout = Recording {
        inner: cosmic(t),
        laid: laid.clone(),
    };
    (DocView::new(Box::new(layout), t, W_PT, H_PT, 1.0), laid)
}

/// Each picker cell's top-left pixel, found through the view's hit-test
/// (at scale 1, a pixel is a point): the least x and y that hit it.
fn cells(v: &DocView) -> BTreeMap<(u32, u32), (u32, u32)> {
    let mut found: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for y in 0..H_PT as u32 {
        for x in 0..W_PT as u32 {
            if let Some(c) = v.picker_cell_at(x as f32 + 0.5, y as f32 + 0.5) {
                let e = found.entry(c).or_insert((x, y));
                e.0 = e.0.min(x);
                e.1 = e.1.min(y);
            }
        }
    }
    found
}

#[test]
fn picker_draws_cells_selection_and_label_under_the_cursor_line() {
    let t = Tokens::defaults(Theme::Light);
    let half = (t.px(TokenId::FontSize) / 2.0) as u32;
    let mut src = Fake::new(&["alpha", "beta", "gamma"], t.clone());
    src.cursor = Pos { line: 1, col: 0 };
    let (mut v, laid) = recording(&t);
    v.paint_all(&src);
    assert_eq!(v.picker_cell_at(W_PT / 2.0, 100.0), None);
    laid.borrow_mut().clear();

    let mut p = TablePicker::new();
    p.select(2, 1);
    src.picker = Some(p);
    // A partial repaint brings it up.
    v.paint_lines(&src, 0..0);
    let found = cells(&v);
    // A 5 x 5 grid.
    assert_eq!(found.len(), 25, "{found:?}");
    assert!(found.contains_key(&(4, 4)));
    // Under the cursor's line.
    let below = line_top(&v, &t, 1.0, 2) as u32;
    assert!(found[&(0, 0)].1 >= below, "{found:?} above {below}");
    // Cells up to the selection are filled in the overlay text colour, the
    // rest show the overlay background.
    let text = opaque(t.color(TokenId::ColorOverlayText));
    let bg = opaque(t.color(TokenId::ColorOverlayBg));
    for (&(c, r), &(x, y)) in &found {
        let want = if c <= 2 && r <= 1 { text } else { bg };
        assert_eq!(rgba(&v, x + half, y + half), want, "cell {c},{r}");
    }
    // The label, `<c> x <r>`.
    let texts: Vec<String> = laid.borrow().iter().map(|l| l.text.clone()).collect();
    assert!(texts.iter().any(|s| s == "3 x 2"), "{texts:?}");

    // A line repaint under it leaves it on top.
    let (x, y) = found[&(0, 0)];
    src.set_line(2, "gamma ray");
    v.paint_lines(&src, 2..3);
    assert_eq!(rgba(&v, x + half, y + half), text);

    // Closed, the page is back.
    src.picker = None;
    v.paint_lines(&src, 0..0);
    assert_eq!(v.picker_cell_at(x as f32 + 0.5, y as f32 + 0.5), None);
    assert_eq!(
        rgba(&v, x + half, y + half),
        opaque(t.color(TokenId::ColorBg))
    );
}

#[test]
fn picker_at_the_bottom_goes_above_the_cursor_line() {
    let t = Tokens::defaults(Theme::Light);
    let lines: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut src = Fake::new(&refs, t.clone());
    // The last line on screen, near the status bar.
    let mut v = view(&t, 1.0);
    v.paint_all(&src);
    let bottom = (H_PT - t.px(TokenId::SizeStatusHeight)) as u32;
    let last = (0..60)
        .take_while(|&i| (line_top(&v, &t, 1.0, i + 1) as u32) < bottom)
        .last()
        .unwrap();
    src.cursor = Pos { line: last, col: 0 };
    src.picker = Some(TablePicker::new());
    v.paint_all(&src);
    let found = cells(&v);
    assert_eq!(found.len(), 25);
    let top = line_top(&v, &t, 1.0, last) as u32;
    let lowest = found.values().map(|&(_, y)| y).max().unwrap();
    assert!(lowest < top, "cells reach {lowest}, line starts at {top}");
}

#[test]
fn table_command_opens_the_picker_and_enter_inserts() {
    let t = Tokens::defaults(Theme::Light);
    let dir = scratch("editor");
    let mut ed = open_editor(&dir, "", false);
    let mut v = view(&t, 1.0);
    v.paint_all(&ed);
    for c in ":table".chars() {
        press(&mut ed, &mut v, Key::Char(c));
    }
    press(&mut ed, &mut v, Key::Enter);
    assert_eq!(cells(&v).len(), 25);
    press(&mut ed, &mut v, Key::Char('l'));
    press(&mut ed, &mut v, Key::Char('j'));
    let text = opaque(t.color(TokenId::ColorOverlayText));
    let half = (t.px(TokenId::FontSize) / 2.0) as u32;
    let found = cells(&v);
    let (x, y) = found[&(1, 1)];
    assert_eq!(rgba(&v, x + half, y + half), text);
    press(&mut ed, &mut v, Key::Enter);
    assert_eq!(v.picker_cell_at(x as f32 + 0.5, y as f32 + 0.5), None);
    assert_eq!(
        ed.text(),
        "| Column 1 | Column 2 |\n| --- | --- |\n|  |  |\n|  |  |"
    );
}

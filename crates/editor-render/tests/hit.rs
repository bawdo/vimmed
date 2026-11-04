//! The mouse hit test: the source position and grapheme under a point, and
//! what the point is over (text, a task's checkbox, a link, the table
//! picker), through hidden markdown syntax.

mod common;

use common::{Fake, Geom, H_PT, W_PT, view};
use editor_core::buffer::Pos;
use editor_core::effects::Scroll;
use editor_core::input::{MouseHit, Zone};
use editor_core::tables::TablePicker;
use editor_core::tokens::{Theme, Tokens};
use editor_render::view::DocView;

/// Every hit across the frame's width at height `y` (points), one point
/// apart, with its x.
fn row(v: &DocView, y: f32) -> Vec<(f32, MouseHit)> {
    (0..W_PT as u32)
        .filter_map(|x| {
            let x = x as f32 + 0.5;
            v.hit_test(x, y).map(|h| (x, h))
        })
        .collect()
}

/// The middle of body line `i` in points, at scale 2, when every line above
/// it is one body row.
fn mid(t: &Tokens, i: u32) -> f32 {
    let g = Geom::new(t, 2.0);
    (g.line_rows(i).start + 10) as f32 / 2.0
}

#[test]
fn hit_test_maps_through_hidden_syntax() {
    let t = Tokens::defaults(Theme::Light);
    let src = Fake::new(&["plain", "**bold** tail"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let y = mid(&t, 1);
    // Line 1 shows "bold tail": the far right of it is the source end.
    let end = v.hit_test(W_PT - 1.0, y).unwrap();
    assert_eq!(end.pos, Pos { line: 1, col: 13 });
    assert_eq!(end.under, end.pos, "past the text, nothing is under it");
    assert_eq!(end.zone, Zone::Text);
    // Column 0 of the display is source byte 2, after the hidden `**`.
    let first = v.hit_test(0.0, y).unwrap();
    assert_eq!(first.pos, Pos { line: 1, col: 2 });
    assert_eq!(first.under, Pos { line: 1, col: 2 });
}

#[test]
fn under_is_the_grapheme_under_the_point_and_pos_the_nearest_boundary() {
    let t = Tokens::defaults(Theme::Light);
    let src = Fake::new(&["", "abc"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let hits = row(&v, mid(&t, 1));
    // Over `a` (source byte 0), the nearest boundary goes from 0 to 1
    // half way across it while the grapheme under stays `a`.
    let on_a: Vec<&MouseHit> = hits
        .iter()
        .map(|(_, h)| h)
        .filter(|h| h.under.col == 0)
        .collect();
    assert!(on_a.iter().any(|h| h.pos.col == 0));
    assert!(on_a.iter().any(|h| h.pos.col == 1));
    assert!(hits.iter().any(|(_, h)| h.under.col == 2 && h.pos.col == 3));
}

#[test]
fn a_rendered_link_is_a_link_zone_with_its_source_range() {
    let t = Tokens::defaults(Theme::Light);
    let src = Fake::new(&["", "see [docs](https://e.com) now"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let hits = row(&v, mid(&t, 1));
    let links: Vec<&MouseHit> = hits
        .iter()
        .map(|(_, h)| h)
        .filter(|h| matches!(h.zone, Zone::Link(_)))
        .collect();
    assert!(!links.is_empty());
    for h in &links {
        assert_eq!(h.zone, Zone::Link(5..9), "{h:?}");
        assert!((5..9).contains(&h.under.col), "{h:?}");
    }
    // `see` and `now` are text.
    assert_eq!(hits[0].1.zone, Zone::Text);
    let last_text = hits.iter().rev().find(|(_, h)| h.under.col < 29).unwrap();
    assert_eq!(last_text.1.zone, Zone::Text);
}

#[test]
fn a_tasks_checkbox_is_its_prefix_zone() {
    let t = Tokens::defaults(Theme::Light);
    let src = Fake::new(&["", "- [ ] task", "- plain"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let hits = row(&v, mid(&t, 1));
    // Where the second char of `task` starts (left of the checkbox, in the
    // margin, the nearest text is the task's start too).
    let first_text = hits
        .iter()
        .find(|(_, h)| h.zone == Zone::Text && h.under.col == 7)
        .map(|(x, _)| *x)
        .expect("the task's text");
    assert_eq!(hits[0].1.zone, Zone::Text, "the margin left of the box");
    let boxes: Vec<f32> = hits
        .iter()
        .filter(|(_, h)| h.zone == Zone::Prefix)
        .map(|(x, _)| *x)
        .collect();
    assert!(!boxes.is_empty(), "{hits:?}");
    assert!(boxes.iter().all(|x| *x < first_text));
    // On the prefix the position is the line's: the task's text start.
    let (_, h) = hits.iter().find(|(_, h)| h.zone == Zone::Prefix).unwrap();
    assert_eq!(h.pos.line, 1);
    // A plain bullet is no checkbox.
    assert!(
        row(&v, mid(&t, 2))
            .iter()
            .all(|(_, h)| h.zone != Zone::Prefix)
    );
    // The cursor's own line shows `- [ ]` raw: no checkbox to click.
    let mut src = src;
    src.cursor = Pos { line: 1, col: 7 };
    v.paint_all(&src);
    assert!(
        row(&v, mid(&t, 1))
            .iter()
            .all(|(_, h)| h.zone != Zone::Prefix)
    );
}

#[test]
fn the_table_picker_is_hit_before_the_text_under_it() {
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&["alpha", "beta", "gamma", "delta"], t.clone());
    src.picker = Some(TablePicker::new());
    let mut v = view(&t, 1.0);
    v.paint_all(&src);
    let mut seen = 0;
    for y in 0..H_PT as u32 {
        for x in 0..W_PT as u32 {
            let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
            if let Some((col, row)) = v.picker_cell_at(x, y) {
                assert_eq!(v.hit_test(x, y).unwrap().zone, Zone::Picker { col, row });
                seen += 1;
            }
        }
    }
    assert!(seen > 0);
}

#[test]
fn the_picker_stays_on_screen_when_its_line_scrolls_off() {
    let t = Tokens::defaults(Theme::Light);
    let lines: Vec<String> = (0..80).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut src = Fake::new(&refs, t.clone());
    src.cursor = Pos { line: 2, col: 0 };
    src.picker = Some(TablePicker::new());
    let mut v = view(&t, 1.0);
    v.paint_all(&src);
    // Scrolled until the cursor's line is far above the top.
    assert!(v.scroll_by(Scroll::Points(400)));
    v.paint_all(&src);
    let g = Geom::new(&t, 1.0);
    let top = (0..g.text_bottom())
        .find(|&y| (0..W_PT as u32).any(|x| v.picker_cell_at(x as f32, y as f32).is_some()));
    assert_eq!(
        top.map(|y| y < 40),
        Some(true),
        "the picker sits at the top: {top:?}"
    );
}

#[test]
fn the_picker_fits_a_narrow_frame() {
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&["alpha"], t.clone());
    src.picker = Some(TablePicker::new());
    // So narrow the column's left edge plus the panel would pass the
    // right edge.
    let mut v = DocView::new(Box::new(common::cosmic(&t)), &t, 100.0, H_PT, 1.0);
    v.paint_all(&src);
    let rightmost = (0..100u32)
        .rev()
        .find(|&x| (0..H_PT as u32).any(|y| v.picker_cell_at(x as f32, y as f32) == Some((4, 0))));
    assert!(rightmost.is_some(), "the last column is on screen");
}

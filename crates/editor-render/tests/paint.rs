mod common;

use std::cell::Cell;
use std::rc::Rc;

use common::{Counting, Fake, Geom, H_PT, W_PT, cosmic, view};
use editor_core::buffer::Pos;
use editor_core::tokens::{Theme, TokenId, Tokens};
use editor_render::damage::Damage;
use editor_render::frame::{Frame, copy_rows_bgra};
use editor_render::layout::{FontRole, StyledSpan, TextLayout};
use editor_render::view::DocView;

fn with(pairs: &[(&str, &str)]) -> Tokens {
    let mut t = Tokens::defaults(Theme::Light);
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert!(t.apply_overrides(&pairs).is_empty());
    t
}

#[test]
fn paint_uses_tokens() {
    let t = with(&[("color.bg", "#000000")]);
    let src = Fake::new(&["alpha"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let p = v.frame().pixmap.pixel(1, 1).unwrap();
    assert_eq!((p.red(), p.green(), p.blue(), p.alpha()), (0, 0, 0, 255));
}

#[test]
fn text_paints_in_the_text_colour() {
    let t = with(&[("color.text", "#FF0000")]);
    let g = Geom::new(&t, 2.0);
    let src = Fake::new(&["Hello world"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let rows = g.line_rows(0);
    let f = v.frame();
    let red = (rows.start..rows.end)
        .flat_map(|y| (0..f.width_px).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = f.pixmap.pixel(x, y).unwrap();
            p.red() > 200 && p.green() < 60 && p.blue() < 60
        })
        .count();
    assert!(red > 50, "only {red} red pixels on the first line");
}

#[test]
fn status_paints_dirty_in_its_token_colour() {
    let t = with(&[("color.status.dirty", "#00FF00")]);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["alpha"], t.clone());
    let mut v = view(&t, 2.0);
    let green = |v: &DocView| {
        let f = v.frame();
        (g.text_bottom()..g.height_px)
            .flat_map(|y| (0..f.width_px).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = f.pixmap.pixel(x, y).unwrap();
                p.green() > 200 && p.red() < 60 && p.blue() < 60
            })
            .count()
    };
    v.paint_all(&src);
    assert_eq!(green(&v), 0, "clean: no dirty marker");
    v.take_damage();
    src.dirty = true;
    v.paint_lines(&src, 0..0);
    assert!(green(&v) > 20, "dirty: the marker paints");
    assert_eq!(v.take_damage().0, vec![g.text_bottom()..g.height_px]);
}

#[test]
fn copy_rows_bgra_swizzles_only_damaged_rows() {
    let mut f = Frame::new(4, 4, 1.0);
    for px in f.pixmap.data_mut().as_chunks_mut::<4>().0 {
        px.copy_from_slice(&[1, 2, 3, 4]);
    }
    let stride = 4 * 4 + 8;
    let mut dst = vec![0xAA; stride * 4];
    let mut d = Damage::default();
    d.add(1..2);
    d.add(3..4);
    copy_rows_bgra(&f, &mut dst, stride, &d);
    for y in 0..4 {
        let row = &dst[y * stride..(y + 1) * stride];
        if y == 1 || y == 3 {
            for px in row[..16].as_chunks::<4>().0 {
                assert_eq!(*px, [3, 2, 1, 4], "row {y}");
            }
        } else {
            assert!(row[..16].iter().all(|&b| b == 0xAA), "row {y} untouched");
        }
        assert!(row[16..].iter().all(|&b| b == 0xAA), "row {y} padding");
    }
}

#[test]
fn copy_rows_bgra_tolerates_a_stale_destination() {
    let f = Frame::new(8, 8, 1.0);
    let mut d = Damage::default();
    d.add(0..8);
    // A destination sized for an older, smaller frame: narrower stride, fewer rows.
    let mut dst = vec![0u8; 4 * 4 * 3];
    copy_rows_bgra(&f, &mut dst, 4 * 4, &d);
    // Damage past the frame's own height is ignored too.
    let mut far = Damage::default();
    far.add(100..200);
    let mut big = vec![0u8; 8 * 4 * 8];
    copy_rows_bgra(&f, &mut big, 8 * 4, &far);
}

#[test]
fn resize_then_copy_with_old_buffer_does_not_crash() {
    let t = Tokens::defaults(Theme::Light);
    let src = Fake::new(&["alpha"], t.clone());
    let mut v = view(&t, 1.0);
    v.paint_all(&src);
    let old = v.frame();
    let stride = old.width_px as usize * 4;
    let mut dst = vec![0u8; stride * old.height_px as usize];
    v.take_damage();
    v.resize(W_PT, H_PT, 2.0);
    v.paint_all(&src);
    let d = v.take_damage();
    copy_rows_bgra(v.frame(), &mut dst, stride, &d);
}

#[test]
fn caret_and_hit_round_trip_on_a_laid_line() {
    let t = Tokens::defaults(Theme::Light);
    let mut l = cosmic(&t);
    let span = StyledSpan {
        range: 0..11,
        role: FontRole::Body,
        size_px: 32.0,
        weight: 400,
        italic: false,
        color: t.color(TokenId::ColorText),
        strike: false,
        underline: false,
    };
    let laid = l.layout_line("hello world", &[span], 1000.0);
    assert_eq!(laid.rows, 1);
    assert_eq!(laid.height, 48.0);
    let a = laid.caret(0);
    let b = laid.caret(6);
    let end = laid.caret(11);
    assert_eq!(a.x, 0.0);
    assert!(a.x < b.x && b.x < end.x);
    assert_eq!(laid.hit(b.x + 1.0, 10.0), 6);
    assert_eq!(laid.hit(end.x + 50.0, 10.0), 11);
}

#[test]
fn hit_test_maps_through_hidden_syntax() {
    let t = Tokens::defaults(Theme::Light);
    let src = Fake::new(&["plain", "**bold** tail"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let g = Geom::new(&t, 2.0);
    // Line 1 shows "bold tail": the far right of it is the source end.
    let y = (g.line_rows(1).start + 10) as f32 / 2.0;
    let end = v.hit_test(W_PT - 1.0, y).unwrap();
    assert_eq!(end, Pos { line: 1, col: 13 });
    // Column 0 of the display is source byte 2, after the hidden `**`.
    let first = v.hit_test(0.0, y).unwrap();
    assert_eq!(first, Pos { line: 1, col: 2 });
}

#[test]
fn caret_rect_follows_the_cursor() {
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&["alpha", "beta"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let r0 = v.caret_rect().unwrap();
    src.cursor = Pos { line: 1, col: 2 };
    v.paint_lines(&src, 1..2);
    let r1 = v.caret_rect().unwrap();
    assert!(r1.y > r0.y);
    assert!(r1.x > r0.x);
    src.reader = true;
    v.paint_all(&src);
    assert_eq!(v.caret_rect(), None, "no caret in reader mode");
}

#[test]
fn preedit_paints_underlined_at_the_caret() {
    let t = with(&[("color.preedit.underline", "#00FF00")]);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["ab cd"], t.clone());
    src.cursor = Pos { line: 0, col: 2 };
    let mut v = view(&t, 2.0);
    let green = |v: &DocView| {
        let f = v.frame();
        let rows = g.line_rows(0);
        (rows.start..rows.end)
            .flat_map(|y| (0..f.width_px).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = f.pixmap.pixel(x, y).unwrap();
                p.green() > 200 && p.red() < 60 && p.blue() < 60
            })
            .count()
    };
    v.paint_all(&src);
    assert_eq!(green(&v), 0, "no composition: nothing in its colour");
    let plain = v.caret_rect().unwrap();

    src.preedit = Some(("xyz".into(), 2));
    v.paint_lines(&src, 0..1);
    assert!(green(&v) > 50, "the composition paints in its colour");
    let spans = v.styled_spans(0).unwrap();
    let pre = spans
        .iter()
        .find(|s| s.range == (2..5))
        .expect("a span over the composition");
    assert!(pre.underline);
    assert_eq!(pre.color, t.color(TokenId::ColorPreeditUnderline));
    // The caret sits inside the composition, at its cursor.
    let inside = v.caret_rect().unwrap();
    assert!(inside.x > plain.x, "{} > {}", inside.x, plain.x);

    // Gone again: the line paints as before.
    src.preedit = None;
    v.paint_lines(&src, 0..1);
    assert_eq!(green(&v), 0);
    assert_eq!(v.caret_rect().unwrap().x, plain.x);
}

#[test]
fn scroll_to_cursor_scrolls_only_when_needed() {
    let t = Tokens::defaults(Theme::Light);
    let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let src = Fake::new(&refs, t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    assert!(!v.scroll_to_cursor(Pos { line: 1, col: 0 }));
    assert!(v.scroll_to_cursor(Pos { line: 90, col: 0 }));
    v.paint_all(&src);
    assert!(!v.scroll_to_cursor(Pos { line: 90, col: 0 }));
    assert!(v.scroll_to_cursor(Pos { line: 0, col: 0 }));
}

/// Trim cadence: undamaged on-screen lines keep their layout across a
/// keystroke frame sequence (partial, partial, full), so the full paint lays
/// out nothing and never depends on the shape-run cache for them.
#[test]
fn full_paint_after_partial_paints_lays_out_only_changed_lines() {
    let t = Tokens::defaults(Theme::Light);
    let calls = Rc::new(Cell::new(0));
    let layout = Counting {
        inner: cosmic(&t),
        calls: calls.clone(),
    };
    let mut src = Fake::new(&["alpha", "beta", "gamma", "delta"], t.clone());
    let mut v = DocView::new(Box::new(layout), &t, W_PT, H_PT, 2.0);
    v.paint_all(&src);
    assert!(calls.get() >= 4, "first paint lays out every line");

    calls.set(0);
    src.set_line(1, "betas");
    v.paint_lines(&src, 1..2);
    assert_eq!(calls.get(), 1, "partial: only the edited line");

    calls.set(0);
    src.set_line(1, "betass");
    v.paint_lines(&src, 1..2);
    assert_eq!(calls.get(), 1, "partial: only the edited line");

    calls.set(0);
    v.paint_all(&src);
    assert_eq!(calls.get(), 0, "full: nothing changed since the last frame");
}

#[test]
fn poll_async_has_nothing_pending() {
    let t = Tokens::defaults(Theme::Light);
    let mut v = view(&t, 1.0);
    assert!(!v.poll_async());
}

/// Lines scrolled off screen release their layouts but keep their measured
/// heights, so memory stays bounded by the screen and positions stay stable.
#[test]
fn scrolled_off_lines_release_layouts_and_keep_heights() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 2.0);
    let long = "a long line of ordinary words that keeps going ".repeat(6);
    let mut lines: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
    lines[1] = long;
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let src = Fake::new(&refs, t.clone());
    let calls = Rc::new(Cell::new(0));
    let layout = Counting {
        inner: cosmic(&t),
        calls: calls.clone(),
    };
    let mut v = DocView::new(Box::new(layout), &t, W_PT, H_PT, 2.0);
    v.paint_all(&src);
    assert!(v.is_shaped(0) && v.is_shaped(1));
    let wrapped = v.line_height_px(1).unwrap();
    assert!(wrapped > g.body_line as f32, "line 1 wraps");

    assert!(v.scroll_to_cursor(Pos { line: 190, col: 0 }));
    v.paint_all(&src);
    assert!(v.is_shaped(190));
    for i in 0..100 {
        assert!(
            !v.is_shaped(i),
            "line {i} is off screen and holds no layout"
        );
    }
    assert_eq!(v.line_height_px(1), Some(wrapped), "measured height kept");
    let shaped = (0..200).filter(|&i| v.is_shaped(i)).count();
    assert!(
        shaped <= 30,
        "{shaped} lines shaped; only the screen should be"
    );

    calls.set(0);
    assert!(v.scroll_to_cursor(Pos { line: 0, col: 0 }));
    v.paint_all(&src);
    assert!(calls.get() > 0, "scrolling back lays the screen out again");
    assert!(!v.is_shaped(190));
}

mod common;

use common::{Fake, Geom, H_PT, W_PT, view};
use editor_core::tokens::{Theme, Tokens};
use editor_render::damage::Damage;

#[test]
fn damage_keeps_far_ranges_separate() {
    let mut d = Damage::default();
    d.add(10..20);
    d.add(900..920);
    assert_eq!(d.0, vec![10..20, 900..920]);
}

#[test]
fn damage_merges_touching() {
    let mut d = Damage::default();
    d.add(10..20);
    d.add(20..30);
    assert_eq!(d.0, vec![10..30]);
    d.add(5..12);
    assert_eq!(d.0, vec![5..30]);
    d.add(31..40);
    assert_eq!(d.0, vec![5..30, 31..40], "a one-row gap is not touching");
    d.add(0..100);
    assert_eq!(d.0, vec![0..100]);
}

#[test]
fn damage_ignores_empty_ranges_and_unions() {
    let mut a = Damage::default();
    a.add(7..7);
    assert!(a.0.is_empty());
    a.add(0..4);
    let mut b = Damage::default();
    b.add(4..8);
    b.add(50..60);
    assert_eq!(a.union(&b).0, vec![0..8, 50..60]);
}

#[test]
fn edit_same_height_damages_one_line_and_status_separately() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["alpha", "beta", "gamma"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    v.take_damage();

    src.set_line(1, "betas");
    src.dirty = true;
    v.paint_lines(&src, 1..2);
    let d = v.take_damage();
    assert_eq!(
        d.0,
        vec![g.line_rows(1), g.text_bottom()..g.height_px],
        "the edited line and the status bar, unmerged"
    );
}

#[test]
fn edit_that_wraps_damages_to_bottom() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["alpha", "beta", "gamma"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    v.take_damage();

    let long = "a long line of ordinary words that keeps going ".repeat(6);
    src.set_line(1, &long);
    v.paint_lines(&src, 1..2);
    let d = v.take_damage();
    assert_eq!(d.0, vec![g.line_rows(1).start..g.text_bottom()]);
}

#[test]
fn edit_on_a_line_count_change_damages_to_bottom() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["alpha", "beta", "gamma"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    v.take_damage();

    src = Fake::new(&["alpha", "beta", "new", "gamma"], t.clone());
    v.paint_lines(&src, 2..3);
    assert_eq!(
        v.take_damage().0,
        vec![g.line_rows(2).start..g.text_bottom()]
    );
}

#[test]
fn resize_to_new_scale_damages_everything() {
    let t = Tokens::defaults(Theme::Light);
    let src = Fake::new(&["alpha", "beta"], t.clone());
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    assert_eq!(v.frame().pixmap.height(), (H_PT * 2.0) as u32);
    v.take_damage();

    v.resize(W_PT, H_PT, 1.0);
    let f = v.frame();
    assert_eq!((f.width_px, f.height_px), (W_PT as u32, H_PT as u32));
    assert_eq!(
        (f.pixmap.width(), f.pixmap.height()),
        (W_PT as u32, H_PT as u32)
    );
    assert_eq!(f.scale, 1.0);
    v.paint_all(&src);
    assert_eq!(v.take_damage().0, vec![0..H_PT as u32]);
}

#[test]
fn resize_damage_stands_without_a_repaint() {
    let t = Tokens::defaults(Theme::Light);
    let mut v = view(&t, 2.0);
    v.take_damage();
    v.resize(W_PT / 2.0, H_PT / 2.0, 1.0);
    assert_eq!(v.take_damage().0, vec![0..(H_PT / 2.0) as u32]);
}

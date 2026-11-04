//! Reader view (no caret, no status bar, keys that scroll) and the help
//! overlay, painted from a real editor or the fake source.

mod common;

use common::{Fake, Geom, H_PT, W_PT, open_editor, press, press_with, view};
use editor_core::input::{Key, Mods};
use editor_core::tokens::{Rgba, Theme, TokenId, Tokens};
use editor_core::view_model::ViewSource;
use editor_render::view::DocView;

fn rgba(v: &DocView, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let p = v.frame().pixmap.pixel(x, y).unwrap();
    (p.red(), p.green(), p.blue(), p.alpha())
}

fn opaque(c: Rgba) -> (u8, u8, u8, u8) {
    (c.r, c.g, c.b, 255)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("reader_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn primary_shift() -> Mods {
    Mods {
        cmd: true,
        shift: true,
        ..Mods::default()
    }
}

/// The source line at the top of the text area, `dy` points below it.
fn line_at(v: &DocView, t: &Tokens, dy: f32) -> usize {
    let y = t.px(TokenId::SpacePageTop) + dy;
    v.hit_test(W_PT / 2.0, y).expect("a painted line").line
}

#[test]
fn view_flag_starts_reader_and_hides_status() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 1.0);
    let dir = scratch("view");
    let mut ed = open_editor(&dir, "# Title\n\nsome **bold** text", true);
    assert!(ed.reader());
    let mut v = view(&t, 1.0);
    v.paint_all(&ed);
    assert_eq!(v.caret_rect(), None);
    // The page runs to the bottom: no status bar.
    let bottom = g.height_px - 1;
    assert_eq!(rgba(&v, 1, bottom), opaque(t.color(TokenId::ColorBg)));

    // Leaving reader view brings the status bar and caret back.
    press_with(&mut ed, &mut v, Key::Char('R'), primary_shift());
    assert!(!ed.reader());
    assert_eq!(rgba(&v, 1, bottom), opaque(t.color(TokenId::ColorStatusBg)));
    assert!(v.caret_rect().is_some());
    assert!(g.text_bottom() < g.height_px);

    press_with(&mut ed, &mut v, Key::Char('R'), primary_shift());
    assert!(ed.reader());
    assert_eq!(rgba(&v, 1, bottom), opaque(t.color(TokenId::ColorBg)));
    assert_eq!(v.caret_rect(), None);
}

#[test]
fn reader_keys_scroll_the_view() {
    let t = Tokens::defaults(Theme::Light);
    let dir = scratch("scroll");
    let text: Vec<String> = (0..120).map(|i| format!("line {i}")).collect();
    let mut ed = open_editor(&dir, &text.join("\n"), true);
    let mut v = view(&t, 1.0);
    v.paint_all(&ed);
    assert_eq!(line_at(&v, &t, 1.0), 0);
    press(&mut ed, &mut v, Key::Char('j'));
    assert_eq!(line_at(&v, &t, 1.0), 1);
    press(&mut ed, &mut v, Key::Char('k'));
    assert_eq!(line_at(&v, &t, 1.0), 0);
    press(&mut ed, &mut v, Key::PageDown);
    let page = line_at(&v, &t, 1.0);
    assert!(page > 5, "a page down moved {page} lines");
    press(&mut ed, &mut v, Key::PageUp);
    assert_eq!(line_at(&v, &t, 1.0), 0);
    press(&mut ed, &mut v, Key::Char('G'));
    // The last line sits at the bottom of the window.
    let y = H_PT - 2.0;
    assert_eq!(v.hit_test(W_PT / 2.0, y).unwrap().line, 119);
    press(&mut ed, &mut v, Key::Char('g'));
    press(&mut ed, &mut v, Key::Char('g'));
    assert_eq!(line_at(&v, &t, 1.0), 0);
    // A scroll up at the top goes nowhere.
    press(&mut ed, &mut v, Key::Char('k'));
    assert_eq!(line_at(&v, &t, 1.0), 0);
}

#[test]
fn help_overlay_paints_in_overlay_colours() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 1.0);
    let mut src = Fake::new(&["alpha", "beta"], t.clone());
    let mut v = view(&t, 1.0);
    v.paint_all(&src);
    assert_eq!(rgba(&v, 1, 1), opaque(t.color(TokenId::ColorBg)));

    src.help = Some(0);
    // A partial repaint still brings the overlay up.
    v.paint_lines(&src, 0..0);
    assert_eq!(rgba(&v, 1, 1), opaque(t.color(TokenId::ColorOverlayBg)));
    let first = v.frame().pixmap.data().to_vec();
    // The status bar stays as it was.
    let bottom = g.height_px - 1;
    assert_eq!(rgba(&v, 1, bottom), opaque(t.color(TokenId::ColorStatusBg)));
    // Text in the overlay's own colour shows somewhere.
    let text = opaque(t.color(TokenId::ColorOverlayText));
    let w = v.frame().pixmap.width();
    let found = (0..g.text_bottom()).any(|y| (0..w).any(|x| rgba(&v, x, y) == text));
    assert!(found, "no overlay text drawn");

    // Scrolled a row, it draws differently.
    src.help = Some(1);
    v.paint_all(&src);
    assert!(
        v.frame().pixmap.data() != &first[..],
        "scrolling the help changed nothing"
    );

    // Dismissed, the page is back.
    src.help = None;
    v.paint_lines(&src, 0..0);
    assert_eq!(rgba(&v, 1, 1), opaque(t.color(TokenId::ColorBg)));
}

#[test]
fn help_overlay_survives_a_line_repaint() {
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&["alpha", "beta"], t.clone());
    src.help = Some(0);
    let mut v = view(&t, 1.0);
    v.paint_all(&src);
    let before = v.frame().pixmap.data().to_vec();
    // A line repaint under the overlay (a message fading, say) leaves the
    // overlay on top.
    src.set_line(0, "alphabet");
    v.paint_lines(&src, 0..1);
    assert!(
        v.frame().pixmap.data() == &before[..],
        "the line painted over the overlay"
    );
}

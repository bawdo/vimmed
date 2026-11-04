//! Reader view (no caret, the status line kept, keys that scroll) and the
//! help overlay, painted from a real editor or the fake source.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{
    Fake, Geom, H_PT, Laid, Recording, W_PT, cosmic, open_editor, press, press_with, view,
};
use editor_core::input::{Key, Mods};
use editor_core::strings;
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
    v.hit_test(W_PT / 2.0, y).expect("a painted line").pos.line
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

/// Every text laid out since the last call, and forgets them.
fn laid_texts(laid: &Rc<RefCell<Vec<Laid>>>) -> Vec<String> {
    laid.borrow_mut().drain(..).map(|l| l.text).collect()
}

/// Owner decision D21 (overriding the brief's "status line hidden"): in
/// reader view the status line stays at its normal height. It names
/// reader view and leaves out the hidden cursor's position; the caret
/// stays hidden.
#[test]
fn view_flag_starts_reader_with_status_line() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 1.0);
    let dir = scratch("view");
    let mut ed = open_editor(&dir, "# Title\n\nsome **bold** text", true);
    assert!(ed.reader());
    let (mut v, laid) = recording(&t);
    v.paint_all(&ed);
    assert_eq!(v.caret_rect(), None);
    let bottom = g.height_px - 1;
    assert_eq!(rgba(&v, 1, bottom), opaque(t.color(TokenId::ColorStatusBg)));
    let texts = laid_texts(&laid);
    assert!(texts.iter().any(|s| s == strings::MODE_READER), "{texts:?}");
    assert!(texts.iter().any(|s| s == "doc.md"), "{texts:?}");
    assert!(
        !texts.iter().any(|s| s == "1:1"),
        "position shown: {texts:?}"
    );

    // Leaving reader view brings the caret and position back; the status
    // line was there all along.
    press_with(&mut ed, &mut v, Key::Char('R'), primary_shift());
    assert!(!ed.reader());
    assert_eq!(rgba(&v, 1, bottom), opaque(t.color(TokenId::ColorStatusBg)));
    assert!(v.caret_rect().is_some());
    let texts = laid_texts(&laid);
    assert!(texts.iter().any(|s| s == strings::MODE_NORMAL), "{texts:?}");
    assert!(texts.iter().any(|s| s == "1:1"), "{texts:?}");

    press_with(&mut ed, &mut v, Key::Char('R'), primary_shift());
    assert!(ed.reader());
    assert_eq!(rgba(&v, 1, bottom), opaque(t.color(TokenId::ColorStatusBg)));
    assert_eq!(v.caret_rect(), None);
}

#[test]
fn reader_shows_messages_in_the_status_line() {
    let t = Tokens::defaults(Theme::Light);
    let dir = scratch("message");
    let mut ed = open_editor(&dir, "abc", false);
    let (mut v, laid) = recording(&t);
    v.paint_all(&ed);
    press(&mut ed, &mut v, Key::Char('x'));
    assert!(ed.dirty());
    press_with(&mut ed, &mut v, Key::Char('R'), primary_shift());
    laid_texts(&laid);
    press(&mut ed, &mut v, Key::Char('Z'));
    press(&mut ed, &mut v, Key::Char('Z'));
    let texts = laid_texts(&laid);
    assert!(
        texts.iter().any(|s| s == strings::READ_ONLY_ZQ),
        "message not painted: {texts:?}"
    );
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
    // The last line sits just above the status line.
    let y = Geom::new(&t, 1.0).text_bottom() as f32 - 2.0;
    assert_eq!(v.hit_test(W_PT / 2.0, y).unwrap().pos.line, 119);
    press(&mut ed, &mut v, Key::Char('g'));
    press(&mut ed, &mut v, Key::Char('g'));
    assert_eq!(line_at(&v, &t, 1.0), 0);
    // A scroll up at the top goes nowhere.
    press(&mut ed, &mut v, Key::Char('k'));
    assert_eq!(line_at(&v, &t, 1.0), 0);
}

#[test]
fn leaving_reader_keeps_the_caret_above_the_status_bar() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 1.0);
    let dir = scratch("leave");
    let text: Vec<String> = (0..120).map(|i| format!("line {i}")).collect();
    let mut ed = open_editor(&dir, &text.join("\n"), false);
    let mut v = view(&t, 1.0);
    v.paint_all(&ed);
    // The cursor on the last line, at the bottom of the text area.
    press(&mut ed, &mut v, Key::Char('G'));
    press_with(&mut ed, &mut v, Key::Char('R'), primary_shift());
    // Scrolled up a line in reader view, the cursor's line goes off the
    // bottom of the text area; leaving reader view brings it back into
    // view, above the status line.
    press(&mut ed, &mut v, Key::Char('k'));
    press_with(&mut ed, &mut v, Key::Char('R'), primary_shift());
    assert!(!ed.reader());
    let caret = v.caret_rect().expect("a caret");
    assert!(
        caret.y + caret.h <= g.text_bottom() as f32,
        "caret bottom {} below the text area's {}",
        caret.y + caret.h,
        g.text_bottom()
    );
}

#[test]
fn counted_g_brings_that_line_to_the_top() {
    let t = Tokens::defaults(Theme::Light);
    let dir = scratch("count_g");
    let text: Vec<String> = (0..120).map(|i| format!("line {i}")).collect();
    let mut ed = open_editor(&dir, &text.join("\n"), true);
    let mut v = view(&t, 1.0);
    v.paint_all(&ed);
    for c in ['1', '0', 'G'] {
        press(&mut ed, &mut v, Key::Char(c));
    }
    assert_eq!(line_at(&v, &t, 1.0), 9);
    // Past what the end allows, the last line stays at the bottom.
    for c in ['1', '1', '8', 'g', 'g'] {
        press(&mut ed, &mut v, Key::Char(c));
    }
    let bottom = Geom::new(&t, 1.0).text_bottom() as f32 - 2.0;
    assert_eq!(v.hit_test(W_PT / 2.0, bottom).unwrap().pos.line, 119);
    assert!(line_at(&v, &t, 1.0) < 117);
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

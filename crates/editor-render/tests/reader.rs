//! Reader view (no caret, the status line kept, keys that scroll) and the
//! help overlay, painted from a real editor or the fake source.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{
    Fake, Geom, H_PT, Laid, Recording, W_PT, cosmic, mouse_at, open_editor, press, press_with, view,
};
use editor_core::input::{Key, Mods, MouseKind, Zone};
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

/// The middle of the status bar's page icon, in points, as last painted.
fn icon_mid(v: &DocView) -> (f32, f32) {
    let r = v.reader_icon_rect().expect("a page icon");
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// A click on the status bar's page icon toggles reader view both ways,
/// back to the cursor and mode it was entered from.
#[test]
fn the_page_icon_toggles_reader_view() {
    let t = Tokens::defaults(Theme::Light);
    let dir = scratch("icon");
    let mut ed = open_editor(&dir, "one\ntwo\nthree", false);
    let (mut v, laid) = recording(&t);
    v.paint_all(&ed);
    press(&mut ed, &mut v, Key::Char('j'));
    let (x, y) = icon_mid(&v);
    assert_eq!(v.hit_test(x, y).unwrap().zone, Zone::ReaderToggle);
    laid_texts(&laid);

    mouse_at(&mut ed, &mut v, MouseKind::Down, x, y);
    assert!(ed.reader());
    mouse_at(&mut ed, &mut v, MouseKind::Up, x, y);
    assert!(ed.reader(), "the release does not toggle it back");
    assert_eq!(v.caret_rect(), None);
    let texts = laid_texts(&laid);
    assert!(texts.iter().any(|s| s == strings::MODE_READER), "{texts:?}");

    mouse_at(&mut ed, &mut v, MouseKind::Down, x, y);
    mouse_at(&mut ed, &mut v, MouseKind::Up, x, y);
    assert!(!ed.reader());
    assert_eq!(ed.cursor().line, 1, "the cursor is where it was");
    assert!(v.caret_rect().is_some());
}

/// A drag from the text that ends on the page icon selects and toggles
/// nothing at the icon; while `:` is typed the icon does nothing.
#[test]
fn the_page_icon_ignores_drags_and_the_command_line() {
    let t = Tokens::defaults(Theme::Light);
    let dir = scratch("icon_drag");
    let mut ed = open_editor(&dir, "one two three", false);
    let mut v = view(&t, 1.0);
    v.paint_all(&ed);
    let (x, y) = icon_mid(&v);
    let text_y = t.px(TokenId::SpacePageTop) + 5.0;
    let text_x = v.text_left_px(0).unwrap() + 2.0;
    mouse_at(&mut ed, &mut v, MouseKind::Down, text_x, text_y);
    mouse_at(&mut ed, &mut v, MouseKind::Drag, x, y);
    mouse_at(&mut ed, &mut v, MouseKind::Up, x, y);
    assert!(!ed.reader());

    press(&mut ed, &mut v, Key::Esc);
    press(&mut ed, &mut v, Key::Char(':'));
    mouse_at(&mut ed, &mut v, MouseKind::Down, x, y);
    assert!(!ed.reader(), "refused while `:` is typed");
}

/// `:view` turns reader view on, and in reader view `:view` (the only
/// command its command line runs) turns it off again.
#[test]
fn view_command_toggles_reader_view() {
    let t = Tokens::defaults(Theme::Light);
    let dir = scratch("view_cmd");
    let mut ed = open_editor(&dir, "some text", false);
    let mut v = view(&t, 1.0);
    v.paint_all(&ed);
    let colon = |ed: &mut _, v: &mut _, cmd: &str| {
        press(ed, v, Key::Char(':'));
        for c in cmd.chars() {
            press(ed, v, Key::Char(c));
        }
        press(ed, v, Key::Enter);
    };
    colon(&mut ed, &mut v, "view");
    assert!(ed.reader());
    assert_eq!(v.caret_rect(), None);
    colon(&mut ed, &mut v, "w");
    assert!(ed.reader());
    assert_eq!(ed.message(), Some(strings::READER_ONLY_VIEW));
    colon(&mut ed, &mut v, "reader");
    assert!(!ed.reader());
    assert!(v.caret_rect().is_some());
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

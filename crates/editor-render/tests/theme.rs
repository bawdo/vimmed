//! A live switch to the dark theme repaints everything in dark tokens: the
//! page, the status bar, the help overlay and the table grid picker leave
//! no light-theme colour behind.

mod common;

use common::{H_PT, W_PT, open_editor, press, view};
use editor_core::editor::Editor;
use editor_core::effects::Redraw;
use editor_core::input::{Input, Key};
use editor_core::tokens::{Rgba, Theme, TokenId, Tokens};
use editor_render::ViewSource;
use editor_render::view::DocView;

/// A pixel, as (r, g, b, a).
type Px = (u8, u8, u8, u8);

fn rgba(v: &DocView, x: u32, y: u32) -> Px {
    let p = v.frame().pixmap.pixel(x, y).unwrap();
    (p.red(), p.green(), p.blue(), p.alpha())
}

fn opaque(c: Rgba) -> Px {
    (c.r, c.g, c.b, 255)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("theme_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The opaque light-theme colours that no dark-theme token shares.
fn light_only() -> Vec<Px> {
    let (light, dark) = (
        Tokens::defaults(Theme::Light),
        Tokens::defaults(Theme::Dark),
    );
    let colours = || {
        TokenId::ALL
            .iter()
            .filter(|id| id.name().starts_with("color."))
    };
    let dark: Vec<_> = colours().map(|id| opaque(dark.color(*id))).collect();
    let mut out: Vec<_> = colours()
        .map(|id| light.color(*id))
        .filter(|c| c.a == 255)
        .map(opaque)
        .filter(|c| !dark.contains(c))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every pixel of the frame that is a light-theme-only colour, as
/// (x, y, colour), up to a handful.
fn light_pixels(v: &DocView) -> Vec<(u32, u32, Px)> {
    let light = light_only();
    let (w, h) = (v.frame().pixmap.width(), v.frame().pixmap.height());
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| (x, y, rgba(v, x, y)))
        .filter(|(_, _, c)| light.contains(c))
        .take(8)
        .collect()
}

/// An editor and its view, painted light, then switched to dark as the
/// shell does when the system's appearance changes.
fn switched_to_dark(name: &str, text: &str) -> (Editor, DocView) {
    let light = Tokens::defaults(Theme::Light);
    let mut ed = open_editor(&scratch(name), text, false);
    let mut v = view(&light, 1.0);
    v.paint_all(&ed);
    assert_eq!(rgba(&v, 1, 1), opaque(light.color(TokenId::ColorBg)));
    // So the checks below can see a light colour left behind.
    assert!(!light_pixels(&v).is_empty());
    let fx = ed.handle(Input::Appearance(Theme::Dark));
    assert_eq!(fx.redraw, Redraw::All);
    v.paint_all(&ed);
    v.take_damage();
    (ed, v)
}

fn command(ed: &mut Editor, v: &mut DocView, cmd: &str) {
    for c in cmd.chars() {
        press(ed, v, Key::Char(c));
    }
    press(ed, v, Key::Enter);
}

#[test]
fn page_and_status_bar_repaint_dark() {
    let dark = Tokens::defaults(Theme::Dark);
    let (_ed, v) = switched_to_dark(
        "page",
        "# Heading\n\nSome **bold** text and a [link](x.md).\n\n```rust\nfn main() {}\n```\n",
    );
    assert_eq!(rgba(&v, 1, 1), opaque(dark.color(TokenId::ColorBg)));
    let bottom = H_PT as u32 - 1;
    assert_eq!(
        rgba(&v, 1, bottom),
        opaque(dark.color(TokenId::ColorStatusBg))
    );
    assert_eq!(light_pixels(&v), []);
}

#[test]
fn help_overlay_paints_with_dark_tokens() {
    let dark = Tokens::defaults(Theme::Dark);
    let (mut ed, mut v) = switched_to_dark("help", "alpha\nbeta\n");
    command(&mut ed, &mut v, ":help");
    assert!(ed.help().is_some(), "the help overlay is up");
    assert_eq!(rgba(&v, 1, 1), opaque(dark.color(TokenId::ColorOverlayBg)));
    let text = opaque(dark.color(TokenId::ColorOverlayText));
    let w = W_PT as u32;
    let found = (0..H_PT as u32).any(|y| (0..w).any(|x| rgba(&v, x, y) == text));
    assert!(found, "no overlay text in the dark overlay colour");
    assert_eq!(light_pixels(&v), []);
}

#[test]
fn table_picker_paints_with_dark_tokens() {
    let dark = Tokens::defaults(Theme::Dark);
    let (mut ed, mut v) = switched_to_dark("picker", "");
    command(&mut ed, &mut v, ":table");
    assert!(ed.table_picker().is_some(), "the picker is up");
    // A cell's middle, in the dark overlay colours.
    let half = (dark.px(TokenId::FontSize) / 2.0) as u32;
    let (x, y) = (0..H_PT as u32)
        .flat_map(|y| (0..W_PT as u32).map(move |x| (x, y)))
        .find(|&(x, y)| v.picker_cell_at(x as f32 + 0.5, y as f32 + 0.5) == Some((4, 4)))
        .expect("a picker cell");
    assert_eq!(
        rgba(&v, x + half, y + half),
        opaque(dark.color(TokenId::ColorOverlayBg))
    );
    assert_eq!(light_pixels(&v), []);
}

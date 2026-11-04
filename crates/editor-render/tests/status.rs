//! The status line, the command line typed in it, and the highlights and
//! caret painted over the text. Text checks go through `Probe`, a layout
//! that records what is drawn; pixel checks sample the frame.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{Fake, Geom, H_PT, W_PT, view};
use editor_core::buffer::Pos;
use editor_core::tokens::{Rgba, Theme, TokenId, Tokens};
use editor_core::view_model::{CaretShape, MsgKind, Selection, StatusLeft};
use editor_render::layout::TextLayout;
use editor_render::layout::{FontRole, GlyphGeom, GlyphMask, LaidLine, RowGeom, StyledSpan};
use editor_render::view::DocView;

/// Every glyph is this many pixels wide in a `Probe` layout.
const ADVANCE: f32 = 8.0;

/// One string a `Probe` was asked to draw, with the spans it was laid out
/// with.
#[derive(Debug, Clone)]
struct Drawn {
    text: String,
    spans: Vec<StyledSpan>,
}

/// A layout with fixed metrics (each byte `ADVANCE` wide, rows 1.25 times
/// the font size) that records every line it draws and draws no ink, so a
/// sampled pixel shows only the fills under the text.
#[derive(Default, Clone)]
struct Probe {
    drawn: Rc<RefCell<Vec<Drawn>>>,
}

impl Probe {
    fn take(&self) -> Vec<Drawn> {
        std::mem::take(&mut self.drawn.borrow_mut())
    }
}

impl TextLayout for Probe {
    fn layout_line(&mut self, text: &str, spans: &[StyledSpan], _width_px: f32) -> LaidLine {
        let size = spans.first().map_or(10.0, |s| s.size_px);
        let glyphs = text
            .char_indices()
            .map(|(i, c)| GlyphGeom {
                bytes: i..i + c.len_utf8(),
                x: i as f32 * ADVANCE,
                w: ADVANCE * c.len_utf8() as f32,
            })
            .collect();
        let row = RowGeom {
            top: 0.0,
            height: (size * 1.25).ceil(),
            width: text.len() as f32 * ADVANCE,
            text: 0..text.len(),
            glyphs,
        };
        let inner = Drawn {
            text: text.to_owned(),
            spans: spans.to_vec(),
        };
        LaidLine::new(vec![row], Some(Box::new(inner)))
    }
    fn draw(&mut self, line: &LaidLine, _sink: &mut dyn FnMut(GlyphMask)) {
        if let Some(d) = line.inner::<Drawn>() {
            self.drawn.borrow_mut().push(d.clone());
        }
    }
}

fn tokens(pairs: &[(&str, &str)]) -> Tokens {
    let mut t = Tokens::defaults(Theme::Light);
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert!(t.apply_overrides(&pairs).is_empty());
    t
}

fn probed(t: &Tokens) -> (DocView, Probe) {
    let probe = Probe::default();
    let v = DocView::new(Box::new(probe.clone()), t, W_PT, H_PT, 1.0);
    (v, probe)
}

/// What the status bar drew: the lines laid out at `font.size.status`.
fn status_drawn(probe: &Probe, t: &Tokens) -> Vec<Drawn> {
    let size = t.px(TokenId::FontSizeStatus);
    probe
        .take()
        .into_iter()
        .filter(|d| d.spans.first().is_some_and(|s| s.size_px == size))
        .collect()
}

fn texts(drawn: &[Drawn]) -> Vec<&str> {
    drawn.iter().map(|d| d.text.as_str()).collect()
}

fn color_of(drawn: &[Drawn], text: &str) -> Rgba {
    let d = drawn.iter().find(|d| d.text == text);
    d.unwrap_or_else(|| panic!("{text} not drawn in {:?}", texts(drawn)))
        .spans[0]
        .color
}

fn rgba(v: &DocView, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let p = v.frame().pixmap.pixel(x, y).unwrap();
    (p.red(), p.green(), p.blue(), p.alpha())
}

fn opaque(c: Rgba) -> (u8, u8, u8, u8) {
    (c.r, c.g, c.b, 255)
}

#[test]
fn status_line_shows_mode_file_dirty_and_position_in_mono() {
    let t = Tokens::defaults(Theme::Light);
    let (mut v, probe) = probed(&t);
    let mut src = Fake::new(&["alpha"], t.clone());
    src.left = StatusLeft::Mode("NORMAL");
    src.dirty = true;
    v.paint_all(&src);
    let drawn = status_drawn(&probe, &t);
    assert_eq!(
        texts(&drawn),
        ["NORMAL", "notes.md", "changes not saved", "1:1"]
    );
    for d in &drawn {
        assert_eq!(d.spans[0].role, FontRole::Mono, "{}", d.text);
    }
    assert_eq!(
        color_of(&drawn, "changes not saved"),
        t.color(TokenId::ColorStatusDirty)
    );
    assert_eq!(
        color_of(&drawn, "NORMAL"),
        t.color(TokenId::ColorStatusText)
    );
}

#[test]
fn a_message_replaces_the_mode_in_the_colour_of_its_kind() {
    let t = Tokens::defaults(Theme::Light);
    let (mut v, probe) = probed(&t);
    let mut src = Fake::new(&["alpha"], t.clone());
    for (kind, token) in [
        (MsgKind::Error, TokenId::ColorStatusError),
        (MsgKind::Warning, TokenId::ColorStatusWarning),
        (MsgKind::Info, TokenId::ColorStatusText),
    ] {
        src.left = StatusLeft::Message {
            text: format!("said {kind:?}"),
            kind,
        };
        v.paint_lines(&src, 0..0);
        let drawn = status_drawn(&probe, &t);
        assert!(!texts(&drawn).contains(&"INSERT"), "{:?}", texts(&drawn));
        assert_eq!(color_of(&drawn, &format!("said {kind:?}")), t.color(token));
    }
}

#[test]
fn disk_warnings_show_in_the_warning_colour() {
    let t = Tokens::defaults(Theme::Light);
    let (mut v, probe) = probed(&t);
    let mut src = Fake::new(&["alpha"], t.clone());
    src.warnings = vec!["read-only"];
    v.paint_all(&src);
    let drawn = status_drawn(&probe, &t);
    assert_eq!(
        color_of(&drawn, "read-only"),
        t.color(TokenId::ColorStatusWarning)
    );
}

#[test]
fn showcmd_shows_before_the_position() {
    let t = Tokens::defaults(Theme::Light);
    let (mut v, probe) = probed(&t);
    let mut src = Fake::new(&["alpha"], t.clone());
    src.showcmd = "2d".into();
    v.paint_all(&src);
    let drawn = status_drawn(&probe, &t);
    let at = |s: &str| texts(&drawn).iter().position(|d| *d == s).unwrap();
    assert!(at("2d") < at("1:1"), "{:?}", texts(&drawn));
}

#[test]
fn the_command_line_is_typed_in_place_with_its_caret() {
    let t = tokens(&[("color.caret", "#FF00FF")]);
    let g = Geom::new(&t, 1.0);
    let (mut v, probe) = probed(&t);
    let mut src = Fake::new(&["alpha"], t.clone());
    src.caret = CaretShape::Hidden;
    src.left = StatusLeft::Input {
        prefix: ':',
        text: "s/a".into(),
        cursor: 3,
    };
    v.paint_all(&src);
    let drawn = status_drawn(&probe, &t);
    assert_eq!(texts(&drawn)[0], ":s/a");
    // The caret after `:s/a`, `space.status.pad` in.
    let pad = t.px(TokenId::SpaceStatusPad);
    let x = (pad + 4.0 * ADVANCE) as u32;
    let y = g.text_bottom() + g.status_h / 2;
    assert_eq!(rgba(&v, x, y), (255, 0, 255, 255));
    // A mode label has no caret.
    src.left = StatusLeft::Mode("NORMAL");
    v.paint_lines(&src, 0..0);
    assert_eq!(rgba(&v, x, y), opaque(t.color(TokenId::ColorStatusBg)));
}

#[test]
fn status_change_damages_only_status_rows() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["alpha", "beta"], t.clone());
    src.caret = CaretShape::Block;
    src.left = StatusLeft::Mode("NORMAL");
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    v.take_damage();
    let rows = g.text_bottom()..g.height_px;
    let status = std::slice::from_ref(&rows);

    src.left = StatusLeft::Mode("VISUAL");
    v.paint_lines(&src, 0..0);
    assert_eq!(v.take_damage().0, status, "mode label");

    src.showcmd = "2".into();
    v.paint_lines(&src, 0..0);
    assert_eq!(v.take_damage().0, status, "showcmd");

    src.left = StatusLeft::Message {
        text: "E486: Pattern not found: x".into(),
        kind: MsgKind::Error,
    };
    v.paint_lines(&src, 0..0);
    assert_eq!(v.take_damage().0, status, "message");

    // Typing on the command line, with the caret up there.
    src.caret = CaretShape::Hidden;
    src.left = StatusLeft::Input {
        prefix: ':',
        text: String::new(),
        cursor: 0,
    };
    v.paint_lines(&src, 0..0);
    v.take_damage();
    src.left = StatusLeft::Input {
        prefix: ':',
        text: "w".into(),
        cursor: 1,
    };
    v.paint_lines(&src, 0..0);
    assert_eq!(v.take_damage().0, status, "command line");

    // Nothing changed: nothing is damaged.
    v.paint_lines(&src, 0..0);
    assert!(v.take_damage().0.is_empty());
}

#[test]
fn the_caret_hiding_repaints_its_line() {
    let t = Tokens::defaults(Theme::Light);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["alpha", "beta"], t.clone());
    src.caret = CaretShape::Block;
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    v.take_damage();
    src.caret = CaretShape::Hidden;
    src.left = StatusLeft::Input {
        prefix: ':',
        text: String::new(),
        cursor: 0,
    };
    v.paint_lines(&src, 0..0);
    assert_eq!(
        v.take_damage().0,
        vec![g.line_rows(0), g.text_bottom()..g.height_px]
    );
}

#[test]
fn search_matches_painted_with_tokens() {
    let t = tokens(&[
        ("color.search.match", "#00FF00"),
        ("color.search.current", "#0000FF"),
    ]);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["alpha beta alpha", "plain"], t.clone());
    src.cursor = Pos { line: 1, col: 0 };
    src.caret = CaretShape::Hidden;
    src.matches[0] = vec![0..5, 11..16];
    src.current = Some((0, 11..16));
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let x0 = v.text_left_px(0).unwrap() as u32;
    // In the leading above the glyphs, so only the highlight shows.
    let y = g.line_rows(0).start + 1;
    assert_eq!(rgba(&v, x0 + 2, y), (0, 255, 0, 255), "a match");
    let row = g.line_rows(0);
    let w = v.frame().width_px;
    let blue = (x0..w)
        .filter(|&x| rgba(&v, x, row.start + 1) == (0, 0, 255, 255))
        .count();
    assert!(blue > 20, "the current match: {blue} px");
    assert_eq!(
        rgba(&v, x0 + 2, g.line_rows(1).start + 1),
        opaque(t.color(TokenId::ColorBg)),
        "no match on line 1"
    );
}

#[test]
fn a_fresh_visual_selection_paints_the_grapheme_under_it() {
    let t = tokens(&[("color.selection", "#FFFF00"), ("color.caret", "#FFFF00")]);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["alpha", "beta"], t.clone());
    src.caret = CaretShape::Hidden;
    src.cursor = Pos { line: 1, col: 0 };
    src.selection = Some(Selection {
        anchor: Pos { line: 0, col: 1 },
        head: Pos { line: 0, col: 1 },
        linewise: false,
    });
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let x0 = v.text_left_px(0).unwrap() as u32;
    let y = g.line_rows(0).start + 1;
    let yellow: Vec<u32> = (x0..x0 + 200)
        .filter(|&x| rgba(&v, x, y) == (255, 255, 0, 255))
        .collect();
    // Only `l` (not `a`, not `p`): a few pixels, starting past `a`.
    assert!(yellow.len() >= 4, "{yellow:?}");
    assert!(yellow[0] > x0 + 4, "{yellow:?}");
    assert!(yellow.len() < 30, "{yellow:?}");
}

#[test]
fn normal_mode_caret_is_a_block_and_insert_a_bar() {
    let t = tokens(&[("color.caret", "#FF00FF")]);
    let g = Geom::new(&t, 2.0);
    let mut src = Fake::new(&["alpha"], t.clone());
    let magenta = |v: &DocView| {
        let x0 = v.text_left_px(0).unwrap() as u32;
        let y = g.line_rows(0).start + 1;
        (x0.saturating_sub(4)..x0 + 100)
            .filter(|&x| rgba(v, x, y) == (255, 0, 255, 255))
            .count()
    };
    src.caret = CaretShape::Bar;
    let mut v = view(&t, 2.0);
    v.paint_all(&src);
    let bar = (t.px(TokenId::SizeCaret) * 2.0).round() as usize;
    assert_eq!(magenta(&v), bar, "Insert: a bar `size.caret` wide");

    src.caret = CaretShape::Block;
    v.take_damage();
    v.paint_lines(&src, 0..0);
    assert!(magenta(&v) > 2 * bar, "Normal: a block over `a`");
    assert!(
        v.take_damage().0.contains(&g.line_rows(0)),
        "the caret's line repaints when its shape changes"
    );
}

//! Full block rendering: tables, code blocks, quotes, inline styles, list
//! prefixes and hanging prefixes.

mod common;

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Fake, Laid, Recording, column, cosmic, line_top, view};
use editor_core::buffer::Pos;
use editor_core::tokens::{Rgba, Theme, TokenId, Tokens};
use editor_render::layout::FontRole;
use editor_render::view::DocView;

const S: f32 = 2.0;

fn with(pairs: &[(&str, &str)]) -> Tokens {
    let mut t = Tokens::defaults(Theme::Light);
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert!(t.apply_overrides(&pairs).is_empty());
    t
}

fn at(line: usize, col: usize) -> Pos {
    Pos { line, col }
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

/// Red pixels anywhere in line `i`'s rows.
fn red_in_line(v: &DocView, t: &Tokens, i: usize) -> usize {
    let top = line_top(v, t, S, i) as u32;
    let h = v.line_height_px(i).unwrap() as u32;
    (top..top + h)
        .flat_map(|y| (0..v.frame().width_px).map(move |x| (x, y)))
        .filter(|&(x, y)| is_red(v, x, y))
        .count()
}

/// The first x (left to right) at or after `from` on row `y` whose pixel is
/// not the page background.
fn first_ink(v: &DocView, t: &Tokens, y: u32, from: u32) -> Option<u32> {
    let bg = t.color(TokenId::ColorBg);
    (from..v.frame().width_px).find(|&x| {
        let p = v.frame().pixmap.pixel(x, y).unwrap();
        (p.red(), p.green(), p.blue()) != (bg.r, bg.g, bg.b)
    })
}

fn recording(t: &Tokens) -> (DocView, Rc<RefCell<Vec<Laid>>>) {
    let laid = Rc::new(RefCell::new(Vec::new()));
    let layout = Recording {
        inner: cosmic(t),
        laid: laid.clone(),
    };
    let v = DocView::new(Box::new(layout), t, common::W_PT, common::H_PT, S);
    (v, laid)
}

const TABLE: &[&str] = &[
    "Intro",
    "| a | x |",
    "|---|---|",
    "| cccccccccc | xx |",
    "| e | xxxx |",
    "",
    "after",
];

#[test]
fn table_cells_align_across_rows() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let mut src = Fake::new(TABLE, t.clone());
    src.cursor = at(6, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);

    let mut borders = Vec::new();
    let mut col2_ink = Vec::new();
    for i in [1, 3, 4] {
        let top = line_top(&v, &t, S, i);
        let y = (top + v.line_height_px(i).unwrap() / 2.0) as u32;
        let runs = red_runs(&v, y);
        assert_eq!(
            runs.len(),
            3,
            "line {i}: two columns need three verticals, got {runs:?}"
        );
        col2_ink.push(first_ink(&v, &t, y, runs[1].end));
        borders.push(runs);
    }
    assert_eq!(borders[0], borders[1], "column borders match on every row");
    assert_eq!(borders[1], borders[2], "column borders match on every row");
    let ink: Vec<u32> = col2_ink.into_iter().map(Option::unwrap).collect();
    assert_eq!(ink[1], ink[2], "column 2 text starts at one x");
    // The header is strong: its `x` has its own side bearing.
    assert!(
        ink[0].abs_diff(ink[1]) <= 1,
        "header column 2 at {}, rows at {}",
        ink[0],
        ink[1]
    );
    // The widest first-column cell sets the column: a short cell does not.
    let (col_x, _) = column(&t, S);
    assert!(borders[0][1].start as f32 > col_x + 60.0 * S);
}

#[test]
fn table_reveals_raw_when_cursor_inside() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let mut src = Fake::new(TABLE, t.clone());
    src.cursor = at(6, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);
    for i in 1..5 {
        assert!(red_in_line(&v, &t, i) > 0, "line {i}: the grid shows");
    }

    src.cursor = at(3, 2);
    v.paint_all(&src);
    let body = (t.px(TokenId::FontSize) * t.ratio(TokenId::LineHeightBody) * S).ceil();
    for i in 1..5 {
        assert_eq!(red_in_line(&v, &t, i), 0, "line {i}: raw, no grid");
        assert_eq!(
            v.line_height_px(i),
            Some(body),
            "line {i}: a plain text line"
        );
    }
}

#[test]
fn table_edit_outside_the_cursor_repaints_the_whole_table() {
    let t = with(&[("color.table.border", "#FF0000")]);
    let mut src = Fake::new(TABLE, t.clone());
    src.cursor = at(0, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);
    v.take_damage();
    let header_top = line_top(&v, &t, S, 1) as u32;
    let y = header_top + 4;
    let before = red_runs(&v, y);

    // An edit that does not move the cursor (a substitution, say) widens column 2.
    src.set_line(4, "| e | xxxxxxxxxxxxxxxxxxxxxxxx |");
    v.paint_lines(&src, 4..5);
    let damage = v.take_damage().0;
    assert!(
        damage.iter().any(|r| r.contains(&y)),
        "the header row is repainted: {damage:?}"
    );
    let after = red_runs(&v, y);
    assert_ne!(before, after, "the header's right border moved");
}

#[test]
fn unclosed_fence_gets_accent_bar() {
    let t = with(&[("color.code.unclosed", "#FF0000")]);
    let mut src = Fake::new(&["para", "```", "one", "two"], t.clone());
    src.cursor = at(0, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);
    let (col_x, _) = column(&t, S);
    let bar = (t.px(TokenId::SizeUnclosedBar) * S).round() as u32;

    let mid = |i: usize| (line_top(&v, &t, S, i) + v.line_height_px(i).unwrap() / 2.0) as u32;
    let runs = red_runs(&v, mid(3));
    assert_eq!(
        runs,
        vec![col_x as u32..col_x as u32 + bar],
        "accent on the last line"
    );
    assert!(red_runs(&v, mid(2)).is_empty(), "only the last line");

    // A closed block has no accent.
    let src = Fake::new(&["para", "```", "one", "```"], t.clone());
    v.paint_all(&src);
    assert_eq!(red_in_line(&v, &t, 2) + red_in_line(&v, &t, 3), 0);
}

#[test]
fn nested_quote_draws_two_bars() {
    let t = with(&[("color.quote.bar", "#FF0000")]);
    let mut src = Fake::new(&[">> deep", "x"], t.clone());
    src.cursor = at(1, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);
    let y = (line_top(&v, &t, S, 0) + v.line_height_px(0).unwrap() / 2.0) as u32;
    let runs = red_runs(&v, y);
    let bar = (t.px(TokenId::SizeQuoteBar) * S).round() as u32;
    let gap = (t.px(TokenId::SpaceQuoteGap) * S).round() as u32;
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[0].len() as u32, bar);
    assert_eq!(runs[1].start - runs[0].end, gap);
}

#[test]
fn reveal_shifts_rest_of_line_without_overlap() {
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&["a **b** *c*"], t.clone());
    src.cursor = at(0, 3);
    let (mut v, laid) = recording(&t);
    v.paint_all(&src);
    let laid = laid.borrow();
    let line = laid
        .iter()
        .find(|l| l.text == "a **b** c")
        .expect("the revealed bold and the rendered italic");
    let glyphs: Vec<_> = line.rows.iter().flat_map(|r| r.glyphs.iter()).collect();
    let b_right = glyphs
        .iter()
        .filter(|g| g.bytes.start >= 2 && g.bytes.end <= 7)
        .map(|g| g.x + g.w)
        .fold(f32::MIN, f32::max);
    let c_left = glyphs
        .iter()
        .filter(|g| g.bytes.start >= 8)
        .map(|g| g.x)
        .fold(f32::MAX, f32::min);
    assert!(b_right > 0.0 && c_left < f32::MAX);
    assert!(
        c_left >= b_right,
        "c at {c_left} overlaps **b** ending at {b_right}"
    );
}

#[test]
fn code_block_shows_language_label() {
    let t = with(&[("color.code.label", "#FF0000")]);
    let mut src = Fake::new(&["para", "```rust", "let a = 1;", "```"], t.clone());
    src.cursor = at(0, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);
    let (col_x, col_w) = column(&t, S);
    let top = line_top(&v, &t, S, 1) as u32;
    let h = v.line_height_px(1).unwrap() as u32;
    let right_half = (col_x + col_w / 2.0) as u32..(col_x + col_w) as u32;
    let red = (top..top + h)
        .flat_map(|y| right_half.clone().map(move |x| (x, y)))
        .filter(|&(x, y)| is_red(&v, x, y))
        .count();
    assert!(red > 10, "the label paints top right, {red} pixels");
    assert_eq!(
        red_in_line(&v, &t, 2),
        0,
        "the label is on the fence line only"
    );
}

#[test]
fn code_block_corners_are_rounded() {
    let t = with(&[("color.code.bg", "#FF0000")]);
    let mut src = Fake::new(&["para", "```", "x", "```"], t.clone());
    src.cursor = at(0, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);
    let (col_x, col_w) = column(&t, S);
    let r = (t.px(TokenId::RadiusCode) * S).round() as u32;
    let top = line_top(&v, &t, S, 1) as u32;
    let x0 = col_x as u32;
    assert!(!is_red(&v, x0, top), "top left corner is cut");
    assert!(
        !is_red(&v, x0 + col_w as u32 - 1, top),
        "top right corner is cut"
    );
    assert!(is_red(&v, x0 + r, top + r), "inside the corner is filled");
    let last = line_top(&v, &t, S, 3) as u32 + v.line_height_px(3).unwrap() as u32 - 1;
    assert!(!is_red(&v, x0, last), "bottom left corner is cut");
    assert!(
        is_red(&v, x0, top + r + 1),
        "the edge between corners is filled"
    );
}

#[test]
fn inline_code_paints_on_code_background() {
    let t = with(&[("color.code.bg", "#FF0000")]);
    let mut src = Fake::new(&["a `code` b", "x"], t.clone());
    src.cursor = at(1, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);
    assert!(
        red_in_line(&v, &t, 0) > 50,
        "inline code sits on color.code.bg"
    );
}

#[test]
fn inline_code_on_a_heading_keeps_the_heading_size() {
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&["# Title `code`", "x"], t.clone());
    src.cursor = at(1, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);
    let spans = v.styled_spans(0).expect("line 0 is shaped");
    let code = spans
        .iter()
        .find(|s| s.role == FontRole::Mono)
        .expect("a mono span");
    assert_eq!(code.size_px, t.px(TokenId::FontSizeH1) * S);
}

#[test]
fn links_are_underlined_in_the_link_colour() {
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&["see [here](http://x.test) now", "x"], t.clone());
    src.cursor = at(1, 0);
    let mut v = view(&t, S);
    v.paint_all(&src);
    let spans = v.styled_spans(0).unwrap();
    let link = spans
        .iter()
        .find(|s| s.color == t.color(TokenId::ColorLink))
        .expect("a link span");
    assert!(link.underline);
    assert!(spans.iter().filter(|s| s.underline).count() == 1);
}

#[test]
fn wide_list_numbers_do_not_wrap_or_overlap_the_text() {
    let t = Tokens::defaults(Theme::Light);
    let lines: Vec<String> = (0..1000).map(|_| "1. item".to_string()).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut src = Fake::new(&refs, t.clone());
    src.cursor = at(0, 0);
    let (mut v, laid) = recording(&t);
    v.paint_all(&src);
    src.cursor = at(999, 0);
    v.scroll_to_cursor(src.cursor);
    src.cursor = at(0, 0);
    laid.borrow_mut().clear();
    v.paint_all(&src);
    let laid = laid.borrow();
    let num = laid
        .iter()
        .find(|l| l.text == "1000.")
        .expect("the 1000th marker");
    assert_eq!(num.rows.len(), 1, "the marker does not wrap");
    let marker_w = num.rows[0].width;
    let (col_x, _) = column(&t, S);
    let left = v.text_left_px(999).expect("line 999 is shaped");
    assert!(
        left - col_x >= marker_w,
        "text at {left} overlaps the marker ({marker_w} wide from {col_x})"
    );
}

#[test]
fn hanging_prefix_keeps_heading_text_still_when_revealed() {
    let on = with(&[("size.hanging_prefix", "32")]);
    let mut src = Fake::new(&["## Head", "body"], on.clone());
    src.cursor = at(1, 0);
    let mut v = view(&on, S);
    v.paint_all(&src);
    let (col_x, _) = column(&on, S);
    let y = (line_top(&v, &on, S, 0) + v.line_height_px(0).unwrap() / 2.0) as u32;
    let rendered = first_ink(&v, &on, y, 0);

    src.cursor = at(0, 4);
    v.paint_all(&src);
    assert!(
        first_ink(&v, &on, y, 0).is_some_and(|x| (x as f32) < col_x),
        "the revealed `##` hangs in the margin"
    );
    assert_eq!(
        first_ink(&v, &on, y, col_x as u32),
        rendered,
        "the heading text does not move"
    );

    // Off by default: the revealed prefix sits in the column.
    let off = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&["## Head", "body"], off.clone());
    src.cursor = at(0, 4);
    let mut v = view(&off, S);
    v.paint_all(&src);
    assert!(first_ink(&v, &off, y, 0).is_some_and(|x| x as f32 >= col_x));
}

/// Paints and polls until `done` holds, or fails on timeout.
fn until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn keyword() -> Rgba {
    Tokens::defaults(Theme::Light).color(TokenId::ColorSyntaxKeyword)
}

/// The colour of the span covering display byte `b` of line `i`.
fn colour_at(v: &DocView, i: usize, b: usize) -> Rgba {
    v.styled_spans(i)
        .unwrap()
        .iter()
        .find(|s| s.range.contains(&b))
        .map(|s| s.color)
        .unwrap()
}

#[test]
fn code_blocks_highlight_and_stale_spans_follow_their_lines() {
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(
        &["intro", "```rust", "fn a() {}", "let b = 1;", "```", "end"],
        t.clone(),
    );
    src.cursor = at(0, 0);
    let mut v = view(&t, S);
    v.set_highlighting(Arc::new(|| {}));
    v.paint_all(&src);
    assert_eq!(v.styled_spans(2).unwrap().len(), 1, "first paint is plain");
    until("the block to highlight", || v.poll_async());
    assert_eq!(colour_at(&v, 2, 0), keyword(), "`fn` is a keyword");
    assert_eq!(colour_at(&v, 3, 0), keyword(), "`let` is a keyword");

    // A line inserted at the top of the block: until the reparse lands, every
    // line keeps its own spans (or none), never its old neighbour's.
    src.insert_line(2, "// note");
    v.paint_lines(&src, 2..3);
    assert_eq!(v.styled_spans(2).unwrap().len(), 1, "the new line is plain");
    assert_eq!(colour_at(&v, 3, 0), keyword(), "`fn` keeps its colour");
    assert_eq!(colour_at(&v, 4, 0), keyword(), "`let` keeps its colour");
    assert_ne!(
        colour_at(&v, 3, 3),
        keyword(),
        "`a` is not coloured as a keyword"
    );

    until("the reparse to land", || v.poll_async());
    assert_eq!(
        colour_at(&v, 2, 0),
        t.color(TokenId::ColorSyntaxComment),
        "the new line is a comment"
    );
}

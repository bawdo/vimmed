//! Measured check of the keystroke draw cost (ignored by default, timing is
//! meaningful only in a release build). Run `cargo xtask fixture` first, then:
//!
//! `cargo test -p editor-render --release --test keystroke_timing -- --ignored --nocapture`
//!
//! Both cases use a 900x700 pt view at scale 2 and the committed test fonts.
//!
//! * `one_line_edit_on_fixture`: types one character into a body line of the
//!   generated fixture (the line keeps its height, so only its own rows are
//!   repainted) and back again, 600 `paint_lines` calls.
//! * `height_changing_edit_on_large_document`: a document of about 10,000
//!   lines, where an edit at the top alternately wraps and unwraps a line, so
//!   every `paint_lines` goes through `repaint_to_bottom`.
//!
//! The timer wraps `paint_lines` only. Rebuilding the fake source's block model
//! between iterations is outside it.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Fake, cosmic};
use editor_core::buffer::Pos;
use editor_core::tokens::{Theme, Tokens};
use editor_render::view::DocView;

const W_PT: f32 = 900.0;
const H_PT: f32 = 700.0;
const SCALE: f32 = 2.0;

fn fixture_lines() -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fixture/test.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} missing: run `cargo xtask fixture`", path.display()));
    text.lines().map(str::to_string).collect()
}

fn view(t: &Tokens) -> DocView {
    DocView::new(Box::new(cosmic(t)), t, W_PT, H_PT, SCALE)
}

fn report(label: &str, mut times: Vec<Duration>) {
    times.sort();
    let n = times.len();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "{label}: n={n} p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms",
        ms(times[n / 2]),
        ms(times[(n * 95 / 100).min(n - 1)]),
        ms(times[n - 1])
    );
}

#[test]
#[ignore = "timing: run in release with --ignored --nocapture"]
fn one_line_edit_on_fixture() {
    const ITERS: usize = 600;
    let lines = fixture_lines();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&refs, t.clone());
    let mut v = view(&t);

    // The prose line with bold, italic and inline code (fits on one row).
    let edit = lines
        .iter()
        .position(|l| l.starts_with("This paragraph"))
        .expect("fixture prose line not found");
    println!("editing fixture line {} ({:?})", edit + 1, lines[edit]);
    src.cursor = Pos {
        line: edit,
        col: lines[edit].len(),
    };
    v.paint_all(&src);
    v.take_damage();

    let mut times = Vec::with_capacity(ITERS);
    for n in 0..ITERS {
        let text = if n % 2 == 0 {
            format!("{}x", lines[edit])
        } else {
            lines[edit].clone()
        };
        src.set_line(edit, &text);
        src.cursor.col = text.len();
        let start = Instant::now();
        v.paint_lines(&src, edit..edit + 1);
        times.push(start.elapsed());
        v.take_damage();
    }
    report("one-line edit, fixture, 900x700pt @2x", times);
}

#[test]
#[ignore = "timing: run in release with --ignored --nocapture"]
fn height_changing_edit_on_large_document() {
    const ITERS: usize = 200;
    let base = fixture_lines();
    let mut lines: Vec<String> = Vec::new();
    while lines.len() < 10_000 {
        lines.extend(base.iter().cloned());
    }
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&refs, t.clone());
    let mut v = view(&t);

    // Replace line 0 with prose, so the toggle below wraps and unwraps it.
    src.set_line(0, "alpha");
    src.cursor = Pos { line: 0, col: 5 };
    v.paint_all(&src);
    v.take_damage();

    let long = "a long line of ordinary words that keeps going ".repeat(8);
    let mut times = Vec::with_capacity(ITERS);
    for n in 0..ITERS {
        let text = if n % 2 == 0 { long.as_str() } else { "alpha" };
        src.set_line(0, text);
        src.cursor.col = text.len();
        let start = Instant::now();
        v.paint_lines(&src, 0..1);
        times.push(start.elapsed());
        v.take_damage();
    }
    println!("document lines: {}", lines.len());
    report(
        "height-changing edit at top, 10k-line doc, 900x700pt @2x",
        times,
    );
}

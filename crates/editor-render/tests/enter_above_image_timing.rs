//! Measured check of an Enter above an image (ignored by default, timing is
//! meaningful only in a release build). Run `cargo xtask fixture` first, then:
//!
//! `cargo test -p editor-render --release --test enter_above_image_timing -- --ignored --nocapture`
//!
//! The view is 900 pt wide at scale 2, at the two heights a 14 inch laptop's
//! visible frame gives (880 and 916 pt), over the generated fixture with
//! images on. The fixture's image (line 7) is on screen. Each iteration either
//! inserts an empty line under the prose line 3 (Enter at its end) or removes
//! it again (Backspace joining it back), so every paint changes the line
//! count above the image. Worker results are polled between keys, outside the
//! timer, as the shell's wakes would.
//!
//! The timer wraps `paint_lines` only. The p95 must stay under the 2 ms
//! keystroke draw budget.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Fake, cosmic};
use editor_core::buffer::Pos;
use editor_core::tokens::{Theme, Tokens};
use editor_render::view::DocView;

const W_PT: f32 = 900.0;
const HEIGHTS_PT: [f32; 2] = [880.0, 916.0];
const SCALE: f32 = 2.0;
const ITERS: usize = 200;
const BUDGET: Duration = Duration::from_millis(2);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fixture")
}

fn fixture_lines() -> Vec<String> {
    let path = fixture_dir().join("test.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} missing: run `cargo xtask fixture`", path.display()));
    text.lines().map(str::to_string).collect()
}

fn p95(times: &mut [Duration]) -> Duration {
    times.sort();
    let n = times.len();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let p95 = times[(n * 95 / 100).min(n - 1)];
    println!(
        "  n={n} p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms",
        ms(times[n / 2]),
        ms(p95),
        ms(times[n - 1])
    );
    p95
}

#[test]
#[ignore = "timing: run in release with --ignored --nocapture"]
fn enter_above_image_on_fixture() {
    let lines = fixture_lines();
    let image = lines
        .iter()
        .position(|l| l.starts_with("!["))
        .expect("fixture image line not found");
    let edit = lines
        .iter()
        .position(|l| l.starts_with("This paragraph"))
        .expect("fixture prose line not found");
    assert!(edit < image, "the edit is above the image");
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let t = Tokens::defaults(Theme::Light);

    let mut worst = Duration::ZERO;
    for h_pt in HEIGHTS_PT {
        let mut src = Fake::new(&refs, t.clone());
        let mut v = DocView::new(Box::new(cosmic(&t)), &t, W_PT, h_pt, SCALE);
        v.set_images(&fixture_dir(), Arc::new(|| {}));
        src.cursor = Pos {
            line: edit,
            col: lines[edit].len(),
        };
        v.paint_all(&src);
        let start = Instant::now();
        while !v.has_scaled_image(image) {
            assert!(start.elapsed() < Duration::from_secs(20), "image timed out");
            v.poll_async();
            std::thread::yield_now();
        }
        v.take_damage();

        let mut times = Vec::with_capacity(ITERS);
        for n in 0..ITERS {
            let range = if n % 2 == 0 {
                src.insert_line(edit + 1, "");
                src.cursor = Pos {
                    line: edit + 1,
                    col: 0,
                };
                edit..edit + 2
            } else {
                src.remove_line(edit + 1);
                src.cursor = Pos {
                    line: edit,
                    col: lines[edit].len(),
                };
                edit..edit + 1
            };
            let start = Instant::now();
            v.paint_lines(&src, range);
            times.push(start.elapsed());
            v.take_damage();
            v.poll_async();
            v.take_damage();
        }
        println!("Enter and join above the image, 900x{h_pt}pt @2x:");
        worst = worst.max(p95(&mut times));
    }
    assert!(worst < BUDGET, "p95 {worst:?} over the {BUDGET:?} budget");
}

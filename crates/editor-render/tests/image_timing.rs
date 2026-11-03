//! Measured check of the image draw cost on a keystroke (ignored by default,
//! timing is meaningful only in a release build):
//!
//! `cargo test -p editor-render --release --test image_timing -- --ignored --nocapture`
//!
//! A frame at scale 2 holds one decoded 1200x700 image. An edit above it
//! alternately wraps and unwraps a line, so every `paint_lines` changes a
//! height and goes through `repaint_to_bottom`, which redraws the image.
//! Prints the median and maximum time of that paint.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Fake, view};
use editor_core::tokens::{Theme, Tokens};

const FRAMES: usize = 60;

#[test]
#[ignore = "timing: run in release with --ignored --nocapture"]
fn image_redraw_on_height_changing_edit() {
    let dir = std::env::temp_dir().join(format!("img_timing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbaImage::from_pixel(1200, 700, image::Rgba([200, 30, 30, 255]))
        .save(dir.join("img1.png"))
        .unwrap();

    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(&["alpha", "![pic](img1.png)", "omega"], t.clone());
    let mut v = view(&t, 2.0);
    if std::env::var_os("IMG_TIMING_BASELINE").is_none() {
        v.set_images(&dir, Arc::new(|| {}));
    }
    v.paint_all(&src);
    let start = Instant::now();
    while std::env::var_os("IMG_TIMING_BASELINE").is_none() && !v.poll_async() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "decode timed out"
        );
        std::thread::yield_now();
    }
    v.take_damage();

    let long = "a long line of ordinary words that keeps going ".repeat(6);
    let mut times = Vec::new();
    for n in 0..FRAMES {
        src.set_line(0, if n % 2 == 0 { &long } else { "alpha" });
        let start = Instant::now();
        v.paint_lines(&src, 0..1);
        times.push(start.elapsed());
        v.take_damage();
    }
    let _ = std::fs::remove_dir_all(&dir);
    times.sort();
    println!(
        "image redraw paint_lines: median {:?}, max {:?} over {FRAMES} frames",
        times[FRAMES / 2],
        times[FRAMES - 1]
    );
}

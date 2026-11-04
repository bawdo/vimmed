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
//! keystroke draw budget, and after every paint the image line must still
//! hold its resampled copy (a blit, not a fast placeholder).
//!
//! `enter_above_images_with_editor` does the same through the real `Editor`
//! (which reports each edit's line shift), with more images on screen than
//! `SCALED_KEEP`: the fixture's image followed by narrow bands, each its own
//! file. Every image must keep its copy through every key, so the copies
//! have to move with their lines.
//!
//! `cargo test -p editor-render --release --test enter_above_image_timing -- --ignored --nocapture`

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Fake, cosmic, editor};
use editor_core::buffer::Pos;
use editor_core::effects::Redraw;
use editor_core::input::{Input, Key, KeyEvent, Mods};
use editor_core::tokens::{Theme, Tokens};
use editor_render::ViewSource;
use editor_render::images::SCALED_KEEP;
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
            // Where the image is once this key is applied.
            let shifted = if n % 2 == 0 { image + 1 } else { image };
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
            // Outside the timer, before any poll could hand a copy over: the
            // paint blitted the image, it did not fall back to the placeholder.
            assert!(
                v.has_scaled_image(shifted),
                "key {n}: the image on line {} lost its copy",
                shifted + 1
            );
            v.take_damage();
            v.poll_async();
            v.take_damage();
        }
        println!("Enter and join above the image, 900x{h_pt}pt @2x:");
        worst = worst.max(p95(&mut times));
    }
    assert!(worst < BUDGET, "p95 {worst:?} over the {BUDGET:?} budget");
}

/// Bands under the fixture's image: with it, more images than `SCALED_KEEP`.
const BANDS: usize = SCALED_KEEP + 1;

/// The image lines (the fixture's image, then the bands) when the image is
/// on line `image`.
fn image_lines(image: usize) -> std::ops::Range<usize> {
    image..image + BANDS + 1
}

fn key(k: Key) -> Input {
    Input::Key(KeyEvent {
        key: k,
        mods: Mods::default(),
        time_ns: 0,
    })
}

#[test]
#[ignore = "timing: run in release with --ignored --nocapture"]
fn enter_above_images_with_editor() {
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

    // The fixture's text with the bands under its image, in a scratch
    // directory beside a copy of the image.
    let dir = std::env::temp_dir().join(format!("enter_images_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(fixture_dir().join("img1.png"), dir.join("img1.png")).unwrap();
    let mut doc: Vec<String> = lines[..=image].to_vec();
    for k in 0..BANDS {
        let name = format!("band{k}.png");
        let band = image::RgbaImage::from_pixel(1200, 60, image::Rgba([30, 120, 60, 255]));
        band.save(dir.join(&name)).unwrap();
        doc.push(format!("![band {k}]({name})"));
    }
    doc.extend_from_slice(&lines[image + 1..]);
    let t = Tokens::defaults(Theme::Light);

    let mut worst = Duration::ZERO;
    let mut lost = 0;
    for h_pt in HEIGHTS_PT {
        let mut ed = editor(&dir, &doc.join("\n"));
        let mut v = DocView::new(Box::new(cosmic(&t)), &t, W_PT, h_pt, SCALE);
        v.set_images(&dir, Arc::new(|| {}));
        v.paint_all(&ed);
        let start = Instant::now();
        while !image_lines(image).all(|i| v.has_scaled_image(i)) {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "images timed out"
            );
            v.poll_async();
            std::thread::yield_now();
        }
        // To the end of the prose line, in Insert.
        for _ in 0..edit {
            ed.handle(key(Key::Char('j')));
        }
        ed.handle(key(Key::Char('A')));
        v.paint_all(&ed);
        v.take_damage();

        let mut times = Vec::with_capacity(ITERS);
        let mut lost_here = 0;
        for n in 0..ITERS {
            // Enter at the end of the line, then Backspace joining it back.
            let (k, at) = if n % 2 == 0 {
                (Key::Enter, image + 1)
            } else {
                (Key::Backspace, image)
            };
            let fx = ed.handle(key(k));
            assert!(
                !fx.scroll_to_cursor || !v.scroll_to_cursor(ed.cursor()),
                "key {n}: the view scrolled"
            );
            let start = Instant::now();
            match fx.redraw {
                Redraw::All => v.paint_all(&ed),
                Redraw::Lines(r) => v.paint_lines(&ed, r),
                Redraw::Status => v.paint_lines(&ed, 0..0),
                Redraw::None => {}
            }
            times.push(start.elapsed());
            assert_eq!(ed.line(at), lines[image], "key {n}: image on line {at}");
            // Outside the timer, before any poll could hand a copy over.
            lost_here += image_lines(at).filter(|&i| !v.has_scaled_image(i)).count();
            v.take_damage();
            v.poll_async();
            v.take_damage();
        }
        println!(
            "Enter and join above {} images, editor source, 900x{h_pt}pt @2x:",
            BANDS + 1
        );
        worst = worst.max(p95(&mut times));
        println!("  copies lost over {ITERS} keys: {lost_here}");
        lost += lost_here;
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(lost, 0, "images lost their copies");
    assert!(worst < BUDGET, "p95 {worst:?} over the {BUDGET:?} budget");
}

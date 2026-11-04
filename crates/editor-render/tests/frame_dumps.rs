//! Headless frame dumps, for looking at what the renderer draws without a
//! window (there is no in-app dump facility yet). Ignored by default; run with
//!
//! ```text
//! cargo test -p editor-render --test frame_dumps -- --ignored --nocapture
//! ```
//!
//! PNGs go to `$TMPDIR/frame-dumps/`:
//! - `hang-{off,on}-{none,heading,list,quote}.png`: the hanging-prefix
//!   evaluation (`docs/perf/2026-11-hanging-prefixes.md`), the same document
//!   with `size.hanging_prefix` off and on, the cursor on a paragraph, a
//!   heading, a list item and a quote in turn.
//! - `blocks.png` and `blocks-highlighted.png`: every block kind, before and
//!   after code highlighting lands.

mod common;

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Fake, view};
use editor_core::buffer::Pos;
use editor_core::tokens::{Theme, Tokens};
use editor_render::view::DocView;

fn save(v: &DocView, name: &str) -> PathBuf {
    let f = v.frame();
    save_crop(v, name, 0..f.width_px, 0..f.height_px)
}

/// Saves the pixels in columns `xs` and rows `ys` of the frame.
fn save_crop(v: &DocView, name: &str, xs: Range<u32>, ys: Range<u32>) -> PathBuf {
    let f = v.frame();
    let dir = std::env::temp_dir().join("frame-dumps");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.png"));
    // The page is opaque, so premultiplied and straight RGBA are the same.
    let full = image::RgbaImage::from_raw(f.width_px, f.height_px, f.pixmap.data().to_vec())
        .expect("frame size matches its data");
    let crop =
        image::imageops::crop_imm(&full, xs.start, ys.start, xs.len() as u32, ys.len() as u32)
            .to_image();
    // Opaque, so RGB; best compression keeps committed dumps small.
    let rgb = image::DynamicImage::ImageRgba8(crop).to_rgb8();
    let file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    let png = image::codecs::png::PngEncoder::new_with_quality(
        file,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    );
    rgb.write_with_encoder(png).unwrap();
    println!("{}", path.display());
    path
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

const HANG_DOC: &[&str] = &[
    "## Reading list",
    "Plain paragraph text sets the left edge that everything else is read against.",
    "- First item in a bulleted list",
    "- Second item, long enough to wrap onto a second row inside the column width here",
    "1. An ordered item",
    "> A quoted line, with the cursor elsewhere.",
    "### A smaller heading",
    "Closing paragraph.",
];

#[test]
#[ignore = "writes evaluation screenshots; run by hand"]
fn dump_hanging_prefix_frames() {
    let at = |line, col| Pos { line, col };
    for (hang, tag) in [("0", "off"), ("32", "on")] {
        let t = tokens(&[("size.hanging_prefix", hang)]);
        for (what, cursor) in [
            ("none", at(1, 0)),
            ("heading", at(0, 5)),
            ("list", at(2, 4)),
            ("quote", at(5, 4)),
        ] {
            let mut src = Fake::new(HANG_DOC, t.clone());
            src.cursor = cursor;
            let mut v = view(&t, 2.0);
            v.paint_all(&src);
            // The document and the margin left of it, not the empty page.
            save_crop(&v, &format!("hang-{tag}-{what}"), 60..1460, 60..500);
        }
    }
}

const BLOCKS_DOC: &[&str] = &[
    "# Blocks `inline` heading",
    "Body with **bold**, *italic*, ~~struck~~, `code` and a [link](http://x.test).",
    "> Quoted",
    ">> Nested quote",
    "- [ ] An open task",
    "- [x] A done task",
    "",
    "| Name | Count | Note |",
    "|:-----|------:|:----:|",
    "| apples | 3 | crisp |",
    "| a much longer name | 12000 | ok |",
    "",
    "```rust",
    "fn main() {",
    "    let x = 42; // answer",
    "}",
    "```",
    "",
    "---",
    "```",
    "unclosed block",
];

#[test]
#[ignore = "writes screenshots; run by hand"]
fn dump_blocks_frame() {
    let t = Tokens::defaults(Theme::Light);
    let mut src = Fake::new(BLOCKS_DOC, t.clone());
    src.cursor = Pos { line: 6, col: 0 };
    let mut v = view(&t, 2.0);
    v.set_highlighting(Arc::new(|| {}));
    v.paint_all(&src);
    save(&v, "blocks");
    let start = Instant::now();
    while !v.poll_async() {
        assert!(start.elapsed() < Duration::from_secs(20), "no highlight");
        std::thread::sleep(Duration::from_millis(1));
    }
    save(&v, "blocks-highlighted");
}

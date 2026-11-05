mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{Fake, H_PT, W_PT, view};
use editor_core::buffer::Pos;
use editor_core::host::Waker;
use editor_core::tokens::{Theme, TokenId, Tokens};
use editor_render::images::{ImageState, Images, SCALED_KEEP, Scaled};
use editor_render::paint::resamples_on_this_thread;

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A unique directory under the temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("img_test_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_png(dir: &Path, name: &str, w: u32, h: u32) {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
    img.save(dir.join(name)).unwrap();
}

fn waker() -> (Waker, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    (
        Arc::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        }),
        n,
    )
}

const TIMEOUT: Duration = Duration::from_secs(20);

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}");
        std::thread::yield_now();
    }
}

fn pixel(v: &editor_render::view::DocView, x: usize, y: usize) -> [u8; 4] {
    let fb = v.frame();
    let i = (y * fb.width_px as usize + x) * 4;
    fb.pixmap.data()[i..i + 4].try_into().unwrap()
}

#[test]
fn reserve_reads_header_without_decoding() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    write_png(dir.path(), "img1.png", 1200, 700);
    let (w, _) = waker();
    let mut images = Images::new(dir.path(), w);
    let slot = images.reserve(3, "img1.png");
    assert_eq!(slot.w_pt, t.px(TokenId::SpaceColumn));
    assert!(slot.h_pt <= t.px(TokenId::SizeImageMaxHeight));
    assert_eq!(slot.h_pt, 700.0_f32 * slot.w_pt / 1200.0);
    assert!(matches!(slot.state, ImageState::Pending), "not decoded yet");
    assert_eq!(images.pending(), 1);

    write_png(dir.path(), "tall.png", 100, 2000);
    let tall = images.reserve(4, "tall.png");
    assert_eq!(
        tall.h_pt,
        t.px(TokenId::SizeImageMaxHeight),
        "height clamped"
    );
    assert_eq!(tall.w_pt, 100.0 * tall.h_pt / 2000.0, "aspect kept");

    write_png(dir.path(), "small.png", 40, 30);
    let small = images.reserve(5, "small.png");
    assert_eq!((small.w_pt, small.h_pt), (40.0, 30.0), "never enlarged");
}

#[test]
fn decode_completes_without_relayout() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    write_png(dir.path(), "img1.png", 1200, 700);
    let (w, woken) = waker();
    let src = Fake::new(&["alpha", "![pic](img1.png)", "omega"], t.clone());
    let mut v = view(&t, 2.0);
    v.set_images(dir.path(), w);
    v.paint_all(&src);
    v.take_damage();

    let heights: Vec<f32> = (0..3).map(|i| v.line_height_px(i).unwrap()).collect();
    let col = t.px(TokenId::SpaceColumn);
    let want = (700.0 * col / 1200.0 * 2.0).ceil();
    assert_eq!(heights[1], want, "the image line reserves its final size");
    let page_top = (t.px(TokenId::SpacePageTop) * 2.0).round();
    let start = page_top + heights[0];
    let rows = start as u32..(start + heights[1]) as u32;

    let mut damage = Vec::new();
    until("the image to decode", || {
        if v.poll_async() {
            damage = v.take_damage().0;
            true
        } else {
            false
        }
    });
    assert!(woken.load(Ordering::SeqCst) >= 1, "the waker fired");
    assert_eq!(damage, vec![rows.clone()], "only the image rows");
    let after: Vec<f32> = (0..3).map(|i| v.line_height_px(i).unwrap()).collect();
    assert_eq!(heights, after, "no relayout");

    let px = pixel(
        &v,
        (W_PT * 2.0) as usize / 2,
        ((rows.start + rows.end) / 2) as usize,
    );
    assert!(
        px[0] > 150 && px[1] < 80,
        "decoded pixels painted, got {px:?}"
    );
}

#[test]
fn image_draw_changes_only_its_own_rect() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    write_png(dir.path(), "small.png", 40, 30);
    let (w, _) = waker();
    let src = Fake::new(&["alpha", "![pic](small.png)", "omega"], t.clone());
    let mut v = view(&t, 2.0);
    v.set_images(dir.path(), w);
    v.paint_all(&src);
    let before: Vec<u8> = v.frame().pixmap.data().to_vec();
    until("the image to decode", || v.poll_async());
    let after = v.frame().pixmap.data();
    let width = v.frame().width_px as usize;

    let col_w = (t.px(TokenId::SpaceColumn) * 2.0).round();
    let col_x = ((W_PT * 2.0 - col_w) / 2.0).floor() as usize;
    let page_top = (t.px(TokenId::SpacePageTop) * 2.0).round();
    let top = (page_top + v.line_height_px(0).unwrap()) as usize;
    let (bw, bh) = (80, 60);
    let changed: Vec<(usize, usize)> = (0..after.len() / 4)
        .filter(|&i| after[i * 4..i * 4 + 4] != before[i * 4..i * 4 + 4])
        .map(|i| (i % width, i / width))
        .collect();
    assert!(!changed.is_empty(), "the image was painted");
    for (x, y) in changed {
        assert!(
            (col_x..col_x + bw).contains(&x) && (top..top + bh).contains(&y),
            "pixel ({x},{y}) changed outside the image rect"
        );
    }
    let inside = (top + bh / 2) * width + col_x + bw / 2;
    assert!(after[inside * 4] > 150, "the rect's interior is the image");
}

#[test]
fn translucent_image_blends_with_the_page() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    image::RgbaImage::from_pixel(40, 30, image::Rgba([200, 30, 30, 128]))
        .save(dir.path().join("glass.png"))
        .unwrap();
    let (w, _) = waker();
    let src = Fake::new(&["alpha", "![pic](glass.png)", "omega"], t.clone());
    let mut v = view(&t, 2.0);
    v.set_images(dir.path(), w);
    v.paint_all(&src);
    until("the image to decode", || v.poll_async());
    let col_x = ((W_PT * 2.0 - (t.px(TokenId::SpaceColumn) * 2.0).round()) / 2.0).floor() as usize;
    let page_top = (t.px(TokenId::SpacePageTop) * 2.0).round();
    let top = (page_top + v.line_height_px(0).unwrap()) as usize;
    let px = pixel(&v, col_x + 40, top + 30);
    let bg = t.color(TokenId::ColorBg);
    assert!(px[1] > 30 && px[1] < bg.g - 40, "blended, got {px:?}");
    assert!(
        px[0] < bg.r && px[3] == 255,
        "tinted red over an opaque page"
    );
}

#[test]
fn scrolled_off_images_release_their_pixmaps_and_keep_heights() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    write_png(dir.path(), "img1.png", 1200, 700);
    let (w, _) = waker();
    let mut lines: Vec<String> = vec!["alpha".into(), "![pic](img1.png)".into()];
    lines.extend((0..200).map(|i| format!("line {i}")));
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let src = Fake::new(&refs, t.clone());
    let mut v = view(&t, 2.0);
    v.set_images(dir.path(), w);
    v.paint_all(&src);
    until("the image to decode", || v.poll_async());
    assert!(v.has_scaled_image(1), "on screen and decoded");
    let height = v.line_height_px(1).unwrap();

    assert!(v.scroll_to_cursor(Pos { line: 190, col: 0 }));
    v.paint_all(&src);
    assert!(!v.has_scaled_image(1), "off screen holds no resampled copy");
    assert_eq!(v.line_height_px(1), Some(height), "measured height kept");

    assert!(v.scroll_to_cursor(Pos { line: 0, col: 0 }));
    v.paint_all(&src);
    assert!(
        v.has_scaled_image(1),
        "back on screen it has its copy again"
    );
    let top = (t.px(TokenId::SpacePageTop) * 2.0).round() + v.line_height_px(0).unwrap();
    let px = pixel(&v, W_PT as usize, (top + height / 2.0) as usize);
    assert!(px[0] > 150 && px[1] < 80, "painted again, got {px:?}");
}

#[test]
fn remote_url_never_fetches() {
    let dir = TempDir::new();
    let (w, woken) = waker();
    let mut images = Images::new(dir.path(), w);
    let slot = images.reserve(0, "https://example.com/a.png");
    assert!(matches!(&slot.state, ImageState::Remote(u) if u == "https://example.com/a.png"));
    assert_eq!(images.pending(), 0, "no worker job queued");
    assert!(images.poll().is_empty());
    assert_eq!(woken.load(Ordering::SeqCst), 0);
}

#[test]
fn missing_shows_placeholder() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    let (w, _) = waker();
    let mut images = Images::new(dir.path(), w);
    assert!(matches!(
        images.reserve(0, "nope.png").state,
        ImageState::Missing
    ));
    assert_eq!(images.pending(), 0);

    let mut src = Fake::new(
        &["![](nope.png)", "![alt text](nope.png)", "cursor here"],
        t.clone(),
    );
    src.cursor.line = 2;
    let mut v = view(&t, 1.0);
    let (w, _) = waker();
    v.set_images(dir.path(), w);
    v.paint_all(&src);
    let ph = t.color(TokenId::ColorImagePlaceholder);
    let y = t.px(TokenId::SpacePageTop) as usize + 1;
    // Far right of the column, clear of any label text.
    let x = (W_PT / 2.0 + t.px(TokenId::SpaceColumn) / 2.0) as usize - 2;
    let second = y + v.line_height_px(0).unwrap() as usize;
    for row in [y, second] {
        let px = pixel(&v, x, row);
        assert_eq!(
            px[..3],
            [ph.r, ph.g, ph.b],
            "placeholder colour at row {row}"
        );
    }
    // The label text sits on the placeholder: some pixel in the first line differs from it.
    let h0 = v.line_height_px(0).unwrap() as usize;
    let left = (W_PT / 2.0 - t.px(TokenId::SpaceColumn) / 2.0) as usize;
    let inked = (left..left + 120)
        .any(|xx| (y - 1..y - 1 + h0).any(|yy| pixel(&v, xx, yy)[..3] != [ph.r, ph.g, ph.b]));
    assert!(inked, "the 'image not found' label is drawn");
}

/// An `Images` with `img1.png` (1200x700) reserved and decoded.
fn decoded(dir: &TempDir) -> Images {
    write_png(dir.path(), "img1.png", 1200, 700);
    let (w, _) = waker();
    let mut images = Images::new(dir.path(), w);
    images.reserve(0, "img1.png");
    until("the image to decode", || {
        images.poll();
        matches!(
            images.slot_of("img1.png").map(|s| s.state),
            Some(ImageState::Ready(_))
        )
    });
    images
}

/// Polls until `images` hands over `src` at `w` by `h` pixels.
fn claim(images: &mut Images, src: &str, w: u32, h: u32) -> Scaled {
    let mut got = None;
    until("the resample to land", || {
        images.poll();
        got = images.scaled(src, w, h);
        got.is_some()
    });
    got.unwrap()
}

#[test]
fn pending_resample_keeps_the_alt_label() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    write_png(dir.path(), "img1.png", 1200, 700);
    let (w, _) = waker();
    let src = Fake::new(&["alpha", "![a picture](img1.png)", "omega"], t.clone());
    let mut v = view(&t, 2.0);
    v.set_images(dir.path(), w);
    v.paint_all(&src);
    until("the image to land", || v.poll_async());

    // Decoded, but the new box has no copy yet: paint as a pending decode
    // does, the placeholder with the alt text on it.
    v.resize(W_PT, H_PT, 1.0);
    v.paint_all(&src);
    assert!(!v.has_scaled_image(1));
    let ph = t.color(TokenId::ColorImagePlaceholder);
    let top = t.px(TokenId::SpacePageTop).round() as usize + v.line_height_px(0).unwrap() as usize;
    let left = (W_PT / 2.0 - t.px(TokenId::SpaceColumn) / 2.0) as usize;
    let label_rows = top..top + t.px(TokenId::FontSize) as usize * 2;
    let inked = |v: &editor_render::view::DocView, colour: [u8; 3]| {
        (left..left + 120).any(|x| label_rows.clone().any(|y| pixel(v, x, y)[..3] != colour))
    };
    assert!(
        inked(&v, [ph.r, ph.g, ph.b]),
        "the alt label is drawn on the placeholder"
    );

    until("the new box to land", || v.poll_async());
    v.paint_all(&src);
    assert!(v.has_scaled_image(1));
    let img = pixel(&v, left + 2, top + 2);
    assert!(img[0] > 150 && img[1] < 80, "the image, got {img:?}");
    assert!(
        !inked(&v, [img[0], img[1], img[2]]),
        "no label over the landed image"
    );
}

#[test]
fn image_resample_not_on_ui_thread() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    write_png(dir.path(), "img1.png", 1200, 700);
    let (w, _) = waker();
    let src = Fake::new(&["alpha", "![pic](img1.png)", "omega"], t.clone());
    let resampled = resamples_on_this_thread();
    let mut v = view(&t, 2.0);
    v.set_images(dir.path(), w);
    v.paint_all(&src);
    until("the image to land", || v.poll_async());
    assert!(v.has_scaled_image(1), "the scaled copy arrived");

    // A new scale is a new box: the relayout must not resample.
    v.resize(W_PT, H_PT, 1.0);
    v.paint_all(&src);
    v.take_damage();
    assert!(
        !v.has_scaled_image(1),
        "no scaled copy straight after relayout"
    );
    let height = v.line_height_px(1).unwrap();
    let col = t.px(TokenId::SpaceColumn);
    assert_eq!(height, (700.0 * col / 1200.0).ceil(), "final box height");
    let top = t.px(TokenId::SpacePageTop).round() + v.line_height_px(0).unwrap();
    let mid = (top + height / 2.0) as usize;
    let ph = t.color(TokenId::ColorImagePlaceholder);
    assert_eq!(
        pixel(&v, W_PT as usize / 2, mid)[..3],
        [ph.r, ph.g, ph.b],
        "the placeholder is painted while the copy is pending"
    );

    let mut damage = Vec::new();
    until("the new box to land", || {
        if v.poll_async() {
            damage = v.take_damage().0;
            true
        } else {
            false
        }
    });
    assert!(v.has_scaled_image(1), "the scaled copy exists once polled");
    assert_eq!(v.line_height_px(1), Some(height), "the height never moved");
    assert_eq!(
        damage,
        vec![top as u32..(top + height) as u32],
        "the image line repainted"
    );
    let px = pixel(&v, W_PT as usize / 2, mid);
    assert!(
        px[0] > 150 && px[1] < 80,
        "decoded pixels painted, got {px:?}"
    );
    assert_eq!(
        resamples_on_this_thread(),
        resampled,
        "nothing was resampled on this thread"
    );
}

#[test]
fn stale_resample_result_dropped() {
    let dir = TempDir::new();
    let mut images = decoded(&dir);
    assert!(
        images.scaled("img1.png", 600, 350).is_none(),
        "box A asked for"
    );
    assert!(
        images.scaled("img1.png", 300, 175).is_none(),
        "box B asked for"
    );
    let b = claim(&mut images, "img1.png", 300, 175);
    assert_eq!((b.pixmap.width(), b.pixmap.height()), (300, 175));
    // A landed no later than B. Nobody claimed it, so the next poll drops it.
    images.poll();
    assert!(
        !images.holds_scaled("img1.png", 600, 350),
        "box A discarded"
    );
    assert!(images.holds_scaled("img1.png", 300, 175), "box B kept");
    assert_eq!(images.resamples_queued(), 0, "nothing left in flight");
}

#[test]
fn duplicate_resample_requests_coalesce() {
    let dir = TempDir::new();
    let mut images = decoded(&dir);
    assert!(images.scaled("img1.png", 300, 175).is_none());
    assert!(images.scaled("img1.png", 300, 175).is_none());
    assert_eq!(images.resamples_queued(), 1, "one job for one key");
    claim(&mut images, "img1.png", 300, 175);
    assert_eq!(images.resamples_queued(), 0);
}

#[test]
fn scaled_cache_is_bounded() {
    let dir = TempDir::new();
    let mut images = decoded(&dir);
    let sizes: Vec<(u32, u32)> = (0..SCALED_KEEP as u32 + 2)
        .map(|k| (100 + k * 10, 60 + k * 6))
        .collect();
    for &(w, h) in &sizes {
        images.scaled("img1.png", w, h);
        claim(&mut images, "img1.png", w, h);
        assert!(images.scaled_held() <= SCALED_KEEP, "never over the bound");
    }
    assert_eq!(images.scaled_held(), SCALED_KEEP);
    let (w0, h0) = sizes[0];
    assert!(!images.holds_scaled("img1.png", w0, h0), "the oldest went");
    let (wn, hn) = *sizes.last().unwrap();
    assert!(images.holds_scaled("img1.png", wn, hn), "the newest stays");
}

#[test]
fn scroll_back_does_not_resample_on_ui_thread() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    write_png(dir.path(), "img1.png", 1200, 700);
    let (w, _) = waker();
    let mut lines: Vec<String> = vec!["alpha".into(), "![pic](img1.png)".into()];
    lines.extend((0..200).map(|i| format!("line {i}")));
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let src = Fake::new(&refs, t.clone());
    let resampled = resamples_on_this_thread();
    let mut v = view(&t, 2.0);
    v.set_images(dir.path(), w);
    v.paint_all(&src);
    until("the image to land", || v.poll_async());
    let height = v.line_height_px(1).unwrap();

    assert!(v.scroll_to_cursor(Pos { line: 190, col: 0 }));
    v.paint_all(&src);
    assert!(!v.has_scaled_image(1), "off screen holds no scaled copy");

    assert!(v.scroll_to_cursor(Pos { line: 0, col: 0 }));
    v.paint_all(&src);
    assert_eq!(
        resamples_on_this_thread(),
        resampled,
        "the paint path did not resample"
    );
    until("the image to show again", || {
        v.poll_async();
        v.has_scaled_image(1)
    });
    let top = (t.px(TokenId::SpacePageTop) * 2.0).round() + v.line_height_px(0).unwrap();
    let px = pixel(&v, W_PT as usize, (top + height / 2.0) as usize);
    assert!(px[0] > 150 && px[1] < 80, "painted again, got {px:?}");
    assert_eq!(
        resamples_on_this_thread(),
        resampled,
        "nothing was resampled on this thread"
    );
}

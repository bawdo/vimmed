mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{Fake, W_PT, view};
use editor_core::host::Waker;
use editor_core::tokens::{Theme, TokenId, Tokens};
use editor_render::images::{ImageState, Images};

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

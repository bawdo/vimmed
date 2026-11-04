//! Lazy font fallback: a codepoint no loaded face covers is looked up off
//! the paint path, the face is loaded, and the lines that showed `.notdef`
//! are laid out again.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{Fake, Geom, H_PT, W_PT, font_paths};
use editor_core::buffer::Pos;
use editor_core::host::Waker;
use editor_core::tokens::{Theme, Tokens};
use editor_render::fonts::{FontResolver, FontSet};
use editor_render::layout_cosmic::CosmicLayout;
use editor_render::view::DocView;

const TIMEOUT: Duration = Duration::from_secs(20);

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A unique directory under the temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("fallback_test_{}_{n}", std::process::id()));
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

fn symbols() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/NotoSansSymbols2-Regular.ttf")
}

/// The committed regular faces, no fallback set, and a lookup that finds
/// Noto Sans Symbols 2 for the ballot boxes (and counts its calls).
struct Symbols {
    lookups: Arc<AtomicUsize>,
    found: Option<PathBuf>,
}

impl FontResolver for Symbols {
    fn sans(&self) -> PathBuf {
        font_paths().sans
    }
    fn mono(&self) -> PathBuf {
        font_paths().mono
    }
    fn family(&self, _name: &str) -> Option<PathBuf> {
        None
    }
    fn fallback_set(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn lookup(&self, c: char) -> Option<PathBuf> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        matches!(c, '\u{2610}' | '\u{2611}')
            .then(|| self.found.clone())
            .flatten()
    }
}

fn counting_waker() -> (Waker, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    let w: Waker = Arc::new(move || {
        n2.fetch_add(1, Ordering::SeqCst);
    });
    (w, n)
}

/// A view over the committed fonts with lazy fallback through `r`.
fn view_with(r: Symbols, cache: Option<PathBuf>, waker: Waker, t: &Tokens) -> DocView {
    let fonts = FontSet::with_lookup(Box::new(r), cache, waker);
    let layout = CosmicLayout::new(fonts, t);
    DocView::new(Box::new(layout), t, W_PT, H_PT, 2.0)
}

/// Polls until fonts land, or fails on timeout.
fn wait_for_fonts(doc: &mut DocView, src: &Fake) {
    let start = Instant::now();
    while !doc.poll_fonts(src) {
        assert!(
            start.elapsed() < TIMEOUT,
            "timed out after {TIMEOUT:?} waiting for a fallback face"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Line 1 is a task (its marker is a ballot box) and line 3 has one in its
/// text; the plain line between keeps their damage apart.
fn source(t: &Tokens) -> Fake {
    let mut src = Fake::new(&["plain", "- [ ] task", "plain", "a \u{2610} b"], t.clone());
    src.cursor = Pos { line: 0, col: 0 };
    src
}

fn header_only(dir: &Path) -> PathBuf {
    let p = dir.join("font-paths");
    std::fs::write(&p, "font-paths v1 test\n").unwrap();
    p
}

#[test]
fn missing_glyph_resolves_after_poll() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    let cache = header_only(dir.path());
    let (waker, wakes) = counting_waker();
    let lookups = Arc::new(AtomicUsize::new(0));
    let r = Symbols {
        lookups: lookups.clone(),
        found: Some(symbols()),
    };
    let mut doc = view_with(r, Some(cache.clone()), waker, &t);
    let src = source(&t);
    doc.paint_all(&src);
    let _ = doc.take_damage();

    // Before: the ballot boxes are `.notdef` in the regular faces.
    assert!(!doc.has_missing_glyphs(0));
    assert!(doc.has_missing_glyphs(1), "task marker shows .notdef");
    assert!(!doc.has_missing_glyphs(2));
    assert!(doc.has_missing_glyphs(3), "inline ballot box shows .notdef");
    // Nothing is looked up until the shell says the first frame is out.
    assert!(!doc.poll_fonts(&src));
    assert_eq!(lookups.load(Ordering::SeqCst), 0);

    doc.start_font_fallback();
    let h1 = doc.line_height_px(1);
    wait_for_fonts(&mut doc, &src);

    // After: covered, and only the two affected lines were laid out again
    // and damaged, as separate ranges.
    assert!(
        wakes.load(Ordering::SeqCst) >= 1,
        "the waker was never called"
    );
    assert!(!doc.has_missing_glyphs(1));
    assert!(!doc.has_missing_glyphs(3));
    assert_eq!(doc.line_height_px(1), h1);
    let d = doc.take_damage();
    let g = Geom::new(&t, 2.0);
    assert_eq!(
        d.0,
        vec![g.line_rows(1), g.line_rows(3)],
        "lines 1 and 3, apart"
    );
    // One lookup for the one codepoint, written to the cache file.
    assert_eq!(lookups.load(Ordering::SeqCst), 1);
    let text = std::fs::read_to_string(&cache).unwrap();
    assert!(
        text.contains(&format!("glyph\t2610\t{}\n", symbols().display())),
        "{text}"
    );
}

#[test]
fn a_cached_codepoint_skips_the_lookup() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    let cache = header_only(dir.path());
    let line = format!("glyph\t2610\t{}\n", symbols().display());
    std::fs::write(&cache, format!("font-paths v1 test\n{line}")).unwrap();
    let (waker, _) = counting_waker();
    let lookups = Arc::new(AtomicUsize::new(0));
    let r = Symbols {
        lookups: lookups.clone(),
        found: None,
    };
    let mut doc = view_with(r, Some(cache.clone()), waker, &t);
    let src = source(&t);
    doc.paint_all(&src);
    doc.start_font_fallback();
    wait_for_fonts(&mut doc, &src);
    assert!(!doc.has_missing_glyphs(3));
    assert_eq!(lookups.load(Ordering::SeqCst), 0, "served from the cache");
    // Not written twice.
    let text = std::fs::read_to_string(&cache).unwrap();
    assert_eq!(text.matches("glyph\t").count(), 1, "{text}");
}

#[test]
fn a_codepoint_nothing_covers_is_looked_up_once() {
    let t = Tokens::defaults(Theme::Light);
    let (waker, _) = counting_waker();
    let lookups = Arc::new(AtomicUsize::new(0));
    let r = Symbols {
        lookups: lookups.clone(),
        found: None,
    };
    let mut doc = view_with(r, None, waker, &t);
    let mut src = source(&t);
    doc.paint_all(&src);
    doc.start_font_fallback();
    let start = Instant::now();
    while lookups.load(Ordering::SeqCst) == 0 {
        assert!(start.elapsed() < TIMEOUT, "never looked up");
        std::thread::sleep(Duration::from_millis(1));
    }
    // Typing on the line lays it out again; the codepoint is not asked again.
    src.set_line(3, "a \u{2610} bc");
    doc.paint_lines(&src, 3..4);
    std::thread::sleep(Duration::from_millis(20));
    assert!(!doc.poll_fonts(&src));
    assert!(doc.has_missing_glyphs(3));
    assert_eq!(lookups.load(Ordering::SeqCst), 1);
}

#[test]
fn a_table_cell_resolves_after_poll() {
    let t = Tokens::defaults(Theme::Light);
    let (waker, _) = counting_waker();
    let r = Symbols {
        lookups: Arc::new(AtomicUsize::new(0)),
        found: Some(symbols()),
    };
    let mut doc = view_with(r, None, waker, &t);
    let lines = ["plain", "", "| a | b |", "|---|---|", "| \u{2611} | c |"];
    let mut src = Fake::new(&lines, t.clone());
    src.cursor = Pos { line: 0, col: 0 };
    doc.paint_all(&src);
    let _ = doc.take_damage();
    assert!(doc.has_missing_glyphs(4), "the cell shows .notdef");
    assert!(!doc.has_missing_glyphs(0));
    doc.start_font_fallback();
    wait_for_fonts(&mut doc, &src);
    assert!(!doc.has_missing_glyphs(4));
    let d = doc.take_damage();
    let g = Geom::new(&t, 2.0);
    assert!(!d.is_empty());
    assert!(d.0.iter().all(|r| r.start >= g.line_rows(2).start), "{d:?}");
}

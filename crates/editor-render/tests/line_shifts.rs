//! Line slots move with their lines: an edit that adds or removes lines
//! keeps every other line's layouts and resampled image with that line,
//! driven by the real `Editor` (which reports each edit's line shift). A
//! source that reports no shifts falls back to resizing at the end.

mod common;

use std::cell::RefCell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{Fake, H_PT, Recording, W_PT, cosmic, editor, press};
use editor_core::buffer::{Buffer, LineShift, Pos, ShiftMark};
use editor_core::editor::Editor;
use editor_core::input::Key;
use editor_core::markdown::LineDisplay;
use editor_core::tokens::{Theme, Tokens};
use editor_core::view_model::{CaretShape, LineShifts};
use editor_render::images::SCALED_KEEP;
use editor_render::layout::TextLayout;
use editor_render::view::DocView;
use editor_render::{Selection, StatusModel, ViewSource};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A unique directory under the temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("shift_test_{}_{n}", std::process::id()));
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

const IMAGES: usize = SCALED_KEEP + 2;

/// Two prose lines, then `IMAGES` small images, each its own file (so each
/// needs its own resampled copy), one per line from line 2.
fn image_doc(dir: &Path) -> String {
    let mut text = String::from("Intro line one\nIntro line two\n");
    for k in 0..IMAGES {
        let name = format!("img{k}.png");
        let img = image::RgbaImage::from_pixel(600, 40, image::Rgba([40, 90, 200, 255]));
        img.save(dir.join(&name)).unwrap();
        text.push_str(&format!("![pic {k}]({name})\n"));
    }
    text
}

/// The image lines when the first image is on line `first`.
fn image_lines(first: usize) -> Range<usize> {
    first..first + IMAGES
}

/// Polls the workers until every image line holds its resampled copy.
fn settle(v: &mut DocView, lines: Range<usize>) {
    let start = Instant::now();
    while !lines.clone().all(|i| v.has_scaled_image(i)) {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "images timed out"
        );
        v.poll_async();
        v.take_damage();
        std::thread::yield_now();
    }
}

/// An editor over the image document, painted with every image landed.
fn images_on_screen(dir: &TempDir, t: &Tokens) -> (Editor, DocView) {
    let ed = editor(dir.path(), &image_doc(dir.path()));
    let mut v = DocView::new(Box::new(cosmic(t)), t, W_PT, H_PT, 2.0);
    v.set_images(dir.path(), Arc::new(|| {}));
    v.paint_all(&ed);
    settle(&mut v, image_lines(2));
    (ed, v)
}

/// Every image line in `lines` kept its copy through the last paint (checked
/// before any poll could hand one over) and no resample was asked for.
fn assert_kept(v: &DocView, lines: Range<usize>, what: &str) {
    for i in lines {
        assert!(v.has_scaled_image(i), "{what}: line {i} lost its copy");
    }
    assert_eq!(v.resamples_queued(), 0, "{what}: a resample was asked for");
}

#[test]
fn enter_above_image_keeps_its_scaled_copy() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    let (mut ed, mut v) = images_on_screen(&dir, &t);
    press(&mut ed, &mut v, Key::Char('A'));
    press(&mut ed, &mut v, Key::Enter);
    assert_eq!(ed.line_count(), IMAGES + 3);
    assert_kept(&v, image_lines(3), "after Enter");
}

#[test]
fn join_above_image_keeps_its_scaled_copy() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    let (mut ed, mut v) = images_on_screen(&dir, &t);
    press(&mut ed, &mut v, Key::Char('j'));
    press(&mut ed, &mut v, Key::Char('i'));
    press(&mut ed, &mut v, Key::Backspace);
    assert_eq!(ed.line(0), "Intro line oneIntro line two");
    assert_kept(&v, image_lines(1), "after a join");
}

#[test]
fn undo_of_enter_splices_back() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    let (mut ed, mut v) = images_on_screen(&dir, &t);
    press(&mut ed, &mut v, Key::Char('A'));
    press(&mut ed, &mut v, Key::Enter);
    press(&mut ed, &mut v, Key::Esc);
    settle(&mut v, image_lines(3));
    press(&mut ed, &mut v, Key::Char('u'));
    assert_eq!(ed.line_count(), IMAGES + 2, "the Enter is undone");
    assert_kept(&v, image_lines(2), "after undo");
}

/// Unique prose lines, many more than fit on screen.
fn long_doc() -> Vec<String> {
    (0..200)
        .map(|i| format!("Line {i:03} of a long document"))
        .collect()
}

/// A view whose layout records the text of every line it lays out.
fn recording_view(t: &Tokens) -> (DocView, Rc<RefCell<Vec<common::Laid>>>) {
    let laid = Rc::new(RefCell::new(Vec::new()));
    let layout: Box<dyn TextLayout> = Box::new(Recording {
        inner: cosmic(t),
        laid: laid.clone(),
    });
    (DocView::new(layout, t, W_PT, H_PT, 2.0), laid)
}

#[test]
fn enter_relays_out_only_the_edited_lines() {
    let t = Tokens::defaults(Theme::Light);
    let dir = TempDir::new();
    let lines = long_doc();
    let mut ed = editor(dir.path(), &lines.join("\n"));
    let (mut v, laid) = recording_view(&t);
    v.paint_all(&ed);
    press(&mut ed, &mut v, Key::Char('j'));
    press(&mut ed, &mut v, Key::Char('j'));
    press(&mut ed, &mut v, Key::Char('A'));
    laid.borrow_mut().clear();

    press(&mut ed, &mut v, Key::Enter);
    assert_eq!(ed.line_count(), lines.len() + 1);
    let below = &lines[3..];
    let relaid: Vec<String> = laid
        .borrow()
        .iter()
        .filter(|l| below.contains(&l.text))
        .map(|l| l.text.clone())
        .collect();
    assert!(
        relaid.is_empty(),
        "lines below the edit were laid out again: {relaid:?}"
    );
    assert!(v.is_shaped(4), "the line below the edit is still shaped");
}

#[test]
fn unknown_shifts_fall_back_to_resize() {
    let t = Tokens::defaults(Theme::Light);
    let lines = long_doc();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut src = Fake::new(&refs[..30], t.clone());
    assert!(src.line_shifts_since(None).is_none(), "a Fake reports none");
    let (mut v, laid) = recording_view(&t);
    v.paint_all(&src);
    laid.borrow_mut().clear();

    src.insert_line(1, "");
    v.paint_lines(&src, 1..2);
    // As before line shifts: the slots stay by index, so the line pushed
    // down from 1 to 2 is laid out again in slot 2.
    assert!(
        laid.borrow().iter().any(|l| l.text == lines[1]),
        "slots stayed by index"
    );
    // And the frame is what a fresh view paints.
    let (mut fresh, _) = recording_view(&t);
    fresh.paint_all(&src);
    assert!(
        v.frame().pixmap.data() == fresh.frame().pixmap.data(),
        "the fallback frame matches a fresh paint"
    );
}

/// A `Fake` that reports `lie` as the shifts since any mark it handed out.
struct Lying {
    fake: Fake,
    /// Only for its marks.
    marks: Buffer,
    lie: Option<Vec<LineShift>>,
}

impl ViewSource for Lying {
    fn line_count(&self) -> usize {
        self.fake.line_count()
    }
    fn line(&self, i: usize) -> &str {
        self.fake.line(i)
    }
    fn display(&self, i: usize) -> LineDisplay {
        self.fake.display(i)
    }
    fn cursor(&self) -> Pos {
        self.fake.cursor()
    }
    fn status(&self) -> StatusModel {
        self.fake.status()
    }
    fn tokens(&self) -> &Tokens {
        self.fake.tokens()
    }
    fn theme(&self) -> Theme {
        self.fake.theme()
    }
    fn reader(&self) -> bool {
        self.fake.reader()
    }
    fn selection(&self) -> Option<Selection> {
        self.fake.selection()
    }
    fn matches(&self, i: usize) -> &[Range<usize>] {
        self.fake.matches(i)
    }
    fn current_match(&self) -> Option<(usize, Range<usize>)> {
        self.fake.current_match()
    }
    fn caret_shape(&self) -> CaretShape {
        self.fake.caret_shape()
    }
    fn line_shifts_since(&self, since: Option<ShiftMark>) -> Option<LineShifts> {
        Some(LineShifts {
            mark: self.marks.shift_mark(),
            shifts: since.and(self.lie.clone()),
        })
    }
}

#[test]
fn shifts_that_do_not_fit_fall_back_to_resize() {
    let t = Tokens::defaults(Theme::Light);
    let lines = long_doc();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let lies = [
        // Past the end of the slots.
        vec![LineShift {
            at: 100,
            removed: 5,
            inserted: 1,
        }],
        // In range, but ending at the wrong line count.
        vec![LineShift {
            at: 0,
            removed: 1,
            inserted: 3,
        }],
    ];
    for lie in lies {
        let mut src = Lying {
            fake: Fake::new(&refs[..30], t.clone()),
            marks: Buffer::from_text(""),
            lie: None,
        };
        let (mut v, _) = recording_view(&t);
        v.paint_all(&src);
        src.fake.insert_line(1, "");
        src.lie = Some(lie.clone());
        v.paint_lines(&src, 1..2);
        assert!(v.line_height_px(30).is_some(), "{lie:?}: 31 slots");
        assert_eq!(v.line_height_px(31), None, "{lie:?}: 31 slots");
        let (mut fresh, _) = recording_view(&t);
        fresh.paint_all(&src.fake);
        assert!(
            v.frame().pixmap.data() == fresh.frame().pixmap.data(),
            "{lie:?}: the fallback frame matches a fresh paint"
        );
    }
}

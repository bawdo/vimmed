//! Measured check of the shape-run cache trim cadence (ignored by default,
//! timing is meaningful only in a release build):
//!
//! `cargo test -p editor-render --release --test trim_cadence -- --ignored --nocapture`
//!
//! The frame sequence is a typical keystroke run: a full paint, two partial
//! paints of the edited line, then a full paint. Three variants of that last
//! full paint are timed:
//!
//! - `reuse`: the shipped path. Undamaged lines keep their layouts, so the
//!   full paint lays out nothing and the cache trim never matters to them.
//! - `relayout, trim(1) every frame`: every line is laid out again (a window
//!   narrower than the column forces it), after the shipped cadence trimmed
//!   once per frame. Runs of lines untouched by the partial paints were last
//!   used two frames back and have aged out.
//! - `relayout, no trim on partials`: the same, but the partial frames did
//!   not trim, so those runs are still cached. This is the most any other
//!   trim policy could save on that relayout.
//!
//! The relayout variants paint a narrower frame, so compare their whole-paint
//! times with each other, not with `reuse`; the `layout_line` part is the
//! figure the trim policy moves.

mod common;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{Fake, H_PT, W_PT, cosmic};
use editor_core::tokens::{Theme, Tokens};
use editor_render::layout::{GlyphMask, LaidLine, StyledSpan, TextLayout};
use editor_render::layout_cosmic::CosmicLayout;
use editor_render::view::DocView;

const PROSE: &str = "Most mornings the river fog settled over the valley before sunrise, and the \
farmers waited for it to lift before starting their tractors. By eight o'clock the paddocks were \
usually clear, although the shaded gullies kept their mist until well after lunch.";

/// Delegates to cosmic-text; `end_frame` trims only while `trim` is set.
struct Gated {
    inner: CosmicLayout,
    trim: Rc<Cell<bool>>,
    /// Time spent in `layout_line`.
    spent: Rc<Cell<Duration>>,
}

impl TextLayout for Gated {
    fn layout_line(&mut self, text: &str, spans: &[StyledSpan], width_px: f32) -> LaidLine {
        let start = Instant::now();
        let laid = self.inner.layout_line(text, spans, width_px);
        self.spent.set(self.spent.get() + start.elapsed());
        laid
    }
    fn draw(&mut self, line: &LaidLine, sink: &mut dyn FnMut(GlyphMask)) {
        self.inner.draw(line, sink)
    }
    fn end_frame(&mut self) {
        if self.trim.get() {
            self.inner.end_frame()
        }
    }
}

fn doc() -> Vec<String> {
    let words: Vec<&str> = PROSE.split(' ').collect();
    (0..60)
        .map(|i| {
            let n = 6 + i % 14;
            (0..n)
                .map(|k| words[(i * 7 + k) % words.len()])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Reuse,
    RelayoutTrimEveryFrame,
    RelayoutNoTrimOnPartials,
}

/// Times the final full paint of one keystroke sequence in a fresh view:
/// (whole paint, layout part of it).
fn once(t: &Tokens, lines: &[String], v: Variant) -> (Duration, Duration) {
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let mut src = Fake::new(&refs, t.clone());
    let trim = Rc::new(Cell::new(true));
    let spent = Rc::new(Cell::new(Duration::ZERO));
    let layout = Gated {
        inner: cosmic(t),
        trim: trim.clone(),
        spent: spent.clone(),
    };
    let mut view = DocView::new(Box::new(layout), t, W_PT, H_PT, 2.0);
    view.paint_all(&src);
    // Warm the frame before the timed sequence, as a running editor would be.
    view.paint_all(&src);
    if v == Variant::RelayoutNoTrimOnPartials {
        trim.set(false);
    }
    let edited = format!("{} x", lines[3]);
    src.set_line(3, &edited);
    view.paint_lines(&src, 3..4);
    src.set_line(3, &format!("{edited}y"));
    view.paint_lines(&src, 3..4);
    trim.set(true);
    if v != Variant::Reuse {
        view.resize(W_PT - 200.0, H_PT, 2.0);
    }
    spent.set(Duration::ZERO);
    let start = Instant::now();
    view.paint_all(&src);
    (start.elapsed(), spent.get())
}

fn p50(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len().div_ceil(2) - 1]
}

#[test]
#[ignore = "timing; run in release with --ignored --nocapture"]
fn trim_cadence_micro_check() {
    let t = Tokens::defaults(Theme::Light);
    let lines = doc();
    let rounds = 30;
    let mut res = [Vec::new(), Vec::new(), Vec::new()];
    for _ in 0..rounds {
        for (k, v) in [
            Variant::Reuse,
            Variant::RelayoutTrimEveryFrame,
            Variant::RelayoutNoTrimOnPartials,
        ]
        .into_iter()
        .enumerate()
        {
            res[k].push(once(&t, &lines, v));
        }
    }
    println!(
        "final full paint, p50 of {rounds}, 800x600 pt at scale 2 (whole paint / layout_line part):"
    );
    let names = [
        "reuse (shipped)",
        "relayout, trim(1) every frame",
        "relayout, no trim on partials",
    ];
    for (name, r) in names.iter().zip(res) {
        let whole = p50(r.iter().map(|x| x.0).collect());
        let lay = p50(r.iter().map(|x| x.1).collect());
        println!("  {name:32} {whole:>10.1?} / {lay:>10.1?}");
    }
}

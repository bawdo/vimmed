//! Head-to-head text layout benchmark: cosmic-text against parley.
//!
//! Usage: `cargo run -p layout-bench --release -- target/fixture/test.md`
//!
//! Both libraries get the same font files (the platform sans and mono faces, loaded by path),
//! the same text, the same styled spans, a 660 px column at scale 2 and one layout object per
//! source line, which is how the editor's renderer lays text out. The results feed the decision
//! recorded in `docs/perf/2026-11-layout-bench.md`.
//!
//! Sections:
//! 1. Font system construction in a cold process (10 child processes per library, interleaved).
//!    The child times only its own construction, never process start.
//! 2. Shaping the first visible screen (the first 60 source lines): cold in each child process,
//!    and warm in this process (warm-up excluded).
//! 3. Per-keystroke relayout: one character typed into the first `**` paragraph 500 times.
//! 4. Fallback quality: `.notdef` glyphs for a fixed probe string, with the two faces only and
//!    again with the small fixed fallback set.
//!
//! Percentiles are nearest rank: sort ascending, take the value at rank `ceil(p / 100 * n)`.
//!
//! Diagnostic: `layout-bench --compare <fixture.md> [sans font path]` prints every screen line
//! and every keystroke where the two libraries disagree on visual rows or height, with the row
//! widths, so the "same work" claim can be checked rather than assumed.

use std::borrow::Cow;
use std::hint::black_box;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

// Bench-only geometry. The editor's real values live in its design tokens; these mirror the
// token defaults (16 pt body, 1.5 body line height, 1.4 code line height, 660 pt column).
const SCALE: f32 = 2.0;
const COLUMN_PT: f32 = 660.0;
const BODY_PT: f32 = 16.0;
const LH_BODY: f32 = 1.5;
const LH_CODE: f32 = 1.4;
const BODY_PX: f32 = BODY_PT * SCALE;
const WIDTH_PX: f32 = COLUMN_PT * SCALE;
// Heading size multipliers, h1..h6 (bench-only stand-ins for the heading tokens).
const HEADING_SCALE: [f32; 6] = [2.0, 1.5, 1.25, 1.1, 1.0, 1.0];

const SCREEN_LINES: usize = 60;
const COLD_PROCESSES: usize = 10;
const WARM_SCREEN_WARMUP: usize = 20;
const WARM_SCREEN_ITERS: usize = 200;
const KEY_WARMUP: usize = 50;
const KEY_ITERS: usize = 500;
const ROUNDS: usize = 5;
const SCREEN_SLOT_BASE: usize = 0;
const KEY_SLOT: usize = SCREEN_LINES;
const FALLBACK_PROBE: &str = "☐ ☑ ◦ → 😀 中文 ü é";
const TYPED_PROSE: &[u8] = b"the quick brown fox jumps over the lazy dog ";

// ---------------------------------------------------------------------------------------------
// Fonts

#[derive(Clone, Debug)]
struct Faces {
    sans: PathBuf,
    mono: PathBuf,
    fallback: Vec<PathBuf>,
}

#[cfg(target_os = "macos")]
fn resolve_faces() -> Faces {
    Faces {
        sans: PathBuf::from("/System/Library/Fonts/SFNS.ttf"),
        mono: PathBuf::from("/System/Library/Fonts/SFNSMono.ttf"),
        fallback: vec![
            PathBuf::from("/System/Library/Fonts/Apple Symbols.ttf"),
            PathBuf::from("/System/Library/Fonts/Apple Color Emoji.ttc"),
            PathBuf::from("/System/Library/Fonts/Menlo.ttc"),
        ],
    }
}

#[cfg(not(target_os = "macos"))]
fn resolve_faces() -> Faces {
    let sans = fc_match("sans-serif");
    let mono = fc_match("monospace");
    let mut fallback = Vec::new();
    for pattern in ["emoji", "symbol"] {
        let p = fc_match(pattern);
        if p != sans && p != mono && !fallback.contains(&p) {
            fallback.push(p);
        }
    }
    Faces {
        sans,
        mono,
        fallback,
    }
}

#[cfg(not(target_os = "macos"))]
fn fc_match(pattern: &str) -> PathBuf {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .unwrap_or_else(|e| panic!("fc-match {pattern}: {e}"));
    assert!(out.status.success(), "fc-match {pattern} failed");
    let path = String::from_utf8(out.stdout).expect("fc-match printed non UTF-8");
    PathBuf::from(path.trim())
}

// ---------------------------------------------------------------------------------------------
// Styled spans (computed outside every timed region, identical for both libraries)

#[derive(Clone, Debug)]
struct Span {
    range: Range<usize>,
    mono: bool,
    bold: bool,
    size: f32,
    line_height: f32,
}

/// Marks which source lines sit inside (or are) a fenced code block.
fn code_flags(lines: &[&str]) -> Vec<bool> {
    let mut inside = false;
    lines
        .iter()
        .map(|l| {
            if l.trim_start().starts_with("```") {
                inside = !inside;
                true
            } else {
                inside
            }
        })
        .collect()
}

/// A tiny bench-only classifier: fenced code is mono, `#` headings are bold and larger,
/// `**` toggles bold and backticks toggle mono. Markers stay in the text (raw display).
fn spans_for(line: &str, in_code: bool) -> Vec<Span> {
    if line.is_empty() {
        return Vec::new();
    }
    if in_code {
        return vec![Span {
            range: 0..line.len(),
            mono: true,
            bold: false,
            size: BODY_PX,
            line_height: LH_CODE,
        }];
    }
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    let heading = (1..=6).contains(&hashes) && line[hashes..].starts_with(' ');
    let size = if heading {
        BODY_PX * HEADING_SCALE[hashes - 1]
    } else {
        BODY_PX
    };

    // Per-byte style, then merged into runs. Every toggle is an ASCII byte, so run edges
    // always land on char boundaries.
    let bytes = line.as_bytes();
    let mut style = vec![(false, heading); bytes.len()];
    let (mut bold, mut code) = (heading, false);
    let mut i = 0;
    while i < bytes.len() {
        if !code && bytes[i..].starts_with(b"**") {
            bold = !bold;
            style[i] = (false, true);
            style[i + 1] = (false, true);
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            code = !code;
            style[i] = (true, bold);
            i += 1;
            continue;
        }
        style[i] = (code, bold);
        i += 1;
    }

    let mut spans: Vec<Span> = Vec::new();
    for (i, &(mono, bold)) in style.iter().enumerate() {
        match spans.last_mut() {
            Some(s) if s.mono == mono && s.bold == bold => s.range.end = i + 1,
            _ => spans.push(Span {
                range: i..i + 1,
                mono,
                bold,
                size,
                line_height: LH_BODY,
            }),
        }
    }
    spans
}

// ---------------------------------------------------------------------------------------------
// The two engines behind one small interface

trait Engine: Sized {
    const NAME: &'static str;
    /// Builds the font system with exactly the given files loaded by path.
    fn construct(faces: &Faces, with_fallback: bool) -> Self;
    /// Lays out one source line into layout object `slot`. Returns (visual rows, height px).
    fn layout(&mut self, slot: usize, text: &str, spans: &[Span]) -> (u32, f32);
    /// Lays out `text` plainly and returns the text of every `.notdef` glyph (glyph id 0).
    fn notdef(&mut self, text: &str) -> Vec<String>;
    /// Diagnostic: (text range, width px) of every visual row in a slot's last layout.
    fn rows(&self, slot: usize) -> Vec<(Range<usize>, f32)>;
}

struct Cosmic {
    fs: cosmic_text::FontSystem,
    sans: String,
    mono: String,
    slots: Vec<cosmic_text::Buffer>,
}

impl Cosmic {
    fn attrs<'a>(s: &Span, sans: &'a str, mono: &'a str) -> cosmic_text::Attrs<'a> {
        use cosmic_text::{Attrs, Family, Metrics, Weight};
        let family = if s.mono { mono } else { sans };
        let mut a = Attrs::new().family(Family::Name(family));
        if s.bold {
            a = a.weight(Weight::BOLD);
        }
        if s.size != BODY_PX || s.line_height != LH_BODY {
            a = a.metrics(Metrics::new(s.size, s.size * s.line_height));
        }
        a
    }
}

impl Engine for Cosmic {
    const NAME: &'static str = "cosmic-text";

    fn construct(faces: &Faces, with_fallback: bool) -> Self {
        use cosmic_text::fontdb::{Database, Source};
        let mut db = Database::new();
        let first_family = |db: &mut Database, path: &Path| -> String {
            let ids = db.load_font_source(Source::File(path.to_path_buf()));
            let id = *ids
                .first()
                .unwrap_or_else(|| panic!("no face in {}", path.display()));
            db.face(id).expect("face just loaded").families[0].0.clone()
        };
        let sans = first_family(&mut db, &faces.sans);
        let mono = first_family(&mut db, &faces.mono);
        if with_fallback {
            for p in &faces.fallback {
                db.load_font_source(Source::File(p.clone()));
            }
        }
        db.set_sans_serif_family(sans.clone());
        db.set_monospace_family(mono.clone());
        let fs = cosmic_text::FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        Self {
            fs,
            sans,
            mono,
            slots: Vec::new(),
        }
    }

    fn layout(&mut self, slot: usize, text: &str, spans: &[Span]) -> (u32, f32) {
        use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping};
        let Self {
            fs,
            sans,
            mono,
            slots,
        } = self;
        while slots.len() <= slot {
            let mut b = Buffer::new_empty(Metrics::new(BODY_PX, BODY_PX * LH_BODY));
            b.set_size(Some(WIDTH_PX), None);
            // Break at words, and inside a word only when it can't fit (parley: Anywhere).
            b.set_wrap(cosmic_text::Wrap::WordOrGlyph);
            slots.push(b);
        }
        let buf = &mut slots[slot];
        let default = Attrs::new().family(Family::Name(sans));
        buf.set_rich_text(
            spans
                .iter()
                .map(|s| (&text[s.range.clone()], Self::attrs(s, sans, mono))),
            &default,
            Shaping::Advanced,
            None,
        );
        buf.shape_until_scroll(fs, false);
        let (mut rows, mut height) = (0u32, 0f32);
        for run in buf.layout_runs() {
            rows += 1;
            height += run.line_height;
        }
        (rows, height)
    }

    fn notdef(&mut self, text: &str) -> Vec<String> {
        use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping};
        let mut buf = Buffer::new_empty(Metrics::new(BODY_PX, BODY_PX * LH_BODY));
        buf.set_size(Some(WIDTH_PX), None);
        let attrs = Attrs::new().family(Family::Name(&self.sans));
        buf.set_text(text, &attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut self.fs, false);
        let mut missing = Vec::new();
        for run in buf.layout_runs() {
            for g in run.glyphs {
                if g.glyph_id == 0 {
                    missing.push(run.text[g.start..g.end].to_string());
                }
            }
        }
        missing
    }

    fn rows(&self, slot: usize) -> Vec<(Range<usize>, f32)> {
        self.slots[slot]
            .layout_runs()
            .map(|r| {
                let a = r.glyphs.first().map_or(0, |g| g.start);
                let b = r.glyphs.last().map_or(0, |g| g.end);
                (a..b, r.line_w)
            })
            .collect()
    }
}

type Brush = [u8; 4];

struct Parley {
    fcx: parley::FontContext,
    lcx: parley::LayoutContext<Brush>,
    body: Vec<parley::FontFamilyName<'static>>,
    mono: Vec<parley::FontFamilyName<'static>>,
    slots: Vec<parley::Layout<Brush>>,
}

/// Memory-maps a font file, the same way fontdb's `Source::File` does for cosmic-text.
fn map_blob(path: &Path) -> parley::fontique::Blob<u8> {
    let file = std::fs::File::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    // SAFETY: system font files are not modified while the bench runs; fontdb maps the same
    // files the same way for cosmic-text.
    let map = unsafe { memmap2::Mmap::map(&file) }
        .unwrap_or_else(|e| panic!("mmap {}: {e}", path.display()));
    parley::fontique::Blob::new(Arc::new(map))
}

impl Parley {
    fn register(fcx: &mut parley::FontContext, path: &Path) -> Vec<String> {
        let ids: Vec<_> = fcx
            .collection
            .register_fonts(map_blob(path), None)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let mut names: Vec<String> = Vec::new();
        for id in ids {
            let name = fcx
                .collection
                .family_name(id)
                .expect("registered family has a name")
                .to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    fn stack(names: &[String]) -> Vec<parley::FontFamilyName<'static>> {
        names
            .iter()
            .map(|n| parley::FontFamilyName::Named(Cow::Owned(n.clone())))
            .collect()
    }

    fn build(
        fcx: &mut parley::FontContext,
        lcx: &mut parley::LayoutContext<Brush>,
        body: &[parley::FontFamilyName<'static>],
        mono: &[parley::FontFamilyName<'static>],
        layout: &mut parley::Layout<Brush>,
        text: &str,
        spans: &[Span],
    ) {
        use parley::{FontFamily, FontWeight, LineHeight, StyleProperty};
        let mut b = lcx.ranged_builder(fcx, text, 1.0, true);
        b.push_default(StyleProperty::FontFamily(FontFamily::List(Cow::Borrowed(
            body,
        ))));
        b.push_default(StyleProperty::FontSize(BODY_PX));
        // Matches cosmic-text's Wrap::WordOrGlyph.
        b.push_default(StyleProperty::OverflowWrap(parley::OverflowWrap::Anywhere));
        // Line height is per source line here (all spans of a line share it). parley's line
        // box includes the root style's strut, as CSS does, so the line's own height goes on
        // the root; a per-span value alone would leave code lines at the body height.
        let line_height = spans.first().map_or(LH_BODY, |s| s.line_height);
        b.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(
            line_height,
        )));
        for s in spans {
            if s.mono {
                b.push(
                    StyleProperty::FontFamily(FontFamily::List(Cow::Borrowed(mono))),
                    s.range.clone(),
                );
            }
            if s.bold {
                b.push(StyleProperty::FontWeight(FontWeight::BOLD), s.range.clone());
            }
            if s.size != BODY_PX {
                b.push(StyleProperty::FontSize(s.size), s.range.clone());
            }
        }
        b.build_into(layout, text);
        layout.break_all_lines(Some(WIDTH_PX));
        layout.align(
            parley::Alignment::Start,
            parley::AlignmentOptions::default(),
        );
    }
}

impl Engine for Parley {
    const NAME: &'static str = "parley";

    fn construct(faces: &Faces, with_fallback: bool) -> Self {
        use parley::fontique::{Collection, CollectionOptions};
        let collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        let mut fcx = parley::FontContext {
            collection,
            source_cache: Default::default(),
        };
        let mut body = Self::register(&mut fcx, &faces.sans);
        let mut mono = Self::register(&mut fcx, &faces.mono);
        if with_fallback {
            // parley has no implicit "any loaded face" fallback, so the fallback set is
            // appended to each family stack, in the same order cosmic-text is given it.
            for p in &faces.fallback {
                let names = Self::register(&mut fcx, p);
                body.extend(names.iter().cloned());
                mono.extend(names);
            }
        }
        Self {
            fcx,
            lcx: parley::LayoutContext::new(),
            body: Self::stack(&body),
            mono: Self::stack(&mono),
            slots: Vec::new(),
        }
    }

    fn layout(&mut self, slot: usize, text: &str, spans: &[Span]) -> (u32, f32) {
        let Self {
            fcx,
            lcx,
            body,
            mono,
            slots,
        } = self;
        while slots.len() <= slot {
            slots.push(parley::Layout::new());
        }
        let layout = &mut slots[slot];
        Self::build(fcx, lcx, body, mono, layout, text, spans);
        (layout.len() as u32, layout.height())
    }

    fn notdef(&mut self, text: &str) -> Vec<String> {
        let mut layout = parley::Layout::new();
        Self::build(
            &mut self.fcx,
            &mut self.lcx,
            &self.body,
            &self.mono,
            &mut layout,
            text,
            &[],
        );
        let mut missing = Vec::new();
        for line in layout.lines() {
            for run in line.runs() {
                for cluster in run.clusters() {
                    for g in cluster.glyphs() {
                        if g.id == 0 {
                            missing.push(text[cluster.text_range()].to_string());
                        }
                    }
                }
            }
        }
        missing
    }

    fn rows(&self, slot: usize) -> Vec<(Range<usize>, f32)> {
        self.slots[slot]
            .lines()
            .map(|l| (l.text_range(), l.metrics().advance))
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Statistics

/// Nearest-rank percentile of an ascending slice.
fn pct(sorted: &[u64], p: f64) -> u64 {
    let n = sorted.len();
    let rank = ((p / 100.0) * n as f64).ceil() as usize;
    sorted[rank.clamp(1, n) - 1]
}

fn ms(ns: u64) -> String {
    format!("{:.2} ms", ns as f64 / 1e6)
}

fn us(ns: u64) -> String {
    format!("{:.1} µs", ns as f64 / 1e3)
}

#[derive(Default)]
struct Dist(Vec<u64>);

impl Dist {
    fn sorted(&self) -> Vec<u64> {
        let mut v = self.0.clone();
        v.sort_unstable();
        v
    }
}

// ---------------------------------------------------------------------------------------------
// Workload

struct Doc {
    lines: Vec<String>,
    spans: Vec<Vec<Span>>,
}

fn load_doc(path: &Path) -> Doc {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let lines: Vec<&str> = text.lines().collect();
    let code = code_flags(&lines);
    let spans = lines
        .iter()
        .zip(&code)
        .map(|(l, &c)| spans_for(l, c))
        .collect();
    Doc {
        lines: lines.iter().map(|l| l.to_string()).collect(),
        spans,
    }
}

fn shape_screen<E: Engine>(engine: &mut E, doc: &Doc) -> (u32, f32) {
    let (mut rows, mut height) = (0, 0.0);
    for i in 0..SCREEN_LINES.min(doc.lines.len()) {
        let (r, h) = engine.layout(SCREEN_SLOT_BASE + i, &doc.lines[i], &doc.spans[i]);
        rows += r;
        height += h;
    }
    (rows, height)
}

/// The keystroke workload: typing at the end of the first bold run of the first `**` line.
struct Typing {
    line: String,
    cursor: usize,
    typed: usize,
}

impl Typing {
    fn new(doc: &Doc) -> (Self, usize) {
        let idx = doc
            .lines
            .iter()
            .position(|l| l.contains("**"))
            .expect("fixture has a ** paragraph");
        let line = doc.lines[idx].clone();
        let open = line.find("**").expect("opening **");
        let close = open + 2 + line[open + 2..].find("**").expect("closing **");
        (
            Self {
                line,
                cursor: close + 2,
                typed: 0,
            },
            idx,
        )
    }

    /// Inserts one character and returns the new spans (outside the timed region).
    fn step(&mut self) -> Vec<Span> {
        // Prose with spaces, so both libraries break at words as they would for real typing.
        let c = char::from(TYPED_PROSE[self.typed % TYPED_PROSE.len()]);
        self.line.insert(self.cursor, c);
        self.cursor += 1;
        self.typed += 1;
        spans_for(&self.line, false)
    }
}

// ---------------------------------------------------------------------------------------------
// Child mode: one cold process measures its own construction and first screen

fn child<E: Engine>(doc_path: &Path, faces: &Faces) {
    let doc = load_doc(doc_path);
    let t0 = Instant::now();
    let mut engine = E::construct(faces, false);
    let t1 = Instant::now();
    let (rows, height) = shape_screen(&mut engine, &doc);
    let t2 = Instant::now();
    black_box((rows, height));
    println!(
        "{} {} {} {}",
        (t1 - t0).as_nanos(),
        (t2 - t1).as_nanos(),
        rows,
        height
    );
}

struct ColdSample {
    construct_ns: u64,
    screen_ns: u64,
    rows: u32,
}

fn run_child(lib: &str, doc_path: &Path, faces: &Faces) -> ColdSample {
    let exe = std::env::current_exe().expect("current exe");
    let out = Command::new(exe)
        .arg("--child")
        .arg(lib)
        .arg(doc_path)
        .arg(&faces.sans)
        .arg(&faces.mono)
        .output()
        .expect("spawn child");
    assert!(
        out.status.success(),
        "child {lib} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8(out.stdout).expect("child output is UTF-8");
    let f: Vec<&str> = s.split_whitespace().collect();
    ColdSample {
        construct_ns: f[0].parse().expect("construct ns"),
        screen_ns: f[1].parse().expect("screen ns"),
        rows: f[2].parse().expect("rows"),
    }
}

// ---------------------------------------------------------------------------------------------
// Parent mode

struct Results {
    construct: Dist,
    cold_screen: Dist,
    cold_rows: u32,
    warm_screen: Dist,
    screen_rows: u32,
    screen_height: f32,
    keys: Dist,
    key_rows_end: u32,
    notdef_two: Vec<String>,
    notdef_fallback: Vec<String>,
}

impl Results {
    fn new() -> Self {
        Self {
            construct: Dist::default(),
            cold_screen: Dist::default(),
            cold_rows: 0,
            warm_screen: Dist::default(),
            screen_rows: 0,
            screen_height: 0.0,
            keys: Dist::default(),
            key_rows_end: 0,
            notdef_two: Vec::new(),
            notdef_fallback: Vec::new(),
        }
    }
}

struct Lane<E: Engine> {
    engine: E,
    typing: Typing,
    res: Results,
}

impl<E: Engine> Lane<E> {
    fn new(faces: &Faces, doc: &Doc) -> Self {
        let mut engine = E::construct(faces, false);
        // Warm-up, excluded: the first screen, then typing on a throwaway copy of the line.
        for _ in 0..WARM_SCREEN_WARMUP {
            black_box(shape_screen(&mut engine, doc));
        }
        let (mut warm, _) = Typing::new(doc);
        for _ in 0..KEY_WARMUP {
            let spans = warm.step();
            black_box(engine.layout(KEY_SLOT, &warm.line, &spans));
        }
        let (typing, _) = Typing::new(doc);
        Self {
            engine,
            typing,
            res: Results::new(),
        }
    }

    fn screens(&mut self, doc: &Doc, n: usize) {
        for _ in 0..n {
            let t = Instant::now();
            let (rows, height) = shape_screen(&mut self.engine, doc);
            self.res.warm_screen.0.push(t.elapsed().as_nanos() as u64);
            self.res.screen_rows = rows;
            self.res.screen_height = height;
        }
    }

    fn keys(&mut self, n: usize) {
        for _ in 0..n {
            let spans = self.typing.step();
            let t = Instant::now();
            let (rows, _) = self.engine.layout(KEY_SLOT, &self.typing.line, &spans);
            self.res.keys.0.push(t.elapsed().as_nanos() as u64);
            self.res.key_rows_end = black_box(rows);
        }
    }
}

fn fallback_counts<E: Engine>(faces: &Faces, res: &mut Results) {
    res.notdef_two = E::construct(faces, false).notdef(FALLBACK_PROBE);
    res.notdef_fallback = E::construct(faces, true).notdef(FALLBACK_PROBE);
}

fn row(label: &str, c: String, p: String) {
    println!("| {label} | {c} | {p} |");
}

fn report(c: &Results, p: &Results, faces: &Faces, typed_line: usize) {
    let cc = c.construct.sorted();
    let pc = p.construct.sorted();
    let cs = c.cold_screen.sorted();
    let ps = p.cold_screen.sorted();
    let cw = c.warm_screen.sorted();
    let pw = p.warm_screen.sorted();
    let ck = c.keys.sorted();
    let pk = p.keys.sorted();

    println!("## Layout benchmark: {} vs {}", Cosmic::NAME, Parley::NAME);
    println!();
    println!(
        "Platform: {} {}. Column {COLUMN_PT} pt at scale {SCALE} ({WIDTH_PX} px), body {BODY_PX} px.",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("Sans: {}", faces.sans.display());
    println!("Mono: {}", faces.mono.display());
    for f in &faces.fallback {
        println!("Fallback: {}", f.display());
    }
    println!();
    println!("| Measure | {} | {} |", Cosmic::NAME, Parley::NAME);
    println!("|---|---|---|");
    let trio = |v: &[u64]| {
        format!(
            "{} (min {}, max {})",
            ms(pct(v, 50.0)),
            ms(v[0]),
            ms(v[v.len() - 1])
        )
    };
    row(
        &format!("Construction, cold process, p50 of {COLD_PROCESSES}"),
        trio(&cc),
        trio(&pc),
    );
    row(
        &format!("First screen ({SCREEN_LINES} lines), cold, p50 of {COLD_PROCESSES}"),
        trio(&cs),
        trio(&ps),
    );
    let three = |v: &[u64], f: fn(u64) -> String| {
        format!(
            "{} / {} / {}",
            f(pct(v, 50.0)),
            f(pct(v, 95.0)),
            f(pct(v, 99.0))
        )
    };
    row(
        &format!("First screen, warm, p50 / p95 / p99 of {WARM_SCREEN_ITERS}"),
        three(&cw, ms),
        three(&pw, ms),
    );
    row(
        &format!("Keystroke relayout, p50 / p95 / p99 of {KEY_ITERS}"),
        three(&ck, us),
        three(&pk, us),
    );
    row(
        "Keystroke relayout, max",
        us(ck[ck.len() - 1]),
        us(pk[pk.len() - 1]),
    );
    row(
        "First screen visual rows (cold / warm), total height px",
        format!(
            "{} / {}, {:.0}",
            c.cold_rows, c.screen_rows, c.screen_height
        ),
        format!(
            "{} / {}, {:.0}",
            p.cold_rows, p.screen_rows, p.screen_height
        ),
    );
    row(
        "Typed line visual rows after the last keystroke",
        c.key_rows_end.to_string(),
        p.key_rows_end.to_string(),
    );
    let nd = |v: &[String]| {
        if v.is_empty() {
            "0".to_string()
        } else {
            format!("{} ({})", v.len(), v.join(" "))
        }
    };
    row(
        "`.notdef` glyphs, two faces only",
        nd(&c.notdef_two),
        nd(&p.notdef_two),
    );
    row(
        "`.notdef` glyphs, with fallback set",
        nd(&c.notdef_fallback),
        nd(&p.notdef_fallback),
    );
    println!();
    println!("Fallback probe: `{FALLBACK_PROBE}`. Typed line: source line {typed_line}.");
    println!();

    let c95 = pct(&ck, 95.0) as f64;
    let p95 = pct(&pk, 95.0) as f64;
    let faster = p95 <= 0.75 * c95;
    let not_worse = pct(&pc, 50.0) <= pct(&cc, 50.0);
    println!(
        "Rule on this platform: parley p95 is {:.1}% lower ({}), cold construction p50 {} ({}).",
        (1.0 - p95 / c95) * 100.0,
        if faster { "meets 25%" } else { "short of 25%" },
        if not_worse { "not worse" } else { "worse" },
        if faster && not_worse {
            "parley wins here"
        } else {
            "stay on cosmic-text here"
        }
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--child") {
        let faces = Faces {
            sans: PathBuf::from(&args[4]),
            mono: PathBuf::from(&args[5]),
            fallback: Vec::new(),
        };
        let doc = Path::new(&args[3]);
        match args[2].as_str() {
            "cosmic" => child::<Cosmic>(doc, &faces),
            "parley" => child::<Parley>(doc, &faces),
            other => panic!("unknown library {other}"),
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--compare") {
        // Diagnostic: per-line rows and height where the two libraries disagree.
        let mut faces = resolve_faces();
        if let Some(s) = args.get(3) {
            faces.sans = PathBuf::from(s);
        }
        let doc = load_doc(Path::new(&args[2]));
        let mut c = Cosmic::construct(&faces, false);
        let mut p = Parley::construct(&faces, false);
        for i in 0..SCREEN_LINES.min(doc.lines.len()) {
            let a = c.layout(i, &doc.lines[i], &doc.spans[i]);
            let b = p.layout(i, &doc.lines[i], &doc.spans[i]);
            if a != b {
                println!(
                    "line {}: cosmic {a:?} parley {b:?}: {}",
                    i + 1,
                    doc.lines[i]
                );
            }
        }
        let (mut typing, _) = Typing::new(&doc);
        for k in 1..=KEY_ITERS {
            let spans = typing.step();
            let a = c.layout(KEY_SLOT, &typing.line, &spans);
            let b = p.layout(KEY_SLOT, &typing.line, &spans);
            if a != b {
                println!("keystroke {k}: cosmic {a:?} parley {b:?}");
                println!("  cosmic rows {:?}", c.rows(KEY_SLOT));
                println!("  parley rows {:?}", p.rows(KEY_SLOT));
            }
        }
        return;
    }
    let Some(doc_path) = args.get(1).map(PathBuf::from) else {
        eprintln!("usage: layout-bench <fixture.md>");
        std::process::exit(2);
    };
    let faces = resolve_faces();
    let doc = load_doc(&doc_path);

    // 1 and 2 (cold): interleaved child processes.
    let mut c_res = Results::new();
    let mut p_res = Results::new();
    for _ in 0..COLD_PROCESSES {
        for (lib, res) in [("cosmic", &mut c_res), ("parley", &mut p_res)] {
            let s = run_child(lib, &doc_path, &faces);
            res.construct.0.push(s.construct_ns);
            res.cold_screen.0.push(s.screen_ns);
            res.cold_rows = s.rows;
        }
    }

    // 2 (warm) and 3: both lanes warmed, then measured in alternating rounds.
    let mut cosmic: Lane<Cosmic> = Lane::new(&faces, &doc);
    let mut parley: Lane<Parley> = Lane::new(&faces, &doc);
    cosmic.res = c_res;
    parley.res = p_res;
    for _ in 0..ROUNDS {
        cosmic.screens(&doc, WARM_SCREEN_ITERS / ROUNDS);
        parley.screens(&doc, WARM_SCREEN_ITERS / ROUNDS);
    }
    for _ in 0..ROUNDS {
        cosmic.keys(KEY_ITERS / ROUNDS);
        parley.keys(KEY_ITERS / ROUNDS);
    }

    // 4: fallback quality.
    fallback_counts::<Cosmic>(&faces, &mut cosmic.res);
    fallback_counts::<Parley>(&faces, &mut parley.res);

    let (_, typed_line) = Typing::new(&doc);
    report(&cosmic.res, &parley.res, &faces, typed_line + 1);
}

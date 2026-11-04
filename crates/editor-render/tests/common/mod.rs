//! Shared fixtures: a fake `ViewSource` over real markdown display, and a
//! renderer built from the committed test fonts.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use editor_core::buffer::{Buffer, Pos};
use editor_core::editor::{Config, Editor, OpenRequest};
use editor_core::effects::Redraw;
use editor_core::host::{Host, Waker, WindowRequest};
use editor_core::input::{Input, Key, KeyEvent, Mods};
use editor_core::markdown::{
    Blocks, LineDisplay, LineKind, Prefix, Reveal, SpanBase, code_bg, line_display,
    ordered_display, parse_line,
};
use editor_core::platform::Platform;
use editor_core::tokens::{Theme, TokenId, Tokens};
use editor_core::view_model::{CaretShape, StatusLeft};
use editor_render::fonts::{FontPaths, FontSet};
use editor_render::layout::{GlyphMask, LaidLine, RowGeom, StyledSpan, TextLayout};
use editor_render::layout_cosmic::CosmicLayout;
use editor_render::view::DocView;
use editor_render::{Selection, StatusModel, ViewSource};

pub const W_PT: f32 = 800.0;
pub const H_PT: f32 = 600.0;

pub fn font_paths() -> FontPaths {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts");
    FontPaths {
        sans: dir.join("NotoSans-Regular.ttf"),
        mono: dir.join("NotoSansMono-Regular.ttf"),
        fallback: Vec::new(),
    }
}

pub fn cosmic(tokens: &Tokens) -> CosmicLayout {
    CosmicLayout::new(FontSet::load(&font_paths()), tokens)
}

pub fn view(tokens: &Tokens, scale: f32) -> DocView {
    DocView::new(Box::new(cosmic(tokens)), tokens, W_PT, H_PT, scale)
}

/// A `ViewSource` over plain lines, displayed through the real markdown model
/// the way the editor does it: code backgrounds from `code_bg`, a code line's
/// language from its block, ordered numbers from `ordered_display`, and a
/// cursor anywhere in a code block or table revealing all of it.
pub struct Fake {
    pub lines: Vec<String>,
    buf: Buffer,
    blocks: Blocks,
    pub tokens: Tokens,
    pub cursor: Pos,
    pub dirty: bool,
    pub reader: bool,
    pub selection: Option<Selection>,
    pub matches: Vec<Vec<Range<usize>>>,
    /// The current match: its line and source byte range.
    pub current: Option<(usize, Range<usize>)>,
    /// The file name the status bar shows.
    pub file: String,
    /// What the status bar shows on its left.
    pub left: StatusLeft,
    pub warnings: Vec<&'static str>,
    pub showcmd: String,
    pub caret: CaretShape,
    /// An input method's composition at the cursor and its cursor.
    pub preedit: Option<(String, usize)>,
}

impl Fake {
    pub fn new(lines: &[&str], tokens: Tokens) -> Fake {
        let lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        let buf = Buffer::from_text(&lines.join("\n"));
        let blocks = Blocks::new(&buf);
        let matches = vec![Vec::new(); lines.len()];
        Fake {
            lines,
            buf,
            blocks,
            tokens,
            cursor: Pos::default(),
            dirty: false,
            reader: false,
            selection: None,
            matches,
            current: None,
            file: "notes.md".to_string(),
            left: StatusLeft::Mode("INSERT"),
            warnings: Vec::new(),
            showcmd: String::new(),
            caret: CaretShape::Bar,
            preedit: None,
        }
    }

    pub fn set_line(&mut self, i: usize, text: &str) {
        self.lines[i] = text.to_string();
        self.rebuild();
    }

    /// Inserts `text` as a new line before line `i`.
    pub fn insert_line(&mut self, i: usize, text: &str) {
        self.lines.insert(i, text.to_string());
        self.matches.insert(i, Vec::new());
        self.rebuild();
    }

    /// Removes line `i`, as a join of it onto the line above does.
    pub fn remove_line(&mut self, i: usize) {
        self.lines.remove(i);
        self.matches.remove(i);
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.buf = Buffer::from_text(&self.lines.join("\n"));
        self.blocks = Blocks::new(&self.buf);
    }

    /// The code block or table line `i` belongs to, grouped as the editor does.
    fn group(&self, i: usize) -> Option<(bool, u32)> {
        match *self.blocks.kind(i) {
            LineKind::FenceOpen { block, .. }
            | LineKind::FenceClose { block }
            | LineKind::Code { block } => Some((true, block.0)),
            LineKind::TableHeader { table }
            | LineKind::TableSep { table }
            | LineKind::TableRow { table } => Some((false, table.0)),
            _ => None,
        }
    }

    fn reveal(&self, i: usize) -> Reveal {
        if self.reader {
            Reveal::None
        } else if self.cursor.line == i {
            Reveal::Cursor(self.cursor.col)
        } else if self.group(i).is_some() && self.group(i) == self.group(self.cursor.line) {
            Reveal::Block
        } else {
            Reveal::None
        }
    }
}

impl ViewSource for Fake {
    fn line_count(&self) -> usize {
        self.lines.len()
    }
    fn line(&self, i: usize) -> &str {
        &self.lines[i]
    }
    fn display(&self, i: usize) -> LineDisplay {
        let l = &self.lines[i];
        let kind = self.blocks.kind(i);
        let mut d = line_display(l, kind, &parse_line(l, kind), self.reveal(i));
        match kind {
            LineKind::FenceOpen { .. } | LineKind::FenceClose { .. } => {
                d.deco = code_bg(&self.blocks, i);
            }
            LineKind::Code { block } => {
                d.deco = code_bg(&self.blocks, i);
                let lang = &self.blocks.code_block(*block).lang;
                for s in &mut d.spans {
                    if let SpanBase::CodeBlock { lang: l } = &mut s.style.base {
                        l.clone_from(lang);
                    }
                }
            }
            _ => {}
        }
        if let Prefix::Number(n) = &mut d.prefix {
            *n = ordered_display(&self.blocks, &self.buf, i);
        }
        d
    }
    fn cursor(&self) -> Pos {
        self.cursor
    }
    fn status(&self) -> StatusModel {
        StatusModel {
            left: self.left.clone(),
            file: self.file.clone(),
            dirty: self.dirty,
            warnings: self.warnings.clone(),
            showcmd: self.showcmd.clone(),
            position: format!("{}:{}", self.cursor.line + 1, self.cursor.col + 1),
        }
    }
    fn tokens(&self) -> &Tokens {
        &self.tokens
    }
    fn theme(&self) -> Theme {
        Theme::Light
    }
    fn reader(&self) -> bool {
        self.reader
    }
    fn selection(&self) -> Option<Selection> {
        self.selection
    }
    fn matches(&self, i: usize) -> &[Range<usize>] {
        &self.matches[i]
    }
    fn current_match(&self) -> Option<(usize, Range<usize>)> {
        self.current.clone()
    }
    fn caret_shape(&self) -> CaretShape {
        if self.reader {
            CaretShape::Hidden
        } else {
            self.caret
        }
    }
    fn preedit(&self) -> Option<(&str, usize)> {
        self.preedit.as_ref().map(|(t, c)| (t.as_str(), *c))
    }
}

/// Geometry from tokens, in pixels at `scale`.
pub struct Geom {
    pub height_px: u32,
    pub page_top: u32,
    pub body_line: u32,
    pub status_h: u32,
}

impl Geom {
    pub fn new(t: &Tokens, scale: f32) -> Geom {
        Geom {
            height_px: (H_PT * scale).round() as u32,
            page_top: (t.px(TokenId::SpacePageTop) * scale).round() as u32,
            body_line: (t.px(TokenId::FontSize) * t.ratio(TokenId::LineHeightBody) * scale).ceil()
                as u32,
            status_h: (t.px(TokenId::SizeStatusHeight) * scale).round() as u32,
        }
    }

    pub fn text_bottom(&self) -> u32 {
        self.height_px - self.status_h
    }

    /// Rows of body line `i`, when every line above it is one body row.
    pub fn line_rows(&self, i: u32) -> Range<u32> {
        let top = self.page_top + i * self.body_line;
        top..top + self.body_line
    }
}

/// A layout that counts `layout_line` calls and delegates to cosmic-text.
pub struct Counting {
    pub inner: CosmicLayout,
    pub calls: Rc<Cell<usize>>,
}

impl TextLayout for Counting {
    fn layout_line(&mut self, text: &str, spans: &[StyledSpan], width_px: f32) -> LaidLine {
        self.calls.set(self.calls.get() + 1);
        self.inner.layout_line(text, spans, width_px)
    }
    fn draw(&mut self, line: &LaidLine, sink: &mut dyn FnMut(GlyphMask)) {
        self.inner.draw(line, sink)
    }
    fn end_frame(&mut self) {
        self.inner.end_frame()
    }
}

/// One `layout_line` call as `Recording` saw it.
pub struct Laid {
    pub text: String,
    pub width_px: f32,
    pub rows: Vec<RowGeom>,
}

/// A layout that records every line it lays out and delegates to cosmic-text.
pub struct Recording {
    pub inner: CosmicLayout,
    pub laid: Rc<RefCell<Vec<Laid>>>,
}

impl TextLayout for Recording {
    fn layout_line(&mut self, text: &str, spans: &[StyledSpan], width_px: f32) -> LaidLine {
        let l = self.inner.layout_line(text, spans, width_px);
        self.laid.borrow_mut().push(Laid {
            text: text.to_string(),
            width_px,
            rows: l.row_geometry().to_vec(),
        });
        l
    }
    fn draw(&mut self, line: &LaidLine, sink: &mut dyn FnMut(GlyphMask)) {
        self.inner.draw(line, sink)
    }
    fn end_frame(&mut self) {
        self.inner.end_frame()
    }
}

/// The text column's left edge and width in pixels, as `DocView` computes them.
pub fn column(t: &Tokens, scale: f32) -> (f32, f32) {
    let width = (W_PT * scale).round();
    let side = t.px(TokenId::SpacePageSide) * scale;
    let col_w = (t.px(TokenId::SpaceColumn) * scale)
        .min(width - 2.0 * side)
        .max(1.0)
        .round();
    (((width - col_w) / 2.0).max(0.0).floor(), col_w)
}

/// Line `i`'s top in frame pixels (no scroll), from the view's line heights.
pub fn line_top(v: &DocView, t: &Tokens, scale: f32, i: usize) -> f32 {
    let page_top = (t.px(TokenId::SpacePageTop) * scale).round();
    page_top + (0..i).map(|j| v.line_height_px(j).unwrap()).sum::<f32>()
}

/// A host that does nothing.
pub struct NoHost;

impl Host for NoHost {
    fn clipboard_get(&mut self) -> Option<String> {
        None
    }
    fn clipboard_set(&mut self, _: &str) {}
    fn open_external(&mut self, _: &str) {}
    fn spawn_window(&mut self, _: WindowRequest) {}
    fn now_ns(&self) -> u64 {
        0
    }
    fn waker(&self) -> Waker {
        Arc::new(|| {})
    }
}

/// An editor (vim keys, Normal mode) over `text`, saved as `doc.md` in `dir`.
pub fn editor(dir: &Path, text: &str) -> Editor {
    let path = dir.join("doc.md");
    std::fs::write(&path, text).unwrap();
    let req = OpenRequest {
        path,
        view: false,
        display_name: None,
    };
    Editor::open_with_state(
        req,
        Config::default(),
        Platform::Mac,
        Box::new(NoHost),
        None,
    )
}

/// Sends `k` to the editor and paints what it asks for, as a shell does.
pub fn press(ed: &mut Editor, v: &mut DocView, k: Key) {
    let fx = ed.handle(Input::Key(KeyEvent {
        key: k,
        mods: Mods::default(),
        time_ns: 0,
    }));
    let mut redraw = fx.redraw;
    if fx.scroll_to_cursor && v.scroll_to_cursor(ed.cursor()) {
        redraw = Redraw::All;
    }
    match redraw {
        Redraw::None => {}
        Redraw::All => v.paint_all(ed),
        Redraw::Lines(r) => v.paint_lines(ed, r),
        Redraw::Status => v.paint_lines(ed, 0..0),
    }
    v.take_damage();
}

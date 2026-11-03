//! Shared fixtures: a fake `ViewSource` over real markdown display, and a
//! renderer built from the committed test fonts.

#![allow(dead_code)]

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use editor_core::buffer::{Buffer, Pos};
use editor_core::markdown::{Blocks, LineDisplay, Reveal, line_display, parse_line};
use editor_core::tokens::{Theme, TokenId, Tokens};
use editor_render::fonts::{FontPaths, FontSet};
use editor_render::layout::{GlyphMask, LaidLine, StyledSpan, TextLayout};
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

/// A `ViewSource` over plain lines, displayed through the real markdown model.
pub struct Fake {
    pub lines: Vec<String>,
    blocks: Blocks,
    pub tokens: Tokens,
    pub cursor: Pos,
    pub dirty: bool,
    pub reader: bool,
    pub selection: Option<Selection>,
    pub matches: Vec<Vec<Range<usize>>>,
}

impl Fake {
    pub fn new(lines: &[&str], tokens: Tokens) -> Fake {
        let lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        let blocks = Blocks::new(&Buffer::from_text(&lines.join("\n")));
        let matches = vec![Vec::new(); lines.len()];
        Fake {
            lines,
            blocks,
            tokens,
            cursor: Pos::default(),
            dirty: false,
            reader: false,
            selection: None,
            matches,
        }
    }

    pub fn set_line(&mut self, i: usize, text: &str) {
        self.lines[i] = text.to_string();
        self.blocks = Blocks::new(&Buffer::from_text(&self.lines.join("\n")));
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
        let reveal = if self.cursor.line == i && !self.reader {
            Reveal::Cursor(self.cursor.col)
        } else {
            Reveal::None
        };
        line_display(l, kind, &parse_line(l, kind), reveal)
    }
    fn cursor(&self) -> Pos {
        self.cursor
    }
    fn status(&self) -> StatusModel {
        StatusModel {
            mode: "INSERT",
            file: "notes.md".to_string(),
            dirty: self.dirty,
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

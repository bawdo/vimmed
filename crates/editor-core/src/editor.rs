//! The editor facade: the one type a shell drives. It owns the buffer, the
//! block structure, the cursor and the mode, turns `Input` into edits and
//! `Effects`, and implements `ViewSource` for the renderer.
//!
//! This is the insert-only skeleton: typing, Backspace, Delete, Enter (a
//! plain line split), the arrows, Home and End, and dirty tracking. The vim
//! engine (Task 19 onward) grows inside it.
//!
//! Every position that comes from the user goes through `Buffer::clamp` or
//! `grapheme::next` / `grapheme::prev` before it reaches `Buffer::replace`,
//! so no edit splits a grapheme cluster.

use std::io::ErrorKind;
use std::ops::Range;
use std::path::PathBuf;

use crate::buffer::{Buffer, Pos};
use crate::effects::{Effects, Redraw};
use crate::grapheme;
use crate::host::Host;
use crate::input::{Input, Key, KeyEvent};
use crate::markdown::{
    BlockId, Blocks, InlineKind, LineDisplay, LineKind, Prefix, Reveal, SpanBase, TableId, code_bg,
    line_display, ordered_display, parse_line,
};
use crate::platform::Platform;
use crate::status::{self, Mode};
use crate::strings;
use crate::tokens::{Theme, Tokens};
use crate::view_model::{Selection, StatusModel, ViewSource};

/// What to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub path: PathBuf,
    /// True to open in reader mode.
    pub view: bool,
    /// The name to show in place of the file name.
    pub display_name: Option<String>,
}

/// The editor's settings. A stub until Task 35.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Vim keybindings.
    pub vim: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config { vim: true }
    }
}

/// A run of lines that reveals as a whole: a fenced code block or a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Code(BlockId),
    Table(TableId),
}

/// True when the bench variable (`<P>BENCH`) is set and not empty.
fn bench_enabled() -> bool {
    std::env::var_os(format!("{}BENCH", app_meta::ENV_PREFIX)).is_some_and(|v| !v.is_empty())
}

/// The start of the first bold's content on a paragraph line, if any.
fn first_bold(buf: &Buffer, blocks: &Blocks) -> Option<Pos> {
    (0..buf.line_count()).find_map(|line| {
        let kind = blocks.kind(line);
        if *kind != LineKind::Paragraph {
            return None;
        }
        parse_line(buf.line(line), kind)
            .into_iter()
            .find(|e| matches!(e.kind, InlineKind::Bold))
            .map(|e| Pos {
                line,
                col: e.content.start,
            })
    })
}

pub struct Editor {
    buf: Buffer,
    blocks: Blocks,
    tokens: Tokens,
    cursor: Pos,
    /// The column (in graphemes) Up and Down aim for while they repeat.
    goal: Option<usize>,
    mode: Mode,
    dirty: bool,
    reader: bool,
    title: String,
    message: Option<&'static str>,
    cfg: Config,
    platform: Platform,
    host: Box<dyn Host>,
}

impl Editor {
    /// Reads `req.path` and opens it. A missing file opens empty under its
    /// name. Bench mode (`<P>BENCH` set) opens in Insert at the first bold
    /// on a paragraph line.
    pub fn open(req: OpenRequest, cfg: Config, platform: Platform, host: Box<dyn Host>) -> Editor {
        let (text, message) = match std::fs::read_to_string(&req.path) {
            Ok(text) => (text, None),
            Err(e) if e.kind() == ErrorKind::NotFound => (String::new(), None),
            // TODO(Task 31): real loading (invalid UTF-8, permissions, large
            // files). Until then any other failure opens empty.
            Err(_) => (String::new(), Some(strings::READ_FAILED)),
        };
        marks::mark(marks::names::FILE_READ);
        let editor = Editor::build(req, &text, message, cfg, platform, host, bench_enabled());
        marks::mark(marks::names::PARSED);
        editor
    }

    fn build(
        req: OpenRequest,
        text: &str,
        message: Option<&'static str>,
        cfg: Config,
        platform: Platform,
        host: Box<dyn Host>,
        bench: bool,
    ) -> Editor {
        let buf = Buffer::from_text(text);
        let blocks = Blocks::new(&buf);
        let title = req.display_name.clone().unwrap_or_else(|| {
            req.path.file_name().map_or_else(
                || strings::UNTITLED.to_owned(),
                |n| n.to_string_lossy().into_owned(),
            )
        });
        let cursor = if bench {
            first_bold(&buf, &blocks).unwrap_or_default()
        } else {
            Pos::default()
        };
        Editor {
            buf,
            blocks,
            tokens: Tokens::defaults(Theme::Light),
            cursor,
            goal: None,
            // TODO(Task 19): start in Normal when `cfg.vim` is set (bench
            // mode still starts in Insert). The skeleton is insert-only.
            mode: Mode::Insert,
            dirty: false,
            reader: req.view,
            title,
            message,
            cfg,
            platform,
            host,
        }
    }

    pub fn handle(&mut self, input: Input) -> Effects {
        match input {
            Input::Key(k) => self.key(k),
            Input::Text(text) => self.insert(&text),
            // TODO(later tasks): ask before closing with unsaved changes.
            Input::CloseRequested | Input::Signal(_) => Effects {
                close: Some(0),
                ..Effects::default()
            },
            Input::Preedit { .. }
            | Input::Mouse(_)
            | Input::Scroll { .. }
            | Input::Focus(_)
            | Input::Tick { .. }
            | Input::Appearance(_) => Effects::default(),
        }
    }

    /// The window title: the display name, or the file name.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// A message for the status bar, if there is one. Task 26 shows it.
    pub fn message(&self) -> Option<&str> {
        self.message
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// The whole document, as it would be saved.
    pub fn text(&self) -> String {
        self.buf.to_text()
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn host(&mut self) -> &mut dyn Host {
        self.host.as_mut()
    }

    fn key(&mut self, k: KeyEvent) -> Effects {
        if k.mods.ctrl || self.platform.primary(k.mods) {
            // Shortcuts arrive with later tasks.
            return Effects::default();
        }
        let line = self.buf.line(self.cursor.line);
        let col = self.buf.clamp(self.cursor).col;
        match k.key {
            Key::Char(c) => self.insert(c.encode_utf8(&mut [0; 4])),
            Key::Space => self.insert(" "),
            Key::Enter => self.insert("\n"),
            Key::Backspace => self.backspace(),
            Key::Delete => self.delete(),
            Key::Left => {
                let to = if col > 0 {
                    self.at(grapheme::prev(line, col))
                } else if self.cursor.line > 0 {
                    self.line_end(self.cursor.line - 1)
                } else {
                    return Effects::default();
                };
                self.move_across(to)
            }
            Key::Right => {
                let to = if col < line.len() {
                    self.at(grapheme::next(line, col))
                } else if self.cursor.line + 1 < self.buf.line_count() {
                    Pos {
                        line: self.cursor.line + 1,
                        col: 0,
                    }
                } else {
                    return Effects::default();
                };
                self.move_across(to)
            }
            Key::Home => self.move_across(self.at(0)),
            Key::End => self.move_across(self.line_end(self.cursor.line)),
            Key::Up => self.vertical(false),
            Key::Down => self.vertical(true),
            // TODO(Task 19 onward): Esc, Tab and paging.
            Key::Esc | Key::Tab | Key::PageUp | Key::PageDown => Effects::default(),
        }
    }

    /// A position on the cursor's line.
    fn at(&self, col: usize) -> Pos {
        Pos {
            line: self.cursor.line,
            col,
        }
    }

    fn line_end(&self, line: usize) -> Pos {
        Pos {
            line,
            col: self.buf.line(line).len(),
        }
    }

    fn insert(&mut self, text: &str) -> Effects {
        let at = self.buf.clamp(self.cursor);
        self.edit(at..at, text)
    }

    fn backspace(&mut self) -> Effects {
        let c = self.buf.clamp(self.cursor);
        let from = if c.col > 0 {
            Pos {
                line: c.line,
                col: grapheme::prev(self.buf.line(c.line), c.col),
            }
        } else if c.line > 0 {
            self.line_end(c.line - 1)
        } else {
            return Effects::default();
        };
        self.edit(from..c, "")
    }

    fn delete(&mut self) -> Effects {
        let c = self.buf.clamp(self.cursor);
        let line = self.buf.line(c.line);
        let to = if c.col < line.len() {
            Pos {
                line: c.line,
                col: grapheme::next(line, c.col),
            }
        } else if c.line + 1 < self.buf.line_count() {
            Pos {
                line: c.line + 1,
                col: 0,
            }
        } else {
            return Effects::default();
        };
        self.edit(c..to, "")
    }

    /// Replaces `r` (on grapheme boundaries) with `text`, leaves the cursor
    /// after it, and redraws the lines whose text or kind changed plus the
    /// blocks the cursor left and entered.
    fn edit(&mut self, r: Range<Pos>, text: &str) -> Effects {
        if self.reader {
            return Effects::default();
        }
        let old = self.reveal_extent(self.cursor.line);
        let before = self.buf.line_count();
        let inverse = self.buf.replace(r.clone(), text);
        let delta = self.buf.line_count() as isize - before as isize;
        let changed =
            self.blocks
                .update(&self.buf, r.start.line..inverse.range.end.line + 1, delta);

        // The old extent, in the new buffer: lines after the replaced range
        // moved by `delta`.
        let shift = |i: usize| {
            if i > r.end.line {
                i.saturating_add_signed(delta)
            } else {
                i
            }
        };
        let n = self.buf.line_count();
        let old = shift(old.start).min(n)..(shift(old.end - 1) + 1).min(n);

        self.cursor = self.buf.clamp(inverse.range.end);
        self.goal = None;
        let new = self.reveal_extent(self.cursor.line);
        let was_dirty = std::mem::replace(&mut self.dirty, true);
        Effects {
            redraw: Redraw::Lines(changed)
                .merge(Redraw::Lines(old))
                .merge(Redraw::Lines(new)),
            dirty: (!was_dirty).then_some(true),
            scroll_to_cursor: true,
            ..Effects::default()
        }
    }

    /// A horizontal motion: forgets the Up/Down goal column.
    fn move_across(&mut self, to: Pos) -> Effects {
        self.goal = None;
        self.move_to(to)
    }

    fn vertical(&mut self, down: bool) -> Effects {
        let line = self.cursor.line;
        let target = if down {
            if line + 1 >= self.buf.line_count() {
                return Effects::default();
            }
            line + 1
        } else {
            let Some(up) = line.checked_sub(1) else {
                return Effects::default();
            };
            up
        };
        let goal = match self.goal {
            Some(g) => g,
            None => grapheme::display_col(self.buf.line(line), self.cursor.col),
        };
        self.goal = Some(goal);
        let text = self.buf.line(target);
        let mut col = 0;
        for _ in 0..goal {
            if col >= text.len() {
                break;
            }
            col = grapheme::next(text, col);
        }
        self.move_to(Pos { line: target, col })
    }

    /// Moves the cursor, redrawing what it revealed and what it reveals.
    fn move_to(&mut self, to: Pos) -> Effects {
        let old = self.reveal_extent(self.cursor.line);
        self.cursor = self.buf.clamp(to);
        let new = self.reveal_extent(self.cursor.line);
        Effects {
            redraw: Redraw::Lines(old).merge(Redraw::Lines(new)),
            scroll_to_cursor: true,
            ..Effects::default()
        }
    }

    fn group(&self, line: usize) -> Option<Group> {
        match *self.blocks.kind(line) {
            LineKind::FenceOpen { block, .. }
            | LineKind::FenceClose { block }
            | LineKind::Code { block } => Some(Group::Code(block)),
            LineKind::TableHeader { table }
            | LineKind::TableSep { table }
            | LineKind::TableRow { table } => Some(Group::Table(table)),
            _ => None,
        }
    }

    /// The lines whose display depends on whether the cursor is on `line`:
    /// its whole code block or table, or just the line.
    fn reveal_extent(&self, line: usize) -> Range<usize> {
        let Some(g) = self.group(line) else {
            return line..line + 1;
        };
        let mut start = line;
        while start > 0 && self.group(start - 1) == Some(g) {
            start -= 1;
        }
        let mut end = line + 1;
        while end < self.buf.line_count() && self.group(end) == Some(g) {
            end += 1;
        }
        start..end
    }

    fn reveal(&self, i: usize) -> Reveal {
        if self.reader {
            Reveal::None
        } else if i == self.cursor.line {
            Reveal::Cursor(self.cursor.col)
        } else if self.group(i).is_some() && self.group(i) == self.group(self.cursor.line) {
            Reveal::Block
        } else {
            Reveal::None
        }
    }
}

impl ViewSource for Editor {
    fn line_count(&self) -> usize {
        self.buf.line_count()
    }
    fn line(&self, i: usize) -> &str {
        self.buf.line(i)
    }
    /// The line's display: inline elements from `parse_line`, the code
    /// background from `code_bg`, ordered numbers from `ordered_display`, and
    /// a code line's language from its block.
    fn display(&self, i: usize) -> LineDisplay {
        let line = self.buf.line(i);
        let kind = self.blocks.kind(i);
        let mut d = line_display(line, kind, &parse_line(line, kind), self.reveal(i));
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
            mode: self.mode.name(),
            file: self.title.clone(),
            dirty: self.dirty,
            position: status::position(&self.buf, self.cursor),
        }
    }
    fn tokens(&self) -> &Tokens {
        &self.tokens
    }
    fn theme(&self) -> Theme {
        // TODO(Task 35): follow the appearance and the config.
        Theme::Light
    }
    fn reader(&self) -> bool {
        self.reader
    }
    fn selection(&self) -> Option<Selection> {
        None
    }
    fn matches(&self, _i: usize) -> &[Range<usize>] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::fake::FakeHost;
    use crate::input::{Mods, SignalKind};
    use crate::markdown::{
        BlockDeco, Prefix, Reveal, SpanBase, line_display, parse_inline, parse_line,
    };
    use crate::strings;

    fn req(path: &str) -> OpenRequest {
        OpenRequest {
            path: PathBuf::from(path),
            view: false,
            display_name: None,
        }
    }

    fn build(text: &str, bench: bool) -> Editor {
        Editor::build(
            req("notes.md"),
            text,
            None,
            Config::default(),
            Platform::Mac,
            Box::new(FakeHost::default()),
            bench,
        )
    }

    fn ed(text: &str) -> Editor {
        build(text, false)
    }

    fn key(k: Key) -> Input {
        Input::Key(KeyEvent {
            key: k,
            mods: Mods::default(),
            time_ns: 0,
        })
    }

    fn at(e: &mut Editor, line: usize, col: usize) {
        e.cursor = Pos { line, col };
    }

    fn type_str(e: &mut Editor, s: &str) -> Effects {
        let mut fx = Effects::default();
        for c in s.chars() {
            let k = if c == ' ' { Key::Space } else { Key::Char(c) };
            let out = e.handle(key(k));
            fx.redraw = std::mem::take(&mut fx.redraw).merge(out.redraw);
            fx.dirty = fx.dirty.or(out.dirty);
        }
        fx
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("abc_def-editor-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn typing_inserts_and_marks_dirty() {
        let mut e = ed("hello");
        assert!(!e.dirty());
        at(&mut e, 0, 5);
        let fx = type_str(&mut e, " you");
        assert_eq!(e.text(), "hello you");
        assert_eq!(e.cursor(), Pos { line: 0, col: 9 });
        assert!(e.dirty());
        assert_eq!(fx.dirty, Some(true));
        // Already dirty: the next edit does not report it again.
        assert_eq!(e.handle(key(Key::Char('!'))).dirty, None);
        assert!(e.status().dirty);
    }

    #[test]
    fn text_input_inserts_committed_text() {
        let mut e = ed("ab");
        at(&mut e, 0, 1);
        e.handle(Input::Text("日本".into()));
        assert_eq!(e.text(), "a日本b");
        assert_eq!(e.cursor(), Pos { line: 0, col: 7 });
    }

    #[test]
    fn shortcut_chars_do_not_insert() {
        let mut e = ed("ab");
        let fx = e.handle(Input::Key(KeyEvent {
            key: Key::Char('s'),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
            time_ns: 0,
        }));
        assert_eq!(e.text(), "ab");
        assert_eq!(fx, Effects::default());
        e.handle(Input::Key(KeyEvent {
            key: Key::Char('s'),
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            time_ns: 0,
        }));
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn backspace_at_col0_joins() {
        let mut e = ed("one\ntwo\nthree");
        at(&mut e, 1, 0);
        let fx = e.handle(key(Key::Backspace));
        assert_eq!(e.text(), "onetwo\nthree");
        assert_eq!(e.cursor(), Pos { line: 0, col: 3 });
        assert_eq!(fx.dirty, Some(true));
        assert!(fx.scroll_to_cursor);
        // At the very start there is nothing to join.
        at(&mut e, 0, 0);
        assert_eq!(e.handle(key(Key::Backspace)).dirty, None);
        assert_eq!(e.text(), "onetwo\nthree");
    }

    #[test]
    fn delete_at_line_end_joins_the_next_line() {
        let mut e = ed("ab\ncd");
        at(&mut e, 0, 2);
        e.handle(key(Key::Delete));
        assert_eq!(e.text(), "abcd");
        assert_eq!(e.cursor(), Pos { line: 0, col: 2 });
        e.handle(key(Key::Delete));
        assert_eq!(e.text(), "abd");
    }

    #[test]
    fn enter_splits_the_line_without_continuing_lists() {
        let mut e = ed("- item one");
        at(&mut e, 0, 6);
        e.handle(key(Key::Enter));
        assert_eq!(e.text(), "- item\n one");
        assert_eq!(e.cursor(), Pos { line: 1, col: 0 });
    }

    #[test]
    fn edits_and_motions_never_split_graphemes() {
        // A decomposed accent, then a ZWJ family emoji, then x.
        let s = "e\u{301}👩\u{200d}👩\u{200d}👧x";
        let mut e = ed(s);
        let mut cols = vec![0];
        for _ in 0..4 {
            e.handle(key(Key::Right));
            cols.push(e.cursor().col);
        }
        assert_eq!(cols, [0, 3, 21, 22, 22]);
        for _ in 0..3 {
            e.handle(key(Key::Left));
        }
        assert_eq!(e.cursor().col, 0);

        // Backspace from just after the emoji takes the whole family, then
        // the accent with its base.
        at(&mut e, 0, 21);
        e.handle(key(Key::Backspace));
        assert_eq!(e.text(), "e\u{301}x");
        assert_eq!(e.cursor().col, 3);
        e.handle(key(Key::Backspace));
        assert_eq!(e.text(), "x");
        assert_eq!(e.cursor().col, 0);

        // Delete takes a whole cluster too.
        let mut e = ed(s);
        at(&mut e, 0, 3);
        e.handle(key(Key::Delete));
        assert_eq!(e.text(), "e\u{301}x");
        // A cursor left mid-cluster snaps back before inserting.
        let mut e = ed(s);
        at(&mut e, 0, 7);
        e.handle(key(Key::Char('y')));
        assert_eq!(e.text(), "e\u{301}y👩\u{200d}👩\u{200d}👧x");
    }

    #[test]
    fn up_and_down_keep_the_grapheme_column() {
        let mut e = ed("e\u{301}bcd\nxy\n👩\u{200d}👩\u{200d}👧abc");
        at(&mut e, 0, 5); // after e-acute and b, c: grapheme column 3
        e.handle(key(Key::Down));
        assert_eq!(e.cursor(), Pos { line: 1, col: 2 });
        e.handle(key(Key::Down));
        assert_eq!(e.cursor(), Pos { line: 2, col: 20 });
        e.handle(key(Key::Up));
        e.handle(key(Key::Up));
        assert_eq!(e.cursor(), Pos { line: 0, col: 5 });
        e.handle(key(Key::Up));
        assert_eq!(e.cursor(), Pos { line: 0, col: 5 });
    }

    #[test]
    fn left_and_right_wrap_between_lines() {
        let mut e = ed("ab\ncd");
        at(&mut e, 1, 0);
        e.handle(key(Key::Left));
        assert_eq!(e.cursor(), Pos { line: 0, col: 2 });
        e.handle(key(Key::Right));
        assert_eq!(e.cursor(), Pos { line: 1, col: 0 });
        e.handle(key(Key::End));
        assert_eq!(e.cursor(), Pos { line: 1, col: 2 });
        e.handle(key(Key::Home));
        assert_eq!(e.cursor(), Pos { line: 1, col: 0 });
    }

    #[test]
    fn effects_redraw_only_edited_line() {
        let mut e = ed("first\n\nsecond line\n\nthird");
        at(&mut e, 2, 6);
        let fx = e.handle(key(Key::Char('x')));
        assert_eq!(fx.redraw, Redraw::Lines(2..3));
        assert!(fx.scroll_to_cursor);
        let fx = e.handle(key(Key::Backspace));
        assert_eq!(fx.redraw, Redraw::Lines(2..3));
        let fx = e.handle(key(Key::Right));
        assert_eq!(fx.redraw, Redraw::Lines(2..3));
    }

    #[test]
    fn leaving_a_code_block_redraws_the_whole_block() {
        let mut e = ed("```\na\nb\n```\nafter");
        at(&mut e, 3, 3);
        let fx = e.handle(key(Key::Down));
        assert_eq!(e.cursor(), Pos { line: 4, col: 3 });
        assert_eq!(fx.redraw, Redraw::Lines(0..5));
    }

    #[test]
    fn typing_a_fence_redraws_the_lines_it_turns_to_code() {
        let mut e = ed("``\na\nb");
        at(&mut e, 0, 2);
        let fx = e.handle(key(Key::Char('`')));
        assert_eq!(fx.redraw, Redraw::Lines(0..3));
    }

    #[test]
    fn bench_mode_starts_at_first_bold_paragraph() {
        let text = "# **Heading** bold\n\n- **item**\n\nplain\nsome **bold** text";
        let e = build(text, true);
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), Pos { line: 5, col: 7 });
        assert_eq!(e.status().mode, "INSERT");
    }

    #[test]
    fn bench_mode_without_bold_starts_at_the_document_start() {
        let e = build("plain\ntext", true);
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), Pos::default());
    }

    #[test]
    fn missing_file_opens_empty_with_name() {
        let path = scratch("missing").join("nothing-here.md");
        let e = Editor::open(
            req(path.to_str().unwrap()),
            Config::default(),
            Platform::Mac,
            Box::new(FakeHost::default()),
        );
        assert_eq!(e.title(), "nothing-here.md");
        assert_eq!(e.text(), "");
        assert_eq!(e.line_count(), 1);
        assert_eq!(e.message(), None);
        assert!(!e.dirty());
        assert_eq!(e.status().file, "nothing-here.md");
    }

    #[test]
    fn existing_file_opens_with_its_text() {
        let path = scratch("existing").join("doc.md");
        std::fs::write(&path, "# Title\n\nbody\n").unwrap();
        let mut r = req(path.to_str().unwrap());
        r.display_name = Some("Shown Name".into());
        let e = Editor::open(
            r,
            Config::default(),
            Platform::Mac,
            Box::new(FakeHost::default()),
        );
        assert_eq!(e.text(), "# Title\n\nbody\n");
        assert_eq!(e.line_count(), 3);
        assert_eq!(e.title(), "Shown Name");
        assert_eq!(e.status().file, "Shown Name");
    }

    #[test]
    fn unreadable_file_opens_empty_with_a_message() {
        // A directory exists but cannot be read as text.
        let dir = scratch("unreadable");
        let e = Editor::open(
            req(dir.to_str().unwrap()),
            Config::default(),
            Platform::Mac,
            Box::new(FakeHost::default()),
        );
        assert_eq!(e.text(), "");
        assert_eq!(e.message(), Some(strings::READ_FAILED));
    }

    #[test]
    fn config_defaults_to_vim() {
        assert!(Config::default().vim);
    }

    #[test]
    fn status_shows_mode_file_and_grapheme_position() {
        let mut e = ed("e\u{301}x");
        at(&mut e, 0, 3);
        let s = e.status();
        assert_eq!(s.mode, "INSERT");
        assert_eq!(s.file, "notes.md");
        assert!(!s.dirty);
        assert_eq!(s.position, "1:2");
    }

    #[test]
    fn reader_mode_ignores_edits_and_reveals_nothing() {
        let mut e = ed("**b**");
        e.reader = true;
        let fx = e.handle(key(Key::Char('x')));
        assert_eq!(e.text(), "**b**");
        assert_eq!(fx.dirty, None);
        assert_eq!(e.reveal(0), Reveal::None);
    }

    #[test]
    fn close_requested_closes_with_code_zero() {
        let mut e = ed("");
        assert_eq!(e.handle(Input::CloseRequested).close, Some(0));
        assert_eq!(
            e.handle(Input::Signal(SignalKind::Terminate)).close,
            Some(0)
        );
    }

    // ViewSource::display: how each line's display is assembled.

    #[test]
    fn display_parses_inline_after_the_prefix() {
        let text = "* a *b*";
        let mut e = ed(&format!("x\n{text}"));
        at(&mut e, 0, 0);
        let kind = e.blocks.kind(1).clone();
        let d = e.display(1);
        assert_eq!(
            d,
            line_display(text, &kind, &parse_line(text, &kind), Reveal::None)
        );
        assert_ne!(
            d,
            line_display(text, &kind, &parse_inline(text), Reveal::None)
        );
        assert!(d.spans.iter().any(|s| s.src == (5..6) && s.style.em));
    }

    #[test]
    fn display_takes_code_bg_from_the_blocks() {
        // Unclosed: line_display alone would say `unclosed: false`.
        let mut e = ed("x\n```rust\nfn f()");
        at(&mut e, 0, 0);
        assert_eq!(
            e.display(2).deco,
            BlockDeco::CodeBg {
                first: false,
                last: true,
                unclosed: true
            }
        );
        assert_eq!(
            e.display(1).deco,
            BlockDeco::CodeBg {
                first: true,
                last: false,
                unclosed: true
            }
        );
    }

    #[test]
    fn display_names_the_code_block_language() {
        let mut e = ed("x\n```rust\nfn f()\n```");
        at(&mut e, 0, 0);
        let d = e.display(2);
        assert!(!d.spans.is_empty());
        for s in &d.spans {
            assert_eq!(
                s.style.base,
                SpanBase::CodeBlock {
                    lang: Some("rust".into())
                }
            );
        }
    }

    #[test]
    fn display_numbers_ordered_items_from_the_blocks() {
        let mut e = ed("x\n1. a\n1. b\n  1. c");
        at(&mut e, 0, 0);
        assert_eq!(e.display(2).prefix, Prefix::Number("2.".into()));
        assert_eq!(e.display(3).prefix, Prefix::Number("a.".into()));
    }

    #[test]
    fn reveal_follows_the_cursor() {
        let mut e = ed("para **b**\n```\ncode\n```\n| a |\n|---|\n| 1 |\nend");
        at(&mut e, 0, 6);
        assert_eq!(e.reveal(0), Reveal::Cursor(6));
        assert_eq!(e.reveal(1), Reveal::None);
        assert_eq!(e.reveal(4), Reveal::None);
        // The cursor line shows its element raw.
        assert!(
            e.display(0)
                .spans
                .iter()
                .any(|s| s.style.base == SpanBase::Syntax)
        );

        at(&mut e, 2, 1);
        assert_eq!(e.reveal(2), Reveal::Cursor(1));
        assert_eq!(e.reveal(1), Reveal::Block);
        assert_eq!(e.reveal(3), Reveal::Block);
        assert_eq!(e.reveal(0), Reveal::None);
        assert_eq!(e.reveal(4), Reveal::None);

        at(&mut e, 6, 0);
        assert_eq!(e.reveal(4), Reveal::Block);
        assert_eq!(e.reveal(5), Reveal::Block);
        assert_eq!(e.reveal(3), Reveal::None);
        assert_eq!(e.reveal(7), Reveal::None);
    }
}

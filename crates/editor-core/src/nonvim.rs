//! Non-vim editing (`Config::vim` false): what a key means, and the state
//! the editor keeps for it. The editor carries the actions out
//! (`editor::nonvim_keys`), as it does the vim engine's steps.
//!
//! - The arrows move and Shift+arrows select. Home and End go to the line's
//!   start and end. Words and lines follow each platform: on the Mac,
//!   Alt+Left/Right go by word, Cmd+Left/Right to the line's start and end
//!   and Cmd+Up/Down to the document's (Alt+Up/Down to the line's, a
//!   markdown line being a paragraph); on Linux, Ctrl+Left/Right go by word
//!   and Ctrl+Home/End to the document's start and end. Shift with any of
//!   them selects.
//! - Primary+B wraps the selection in `**` or takes the `**` round it off,
//!   and Primary+I does the same with `*` (`toggle_wrap`). Primary+K is the
//!   link prompt, as Primary+L is. Primary+Z undoes, Primary+Shift+Z redoes.
//!   Primary+C, X and V use the clipboard. Primary+F opens the find bar.
//!   Primary+S, Primary+L, Primary+Shift+R and Ctrl+Space are the editor's
//!   own, with vim or without.
//! - The selection runs from an anchor to the cursor, the cursor's side
//!   open: typing, Backspace, Delete, Enter, Tab or a paste replace it.
//!
//! Undo steps (`Run`): a run of typed text is one step, and so is a run of
//! Backspace and Delete. Anything else ends the run: another key, a click,
//! a chord, the other kind of run, a write (Primary+S or autosave: the save
//! point is never inside a step, so undo comes back to the text written)
//! or a reload. Enter, Tab, Shift+Tab, Ctrl+Space, a paste, a cut, a
//! formatting chord and a link are each a step of their own, as is typing
//! over a selection (the typing after it joins that step). An input
//! method's composition and the main loop's ticks do not end a run; a tick
//! that writes or reloads does, as above.
//!
//! The find bar (`FindBar`) is a line in the status row, `find: <query>`,
//! with `<i> of <n>` (`0 of 0` with no match) on the right. It uses the
//! search engine (`crate::search`): smartcase, and `search wrapped` when
//! Enter (next) or Shift+Enter (previous) goes round the end, shown before
//! the counter for a second. Typing searches again from where the bar was
//! opened, a match there counting. Esc closes it, leaving the cursor on
//! the match and taking the highlights away.

use std::ops::Range;

use crate::buffer::{Buffer, Pos};
use crate::grapheme;
use crate::input::{Key, KeyEvent, Mods};
use crate::platform::Platform;
use crate::vim::operator::ListKey;

/// Where a motion goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    WordLeft,
    WordRight,
    DocStart,
    DocEnd,
}

/// What a key means with vim off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Moves the cursor; with `select`, the selection's head.
    Move {
        motion: Motion,
        select: bool,
    },
    /// Types the text.
    Type(String),
    Backspace,
    Delete,
    /// Enter, Tab or Shift+Tab (`list_keys`).
    List(ListKey),
    Bold,
    Italic,
    Link,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    Find,
    /// Drops the selection.
    Esc,
    /// Nothing (a chord the spec leaves free, paging).
    Nothing,
}

/// What `k` means on `platform`. Primary+S, Primary+L, Primary+Shift+R and
/// Ctrl+Space never get here (the editor takes them first).
pub fn action(k: &KeyEvent, platform: Platform) -> Action {
    let m = k.mods;
    let primary = platform.primary(m);
    if let Some(motion) = motion(k.key, m, platform) {
        return Action::Move {
            motion,
            select: m.shift,
        };
    }
    if primary {
        // The primary modifier alone, Shift allowed.
        let other = match platform {
            Platform::Mac => m.ctrl,
            Platform::Linux => m.cmd,
        };
        if other || m.alt {
            return Action::Nothing;
        }
        return match k.key {
            Key::Char('b' | 'B') => Action::Bold,
            Key::Char('i' | 'I') => Action::Italic,
            Key::Char('k' | 'K') => Action::Link,
            Key::Char('z') if !m.shift => Action::Undo,
            Key::Char('z' | 'Z') => Action::Redo,
            Key::Char('c' | 'C') => Action::Copy,
            Key::Char('x' | 'X') => Action::Cut,
            Key::Char('v' | 'V') => Action::Paste,
            Key::Char('f' | 'F') => Action::Find,
            _ => Action::Nothing,
        };
    }
    // Ctrl (or Cmd on Linux) with anything else is no text.
    if m.ctrl || m.cmd {
        return Action::Nothing;
    }
    match k.key {
        Key::Char(c) => Action::Type(c.to_string()),
        Key::Space => Action::Type(" ".to_owned()),
        Key::Enter => Action::List(ListKey::Enter),
        Key::Tab if m.shift => Action::List(ListKey::BackTab),
        Key::Tab => Action::List(ListKey::Tab),
        Key::Backspace => Action::Backspace,
        Key::Delete => Action::Delete,
        Key::Esc => Action::Esc,
        _ => Action::Nothing,
    }
}

/// The motion `key` with `m` held makes on `platform`, if it is one.
/// Shift only selects, so it is left out here.
fn motion(key: Key, m: Mods, platform: Platform) -> Option<Motion> {
    let plain = !m.ctrl && !m.alt && !m.cmd;
    let basic = match key {
        Key::Left => Motion::Left,
        Key::Right => Motion::Right,
        Key::Up => Motion::Up,
        Key::Down => Motion::Down,
        Key::Home => Motion::LineStart,
        Key::End => Motion::LineEnd,
        _ => return None,
    };
    if plain {
        return Some(basic);
    }
    let only = |want: Mods| Mods { shift: false, ..m } == want;
    let alt = Mods {
        alt: true,
        ..Mods::default()
    };
    let cmd = Mods {
        cmd: true,
        ..Mods::default()
    };
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    match platform {
        Platform::Mac if only(alt) => match key {
            Key::Left => Some(Motion::WordLeft),
            Key::Right => Some(Motion::WordRight),
            Key::Up => Some(Motion::LineStart),
            Key::Down => Some(Motion::LineEnd),
            _ => None,
        },
        Platform::Mac if only(cmd) => match key {
            Key::Left => Some(Motion::LineStart),
            Key::Right => Some(Motion::LineEnd),
            Key::Up => Some(Motion::DocStart),
            Key::Down => Some(Motion::DocEnd),
            _ => None,
        },
        Platform::Linux if only(ctrl) => match key {
            Key::Left => Some(Motion::WordLeft),
            Key::Right => Some(Motion::WordRight),
            Key::Home => Some(Motion::DocStart),
            Key::End => Some(Motion::DocEnd),
            _ => None,
        },
        _ => None,
    }
}

/// True for a grapheme that is part of a word: one starting with a letter,
/// a digit or `_`.
fn wordy(g: &str) -> bool {
    g.chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// The end of the next word after `at`, over line breaks: Alt+Right on the
/// Mac, Ctrl+Right on Linux. At the document's end it stays.
pub fn word_right(buf: &Buffer, at: Pos) -> Pos {
    let mut p = buf.clamp(at);
    // Past what is not a word, line breaks included.
    loop {
        let line = buf.line(p.line);
        if p.col >= line.len() {
            if p.line + 1 >= buf.line_count() {
                return p;
            }
            p = Pos {
                line: p.line + 1,
                col: 0,
            };
            continue;
        }
        let next = grapheme::next(line, p.col);
        if wordy(&line[p.col..next]) {
            break;
        }
        p.col = next;
    }
    // Then to the word's end.
    let line = buf.line(p.line);
    while p.col < line.len() {
        let next = grapheme::next(line, p.col);
        if !wordy(&line[p.col..next]) {
            break;
        }
        p.col = next;
    }
    p
}

/// The start of the word before `at`, over line breaks: Alt+Left on the
/// Mac, Ctrl+Left on Linux. At the document's start it stays.
pub fn word_left(buf: &Buffer, at: Pos) -> Pos {
    let mut p = buf.clamp(at);
    loop {
        if p.col == 0 {
            if p.line == 0 {
                return p;
            }
            p = Pos {
                line: p.line - 1,
                col: buf.line(p.line - 1).len(),
            };
            continue;
        }
        let line = buf.line(p.line);
        let prev = grapheme::prev(line, p.col);
        if wordy(&line[prev..p.col]) {
            break;
        }
        p.col = prev;
    }
    let line = buf.line(p.line);
    while p.col > 0 {
        let prev = grapheme::prev(line, p.col);
        if !wordy(&line[prev..p.col]) {
            break;
        }
        p.col = prev;
    }
    p
}

/// One grapheme left of `at`, over a line break; `None` at the start.
pub fn left(buf: &Buffer, at: Pos) -> Option<Pos> {
    let at = buf.clamp(at);
    if at.col > 0 {
        let col = grapheme::prev(buf.line(at.line), at.col);
        return Some(Pos { col, ..at });
    }
    let line = at.line.checked_sub(1)?;
    Some(Pos {
        line,
        col: buf.line(line).len(),
    })
}

/// One grapheme right of `at`, over a line break; `None` at the end.
pub fn right(buf: &Buffer, at: Pos) -> Option<Pos> {
    let at = buf.clamp(at);
    let line = buf.line(at.line);
    if at.col < line.len() {
        let col = grapheme::next(line, at.col);
        return Some(Pos { col, ..at });
    }
    (at.line + 1 < buf.line_count()).then_some(Pos {
        line: at.line + 1,
        col: 0,
    })
}

/// The last position in `buf`.
pub fn doc_end(buf: &Buffer) -> Pos {
    let line = buf.line_count() - 1;
    Pos {
        line,
        col: buf.line(line).len(),
    }
}

/// Where `text` inserted at `at` ends.
fn after(at: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        None => Pos {
            col: at.col + text.len(),
            ..at
        },
        Some(i) => Pos {
            line: at.line + text.matches('\n').count(),
            col: text.len() - i - 1,
        },
    }
}

/// What a formatting chord does: replace `range` with `text`, then select
/// `select` (the text the markers wrap, without them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrap {
    pub range: Range<Pos>,
    pub text: String,
    pub select: Range<Pos>,
}

/// True when runs of `*` `n` long hold the marker `width` stars wide: `*`
/// in `*x*` or `***x***`, `**` in `**x**` or `***x***`.
fn marked(n: usize, width: usize) -> bool {
    n == width || n >= 3
}

/// Primary+B (`width` 2) or Primary+I (`width` 1) over `sel`: takes the
/// marker off when it is round the selection, or just inside its ends;
/// else wraps the selection in it. With nothing selected, a marker pair
/// round the cursor comes off, else one goes in with the cursor between.
pub fn toggle_wrap(buf: &Buffer, sel: Range<Pos>, width: usize) -> Wrap {
    let stars = |n: usize| "*".repeat(n);
    let (start, end) = (buf.clamp(sel.start), buf.clamp(sel.end));
    let before = buf.line(start.line)[..start.col]
        .bytes()
        .rev()
        .take_while(|&b| b == b'*')
        .count();
    let after_end = buf.line(end.line)[end.col..]
        .bytes()
        .take_while(|&b| b == b'*')
        .count();
    let inner = buf.text_range(start..end);
    // Round the selection.
    if marked(before.min(after_end), width) {
        let from = Pos {
            col: start.col - width,
            ..start
        };
        let to = Pos {
            col: end.col + width,
            ..end
        };
        let select = from..after(from, &inner);
        return Wrap {
            range: from..to,
            text: inner,
            select,
        };
    }
    // Just inside its ends.
    let lead = inner.bytes().take_while(|&b| b == b'*').count();
    let trail = inner.bytes().rev().take_while(|&b| b == b'*').count();
    if inner.len() >= 2 * width && lead < inner.len() && marked(lead.min(trail), width) {
        let text = inner[width..inner.len() - width].to_owned();
        let select = start..after(start, &text);
        return Wrap {
            range: start..end,
            text,
            select,
        };
    }
    let from = after(start, &stars(width));
    let select = from..after(from, &inner);
    Wrap {
        range: start..end,
        text: format!("{}{inner}{}", stars(width), stars(width)),
        select,
    }
}

/// Which kind of run a typed key belongs to (see the module's notes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Typing,
    Deleting,
}

/// A run of typing: its kind and the undo step it is building.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    kind: RunKind,
    group: u64,
}

/// The find bar while it shows: where it was opened, and its counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindBar {
    /// The cursor when the bar opened: typing searches from here.
    pub origin: Pos,
    /// The current match, counting from 1 (0 with none).
    pub at: usize,
    /// How many matches there are.
    pub of: usize,
}

/// The non-vim state the editor keeps.
#[derive(Debug, Default)]
pub struct NonVim {
    /// The run the last input typed into.
    run: Option<Run>,
    /// The run the input being handled may go on with.
    prev: Option<Run>,
    find: Option<FindBar>,
}

impl NonVim {
    /// An input other than a tick or a composition arrived: the run goes
    /// on only if this input types more of the same kind (`ran`).
    pub fn input(&mut self) {
        self.prev = self.run.take();
    }

    /// The undo step a typed key of `kind` may join: the run's, while
    /// nothing else has come between.
    pub fn joins(&mut self, kind: RunKind) -> Option<u64> {
        self.prev.take().filter(|r| r.kind == kind).map(|r| r.group)
    }

    /// A typed key of `kind` built undo step `group`.
    pub fn ran(&mut self, kind: RunKind, group: u64) {
        self.run = Some(Run { kind, group });
    }

    /// The find bar, while it shows.
    pub fn find(&self) -> Option<&FindBar> {
        self.find.as_ref()
    }

    pub fn find_mut(&mut self) -> Option<&mut FindBar> {
        self.find.as_mut()
    }

    /// Opens the find bar at `origin`.
    pub fn open_find(&mut self, origin: Pos) {
        self.find = Some(FindBar {
            origin,
            at: 0,
            of: 0,
        });
    }

    pub fn close_find(&mut self) {
        self.find = None;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::editor::{Config, Editor, OpenRequest};
    use crate::file::{DiskEvent, StateDir};
    use crate::input::Input;
    use crate::platform::Platform;
    use crate::status::Mode;
    use crate::strings;
    use crate::testkit::{
        FakeHost, ViewSource, build_reader, ed, keys, plain_linux_with_host, plain_with_files,
        plain_with_host, pos,
    };
    use crate::view_model::{CaretShape, Selection, StatusLeft};

    /// The selection as the renderer gets it: `anchor` to `head`, head
    /// inclusive.
    fn sel(anchor: (usize, usize), head: (usize, usize)) -> Option<Selection> {
        Some(Selection {
            anchor: pos(anchor.0, anchor.1),
            head: pos(head.0, head.1),
            linewise: false,
        })
    }

    #[test]
    fn bold_wraps_selection() {
        let (mut e, _host) = plain_with_host("make it bold");
        keys(&mut e, "<End><M-Left><S-M-Right>");
        assert_eq!(e.selection(), sel((0, 8), (0, 11)));
        keys(&mut e, "<D-b>");
        assert_eq!(e.text(), "make it **bold**");
        // The selection stays on the text, inside the markers.
        assert_eq!(e.selection(), sel((0, 10), (0, 13)));
        // Again: unwrapped.
        keys(&mut e, "<D-b>");
        assert_eq!(e.text(), "make it bold");
        assert_eq!(e.selection(), sel((0, 8), (0, 11)));
        // Italic over bold, then each taken off on its own.
        keys(&mut e, "<D-b><D-i>");
        assert_eq!(e.text(), "make it ***bold***");
        keys(&mut e, "<D-b>");
        assert_eq!(e.text(), "make it *bold*");
        keys(&mut e, "<D-i>");
        assert_eq!(e.text(), "make it bold");
        // Italic on the text of a bold run does not eat a bold star.
        keys(&mut e, "<D-b><D-i><D-i>");
        assert_eq!(e.text(), "make it **bold**");
        // A selection holding its markers unwraps from the inside.
        keys(&mut e, "<End>");
        for _ in 0..8 {
            keys(&mut e, "<S-Left>");
        }
        assert_eq!(e.selection(), sel((0, 8), (0, 15)));
        keys(&mut e, "<D-b>");
        assert_eq!(e.text(), "make it bold");
        assert_eq!(e.selection(), sel((0, 8), (0, 11)));
        // With no selection, a pair with the cursor between.
        keys(&mut e, "<Right><D-b>x");
        assert_eq!(e.text(), "make it bold**x**");
        // Each chord is one undo step.
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "make it bold****");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "make it bold");
    }

    #[test]
    fn undo_redo_keys() {
        let (mut e, _host) = plain_with_host("");
        keys(&mut e, "hello<BS><BS>p!");
        assert_eq!(e.text(), "help!");
        // Typing is one step, the run of Backspaces another, and the
        // typing after them a third.
        assert_eq!(e.history().undo_len(), 3);
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "hel");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "hello");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "");
        assert!(!e.dirty());
        keys(&mut e, "<D-S-z>");
        assert_eq!(e.text(), "hello");
        keys(&mut e, "<D-S-z><D-S-z>");
        assert_eq!(e.text(), "help!");
        assert!(e.dirty());
        // A space is typing; an arrow ends the step.
        let (mut e, _host) = plain_with_host("");
        keys(&mut e, "ab cd<Left>X<Right>yz");
        assert_eq!(e.text(), "ab cXdyz");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "ab cXd");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "ab cd");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "");
    }

    #[test]
    fn undo_redo_keys_on_linux() {
        let (mut e, _host) = plain_linux_with_host("");
        keys(&mut e, "ab");
        keys(&mut e, "<C-z>");
        assert_eq!(e.text(), "");
        keys(&mut e, "<C-S-z>");
        assert_eq!(e.text(), "ab");
        // Cmd is no shortcut on Linux.
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn a_write_ends_the_typed_run() {
        // Primary+S, and autosave, write at a step's end: the typing after
        // is a step of its own, and undo comes back to the text written.
        let (mut e, files) = plain_with_files("");
        keys(&mut e, "ab<D-s>cd");
        assert_eq!(files.disk(), "ab");
        assert_eq!(e.message(), Some(strings::WRITTEN));
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "ab");
        assert!(!e.dirty());
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "");
    }

    #[test]
    fn autosave_writes_between_runs_never_inside_one() {
        use crate::testkit::{FakeFiles, req};
        // Well past autosave's idle pause.
        const LATER: u64 = 5_000_000_000;
        let host = FakeHost::default();
        let mut e = Editor::build(
            req("notes.md"),
            "",
            None,
            Config {
                vim: false,
                autosave: true,
                ..Config::default()
            },
            Platform::Mac,
            Box::new(host.clone()),
            false,
        );
        let files = FakeFiles::default();
        e.set_saver(Box::new(files.clone()));
        keys(&mut e, "ab");
        // Ticks that write nothing (a message fading) leave the run be.
        e.handle(Input::Tick { now_ns: 1 });
        keys(&mut e, "c");
        host.state().now = LATER;
        e.handle(Input::Tick { now_ns: LATER });
        assert_eq!(files.disk(), "abc");
        keys(&mut e, "de");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "abc");
        assert!(!e.dirty());
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "");
    }

    #[test]
    fn find_bar_takes_a_composition() {
        let (mut e, _host) = plain_with_host("caf\u{e9} cafe");
        keys(&mut e, "<D-f>caf");
        assert!(e.ime_active());
        e.handle(Input::Preedit {
            text: "e\u{301}".into(),
            cursor: 3,
        });
        assert_eq!(
            e.status().left,
            StatusLeft::Input {
                prefix: strings::FIND_PROMPT,
                text: "cafe\u{301}".into(),
                cursor: 6,
            }
        );
        assert_eq!(ViewSource::preedit(&e), None);
        e.handle(Input::Text("e".into()));
        assert_eq!(e.status().showcmd, "1 of 1");
        assert_eq!(e.cursor(), pos(0, 6));
        assert_eq!(e.text(), "caf\u{e9} cafe");
    }

    #[test]
    fn find_bar_counts_and_wraps() {
        let (mut e, _host) = plain_with_host("foo bar Foo\nfoo end");
        keys(&mut e, "<D-f>");
        let find = |e: &Editor| match e.status().left {
            StatusLeft::Input { prefix, text, .. } => Some(format!("{prefix}{text}")),
            _ => None,
        };
        assert_eq!(find(&e).as_deref(), Some("find: "));
        assert_eq!(e.status().showcmd, "0 of 0");
        assert_eq!(e.caret_shape(), CaretShape::Hidden);
        // Smartcase: all lowercase matches either case. The match at the
        // cursor counts.
        keys(&mut e, "foo");
        assert_eq!(find(&e).as_deref(), Some("find: foo"));
        assert_eq!(e.text(), "foo bar Foo\nfoo end");
        assert_eq!(e.status().showcmd, "1 of 3");
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "<CR>");
        assert_eq!(e.cursor(), pos(0, 8));
        assert_eq!(e.status().showcmd, "2 of 3");
        keys(&mut e, "<CR>");
        assert_eq!(e.cursor(), pos(1, 0));
        assert_eq!(e.status().showcmd, "3 of 3");
        // Past the end it wraps, and says so beside the counter.
        keys(&mut e, "<CR>");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.message(), Some(strings::SEARCH_WRAPPED));
        assert_eq!(e.status().showcmd, "search wrapped  1 of 3");
        assert_eq!(find(&e).as_deref(), Some("find: foo"));
        // Shift+Enter goes back, wrapping the other way.
        keys(&mut e, "<S-CR>");
        assert_eq!(e.cursor(), pos(1, 0));
        assert_eq!(e.status().showcmd, "search wrapped  3 of 3");
        keys(&mut e, "<S-CR>");
        assert_eq!(e.cursor(), pos(0, 8));
        assert_eq!(e.message(), None);
        assert_eq!(e.status().showcmd, "2 of 3");
        // An uppercase letter makes it case sensitive.
        keys(&mut e, "<BS><BS><BS>Foo");
        assert_eq!(e.status().showcmd, "1 of 1");
        assert_eq!(e.cursor(), pos(0, 8));
        keys(&mut e, "x");
        assert_eq!(e.status().showcmd, "0 of 0");
        keys(&mut e, "<BS>");
        // Esc closes it, leaving the cursor on the match and nothing typed.
        keys(&mut e, "<Esc>");
        assert_eq!(e.status().left, StatusLeft::Mode(""));
        assert_eq!(e.status().showcmd, "");
        assert_eq!(e.cursor(), pos(0, 8));
        assert_eq!(e.text(), "foo bar Foo\nfoo end");
        assert!(e.matches(0).is_empty());
        assert_eq!(e.caret_shape(), CaretShape::Bar);
        // Committed text types into the bar while it shows.
        keys(&mut e, "<D-f>");
        e.handle(Input::Text("end".into()));
        assert_eq!(find(&e).as_deref(), Some("find: end"));
        assert_eq!(e.cursor(), pos(1, 4));
        assert_eq!(e.text(), "foo bar Foo\nfoo end");
    }

    #[test]
    fn save_over_changed_prompts_o_r_c() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello\n").unwrap();
        let state = StateDir::at(dir.path().join("state"));
        let mut e = Editor::open_with_state(
            OpenRequest {
                path: path.clone(),
                view: false,
                display_name: None,
            },
            Config {
                vim: false,
                ..Config::default()
            },
            Platform::Mac,
            Box::new(FakeHost::default()),
            Some(&state),
        );
        e.discard_recovery();
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        let asks = |e: &Editor| e.prompt().as_deref() == Some(strings::CHANGED_ON_DISK_PROMPT);
        assert_eq!(
            strings::CHANGED_ON_DISK_PROMPT,
            "file changed on disk: [o]verwrite [r]eload [c]ancel"
        );

        // c (and Esc) cancel: nothing written, nothing lost.
        keys(&mut e, "<D-s>");
        assert!(asks(&e));
        assert_eq!(e.mode(), Mode::Prompt);
        // A modified key is no answer.
        keys(&mut e, "<D-o><D-r>");
        assert!(asks(&e));
        keys(&mut e, "c");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.text(), "xhello\n");
        assert!(e.dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere\n");
        keys(&mut e, "<D-s><Esc>");
        assert!(!asks(&e));
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere\n");

        // r reloads, as `:e!` does.
        keys(&mut e, "<D-s>r");
        assert_eq!(e.text(), "changed elsewhere\n");
        assert!(!e.dirty());
        assert_eq!(e.mode(), Mode::Insert);

        // o overwrites, as `:w!` does.
        keys(&mut e, "y");
        fs::write(&path, "third\n").unwrap();
        keys(&mut e, "<D-s>");
        assert!(asks(&e));
        keys(&mut e, "o");
        assert!(!e.dirty());
        assert_eq!(e.message(), Some(strings::WRITTEN));
        assert_eq!(fs::read_to_string(&path).unwrap(), e.text());

        // The watch's warning asks before trying to write at all.
        keys(&mut e, "z");
        e.disk_event(DiskEvent::Changed);
        keys(&mut e, "<D-s>");
        assert!(asks(&e));
        keys(&mut e, "c");
        assert!(e.dirty());
    }

    #[test]
    fn no_mode_label_in_status() {
        let (mut e, _host) = plain_with_host("ab");
        assert_eq!(e.status().left, StatusLeft::Mode(""));
        keys(&mut e, "x<S-Right>");
        assert_eq!(e.status().left, StatusLeft::Mode(""));
        // Vim still names its modes, and reader view names itself.
        assert_eq!(ed("ab").status().left, StatusLeft::Mode("NORMAL"));
        assert_eq!(
            build_reader("ab").status().left,
            StatusLeft::Mode(strings::MODE_READER)
        );
    }

    #[test]
    fn read_only_claims_no_insert() {
        // A file that is not valid UTF-8 opens read-only: typing does
        // nothing, the warning shows and nothing claims Insert.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.md");
        fs::write(&path, b"\xff\xfe bad").unwrap();
        let state = StateDir::at(dir.path().join("state"));
        let mut e = Editor::open_with_state(
            OpenRequest {
                path,
                view: false,
                display_name: None,
            },
            Config {
                vim: false,
                ..Config::default()
            },
            Platform::Mac,
            Box::new(FakeHost::default()),
            Some(&state),
        );
        keys(&mut e, "abc<CR><D-b><D-v>");
        e.handle(Input::Text("typed".into()));
        assert_eq!(e.text(), "");
        assert!(!e.dirty());
        let s = e.status();
        assert_eq!(s.left, StatusLeft::Mode(""));
        assert!(s.warnings.contains(&strings::NOT_UTF8_READ_ONLY));
        assert_eq!(e.caret_shape(), CaretShape::Block);
        assert!(!e.ime_active());
    }

    #[test]
    fn colon_types_a_colon() {
        let (mut e, _host) = plain_with_host("ab");
        keys(&mut e, ":w<CR>");
        assert_eq!(e.text(), ":w\nab");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.status().left, StatusLeft::Mode(""));
        assert_eq!(e.message(), None);
    }

    #[test]
    fn mac_moves_by_word_with_alt_and_by_line_with_cmd() {
        let (mut e, _host) = plain_with_host("one two_2, three\nnext line");
        keys(&mut e, "<M-Right>");
        assert_eq!(e.cursor(), pos(0, 3));
        keys(&mut e, "<M-Right>");
        assert_eq!(e.cursor(), pos(0, 9));
        keys(&mut e, "<M-Right><M-Right>");
        // Over the line break to the end of the next word.
        assert_eq!(e.cursor(), pos(1, 4));
        keys(&mut e, "<M-Left>");
        assert_eq!(e.cursor(), pos(1, 0));
        keys(&mut e, "<M-Left>");
        assert_eq!(e.cursor(), pos(0, 11));
        keys(&mut e, "<D-Left>");
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "<D-Right>");
        assert_eq!(e.cursor(), pos(0, 16));
        keys(&mut e, "<Home>");
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "<End>");
        assert_eq!(e.cursor(), pos(0, 16));
        keys(&mut e, "<D-Down>");
        assert_eq!(e.cursor(), pos(1, 9));
        keys(&mut e, "<D-Up>");
        assert_eq!(e.cursor(), pos(0, 0));
        // Ctrl is not the word key on the Mac.
        keys(&mut e, "<C-Right>");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.text(), "one two_2, three\nnext line");
    }

    #[test]
    fn linux_moves_by_word_with_ctrl() {
        let (mut e, _host) = plain_linux_with_host("one two\nnext");
        keys(&mut e, "<C-Right>");
        assert_eq!(e.cursor(), pos(0, 3));
        keys(&mut e, "<C-Right><C-Right>");
        assert_eq!(e.cursor(), pos(1, 4));
        keys(&mut e, "<C-Left>");
        assert_eq!(e.cursor(), pos(1, 0));
        keys(&mut e, "<Home>");
        assert_eq!(e.cursor(), pos(1, 0));
        keys(&mut e, "<End>");
        assert_eq!(e.cursor(), pos(1, 4));
        keys(&mut e, "<C-Home>");
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "<C-End>");
        assert_eq!(e.cursor(), pos(1, 4));
        // Alt is not the word key on Linux.
        keys(&mut e, "<C-Home><M-Right>");
        assert_eq!(e.cursor(), pos(0, 0));
        // Shift+Ctrl selects by word.
        keys(&mut e, "<S-C-Right>");
        assert_eq!(e.selection(), sel((0, 0), (0, 2)));
    }

    #[test]
    fn shift_arrows_select_and_typing_replaces() {
        let (mut e, _host) = plain_with_host("abcd\nef");
        keys(&mut e, "<Right><S-Right><S-Right>");
        assert_eq!(e.selection(), sel((0, 1), (0, 2)));
        // Back past the anchor flips it.
        keys(&mut e, "<S-Left><S-Left><S-Left>");
        assert_eq!(e.selection(), sel((0, 0), (0, 0)));
        keys(&mut e, "<S-Right>");
        assert_eq!(e.selection(), None);
        // Across a line break.
        keys(&mut e, "<S-Down>");
        assert_eq!(e.selection(), sel((0, 1), (1, 0)));
        keys(&mut e, "X");
        assert_eq!(e.text(), "aXf");
        assert_eq!(e.selection(), None);
        assert_eq!(e.cursor(), pos(0, 2));
        // Replacing is its own step, then the typing after joins it.
        keys(&mut e, "Y");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "abcd\nef");
        // Left and Right with a selection go to its ends.
        keys(&mut e, "<D-Up><Right><S-Right><S-Right><Left>");
        assert_eq!(e.cursor(), pos(0, 1));
        assert_eq!(e.selection(), None);
        keys(&mut e, "<S-Right><S-Right><Right>");
        assert_eq!(e.cursor(), pos(0, 3));
        // Backspace and Delete take a selection; Esc drops one.
        keys(&mut e, "<S-Left><S-Left><BS>");
        assert_eq!(e.text(), "ad\nef");
        keys(&mut e, "<S-Right><Esc>");
        assert_eq!(e.selection(), None);
        assert_eq!(e.text(), "ad\nef");
        keys(&mut e, "<S-Left><Del>");
        assert_eq!(e.text(), "a\nef");
        // Enter replaces it too, continuing as Enter does.
        keys(&mut e, "<Home><S-End><CR>");
        assert_eq!(e.text(), "\n\nef");
    }

    #[test]
    fn clipboard_keys_use_the_selection() {
        let (mut e, host) = plain_with_host("one two");
        keys(&mut e, "<S-M-Right><D-c>");
        assert_eq!(host.clipboard(), "one");
        assert_eq!(e.text(), "one two");
        keys(&mut e, "<D-x>");
        assert_eq!(host.clipboard(), "one");
        assert_eq!(e.text(), " two");
        assert_eq!(e.selection(), None);
        keys(&mut e, "<End><S-M-Left><D-v>");
        assert_eq!(e.text(), " one");
        assert_eq!(e.cursor(), pos(0, 4));
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), " two");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "one two");
    }

    #[test]
    fn a_paste_past_the_budgets_is_refused() {
        use crate::vim::operator::MAX_PUT_LINES;
        let (mut e, host) = plain_with_host("one two");
        host.set_clipboard(&"\n".repeat(MAX_PUT_LINES + 1));
        keys(&mut e, "<D-v>");
        assert_eq!(e.text(), "one two");
        assert!(!e.history().can_undo());
        assert_eq!(e.message(), Some(strings::RESULTING_TEXT_TOO_LONG));
        // Over a selection, only what it adds beyond the selection counts.
        host.set_clipboard("1");
        keys(&mut e, "<S-M-Right><D-v>");
        assert_eq!(e.text(), "1 two");
    }

    #[test]
    fn primary_k_and_l_link_the_selection() {
        let (mut e, _host) = plain_with_host("see docs here");
        keys(&mut e, "<M-Right><Right><S-M-Right><D-k>");
        assert_eq!(e.mode(), Mode::Command);
        keys(&mut e, "u<CR>");
        assert_eq!(e.text(), "see [docs](u) here");
        // Still typing, after the link.
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(0, 13));
        assert_eq!(e.selection(), None);
        keys(&mut e, "<D-l>v<CR>");
        assert_eq!(e.text(), "see [docs](u)[](v) here");
        assert_eq!(e.cursor(), pos(0, 14));
    }

    #[test]
    fn undo_at_a_line_end_leaves_the_cursor_there() {
        // Vim's Normal-mode rest would put it one grapheme short.
        let (mut e, _host) = plain_with_host("hello");
        keys(&mut e, "<End> world<D-z>");
        assert_eq!(e.text(), "hello");
        assert_eq!(e.cursor(), pos(0, 5));
        keys(&mut e, "!");
        assert_eq!(e.text(), "hello!");
    }

    #[test]
    fn redo_puts_the_cursor_after_the_redone_text() {
        let (mut e, _host) = plain_with_host("hello");
        keys(&mut e, "<End> world<D-z><D-S-z>");
        assert_eq!(e.text(), "hello world");
        assert_eq!(e.cursor(), pos(0, 11));
        keys(&mut e, "!");
        assert_eq!(e.text(), "hello world!");
        // A run of Backspaces redone leaves the cursor where they ended.
        keys(&mut e, "<BS><BS><D-z><D-S-z>");
        assert_eq!(e.text(), "hello worl");
        assert_eq!(e.cursor(), pos(0, 10));
    }

    /// A vim-off editor opened on the real file `name` holding `text` in a
    /// temporary directory (the guard), with its state directory there
    /// too and no recovery copy, and a handle on its host's clock.
    fn open_plain(
        text: &str,
        autosave: bool,
    ) -> (Editor, FakeHost, std::path::PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, text).unwrap();
        let state = StateDir::at(dir.path().join("state"));
        let host = FakeHost::default();
        let mut e = Editor::open_with_state(
            OpenRequest {
                path: path.clone(),
                view: false,
                display_name: None,
            },
            Config {
                vim: false,
                autosave,
                ..Config::default()
            },
            Platform::Mac,
            Box::new(host.clone()),
            Some(&state),
        );
        e.discard_recovery();
        (e, host, path, dir)
    }

    #[test]
    fn close_save_over_changed_asks_o_r_c() {
        let asks = |e: &Editor| e.prompt().as_deref() == Some(strings::CHANGED_ON_DISK_PROMPT);
        // o writes over it, then the close goes ahead.
        let (mut e, _host, path, _dir) = open_plain("hello\n", false);
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.handle(Input::CloseRequested);
        assert_eq!(e.prompt().as_deref(), Some(strings::UNSAVED_CHANGES_PROMPT));
        let fx = keys(&mut e, "s");
        assert_eq!(fx.close, None);
        assert!(asks(&e));
        assert_ne!(e.message(), Some(strings::CHANGED_ON_DISK_REFUSED));
        let fx = keys(&mut e, "o");
        assert_eq!(fx.close, Some(0));
        assert_eq!(fs::read_to_string(&path).unwrap(), "xhello\n");

        // r reloads and stays open.
        let (mut e, _host, path, _dir) = open_plain("hello\n", false);
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.handle(Input::CloseRequested);
        keys(&mut e, "s");
        let fx = keys(&mut e, "r");
        assert_eq!(fx.close, None);
        assert_eq!(e.text(), "changed elsewhere\n");
        assert!(!e.dirty());
        assert_eq!(e.mode(), Mode::Insert);

        // c cancels the close.
        let (mut e, _host, path, _dir) = open_plain("hello\n", false);
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.handle(Input::CloseRequested);
        keys(&mut e, "s");
        let fx = keys(&mut e, "c");
        assert_eq!(fx.close, None);
        assert_eq!(e.prompt(), None);
        assert_eq!(e.mode(), Mode::Insert);
        assert!(e.dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere\n");
    }

    #[test]
    fn autosave_over_changed_says_so_without_vim_commands() {
        let (mut e, host, path, _dir) = open_plain("hello\n", true);
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        host.state().now = 5_000_000_000;
        e.handle(Input::Tick {
            now_ns: 5_000_000_000,
        });
        assert_eq!(e.prompt(), None);
        assert_eq!(e.message(), Some(strings::CHANGED_ON_DISK_AUTOSAVE));
        assert!(!strings::CHANGED_ON_DISK_AUTOSAVE.contains(':'));
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere\n");
    }

    #[test]
    fn a_table_key_over_a_selection_only_moves() {
        let t = "| h1 | h2 |\n| --- | --- |\n| a | b |";
        let (mut e, _host) = plain_with_host(t);
        keys(&mut e, "<Down><Down><Right><Right><S-Right><Tab>");
        assert_eq!(e.text(), t);
        assert_eq!(e.cursor(), pos(2, 6));
        assert_eq!(e.selection(), None);
        assert_eq!(e.history().undo_len(), 0);
    }

    #[test]
    fn reader_view_drops_the_selection() {
        let (mut e, _host) = plain_with_host("abc");
        keys(&mut e, "<S-Right><D-S-r>");
        assert!(e.reader());
        assert_eq!(e.selection(), None);
        keys(&mut e, "<D-S-r>");
        assert!(!e.reader());
        assert_eq!(e.selection(), None);
        keys(&mut e, "x");
        assert_eq!(e.text(), "axbc");
    }
}

//! Test helpers shared across the crate: a recording host, editors built
//! from text, a key-sequence driver and a position shorthand.

use std::cell::{RefCell, RefMut};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::buffer::Pos;
use crate::editor::{Config, Editor, OpenRequest};
use crate::effects::Effects;
use crate::host::{Host, Waker, WindowRequest};
use crate::input::{Input, Key, KeyEvent, Mods};
use crate::platform::Platform;
use crate::saver::Saver;

pub(crate) use crate::view_model::ViewSource;

/// What a `FakeHost` has been asked, shared between its clones so a test
/// can watch it after the editor owns the host.
#[derive(Default)]
pub(crate) struct HostState {
    pub clipboard: Option<String>,
    pub opened: Vec<String>,
    pub spawned: Vec<WindowRequest>,
    pub now: u64,
}

/// A host that records what it is asked. Clones share one `HostState`.
#[derive(Default, Clone)]
pub(crate) struct FakeHost {
    state: Rc<RefCell<HostState>>,
}

impl FakeHost {
    pub(crate) fn state(&self) -> RefMut<'_, HostState> {
        self.state.borrow_mut()
    }

    /// The clipboard's text, "" when empty.
    pub(crate) fn clipboard(&self) -> String {
        self.state.borrow().clipboard.clone().unwrap_or_default()
    }

    /// Puts `text` on the clipboard, as another app copying would.
    pub(crate) fn set_clipboard(&self, text: &str) {
        self.state().clipboard = Some(text.to_owned());
    }
}

impl Host for FakeHost {
    fn clipboard_get(&mut self) -> Option<String> {
        self.state.borrow().clipboard.clone()
    }
    fn clipboard_set(&mut self, text: &str) {
        self.state().clipboard = Some(text.to_owned());
    }
    fn open_external(&mut self, target: &str) {
        self.state().opened.push(target.to_owned());
    }
    fn spawn_window(&mut self, req: WindowRequest) {
        self.state().spawned.push(req);
    }
    fn now_ns(&self) -> u64 {
        self.state.borrow().now
    }
    fn waker(&self) -> Waker {
        Arc::new(|| {})
    }
}

pub(crate) fn req(path: &str) -> OpenRequest {
    OpenRequest {
        path: PathBuf::from(path),
        view: false,
        display_name: None,
    }
}

/// An editor on `text` (named `notes.md`) with the given config.
pub(crate) fn build(text: &str, cfg: Config, bench: bool) -> Editor {
    build_with_host(text, cfg, bench).0
}

fn build_with_host(text: &str, cfg: Config, bench: bool) -> (Editor, FakeHost) {
    build_on(text, cfg, Platform::Mac, bench)
}

fn build_on(text: &str, cfg: Config, platform: Platform, bench: bool) -> (Editor, FakeHost) {
    let host = FakeHost::default();
    let e = Editor::build(
        req("notes.md"),
        text,
        None,
        cfg,
        platform,
        Box::new(host.clone()),
        bench,
    );
    (e, host)
}

/// A vim editor on `text`, in Normal mode at the document start.
pub(crate) fn ed(text: &str) -> Editor {
    build(text, Config::default(), false)
}

/// A vim editor on `text` in reader mode.
pub(crate) fn build_reader(text: &str) -> Editor {
    Editor::build(
        OpenRequest {
            view: true,
            ..req("notes.md")
        },
        text,
        None,
        Config::default(),
        Platform::Mac,
        Box::new(FakeHost::default()),
        false,
    )
}

/// Like `ed`, with a handle on the editor's host.
pub(crate) fn ed_with_host(text: &str) -> (Editor, FakeHost) {
    build_with_host(text, Config::default(), false)
}

/// A call the editor made to its `Saver`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileCall {
    Write { force: bool },
    Reload,
    Close { code: i32 },
}

#[derive(Default)]
struct FilesState {
    calls: Vec<FileCall>,
    fail_writes: bool,
    /// The text on disk: what a write left, or what a reload reads.
    disk: String,
}

/// A `Saver` double that records the calls and keeps the text "on disk".
/// Writes succeed until `fail_writes`. Clones share one record.
#[derive(Default, Clone)]
pub(crate) struct FakeFiles {
    state: Rc<RefCell<FilesState>>,
}

impl FakeFiles {
    pub(crate) fn calls(&self) -> Vec<FileCall> {
        self.state.borrow().calls.clone()
    }

    /// Makes every write from now on fail, as a full disk does.
    pub(crate) fn fail_writes(&self) {
        self.state.borrow_mut().fail_writes = true;
    }

    /// The text on disk.
    pub(crate) fn disk(&self) -> String {
        self.state.borrow().disk.clone()
    }

    /// Changes the text on disk, as another program would.
    pub(crate) fn set_disk(&self, text: &str) {
        self.state.borrow_mut().disk = text.to_owned();
    }
}

impl Saver for FakeFiles {
    fn write(&mut self, text: &str, force: bool) -> std::io::Result<()> {
        let mut s = self.state.borrow_mut();
        s.calls.push(FileCall::Write { force });
        if s.fail_writes {
            return Err(std::io::Error::other("No space left on device"));
        }
        s.disk = text.to_owned();
        Ok(())
    }
    fn reload(&mut self) -> std::io::Result<String> {
        let mut s = self.state.borrow_mut();
        s.calls.push(FileCall::Reload);
        Ok(s.disk.clone())
    }
    fn close(&mut self, code: i32) {
        self.state.borrow_mut().calls.push(FileCall::Close { code });
    }
}

/// Like `ed`, with a handle on the calls it makes to its file, which holds
/// `text` to begin with.
pub(crate) fn ed_with_files(text: &str) -> (Editor, FakeFiles) {
    let mut e = ed(text);
    let files = FakeFiles::default();
    files.set_disk(text);
    e.set_saver(Box::new(files.clone()));
    (e, files)
}

/// Like `ed_with_host`, on Linux, where Ctrl is the primary modifier.
pub(crate) fn ed_linux_with_host(text: &str) -> (Editor, FakeHost) {
    build_on(text, Config::default(), Platform::Linux, false)
}

/// A vim-off editor on `text` (Insert only), with a handle on its host.
pub(crate) fn plain_with_host(text: &str) -> (Editor, FakeHost) {
    build_with_host(text, Config { vim: false }, false)
}

pub(crate) fn pos(line: usize, col: usize) -> Pos {
    Pos { line, col }
}

pub(crate) fn key_with(key: Key, mods: Mods) -> Input {
    Input::Key(KeyEvent {
        key,
        mods,
        time_ns: 0,
    })
}

pub(crate) fn key(k: Key) -> Input {
    key_with(k, Mods::default())
}

/// The key a `<name>` escape stands for: `Esc`, `CR`, `BS`, `Tab`, `Del`,
/// `Space`, `lt`, the arrows, `Home`, `End`, `C-x` for Ctrl and a char, or
/// `D-x` for Cmd and a char (the primary modifier on the Mac, which the
/// test editors run as).
fn named(name: &str) -> Option<(Key, Mods)> {
    let plain = |k| Some((k, Mods::default()));
    match name {
        "Esc" => plain(Key::Esc),
        "CR" => plain(Key::Enter),
        "BS" => plain(Key::Backspace),
        "Tab" => plain(Key::Tab),
        "Del" => plain(Key::Delete),
        "Space" => plain(Key::Space),
        "lt" => plain(Key::Char('<')),
        "Left" => plain(Key::Left),
        "Right" => plain(Key::Right),
        "Up" => plain(Key::Up),
        "Down" => plain(Key::Down),
        "Home" => plain(Key::Home),
        "End" => plain(Key::End),
        _ => {
            let (mods, rest) = if let Some(rest) = name.strip_prefix("C-") {
                (
                    Mods {
                        ctrl: true,
                        ..Mods::default()
                    },
                    rest,
                )
            } else {
                (
                    Mods {
                        cmd: true,
                        ..Mods::default()
                    },
                    name.strip_prefix("D-")?,
                )
            };
            let mut rest = rest.chars();
            let c = rest.next()?;
            rest.next().is_none().then_some((Key::Char(c), mods))
        }
    }
}

/// Presses each key in `seq`. `<Esc>`-style escapes name special keys (see
/// `named`); a `<` that starts no known escape is a literal `<`.
pub(crate) fn keys(e: &mut Editor, seq: &str) -> Effects {
    let mut fx = Effects::default();
    let mut rest = seq;
    while let Some(c) = rest.chars().next() {
        let escape = c
            .eq(&'<')
            .then(|| rest.find('>'))
            .flatten()
            .and_then(|end| named(&rest[1..end]).map(|k| (k, end + 1)));
        let ((k, mods), used) = match escape {
            Some(found) => found,
            None if c == ' ' => ((Key::Space, Mods::default()), 1),
            None => ((Key::Char(c), Mods::default()), c.len_utf8()),
        };
        fx = fx.merge(e.handle(key_with(k, mods)));
        rest = &rest[used..];
    }
    fx
}

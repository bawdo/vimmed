//! Test helpers shared across the crate: a recording host, editors built
//! from text, a key-sequence driver and a position shorthand.

use std::cell::{RefCell, RefMut};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::buffer::{Buffer, Pos};
use crate::editor::{Config, Editor, OpenRequest};
use crate::effects::Effects;
use crate::file::{DiskStat, Loaded, StateDir, WriteError};
use crate::host::{Host, Waker, WindowRequest};
use crate::input::{Input, Key, KeyEvent, Mods};
use crate::keyseq::parse_keys;
use crate::platform::Platform;
use crate::saver::Saver;

pub(crate) use crate::view_model::ViewSource;

/// Held by the tests that build a document at the normal-build budgets
/// (hundreds of MiB each), so at most one such document exists at a time
/// while the test threads run in parallel.
static BIG_DOC: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Takes the big-document lock, recovering it if a failed test poisoned it.
pub(crate) fn big_doc_lock() -> std::sync::MutexGuard<'static, ()> {
    BIG_DOC
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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

/// A vim editor on `text` for the file at `path` (which need not exist;
/// nothing is read), with a handle on its host. `view` opens it in reader
/// mode.
pub(crate) fn ed_at(path: &std::path::Path, text: &str, view: bool) -> (Editor, FakeHost) {
    let host = FakeHost::default();
    let e = Editor::build(
        OpenRequest {
            path: path.to_owned(),
            view,
            display_name: None,
        },
        text,
        None,
        Config::default(),
        Platform::Mac,
        Box::new(host.clone()),
        false,
    );
    (e, host)
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

/// Like `ed_with_host`, with the named registers shared through `state`
/// (a temporary directory). Editors built any other way keep their named
/// registers in memory, so no test reaches the real state directory.
pub(crate) fn ed_with_state(text: &str, state: &StateDir) -> (Editor, FakeHost) {
    let (mut e, host) = build_with_host(text, Config::default(), false);
    e.share_registers(state);
    (e, host)
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
    fn write(&mut self, text: &str, force: bool) -> Result<(), WriteError> {
        let mut s = self.state.borrow_mut();
        s.calls.push(FileCall::Write { force });
        if s.fail_writes {
            let full = std::io::Error::other("No space left on device");
            return Err(WriteError::Io(full));
        }
        s.disk = text.to_owned();
        Ok(())
    }
    fn reload(&mut self) -> Loaded {
        let mut s = self.state.borrow_mut();
        s.calls.push(FileCall::Reload);
        Loaded::Text {
            buffer: Buffer::from_text(&s.disk),
            stat: DiskStat::default(),
        }
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
    build_with_host(
        text,
        Config {
            vim: false,
            ..Config::default()
        },
        false,
    )
}

/// Like `plain_with_host`, on Linux, where Ctrl is the primary modifier.
pub(crate) fn plain_linux_with_host(text: &str) -> (Editor, FakeHost) {
    build_on(
        text,
        Config {
            vim: false,
            ..Config::default()
        },
        Platform::Linux,
        false,
    )
}

/// Like `plain_with_host`, with a handle on the calls it makes to its
/// file, which holds `text` to begin with.
pub(crate) fn plain_with_files(text: &str) -> (Editor, FakeFiles) {
    let (mut e, _host) = plain_with_host(text);
    let files = FakeFiles::default();
    files.set_disk(text);
    e.set_saver(Box::new(files.clone()));
    (e, files)
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

/// Presses each key in `seq`. `<Esc>`-style escapes name special keys (see
/// `keyseq::parse_keys`); a `<` that starts no known escape is a literal `<`.
pub(crate) fn keys(e: &mut Editor, seq: &str) -> Effects {
    let mut fx = Effects::default();
    for (k, mods) in parse_keys(seq) {
        fx = fx.merge(e.handle(key_with(k, mods)));
    }
    fx
}

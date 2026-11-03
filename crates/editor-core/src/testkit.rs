//! Test helpers shared across the crate: a recording host, editors built
//! from text, a key-sequence driver and a position shorthand.

use std::path::PathBuf;
use std::sync::Arc;

use crate::buffer::Pos;
use crate::editor::{Config, Editor, OpenRequest};
use crate::effects::Effects;
use crate::host::{Host, Waker, WindowRequest};
use crate::input::{Input, Key, KeyEvent, Mods};
use crate::platform::Platform;

pub(crate) use crate::view_model::ViewSource;

/// A host that records what it is asked.
#[derive(Default)]
pub(crate) struct FakeHost {
    pub clipboard: Option<String>,
    pub opened: Vec<String>,
    pub spawned: Vec<WindowRequest>,
    pub now: u64,
}

impl Host for FakeHost {
    fn clipboard_get(&mut self) -> Option<String> {
        self.clipboard.clone()
    }
    fn clipboard_set(&mut self, text: &str) {
        self.clipboard = Some(text.to_owned());
    }
    fn open_external(&mut self, target: &str) {
        self.opened.push(target.to_owned());
    }
    fn spawn_window(&mut self, req: WindowRequest) {
        self.spawned.push(req);
    }
    fn now_ns(&self) -> u64 {
        self.now
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
    Editor::build(
        req("notes.md"),
        text,
        None,
        cfg,
        Platform::Mac,
        Box::new(FakeHost::default()),
        bench,
    )
}

/// A vim editor on `text`, in Normal mode at the document start.
pub(crate) fn ed(text: &str) -> Editor {
    build(text, Config::default(), false)
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
/// `Space`, `lt`, the arrows, `Home`, `End`, or `C-x` for Ctrl and a char.
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
            let mut rest = name.strip_prefix("C-")?.chars();
            let c = rest.next()?;
            rest.next().is_none().then_some((
                Key::Char(c),
                Mods {
                    ctrl: true,
                    ..Mods::default()
                },
            ))
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
        let out = e.handle(key_with(k, mods));
        fx.redraw = std::mem::take(&mut fx.redraw).merge(out.redraw);
        fx.dirty = fx.dirty.or(out.dirty);
        fx.scroll_to_cursor |= out.scroll_to_cursor;
        rest = &rest[used..];
    }
    fx
}

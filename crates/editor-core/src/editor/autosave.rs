//! Autosave (`Config::autosave`, off by default): `AUTOSAVE_IDLE_NS` after
//! the last change, the document is written down the same path as `:w`
//! (the `Saver`'s changed-on-disk check and typed errors, the save point
//! undo comes back to, the file watch rebased on the version written),
//! without the `written` message. A refused or failed autosave says why,
//! once, and is not tried again until the next change.
//!
//! The wait is the shell's timer: a change asks to be woken
//! (`Effects::wake_at`) when it falls due, and the tick then writes.
//!
//! An autosave is held, saying nothing, while the document cannot or
//! should not be written: reader view, read-only from the already-open
//! prompt, a file that was not valid UTF-8 or could not be read, an
//! unnamed buffer, a warning from the file watch (a file gone, changed,
//! read-only or unreadable on disk: writing over or back to it is `:w`'s
//! choice, never autosave's), a prompt asking, a `:s///c` session (one
//! undo step) and a vim Insert session (also one undo step, and one `.`
//! change: a save point inside it would split the step). Once the hold
//! lifts, the write falls due again, at once if its time has passed.

use crate::buffer::ShiftMark;
use crate::effects::Effects;
use crate::status::Mode;

use super::Editor;

/// How long after the last change autosave writes.
pub const AUTOSAVE_IDLE_NS: u64 = 1_000_000_000;

/// Where autosave stood before an input, for `autosave_after`.
pub(super) struct Before {
    /// The buffer's edit history: any edit since moves it.
    edits: ShiftMark,
    held: bool,
}

impl Editor {
    /// Takes note of where autosave stands before an input.
    pub(super) fn autosave_before(&self) -> Before {
        Before {
            edits: self.buf.shift_mark(),
            held: self.autosave_held(),
        }
    }

    /// After an input: a change (one that left the document dirty) sets
    /// the write due `AUTOSAVE_IDLE_NS` from now, and a clean document
    /// has nothing due. Asks to be woken when the write first falls due,
    /// and again when a hold lifts; a later change only moves the time,
    /// which the tick it woke for finds.
    pub(super) fn autosave_after(&mut self, before: &Before) -> Effects {
        if !self.cfg.autosave {
            return Effects::default();
        }
        if !self.dirty {
            self.autosave_due = None;
            return Effects::default();
        }
        let first = self.autosave_due.is_none();
        if self.buf.shift_mark() != before.edits {
            let due = self.host.now_ns().saturating_add(AUTOSAVE_IDLE_NS);
            self.autosave_due = Some(due);
        }
        let Some(due) = self.autosave_due else {
            return Effects::default();
        };
        let lifted = before.held && !self.autosave_held();
        if (first || lifted) && !self.autosave_held() {
            return Effects {
                wake_at: Some(due),
                ..Effects::default()
            };
        }
        Effects::default()
    }

    /// The main loop woke at `now_ns`: writes the document if its write is
    /// due and nothing holds it. One not due yet asks to be woken then.
    /// Written or not, nothing more is due until the next change.
    pub(super) fn autosave_tick(&mut self, now_ns: u64) -> Effects {
        let Some(due) = self.autosave_due else {
            return Effects::default();
        };
        if !self.dirty {
            self.autosave_due = None;
            return Effects::default();
        }
        if self.autosave_held() {
            return Effects::default();
        }
        if now_ns < due {
            return Effects {
                wake_at: Some(due),
                ..Effects::default()
            };
        }
        self.autosave_due = None;
        // The dirty change it makes, and a refusal's message, reach the
        // shell through `handle`.
        self.write_quietly(false);
        Effects::default()
    }

    /// True while autosave must not write (see the module's notes).
    /// Closing drops the waiting write instead (`close`).
    fn autosave_held(&self) -> bool {
        self.refuses_edits()
            || self.disk.is_some()
            || self.saver.refuses_writes()
            || self.path.as_os_str().is_empty()
            || self.asking.is_some()
            || matches!(self.mode, Mode::Prompt | Mode::Confirm)
            || (self.cfg.vim && self.mode == Mode::Insert)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::AUTOSAVE_IDLE_NS;
    use crate::editor::{Config, Editor, OpenRequest};
    use crate::effects::Effects;
    use crate::file::{DiskEvent, StateDir};
    use crate::input::Input;
    use crate::platform::Platform;
    use crate::status::{self, Mode};
    use crate::strings;
    use crate::testkit::{FakeFiles, FakeHost, FileCall, ViewSource, ed_with_files, keys, req};
    use crate::view_model::{MsgKind, StatusLeft};

    /// The clock's reading when each test starts.
    const START: u64 = 1_000;

    fn autosave_on() -> Config {
        Config {
            autosave: true,
            ..Config::default()
        }
    }

    /// An autosaving editor on `text` for `path`, whose file (a
    /// `FakeFiles`) holds `text`, with a handle on its host's clock.
    fn autosaving_at(path: &str, text: &str) -> (Editor, FakeHost, FakeFiles) {
        built_at(path, text, autosave_on())
    }

    /// `autosaving_at` with the settings `cfg`.
    fn built_at(path: &str, text: &str, cfg: Config) -> (Editor, FakeHost, FakeFiles) {
        let host = FakeHost::default();
        host.state().now = START;
        let mut e = Editor::build(
            req(path),
            text,
            None,
            cfg,
            Platform::Mac,
            Box::new(host.clone()),
            false,
        );
        let files = FakeFiles::default();
        files.set_disk(text);
        e.set_saver(Box::new(files.clone()));
        (e, host, files)
    }

    fn autosaving(text: &str) -> (Editor, FakeHost, FakeFiles) {
        autosaving_at("notes.md", text)
    }

    /// An autosaving editor opened on the real file at `path`, with its
    /// lock record in a temporary state directory (removed when the guard
    /// drops). It keeps no recovery copy, whose own wakes these tests do
    /// not watch (`recovering` has autosave's part in that).
    fn open_autosaving(path: &Path) -> (Editor, FakeHost, tempfile::TempDir) {
        let host = FakeHost::default();
        host.state().now = START;
        let r = OpenRequest {
            path: path.to_owned(),
            view: false,
            display_name: None,
        };
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path().join("state"));
        let mut e = Editor::open_with_state(
            r,
            autosave_on(),
            Platform::Mac,
            Box::new(host.clone()),
            Some(&state),
        );
        e.discard_recovery();
        (e, host, tmp)
    }

    /// Sets the clock to `at` and ticks, as the shell's timer does.
    fn tick(e: &mut Editor, host: &FakeHost, at: u64) -> Effects {
        host.state().now = at;
        e.handle(Input::Tick { now_ns: at })
    }

    fn writes(files: &FakeFiles) -> usize {
        files
            .calls()
            .iter()
            .filter(|c| matches!(c, FileCall::Write { .. }))
            .count()
    }

    fn error(e: &Editor) -> Option<String> {
        match e.status().left {
            StatusLeft::Message {
                text,
                kind: MsgKind::Error,
            } => Some(text),
            _ => None,
        }
    }

    #[test]
    fn dirty_transitions_emit_effects_once() {
        let (mut e, files) = ed_with_files("hello");
        assert_eq!(keys(&mut e, "x").dirty, Some(true));
        assert_eq!(keys(&mut e, "x").dirty, None);
        assert_eq!(keys(&mut e, ":w<CR>").dirty, Some(false));
        // A write of a clean document is no transition.
        assert_eq!(keys(&mut e, ":w<CR>").dirty, None);
        assert_eq!(keys(&mut e, "x").dirty, Some(true));
        // Undo back to the save point is clean again; redo leaves it.
        assert_eq!(keys(&mut e, "u").dirty, Some(false));
        assert_eq!(keys(&mut e, "<C-r>").dirty, Some(true));
        // `:e!` drops the changes: clean, once.
        assert_eq!(keys(&mut e, ":e!<CR>").dirty, Some(false));
        assert_eq!(keys(&mut e, ":e!<CR>").dirty, None);
        // A quiet reload of a clean document with new text on disk stays
        // clean: nothing to report, though the text was replaced.
        files.set_disk("from elsewhere");
        e.disk = Some(DiskEvent::Changed);
        let fx = keys(&mut e, "l");
        assert_eq!(e.text(), "from elsewhere");
        assert_eq!(fx.dirty, None);
        // Undo back to the save point with a change waiting on disk: the
        // undo and the reload it lets through report clean once.
        assert_eq!(keys(&mut e, "x").dirty, Some(true));
        files.set_disk("again");
        e.disk = Some(DiskEvent::Changed);
        let fx = keys(&mut e, "u");
        assert_eq!(e.text(), "again");
        assert_eq!(fx.dirty, Some(false));
        assert!(!e.dirty());
    }

    #[test]
    fn the_title_takes_a_bullet_while_dirty() {
        assert_eq!(strings::window_title("notes.md", false), "notes.md");
        assert_eq!(strings::window_title("notes.md", true), "\u{2022} notes.md");
    }

    #[test]
    fn autosave_off_by_default() {
        assert!(!Config::default().autosave);
        let (mut e, host, files) = built_at("notes.md", "hello", Config::default());
        let fx = keys(&mut e, "x");
        assert_eq!(fx.wake_at, None);
        tick(&mut e, &host, START + 10 * AUTOSAVE_IDLE_NS);
        assert_eq!(writes(&files), 0);
        assert!(e.dirty());
    }

    #[test]
    fn autosave_writes_after_1s_idle() {
        assert_eq!(AUTOSAVE_IDLE_NS, 1_000_000_000);
        let (mut e, host, files) = autosaving("hello");
        let fx = keys(&mut e, "x");
        let first = START + AUTOSAVE_IDLE_NS;
        assert_eq!(fx.wake_at, Some(first));
        // Another change half a second on pushes the write back.
        host.state().now = START + AUTOSAVE_IDLE_NS / 2;
        keys(&mut e, "x");
        let due = START + AUTOSAVE_IDLE_NS / 2 + AUTOSAVE_IDLE_NS;
        let fx = tick(&mut e, &host, first);
        assert_eq!(writes(&files), 0);
        assert_eq!(fx.wake_at, Some(due));
        // Not yet, by a nanosecond.
        tick(&mut e, &host, due - 1);
        assert_eq!(writes(&files), 0);
        let fx = tick(&mut e, &host, due);
        assert_eq!(files.calls(), [FileCall::Write { force: false }]);
        assert_eq!(files.disk(), "llo");
        assert!(!e.dirty());
        assert_eq!(fx.dirty, Some(false));
        // Quietly: no `written`.
        assert_eq!(e.message(), None);
        // The save point is set: undo past it is dirty, redo is clean.
        keys(&mut e, "u");
        assert!(e.dirty());
        keys(&mut e, "<C-r>");
        assert!(!e.dirty());
        // Nothing more to write until the next change.
        tick(&mut e, &host, due + 10 * AUTOSAVE_IDLE_NS);
        assert_eq!(writes(&files), 1);
    }

    #[test]
    fn autosave_respects_changed_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello\n").unwrap();
        let (mut e, host, _state) = open_autosaving(&path);
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        let due = START + AUTOSAVE_IDLE_NS;
        let fx = tick(&mut e, &host, due);
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere\n");
        let refused = Some(strings::CHANGED_ON_DISK_REFUSED);
        assert_eq!(error(&e).as_deref(), refused);
        assert!(e.dirty());
        // The refusal fades like any other message, and is not tried
        // again on later ticks.
        let faded = due + status::MESSAGE_FOR_NS;
        assert_eq!(fx.wake_at, Some(faded));
        tick(&mut e, &host, faded);
        assert_eq!(e.message(), None);
        let fx = tick(&mut e, &host, faded + 10 * AUTOSAVE_IDLE_NS);
        assert_eq!(e.message(), None);
        assert_eq!(fx.wake_at, None);
        // The next change tries again, and is refused again.
        let now = faded + 20 * AUTOSAVE_IDLE_NS;
        host.state().now = now;
        keys(&mut e, "x");
        tick(&mut e, &host, now + AUTOSAVE_IDLE_NS);
        assert_eq!(error(&e).as_deref(), refused);
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere\n");
        // `:w!` is still the way through.
        keys(&mut e, ":w!<CR>");
        assert_eq!(fs::read_to_string(&path).unwrap(), "llo\n");
    }

    #[test]
    fn a_failed_autosave_waits_for_the_next_change() {
        let (mut e, host, files) = autosaving("hello");
        files.fail_writes();
        keys(&mut e, "x");
        let due = START + AUTOSAVE_IDLE_NS;
        tick(&mut e, &host, due);
        assert_eq!(writes(&files), 1);
        assert!(error(&e).is_some_and(|m| m.starts_with("write failed: ")));
        tick(&mut e, &host, due + 10 * AUTOSAVE_IDLE_NS);
        tick(&mut e, &host, due + 20 * AUTOSAVE_IDLE_NS);
        assert_eq!(writes(&files), 1);
        let now = due + 30 * AUTOSAVE_IDLE_NS;
        host.state().now = now;
        keys(&mut e, "x");
        tick(&mut e, &host, now + AUTOSAVE_IDLE_NS);
        assert_eq!(writes(&files), 2);
    }

    #[test]
    fn autosave_waits_for_the_insert_session_to_end() {
        let (mut e, host, files) = autosaving("");
        keys(&mut e, "iab");
        let due = START + AUTOSAVE_IDLE_NS;
        let fx = tick(&mut e, &host, due + AUTOSAVE_IDLE_NS);
        assert_eq!(writes(&files), 0);
        assert_eq!(fx.wake_at, None);
        assert_eq!(e.message(), None);
        // Esc ends the session: the write is overdue, so it asks to be
        // woken at once, and the tick writes.
        let fx = keys(&mut e, "<Esc>");
        assert_eq!(fx.wake_at, Some(due));
        tick(&mut e, &host, due + 2 * AUTOSAVE_IDLE_NS);
        assert_eq!(files.disk(), "ab");
        assert!(!e.dirty());
        // The session is still one undo step, and one `.` change.
        keys(&mut e, "u");
        assert_eq!(e.text(), "");
        keys(&mut e, ".");
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn autosave_without_vim_writes_while_typing_pauses() {
        let (mut e, host, files) = autosaving("");
        e.cfg.vim = false;
        e.mode = Mode::Insert;
        keys(&mut e, "ab");
        tick(&mut e, &host, START + AUTOSAVE_IDLE_NS);
        assert_eq!(files.disk(), "ab");
    }

    #[test]
    fn autosave_holds_silently_while_a_prompt_asks() {
        let (mut e, host, files) = autosaving("hello");
        keys(&mut e, "x");
        e.handle(Input::CloseRequested);
        assert_eq!(e.mode(), Mode::Prompt);
        let due = START + AUTOSAVE_IDLE_NS;
        let fx = tick(&mut e, &host, due);
        assert_eq!(writes(&files), 0);
        assert_eq!(fx.wake_at, None);
        // Cancelling lets it through.
        let fx = keys(&mut e, "<Esc>");
        assert_eq!(fx.wake_at, Some(due));
        tick(&mut e, &host, due + 1);
        assert_eq!(files.disk(), "ello");
    }

    #[test]
    fn autosave_skips_reader_view_and_read_only() {
        for which in ["reader", "read-only"] {
            let (mut e, host, files) = autosaving("hello");
            keys(&mut e, "x");
            match which {
                "reader" => e.reader = true,
                _ => e.read_only_chosen = true,
            }
            let fx = tick(&mut e, &host, START + 5 * AUTOSAVE_IDLE_NS);
            assert_eq!(writes(&files), 0, "{which}");
            assert_eq!(e.message(), None, "{which}");
            assert_eq!(fx.wake_at, None, "{which}");
        }
    }

    #[test]
    fn autosave_skips_an_unnamed_buffer() {
        let (mut e, host, files) = autosaving_at("", "hello");
        keys(&mut e, "x");
        tick(&mut e, &host, START + 5 * AUTOSAVE_IDLE_NS);
        assert_eq!(writes(&files), 0);
        assert_eq!(e.message(), None);
    }

    #[test]
    fn autosave_skips_files_it_must_not_write() {
        let dir = tempfile::tempdir().unwrap();
        // Not valid UTF-8.
        let latin1 = dir.path().join("latin1.md");
        fs::write(&latin1, b"caf\xe9\n").unwrap();
        // A directory exists but cannot be read as text.
        let unreadable = dir.path().join("subdir");
        fs::create_dir(&unreadable).unwrap();
        for path in [&latin1, &unreadable] {
            let (mut e, host, _state) = open_autosaving(path);
            keys(&mut e, "<Esc>ihello<Esc>");
            assert!(e.dirty());
            tick(&mut e, &host, START + 5 * AUTOSAVE_IDLE_NS);
            assert_eq!(error(&e), None, "{}", path.display());
        }
        assert_eq!(fs::read(&latin1).unwrap(), b"caf\xe9\n");
    }

    #[test]
    fn autosave_never_writes_back_a_deleted_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello\n").unwrap();
        let (mut e, host, _state) = open_autosaving(&path);
        keys(&mut e, "x");
        fs::remove_file(&path).unwrap();
        e.disk_event(DiskEvent::Gone);
        let fx = tick(&mut e, &host, START + AUTOSAVE_IDLE_NS);
        assert!(!path.exists());
        assert_eq!(e.status().warnings, [strings::NO_LONGER_ON_DISK]);
        assert_eq!(error(&e), None);
        assert_eq!(fx.wake_at, None);
        // Another change waits too.
        let now = START + 2 * AUTOSAVE_IDLE_NS;
        host.state().now = now;
        keys(&mut e, "x");
        tick(&mut e, &host, now + 5 * AUTOSAVE_IDLE_NS);
        assert!(!path.exists());
        assert_eq!(e.status().warnings, [strings::NO_LONGER_ON_DISK]);
        // Writing it back is `:w`'s choice.
        keys(&mut e, ":w<CR>");
        assert_eq!(fs::read_to_string(&path).unwrap(), "llo\n");
        assert!(e.status().warnings.is_empty());
    }

    #[test]
    fn autosave_holds_silently_while_the_disk_warns() {
        for ev in [
            DiskEvent::Gone,
            DiskEvent::Changed,
            DiskEvent::ReadOnly,
            DiskEvent::Unreadable,
        ] {
            let (mut e, host, files) = autosaving("hello");
            keys(&mut e, "x");
            e.disk = Some(ev);
            tick(&mut e, &host, START + 5 * AUTOSAVE_IDLE_NS);
            assert_eq!(writes(&files), 0, "{ev:?}");
            assert_eq!(e.message(), None, "{ev:?}");
        }
    }

    #[test]
    fn closing_drops_the_waiting_autosave() {
        let (mut e, host, files) = autosaving("hello");
        keys(&mut e, "x");
        assert_eq!(keys(&mut e, ":q!<CR>").close, Some(0));
        // A wake already queued still arrives after the close.
        tick(&mut e, &host, START + 5 * AUTOSAVE_IDLE_NS);
        assert_eq!(writes(&files), 0);
        assert_eq!(files.disk(), "hello");
    }
}

//! External changes to the open file (`crate::file::watch`): a git
//! checkout, a formatter, another editor.
//!
//! The shell calls `start_watching` once the first frame is up. The
//! watcher's thread wakes the main loop when it finds something, and the
//! editor takes it on the `Input::Tick` that follows (`poll_disk`);
//! `Input::Focus(true)` asks it to look at once.
//!
//! - Changed, with no unsaved changes: reloaded quietly as one undo step
//!   (`u` undoes it), the cursor on the same line, saying
//!   `reloaded from disk`. A reload waits while a mode holds positions in
//!   the text (the command line, a search, `:s///c`, a prompt, the table
//!   picker, a composition, vim's Insert) and happens when it ends.
//! - Changed, with unsaved changes: no reload; `file changed on disk`
//!   shows beside `changes not saved`, `:w` refuses, `:w!` overwrites and
//!   `:e!` reloads. Undoing back to the saved text takes the reload.
//! - Gone: the text stays and `file no longer on disk` shows; `:w` writes
//!   it back to the same path.
//! - Unreadable or read-only: `read-only` shows (one warning, shared with
//!   the lock's read-only mode, neither clearing the other); `:w` fails
//!   with the OS error.
//! - Back: the warning goes, and a clean document takes the file again
//!   (a file that could not be read at open gets its text now).
//!
//! Every successful write or reload rebases the watch on the version the
//! saver recorded, so the editor's own changes never come back as one.
//! Reader view and read-only documents reload too: they are never dirty.

use std::fs;

use crate::effects::{Effects, Redraw};
use crate::file::{DiskEvent, DiskStat, Watcher};
use crate::status::Mode;
use crate::strings;

use super::Editor;

impl Editor {
    /// Starts watching the file. The shell calls this once the first frame
    /// is up, never on the start-up path. A file that does not exist yet
    /// is watched from its first write.
    pub fn start_watching(&mut self) {
        self.watch_wanted = true;
        self.spawn_watcher();
    }

    /// Stops watching the file, without waiting for the watch's thread
    /// (it ends at its next wait), and forgets what it found, so no
    /// waiting reload runs after. `close` does this after the lock record
    /// goes; dropping the editor stops the watch too.
    pub fn stop_watching(&mut self) {
        self.watch_wanted = false;
        self.watcher = None;
        self.disk = None;
    }

    /// The watch on the file, while there is one.
    pub fn watcher(&self) -> Option<&Watcher> {
        self.watcher.as_ref()
    }

    /// Starts the watch if it is wanted, not running and there is a file.
    fn spawn_watcher(&mut self) {
        if !self.watch_wanted || self.watcher.is_some() || self.path.as_os_str().is_empty() {
            return;
        }
        let Some(stat) = self.saver.stat().or_else(|| self.stat_now()) else {
            return;
        };
        let waker = self.host.waker();
        self.watcher = Some(Watcher::spawn(self.path.clone(), stat, waker));
    }

    /// The file's stat as it is now, if there is one.
    fn stat_now(&self) -> Option<DiskStat> {
        fs::metadata(&self.path).ok().map(|m| DiskStat::of(&m))
    }

    /// After a successful write: the disk holds the document, and the
    /// watch takes the version written (starting, for a file the write
    /// created).
    pub(super) fn wrote(&mut self) {
        self.disk = None;
        if let (Some(w), Some(stat)) = (self.watcher.as_mut(), self.saver.stat()) {
            w.rebase(stat);
        }
        self.spawn_watcher();
    }

    /// After a reload: one that `read` the file holds its version; one that
    /// failed leaves the watch comparing with the version there now, so a
    /// later change is still seen. The watch reports afresh either way.
    pub(super) fn reloaded(&mut self, read: bool) {
        let stat = if read {
            self.saver.stat()
        } else {
            self.watcher.as_ref().and_then(|_| self.stat_now())
        };
        if read || stat.is_some() {
            self.disk = None;
        }
        if let (Some(w), Some(stat)) = (self.watcher.as_mut(), stat) {
            w.rebase(stat);
        }
    }

    /// The window came to the front: the watch looks at once.
    pub(super) fn focus_in(&mut self) -> Effects {
        if let Some(w) = &self.watcher {
            w.check_now();
        }
        Effects::default()
    }

    /// Takes every event the watch has found.
    pub(super) fn poll_disk(&mut self) -> Effects {
        let mut fx = Effects::default();
        while let Some(ev) = self.watcher.as_mut().and_then(Watcher::poll) {
            fx = fx.merge(self.disk_event(ev));
        }
        fx
    }

    /// What the watcher found on disk.
    pub(crate) fn disk_event(&mut self, ev: DiskEvent) -> Effects {
        let fx = match ev {
            DiskEvent::Back => {
                self.disk = None;
                if !self.dirty && self.can_reload_now() {
                    self.quiet_reload(false)
                } else {
                    Effects::default()
                }
            }
            ev => {
                self.disk = Some(ev);
                self.take_waiting_reload()
            }
        };
        fx.merge(Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        })
    }

    /// The reload a change on disk waits for, once the document is clean
    /// and in a mode that can take it.
    pub(super) fn take_waiting_reload(&mut self) -> Effects {
        if self.disk != Some(DiskEvent::Changed) || self.dirty || !self.can_reload_now() {
            return Effects::default();
        }
        self.quiet_reload(true)
    }

    /// True when replacing the text now leaves no mode holding positions
    /// in the old one.
    fn can_reload_now(&self) -> bool {
        let mode = match self.mode {
            Mode::Normal | Mode::Visual | Mode::VisualLine => true,
            // Without vim, Insert is where the editor rests.
            Mode::Insert => !self.cfg.vim,
            Mode::Search | Mode::Command | Mode::Confirm | Mode::Prompt => false,
        };
        mode && self.preedit.is_none() && self.picker.is_none() && self.asking.is_none()
    }

    /// Reloads as `:e!` does, keeping the cursor's line (clamped), and
    /// says `reloaded from disk` for a `changed` file, or for one back
    /// whose text was not the document's. Visual is left only when the
    /// text was replaced (its anchor was in the old text). A failed reload
    /// says why and is not tried again for this change.
    fn quiet_reload(&mut self, changed: bool) -> Effects {
        self.disk = None;
        let before = self.buf.to_text();
        let (read, fx) = self.reload_step();
        let replaced = read && before != self.buf.to_text();
        if replaced {
            self.leave_visual();
        }
        if read && (changed || replaced) {
            self.say(strings::RELOADED_FROM_DISK.to_owned());
        }
        fx
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use crate::editor::{Config, Editor, OpenRequest};
    use crate::effects::Redraw;
    use crate::file::{DiskEvent, StateDir, Watcher};
    use crate::input::Input;
    use crate::platform::Platform;
    use crate::status::Mode;
    use crate::strings;
    use crate::testkit::{FakeHost, ViewSource, keys};
    use crate::view_model::{MsgKind, StatusLeft};

    /// An editor opened on `path` (in reader view with `view`), its host's
    /// clock at 1000, and the temporary state directory its lock record
    /// goes in.
    fn open_as(path: &Path, view: bool) -> (Editor, tempfile::TempDir) {
        open_with(path, view, Config::default())
    }

    fn open_with(path: &Path, view: bool, cfg: Config) -> (Editor, tempfile::TempDir) {
        let host = FakeHost::default();
        host.state().now = 1_000;
        let req = OpenRequest {
            path: path.to_owned(),
            view,
            display_name: None,
        };
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path().join("state"));
        let e = Editor::open_with_state(req, cfg, Platform::Mac, Box::new(host), Some(&state));
        (e, tmp)
    }

    fn open(path: &Path) -> (Editor, tempfile::TempDir) {
        open_as(path, false)
    }

    /// A temporary directory holding `doc.md` with `text`.
    fn doc(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, text).unwrap();
        (dir, path)
    }

    fn colon(e: &mut Editor, typed: &str) {
        keys(e, &format!(":{typed}<CR>"));
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

    fn root() -> bool {
        // SAFETY: `geteuid` cannot fail or touch memory.
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn clean_buffer_reloads_and_u_undoes_it() {
        let (_dir, path) = doc("one\ntwo\nthree\n");
        let (mut e, _state) = open(&path);
        keys(&mut e, "2jl");
        fs::write(&path, "uno\ndos\n").unwrap();
        let fx = e.disk_event(DiskEvent::Changed);
        assert_eq!(e.text(), "uno\ndos\n");
        assert_eq!(e.message(), Some(strings::RELOADED_FROM_DISK));
        assert!(!e.dirty());
        assert_eq!(fx.redraw, Redraw::All);
        assert!(e.status().warnings.is_empty());
        // The cursor keeps its line, clamped to the shorter document.
        assert_eq!(e.cursor().line, 1);
        // `u` undoes the reload as one step, which differs from the disk.
        keys(&mut e, "u");
        assert_eq!(e.text(), "one\ntwo\nthree\n");
        assert!(e.dirty());
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "uno\ndos\n");
        assert!(!e.dirty());
        // The reload recorded the new version: a plain :w goes through.
        keys(&mut e, "x");
        colon(&mut e, "w");
        assert_eq!(e.message(), Some(strings::WRITTEN));
    }

    #[test]
    fn dirty_buffer_warns_and_keeps_text() {
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open(&path);
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.disk_event(DiskEvent::Changed);
        assert_eq!(e.text(), "ello\n");
        assert!(e.dirty());
        let status = e.status();
        assert!(status.dirty);
        assert_eq!(status.warnings, [strings::CHANGED_ON_DISK]);
        colon(&mut e, "w");
        let refused = Some(strings::CHANGED_ON_DISK_REFUSED);
        assert_eq!(error(&e).as_deref(), refused);
        colon(&mut e, "w!");
        assert_eq!(fs::read_to_string(&path).unwrap(), "ello\n");
        assert!(e.status().warnings.is_empty());
    }

    #[test]
    fn e_bang_clears_the_changed_warning() {
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open(&path);
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.disk_event(DiskEvent::Changed);
        colon(&mut e, "e!");
        assert_eq!(e.text(), "changed elsewhere\n");
        assert!(e.status().warnings.is_empty());
    }

    #[test]
    fn undoing_to_clean_takes_the_waiting_reload() {
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open(&path);
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.disk_event(DiskEvent::Changed);
        keys(&mut e, "u");
        assert_eq!(e.text(), "changed elsewhere\n");
        assert_eq!(e.message(), Some(strings::RELOADED_FROM_DISK));
        assert!(e.status().warnings.is_empty());
    }

    #[test]
    fn a_reload_waits_for_the_command_line_to_close() {
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open(&path);
        keys(&mut e, ":");
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.disk_event(DiskEvent::Changed);
        assert_eq!(e.text(), "hello\n");
        assert_eq!(e.mode(), Mode::Command);
        keys(&mut e, "<Esc>");
        assert_eq!(e.text(), "changed elsewhere\n");
    }

    #[test]
    fn reader_view_reloads_when_clean() {
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open_as(&path, true);
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.disk_event(DiskEvent::Changed);
        assert_eq!(e.text(), "changed elsewhere\n");
    }

    #[test]
    fn deleted_then_w_writes_back() {
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open(&path);
        keys(&mut e, "x");
        fs::remove_file(&path).unwrap();
        e.disk_event(DiskEvent::Gone);
        assert_eq!(e.text(), "ello\n");
        assert_eq!(e.status().warnings, [strings::NO_LONGER_ON_DISK]);
        colon(&mut e, "w");
        assert_eq!(e.message(), Some(strings::WRITTEN));
        assert_eq!(fs::read_to_string(&path).unwrap(), "ello\n");
        assert!(e.status().warnings.is_empty());
    }

    #[test]
    fn read_only_on_disk_shows_one_read_only_warning() {
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open(&path);
        e.disk_event(DiskEvent::ReadOnly);
        assert_eq!(e.status().warnings, [strings::READ_ONLY]);
        e.disk_event(DiskEvent::Unreadable);
        assert_eq!(e.status().warnings, [strings::READ_ONLY]);
        // The lock's read-only mode and the disk's share one warning, and
        // neither clears the other.
        e.read_only_chosen = true;
        assert_eq!(e.status().warnings, [strings::READ_ONLY]);
        e.disk_event(DiskEvent::Back);
        assert_eq!(e.status().warnings, [strings::READ_ONLY]);
        e.read_only_chosen = false;
        assert!(e.status().warnings.is_empty());
        e.disk_event(DiskEvent::ReadOnly);
        assert_eq!(e.status().warnings, [strings::READ_ONLY]);
    }

    #[test]
    fn an_unreadable_open_reloads_once_it_is_back() {
        if root() {
            eprintln!("an_unreadable_open_reloads_once_it_is_back: running as root; skipped");
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = doc("secret\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o200)).unwrap();
        let (mut e, _state) = open(&path);
        assert_eq!(e.message(), Some(strings::READ_FAILED));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        e.disk_event(DiskEvent::Back);
        assert_eq!(e.text(), "secret\n");
        keys(&mut e, "x");
        colon(&mut e, "w");
        assert_eq!(e.message(), Some(strings::WRITTEN));
    }

    /// Waits (bounded) until the editor's watcher has looked at the file
    /// more than `after` times.
    fn wait_checks(w: &Watcher, after: u64) {
        for _ in 0..1000 {
            if w.checks() > after {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the watcher never looked at the file");
    }

    fn tick(e: &mut Editor) {
        e.handle(Input::Tick { now_ns: 1_000 });
    }

    #[test]
    fn own_write_does_not_trigger_reload() {
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open(&path);
        assert!(e.watcher().is_none(), "no watch before the first frame");
        e.start_watching();
        wait_checks(e.watcher().unwrap(), 0);
        keys(&mut e, "x");
        colon(&mut e, "w");
        // Focus in looks at the file at once.
        let seen = e.watcher().unwrap().checks();
        e.handle(Input::Focus(true));
        wait_checks(e.watcher().unwrap(), seen);
        tick(&mut e);
        assert_eq!(e.message(), Some(strings::WRITTEN));
        assert_eq!(e.text(), "ello\n");
        assert!(e.status().warnings.is_empty());
        // A change made elsewhere is still seen (waiting, bounded, for the
        // reload itself: a look begun before the write may finish first).
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.handle(Input::Focus(true));
        for _ in 0..1000 {
            tick(&mut e);
            if e.text() != "ello\n" {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(e.text(), "changed elsewhere\n");
        assert_eq!(e.message(), Some(strings::RELOADED_FROM_DISK));
        // Closing stops the watch, whose thread then ends by itself.
        let probe = e.watcher().unwrap().probe();
        colon(&mut e, "q");
        assert!(e.watcher().is_none());
        probe.wait_ended();
    }

    #[test]
    fn a_missing_file_is_watched_from_its_first_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.md");
        let (mut e, _state) = open(&path);
        e.start_watching();
        assert!(e.watcher().is_none());
        keys(&mut e, "ihello<Esc>");
        colon(&mut e, "w");
        assert!(e.watcher().is_some());
        let probe = e.watcher().unwrap().probe();
        e.stop_watching();
        probe.wait_ended();
    }

    #[test]
    fn back_with_the_same_text_keeps_visual() {
        let (_dir, path) = doc("hello\nworld\n");
        let (mut e, _state) = open(&path);
        keys(&mut e, "vj");
        e.disk_event(DiskEvent::ReadOnly);
        e.disk_event(DiskEvent::Back);
        assert_eq!(e.mode(), Mode::Visual);
        // Back with other text leaves Visual: its anchor was in the old.
        e.disk_event(DiskEvent::ReadOnly);
        fs::write(&path, "other\n").unwrap();
        e.disk_event(DiskEvent::Back);
        assert_eq!(e.text(), "other\n");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn closing_drops_a_waiting_reload() {
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open(&path);
        keys(&mut e, ":");
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.disk_event(DiskEvent::Changed);
        let fx = keys(&mut e, "q<CR>");
        assert_eq!(fx.close, Some(0));
        assert_eq!(e.text(), "hello\n");
    }

    #[test]
    fn w_typed_while_a_reload_waits_ends_reloaded() {
        // The document is clean, so the refused `:w` lost nothing: the
        // reload that waited for the command line then takes the file,
        // and its message replaces the refusal.
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open(&path);
        keys(&mut e, ":");
        fs::write(&path, "changed elsewhere\n").unwrap();
        e.disk_event(DiskEvent::Changed);
        keys(&mut e, "w<CR>");
        assert_eq!(e.text(), "changed elsewhere\n");
        assert_eq!(e.message(), Some(strings::RELOADED_FROM_DISK));
        assert!(!e.dirty());
        assert!(e.status().warnings.is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere\n");
    }

    #[test]
    fn vim_off_insert_reload_is_its_own_undo_step() {
        // With vim off, Insert holds no open undo group between keys, so
        // the reload cannot merge into the typing before it.
        let (_dir, path) = doc("hello\n");
        let (mut e, _state) = open_with(
            &path,
            false,
            Config {
                vim: false,
                ..Config::default()
            },
        );
        assert_eq!(e.mode(), Mode::Insert);
        keys(&mut e, "ab<D-s>");
        assert_eq!(e.message(), Some(strings::WRITTEN));
        fs::write(&path, "other\n").unwrap();
        e.disk_event(DiskEvent::Changed);
        assert_eq!(e.text(), "other\n");
        e.undo(false, 1);
        assert_eq!(e.text(), "abhello\n");
    }
}

//! Where the document goes to disk and comes back from: `Editor::write`,
//! `Editor::reload` and `Editor::close` are thin calls into a `Saver`.
//! `FileSaver` is the file the editor was opened on; `Unwired` stands in
//! for an editor built with no file behind it (the tests' editors).

use std::io;
use std::path::PathBuf;

use crate::file::{self, DiskStat, Loaded, WriteError};
use crate::strings;

/// The file behind an editor.
pub trait Saver {
    /// Writes `text` as the document. `force` is `:w!`, which skips only
    /// the check that the file did not change on disk.
    fn write(&mut self, text: &str, force: bool) -> Result<(), WriteError>;
    /// The document as it is on disk now, for `:e!`.
    fn reload(&mut self) -> Loaded;
    /// The window is closing with exit code `code`.
    fn close(&mut self, code: i32);
}

/// The file at `path`, and the version of it the document was read from.
pub struct FileSaver {
    path: PathBuf,
    /// The stat the text was read or last written at; `None` while there
    /// is no file.
    stat: Option<DiskStat>,
    /// The file was not valid UTF-8: it is never written.
    not_utf8: bool,
    /// The file exists but could not be read when the editor opened, so
    /// the document is not its text: every write fails with this reason
    /// (the OS error number, else the kind and text), until a reload reads it.
    unreadable: Option<(Option<i32>, io::ErrorKind, String)>,
}

impl FileSaver {
    /// The file at `path`, as `loaded` found it.
    pub fn new(path: PathBuf, loaded: &Loaded) -> FileSaver {
        let mut saver = FileSaver {
            path,
            stat: None,
            not_utf8: false,
            unreadable: None,
        };
        saver.loaded(loaded);
        saver
    }

    /// Takes in what a read found. A file that could not be read keeps
    /// the stat from before; one never read refuses writes from now on.
    fn loaded(&mut self, loaded: &Loaded) {
        match loaded {
            Loaded::Text { stat, .. } => {
                self.stat = Some(*stat);
                self.not_utf8 = false;
                self.unreadable = None;
            }
            Loaded::NotUtf8 { .. } => self.not_utf8 = true,
            Loaded::Error(e) if self.stat.is_none() => {
                self.unreadable = Some((e.raw_os_error(), e.kind(), e.to_string()));
            }
            Loaded::Missing | Loaded::Error(_) => {}
        }
    }

    /// The read error a write fails with, rebuilt.
    fn read_error(&self) -> Option<io::Error> {
        self.unreadable.as_ref().map(|(raw, kind, text)| match raw {
            Some(n) => io::Error::from_raw_os_error(*n),
            None => io::Error::new(*kind, text.clone()),
        })
    }
}

impl Saver for FileSaver {
    fn write(&mut self, text: &str, force: bool) -> Result<(), WriteError> {
        if self.not_utf8 {
            return Err(WriteError::NotUtf8Source);
        }
        if let Some(e) = self.read_error() {
            return Err(WriteError::Io(e));
        }
        self.stat = Some(file::write(&self.path, text, self.stat.as_ref(), force)?);
        Ok(())
    }
    fn reload(&mut self) -> Loaded {
        let loaded = file::load(&self.path);
        self.loaded(&loaded);
        loaded
    }
    fn close(&mut self, _code: i32) {}
}

/// No file behind the editor: writes and reloads fail, saying so.
pub(crate) struct Unwired;

impl Saver for Unwired {
    fn write(&mut self, _text: &str, _force: bool) -> Result<(), WriteError> {
        Err(WriteError::Io(io::Error::other(strings::NO_FILE_ACCESS)))
    }
    fn reload(&mut self) -> Loaded {
        Loaded::Error(io::Error::other(strings::NO_FILE_ACCESS))
    }
    fn close(&mut self, _code: i32) {}
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::Saver;
    use crate::editor::{Config, Editor, OpenRequest};
    use crate::effects::Effects;
    use crate::input::Input;
    use crate::platform::Platform;
    use crate::status::{self, Mode};
    use crate::strings;
    use crate::testkit::{FakeHost, FileCall, ViewSource, ed_with_files, keys};
    use crate::view_model::{MsgKind, StatusLeft};

    /// An editor opened on `path`, with a host whose clock reads 1000. Its
    /// lock record goes in a temporary state directory (shared by these
    /// tests; each file has its own record, gone when the editor is).
    fn open(path: &Path) -> Editor {
        let host = FakeHost::default();
        host.state().now = 1_000;
        let req = OpenRequest {
            path: path.to_owned(),
            view: false,
            display_name: None,
        };
        let state = crate::file::StateDir::at(
            std::env::temp_dir().join(format!("abc_def-saver-state-{}", std::process::id())),
        );
        Editor::open_with_state(
            req,
            Config::default(),
            Platform::Mac,
            Box::new(host),
            Some(&state),
        )
    }

    fn colon(e: &mut Editor, typed: &str) -> Effects {
        keys(e, &format!(":{typed}<CR>"))
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

    fn os_error(code: i32) -> String {
        std::io::Error::from_raw_os_error(code).to_string()
    }

    #[test]
    fn w_writes_the_file_and_says_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello\r\n").unwrap();
        let mut e = open(&path);
        keys(&mut e, "x");
        let fx = colon(&mut e, "w");
        assert_eq!(fs::read_to_string(&path).unwrap(), "ello\r\n");
        assert_eq!(e.message(), Some(strings::WRITTEN));
        assert!(!e.dirty());
        assert_eq!(fx.dirty, Some(false));
    }

    #[test]
    fn invalid_utf8_opens_read_only_and_refuses_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latin1.md");
        let bytes = b"caf\xe9\n".to_vec();
        fs::write(&path, &bytes).unwrap();
        let mut e = open(&path);
        assert_eq!(e.status().warnings, [strings::NOT_UTF8_READ_ONLY]);
        keys(&mut e, "ihello<Esc>");
        for cmd in ["w", "w!", "wq", "x"] {
            let fx = colon(&mut e, cmd);
            assert_eq!(fx.close, None, "{cmd}");
            let refused = Some(strings::NOT_UTF8_REFUSED);
            assert_eq!(error(&e).as_deref(), refused, "{cmd}");
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(e.dirty());
    }

    #[test]
    fn missing_file_opens_empty_and_first_w_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.md");
        let mut e = open(&path);
        assert_eq!(e.text(), "");
        assert_eq!(e.message(), None);
        keys(&mut e, "ihello<Esc>");
        colon(&mut e, "w");
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
        assert_eq!(e.message(), Some(strings::WRITTEN));
    }

    #[test]
    fn missing_parent_says_the_os_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-dir").join("new.md");
        let mut e = open(&path);
        keys(&mut e, "ihello<Esc>");
        colon(&mut e, "w");
        let want = format!("write failed: {}", os_error(libc::ENOENT));
        assert_eq!(error(&e), Some(want));
        assert!(e.dirty());
    }

    #[test]
    fn cq_exits_1() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("COMMIT_EDITMSG");
        fs::write(&path, "message\n").unwrap();
        let mut e = open(&path);
        keys(&mut e, "x");
        assert_eq!(colon(&mut e, "cq").close, Some(1));
        assert_eq!(fs::read_to_string(&path).unwrap(), "message\n");
    }

    #[test]
    fn wq_does_not_close_on_failed_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-dir").join("new.md");
        let mut e = open(&path);
        keys(&mut e, "ihello<Esc>");
        assert_eq!(colon(&mut e, "wq").close, None);
        let want = format!("write failed: {}", os_error(libc::ENOENT));
        assert_eq!(error(&e), Some(want));
        // Once the directory is there, the same command writes and closes.
        fs::create_dir(path.parent().unwrap()).unwrap();
        assert_eq!(colon(&mut e, "wq").close, Some(0));
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn w_refuses_a_file_changed_on_disk_until_w_bang() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello\n").unwrap();
        let mut e = open(&path);
        keys(&mut e, "x");
        fs::write(&path, "changed elsewhere\n").unwrap();
        colon(&mut e, "w");
        let refused = Some(strings::CHANGED_ON_DISK_REFUSED);
        assert_eq!(error(&e).as_deref(), refused);
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere\n");
        assert!(e.dirty());
        colon(&mut e, "w!");
        assert_eq!(fs::read_to_string(&path).unwrap(), "ello\n");
        assert!(!e.dirty());
        // The write recorded the new version: a plain :w now goes through.
        keys(&mut e, "x");
        colon(&mut e, "w");
        assert_eq!(fs::read_to_string(&path).unwrap(), "llo\n");
    }

    #[test]
    fn e_bang_takes_the_line_endings_and_bom_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "one\ntwo").unwrap();
        let mut e = open(&path);
        keys(&mut e, "x");
        fs::write(&path, "\u{FEFF}three\r\nfour\r\n").unwrap();
        let fx = colon(&mut e, "e!");
        assert_eq!(e.text(), "\u{FEFF}three\r\nfour\r\n");
        assert!(!e.dirty());
        assert_eq!(fx.dirty, Some(false));
        // One undo step back to the text before, and the version reloaded
        // is the one a plain :w now expects.
        keys(&mut e, "u");
        assert_eq!(e.buffer().line(0), "ne");
        colon(&mut e, "w");
        assert_eq!(e.message(), Some(strings::WRITTEN));
    }

    #[test]
    fn the_open_message_fades() {
        // A directory exists but cannot be read as text.
        let dir = tempfile::tempdir().unwrap();
        let mut e = open(dir.path());
        assert_eq!(e.message(), Some(strings::READ_FAILED));
        let due = 1_000 + status::MESSAGE_FOR_NS;
        assert_eq!(e.open_effects().wake_at, Some(due));
        e.handle(Input::Tick { now_ns: due });
        assert_eq!(e.message(), None);
    }

    #[test]
    fn an_unreadable_file_refuses_writes_with_the_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.md");
        fs::write(&path, "secret").unwrap();
        let denied = std::io::Error::from_raw_os_error(libc::EACCES);
        let mut saver = super::FileSaver::new(path.clone(), &super::Loaded::Error(denied));
        for force in [false, true] {
            let err = saver.write("", force).unwrap_err();
            let want = format!("write failed: {}", os_error(libc::EACCES));
            assert_eq!(err.message(), want, "force {force}");
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "secret");
    }

    #[test]
    fn an_unreadable_open_says_the_read_error_on_w() {
        // A directory exists but cannot be read as text.
        let dir = tempfile::tempdir().unwrap();
        let mut e = open(dir.path());
        for cmd in ["w", "w!"] {
            colon(&mut e, cmd);
            let want = format!("write failed: {}", os_error(libc::EISDIR));
            assert_eq!(error(&e), Some(want), "{cmd}");
        }
    }

    #[test]
    fn a_write_only_file_is_never_blanked() {
        // SAFETY: `geteuid` cannot fail or touch memory.
        if unsafe { libc::geteuid() } == 0 {
            eprintln!(
                "a_write_only_file_is_never_blanked: running as root, mode 0200 denies nothing; skipped"
            );
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drop-box.md");
        fs::write(&path, "secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o200)).unwrap();
        let mut e = open(&path);
        assert_eq!(e.message(), Some(strings::READ_FAILED));
        colon(&mut e, "w!");
        let want = format!("write failed: {}", os_error(libc::EACCES));
        assert_eq!(error(&e), Some(want));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "secret");
    }

    #[test]
    fn e_bang_on_a_missing_file_says_the_os_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello").unwrap();
        let mut e = open(&path);
        fs::remove_file(&path).unwrap();
        colon(&mut e, "e!");
        let want = format!("reload failed: {}", os_error(libc::ENOENT));
        assert_eq!(error(&e), Some(want));
        assert_eq!(e.text(), "hello");
    }

    #[test]
    fn primary_s_is_ignored_while_confirming() {
        let (mut e, files) = ed_with_files("a a");
        keys(&mut e, ":s/a/b/gc<CR>y<D-s>");
        assert!(files.calls().is_empty());
        assert_eq!(e.mode(), Mode::Confirm);
        keys(&mut e, "y");
        assert_eq!(e.text(), "b b");
        // The whole session is still one undo step.
        keys(&mut e, "u");
        assert_eq!(e.text(), "a a");
    }

    #[test]
    fn primary_s_writes_in_any_mode() {
        let (mut e, files) = ed_with_files("hello");
        keys(&mut e, "x<D-s>");
        assert_eq!(files.calls(), [FileCall::Write { force: false }]);
        assert_eq!(files.disk(), "ello");
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "iab<D-s>");
        assert_eq!(files.disk(), "abello");
        assert_eq!(e.mode(), Mode::Insert);
        assert!(!e.dirty());
        // Other primary chords are still swallowed.
        keys(&mut e, "<D-z>");
        assert_eq!(files.calls().len(), 2);
    }

    #[test]
    fn zz_writes_if_changed_and_closes() {
        let (mut e, files) = ed_with_files("hello");
        assert_eq!(keys(&mut e, "ZZ").close, Some(0));
        assert_eq!(files.calls(), [FileCall::Close { code: 0 }]);
        let (mut e, files) = ed_with_files("hello");
        assert_eq!(keys(&mut e, "xZZ").close, Some(0));
        let wrote_and_closed = [
            FileCall::Write { force: false },
            FileCall::Close { code: 0 },
        ];
        assert_eq!(files.calls(), wrote_and_closed);
        assert_eq!(files.disk(), "ello");
    }

    #[test]
    fn zq_closes_and_discards() {
        let (mut e, files) = ed_with_files("hello");
        assert_eq!(keys(&mut e, "xZQ").close, Some(0));
        assert_eq!(files.calls(), [FileCall::Close { code: 0 }]);
        assert_eq!(files.disk(), "hello");
        // Z then another key is nothing.
        let (mut e, _) = ed_with_files("hello");
        assert_eq!(keys(&mut e, "Zx").close, None);
        assert_eq!(e.text(), "hello");
    }

    #[test]
    fn undo_to_saved_text_is_clean() {
        let (mut e, _files) = ed_with_files("hello");
        keys(&mut e, "x");
        colon(&mut e, "w");
        keys(&mut e, "x");
        assert!(e.dirty());
        let fx = keys(&mut e, "u");
        assert_eq!(e.text(), "ello");
        assert!(!e.dirty());
        assert_eq!(fx.dirty, Some(false));
        let fx = keys(&mut e, "u");
        assert_eq!(e.text(), "hello");
        assert!(e.dirty());
        assert_eq!(fx.dirty, Some(true));
        let fx = keys(&mut e, "<C-r>");
        assert!(!e.dirty());
        assert_eq!(fx.dirty, Some(false));
        // A new change after undoing past the save point clears the redo
        // that led back to it.
        keys(&mut e, "ux");
        keys(&mut e, "u");
        assert_eq!(e.text(), "hello");
        assert!(e.dirty());
    }

    #[test]
    fn a_write_inside_an_insert_session_splits_its_undo_step() {
        let (mut e, _files) = ed_with_files("");
        keys(&mut e, "iab<D-s>cd<Esc>");
        assert_eq!(e.text(), "abcd");
        keys(&mut e, "u");
        assert_eq!(e.text(), "ab");
        assert!(!e.dirty());
    }

    #[test]
    fn a_fresh_editor_is_at_its_save_point() {
        let (mut e, _files) = ed_with_files("hello");
        keys(&mut e, "xu");
        assert_eq!(e.text(), "hello");
        assert!(!e.dirty());
    }
}

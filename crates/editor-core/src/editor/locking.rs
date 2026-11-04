//! The file's lock record (`crate::file::lock`) at open, and the
//! read-only mode its prompt can choose.
//!
//! Opening a named file takes a lock record in the state directory. When
//! another live window holds the file, the prompt
//! `already open in another window (pid N): [o]pen read-only [e]dit anyway [q]uit`
//! asks before any editing (`prompt`); Esc does nothing, so the editor is
//! never left editing without a choice. `o` makes the document read-only:
//! edits are refused as reader view refuses them, writes say why, and the
//! warning `read-only` stays beside the file name. When no record can be
//! taken (no state directory, or one that cannot be written) the file
//! opens as usual and says so once.
//!
//! The lock lives as long as the open file: closing releases it, and so
//! does dropping the editor. `:e!` keeps it.

use crate::file::lock::{Lock, LockOutcome};
use crate::file::{DiskEvent, StateDir};
use crate::status;
use crate::strings;
use crate::view_model::MsgKind;

use super::Editor;
use super::prompt::Ask;

impl Editor {
    /// Takes the lock record for the open file in `state` (`None`: the
    /// environment names no state directory). Another live holder asks
    /// the already-open prompt; a lock that cannot be taken says so once,
    /// and the file opens as usual.
    pub(super) fn lock_at_open(&mut self, state: Option<&StateDir>) {
        let outcome = match state {
            Some(state) => Lock::acquire(state, &self.path),
            None => {
                self.lock_unavailable(strings::NO_STATE_DIRECTORY);
                return;
            }
        };
        match outcome {
            LockOutcome::Acquired(lock) => self.lock = Some(lock),
            LockOutcome::HeldBy { pid, lock } => {
                self.lock = Some(lock);
                // The shell paints everything once the window is up.
                let _ = self.ask(Ask::AlreadyOpen { pid });
            }
            // A holder seen all the same still asks; the failure is said
            // once the prompt is answered.
            LockOutcome::Unavailable {
                error,
                holder: Some(pid),
            } => {
                self.lock_warning = Some(error.to_string());
                let _ = self.ask(Ask::AlreadyOpen { pid });
            }
            // The recovery prompt about to ask would hide it: it is said
            // once that is answered.
            LockOutcome::Unavailable {
                error,
                holder: None,
            } if self.recovery_found.is_some() => self.lock_warning = Some(error.to_string()),
            LockOutcome::Unavailable {
                error,
                holder: None,
            } => self.lock_unavailable(&error.to_string()),
        }
    }

    /// Once the prompts at open are answered: says the lock failure held
    /// back, if any.
    pub(super) fn say_held_lock_warning(&mut self) {
        if let Some(reason) = self.lock_warning.take() {
            self.lock_unavailable(&reason);
        }
    }

    fn lock_unavailable(&mut self, reason: &str) {
        self.say_as(
            strings::lock_unavailable(reason),
            MsgKind::Warning,
            status::MESSAGE_FOR_NS,
        );
    }

    /// Lets the lock record go. `close` does this; a shell that exits
    /// without closing the editor or dropping it (a bench run at its
    /// deadline, the router quitting) calls it first. Doing it twice is
    /// harmless.
    pub fn release_lock(&mut self) {
        if let Some(lock) = self.lock.take() {
            lock.release();
        }
    }

    /// `o` at the already-open prompt: the document refuses edits and
    /// writes from now on, with `read-only` beside the file name (unless
    /// `read-only: not valid UTF-8` already says so: `shown_warnings`).
    pub(super) fn open_read_only(&mut self) {
        self.read_only_chosen = true;
    }

    /// The warnings beside the file name: the file's own, what the file
    /// watch found (`disk`), then `read-only` once, whether chosen here or
    /// found on disk, which a not-UTF-8 warning already covers.
    pub(super) fn shown_warnings(&self) -> Vec<&'static str> {
        let mut w = self.warnings.clone();
        match self.disk {
            Some(DiskEvent::Changed) => w.push(strings::CHANGED_ON_DISK),
            Some(DiskEvent::Gone) => w.push(strings::NO_LONGER_ON_DISK),
            _ => {}
        }
        let disk_read_only = matches!(self.disk, Some(DiskEvent::ReadOnly | DiskEvent::Unreadable));
        if (self.read_only_chosen || disk_read_only) && !w.contains(&strings::NOT_UTF8_READ_ONLY) {
            w.push(strings::READ_ONLY);
        }
        w
    }

    /// True while the document refuses edits: reader view, or opened
    /// read-only from the already-open prompt. With vim off, also a
    /// document read from a file that is not valid UTF-8 (never written
    /// back): with no mode to show, an edit there would look like one that
    /// could be saved. Vim keeps editing it, `:w` refusing.
    pub(super) fn refuses_edits(&self) -> bool {
        self.reader
            || self.read_only_chosen
            || (!self.cfg.vim && self.warnings.contains(&strings::NOT_UTF8_READ_ONLY))
    }

    /// A write refused because the document was opened read-only, saying
    /// so as an Error. False when writing may go ahead.
    pub(super) fn refuse_write(&mut self) -> bool {
        if !self.read_only_chosen {
            return false;
        }
        self.say_as(
            strings::READ_ONLY_REFUSED.to_owned(),
            MsgKind::Error,
            status::MESSAGE_FOR_NS,
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command};

    use crate::editor::{Config, Editor, OpenRequest};
    use crate::file::StateDir;
    use crate::file::lock::fnv1a64;
    use crate::input::Input;
    use crate::platform::Platform;
    use crate::status::{self, Mode};
    use crate::strings;
    use crate::testkit::{FakeHost, ViewSource, keys};
    use crate::view_model::{MsgKind, StatusLeft};

    /// A child process that holds the file, killed and reaped on drop.
    struct Holder(Child);
    impl Holder {
        fn spawn() -> Holder {
            Holder(Command::new("sleep").arg("60").spawn().unwrap())
        }
        fn pid(&self) -> u32 {
            self.0.id()
        }
    }
    impl Drop for Holder {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A temporary directory with `doc.md` (holding `hello`) and a state
    /// directory beside it.
    struct Fixture {
        dir: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("doc.md"), "hello").unwrap();
            Fixture { dir }
        }
        fn doc(&self) -> PathBuf {
            self.dir.path().join("doc.md")
        }
        fn state(&self) -> StateDir {
            StateDir::at(self.dir.path().join("state"))
        }
        /// The lock records in the state directory.
        fn records(&self) -> Vec<String> {
            let Ok(entries) = fs::read_dir(self.state().locks()) else {
                return Vec::new();
            };
            let mut v: Vec<String> = entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
        /// Writes the record `holder` would have written for the document.
        fn plant(&self, holder: &Holder) {
            let state = self.state();
            state.ensure().unwrap();
            let canon = fs::canonicalize(self.doc()).unwrap();
            let hash = fnv1a64(canon.to_string_lossy().as_bytes());
            let pid = holder.pid();
            let rec = state.locks().join(format!("{hash:016x}-{pid}.lock"));
            fs::write(rec, format!("{pid}\n{}\n", canon.display())).unwrap();
        }
    }

    /// An editor opened on `path` with `state`, its host's clock at 1000.
    fn open(path: &Path, state: Option<&StateDir>) -> Editor {
        let host = FakeHost::default();
        host.state().now = 1_000;
        let req = OpenRequest {
            path: path.to_owned(),
            view: false,
            display_name: None,
        };
        Editor::open_with_state(req, Config::default(), Platform::Mac, Box::new(host), state)
    }

    /// An editor opened on the fixture's document while `holder` holds it.
    fn open_held(f: &Fixture, holder: &Holder) -> Editor {
        f.plant(holder);
        open(&f.doc(), Some(&f.state()))
    }

    fn asks_already_open(e: &Editor, holder: &Holder) -> bool {
        e.mode() == Mode::Prompt
            && e.status().left == StatusLeft::Prompt(strings::already_open(holder.pid()))
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
    fn first_open_takes_the_lock_quietly() {
        let f = Fixture::new();
        let mut e = open(&f.doc(), Some(&f.state()));
        assert_eq!(f.records().len(), 1);
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.message(), None);
        assert!(e.status().warnings.is_empty());
        keys(&mut e, "x");
        assert_eq!(e.text(), "ello");
    }

    #[test]
    fn a_held_file_asks_before_any_editing() {
        let f = Fixture::new();
        let holder = Holder::spawn();
        let mut e = open_held(&f, &holder);
        assert!(asks_already_open(&e, &holder));
        assert_eq!(
            e.prompt().as_deref(),
            Some(
                format!(
                    "already open in another window (pid {}): [o]pen read-only [e]dit anyway [q]uit",
                    holder.pid()
                )
                .as_str()
            )
        );
        assert!(!e.ime_active());
        keys(&mut e, "xdd");
        assert_eq!(e.text(), "hello");
        assert!(asks_already_open(&e, &holder));
        // Our own record sits next to the holder's.
        assert_eq!(f.records().len(), 2);
    }

    #[test]
    fn prompt_o_makes_read_only() {
        let f = Fixture::new();
        let holder = Holder::spawn();
        let mut e = open_held(&f, &holder);
        let fx = keys(&mut e, "o");
        assert_eq!(fx.close, None);
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.prompt(), None);
        assert_eq!(e.status().warnings, [strings::READ_ONLY]);
        assert_eq!(e.status().left, StatusLeft::Mode(strings::MODE_NORMAL));
        keys(&mut e, "xddihello<Esc>");
        assert_eq!(e.text(), "hello");
        assert!(!e.dirty());
        // The warning stays: it is not a message that fades.
        e.handle(Input::Tick {
            now_ns: 1_000 + 10 * status::MESSAGE_FOR_NS,
        });
        assert_eq!(e.status().warnings, [strings::READ_ONLY]);
        // Moving about still works.
        keys(&mut e, "$");
        assert_eq!(e.cursor().col, 4);
    }

    #[test]
    fn prompt_e_edits() {
        let f = Fixture::new();
        let holder = Holder::spawn();
        let mut e = open_held(&f, &holder);
        assert_eq!(keys(&mut e, "e").close, None);
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.prompt(), None);
        assert!(e.status().warnings.is_empty());
        keys(&mut e, "x:w<CR>");
        assert_eq!(fs::read_to_string(f.doc()).unwrap(), "ello");
        // The record is still ours, for a third window to see.
        assert_eq!(f.records().len(), 2);
    }

    #[test]
    fn prompt_q_exits_0() {
        let f = Fixture::new();
        let holder = Holder::spawn();
        let mut e = open_held(&f, &holder);
        assert_eq!(keys(&mut e, "q").close, Some(0));
        // Our record went with the window; the holder's stays.
        assert_eq!(f.records().len(), 1);
        assert!(f.records()[0].ends_with(&format!("-{}.lock", holder.pid())));
    }

    #[test]
    fn prompt_esc_does_nothing() {
        let f = Fixture::new();
        let holder = Holder::spawn();
        let mut e = open_held(&f, &holder);
        let fx = keys(&mut e, "<Esc>");
        assert_eq!(fx.close, None);
        assert!(asks_already_open(&e, &holder));
        // Nor does the close prompt's cancel, nor a modified answer.
        keys(&mut e, "c<D-o><C-e><D-q>");
        assert!(asks_already_open(&e, &holder));
        assert!(e.status().warnings.is_empty());
        assert_eq!(e.text(), "hello");
        // Shift is still an answer.
        keys(&mut e, "E");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn read_only_refuses_writes_and_edits() {
        let f = Fixture::new();
        let holder = Holder::spawn();
        let mut e = open_held(&f, &holder);
        keys(&mut e, "o");
        for typed in [":w<CR>", ":w!<CR>", "<D-s>", ":wq<CR>"] {
            let fx = keys(&mut e, typed);
            assert_eq!(fx.close, None, "{typed}");
            let refused = Some(strings::READ_ONLY_REFUSED);
            assert_eq!(error(&e).as_deref(), refused, "{typed}");
            keys(&mut e, "<Esc>");
        }
        assert_eq!(fs::read_to_string(f.doc()).unwrap(), "hello");
        // Every way of editing is refused, as in reader view.
        e.host().clipboard_set("pasted");
        let edits = [
            "x",
            "dd",
            "ihi<Esc>",
            "ohi<Esc>",
            "p",
            "<D-v>",
            "vd",
            "v<D-x>",
            "<C-Space>",
            ">>",
            ".",
            ":s/h/j/<CR>",
            "<D-l>",
        ];
        for typed in edits {
            keys(&mut e, typed);
            keys(&mut e, "<Esc><Esc>");
            assert_eq!(e.text(), "hello", "{typed}");
            assert!(!e.dirty(), "{typed}");
        }
        assert!(!e.ime_active());
        // `:q` closes without asking.
        assert_eq!(keys(&mut e, ":q<CR>").close, Some(0));
        assert_eq!(f.records().len(), 1);
    }

    #[test]
    fn read_only_with_vim_off_refuses_typing() {
        let f = Fixture::new();
        let holder = Holder::spawn();
        f.plant(&holder);
        let req = OpenRequest {
            path: f.doc(),
            view: false,
            display_name: None,
        };
        let mut e = Editor::open_with_state(
            req,
            Config {
                vim: false,
                ..Config::default()
            },
            Platform::Mac,
            Box::new(FakeHost::default()),
            Some(&f.state()),
        );
        assert!(asks_already_open(&e, &holder));
        keys(&mut e, "o");
        keys(&mut e, "abc<CR><BS><Del>");
        e.handle(Input::Text("typed".into()));
        assert_eq!(e.text(), "hello");
        assert!(!e.ime_active());
        assert_eq!(e.handle(Input::CloseRequested).close, Some(0));
    }

    #[test]
    fn lock_failure_opens_with_warning() {
        let f = Fixture::new();
        // A regular file sits where the state directory must be made.
        let blocked = f.dir.path().join("blocked");
        fs::write(&blocked, "x").unwrap();
        let state = StateDir::at(&blocked);
        let mut e = open(&f.doc(), Some(&state));
        let reason = state.ensure().unwrap_err().to_string();
        assert_eq!(
            e.status().left,
            StatusLeft::Message {
                text: strings::lock_unavailable(&reason),
                kind: MsgKind::Warning,
            }
        );
        // It is a one-off: it fades like any other message.
        let due = 1_000 + status::MESSAGE_FOR_NS;
        assert_eq!(e.open_effects().wake_at, Some(due));
        e.handle(Input::Tick { now_ns: due });
        assert_eq!(e.message(), None);
        assert!(e.status().warnings.is_empty());
        keys(&mut e, "x:w<CR>");
        assert_eq!(fs::read_to_string(f.doc()).unwrap(), "ello");
    }

    #[test]
    fn no_state_dir_opens_without_lock() {
        let f = Fixture::new();
        let mut e = open(&f.doc(), None);
        assert_eq!(
            e.status().left,
            StatusLeft::Message {
                text: strings::lock_unavailable(strings::NO_STATE_DIRECTORY),
                kind: MsgKind::Warning,
            }
        );
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "x");
        assert_eq!(e.text(), "ello");
        assert!(!f.state().root().exists());
    }

    #[test]
    fn an_unreadable_file_takes_no_lock() {
        // A directory exists but cannot be read as text.
        let f = Fixture::new();
        let e = open(f.dir.path(), Some(&f.state()));
        assert_eq!(e.message(), Some(strings::READ_FAILED));
        assert!(f.records().is_empty());
    }

    #[test]
    fn a_missing_file_takes_the_lock() {
        let f = Fixture::new();
        let _e = open(&f.dir.path().join("new.md"), Some(&f.state()));
        assert_eq!(f.records().len(), 1);
    }

    /// Puts a directory where this process's record for the document
    /// goes, so writing it fails (even for root), and returns why.
    fn block_our_record(f: &Fixture) -> String {
        let state = f.state();
        state.ensure().unwrap();
        let canon = fs::canonicalize(f.doc()).unwrap();
        let hash = fnv1a64(canon.to_string_lossy().as_bytes());
        let ours = state
            .locks()
            .join(format!("{hash:016x}-{}.lock", std::process::id()));
        fs::create_dir(&ours).unwrap();
        fs::write(&ours, "").unwrap_err().to_string()
    }

    fn warns(e: &Editor, text: &str) -> bool {
        e.status().left
            == StatusLeft::Message {
                text: text.to_owned(),
                kind: MsgKind::Warning,
            }
    }

    #[test]
    fn a_held_file_asks_even_when_the_lock_cannot_be_written() {
        for answer in ["e", "o"] {
            let f = Fixture::new();
            let holder = Holder::spawn();
            let reason = block_our_record(&f);
            let mut e = open_held(&f, &holder);
            assert!(asks_already_open(&e, &holder), "{answer}");
            // The answer is carried out, then the lock failure is said.
            let fx = keys(&mut e, answer);
            assert_eq!(fx.close, None, "{answer}");
            assert_eq!(e.mode(), Mode::Normal, "{answer}");
            assert!(warns(&e, &strings::lock_unavailable(&reason)), "{answer}");
            assert_eq!(fx.wake_at, Some(1_000 + status::MESSAGE_FOR_NS), "{answer}");
            keys(&mut e, "x");
            let want = if answer == "e" { "ello" } else { "hello" };
            assert_eq!(e.text(), want, "{answer}");
        }
    }

    #[test]
    fn a_held_file_quit_with_a_failed_lock_exits_0() {
        let f = Fixture::new();
        let holder = Holder::spawn();
        block_our_record(&f);
        let mut e = open_held(&f, &holder);
        assert_eq!(keys(&mut e, "q").close, Some(0));
    }

    #[test]
    fn a_held_not_utf8_file_opened_read_only_shows_one_warning() {
        let f = Fixture::new();
        fs::write(f.doc(), b"caf\xe9\n").unwrap();
        let holder = Holder::spawn();
        let mut e = open_held(&f, &holder);
        keys(&mut e, "o");
        assert_eq!(e.status().warnings, [strings::NOT_UTF8_READ_ONLY]);
        // Once the file is valid UTF-8 again, the choice still stands.
        fs::write(f.doc(), "hello").unwrap();
        keys(&mut e, ":e!<CR>");
        assert_eq!(e.text(), "hello");
        assert_eq!(e.status().warnings, [strings::READ_ONLY]);
        keys(&mut e, "x");
        assert_eq!(e.text(), "hello");
    }

    #[test]
    fn lock_released_on_close() {
        let f = Fixture::new();
        let mut e = open(&f.doc(), Some(&f.state()));
        assert_eq!(f.records().len(), 1);
        // `:e!` keeps the same lock.
        let before = f.records();
        keys(&mut e, "x:e!<CR>");
        assert_eq!(e.text(), "hello");
        assert_eq!(f.records(), before);
        assert_eq!(e.handle(Input::CloseRequested).close, Some(0));
        assert!(f.records().is_empty());
        // Dropping the editor releases it too.
        let e = open(&f.doc(), Some(&f.state()));
        assert_eq!(f.records().len(), 1);
        drop(e);
        assert!(f.records().is_empty());
    }
}

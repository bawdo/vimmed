//! Recovery copies of unsaved text (`crate::file::recovery`) and the
//! recovery prompt at open.
//!
//! While the document is dirty, this window keeps a copy of its text in
//! the state directory: written 2 s after the last change, and at least
//! every 10 s while changes keep coming, on a worker thread (the timing is
//! `Recovery`'s; the wait is the shell's timer, as autosave's is). The
//! copy goes as soon as the document is clean again (any write, `:e!`, an
//! undo back to the save point) and when the window closes deliberately
//! (`:q!`, `ZQ`, `:cq`, the close prompt's discard). A signal (SIGTERM)
//! writes it at once, if dirty, and keeps it.
//!
//! At open, a copy left by a window that is gone and written after the
//! file was last modified asks
//! `recovery copy found: [r]estore [d]iscard [c]ompare`, once the
//! already-open prompt (`locking`) is answered: `r` puts the copy's text
//! in the document as one undo step and leaves it dirty, `d` removes the
//! copy, and `c` opens it in a reader window beside this one, the prompt
//! staying until `r` or `d`. Esc does nothing. A document that refuses
//! edits (reader view, or `o` at the already-open prompt) is not asked:
//! the copy stays for an open that can take it. `q` at the already-open
//! prompt quits before anything is asked.
//!
//! With no state directory there are no copies at all, and nothing more
//! is said than the lock's warning. A copy that cannot be written says so
//! once, until a write succeeds again.

use std::fs;
use std::os::unix::fs::MetadataExt;

use crate::buffer::{Buffer, Pos, ShiftMark};
use crate::effects::{Effects, Redraw};
use crate::file::StateDir;
use crate::file::lock;
use crate::file::recovery::{self, Recovery};
use crate::host::WindowRequest;
use crate::input::SignalKind;
use crate::status;
use crate::strings;
use crate::view_model::MsgKind;
use crate::vim;
use crate::vim::operator::OpOutcome;

use super::Editor;
use super::prompt::Ask;

impl Editor {
    /// At open: looks for a copy a window that is gone left of the file,
    /// newer than the file on disk, and starts this window's own copy.
    pub(super) fn start_recovery(&mut self, state: &StateDir) {
        let mtime = fs::metadata(&self.path)
            .ok()
            .map(|m| m.mtime().saturating_mul(1_000_000_000) + m.mtime_nsec());
        self.recovery_found = recovery::find_newer(state, &self.path, mtime, lock::is_alive);
        let mut r = Recovery::new(state, &self.path, std::process::id());
        r.set_waker(self.host.waker());
        self.recovery = Some(r);
    }

    /// Asks the recovery prompt when a copy was found and the document
    /// can take it; otherwise says the lock warning held back, if any.
    pub(super) fn offer_recovery(&mut self) {
        if self.refuses_edits() {
            // Kept on disk for an open that can take it.
            self.recovery_found = None;
        }
        if self.recovery_found.is_some() {
            let _ = self.ask(Ask::Recovery);
        } else {
            self.say_held_lock_warning();
        }
    }

    /// `r` at the recovery prompt: the copy's text replaces the
    /// document's, as one undo step, leaving it dirty. This window's own
    /// copy is written before the old one goes.
    pub(super) fn restore_recovery(&mut self) -> Effects {
        let Some(found) = self.recovery_found.take() else {
            return Effects::default();
        };
        let copy = match fs::read_to_string(&found.text_path) {
            Ok(text) => Buffer::from_text(&text),
            Err(e) => {
                self.recovery_trouble(&e);
                return Effects::default();
            }
        };
        let lines = (0..copy.line_count()).map(|i| copy.line(i));
        let text = lines.collect::<Vec<_>>().join("\n");
        let last = self.buf.line_count() - 1;
        let end = Pos {
            line: last,
            col: self.buf.line(last).len(),
        };
        let fx = self.run(|ctx| {
            ctx.replace(Pos::default()..end, &text);
            OpOutcome {
                cursor: Pos::default(),
                insert: false,
            }
        });
        if self.cfg.vim {
            self.cursor = vim::rest(&self.buf, self.cursor);
        }
        let now = self.buf.to_text();
        let kept = self.recovery.as_mut().map(|r| r.write_now(&now));
        match kept {
            Some(Ok(())) => {
                if let Err(e) = found.delete() {
                    self.recovery_trouble(&e);
                }
            }
            Some(Err(e)) => self.say_as(
                strings::recovery_failed(&e.to_string()),
                MsgKind::Warning,
                status::MESSAGE_FOR_NS,
            ),
            None => {}
        }
        fx.merge(Effects {
            redraw: Redraw::All,
            ..Effects::default()
        })
    }

    /// `d` at the recovery prompt: the copy found is removed.
    pub(super) fn discard_found(&mut self) {
        if let Some(found) = self.recovery_found.take()
            && let Err(e) = found.delete()
        {
            self.recovery_trouble(&e);
        }
    }

    /// `c` at the recovery prompt: the copy opens in a reader window.
    pub(super) fn compare_recovery(&mut self) {
        if let Some(found) = &self.recovery_found {
            let req = WindowRequest {
                path: found.text_path.clone(),
                view: true,
                display_name: Some(strings::recovery_title(&self.title)),
            };
            self.host.spawn_window(req);
        }
    }

    fn recovery_trouble(&mut self, e: &std::io::Error) {
        self.say_as(
            strings::recovery_unreadable(&e.to_string()),
            MsgKind::Error,
            status::MESSAGE_FOR_NS,
        );
    }

    /// After an input: a clean document has no copy; a change to a dirty
    /// one schedules the copy's write, asking to be woken when a new wait
    /// starts.
    pub(super) fn recovery_after(&mut self, edits: ShiftMark) -> Effects {
        let Some(r) = self.recovery.as_mut() else {
            return Effects::default();
        };
        if !self.dirty {
            r.delete();
            return Effects::default();
        }
        if self.buf.shift_mark() == edits {
            return Effects::default();
        }
        let buf = &self.buf;
        Effects {
            wake_at: r.schedule(self.host.now_ns(), || buf.to_text()),
            ..Effects::default()
        }
    }

    /// The main loop woke at `now_ns`: the copy is written if it is due,
    /// and a write that failed says so, once.
    pub(super) fn recovery_tick(&mut self, now_ns: u64) -> Effects {
        let Some(r) = self.recovery.as_mut() else {
            return Effects::default();
        };
        let buf = &self.buf;
        let wake_at = if self.dirty {
            r.tick(now_ns, || buf.to_text())
        } else {
            None
        };
        if let Some(reason) = r.take_error() {
            self.say_as(
                strings::recovery_failed(&reason),
                MsgKind::Warning,
                status::MESSAGE_FOR_NS,
            );
        }
        Effects {
            wake_at,
            ..Effects::default()
        }
    }

    /// A deliberate way out: this window's copy goes, and none is written
    /// from now on. `close` does this; a shell that exits without closing
    /// the editor (a bench run at its deadline) calls it first.
    pub fn discard_recovery(&mut self) {
        if let Some(mut r) = self.recovery.take() {
            r.delete();
        }
    }

    /// The process was sent signal `s`: the copy is written now, if the
    /// document is dirty, and kept; then the window closes with 128 plus
    /// the signal's number, letting the lock record go.
    pub(super) fn close_on_signal(&mut self, s: SignalKind) -> Effects {
        if let Some(mut r) = self.recovery.take() {
            if !self.dirty {
                r.delete();
            } else if let Err(e) = r.write_now(&self.buf.to_text()) {
                let why = strings::recovery_failed(&e.to_string());
                eprintln!("{}: {why}", app_meta::SLUG);
            }
        }
        self.close(s.exit_code())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command};
    use std::time::{Duration, SystemTime};

    use crate::editor::autosave::AUTOSAVE_IDLE_NS;
    use crate::editor::{Config, Editor, OpenRequest};
    use crate::file::StateDir;
    use crate::file::lock::fnv1a64;
    use crate::file::recovery::{RECOVERY_IDLE_NS, Recovery, find_newer};
    use crate::host::WindowRequest;
    use crate::input::{Input, SignalKind};
    use crate::platform::Platform;
    use crate::status::{self, Mode};
    use crate::strings;
    use crate::testkit::{FakeHost, ViewSource, keys};
    use crate::view_model::{MsgKind, StatusLeft};

    /// The clock's reading when each test starts.
    const START: u64 = 1_000;

    /// A temporary directory with `doc.md` (holding `hello`, modified an
    /// hour ago) and a state directory beside it.
    struct Fixture {
        dir: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let f = Fixture { dir };
            fs::write(f.doc(), "hello").unwrap();
            f.set_doc_mtime(SystemTime::now() - Duration::from_secs(3600));
            f
        }
        fn doc(&self) -> PathBuf {
            self.dir.path().join("doc.md")
        }
        fn state(&self) -> StateDir {
            StateDir::at(self.dir.path().join("state"))
        }
        fn set_doc_mtime(&self, t: SystemTime) {
            let file = fs::File::options().write(true).open(self.doc()).unwrap();
            file.set_modified(t).unwrap();
        }
        /// Where this process's copy of the document goes.
        fn own_copy(&self) -> PathBuf {
            Recovery::new(&self.state(), &self.doc(), std::process::id())
                .text_path()
                .to_owned()
        }
        /// The text of this process's copy, if there is one.
        fn copy(&self) -> Option<String> {
            fs::read_to_string(self.own_copy()).ok()
        }
        /// Every file in the recovery folder.
        fn copies(&self) -> Vec<String> {
            let Ok(entries) = fs::read_dir(self.state().recovery()) else {
                return Vec::new();
            };
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        }
        /// Leaves a copy holding `text`, as a window that has since gone
        /// would have. Returns the copy's text file.
        fn plant(&self, text: &str) -> PathBuf {
            let mut child = Command::new("true").spawn().unwrap();
            let dead = child.id();
            child.wait().unwrap();
            let mut r = Recovery::new(&self.state(), &self.doc(), dead);
            r.write_now(text).unwrap();
            r.text_path().to_owned()
        }
        fn lock_records(&self) -> usize {
            fs::read_dir(self.state().locks()).map_or(0, Iterator::count)
        }
    }

    /// A child process that holds the file, killed and reaped on drop.
    struct Holder(Child);
    impl Holder {
        fn spawn() -> Holder {
            Holder(Command::new("sleep").arg("60").spawn().unwrap())
        }
        /// Writes the lock record this process would have for `doc`.
        fn hold(&self, f: &Fixture) {
            let state = f.state();
            state.ensure().unwrap();
            let canon = fs::canonicalize(f.doc()).unwrap();
            let hash = fnv1a64(canon.to_string_lossy().as_bytes());
            let pid = self.0.id();
            let rec = state.locks().join(format!("{hash:016x}-{pid}.lock"));
            fs::write(rec, format!("{pid}\n{}\n", canon.display())).unwrap();
        }
    }
    impl Drop for Holder {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn open_at(
        path: &Path,
        state: Option<&StateDir>,
        cfg: Config,
        view: bool,
    ) -> (Editor, FakeHost) {
        let host = FakeHost::default();
        host.state().now = START;
        let req = OpenRequest {
            path: path.to_owned(),
            view,
            display_name: None,
        };
        let e = Editor::open_with_state(req, cfg, Platform::Mac, Box::new(host.clone()), state);
        (e, host)
    }

    fn open(f: &Fixture) -> (Editor, FakeHost) {
        open_at(&f.doc(), Some(&f.state()), Config::default(), false)
    }

    /// Sets the clock to `at` and ticks, as the shell's timer does, then
    /// waits for the copy's worker.
    fn tick(e: &mut Editor, host: &FakeHost, at: u64) -> crate::effects::Effects {
        host.state().now = at;
        let fx = e.handle(Input::Tick { now_ns: at });
        settle(e);
        fx
    }

    fn settle(e: &Editor) {
        if let Some(r) = &e.recovery {
            r.settle();
        }
    }

    /// Types `typed` at the start and lets the copy be written.
    fn type_and_wait(e: &mut Editor, host: &FakeHost, typed: &str) {
        let now = host.state().now;
        let fx = keys(e, typed);
        assert_eq!(fx.wake_at, Some(now + RECOVERY_IDLE_NS), "{typed}");
        tick(e, host, now + RECOVERY_IDLE_NS);
    }

    fn asks_recovery(e: &Editor) -> bool {
        e.mode() == Mode::Prompt
            && e.status().left == StatusLeft::Prompt(strings::RECOVERY_PROMPT.into())
    }

    #[test]
    fn a_change_writes_the_copy_2s_later() {
        let f = Fixture::new();
        let (mut e, host) = open(&f);
        // Nothing until there is a change.
        assert!(f.copies().is_empty());
        let fx = keys(&mut e, "x");
        assert_eq!(fx.wake_at, Some(START + RECOVERY_IDLE_NS));
        // Not yet.
        let fx = tick(&mut e, &host, START + RECOVERY_IDLE_NS - 1);
        assert_eq!(fx.wake_at, Some(START + RECOVERY_IDLE_NS));
        assert_eq!(f.copy(), None);
        tick(&mut e, &host, START + RECOVERY_IDLE_NS);
        assert_eq!(f.copy().as_deref(), Some("ello"));
        // Another window, finding it, would see it is this one's.
        let found = find_newer(&f.state(), &f.doc(), None, |_| false).unwrap();
        assert_eq!(found.text_path, f.own_copy());
        assert!(find_newer(&f.state(), &f.doc(), None, |pid| pid == std::process::id()).is_none());
    }

    #[test]
    fn deleted_on_write_and_on_discard() {
        // Each way the document is written, or deliberately left.
        let ways: &[(&str, bool)] = &[
            (":w<CR>", true),
            ("<D-s>", true),
            (":wq<CR>", true),
            (":x<CR>", true),
            ("ZZ", true),
            (":q!<CR>", false),
            ("ZQ", false),
            (":e!<CR>", false),
            (":cq<CR>", false),
            // Undoing back to the save point: clean again.
            ("u", false),
        ];
        for &(typed, writes) in ways {
            let f = Fixture::new();
            let (mut e, host) = open(&f);
            type_and_wait(&mut e, &host, "x");
            assert_eq!(f.copy().as_deref(), Some("ello"), "{typed}");
            keys(&mut e, typed);
            settle(&e);
            assert_eq!(f.copy(), None, "{typed}");
            assert!(f.copies().is_empty(), "{typed}: {:?}", f.copies());
            let disk = fs::read_to_string(f.doc()).unwrap();
            assert_eq!(disk, if writes { "ello" } else { "hello" }, "{typed}");
        }
        // The close prompt's save and discard.
        for answer in ["s", "d"] {
            let f = Fixture::new();
            let (mut e, host) = open(&f);
            type_and_wait(&mut e, &host, "x");
            e.handle(Input::CloseRequested);
            assert_eq!(f.copy().as_deref(), Some("ello"), "{answer}");
            assert_eq!(keys(&mut e, answer).close, Some(0), "{answer}");
            assert!(f.copies().is_empty(), "{answer}");
        }
        // The recovery prompt's discard is `discard_takes_the_copy_away`.
    }

    #[test]
    fn autosave_deletes_the_copy() {
        let f = Fixture::new();
        let cfg = Config {
            autosave: true,
            ..Config::default()
        };
        let (mut e, host) = open_at(&f.doc(), Some(&f.state()), cfg, false);
        // An Insert session holds autosave, never the copy.
        keys(&mut e, "ihey");
        tick(&mut e, &host, START + RECOVERY_IDLE_NS);
        assert_eq!(f.copy().as_deref(), Some("heyhello"));
        // Leaving Insert lets the autosave due since 1 s go.
        let fx = keys(&mut e, "<Esc>");
        assert_eq!(fx.wake_at, Some(START + AUTOSAVE_IDLE_NS));
        tick(&mut e, &host, START + RECOVERY_IDLE_NS);
        assert!(!e.dirty());
        assert_eq!(fs::read_to_string(f.doc()).unwrap(), "heyhello");
        assert!(f.copies().is_empty(), "{:?}", f.copies());
    }

    #[test]
    fn sigterm_writes_copy_and_exits_143() {
        let f = Fixture::new();
        let (mut e, _host) = open(&f);
        assert_eq!(f.lock_records(), 1);
        // Typed just now: no copy written yet.
        keys(&mut e, "xx");
        assert_eq!(f.copy(), None);
        let fx = e.handle(Input::Signal(SignalKind::Terminate));
        assert_eq!(fx.close, Some(143));
        // Written at once, on this thread, and kept.
        assert_eq!(f.copy().as_deref(), Some("llo"));
        assert_eq!(f.lock_records(), 0);
        drop(e);
        assert_eq!(f.copy().as_deref(), Some("llo"));
        // The next open (this pid now standing in for a new process)
        // would find it, as it is newer than the file.
        let found = find_newer(&f.state(), &f.doc(), Some(0), |_| false).unwrap();
        assert_eq!(fs::read_to_string(found.text_path).unwrap(), "llo");

        // A clean document leaves no copy, even one written before.
        let f = Fixture::new();
        let (mut e, host) = open(&f);
        type_and_wait(&mut e, &host, "x");
        keys(&mut e, "u");
        let fx = e.handle(Input::Signal(SignalKind::Terminate));
        assert_eq!(fx.close, Some(143));
        assert!(f.copies().is_empty());

        // With no state directory there is nothing to write.
        let f = Fixture::new();
        let (mut e, _host) = open_at(&f.doc(), None, Config::default(), false);
        keys(&mut e, "x");
        assert_eq!(
            e.handle(Input::Signal(SignalKind::Terminate)).close,
            Some(143)
        );
        assert!(!f.state().root().exists());
    }

    #[test]
    fn compare_spawns_reader_window_and_keeps_prompt() {
        let f = Fixture::new();
        let planted = f.plant("from the copy");
        let (mut e, host) = open(&f);
        assert!(asks_recovery(&e));
        assert_eq!(e.prompt().as_deref(), Some(strings::RECOVERY_PROMPT));
        assert!(!e.ime_active());
        let fx = keys(&mut e, "c");
        assert_eq!(fx.close, None);
        let want = WindowRequest {
            path: planted.clone(),
            view: true,
            display_name: Some("doc.md (recovery)".into()),
        };
        assert_eq!(host.state().spawned, [want]);
        assert!(asks_recovery(&e));
        // Nothing else answers: not Esc, nor a modified key, nor editing.
        keys(&mut e, "<Esc>x<D-r><C-d>q");
        assert!(asks_recovery(&e));
        assert_eq!(e.text(), "hello");
        assert!(planted.exists());
        assert_eq!(host.state().spawned.len(), 1);
    }

    #[test]
    fn discard_takes_the_copy_away() {
        let f = Fixture::new();
        let planted = f.plant("from the copy");
        let (mut e, _host) = open(&f);
        let fx = keys(&mut e, "D");
        assert_eq!(fx.close, None);
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.prompt(), None);
        assert_eq!(e.text(), "hello");
        assert!(!e.dirty());
        assert!(!planted.exists());
        assert!(f.copies().is_empty(), "{:?}", f.copies());
        // Never asked again.
        drop(e);
        let (e, _host) = open(&f);
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn restore_replaces_the_text_as_one_undo_step_and_stays_dirty() {
        let f = Fixture::new();
        let planted = f.plant("one\ntwo");
        let (mut e, _host) = open(&f);
        let fx = keys(&mut e, "r");
        assert_eq!(fx.close, None);
        assert_eq!(fx.dirty, Some(true));
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.text(), "one\ntwo");
        assert!(e.dirty());
        // This window's own copy holds the text before the old one goes.
        assert!(!planted.exists());
        assert_eq!(f.copy().as_deref(), Some("one\ntwo"));
        // One undo step takes it back, clean, and the copy goes.
        keys(&mut e, "u");
        assert_eq!(e.text(), "hello");
        assert!(!e.dirty());
        settle(&e);
        assert!(f.copies().is_empty(), "{:?}", f.copies());
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "one\ntwo");
        assert!(e.dirty());
    }

    #[test]
    fn restore_with_vim_off() {
        let f = Fixture::new();
        f.plant("restored");
        let cfg = Config {
            vim: false,
            ..Config::default()
        };
        let (mut e, _host) = open_at(&f.doc(), Some(&f.state()), cfg, false);
        assert!(asks_recovery(&e));
        e.handle(Input::Text("r".into()));
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.text(), "restored");
        assert!(e.dirty());
    }

    #[test]
    fn recovery_prompt_follows_the_lock_prompt() {
        for answer in ["e", "o", "q"] {
            let f = Fixture::new();
            let planted = f.plant("from the copy");
            let holder = Holder::spawn();
            holder.hold(&f);
            let (mut e, _host) = open(&f);
            assert_eq!(
                e.prompt(),
                Some(strings::already_open(holder.0.id())),
                "{answer}"
            );
            let fx = keys(&mut e, answer);
            match answer {
                "e" => {
                    assert!(asks_recovery(&e));
                    keys(&mut e, "r");
                    assert_eq!(e.text(), "from the copy");
                    assert!(!planted.exists());
                }
                // Read-only cannot take the copy: not asked, and kept.
                "o" => {
                    assert_eq!(e.mode(), Mode::Normal);
                    assert_eq!(e.prompt(), None);
                    assert_eq!(e.status().warnings, [strings::READ_ONLY]);
                    assert!(planted.exists());
                }
                _ => {
                    assert_eq!(fx.close, Some(0));
                    assert!(planted.exists());
                }
            }
        }
    }

    #[test]
    fn the_lock_warning_waits_for_the_recovery_prompt() {
        let f = Fixture::new();
        f.plant("from the copy");
        // A directory where this process's lock record goes: no lock.
        let state = f.state();
        state.ensure().unwrap();
        let canon = fs::canonicalize(f.doc()).unwrap();
        let hash = fnv1a64(canon.to_string_lossy().as_bytes());
        let ours = state
            .locks()
            .join(format!("{hash:016x}-{}.lock", std::process::id()));
        fs::create_dir(&ours).unwrap();
        let reason = fs::write(&ours, "").unwrap_err().to_string();
        let (mut e, _host) = open(&f);
        assert!(asks_recovery(&e));
        let fx = keys(&mut e, "d");
        assert_eq!(
            e.status().left,
            StatusLeft::Message {
                text: strings::lock_unavailable(&reason),
                kind: MsgKind::Warning,
            }
        );
        assert_eq!(fx.wake_at, Some(START + status::MESSAGE_FOR_NS));
    }

    #[test]
    fn stale_live_and_reader_copies_ask_nothing() {
        // Older than the file on disk.
        let f = Fixture::new();
        let planted = f.plant("old");
        f.set_doc_mtime(SystemTime::now() + Duration::from_secs(3600));
        let (e, _host) = open(&f);
        assert_eq!(e.mode(), Mode::Normal);
        assert!(planted.exists());

        // A live window's copy: it is still being edited.
        let f = Fixture::new();
        let holder = Holder::spawn();
        let mut r = Recovery::new(&f.state(), &f.doc(), holder.0.id());
        r.write_now("theirs").unwrap();
        let (e, _host) = open(&f);
        assert_eq!(e.mode(), Mode::Normal);

        // Reader view cannot take it: kept for later.
        let f = Fixture::new();
        let planted = f.plant("from the copy");
        let (e, _host) = open_at(&f.doc(), Some(&f.state()), Config::default(), true);
        assert_eq!(e.prompt(), None);
        assert!(planted.exists());

        // Another file's copy is not this one's.
        let f = Fixture::new();
        f.plant("from the copy");
        let other = f.dir.path().join("other.md");
        fs::write(&other, "x").unwrap();
        let (e, _host) = open_at(&other, Some(&f.state()), Config::default(), false);
        assert_eq!(e.prompt(), None);
    }

    #[test]
    fn a_missing_file_with_a_copy_asks() {
        let f = Fixture::new();
        f.plant("never saved");
        fs::remove_file(f.doc()).unwrap();
        let (mut e, _host) = open(&f);
        assert!(asks_recovery(&e));
        keys(&mut e, "r");
        assert_eq!(e.text(), "never saved");
    }

    #[test]
    fn no_state_dir_writes_no_copies() {
        let f = Fixture::new();
        let (mut e, host) = open_at(&f.doc(), None, Config::default(), false);
        assert!(e.recovery.is_none());
        let fx = keys(&mut e, "x");
        assert_eq!(fx.wake_at, None);
        tick(&mut e, &host, START + 60 * RECOVERY_IDLE_NS);
        // Only the lock's warning was ever said, and it has faded.
        assert_eq!(e.message(), None);
        assert!(!f.state().root().exists());
    }

    #[test]
    fn a_failed_copy_write_warns_once() {
        let f = Fixture::new();
        let (mut e, host) = open(&f);
        // The recovery folder is replaced by a file: writes fail.
        fs::remove_dir(f.state().recovery()).unwrap();
        fs::write(f.state().recovery(), "x").unwrap();
        type_and_wait(&mut e, &host, "x");
        let at = START + RECOVERY_IDLE_NS + 1;
        tick(&mut e, &host, at);
        let said = match e.status().left {
            StatusLeft::Message {
                text,
                kind: MsgKind::Warning,
            } => text,
            other => panic!("{other:?}"),
        };
        assert!(said.starts_with(&strings::recovery_failed("")), "{said}");
        // Once it fades, more failures say nothing more.
        let later = at + status::MESSAGE_FOR_NS;
        tick(&mut e, &host, later);
        assert_eq!(e.message(), None);
        host.state().now = later;
        type_and_wait(&mut e, &host, "x");
        tick(&mut e, &host, later + RECOVERY_IDLE_NS + 1);
        assert_eq!(e.message(), None);
    }
}

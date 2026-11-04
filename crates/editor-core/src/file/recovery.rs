//! Recovery copies of unsaved text, so a window that dies (a crash, a
//! SIGKILL, a power cut) loses at most a few seconds of typing.
//!
//! A copy is two files in the state directory's `recovery` folder, keyed
//! like the lock records (`crate::file::lock`) by the FNV-1a hash of the
//! file's canonical path and the pid of the window that wrote it:
//! `<hash>-<pid>.txt` holds the text and `<hash>-<pid>.toml` says whose it
//! is (`path`, `written_at_ns` on the wall clock, `pid`). Each window
//! writes only its own pair, so two windows on one file never share a
//! copy.
//!
//! Timing (`schedule`, `tick`, on the host's monotonic clock): a copy is
//! written `RECOVERY_IDLE_NS` after the last change, and while changes
//! keep coming no change waits more than `RECOVERY_MAX_NS` to be written.
//! The writes happen on a worker thread, started at the first one. Every
//! write goes to a temporary file that is renamed into place, and one
//! lock covers each write, `write_now` and `delete`, so a synchronous write
//! (SIGTERM) or a delete never races a write the worker has under way;
//! queued writes from before either are dropped.
//!
//! `find_newer` is the scan at open: a copy left by a process that is no
//! longer alive, written after the file on disk was last modified.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use super::lock::{absolute_key_path, fnv1a64};
use super::statedir::StateDir;
use crate::host::Waker;

/// How long after the last change a copy is written.
pub const RECOVERY_IDLE_NS: u64 = 2_000_000_000;

/// The longest a change waits to be written while changes keep coming.
pub const RECOVERY_MAX_NS: u64 = 10_000_000_000;

/// A copy `find_newer` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The copy's text.
    pub text_path: PathBuf,
    /// When it was written, in nanoseconds since the epoch.
    pub written_at_ns: i64,
}

impl Found {
    /// The copy's metadata file, beside its text.
    pub fn meta_path(&self) -> PathBuf {
        self.text_path.with_extension("toml")
    }

    /// Removes the copy (metadata first, so a half-removed copy is never
    /// found). A copy already gone is fine.
    pub fn delete(&self) -> io::Result<()> {
        remove(&self.meta_path())?;
        remove(&self.text_path)
    }
}

/// What a write on the worker is for.
enum Job {
    /// The text, and the generation it was queued in.
    Write { generation: u64, text: String },
    /// Answered once every job before it is done (tests).
    #[cfg(test)]
    Settle(Sender<()>),
}

/// What the editor's side and the worker share.
struct Shared {
    /// Bumped by `write_now`, `delete` and drop: writes queued before are
    /// dropped.
    generation: AtomicU64,
    /// Held across each write and each delete.
    files: Mutex<Outcome>,
}

/// How the writes are going.
#[derive(Debug, Default)]
struct Outcome {
    /// Why the last write failed, while it is the last word.
    error: Option<String>,
    /// That error was handed to the editor already.
    reported: bool,
    /// Writes that succeeded (tests count them).
    written: u64,
}

impl Outcome {
    /// Takes a write's result. True when it failed and that is news.
    fn record(&mut self, r: io::Result<()>) -> bool {
        match r {
            Ok(()) => {
                self.written += 1;
                self.error = None;
                self.reported = false;
                false
            }
            Err(e) => {
                self.error = Some(e.to_string());
                !self.reported
            }
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Outcome> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Where one window's copy of one file goes.
#[derive(Debug, Clone)]
struct Target {
    state: StateDir,
    text: PathBuf,
    meta: PathBuf,
    /// The canonical path the copy is of, as the metadata records it.
    key: String,
    pid: u32,
}

/// One window's recovery copy of one file.
pub struct Recovery {
    target: Target,
    /// The first change not yet written, and the last change.
    pending: Option<(u64, u64)>,
    /// A copy may be on disk (written, or queued to be).
    on_disk: bool,
    shared: Arc<Shared>,
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
    waker: Option<Waker>,
}

impl Recovery {
    /// The copy `pid` keeps of `path` in `state`. Nothing is written, and
    /// no thread started, until the first write.
    pub fn new(state: &StateDir, path: &Path, pid: u32) -> Recovery {
        let key = absolute_key_path(path).to_string_lossy().into_owned();
        let hash = format!("{:016x}", fnv1a64(key.as_bytes()));
        let dir = state.recovery();
        Recovery {
            target: Target {
                state: state.clone(),
                text: dir.join(format!("{hash}-{pid}.txt")),
                meta: dir.join(format!("{hash}-{pid}.toml")),
                key,
                pid,
            },
            pending: None,
            on_disk: false,
            shared: Arc::new(Shared {
                generation: AtomicU64::new(0),
                files: Mutex::new(Outcome::default()),
            }),
            jobs: None,
            thread: None,
            waker: None,
        }
    }

    /// Calls `waker` after a failed write, so the editor can say so.
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// Where the copy's text goes.
    pub fn text_path(&self) -> &Path {
        &self.target.text
    }

    /// Where the copy's metadata goes.
    pub fn meta_path(&self) -> &Path {
        &self.target.meta
    }

    /// The document changed at `now_ns`. Returns when to `tick` next, if
    /// this change starts a new wait; a later change only moves the time,
    /// which the tick then finds. A change that has waited
    /// `RECOVERY_MAX_NS` since the first one not written has the copy
    /// written now, from `text_snapshot`.
    pub fn schedule(&mut self, now_ns: u64, text_snapshot: impl FnOnce() -> String) -> Option<u64> {
        let fresh = self.pending.is_none();
        let first = self.pending.map_or(now_ns, |(first, _)| first);
        if now_ns.saturating_sub(first) >= RECOVERY_MAX_NS {
            self.pending = None;
            self.write_async(text_snapshot());
            return None;
        }
        self.pending = Some((first, now_ns));
        if fresh { self.due() } else { None }
    }

    /// The main loop woke at `now_ns`: writes the copy (from
    /// `text_snapshot`) when it is due. One not due yet returns when it is.
    pub fn tick(&mut self, now_ns: u64, text_snapshot: impl FnOnce() -> String) -> Option<u64> {
        let due = self.due()?;
        if now_ns < due {
            return Some(due);
        }
        self.pending = None;
        self.write_async(text_snapshot());
        None
    }

    /// When the copy waiting is due, if one is.
    fn due(&self) -> Option<u64> {
        self.pending.map(|(first, last)| {
            last.saturating_add(RECOVERY_IDLE_NS)
                .min(first.saturating_add(RECOVERY_MAX_NS))
        })
    }

    /// True while a change waits to be written.
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Hands `text` to the worker, starting it the first time.
    fn write_async(&mut self, text: String) {
        self.on_disk = true;
        let generation = self.shared.generation.load(Ordering::Acquire);
        if self.jobs.is_none() {
            let (tx, rx) = mpsc::channel();
            let (target, shared) = (self.target.clone(), Arc::clone(&self.shared));
            let waker = self.waker.clone();
            let spawned = thread::Builder::new()
                .name("recovery".to_owned())
                .spawn(move || work(&target, &shared, &rx, waker.as_ref()));
            match spawned {
                Ok(t) => {
                    self.jobs = Some(tx);
                    self.thread = Some(t);
                }
                // No thread: write here, rather than not at all.
                Err(_) => {
                    let mut out = self.shared.lock();
                    out.record(write_files(&self.target, &text, wall_now_ns()));
                    return;
                }
            }
        }
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::Write { generation, text });
        }
    }

    /// Writes `text` as the copy now, on this thread (SIGTERM). Writes
    /// queued before it are dropped, and one under way finishes first.
    pub fn write_now(&mut self, text: &str) -> io::Result<()> {
        self.pending = None;
        self.on_disk = true;
        let mut out = self.shared.lock();
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        let r = write_files(&self.target, text, wall_now_ns());
        out.record(r.as_ref().map(|_| ()).map_err(clone_error));
        // The caller says why itself.
        if r.is_err() {
            out.reported = true;
        }
        r
    }

    /// Removes the copy, and drops whatever was waiting or queued. Does
    /// nothing on disk when nothing was ever written.
    pub fn delete(&mut self) {
        self.pending = None;
        if !self.on_disk {
            return;
        }
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        let _held = self.shared.lock();
        let _ = remove(&self.target.meta);
        let _ = remove(&self.target.text);
        self.on_disk = false;
    }

    /// Why the last write failed, once per run of failures: a write that
    /// succeeds makes the next failure news again.
    pub fn take_error(&mut self) -> Option<String> {
        let mut out = self.shared.lock();
        if out.reported {
            return None;
        }
        out.reported = out.error.is_some();
        out.error.clone()
    }

    /// Waits until the worker has done every job queued so far (tests).
    #[cfg(test)]
    pub(crate) fn settle(&self) {
        let Some(jobs) = &self.jobs else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        if jobs.send(Job::Settle(tx)).is_ok() {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
        }
    }

    /// How many writes have succeeded (tests).
    #[cfg(test)]
    pub(crate) fn written(&self) -> u64 {
        self.shared.lock().written
    }
}

/// Stops the worker without waiting for it: closing never blocks on a
/// write. Writes still queued are dropped; one under way finishes.
impl Drop for Recovery {
    fn drop(&mut self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.jobs = None;
        drop(self.thread.take());
    }
}

/// The newest copy of `path` in `state` whose writer is no longer alive
/// (`live` says whether a pid is) and that was written after the file on
/// disk was last modified (`file_mtime_ns`, nanoseconds since the epoch;
/// `None` when there is no file, so any copy is newer).
pub fn find_newer(
    state: &StateDir,
    path: &Path,
    file_mtime_ns: Option<i64>,
    live: impl Fn(u32) -> bool,
) -> Option<Found> {
    let key = absolute_key_path(path).to_string_lossy().into_owned();
    let hash = format!("{:016x}", fnv1a64(key.as_bytes()));
    let mut best: Option<Found> = None;
    for entry in fs::read_dir(state.recovery()).ok()?.flatten() {
        let name = entry.file_name();
        let Some(pid) = copy_pid(&name.to_string_lossy(), &hash) else {
            continue;
        };
        if live(pid) {
            continue;
        }
        let Some(written_at_ns) = read_meta(&entry.path(), &key, pid) else {
            continue;
        };
        let older = file_mtime_ns.is_some_and(|m| written_at_ns <= m);
        let beaten = best
            .as_ref()
            .is_some_and(|b| b.written_at_ns >= written_at_ns);
        let text_path = entry.path().with_extension("txt");
        if older || beaten || !text_path.is_file() {
            continue;
        }
        best = Some(Found {
            text_path,
            written_at_ns,
        });
    }
    best
}

/// The pid in a metadata name `<hash>-<pid>.toml` for this `hash`.
fn copy_pid(name: &str, hash: &str) -> Option<u32> {
    let rest = name.strip_prefix(hash)?.strip_prefix('-')?;
    rest.strip_suffix(".toml")?.parse().ok()
}

/// `written_at_ns` from the metadata at `meta`, when it is a copy of the
/// file `key` by `pid`.
fn read_meta(meta: &Path, key: &str, pid: u32) -> Option<i64> {
    let table: toml::Table = fs::read_to_string(meta).ok()?.parse().ok()?;
    let ours =
        table.get("path")?.as_str()? == key && table.get("pid")?.as_integer()? == i64::from(pid);
    if !ours {
        return None;
    }
    table.get("written_at_ns")?.as_integer()
}

/// The worker: does each job in turn until the `Recovery` is dropped. A
/// write queued before the latest `write_now`, `delete` or drop is
/// dropped.
fn work(target: &Target, shared: &Shared, jobs: &Receiver<Job>, waker: Option<&Waker>) {
    for job in jobs {
        match job {
            Job::Write { generation, text } => {
                let mut out = shared.lock();
                if shared.generation.load(Ordering::Acquire) != generation {
                    continue;
                }
                let news = out.record(write_files(target, &text, wall_now_ns()));
                drop(out);
                if news && let Some(wake) = waker {
                    wake();
                }
            }
            #[cfg(test)]
            Job::Settle(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// Nanoseconds since the epoch, as file modification times count them.
fn wall_now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// An `io::Error` like `e` (its kind and message), to keep beside it.
fn clone_error(e: &io::Error) -> io::Error {
    io::Error::new(e.kind(), e.to_string())
}

/// Writes `text` and its metadata to `target`, each to a temporary file
/// renamed into place: the text first, so metadata never names a text
/// that is not there.
fn write_files(target: &Target, text: &str, written_at_ns: i64) -> io::Result<()> {
    target.state.ensure()?;
    let mut meta = toml::Table::new();
    meta.insert("path".into(), target.key.clone().into());
    meta.insert("written_at_ns".into(), written_at_ns.into());
    meta.insert("pid".into(), i64::from(target.pid).into());
    replace(&target.text, text.as_bytes())?;
    replace(&target.meta, meta.to_string().as_bytes())
}

/// Puts `bytes` at `path` through a temporary file beside it, readable
/// only by the user.
fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut opts = OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let written = opts.open(&tmp).and_then(|mut f| f.write_all(bytes));
    if let Err(e) = written.and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Removes `path`; one already gone is fine.
fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const T0: u64 = 5_000;
    const SEC: u64 = 1_000_000_000;

    /// A temporary directory with `doc.md` and a state directory beside it.
    struct Fixture {
        dir: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("doc.md"), "on disk").unwrap();
            Fixture { dir }
        }
        fn doc(&self) -> PathBuf {
            self.dir.path().join("doc.md")
        }
        fn state(&self) -> StateDir {
            StateDir::at(self.dir.path().join("state"))
        }
        fn copies(&self) -> Vec<String> {
            let Ok(entries) = fs::read_dir(self.state().recovery()) else {
                return Vec::new();
            };
            let mut v: Vec<String> = entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    fn text(r: &Recovery) -> String {
        fs::read_to_string(r.text_path()).unwrap()
    }

    fn meta(r: &Recovery) -> toml::Table {
        fs::read_to_string(r.meta_path()).unwrap().parse().unwrap()
    }

    fn never() -> String {
        panic!("no snapshot is due")
    }

    #[test]
    fn writes_after_2s_idle() {
        let f = Fixture::new();
        let mut r = Recovery::new(&f.state(), &f.doc(), 4242);
        // Nothing on disk, and no thread, until a write.
        assert!(f.copies().is_empty());
        assert_eq!(r.schedule(T0, never), Some(T0 + RECOVERY_IDLE_NS));
        // A later change moves the time; the tick finds it.
        assert_eq!(r.schedule(T0 + SEC / 2, never), None);
        let moved = T0 + SEC / 2 + RECOVERY_IDLE_NS;
        assert_eq!(r.tick(T0 + RECOVERY_IDLE_NS, never), Some(moved));
        assert_eq!(r.tick(moved - 1, never), Some(moved));
        r.settle();
        assert!(f.copies().is_empty());
        assert_eq!(r.tick(moved, || "typed".into()), None);
        r.settle();
        assert_eq!(text(&r), "typed");
        let m = meta(&r);
        let canon = fs::canonicalize(f.doc()).unwrap();
        assert_eq!(
            m.get("path").and_then(|v| v.as_str()),
            canon.to_str(),
            "{m:?}"
        );
        assert_eq!(m.get("pid").and_then(|v| v.as_integer()), Some(4242));
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let at = m.get("written_at_ns").and_then(|v| v.as_integer()).unwrap();
        let now = i64::try_from(now.as_nanos()).unwrap();
        assert!(at > 0 && at <= now, "{at} {now}");
        // The names follow the lock records' key.
        let hash = format!("{:016x}", fnv1a64(canon.to_string_lossy().as_bytes()));
        assert_eq!(
            f.copies(),
            [format!("{hash}-4242.toml"), format!("{hash}-4242.txt")]
        );
        // Nothing more is due until the next change.
        assert_eq!(r.tick(moved + 20 * SEC, never), None);
        assert_eq!(r.written(), 1);
        // The copy is private.
        use std::os::unix::fs::PermissionsExt;
        for p in [r.text_path(), r.meta_path()] {
            let mode = fs::metadata(p).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{p:?}");
        }
    }

    #[test]
    fn writes_every_10s_while_typing() {
        let f = Fixture::new();
        // A change every second for 25 s: never 2 s idle. The shell's
        // timer ticks when asked; the 10 s limit is met by the tick.
        let mut r = Recovery::new(&f.state(), &f.doc(), 7);
        let mut wake = None;
        for i in 0..=25u64 {
            let now = T0 + i * SEC;
            if let Some(at) = wake.filter(|&at| at <= now) {
                wake = r.tick(at, || format!("tick {i}"));
            }
            if let Some(at) = r.schedule(now, || format!("v{i}")) {
                wake = Some(at);
            }
            r.settle();
            let want = match i {
                0..=9 => 0,
                10..=19 => 1,
                _ => 2,
            };
            assert_eq!(r.written(), want, "after the change at {i} s");
        }
        // The change at 10 s waited the longest: 10 s, to the tick at 20.
        assert_eq!(text(&r), "tick 20");
        // Once the changes stop, the last ones go 2 s later.
        let last = T0 + 25 * SEC;
        assert_eq!(r.tick(last, never), Some(last + RECOVERY_IDLE_NS));
        assert_eq!(r.tick(last + RECOVERY_IDLE_NS, || "end".into()), None);
        r.settle();
        assert_eq!(text(&r), "end");
        assert_eq!(r.written(), 3);

        // With no tick at all (a timer that never fired), a change itself
        // writes once the first change not written is 10 s old.
        let mut r = Recovery::new(&f.state(), &f.doc(), 8);
        for i in 0..=25u64 {
            r.schedule(T0 + i * SEC, || format!("v{i}"));
            r.settle();
            let want = match i {
                0..=9 => 0,
                10..=20 => 1,
                _ => 2,
            };
            assert_eq!(r.written(), want, "after the change at {i} s");
        }
        assert_eq!(text(&r), "v21");
    }

    #[test]
    fn two_pids_never_share_a_copy() {
        let f = Fixture::new();
        let mut a = Recovery::new(&f.state(), &f.doc(), 100);
        let mut b = Recovery::new(&f.state(), &f.doc(), 200);
        assert_ne!(a.text_path(), b.text_path());
        assert_ne!(a.meta_path(), b.meta_path());
        a.write_now("from a").unwrap();
        b.write_now("from b").unwrap();
        assert_eq!(text(&a), "from a");
        assert_eq!(text(&b), "from b");
        assert_eq!(f.copies().len(), 4);
        a.delete();
        assert_eq!(text(&b), "from b");
        assert_eq!(f.copies().len(), 2);
    }

    #[test]
    fn write_now_wins_over_a_queued_write() {
        for _ in 0..20 {
            let f = Fixture::new();
            let mut r = Recovery::new(&f.state(), &f.doc(), 9);
            r.schedule(T0, never);
            r.tick(T0 + RECOVERY_IDLE_NS, || "queued".into());
            r.write_now("now").unwrap();
            r.settle();
            assert_eq!(text(&r), "now");
            // No temporary file is left beside the copy.
            assert_eq!(f.copies().len(), 2, "{:?}", f.copies());
        }
    }

    #[test]
    fn delete_drops_queued_writes_and_removes_the_copy() {
        let f = Fixture::new();
        let mut r = Recovery::new(&f.state(), &f.doc(), 9);
        r.write_now("first").unwrap();
        r.schedule(T0, never);
        r.tick(T0 + RECOVERY_IDLE_NS, || "queued".into());
        r.delete();
        r.settle();
        assert!(f.copies().is_empty(), "{:?}", f.copies());
        // Nothing is due after a delete.
        assert_eq!(r.tick(T0 + 60 * SEC, never), None);
        // Deleting with nothing written touches nothing.
        let mut fresh = Recovery::new(&f.state(), &f.doc(), 10);
        fresh.delete();
    }

    #[test]
    fn a_failed_write_is_reported_once() {
        let f = Fixture::new();
        // A regular file where the state directory must be made.
        fs::write(f.dir.path().join("state"), "x").unwrap();
        let mut r = Recovery::new(&f.state(), &f.doc(), 9);
        let woken = Arc::new(AtomicU64::new(0));
        let w = Arc::clone(&woken);
        r.set_waker(Arc::new(move || {
            w.fetch_add(1, Ordering::AcqRel);
        }));
        r.schedule(T0, never);
        r.tick(T0 + RECOVERY_IDLE_NS, || "a".into());
        r.settle();
        assert!(woken.load(Ordering::Acquire) >= 1);
        assert!(r.take_error().is_some());
        assert_eq!(r.take_error(), None);
        // Failing again is no news.
        r.schedule(T0 + 3 * SEC, never);
        r.tick(T0 + 5 * SEC, || "b".into());
        r.settle();
        assert_eq!(r.take_error(), None);
        // `write_now` says why itself.
        assert!(r.write_now("c").is_err());
    }

    #[test]
    fn drop_never_waits_and_drops_queued_writes() {
        let f = Fixture::new();
        let mut r = Recovery::new(&f.state(), &f.doc(), 9);
        r.write_now("first").unwrap();
        // As if a write were under way: the worker cannot start the one
        // queued below until this is let go.
        let shared = Arc::clone(&r.shared);
        let busy = shared.lock();
        r.schedule(T0, never);
        r.tick(T0 + RECOVERY_IDLE_NS, || "queued".into());
        // Dropping never waits for the write (this would deadlock if it
        // did), and the write queued is dropped.
        drop(r);
        drop(busy);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(fs::read_to_string(first_text(&f)).unwrap(), "first");
    }

    fn first_text(f: &Fixture) -> PathBuf {
        Recovery::new(&f.state(), &f.doc(), 9)
            .text_path()
            .to_owned()
    }

    /// Writes a copy as `pid` would have at `written_at_ns`.
    fn plant(f: &Fixture, pid: u32, written_at_ns: i64, body: &str) -> PathBuf {
        let r = Recovery::new(&f.state(), &f.doc(), pid);
        write_files(&r.target, body, written_at_ns).unwrap();
        r.text_path().to_owned()
    }

    #[test]
    fn find_newer_ignores_live_owner_and_older_copies() {
        let f = Fixture::new();
        let state = f.state();
        let dead = |_| false;
        assert_eq!(find_newer(&state, &f.doc(), Some(10), dead), None);
        let older = plant(&f, 101, 50, "older");
        let newest = plant(&f, 102, 300, "newest");
        let middle = plant(&f, 103, 200, "middle");
        // The newest copy of a dead process wins.
        let found = find_newer(&state, &f.doc(), Some(100), dead).unwrap();
        assert_eq!(
            found,
            Found {
                text_path: newest.clone(),
                written_at_ns: 300
            }
        );
        // A live owner's copy is not left over: it is being edited.
        let live = |pid| pid == 102;
        let found = find_newer(&state, &f.doc(), Some(100), live).unwrap();
        assert_eq!(found.text_path, middle);
        // Copies older than the file on disk are ignored.
        assert_eq!(find_newer(&state, &f.doc(), Some(300), dead), None);
        // With no file on disk, any copy is newer.
        let found = find_newer(&state, &f.doc(), None, |pid| pid != 101).unwrap();
        assert_eq!(found.text_path, older);
        // A copy whose metadata names another file (a hash collision) or
        // that is not a copy at all does not count, nor does another
        // file's copy.
        let other = f.dir.path().join("other.md");
        fs::write(&other, "x").unwrap();
        assert_eq!(find_newer(&state, &other, Some(0), dead), None);
        fs::write(
            newest.with_extension("toml"),
            "path = \"/else\"\nwritten_at_ns = 300\npid = 102\n",
        )
        .unwrap();
        let found = find_newer(&state, &f.doc(), Some(100), dead).unwrap();
        assert_eq!(found.text_path, middle);
        fs::write(state.recovery().join("junk.toml"), "x").unwrap();
        fs::write(state.recovery().join("notes.txt"), "x").unwrap();
        // A copy whose text is gone is no copy.
        fs::remove_file(&middle).unwrap();
        let found = find_newer(&state, &f.doc(), Some(10), dead).unwrap();
        assert_eq!(found.text_path, older);
        // Removing a found copy takes both files.
        found.delete().unwrap();
        assert!(!older.exists() && !older.with_extension("toml").exists());
        found.delete().unwrap();
    }

    #[test]
    fn no_recovery_folder_finds_nothing() {
        let f = Fixture::new();
        assert_eq!(find_newer(&f.state(), &f.doc(), None, |_| false), None);
    }
}

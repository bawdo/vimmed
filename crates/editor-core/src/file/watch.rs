//! Watching the open file for changes made outside the editor (a git
//! checkout, a formatter, another editor).
//!
//! `Watcher::spawn` starts a thread that stats the file every second (and
//! at once when `check_now` asks), compares what it finds with the stat
//! the document was read or last written at (`classify`), and reports each
//! new state once through `poll`, waking the shell's main loop to take it.
//! `rebase` gives it the stat after the editor's own write or reload, so
//! the editor's changes never come back as an external one.

use std::fs::{self, Metadata};
use std::io::{self, ErrorKind};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::file::DiskStat;
use crate::host::Waker;

/// How often the thread looks at the file.
const INTERVAL: Duration = Duration::from_secs(1);

/// What the watcher found on disk, against the stat it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskEvent {
    /// Another version of the file: replaced, resized or modified.
    Changed,
    /// No file at the path any more (deleted or moved away).
    Gone,
    /// The file is there but cannot be read.
    Unreadable,
    /// The file can be read but not written.
    ReadOnly,
    /// The file is fine again, after one of the others.
    Back,
}

/// What the file's metadata `new` (from `fs::metadata`, following links)
/// says against `old`: `None` while it is the same version and can be
/// read and written. A file that cannot be read is `Unreadable` before
/// anything else (its text cannot be reloaded); a new version comes
/// before a read-only one.
pub fn classify(old: &DiskStat, new: io::Result<Metadata>) -> Option<DiskEvent> {
    let m = match new {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => return Some(DiskEvent::Gone),
        Err(_) => return Some(DiskEvent::Unreadable),
    };
    let (read, write) = access(&m);
    if m.is_dir() || !read {
        return Some(DiskEvent::Unreadable);
    }
    if old.changed(&DiskStat::of(&m)) {
        return Some(DiskEvent::Changed);
    }
    (!write).then_some(DiskEvent::ReadOnly)
}

/// Whether this process may read and write the file `m` describes, by its
/// permission bits: the owner's when it is ours, the group's when we are
/// in its group, else everyone's. Root may do both.
fn access(m: &Metadata) -> (bool, bool) {
    // SAFETY: `geteuid` cannot fail or touch memory.
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return (true, true);
    }
    let bits = if m.uid() == euid {
        m.mode() >> 6
    } else if in_group(m.gid()) {
        m.mode() >> 3
    } else {
        m.mode()
    };
    (bits & 0o4 != 0, bits & 0o2 != 0)
}

/// True when `gid` is this process's group or one of its supplementary
/// groups.
fn in_group(gid: u32) -> bool {
    // SAFETY: `getegid` cannot fail or touch memory.
    if unsafe { libc::getegid() } == gid {
        return true;
    }
    // SAFETY: a count of 0 only asks how many groups there are.
    let n = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    let Ok(len) = usize::try_from(n) else {
        return false;
    };
    let mut groups: Vec<libc::gid_t> = vec![0; len];
    // SAFETY: `groups` holds `n` entries for `getgroups` to fill.
    let got = unsafe { libc::getgroups(n, groups.as_mut_ptr()) };
    let Ok(got) = usize::try_from(got) else {
        return false;
    };
    groups.truncate(got);
    groups.contains(&gid)
}

/// What the thread last reported, so each state is reported once, and
/// `Back` once the file is fine again after any of them.
#[derive(Debug, Default)]
struct Tracker {
    last: Option<DiskEvent>,
}

impl Tracker {
    /// The event to report for the state `now` (`classify`), if it is news.
    fn next(&mut self, now: Option<DiskEvent>) -> Option<DiskEvent> {
        if now == self.last {
            return None;
        }
        let was = std::mem::replace(&mut self.last, now);
        match now {
            Some(ev) => Some(ev),
            None => was.map(|_| DiskEvent::Back),
        }
    }

    /// Forgets what was reported, so the next state is reported again.
    fn forget(&mut self) {
        self.last = None;
    }
}

/// What the editor's side and the thread share.
struct Ctl {
    /// The version the document holds.
    stat: DiskStat,
    /// Bumped by every `rebase`: events found against an older stat are
    /// dropped.
    generation: u64,
    /// `check_now` or `rebase` asked for a look before the next second.
    check: bool,
    /// `rebase` asked for the state to be reported afresh. Only a rebase
    /// does: a look `check_now` asks for keeps what was reported, so a
    /// warning that cleared still says `Back`.
    forget: bool,
    /// The watcher was dropped: the thread ends.
    stop: bool,
    /// How many looks the thread has finished.
    checks: u64,
    /// The thread has returned.
    ended: bool,
}

/// Marks the thread ended when it returns, however it returns.
struct Ended<'a>(&'a Shared);

impl Drop for Ended<'_> {
    fn drop(&mut self) {
        self.0.lock().ended = true;
    }
}

/// A test's view of whether a watcher's thread has ended, which outlives
/// the watcher.
#[cfg(test)]
pub(crate) struct Probe(Arc<Shared>);

#[cfg(test)]
impl Probe {
    /// Waits (bounded) for the thread to end; panics if it does not.
    pub(crate) fn wait_ended(&self) {
        for _ in 0..1000 {
            if self.0.lock().ended {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("the watch thread never ended");
    }
}

struct Shared {
    ctl: Mutex<Ctl>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Ctl> {
        self.ctl.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The watch on one file. Dropping it stops the thread without waiting
/// for it; `join` waits.
pub struct Watcher {
    shared: Arc<Shared>,
    events: Receiver<(u64, DiskEvent)>,
    /// The generation `rebase` last set; older events are stale.
    generation: u64,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Starts watching `path`, whose version the document holds is
    /// `stat`. The first look is at once, then one every second.
    /// `waker` is called whenever there is an event to `poll`.
    pub fn spawn(path: PathBuf, stat: DiskStat, waker: Waker) -> Watcher {
        let shared = Arc::new(Shared {
            ctl: Mutex::new(Ctl {
                stat,
                generation: 0,
                check: true,
                forget: false,
                stop: false,
                checks: 0,
                ended: false,
            }),
            cv: Condvar::new(),
        });
        let (tx, events) = mpsc::channel();
        let theirs = Arc::clone(&shared);
        // A thread that cannot be started leaves a watcher that never
        // reports anything.
        let thread = thread::Builder::new()
            .name("file-watch".to_owned())
            .spawn(move || watch(&path, &theirs, &tx, &waker))
            .ok();
        Watcher {
            shared,
            events,
            generation: 0,
            thread,
        }
    }

    /// The next event, if one is waiting. Events found before the last
    /// `rebase` are dropped.
    pub fn poll(&mut self) -> Option<DiskEvent> {
        loop {
            let (generation, ev) = self.events.try_recv().ok()?;
            if generation == self.generation {
                return Some(ev);
            }
        }
    }

    /// The document now holds the version at `stat` (the editor wrote or
    /// reloaded it): anything found before is dropped, and the file is
    /// looked at again at once, its state reported afresh.
    pub fn rebase(&mut self, stat: DiskStat) {
        self.generation += 1;
        let mut ctl = self.shared.lock();
        ctl.stat = stat;
        ctl.generation = self.generation;
        ctl.check = true;
        ctl.forget = true;
        self.shared.cv.notify_all();
    }

    /// Looks at the file now rather than at the next second (the window
    /// came to the front). What changed since the last report is reported
    /// as usual, `Back` included.
    pub fn check_now(&self) {
        self.shared.lock().check = true;
        self.shared.cv.notify_all();
    }

    /// How many times the thread has looked at the file.
    pub fn checks(&self) -> u64 {
        self.shared.lock().checks
    }

    /// Stops the thread and waits for it to end (tests, so none is left
    /// running). Dropping the watcher stops it without waiting.
    pub fn join(mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    /// Tells the thread to end at its next wait.
    fn stop(&self) {
        self.shared.lock().stop = true;
        self.shared.cv.notify_all();
    }

    /// A view of whether the thread has ended.
    #[cfg(test)]
    pub(crate) fn probe(&self) -> Probe {
        Probe(Arc::clone(&self.shared))
    }
}

/// Stops the thread and lets it go without waiting: closing a window never
/// blocks on a stat (a dead network mount). The thread holds only the
/// path and the shared state, and ends at its next wait.
impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
        drop(self.thread.take());
    }
}

/// The thread: looks at `path` at once, then every `INTERVAL` or when
/// asked, and sends each new state with the generation it was found at.
fn watch(path: &Path, shared: &Shared, tx: &Sender<(u64, DiskEvent)>, waker: &Waker) {
    let _ended = Ended(shared);
    let mut tracker = Tracker::default();
    loop {
        let (stat, generation) = {
            let mut ctl = shared.lock();
            let due = Instant::now() + INTERVAL;
            while !ctl.stop && !ctl.check {
                let left = due.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                ctl = shared
                    .cv
                    .wait_timeout(ctl, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            if ctl.stop {
                return;
            }
            ctl.check = false;
            if std::mem::take(&mut ctl.forget) {
                tracker.forget();
            }
            (ctl.stat, ctl.generation)
        };
        let now = classify(&stat, fs::metadata(path));
        let news = tracker.next(now);
        if let Some(ev) = news
            && tx.send((generation, ev)).is_err()
        {
            return;
        }
        // Counted before the wake, so a woken main loop sees the look done.
        shared.lock().checks += 1;
        if news.is_some() {
            waker();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn root() -> bool {
        // SAFETY: `geteuid` cannot fail or touch memory.
        unsafe { libc::geteuid() == 0 }
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn classify_detects_change_gone_and_readonly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello").unwrap();
        let stat = DiskStat::of(&fs::metadata(&path).unwrap());
        assert_eq!(classify(&stat, fs::metadata(&path)), None);
        if root() {
            eprintln!("classify: running as root, modes deny nothing; mode checks skipped");
        } else {
            chmod(&path, 0o444);
            assert_eq!(
                classify(&stat, fs::metadata(&path)),
                Some(DiskEvent::ReadOnly)
            );
            chmod(&path, 0o200);
            assert_eq!(
                classify(&stat, fs::metadata(&path)),
                Some(DiskEvent::Unreadable)
            );
            chmod(&path, 0o644);
            assert_eq!(classify(&stat, fs::metadata(&path)), None);
        }
        let denied = Err(io::Error::from_raw_os_error(libc::EACCES));
        assert_eq!(classify(&stat, denied), Some(DiskEvent::Unreadable));
        fs::write(&path, "hello, world").unwrap();
        assert_eq!(
            classify(&stat, fs::metadata(&path)),
            Some(DiskEvent::Changed)
        );
        fs::remove_file(&path).unwrap();
        assert_eq!(classify(&stat, fs::metadata(&path)), Some(DiskEvent::Gone));
        // A directory in its place cannot be read as text.
        fs::create_dir(&path).unwrap();
        assert_eq!(
            classify(&stat, fs::metadata(&path)),
            Some(DiskEvent::Unreadable)
        );
    }

    #[test]
    fn each_state_is_reported_once_and_back_after_any() {
        use DiskEvent::*;
        let mut t = Tracker::default();
        assert_eq!(t.next(None), None);
        assert_eq!(t.next(Some(Gone)), Some(Gone));
        assert_eq!(t.next(Some(Gone)), None);
        assert_eq!(t.next(Some(Changed)), Some(Changed));
        assert_eq!(t.next(Some(Changed)), None);
        assert_eq!(t.next(None), Some(Back));
        assert_eq!(t.next(None), None);
        assert_eq!(t.next(Some(ReadOnly)), Some(ReadOnly));
        // Forgetting what was said (a rebase, a focus check) says it again.
        t.forget();
        assert_eq!(t.next(Some(ReadOnly)), Some(ReadOnly));
    }

    /// Waits (bounded) until `w` has looked at the file more than `after`
    /// times.
    fn wait_checks(w: &Watcher, after: u64) {
        for _ in 0..1000 {
            if w.checks() > after {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("the watcher never looked at the file");
    }

    #[test]
    fn the_thread_reports_a_change_and_rebase_drops_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello").unwrap();
        let stat = DiskStat::of(&fs::metadata(&path).unwrap());
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let waker: Waker = Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        });
        let mut w = Watcher::spawn(path.clone(), stat, waker);
        // The first look is at once, and finds nothing to say.
        wait_checks(&w, 0);
        assert_eq!(w.poll(), None);
        fs::write(&path, "changed elsewhere").unwrap();
        let seen = w.checks();
        w.check_now();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(w.checks() > seen);
        assert_eq!(w.poll(), Some(DiskEvent::Changed));
        assert_eq!(w.poll(), None);
        // A change the editor made itself, rebased before it was taken,
        // is never reported.
        fs::write(&path, "written here").unwrap();
        let seen = w.checks();
        w.check_now();
        wait_checks(&w, seen);
        w.rebase(DiskStat::of(&fs::metadata(&path).unwrap()));
        let seen = w.checks();
        wait_checks(&w, seen);
        assert_eq!(w.poll(), None);
        w.join();
    }

    /// A waker that signals `rx` on each call.
    fn signal_waker() -> (Waker, Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let waker: Waker = Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        });
        (waker, rx)
    }

    /// Looks at once and takes the event that look reports.
    fn look(w: &mut Watcher, rx: &Receiver<()>) -> Option<DiskEvent> {
        w.check_now();
        rx.recv_timeout(Duration::from_secs(5)).ok()?;
        w.poll()
    }

    #[test]
    fn check_now_keeps_the_back_transition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello").unwrap();
        let stat = DiskStat::of(&fs::metadata(&path).unwrap());
        let (waker, rx) = signal_waker();
        let mut w = Watcher::spawn(path.clone(), stat, waker);
        wait_checks(&w, 0);
        assert_eq!(w.poll(), None);
        // Moved away and back: the same version returns.
        let away = dir.path().join("away.md");
        fs::rename(&path, &away).unwrap();
        assert_eq!(look(&mut w, &rx), Some(DiskEvent::Gone));
        fs::rename(&away, &path).unwrap();
        assert_eq!(look(&mut w, &rx), Some(DiskEvent::Back));
        if root() {
            eprintln!("check_now_keeps_the_back_transition: running as root; mode part skipped");
        } else {
            // A mode change leaves the version alone.
            chmod(&path, 0o444);
            assert_eq!(look(&mut w, &rx), Some(DiskEvent::ReadOnly));
            chmod(&path, 0o644);
            assert_eq!(look(&mut w, &rx), Some(DiskEvent::Back));
        }
        w.join();
    }

    #[test]
    fn drop_does_not_wait_for_the_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "hello").unwrap();
        // A waker that holds the thread until released, as a stat on a
        // dead network mount would.
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let entered_tx = Mutex::new(entered_tx);
        let release_rx = Mutex::new(release_rx);
        let waker: Waker = Arc::new(move || {
            let _ = entered_tx.lock().unwrap().send(());
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        });
        // A stat that is not the file's: the first look reports a change.
        let w = Watcher::spawn(path, DiskStat::default(), waker);
        let probe = w.probe();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            drop(w);
            let _ = done_tx.send(());
        });
        let dropped = done_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        release_tx.send(()).unwrap();
        assert!(dropped, "dropping the watcher waited for its thread");
        // Released, the thread ends by itself.
        probe.wait_ended();
    }
}

//! Per-process lock records, so a second window opening the same file can
//! see the first. A record is `locks/<fnv1a64 hex of the path>-<pid>.lock`
//! holding `<pid>\n<path>\n`. Liveness is `kill(pid, 0)`; records of dead
//! processes are removed during the scan.

use std::fs;
use std::path::{Path, PathBuf};

use super::statedir::StateDir;

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// 64-bit FNV-1a. Used for every state-file key, because `DefaultHasher`
/// is not stable across Rust releases.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(OFFSET_BASIS, |h, &b| (h ^ u64::from(b)).wrapping_mul(PRIME))
}

/// This process's claim on one file. The record is removed on
/// [`Lock::release`] or when the lock is dropped.
#[derive(Debug)]
pub struct Lock {
    record: PathBuf,
}

#[derive(Debug)]
pub enum LockOutcome {
    /// No other live process holds the file.
    Acquired(Lock),
    /// Another live process (`pid`) holds the file. We still hold our own
    /// record, so "edit anyway" is visible to a third window.
    HeldBy { pid: u32, lock: Lock },
    /// No record could be written (the state directory could not be made,
    /// or the record could not be written), so no lock is held and no
    /// other window will see this one. Opening goes on regardless. `holder`
    /// is a live process seen holding the file all the same, if any.
    Unavailable {
        error: std::io::Error,
        holder: Option<u32>,
    },
}

impl Lock {
    /// Scans the lock records for `path`, removes those of dead processes,
    /// then writes our own record.
    ///
    /// The path is keyed by its canonical form (symlinks resolved), so two
    /// windows reaching the same file through different paths see each
    /// other. A file that does not exist yet falls back to its canonical
    /// parent plus the file name, or to the current directory joined with
    /// the path.
    ///
    /// The caller skips locking when there is no [`StateDir`] (no `$HOME`
    /// and no `$XDG_STATE_HOME`); what the user sees then is decided by the
    /// editor wiring, not here. When the state directory cannot be made or
    /// our record cannot be written, the outcome is
    /// [`LockOutcome::Unavailable`] with the reason and any live holder
    /// the scan saw: no lock is held then.
    ///
    /// The reported holder is never our own pid, even when a record of a
    /// recycled pid of ours is lying about.
    pub fn acquire(state: &StateDir, path: &Path) -> LockOutcome {
        let abs = absolute_key_path(path);
        let hash = format!("{:016x}", fnv1a64(abs.to_string_lossy().as_bytes()));
        let me = std::process::id();
        let dir = state.locks();
        // Scan even when the directory cannot be made ours: a holder seen
        // there still asks the prompt.
        let ensured = state.ensure();

        let mut holder: Option<u32> = None;
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(pid) = record_pid(&name.to_string_lossy(), &hash) else {
                    continue;
                };
                if pid == me {
                    continue;
                }
                if is_alive(pid) {
                    holder = Some(holder.map_or(pid, |h| h.min(pid)));
                } else {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }

        if let Err(error) = ensured {
            return LockOutcome::Unavailable { error, holder };
        }
        let record = dir.join(format!("{hash}-{me}.lock"));
        let body = format!("{me}\n{}\n", abs.display());
        if let Err(error) = fs::write(&record, body) {
            // A half-written record is no record.
            let _ = fs::remove_file(&record);
            return LockOutcome::Unavailable { error, holder };
        }
        let lock = Lock { record };
        match holder {
            Some(pid) => LockOutcome::HeldBy { pid, lock },
            None => LockOutcome::Acquired(lock),
        }
    }

    /// Removes our record. Dropping the lock does the same.
    pub fn release(self) {}
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.record);
    }
}

/// The pid in a record name `<hash>-<pid>.lock` for this `hash`.
fn record_pid(name: &str, hash: &str) -> Option<u32> {
    let rest = name.strip_prefix(hash)?.strip_prefix('-')?;
    rest.strip_suffix(".lock")?.parse().ok()
}

pub(crate) fn absolute_key_path(path: &Path) -> PathBuf {
    if let Ok(canon) = fs::canonicalize(path) {
        return canon;
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    match (abs.parent().map(fs::canonicalize), abs.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => abs,
    }
}

/// `kill(pid, 0)`: success or EPERM (alive, but not ours to signal) means
/// the process exists; ESRCH means it does not.
#[cfg(unix)]
pub(crate) fn is_alive(pid: u32) -> bool {
    // Pid 0 and values above i32::MAX would address process groups or wrap
    // to negative pids, never a single process.
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid == 0 {
        return false;
    }
    // SAFETY: `kill` with signal 0 only checks permissions and existence;
    // it takes plain integers and touches no memory.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub(crate) fn is_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::{Child, Command};

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let p =
                std::env::temp_dir().join(format!("abc_def-lock-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        fn state(&self) -> StateDir {
            StateDir::at(self.0.join("state"))
        }
        fn file(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, "x").unwrap();
            p
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A child process that is killed and reaped on drop, even on panic.
    struct Sleeper(Child);
    impl Sleeper {
        fn spawn() -> Sleeper {
            Sleeper(Command::new("sleep").arg("60").spawn().unwrap())
        }
        fn pid(&self) -> u32 {
            self.0.id()
        }
    }
    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Writes the record a process `pid` would have written for `path`.
    fn plant(state: &StateDir, path: &Path, pid: u32) -> PathBuf {
        state.ensure().unwrap();
        let canon = fs::canonicalize(path).unwrap();
        let rec = state.locks().join(format!(
            "{:016x}-{pid}.lock",
            fnv1a64(canon.to_string_lossy().as_bytes())
        ));
        fs::write(&rec, format!("{pid}\n{}\n", canon.display())).unwrap();
        rec
    }

    fn records(state: &StateDir) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(state.locks())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn fnv_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn first_open_acquires_and_writes_the_record() {
        let d = Dir::new("first");
        let f = d.file("a.md");
        let state = d.state();
        let LockOutcome::Acquired(lock) = Lock::acquire(&state, &f) else {
            panic!("expected Acquired");
        };
        let names = records(&state);
        assert_eq!(names.len(), 1);
        let canon = fs::canonicalize(&f).unwrap();
        let want = format!(
            "{:016x}-{}.lock",
            fnv1a64(canon.to_string_lossy().as_bytes()),
            std::process::id()
        );
        assert_eq!(names[0], want);
        let body = fs::read_to_string(state.locks().join(&want)).unwrap();
        assert_eq!(
            body,
            format!("{}\n{}\n", std::process::id(), canon.display())
        );
        drop(lock);
    }

    #[test]
    fn second_open_sees_live_holder() {
        let d = Dir::new("live");
        let f = d.file("a.md");
        let state = d.state();
        let holder = Sleeper::spawn();
        plant(&state, &f, holder.pid());
        let LockOutcome::HeldBy { pid, lock } = Lock::acquire(&state, &f) else {
            panic!("expected HeldBy");
        };
        assert_eq!(pid, holder.pid());
        // "Edit anyway" keeps its own record next to the holder's.
        assert_eq!(records(&state).len(), 2);
        drop(lock);
        assert_eq!(records(&state).len(), 1);
    }

    #[test]
    fn symlinked_path_sees_the_same_holder() {
        let d = Dir::new("symlink");
        let f = d.file("real/a.md");
        let link = d.0.join("link.md");
        std::os::unix::fs::symlink(&f, &link).unwrap();
        let state = d.state();
        let holder = Sleeper::spawn();
        plant(&state, &f, holder.pid());
        let LockOutcome::HeldBy { pid, .. } = Lock::acquire(&state, &link) else {
            panic!("expected HeldBy through the symlink");
        };
        assert_eq!(pid, holder.pid());
    }

    #[test]
    fn stale_lock_ignored_and_removed() {
        let d = Dir::new("stale");
        let f = d.file("a.md");
        let state = d.state();
        let mut child = Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        let rec = plant(&state, &f, dead);
        let outcome = Lock::acquire(&state, &f);
        assert!(matches!(outcome, LockOutcome::Acquired(_)));
        assert!(!rec.exists(), "stale record should be removed");
        assert_eq!(records(&state).len(), 1);
    }

    #[test]
    fn a_record_with_our_own_pid_is_never_the_holder() {
        let d = Dir::new("own");
        let f = d.file("a.md");
        let state = d.state();
        plant(&state, &f, std::process::id());
        assert!(matches!(
            Lock::acquire(&state, &f),
            LockOutcome::Acquired(_)
        ));
    }

    #[test]
    fn other_files_and_junk_do_not_count() {
        let d = Dir::new("other");
        let f = d.file("a.md");
        let g = d.file("b.md");
        let state = d.state();
        let holder = Sleeper::spawn();
        plant(&state, &g, holder.pid());
        fs::write(state.locks().join("junk.lock"), "x").unwrap();
        fs::write(state.locks().join("notes.txt"), "x").unwrap();
        assert!(matches!(
            Lock::acquire(&state, &f),
            LockOutcome::Acquired(_)
        ));
    }

    #[test]
    fn an_unwritable_state_directory_is_unavailable() {
        let d = Dir::new("unwritable");
        let f = d.file("a.md");
        // A regular file sits where the state directory must be made, so
        // creating it fails even for root.
        fs::write(d.0.join("state"), "x").unwrap();
        let outcome = Lock::acquire(&d.state(), &f);
        assert!(
            matches!(outcome, LockOutcome::Unavailable { holder: None, .. }),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_live_holder_is_reported_when_our_record_cannot_be_written() {
        let d = Dir::new("unwritable-held");
        let f = d.file("a.md");
        let state = d.state();
        let holder = Sleeper::spawn();
        let theirs = plant(&state, &f, holder.pid());
        // A directory sits where our record goes, so writing it fails,
        // even for root.
        let canon = fs::canonicalize(&f).unwrap();
        let ours = state.locks().join(format!(
            "{:016x}-{}.lock",
            fnv1a64(canon.to_string_lossy().as_bytes()),
            std::process::id()
        ));
        fs::create_dir(&ours).unwrap();
        let outcome = Lock::acquire(&state, &f);
        let LockOutcome::Unavailable { holder: seen, .. } = outcome else {
            panic!("expected Unavailable, got {outcome:?}");
        };
        assert_eq!(seen, Some(holder.pid()));
        assert!(theirs.exists());
    }

    #[test]
    fn lock_released_on_close() {
        let d = Dir::new("release");
        let f = d.file("a.md");
        let state = d.state();
        let LockOutcome::Acquired(lock) = Lock::acquire(&state, &f) else {
            panic!("expected Acquired");
        };
        assert_eq!(records(&state).len(), 1);
        lock.release();
        assert!(records(&state).is_empty());
        let LockOutcome::Acquired(lock) = Lock::acquire(&state, &f) else {
            panic!("expected Acquired");
        };
        assert_eq!(records(&state).len(), 1);
        drop(lock);
        assert!(records(&state).is_empty());
    }
}

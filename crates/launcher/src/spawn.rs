//! Starting the window detached, with `posix_spawn` and nothing but libc.
//!
//! The child gets its own session (`POSIX_SPAWN_SETSID`), so terminal job
//! control signals and a closing terminal's SIGHUP reach the launcher only.
//! Its stdio is `/dev/null`, its signal mask is empty and the signals below
//! are reset to their default actions.

use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use libc::{c_char, c_int, c_short};

/// Darwin defines this in `<sys/spawn.h>` (included by `<spawn.h>`) as
/// `#define POSIX_SPAWN_SETSID 0x0400`, a Darwin-specific flag that the
/// `libc` crate does not export for Apple targets.
#[cfg(target_os = "macos")]
const POSIX_SPAWN_SETSID: c_short = 0x0400;
#[cfg(not(target_os = "macos"))]
const POSIX_SPAWN_SETSID: c_short = libc::POSIX_SPAWN_SETSID;

const FLAGS: c_short = POSIX_SPAWN_SETSID
    | libc::POSIX_SPAWN_SETSIGDEF as c_short
    | libc::POSIX_SPAWN_SETSIGMASK as c_short;

/// Reset to `SIG_DFL` in the child. An inherited `SIG_IGN` (from `nohup`, or
/// a non-interactive shell's background job) would otherwise survive exec.
/// SIGPIPE is here too because the Rust runtime ignores it in the launcher.
const DEFAULT_SIGNALS: [c_int; 6] = [
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTSTP,
    libc::SIGHUP,
    libc::SIGTERM,
    libc::SIGPIPE,
];

fn check(rc: c_int) -> io::Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(rc))
    }
}

fn cstring(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

/// Spawn attributes, boxed so the initialised object never moves.
struct Attr(Box<MaybeUninit<libc::posix_spawnattr_t>>);

impl Attr {
    fn new() -> io::Result<Self> {
        let mut a = Box::new(MaybeUninit::uninit());
        // SAFETY: `a` points to writable storage for one posix_spawnattr_t.
        check(unsafe { libc::posix_spawnattr_init(a.as_mut_ptr()) })?;
        Ok(Attr(a))
    }

    fn ptr(&mut self) -> *mut libc::posix_spawnattr_t {
        self.0.as_mut_ptr()
    }
}

impl Drop for Attr {
    fn drop(&mut self) {
        // SAFETY: initialised in `new`, destroyed exactly once here.
        unsafe { libc::posix_spawnattr_destroy(self.ptr()) };
    }
}

/// Spawn file actions, boxed so the initialised object never moves.
struct Actions(Box<MaybeUninit<libc::posix_spawn_file_actions_t>>);

impl Actions {
    fn new() -> io::Result<Self> {
        let mut a = Box::new(MaybeUninit::uninit());
        // SAFETY: `a` points to writable storage for one file actions object.
        check(unsafe { libc::posix_spawn_file_actions_init(a.as_mut_ptr()) })?;
        Ok(Actions(a))
    }

    fn ptr(&mut self) -> *mut libc::posix_spawn_file_actions_t {
        self.0.as_mut_ptr()
    }
}

impl Drop for Actions {
    fn drop(&mut self) {
        // SAFETY: initialised in `new`, destroyed exactly once here.
        unsafe { libc::posix_spawn_file_actions_destroy(self.ptr()) };
    }
}

fn empty_sigset() -> libc::sigset_t {
    let mut set = MaybeUninit::uninit();
    // SAFETY: sigemptyset fully initialises the set it is given.
    unsafe {
        libc::sigemptyset(set.as_mut_ptr());
        set.assume_init()
    }
}

/// The current environment with each `set` entry replacing any variable of
/// the same name. Nothing else is removed.
fn environment(set: &[(OsString, OsString)]) -> io::Result<Vec<CString>> {
    let mut out = Vec::new();
    let overridden = |k: &OsStr| set.iter().any(|(name, _)| name == k);
    let vars = std::env::vars_os().filter(|(k, _)| !overridden(k));
    for (k, v) in vars.chain(set.iter().cloned()) {
        let mut entry = k.as_bytes().to_vec();
        entry.push(b'=');
        entry.extend_from_slice(v.as_bytes());
        out.push(cstring(&entry)?);
    }
    Ok(out)
}

fn null_terminated(strings: &[CString]) -> Vec<*mut c_char> {
    strings
        .iter()
        .map(|s| s.as_ptr() as *mut c_char)
        .chain(std::iter::once(ptr::null_mut()))
        .collect()
}

/// Start `exe` with `args` (argv[0] is `exe` itself) in a new session, with
/// stdio on `/dev/null`, an empty signal mask and default signal actions. The
/// child inherits the whole environment, with `set_env` added or replaced.
///
/// Returns the child's pid as soon as `posix_spawn` succeeds. A binary that
/// cannot be executed is reported here as an error on both glibc and Darwin.
pub fn spawn(
    exe: &Path,
    args: &[OsString],
    set_env: &[(OsString, OsString)],
) -> io::Result<libc::pid_t> {
    let path = cstring(exe.as_os_str().as_bytes())?;
    let mut argv_owned = vec![path.clone()];
    for a in args {
        argv_owned.push(cstring(a.as_bytes())?);
    }
    let argv = null_terminated(&argv_owned);
    let env_owned = environment(set_env)?;
    let envp = null_terminated(&env_owned);

    let mut attr = Attr::new()?;
    let mut defaults = empty_sigset();
    for sig in DEFAULT_SIGNALS {
        // SAFETY: `defaults` is an initialised sigset and `sig` a valid signal.
        unsafe { libc::sigaddset(&mut defaults, sig) };
    }
    let mask = empty_sigset();
    // SAFETY: `attr` is initialised; the sigsets are copied by the calls.
    unsafe {
        check(libc::posix_spawnattr_setflags(attr.ptr(), FLAGS))?;
        check(libc::posix_spawnattr_setsigdefault(attr.ptr(), &defaults))?;
        check(libc::posix_spawnattr_setsigmask(attr.ptr(), &mask))?;
    }

    let mut actions = Actions::new()?;
    let dev_null = c"/dev/null";
    for (fd, flags) in [
        (libc::STDIN_FILENO, libc::O_RDONLY),
        (libc::STDOUT_FILENO, libc::O_WRONLY),
        (libc::STDERR_FILENO, libc::O_WRONLY),
    ] {
        // SAFETY: `actions` is initialised and `dev_null` is a static C string.
        check(unsafe {
            libc::posix_spawn_file_actions_addopen(actions.ptr(), fd, dev_null.as_ptr(), flags, 0)
        })?;
    }

    let mut pid: libc::pid_t = 0;
    // SAFETY: every pointer is valid for the call: `path`, `argv` and `envp`
    // (null-terminated arrays of C strings owned above) outlive it, and
    // `actions` and `attr` are initialised.
    check(unsafe {
        libc::posix_spawn(
            &mut pid,
            path.as_ptr(),
            actions.ptr(),
            attr.ptr(),
            argv.as_ptr(),
            envp.as_ptr(),
        )
    })?;
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_replaces_and_keeps() {
        let path = std::env::var_os("PATH").expect("PATH is set under cargo test");
        let set = [(OsString::from("PATH"), OsString::from("/x"))];
        let env = environment(&set).unwrap();
        let path_entries: Vec<_> = env
            .iter()
            .filter(|e| e.as_bytes().starts_with(b"PATH="))
            .collect();
        assert_eq!(path_entries.len(), 1);
        assert_eq!(path_entries[0].as_bytes(), b"PATH=/x");
        assert_ne!(path.as_bytes(), b"/x");
        // Everything else is kept.
        assert_eq!(env.len(), std::env::vars_os().count());
    }
}

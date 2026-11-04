//! Starting a window process detached, with `posix_spawn` and nothing but
//! libc. The launcher uses it to start the window; window processes use it
//! to start sibling windows (the macOS router, and later spawns).
//!
//! The child gets its own session (`POSIX_SPAWN_SETSID`), so terminal job
//! control signals and a closing terminal's SIGHUP never reach it. Its stdio
//! is `/dev/null`, its signal mask is empty and the signals below are reset
//! to their default actions. No other file descriptor is inherited.

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

const BASE_FLAGS: c_short = POSIX_SPAWN_SETSID
    | libc::POSIX_SPAWN_SETSIGDEF as c_short
    | libc::POSIX_SPAWN_SETSIGMASK as c_short;

/// Darwin: with `POSIX_SPAWN_CLOEXEC_DEFAULT` every descriptor the file
/// actions don't name is closed in the child, as if each were FD_CLOEXEC.
#[cfg(target_os = "macos")]
const FLAGS: c_short = BASE_FLAGS | libc::POSIX_SPAWN_CLOEXEC_DEFAULT as c_short;
/// Elsewhere the file actions close them (see `close_others`).
#[cfg(not(target_os = "macos"))]
const FLAGS: c_short = BASE_FLAGS;

/// Reset to `SIG_DFL` in the child. An inherited `SIG_IGN` (from `nohup`, or
/// a non-interactive shell's background job) would otherwise survive exec.
/// SIGPIPE is here too because the Rust runtime ignores it in the parent.
const DEFAULT_SIGNALS: [c_int; 6] = [
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTSTP,
    libc::SIGHUP,
    libc::SIGTERM,
    libc::SIGPIPE,
];

/// Variables a window process sets for itself or that are meant for it alone
/// (the GTK renderer choice and GDK debug flags). A sibling must choose its
/// own, so window-side spawns strip them.
pub const TOOLKIT_VARS: [&str; 3] = ["GDK_DISABLE", "GDK_DEBUG", "GSK_RENDERER"];

/// The `strip_env` list for a window-side spawn: [`TOOLKIT_VARS`] plus every
/// variable in the current environment whose name starts with `prefix` (the
/// caller passes `app_meta::ENV_PREFIX`). Names that are not UTF-8 cannot be
/// listed and are kept.
pub fn sibling_strip_env(prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = TOOLKIT_VARS.iter().map(|v| v.to_string()).collect();
    if !prefix.is_empty() {
        let prefixed = std::env::vars_os()
            .filter_map(|(k, _)| k.into_string().ok())
            .filter(|k| k.starts_with(prefix) && !out.contains(k));
        out.extend(prefixed.collect::<Vec<_>>());
    }
    out
}

/// The first executable file named `name` in the directories of `path` (a
/// `PATH` value), as a shell would find it. `posix_spawn` does not search
/// `PATH`, so a window starting a system program by name (Linux's
/// `xdg-open`) looks it up here first.
pub fn find_on_path(name: &str, path: Option<&OsStr>) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

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

/// The current environment without the `strip` names, and with each `set`
/// entry replacing any variable of the same name.
fn environment(strip: &[&str], set: &[(OsString, OsString)]) -> io::Result<Vec<CString>> {
    let mut out = Vec::new();
    let overridden = |k: &OsStr| set.iter().any(|(name, _)| name == k);
    let stripped = |k: &OsStr| strip.iter().any(|name| OsStr::new(name) == k);
    let vars = std::env::vars_os().filter(|(k, _)| !overridden(k) && !stripped(k));
    for (k, v) in vars.chain(set.iter().cloned()) {
        let mut entry = k.as_bytes().to_vec();
        entry.push(b'=');
        entry.extend_from_slice(v.as_bytes());
        out.push(cstring(&entry)?);
    }
    Ok(out)
}

/// Closes every descriptor above stderr in the child. Darwin does this with
/// `POSIX_SPAWN_CLOEXEC_DEFAULT` in `FLAGS` instead.
#[cfg(target_os = "macos")]
fn close_others(_actions: &mut Actions) -> io::Result<()> {
    Ok(())
}

/// glibc 2.34 or later: a close-from action, run after the stdio opens.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn close_others(actions: &mut Actions) -> io::Result<()> {
    // SAFETY: `actions` is initialised; the call only records the action.
    check(unsafe {
        libc::posix_spawn_file_actions_addclosefrom_np(actions.ptr(), libc::STDERR_FILENO + 1)
    })
}

/// No close-from action is known here: descriptors without FD_CLOEXEC leak.
#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
fn close_others(_actions: &mut Actions) -> io::Result<()> {
    Ok(())
}

fn null_terminated(strings: &[CString]) -> Vec<*mut c_char> {
    strings
        .iter()
        .map(|s| s.as_ptr() as *mut c_char)
        .chain(std::iter::once(ptr::null_mut()))
        .collect()
}

/// Start `exe` with `args` (argv[0] is `exe` itself) in a new session, with
/// stdio on `/dev/null`, an empty signal mask, default signal actions and no
/// other inherited file descriptors. The child inherits the environment less
/// the `strip_env` names.
///
/// Returns the child's pid as soon as `posix_spawn` succeeds. A binary that
/// cannot be executed is reported here as an error on both glibc and Darwin.
pub fn spawn_detached(
    exe: &Path,
    args: &[OsString],
    strip_env: &[&str],
) -> io::Result<libc::pid_t> {
    spawn_detached_with(exe, args, strip_env, &[])
}

/// [`spawn_detached`], with `set_env` added to the child's environment (or
/// replacing a variable of the same name). The launcher sets `<P>PROTOCOL`
/// this way.
pub fn spawn_detached_with(
    exe: &Path,
    args: &[OsString],
    strip_env: &[&str],
    set_env: &[(OsString, OsString)],
) -> io::Result<libc::pid_t> {
    let path = cstring(exe.as_os_str().as_bytes())?;
    let mut argv_owned = vec![path.clone()];
    for a in args {
        argv_owned.push(cstring(a.as_bytes())?);
    }
    let argv = null_terminated(&argv_owned);
    let env_owned = environment(strip_env, set_env)?;
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
    close_others(&mut actions)?;

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
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn find_on_path_takes_the_first_executable() {
        let dir = scratch_file("path");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        // In `a` but not executable, so `b`'s is the one.
        std::fs::write(a.join("tool"), "").unwrap();
        std::fs::write(b.join("tool"), "").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let exec = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(b.join("tool"), exec).unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        let found = find_on_path("tool", Some(&path));
        let missing = find_on_path("nothing-here", Some(&path));
        let unset = find_on_path("tool", None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found, Some(b.join("tool")));
        assert_eq!(missing, None);
        assert_eq!(unset, None);
    }

    #[test]
    fn environment_replaces_and_keeps() {
        let path = std::env::var_os("PATH").expect("PATH is set under cargo test");
        let set = [(OsString::from("PATH"), OsString::from("/x"))];
        let env = environment(&[], &set).unwrap();
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

    #[test]
    fn environment_strips_names() {
        assert!(
            std::env::var_os("PATH").is_some(),
            "PATH is set under cargo test"
        );
        let env = environment(&["PATH"], &[]).unwrap();
        assert!(!env.iter().any(|e| e.as_bytes().starts_with(b"PATH=")));
        assert_eq!(env.len(), std::env::vars_os().count() - 1);
    }

    #[test]
    fn sibling_strip_env_lists_toolkit_and_prefixed() {
        // Read-only: PATH stands in for a prefixed name, so the test never
        // changes its own environment.
        assert!(
            std::env::var_os("PATH").is_some(),
            "PATH is set under cargo test"
        );
        let got = sibling_strip_env("PAT");
        for v in TOOLKIT_VARS {
            assert!(got.iter().any(|g| g == v), "{v} missing from {got:?}");
        }
        assert!(got.iter().any(|g| g == "PATH"), "{got:?}");
        assert!(!got.iter().any(|g| g == "HOME"), "{got:?}");
        assert_eq!(sibling_strip_env("").len(), TOOLKIT_VARS.len());
    }

    /// Where a probe child writes its verdict.
    fn scratch_file(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("detach-test-{}-{tag}", std::process::id()))
    }

    fn wait_for(path: &Path) -> String {
        let start = Instant::now();
        loop {
            if let Ok(s) = std::fs::read_to_string(path) {
                return s;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "child never wrote {path:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// `sh` arguments that write `open` or `closed` to `out` (through a
    /// rename, so a polling reader never sees a partial file), depending on
    /// whether `fd` is open in the shell.
    fn fd_probe(fd: c_int, out: &Path) -> Vec<OsString> {
        let out = out.display();
        let script = format!(
            "if [ -e /dev/fd/{fd} ]; then v=open; else v=closed; fi; \
             echo $v > '{out}.tmp' && mv '{out}.tmp' '{out}'"
        );
        vec![OsString::from("-c"), OsString::from(script)]
    }

    #[test]
    fn inherited_fd_is_not_open_in_child() {
        // An fd without FD_CLOEXEC, as a careless library might leave one.
        const FD: c_int = 213;
        let file = std::fs::File::open("/dev/null").unwrap();
        // SAFETY: dup2 onto an fd number only this test uses, then clear
        // FD_CLOEXEC on it. Both touch descriptors only.
        unsafe {
            assert_eq!(libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&file), FD), FD);
            assert_eq!(libc::fcntl(FD, libc::F_SETFD, 0), 0);
        }

        // Control: an ordinary spawn inherits it, so the probe can see it.
        let control = scratch_file("control");
        let _ = std::fs::remove_file(&control);
        let status = std::process::Command::new("/bin/sh")
            .args(fd_probe(FD, &control))
            .status()
            .unwrap();
        assert!(status.success());
        let control_verdict = wait_for(&control);

        let out = scratch_file("detached");
        let _ = std::fs::remove_file(&out);
        let pid = spawn_detached(Path::new("/bin/sh"), &fd_probe(FD, &out), &[]).unwrap();
        let verdict = wait_for(&out);
        // SAFETY: closes the fd this test opened and reaps the child it
        // spawned.
        unsafe {
            libc::close(FD);
            libc::waitpid(pid, ptr::null_mut(), 0);
        }
        let _ = std::fs::remove_file(&control);
        let _ = std::fs::remove_file(&out);
        assert_eq!(control_verdict, "open\n", "the probe must see the fd");
        assert_eq!(verdict, "closed\n");
    }
}

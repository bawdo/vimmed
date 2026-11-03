//! The command the user types. It starts the window process in its own
//! session and, unless `-b` is given, waits for it and reports its exit.
//!
//! Exit codes: the window's own; 128 + n when the window died from signal n;
//! 130 on SIGINT in the launcher; 2 for a usage error, no file or a
//! directory; 220 when the window binary could not be started.

mod args;
mod locate;
mod spawn;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use app_meta::SLUG;
use args::{Cmd, UsageError};
use marks::names::{LAUNCHER_MAIN, LAUNCHER_SPAWNED};

const EXIT_USAGE: i32 = 2;
const EXIT_SIGINT: i32 = 130;
const EXIT_NOT_STARTED: i32 = 220;

fn usage() -> String {
    format!("usage: {SLUG} [-b] [--view] [--] FILE")
}

fn main() {
    marks::init();
    marks::mark(LAUNCHER_MAIN);
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    std::process::exit(run(&argv));
}

fn run(argv: &[OsString]) -> i32 {
    let (path, background, view) = match args::parse(argv) {
        Ok(Cmd::Open {
            path,
            background,
            view,
        }) => (path, background, view),
        Ok(Cmd::Help) => {
            println!("{SLUG} {}", app_meta::VERSION);
            println!("{}", usage());
            println!("  -b      return once the window has started");
            println!("  --view  open in view mode");
            return 0;
        }
        Ok(Cmd::Version) => {
            println!("{SLUG} {}", app_meta::VERSION);
            return 0;
        }
        Err(UsageError(msg)) => {
            eprintln!("{SLUG}: {msg}");
            eprintln!("{}", usage());
            return EXIT_USAGE;
        }
    };

    if path.is_dir() {
        eprintln!("{SLUG}: {} is a directory", path.display());
        return EXIT_USAGE;
    }
    let path = resolve(&path);

    let exe = std::env::current_exe().unwrap_or_default();
    let Some(window) = locate::window_binary(&exe) else {
        eprintln!("{SLUG}: window binary not found");
        return EXIT_NOT_STARTED;
    };

    let mut window_args = Vec::new();
    if view {
        window_args.push(OsString::from("--view"));
    }
    window_args.push(path.into_os_string());
    let protocol = (
        OsString::from(format!("{}PROTOCOL", app_meta::ENV_PREFIX)),
        OsString::from(app_meta::PROTOCOL.to_string()),
    );

    if !background {
        // Before the spawn, so a Ctrl-C at any point after it exits 130.
        prepare_to_wait();
    }

    let pid = match spawn::spawn(&window, &window_args, &[protocol]) {
        Ok(pid) => pid,
        Err(e) => {
            eprintln!("{SLUG}: could not start {}: {e}", window.display());
            return EXIT_NOT_STARTED;
        }
    };
    marks::mark(LAUNCHER_SPAWNED);

    if background {
        return 0;
    }
    match wait(pid) {
        Ok(status) => exit_code(status),
        Err(e) => {
            eprintln!("{SLUG}: lost track of the window: {e}");
            1
        }
    }
}

/// The path as the window should get it: canonical when it exists, otherwise
/// the canonical parent joined with the name, so a new file opens empty.
fn resolve(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    match (std::fs::canonicalize(parent), path.file_name()) {
        (Ok(dir), Some(name)) => dir.join(name),
        _ => std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}

extern "C" fn on_sigint(_: libc::c_int) {
    // SAFETY: _exit is async-signal-safe.
    unsafe { libc::_exit(EXIT_SIGINT) };
}

fn set_handler(sig: libc::c_int, handler: libc::sighandler_t) -> libc::sighandler_t {
    // SAFETY: a zeroed sigaction is valid; the mask is then set explicitly and
    // `handler` is SIG_DFL or an async-signal-safe extern "C" fn.
    unsafe {
        let mut new: libc::sigaction = std::mem::zeroed();
        let mut old: libc::sigaction = std::mem::zeroed();
        new.sa_sigaction = handler;
        libc::sigemptyset(&mut new.sa_mask);
        new.sa_flags = 0;
        libc::sigaction(sig, &new, &mut old);
        old.sa_sigaction
    }
}

/// SIGINT exits 130 at once, leaving the window running in its own session.
/// SIGCHLD goes back to its default, because an inherited SIG_IGN would let
/// the kernel reap the window and `waitpid` would never see its status.
fn prepare_to_wait() {
    let old = set_handler(
        libc::SIGINT,
        on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t,
    );
    if old == libc::SIG_IGN {
        // Started with SIGINT ignored (a non-interactive shell's background
        // job): keep it that way, as the shell intended.
        set_handler(libc::SIGINT, libc::SIG_IGN);
    }
    set_handler(libc::SIGCHLD, libc::SIG_DFL);
}

/// Wait for `pid` to terminate, retrying on EINTR.
fn wait(pid: libc::pid_t) -> io::Result<libc::c_int> {
    let mut status: libc::c_int = 0;
    loop {
        // SAFETY: `status` is a valid, writable int.
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            return Ok(status);
        }
        let err = io::Error::last_os_error();
        if r == -1 && err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
}

/// The window's own code, or 128 + n (with a note on stderr) for signal n.
fn exit_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        let n = libc::WTERMSIG(status);
        eprintln!("{SLUG}: window ended by signal {n}");
        128 + n
    } else {
        1
    }
}

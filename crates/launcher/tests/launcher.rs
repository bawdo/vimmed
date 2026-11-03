//! End-to-end tests of the launcher binary against the `fake_window` helper.
//!
//! Env is set on the child `Command` only, never on the test process.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(10);

fn launcher() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_launcher"));
    c.env(
        format!("{}WINDOW_BIN", app_meta::ENV_PREFIX),
        env!("CARGO_BIN_EXE_fake_window"),
    );
    c.stdin(Stdio::null());
    c
}

/// A fresh, empty directory for one test, removed when dropped.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch() -> Scratch {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "launcher-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// Poll until `path` holds `<sid> <pid>`, then return both.
fn read_sid(path: &Path) -> (i32, i32) {
    let start = Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let mut it = text.split_whitespace().map(|s| s.parse::<i32>().unwrap());
            return (it.next().unwrap(), it.next().unwrap());
        }
        assert!(
            start.elapsed() < DEADLINE,
            "fake window never wrote {path:?}"
        );
        std::thread::sleep(POLL);
    }
}

/// Wait for a child with a deadline, killing it if the deadline passes.
fn wait_with_deadline(child: &mut Child) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > DEADLINE {
            let _ = child.kill();
            panic!("launcher did not exit within {DEADLINE:?}");
        }
        std::thread::sleep(POLL);
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn kill_hard(pid: i32) {
    // SAFETY: SIGKILL to a pid this test started (through the launcher).
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

#[test]
fn passes_exit_code_through() {
    let dir = scratch();
    let out = launcher()
        .env("FAKE_MODE", "exit:7")
        .arg(dir.join("a.md"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7), "{out:?}");
}

#[test]
fn maps_signal_to_128_plus_n() {
    let dir = scratch();
    let out = launcher()
        .env("FAKE_MODE", "signal:9")
        .arg(dir.join("a.md"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(137), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("window ended by signal 9"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.starts_with(&format!("{}: ", app_meta::SLUG)),
        "stderr: {stderr}"
    );
}

#[test]
fn background_returns_immediately() {
    let dir = scratch();
    let sid_file = dir.join("sid");
    // Warm up: macOS spends about 300 ms validating a freshly linked binary
    // on its first exec, which would land inside the timed run after every
    // rebuild. Run both binaries once so the timing measures the launcher.
    let warm = launcher()
        .env("FAKE_MODE", "exit:0")
        .arg(dir.join("warm.md"))
        .output()
        .unwrap();
    assert_eq!(warm.status.code(), Some(0), "{warm:?}");
    let start = Instant::now();
    // `output()` also waits for the pipes to close, so this fails if the
    // window held on to the launcher's stdout or stderr.
    let out = launcher()
        .env("FAKE_MODE", "write-sid,sleep:5000")
        .env("FAKE_OUT", &sid_file)
        .arg("-b")
        .arg(dir.join("a.md"))
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    let (_, pid) = read_sid(&sid_file);
    kill_hard(pid);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
}

#[test]
fn window_in_own_session() {
    let dir = scratch();
    let sid_file = dir.join("sid");
    let out = launcher()
        .env("FAKE_MODE", "write-sid")
        .env("FAKE_OUT", &sid_file)
        .arg(dir.join("a.md"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let (sid, pid) = read_sid(&sid_file);
    assert_eq!(sid, pid, "the window must lead its own session");
    // SAFETY: getsid(0) reads the test process's own session id.
    assert_ne!(sid, unsafe { libc::getsid(0) });
}

#[test]
fn directory_is_error() {
    let dir = scratch();
    let out = launcher().arg(&*dir).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("is a directory"), "stderr: {stderr}");
}

#[test]
fn no_file_is_usage_error() {
    let out = launcher().output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(!out.stderr.is_empty());
}

#[test]
fn version_prints_slug_and_version() {
    let out = launcher().arg("--version").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).lines().next(),
        Some(format!("{} {}", app_meta::SLUG, app_meta::VERSION).as_str())
    );
}

#[test]
fn missing_window_binary_is_220() {
    let dir = scratch();
    let out = launcher()
        .env(
            format!("{}WINDOW_BIN", app_meta::ENV_PREFIX),
            dir.join("no-such-window"),
        )
        .arg(dir.join("a.md"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(220), "{out:?}");
}

#[test]
fn sigint_exits_130_and_window_survives() {
    let dir = scratch();
    let sid_file = dir.join("sid");
    let mut child = launcher()
        .env("FAKE_MODE", "write-sid,sleep:60000")
        .env("FAKE_OUT", &sid_file)
        .arg(dir.join("a.md"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The launcher installs its SIGINT handler before spawning, so once the
    // window has written its pid the handler is in place.
    let (_, window) = read_sid(&sid_file);
    // SAFETY: SIGINT to the launcher this test spawned.
    unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
    let status = wait_with_deadline(&mut child);
    let survived = alive(window);
    kill_hard(window);
    assert_eq!(status.code(), Some(130), "{status:?}");
    assert!(survived, "the window must outlive the launcher");
}

//! Finder routing. Every instance's `application:openURLs:` handler starts
//! one sibling window process per URL and never opens a URL in itself, so
//! routing is right whichever instance Launch Services picks.
//!
//! An instance started with no path is the router: it creates no window,
//! waits up to `ROUTER_WAIT_NS` after `didFinishLaunching` for `openURLs`,
//! and exits 0 once `SETTLE_NS` have passed since its last spawn.
//!
//! The decision is pure and tested everywhere; the AppKit glue is macOS only.

use std::ffi::OsString;
use std::path::Path;

/// How long the router waits after launching for its first `openURLs`.
pub const ROUTER_WAIT_NS: u64 = 2_000_000_000;
/// How long after the last spawn the router stays for more `openURLs`.
pub const SETTLE_NS: u64 = 500_000_000;
/// How long the spawning instance looks for the child, to yield activation.
pub const YIELD_WAIT_NS: u64 = 500_000_000;
/// How often it looks.
pub const YIELD_POLL_NS: u64 = 50_000_000;

/// The window's argv (after argv[0]) for a sibling opening `path`: flags,
/// then `--`, then the path, so a file named like a flag still opens.
pub fn sibling_args(path: &Path, view: bool, display_name: Option<&str>) -> Vec<OsString> {
    let mut out = Vec::new();
    if view {
        out.push(OsString::from("--view"));
    }
    if let Some(name) = display_name {
        out.push(OsString::from("--display-name"));
        out.push(OsString::from(name));
    }
    out.push(OsString::from("--"));
    out.push(path.as_os_str().to_owned());
    out
}

/// What the router should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Not the router's call: a window is up, or launching hasn't finished.
    Stay,
    /// Ask again at this time (ns on the `marks::now_ns` clock).
    CheckAt(u64),
    /// Exit 0.
    Exit,
}

/// The router's clock events, all in ns on the `marks::now_ns` clock.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Router {
    launched_at: Option<u64>,
    last_spawn_at: Option<u64>,
}

impl Router {
    /// `applicationDidFinishLaunching:` ran.
    pub fn launched(&mut self, now: u64) {
        self.launched_at.get_or_insert(now);
    }

    /// A sibling was spawned. `openURLs` can come before launching finishes.
    pub fn spawned(&mut self, now: u64) {
        self.last_spawn_at = Some(now);
    }

    /// Whether an instance with or without a window should exit at `now`.
    pub fn decide(&self, now: u64, has_window: bool) -> Decision {
        let Some(launched) = self.launched_at else {
            return Decision::Stay;
        };
        if has_window {
            return Decision::Stay;
        }
        let deadline = match self.last_spawn_at {
            Some(spawn) => spawn + SETTLE_NS,
            None => launched + ROUTER_WAIT_NS,
        };
        if now >= deadline {
            Decision::Exit
        } else {
            Decision::CheckAt(deadline)
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use mac::{did_launch, is_router_mode, open_urls, set_router_mode, spawn_and_yield};

/// The AppKit side: the handlers, the spawn and the activation hand-over.
#[cfg(target_os = "macos")]
mod mac {
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use dispatch2::{DispatchQueue, DispatchTime};
    use marks::names;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSRunningApplication};
    use objc2_foundation::{NSArray, NSURL};

    use super::{Decision, Router, YIELD_POLL_NS, YIELD_WAIT_NS, sibling_args};

    thread_local! {
        static ROUTER: Cell<Router> = const {
            Cell::new(Router { launched_at: None, last_spawn_at: None })
        };
        /// Started with no path: no window, exits by the router's decision.
        static ROUTER_MODE: Cell<bool> = const { Cell::new(false) };
        /// `applicationDidFinishLaunching:` has run.
        static LAUNCHED: Cell<bool> = const { Cell::new(false) };
    }

    pub fn set_router_mode(on: bool) {
        ROUTER_MODE.set(on);
    }

    pub fn is_router_mode() -> bool {
        ROUTER_MODE.get()
    }

    /// Runs `f` on the main queue after `ns` nanoseconds.
    fn after(ns: u64, f: impl FnOnce() + Send + 'static) {
        if let Ok(t) = DispatchTime::try_from(Duration::from_nanos(ns)) {
            let _ = DispatchQueue::main().after(t, f);
        }
    }

    /// `applicationDidFinishLaunching:` ran (after the window, if any).
    pub fn did_launch() {
        LAUNCHED.set(true);
        ROUTER.with(|r| {
            let mut v = r.get();
            v.launched(marks::now_ns());
            r.set(v);
        });
        if ROUTER_MODE.get() {
            check();
        }
    }

    /// Router mode only: exit now, or look again when it might be time.
    fn check() {
        let now = marks::now_ns();
        match ROUTER.get().decide(now, false) {
            Decision::Exit => crate::app::finish(0),
            Decision::CheckAt(t) => after(t.saturating_sub(now), check),
            Decision::Stay => {}
        }
    }

    /// A path from our own argv, echoed back as an open request while
    /// launching (AppKit can treat command-line arguments as documents).
    fn is_argv_echo(path: &Path) -> bool {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let want = canon(path);
        std::env::args_os()
            .skip(1)
            .any(|a| canon(Path::new(&a)) == want)
    }

    /// `application:openURLs:`: one sibling window process per file URL.
    /// Never opens anything in this instance.
    pub fn open_urls(urls: &NSArray<NSURL>) {
        let router = ROUTER_MODE.get();
        let who = if router {
            names::ROUTER_OPEN_URLS
        } else {
            names::WINDOW_OPEN_URLS
        };
        marks::value(who, f64::from(std::process::id()));
        for url in urls.iter() {
            let Some(path) = url.to_file_path() else {
                continue;
            };
            if !router && !LAUNCHED.get() && is_argv_echo(&path) {
                continue;
            }
            spawn_and_yield(&path, &sibling_args(&path, false, None));
        }
        if router && LAUNCHED.get() {
            check();
        }
    }

    /// Spawns a sibling for `path` and hands activation to it when it shows
    /// up. Errors go to stderr: there is no window to show them in.
    pub fn spawn_and_yield(path: &Path, args: &[OsString]) {
        match spawn_sibling(args) {
            Ok(pid) => {
                let now = marks::now_ns();
                marks::value(names::ROUTED_SPAWN, f64::from(pid));
                ROUTER.with(|r| {
                    let mut v = r.get();
                    v.spawned(now);
                    r.set(v);
                });
                yield_to(pid, now + YIELD_WAIT_NS);
            }
            Err(e) => eprintln!(
                "{}: could not start a window for {}: {e}",
                app_meta::SLUG,
                path.display()
            ),
        }
    }

    /// Starts this executable detached with `args`, stripping the toolkit
    /// variables and every `<ENV_PREFIX>*` variable from its environment.
    fn spawn_sibling(args: &[OsString]) -> io::Result<i32> {
        let exe: PathBuf = std::env::current_exe()?;
        let strip = detach::sibling_strip_env(app_meta::ENV_PREFIX);
        let strip: Vec<&str> = strip.iter().map(String::as_str).collect();
        detach::spawn_detached(&exe, args, &strip)
    }

    /// Best effort: once the child registers as an application (looked for
    /// until `deadline`), yield activation to it.
    fn yield_to(pid: i32, deadline: u64) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if let Some(child) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            NSApplication::sharedApplication(mtm).yieldActivationToApplication(&child);
            return;
        }
        if marks::now_ns() + YIELD_POLL_NS <= deadline {
            after(YIELD_POLL_NS, move || yield_to(pid, deadline));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn router_decides_exit() {
        let t0 = 1_000 * MS;
        let mut r = Router::default();
        // Nothing to decide before launching finishes, even after a spawn.
        assert_eq!(r.decide(t0, false), Decision::Stay);

        // No openURLs: wait 2 s from launch, then exit.
        r.launched(t0);
        assert_eq!(r.decide(t0, false), Decision::CheckAt(t0 + 2_000 * MS));
        assert_eq!(
            r.decide(t0 + 1_999 * MS, false),
            Decision::CheckAt(t0 + 2_000 * MS)
        );
        assert_eq!(r.decide(t0 + 2_000 * MS, false), Decision::Exit);

        // A spawn: exit 500 ms after it, even before the 2 s are up.
        let mut r = Router::default();
        r.launched(t0);
        r.spawned(t0 + 100 * MS);
        assert_eq!(
            r.decide(t0 + 300 * MS, false),
            Decision::CheckAt(t0 + 600 * MS)
        );
        assert_eq!(r.decide(t0 + 600 * MS, false), Decision::Exit);

        // Another openURLs restarts the 500 ms, past the 2 s if need be.
        r.spawned(t0 + 1_800 * MS);
        assert_eq!(
            r.decide(t0 + 2_000 * MS, false),
            Decision::CheckAt(t0 + 2_300 * MS)
        );
        assert_eq!(r.decide(t0 + 2_300 * MS, false), Decision::Exit);

        // An instance with a window never exits on the router's account.
        assert_eq!(r.decide(t0 + 10_000 * MS, true), Decision::Stay);
    }

    #[test]
    fn spawn_before_launch_counts() {
        let t0 = 1_000 * MS;
        let mut r = Router::default();
        r.spawned(t0);
        r.launched(t0 + 50 * MS);
        assert_eq!(
            r.decide(t0 + 50 * MS, false),
            Decision::CheckAt(t0 + 500 * MS)
        );
        assert_eq!(r.decide(t0 + 500 * MS, false), Decision::Exit);
    }

    #[test]
    fn sibling_args_end_with_the_path() {
        let os = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        let p = Path::new("/x/-b.md");
        assert_eq!(sibling_args(p, false, None), os(&["--", "/x/-b.md"]));
        assert_eq!(
            sibling_args(p, true, Some("Notes")),
            os(&["--view", "--display-name", "Notes", "--", "/x/-b.md"])
        );
    }

    #[test]
    fn launch_time_is_the_first() {
        let t0 = 1_000 * MS;
        let mut r = Router::default();
        r.launched(t0);
        r.launched(t0 + 900 * MS);
        assert_eq!(r.decide(t0 + 2_000 * MS, false), Decision::Exit);
    }
}

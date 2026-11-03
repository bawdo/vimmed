//! Startup and interaction timing marks.
//!
//! Nothing is written unless `<P>MARKS` (a log path) is set, so release builds
//! pay only for one cached flag check. `<P>T0` (ns on the `now_ns` clock) is the
//! pre-spawn timestamp that marks are measured from.

pub mod bench;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// Monotonic nanoseconds. Linux: `CLOCK_MONOTONIC`. macOS: `CLOCK_UPTIME_RAW`,
/// the same base as `NSEvent.timestamp` and `CACurrentMediaTime`.
pub fn now_ns() -> u64 {
    #[cfg(target_os = "macos")]
    const CLOCK: libc::clockid_t = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_os = "macos"))]
    const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;

    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    unsafe { libc::clock_gettime(CLOCK, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Mark names. Values are the lower-case snake names written to the log.
pub mod names {
    pub const LAUNCHER_MAIN: &str = "launcher_main";
    pub const LAUNCHER_SPAWNED: &str = "launcher_spawned";
    pub const WINDOW_MAIN: &str = "window_main";
    pub const CONFIG_READ: &str = "config_read";
    pub const STATE_SCANNED: &str = "state_scanned";
    pub const FILE_READ: &str = "file_read";
    pub const PARSED: &str = "parsed";
    pub const FONTS_LOADED: &str = "fonts_loaded";
    pub const TOOLKIT_INIT: &str = "toolkit_init";
    pub const WINDOW_CREATED: &str = "window_created";
    pub const FIRST_PAINT: &str = "first_paint";
    pub const FIRST_FRAME: &str = "first_frame";
    pub const FIRST_FRAME_GLASS: &str = "first_frame_glass";
    pub const IMAGES_PAINTED: &str = "images_painted";
    pub const HIGHLIGHT_READY: &str = "highlight_ready";
    pub const LAYOUT_MS: &str = "layout_ms";
    pub const RASTER_MS: &str = "raster_ms";
    pub const KEY_MS: &str = "key_ms";
    pub const KEY_WAIT_MS: &str = "key_wait_ms";
    pub const KEY_DRAW_MS: &str = "key_draw_ms";
    pub const KEY_RENDER_MS: &str = "key_render_ms";
    pub const KEY_HANDLER_MS: &str = "key_handler_ms";
    pub const KEY_GLASS_MS: &str = "key_glass_ms";
    pub const KEY_GLASS_EST_MS: &str = "key_glass_est_ms";
    pub const GLASS_UNAVAILABLE: &str = "glass_unavailable";
    /// A value, not a time: the display's refresh rate in Hz.
    pub const REFRESH_HZ: &str = "refresh_hz";
}

struct Log {
    t0: u64,
    file: Option<File>,
}

static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

fn env_name(suffix: &str) -> String {
    format!("{}{}", app_meta::ENV_PREFIX, suffix)
}

/// The release gate as a pure decision: a non-empty path enables marks.
fn gate(marks_path: Option<&str>) -> bool {
    marks_path.is_some_and(|p| !p.is_empty())
}

/// True only when `<P>MARKS` is set. No marks are written otherwise.
pub fn enabled() -> bool {
    gate(std::env::var(env_name("MARKS")).ok().as_deref())
}

fn log() -> &'static Mutex<Log> {
    LOG.get_or_init(|| {
        let t0 = std::env::var(env_name("T0"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(now_ns);
        let file = std::env::var(env_name("MARKS"))
            .ok()
            .filter(|p| gate(Some(p)))
            .and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());
        Mutex::new(Log { t0, file })
    })
}

fn format_line(name: &str, ms: f64) -> String {
    format!("{name},{ms:.3}")
}

fn write_line(l: &mut Log, name: &str, ms: f64) {
    if let Some(f) = l.file.as_mut() {
        let _ = writeln!(f, "{}", format_line(name, ms));
    }
}

/// Read T0 and open the log. Call first thing in `main`.
pub fn init() {
    let _ = log();
}

/// Record a named milestone as milliseconds since T0.
pub fn mark(name: &str) {
    let now = now_ns();
    let mut l = log().lock().unwrap();
    let ms = now.saturating_sub(l.t0) as f64 / 1e6;
    write_line(&mut l, name, ms);
}

/// Record a named value in milliseconds.
pub fn value(name: &str, ms: f64) {
    let mut l = log().lock().unwrap();
    write_line(&mut l, name, ms);
}

/// Convert a `now_ns` timestamp to milliseconds since T0.
pub fn ms_since_t0(ns: u64) -> f64 {
    let l = log().lock().unwrap();
    ns.saturating_sub(l.t0) as f64 / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_format_is_name_comma_ms() {
        assert_eq!(
            format_line(names::FIRST_FRAME, 12.3449),
            "first_frame,12.345"
        );
    }

    #[test]
    fn disabled_without_env() {
        assert!(!gate(None));
        assert!(!gate(Some("")));
        assert!(gate(Some("/some/path.log")));
        // The real gate, with the variable unset in the test environment.
        assert!(!enabled());
    }
}

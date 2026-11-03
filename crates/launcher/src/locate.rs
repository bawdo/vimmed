//! Finding the window binary.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The name of the env override, `<P>WINDOW_BIN`.
pub fn override_var() -> String {
    format!("{}WINDOW_BIN", app_meta::ENV_PREFIX)
}

/// The window binary for the launcher at `exe`, or `None` when no candidate
/// exists.
///
/// Candidates, first existing file wins:
/// 1. Linux only: `LINUX_LIBEXEC_DIR/WINDOW_BIN` (the installed package).
/// 2. Next to the canonicalised launcher: `WINDOW_BIN` on Linux, and the
///    bundle executable `BUNDLE_NAME` on macOS.
/// 3. Next to the launcher: `window` (development builds).
///
/// `<P>WINDOW_BIN` is a test hook. When it is set and non-empty it is used
/// as is and nothing else is tried, so a test is never handed a real window
/// that happens to sit next to the launcher in `target/`, and a missing file
/// there fails to start (exit 220) rather than falling back.
pub fn window_binary(exe: &Path) -> Option<PathBuf> {
    window_binary_with(exe, std::env::var_os(override_var()))
}

fn window_binary_with(exe: &Path, env_override: Option<OsString>) -> Option<PathBuf> {
    if let Some(p) = env_override.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    candidates(exe).into_iter().find(|p| p.is_file())
}

fn candidates(exe: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "linux") {
        out.push(Path::new(app_meta::LINUX_LIBEXEC_DIR).join(app_meta::WINDOW_BIN));
    }
    let real = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    if let Some(dir) = real.parent() {
        let packaged = if cfg!(target_os = "macos") {
            app_meta::BUNDLE_NAME
        } else {
            app_meta::WINDOW_BIN
        };
        out.push(dir.join(packaged));
        out.push(dir.join("window"));
    }
    // Through a symlink, also look beside the link itself.
    if let Some(dir) = exe.parent().filter(|d| Some(*d) != real.parent()) {
        out.push(dir.join("window"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("launcher-locate-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn override_wins_even_when_missing() {
        let got = window_binary_with(
            Path::new("/nowhere/launcher"),
            Some(OsString::from("/nowhere/fake")),
        );
        assert_eq!(got, Some(PathBuf::from("/nowhere/fake")));
    }

    #[test]
    fn empty_override_is_ignored() {
        let dir = scratch("empty");
        let exe = dir.join("launcher");
        std::fs::write(&exe, "").unwrap();
        std::fs::write(dir.join("window"), "").unwrap();
        let got = window_binary_with(&exe, Some(OsString::new()));
        if !cfg!(target_os = "linux") || !candidates(&exe)[0].is_file() {
            assert_eq!(
                got.map(|p| p.file_name().unwrap().to_owned()),
                Some(OsString::from("window"))
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn packaged_name_beats_dev_name() {
        let dir = scratch("packaged");
        let exe = dir.join("launcher");
        let packaged = if cfg!(target_os = "macos") {
            app_meta::BUNDLE_NAME
        } else {
            app_meta::WINDOW_BIN
        };
        std::fs::write(&exe, "").unwrap();
        std::fs::write(dir.join("window"), "").unwrap();
        std::fs::write(dir.join(packaged), "").unwrap();
        let got = window_binary_with(&exe, None).unwrap();
        // On an installed Linux system candidate 1 may exist and win.
        if !cfg!(target_os = "linux") || !candidates(&exe)[0].is_file() {
            assert_eq!(got.file_name().unwrap(), packaged);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_found_is_none() {
        let dir = scratch("none");
        let exe = dir.join("launcher");
        std::fs::write(&exe, "").unwrap();
        if !cfg!(target_os = "linux") || !candidates(&exe)[0].is_file() {
            assert_eq!(window_binary_with(&exe, None), None);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

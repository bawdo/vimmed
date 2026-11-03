//! `cargo xtask bundle-mac [--release]`: builds the window and the launcher
//! and lays out `target/dist/<BUNDLE_NAME>.app`.
//!
//! - `Contents/MacOS/<BUNDLE_NAME>`: the window binary, the bundle's
//!   executable, which the launcher finds beside itself.
//! - `Contents/MacOS/<SLUG>`: the launcher, the command users run.
//! - `Contents/Info.plist`, from `packaging/templates/Info.plist.in`.
//!
//! Nothing here needs the WindowServer.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::templates;

/// Where the bundle goes, relative to the workspace root.
pub fn app_dir(root: &Path) -> PathBuf {
    target_dir(root)
        .join("dist")
        .join(format!("{}.app", app_meta::BUNDLE_NAME))
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { root.join(p) })
        .unwrap_or_else(|| root.join("target"))
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let release = match args {
        [] => false,
        [a] if a == "--release" => true,
        _ => return Err("usage: cargo xtask bundle-mac [--release]".into()),
    };
    let mut build = Command::new("cargo");
    build
        .current_dir(root)
        .args(["build", "-p", "window", "-p", "launcher"]);
    if release {
        build.arg("--release");
    }
    let ok = build.status().map_err(|e| e.to_string())?.success();
    if !ok {
        return Err("cargo build failed".into());
    }
    let bin = target_dir(root).join(if release { "release" } else { "debug" });
    let app = app_dir(root);
    write_bundle(root, &bin, &app)?;
    println!("bundle: {}", app.display());
    Ok(())
}

/// Lays out the bundle at `app` from the binaries in `bin`, replacing any
/// bundle already there.
pub fn write_bundle(root: &Path, bin: &Path, app: &Path) -> Result<(), String> {
    let err = |what: &str, p: &Path, e: std::io::Error| format!("{what} {}: {e}", p.display());
    if app.exists() {
        std::fs::remove_dir_all(app).map_err(|e| err("remove", app, e))?;
    }
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).map_err(|e| err("create", &macos, e))?;
    for (from, to) in [
        ("window", app_meta::BUNDLE_NAME),
        ("launcher", app_meta::SLUG),
    ] {
        let (src, dst) = (bin.join(from), macos.join(to));
        std::fs::copy(&src, &dst).map_err(|e| err("copy", &src, e))?;
    }
    let tpl = root.join("packaging/templates/Info.plist.in");
    let text = std::fs::read_to_string(&tpl).map_err(|e| err("read", &tpl, e))?;
    let plist = templates::render(&text, &templates::app_vars())?;
    let contents = app.join("Contents");
    std::fs::write(contents.join("Info.plist"), plist).map_err(|e| err("write", &contents, e))?;
    std::fs::write(contents.join("PkgInfo"), "APPL????").map_err(|e| err("write", &contents, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_out_the_bundle() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let scratch = std::env::temp_dir().join(format!("bundle-mac-{}", std::process::id()));
        let bin = scratch.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("window"), "w").unwrap();
        std::fs::write(bin.join("launcher"), "l").unwrap();
        let app = scratch.join("X.app");
        std::fs::create_dir_all(app.join("stale")).unwrap();
        write_bundle(&root, &bin, &app).unwrap();
        let macos = app.join("Contents/MacOS");
        assert_eq!(
            std::fs::read_to_string(macos.join(app_meta::BUNDLE_NAME)).unwrap(),
            "w"
        );
        assert_eq!(
            std::fs::read_to_string(macos.join(app_meta::SLUG)).unwrap(),
            "l"
        );
        let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains(app_meta::BUNDLE_ID));
        assert!(!app.join("stale").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn rejects_unknown_arguments() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        assert!(run(&root, &["--debug".into()]).is_err());
    }
}

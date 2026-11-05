//! `cargo xtask bundle-mac [--release]`: builds the window and the launcher
//! and lays out `target/dist/<BUNDLE_NAME>.app`.
//!
//! - `Contents/MacOS/<WINDOW_BIN>` (`<SLUG>-window`): the window binary, the
//!   bundle's executable, which the launcher finds beside itself.
//! - `Contents/MacOS/<SLUG>`: the launcher, the command users run.
//! - `Contents/Info.plist`, from `packaging/templates/Info.plist.in`.
//! - `Contents/Resources/AppIcon.icns`, copied from `packaging/icons/`
//!   (made by `scripts/make-icons.sh`).
//!
//! Nothing here needs the WindowServer.
//!
//! `cargo xtask install-mac` builds the release bundle and installs it for
//! the current user: `~/Applications/<BUNDLE_NAME>.app`, and the symlink
//! `~/.local/bin/<SLUG>` to the launcher inside it.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::templates;
use app_meta::derive::Names;

/// Where the bundle goes, relative to the workspace root.
pub fn app_dir(root: &Path) -> PathBuf {
    target_dir(root)
        .join("dist")
        .join(format!("{}.app", app_meta::BUNDLE_NAME))
}

pub fn target_dir(root: &Path) -> PathBuf {
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

/// The two executables in `Contents/MacOS/`, as `(built binary, bundle
/// name)`: the window first (the bundle's `CFBundleExecutable`), then the
/// launcher. The window is `<SLUG>-window`, never the CamelCase bundle name,
/// which for a one-word slug differs from the launcher's name only by case.
pub fn bundle_executables(n: &Names) -> [(&'static str, String); 2] {
    [
        ("window", n.window_bin.clone()),
        ("launcher", n.slug.clone()),
    ]
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
    let entries = bundle_executables(&templates::current_names());
    if entries[0].1.eq_ignore_ascii_case(&entries[1].1) {
        return Err(format!(
            "the window and the launcher would both be Contents/MacOS/{} on a case-insensitive volume",
            entries[0].1
        ));
    }
    for (from, to) in entries {
        let (src, dst) = (bin.join(from), macos.join(to));
        std::fs::copy(&src, &dst).map_err(|e| err("copy", &src, e))?;
    }
    let plist = templates::render_file(root, "Info.plist.in", &templates::current_names())?;
    let contents = app.join("Contents");
    let resources = contents.join("Resources");
    std::fs::create_dir_all(&resources).map_err(|e| err("create", &resources, e))?;
    let icon = root.join("packaging/icons/AppIcon.icns");
    std::fs::copy(&icon, resources.join("AppIcon.icns")).map_err(|e| err("copy", &icon, e))?;
    std::fs::write(contents.join("Info.plist"), plist).map_err(|e| err("write", &contents, e))?;
    std::fs::write(contents.join("PkgInfo"), "APPL????").map_err(|e| err("write", &contents, e))?;
    Ok(())
}

/// Where `install-mac` puts things for a given home directory.
#[derive(Debug)]
pub struct InstallPaths {
    /// `~/Applications/<BUNDLE_NAME>.app`
    pub app: PathBuf,
    /// `~/.local/bin/<SLUG>`, the command on PATH.
    pub link: PathBuf,
    /// The launcher inside the installed bundle, which `link` points at.
    pub link_target: PathBuf,
}

pub fn install_paths(home: &Path, n: &Names) -> InstallPaths {
    let app = home
        .join("Applications")
        .join(format!("{}.app", n.bundle_name));
    InstallPaths {
        link: home.join(".local/bin").join(&n.slug),
        link_target: app.join("Contents/MacOS").join(&n.slug),
        app,
    }
}

/// Copies `src_app` to `~/Applications` (replacing an older copy) and points
/// `~/.local/bin/<SLUG>` at its launcher. The launcher resolves the link to
/// find the window binary beside it. A regular file where the link goes is
/// left alone and reported, since it isn't ours to replace.
pub fn install(src_app: &Path, home: &Path, n: &Names) -> Result<InstallPaths, String> {
    let p = install_paths(home, n);
    if let Ok(meta) = std::fs::symlink_metadata(&p.link)
        && !meta.file_type().is_symlink()
    {
        return Err(format!(
            "{} exists and is not a symlink; move it aside and run again",
            p.link.display()
        ));
    }
    let err =
        |what: &str, path: &Path, e: std::io::Error| format!("{what} {}: {e}", path.display());
    if p.app.exists() {
        std::fs::remove_dir_all(&p.app).map_err(|e| err("remove", &p.app, e))?;
    }
    copy_tree(src_app, &p.app)?;
    let bin = p.link.parent().expect("the link has a parent");
    std::fs::create_dir_all(bin).map_err(|e| err("create", bin, e))?;
    if std::fs::symlink_metadata(&p.link).is_ok() {
        std::fs::remove_file(&p.link).map_err(|e| err("remove", &p.link, e))?;
    }
    std::os::unix::fs::symlink(&p.link_target, &p.link).map_err(|e| err("link", &p.link, e))?;
    Ok(p)
}

/// Copies a directory tree, keeping file permissions and symlinks.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let err =
        |what: &str, path: &Path, e: std::io::Error| format!("{what} {}: {e}", path.display());
    std::fs::create_dir_all(to).map_err(|e| err("create", to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| err("read", from, e))? {
        let entry = entry.map_err(|e| err("read", from, e))?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(|e| err("stat", &src, e))?;
        if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else if kind.is_symlink() {
            let target = std::fs::read_link(&src).map_err(|e| err("read link", &src, e))?;
            std::os::unix::fs::symlink(target, &dst).map_err(|e| err("link", &dst, e))?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| err("copy", &src, e))?;
        }
    }
    Ok(())
}

/// `cargo xtask install-mac`: builds the release bundle, then installs it
/// for the current user.
pub fn install_mac(root: &Path, args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("usage: cargo xtask install-mac".into());
    }
    if !cfg!(target_os = "macos") {
        return Err("install-mac runs on macOS only".into());
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    run(root, &["--release".into()])?;
    let p = install(
        &app_dir(root),
        Path::new(&home),
        &templates::current_names(),
    )?;
    println!("installed: {}", p.app.display());
    println!(
        "command:   {} -> {}",
        p.link.display(),
        p.link_target.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abc() -> Names {
        Names::from_slug("abc_def", "com.bawdo").unwrap()
    }

    fn fake_app(dir: &Path) -> PathBuf {
        let app = dir.join("AbcDef.app");
        let macos = app.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("abc_def"), "l").unwrap();
        std::fs::write(macos.join("AbcDef"), "w").unwrap();
        std::fs::write(app.join("Contents/Info.plist"), "p").unwrap();
        app
    }

    #[test]
    fn install_paths_are_under_home() {
        let p = install_paths(Path::new("/h"), &abc());
        assert_eq!(p.app, Path::new("/h/Applications/AbcDef.app"));
        assert_eq!(p.link, Path::new("/h/.local/bin/abc_def"));
        assert_eq!(
            p.link_target,
            Path::new("/h/Applications/AbcDef.app/Contents/MacOS/abc_def")
        );
    }

    #[test]
    fn install_copies_the_app_and_links_the_launcher() {
        let s = std::env::temp_dir().join(format!("install-mac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&s);
        let src = fake_app(&s.join("dist"));
        let home = s.join("home");
        let p = install(&src, &home, &abc()).unwrap();
        assert_eq!(std::fs::read_to_string(&p.link_target).unwrap(), "l");
        assert_eq!(std::fs::read_link(&p.link).unwrap(), p.link_target);
        // A second install replaces the old bundle and the old link.
        std::fs::write(p.app.join("Contents/old"), "x").unwrap();
        std::fs::write(src.join("Contents/MacOS/abc_def"), "l2").unwrap();
        let p = install(&src, &home, &abc()).unwrap();
        assert!(!p.app.join("Contents/old").exists());
        assert_eq!(std::fs::read_to_string(&p.link).unwrap(), "l2");
        std::fs::remove_dir_all(&s).unwrap();
    }

    #[test]
    fn install_refuses_to_replace_a_regular_file() {
        let s = std::env::temp_dir().join(format!("install-mac-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&s);
        let src = fake_app(&s.join("dist"));
        let home = s.join("home");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::fs::write(home.join(".local/bin/abc_def"), "mine").unwrap();
        assert!(install(&src, &home, &abc()).is_err());
        assert_eq!(
            std::fs::read_to_string(home.join(".local/bin/abc_def")).unwrap(),
            "mine"
        );
        std::fs::remove_dir_all(&s).unwrap();
    }

    #[test]
    fn install_mac_rejects_arguments() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        assert!(install_mac(&root, &["--force".into()]).is_err());
    }

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
            std::fs::read_to_string(macos.join(app_meta::WINDOW_BIN)).unwrap(),
            "w"
        );
        assert_eq!(
            std::fs::read_to_string(macos.join(app_meta::SLUG)).unwrap(),
            "l"
        );
        let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains(app_meta::BUNDLE_ID));
        assert!(plist.contains("<key>CFBundleIconFile</key>\n\t<string>AppIcon</string>"));
        assert_eq!(
            std::fs::read(app.join("Contents/Resources/AppIcon.icns")).unwrap(),
            std::fs::read(root.join("packaging/icons/AppIcon.icns")).unwrap()
        );
        assert!(!app.join("stale").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// macOS volumes are case-insensitive by default, so two executables
    /// whose names differ only by case would land on one file.
    #[test]
    fn bundle_executables_never_collide() {
        for slug in ["abc", "abcdef", "abc_def", "a1_b2_c3"] {
            let n = Names::from_slug(slug, "com.bawdo").unwrap();
            let names: Vec<String> = bundle_executables(&n)
                .iter()
                .map(|(_, to)| to.to_ascii_lowercase())
                .collect();
            assert_eq!(names.len(), 2);
            assert_ne!(names[0], names[1], "{slug}: {names:?}");
        }
    }

    #[test]
    fn bundle_executable_is_the_window() {
        let n = Names::from_slug("abcdef", "com.bawdo").unwrap();
        assert_eq!(
            bundle_executables(&n),
            [
                ("window", "abcdef-window".to_string()),
                ("launcher", "abcdef".to_string())
            ]
        );
    }

    #[test]
    fn rejects_unknown_arguments() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        assert!(run(&root, &["--debug".into()]).is_err());
    }
}

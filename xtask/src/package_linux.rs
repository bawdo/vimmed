//! `cargo xtask package-linux`: the `.deb`, the `.rpm` and the PKGBUILD, run
//! in the Linux check container or on a Linux machine with cargo-deb and
//! cargo-generate-rpm installed.
//!
//! It builds the release binaries, then stages `<target>/pkg/`:
//!
//! - `Cargo.toml`, rendered from `pkg-Cargo.toml.in`: a package with no code
//!   that only carries the `.deb` and `.rpm` metadata and asset lists.
//! - `bin/<SLUG>` and `bin/<SLUG>-window`: the launcher and the window,
//!   renamed.
//! - `<BUNDLE_ID>.desktop` and `<BUNDLE_ID>.metainfo.xml`, rendered.
//! - `arch/`: the rendered `PKGBUILD` with the sources it lists (a source
//!   tarball of the workspace, the desktop and the metainfo files), ready
//!   for `makepkg`.
//!
//! cargo-deb and cargo-generate-rpm then package the staged files, without
//! building, into `<target>/dist/`.

use crate::bundle_mac::target_dir;
use crate::templates;
use app_meta::derive::Names;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn io_err(what: &str, p: &Path, e: std::io::Error) -> String {
    format!("{what} {}: {e}", p.display())
}

fn cargo(dir: &Path, args: &[String]) -> Result<(), String> {
    let ok = Command::new("cargo")
        .current_dir(dir)
        .args(args)
        .status()
        .map_err(|e| format!("cargo: {e}"))?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("cargo {} failed", args.join(" ")))
    }
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("usage: cargo xtask package-linux".into());
    }
    let build: Vec<String> = ["build", "--release", "-p", "window", "-p", "launcher"]
        .map(String::from)
        .to_vec();
    cargo(root, &build)?;

    let n = templates::current_names();
    let target = target_dir(root);
    let (pkg, dist) = (target.join("pkg"), target.join("dist"));
    stage(root, &target.join("release"), &pkg, &n)?;
    let tree = format!("{}-{}", n.kebab, app_meta::VERSION);
    let tarball = pkg.join("arch").join(format!("{tree}.tar.gz"));
    source_tarball(root, &source_files(root)?, &tarball, &tree)?;

    fs::create_dir_all(&dist).map_err(|e| io_err("create", &dist, e))?;
    for entry in fs::read_dir(&dist).map_err(|e| io_err("read", &dist, e))? {
        let p = entry.map_err(|e| io_err("read", &dist, e))?.path();
        if p.extension().is_some_and(|x| x == "deb" || x == "rpm") {
            fs::remove_file(&p).map_err(|e| io_err("remove", &p, e))?;
        }
    }
    cargo(root, &deb_args(&pkg.join("Cargo.toml"), &dist))?;
    cargo(&pkg, &rpm_args(&dist))?;
    println!("packages: {}", dist.display());
    println!("PKGBUILD: {}", pkg.join("arch/PKGBUILD").display());
    Ok(())
}

/// `cargo deb` over the staged manifest, packaging the prebuilt binaries.
pub fn deb_args(manifest: &Path, dist: &Path) -> Vec<String> {
    vec![
        "deb".into(),
        "--manifest-path".into(),
        manifest.display().to_string(),
        "--no-build".into(),
        "--no-strip".into(),
        "--output".into(),
        format!("{}/", dist.display()),
    ]
}

/// `cargo generate-rpm`, run from the staging directory (it reads
/// `./Cargo.toml` and resolves asset sources from there).
pub fn rpm_args(dist: &Path) -> Vec<String> {
    vec![
        "generate-rpm".into(),
        "--output".into(),
        format!("{}/", dist.display()),
    ]
}

/// Lays out the staging directory `pkg` from the binaries in `bin`,
/// replacing whatever was there.
pub fn stage(root: &Path, bin: &Path, pkg: &Path, n: &Names) -> Result<(), String> {
    if pkg.exists() {
        fs::remove_dir_all(pkg).map_err(|e| io_err("remove", pkg, e))?;
    }
    let arch = pkg.join("arch");
    for dir in [pkg.join("bin"), pkg.join("src"), arch.clone()] {
        fs::create_dir_all(&dir).map_err(|e| io_err("create", &dir, e))?;
    }
    for (from, to) in [
        ("launcher", n.slug.clone()),
        ("window", n.window_bin.clone()),
    ] {
        let (src, dst) = (bin.join(from), pkg.join("bin").join(to));
        fs::copy(&src, &dst).map_err(|e| io_err("copy", &src, e))?;
        fs::set_permissions(&dst, fs::Permissions::from_mode(0o755))
            .map_err(|e| io_err("chmod", &dst, e))?;
    }
    let write =
        |path: &Path, text: &str| fs::write(path, text).map_err(|e| io_err("write", path, e));
    write(&pkg.join("src/lib.rs"), "")?;
    write(
        &pkg.join("Cargo.toml"),
        &templates::render_file(root, "pkg-Cargo.toml.in", n)?,
    )?;
    let desktop = templates::render_file(root, "app.desktop.in", n)?;
    let metainfo = templates::render_file(root, "metainfo.xml.in", n)?;
    for dir in [pkg, arch.as_path()] {
        write(&dir.join(format!("{}.desktop", n.bundle_id)), &desktop)?;
        write(
            &dir.join(format!("{}.metainfo.xml", n.bundle_id)),
            &metainfo,
        )?;
    }
    write(
        &arch.join("PKGBUILD"),
        &templates::render_file(root, "PKGBUILD.in", n)?,
    )?;
    Ok(())
}

/// The workspace's source files, relative to `root`: what git tracks (plus
/// untracked files it doesn't ignore) when git can read the checkout.
/// Otherwise (the rename test's copy has no `.git`, and a worktree mounted in
/// a container has a dangling one) every file except `.git`, top-level
/// `target*` directories and the plain top-level names in `.gitignore`.
pub fn source_files(root: &Path) -> Result<Vec<String>, String> {
    if root.join(".git").exists()
        && let Ok(out) = Command::new("git")
            .current_dir(root)
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ])
            .output()
        && out.status.success()
    {
        return Ok(out
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .filter(|rel| root.join(rel).is_file())
            .collect());
    }
    let ignored =
        top_level_ignores(&fs::read_to_string(root.join(".gitignore")).unwrap_or_default());
    fn walk(root: &Path, dir: &Path, skip: &[String], out: &mut Vec<String>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| io_err("read", dir, e))? {
            let entry = entry.map_err(|e| io_err("read", dir, e))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if dir == root && (name == ".git" || name.starts_with("target") || skip.contains(&name))
            {
                continue;
            }
            let kind = entry.file_type().map_err(|e| io_err("stat", &path, e))?;
            if kind.is_dir() {
                walk(root, &path, skip, out)?;
            } else if kind.is_file() {
                let rel = path.strip_prefix(root).expect("walked under root");
                out.push(rel.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &ignored, &mut out)?;
    Ok(out)
}

/// The plain names in a `.gitignore` (`spike/`, `/notes`): no globs, no
/// negation, no inner slash. Enough for the top-level directories this
/// workspace ignores.
fn top_level_ignores(gitignore: &str) -> Vec<String> {
    gitignore
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .map(|l| l.trim_start_matches('/').trim_end_matches('/'))
        .filter(|l| !l.is_empty() && !l.contains(['*', '?', '[', '/']))
        .map(String::from)
        .collect()
}

/// Writes `dest`, a gzipped tarball of `files` (relative to `root`) under
/// the top-level directory `prefix`.
pub fn source_tarball(
    root: &Path,
    files: &[String],
    dest: &Path,
    prefix: &str,
) -> Result<(), String> {
    let parent = dest.parent().ok_or("tarball path has no parent")?;
    let scratch = parent.join(".tree");
    if scratch.exists() {
        fs::remove_dir_all(&scratch).map_err(|e| io_err("remove", &scratch, e))?;
    }
    let tree = scratch.join(prefix);
    for rel in files {
        let to = tree.join(rel);
        if let Some(p) = to.parent() {
            fs::create_dir_all(p).map_err(|e| io_err("create", p, e))?;
        }
        let from = root.join(rel);
        fs::copy(&from, &to).map_err(|e| io_err("copy", &from, e))?;
    }
    let status = Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .arg("-czf")
        .arg(dest)
        .arg("-C")
        .arg(&scratch)
        .arg(prefix)
        .status()
        .map_err(|e| format!("tar: {e}"));
    let _ = fs::remove_dir_all(&scratch);
    if status?.success() {
        Ok(())
    } else {
        Err(format!("tar failed writing {}", dest.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("pkg-linux-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn names() -> Names {
        Names::from_slug("abc_def", "com.bawdo").unwrap()
    }

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    #[test]
    fn deb_args_package_the_prebuilt_binaries() {
        let args = deb_args(Path::new("/s/Cargo.toml"), Path::new("/d"));
        let joined = args.join(" ");
        assert_eq!(args[0], "deb");
        assert!(joined.contains("--manifest-path /s/Cargo.toml"));
        assert!(joined.contains("--output /d"));
        assert!(args.iter().any(|a| a == "--no-build"));
        assert!(args.iter().any(|a| a == "--no-strip"));
    }

    #[test]
    fn rpm_args_write_into_dist() {
        let args = rpm_args(Path::new("/d"));
        assert_eq!(args[0], "generate-rpm");
        assert!(args.join(" ").contains("--output /d/"));
    }

    #[test]
    fn stage_lays_out_the_package_tree() {
        use std::os::unix::fs::PermissionsExt;
        let s = scratch("stage");
        let bin = s.join("release");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("launcher"), "l").unwrap();
        fs::write(bin.join("window"), "w").unwrap();
        let pkg = s.join("pkg");
        fs::create_dir_all(pkg.join("stale")).unwrap();
        stage(&root(), &bin, &pkg, &names()).unwrap();

        assert!(!pkg.join("stale").exists());
        assert_eq!(fs::read_to_string(pkg.join("bin/abc_def")).unwrap(), "l");
        assert_eq!(
            fs::read_to_string(pkg.join("bin/abc_def-window")).unwrap(),
            "w"
        );
        let mode = fs::metadata(pkg.join("bin/abc_def"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(pkg.join("src/lib.rs").is_file());

        let manifest = fs::read_to_string(pkg.join("Cargo.toml")).unwrap();
        for dest in [
            "\"/usr/bin/abc_def\"",
            "\"/usr/lib/abc_def/abc_def-window\"",
            "\"/usr/share/applications/com.bawdo.abc-def.desktop\"",
            "\"/usr/share/metainfo/com.bawdo.abc-def.metainfo.xml\"",
        ] {
            assert_eq!(manifest.matches(dest).count(), 2, "{dest} in deb and rpm");
        }
        let desktop = fs::read_to_string(pkg.join("com.bawdo.abc-def.desktop")).unwrap();
        assert!(desktop.contains("Exec=abc_def -b %f"));
        assert!(pkg.join("com.bawdo.abc-def.metainfo.xml").is_file());

        let arch = pkg.join("arch");
        let pkgbuild = fs::read_to_string(arch.join("PKGBUILD")).unwrap();
        assert!(pkgbuild.contains("cargo build --frozen --release"));
        assert!(pkgbuild.contains("\"$pkgdir/usr/bin/abc_def\""));
        assert!(pkgbuild.contains("\"$pkgdir/usr/lib/abc_def/abc_def-window\""));
        assert!(arch.join("com.bawdo.abc-def.desktop").is_file());
        assert!(arch.join("com.bawdo.abc-def.metainfo.xml").is_file());
        fs::remove_dir_all(&s).unwrap();
    }

    #[test]
    fn source_files_without_git_skip_target_dirs() {
        let s = scratch("walk");
        for f in [
            "Cargo.toml",
            "crates/a/src/lib.rs",
            ".cargo/config.toml",
            "target/release/x",
            "target-linux/y",
            "spike/z",
            ".superpowers/notes.md",
        ] {
            let p = s.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        }
        fs::write(
            s.join(".gitignore"),
            "target/\nspike/\n.superpowers/\n*.log\n",
        )
        .unwrap();
        let mut got = source_files(&s).unwrap();
        got.sort();
        assert_eq!(
            got,
            [
                ".cargo/config.toml",
                ".gitignore",
                "Cargo.toml",
                "crates/a/src/lib.rs"
            ]
        );
        fs::remove_dir_all(&s).unwrap();
    }

    #[test]
    fn source_tarball_unpacks_under_its_prefix() {
        let s = scratch("tar");
        let tree = s.join("tree");
        fs::create_dir_all(tree.join("src")).unwrap();
        fs::write(tree.join("Cargo.toml"), "m").unwrap();
        fs::write(tree.join("src/main.rs"), "r").unwrap();
        let out = s.join("out/x-1.tar.gz");
        let files = vec!["Cargo.toml".to_string(), "src/main.rs".to_string()];
        source_tarball(&tree, &files, &out, "x-1").unwrap();
        let list = Command::new("tar").arg("-tzf").arg(&out).output().unwrap();
        let list = String::from_utf8_lossy(&list.stdout);
        assert!(list.lines().any(|l| l == "x-1/Cargo.toml"));
        assert!(list.lines().any(|l| l == "x-1/src/main.rs"));
        assert!(!s.join("out/.tree").exists());
        fs::remove_dir_all(&s).unwrap();
    }
}

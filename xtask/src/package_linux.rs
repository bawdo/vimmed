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
//! - `icons/hicolor/<s>x<s>/apps/app-icon.png`: the app icon at each of
//!   `ICON_SIZES`, installed as `<BUNDLE_ID>.png` in the hicolor theme.
//! - `arch/`: the rendered `PKGBUILD` with the sources it lists (a source
//!   tarball of the workspace's build inputs, which carries the icons, and
//!   the desktop and the metainfo files), ready for `makepkg`.
//!
//! cargo-deb and cargo-generate-rpm then package the staged files, without
//! building, into `<target>/dist/`.
//!
//! `cargo xtask package-linux --container` does the same in the Linux check
//! image (`docker/Dockerfile.check`), with `<target>` at `CONTAINER_TARGET`
//! in the workspace. The release workflow builds the Linux assets this way.

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

/// `--container` runs the same command in the check image, with the target
/// directory on the workspace mount (see `container_env`), so the packages
/// land in the checkout rather than on the image's cargo volume.
pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    match args {
        [] => package(root),
        [a] if a == "--container" => in_container(root),
        _ => Err("usage: cargo xtask package-linux [--container]".into()),
    }
}

/// Where `--container` builds, relative to the workspace: its own directory,
/// so the container's build never touches the host's `target/` (the xtask
/// binary running on the host included). The `.deb` and the `.rpm` end up
/// in `<this>/dist/` and the PKGBUILD in `<this>/pkg/arch/`.
pub const CONTAINER_TARGET: &str = "target/linux-package";

/// The environment `--container` runs `package-linux` with: the target
/// directory under the `/work` mount, and the checkout marked safe for git
/// (it belongs to the host user, not the container's root), so the source
/// tarball is made from what git tracks.
pub fn container_env() -> Vec<(&'static str, String)> {
    let work = crate::ci::WORK;
    vec![
        ("CARGO_TARGET_DIR", format!("{work}/{CONTAINER_TARGET}")),
        ("GIT_CONFIG_COUNT", "1".into()),
        ("GIT_CONFIG_KEY_0", "safe.directory".into()),
        ("GIT_CONFIG_VALUE_0", work.into()),
    ]
}

fn in_container(root: &Path) -> Result<(), String> {
    let (image, volume) =
        crate::ci::check_image(root).map_err(|e| format!("package-linux: {e}"))?;
    crate::ci::run_in_container(
        root,
        &image,
        &volume,
        &container_env(),
        "cargo xtask package-linux",
    )
    .map_err(|e| format!("package-linux: the container run failed: {e}"))?;
    let out = root.join(CONTAINER_TARGET);
    println!("packages: {}", out.join("dist").display());
    println!("PKGBUILD: {}", out.join("pkg/arch/PKGBUILD").display());
    Ok(())
}

fn package(root: &Path) -> Result<(), String> {
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

/// The hicolor sizes shipped, each from
/// `packaging/icons/hicolor/<s>x<s>/apps/app-icon.png` (made by
/// `scripts/make-icons.sh`). The deb and rpm asset lists and the PKGBUILD
/// name the same sizes; the staging test holds them to this list.
pub const ICON_SIZES: [u32; 6] = [32, 48, 64, 128, 256, 512];

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
    // The deb's copyright file and the rpm's licence asset are made from it.
    let license = root.join("LICENSE");
    fs::copy(&license, pkg.join("LICENSE")).map_err(|e| io_err("copy", &license, e))?;
    for size in ICON_SIZES {
        let rel = format!("icons/hicolor/{size}x{size}/apps/app-icon.png");
        let (src, dst) = (root.join("packaging").join(&rel), pkg.join(&rel));
        let dir = dst.parent().expect("an icon path has a parent");
        fs::create_dir_all(dir).map_err(|e| io_err("create", dir, e))?;
        fs::copy(&src, &dst).map_err(|e| io_err("copy", &src, e))?;
    }
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

/// Top-level files `cargo build --frozen --release` reads, plus the
/// LICENSE the PKGBUILD installs.
const BUILD_FILES: [&str; 4] = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "LICENSE"];
/// Top-level directories it reads: the cargo config, every workspace member
/// (all must exist for the workspace to load), and the packaging templates.
/// Nothing in `crates/` reads files outside its crate except app-meta's
/// build script, which reads the root `Cargo.toml`.
const BUILD_DIRS: [&str; 4] = [".cargo", "crates", "xtask", "packaging"];

/// Whether a workspace-relative path is a build input, and so belongs in
/// the source tarball. Docs, reports, notes, the README and Docker files
/// are not, and some of them spell the project name.
pub fn is_build_input(rel: &str) -> bool {
    BUILD_FILES.contains(&rel)
        || rel
            .split_once('/')
            .is_some_and(|(top, _)| BUILD_DIRS.contains(&top))
}

/// The workspace's build inputs (see `is_build_input`), relative to `root`:
/// from what git tracks when git can read the checkout, otherwise (the
/// rename test's copy has no `.git`, and a worktree mounted in a container
/// has a dangling one) from a walk of the build-input directories. Untracked
/// files are left out of a checkout's list.
pub fn source_files(root: &Path) -> Result<Vec<String>, String> {
    if root.join(".git").exists()
        && let Ok(out) = Command::new("git")
            .current_dir(root)
            .args(["ls-files", "-z", "--cached"])
            .output()
        && out.status.success()
    {
        return Ok(out
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .filter(|rel| is_build_input(rel) && root.join(rel).is_file())
            .collect());
    }
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| io_err("read", dir, e))? {
            let entry = entry.map_err(|e| io_err("read", dir, e))?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| io_err("stat", &path, e))?;
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() {
                let rel = path.strip_prefix(root).expect("walked under root");
                out.push(rel.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    let mut out: Vec<String> = BUILD_FILES
        .iter()
        .filter(|f| root.join(f).is_file())
        .map(|f| f.to_string())
        .collect();
    for dir in BUILD_DIRS {
        if root.join(dir).is_dir() {
            walk(root, &root.join(dir), &mut out)?;
        }
    }
    Ok(out)
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
    fn container_builds_into_the_checkout() {
        let env = container_env();
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.clone())
        };
        let target = get("CARGO_TARGET_DIR").unwrap();
        assert_eq!(target, format!("{}/{CONTAINER_TARGET}", crate::ci::WORK));
        assert!(
            target.starts_with(&format!("{}/target/", crate::ci::WORK)),
            "{target} is not under the mounted target directory"
        );
        assert_ne!(target, format!("{}/target", crate::ci::WORK));
        assert_eq!(get("GIT_CONFIG_COUNT").as_deref(), Some("1"));
        assert_eq!(get("GIT_CONFIG_KEY_0").as_deref(), Some("safe.directory"));
        assert_eq!(get("GIT_CONFIG_VALUE_0").as_deref(), Some(crate::ci::WORK));
    }

    #[test]
    fn run_rejects_unknown_arguments() {
        let e = run(Path::new("/nonexistent"), &["--bogus".into()]).unwrap_err();
        assert!(e.contains("--container"), "{e}");
        let two = ["--container".to_string(), "x".to_string()];
        assert!(run(Path::new("/nonexistent"), &two).is_err());
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
        for size in ICON_SIZES {
            let rel = format!("icons/hicolor/{size}x{size}/apps/app-icon.png");
            let dest =
                format!("\"/usr/share/icons/hicolor/{size}x{size}/apps/com.bawdo.abc-def.png\"");
            assert_eq!(manifest.matches(&dest).count(), 2, "{dest} in deb and rpm");
            assert_eq!(
                manifest.matches(&format!("\"{rel}\"")).count(),
                2,
                "{rel} as the source in deb and rpm"
            );
            assert_eq!(
                fs::read(pkg.join(&rel)).unwrap(),
                fs::read(root().join("packaging").join(&rel)).unwrap(),
                "{rel} staged"
            );
        }
        // The licence and maintainer, and LICENSE shipped by both packagers.
        assert!(manifest.contains("license = \"MIT\""));
        assert!(manifest.contains("authors = [\"Keith Bawden <keith@bawdo.com>\"]"));
        assert!(manifest.contains("maintainer = \"Keith Bawden <keith@bawdo.com>\""));
        assert!(
            manifest.contains("license-file = [\"LICENSE\", \"0\"]"),
            "deb copyright"
        );
        assert!(
            manifest
                .contains("source = \"LICENSE\", dest = \"/usr/share/licenses/abc-def/LICENSE\""),
            "rpm licence asset"
        );
        assert!(!manifest.contains("LicenseRef"));
        assert_eq!(
            fs::read_to_string(pkg.join("LICENSE")).unwrap(),
            fs::read_to_string(root().join("LICENSE")).unwrap()
        );
        let desktop = fs::read_to_string(pkg.join("com.bawdo.abc-def.desktop")).unwrap();
        assert!(desktop.contains("Exec=abc_def -b %f"));
        assert!(desktop.contains("\nIcon=com.bawdo.abc-def\n"));
        let metainfo = fs::read_to_string(pkg.join("com.bawdo.abc-def.metainfo.xml")).unwrap();
        assert!(metainfo.contains("<icon type=\"stock\">com.bawdo.abc-def</icon>"));

        let arch = pkg.join("arch");
        let pkgbuild = fs::read_to_string(arch.join("PKGBUILD")).unwrap();
        assert!(pkgbuild.contains("cargo build --frozen --release"));
        assert!(pkgbuild.contains("\"$pkgdir/usr/bin/abc_def\""));
        assert!(pkgbuild.contains("\"$pkgdir/usr/lib/abc_def/abc_def-window\""));
        assert!(pkgbuild.contains("# Maintainer: Keith Bawden <keith@bawdo.com>"));
        assert!(pkgbuild.contains("license=('MIT')"));
        let sizes = ICON_SIZES.map(|s| s.to_string()).join(" ");
        assert!(pkgbuild.contains(&format!("for _s in {sizes}; do")));
        assert!(pkgbuild.contains(
            "\"packaging/icons/hicolor/${_s}x${_s}/apps/app-icon.png\" \\\n      \"$pkgdir/usr/share/icons/hicolor/${_s}x${_s}/apps/com.bawdo.abc-def.png\""
        ));
        assert!(
            pkgbuild
                .contains("install -Dm644 LICENSE \"$pkgdir/usr/share/licenses/$pkgname/LICENSE\"")
        );
        assert!(arch.join("com.bawdo.abc-def.desktop").is_file());
        assert!(arch.join("com.bawdo.abc-def.metainfo.xml").is_file());
        fs::remove_dir_all(&s).unwrap();
    }

    #[test]
    fn source_files_without_git_keep_only_build_inputs() {
        let s = scratch("walk");
        for f in [
            "Cargo.toml",
            "Cargo.lock",
            "LICENSE",
            "rust-toolchain.toml",
            "crates/a/src/lib.rs",
            "xtask/src/main.rs",
            "packaging/templates/x.in",
            ".cargo/config.toml",
            ".gitignore",
            "README.md",
            "docs/plan.md",
            "reports/r.md",
            "research_notes/n.md",
            "docker/Dockerfile.check",
            "target/release/x",
            "target-linux/y",
            "spike/z",
            ".superpowers/notes.md",
        ] {
            let p = s.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        }
        let mut got = source_files(&s).unwrap();
        got.sort();
        assert_eq!(
            got,
            [
                ".cargo/config.toml",
                "Cargo.lock",
                "Cargo.toml",
                "LICENSE",
                "crates/a/src/lib.rs",
                "packaging/templates/x.in",
                "rust-toolchain.toml",
                "xtask/src/main.rs",
            ]
        );
        fs::remove_dir_all(&s).unwrap();
    }

    #[test]
    fn license_is_mit_and_never_names_the_project() {
        let text = fs::read_to_string(root().join("LICENSE")).unwrap();
        assert!(text.starts_with("MIT License\n"));
        assert!(text.contains("Copyright (c) 2026 Keith Bawden"));
        assert!(text.contains("THE SOFTWARE IS PROVIDED \"AS IS\""));
        let n = templates::current_names();
        assert!(crate::lint::find_forms(text.as_bytes(), &crate::lint::name_forms(&n)).is_empty());
    }

    #[test]
    fn source_files_from_git_keep_only_build_inputs() {
        let got = source_files(&root()).unwrap();
        assert!(got.iter().all(|f| is_build_input(f)), "{got:?}");
        for needed in [
            "Cargo.toml",
            "Cargo.lock",
            "LICENSE",
            "rust-toolchain.toml",
            ".cargo/config.toml",
        ] {
            assert!(got.iter().any(|f| f == needed), "{needed} missing");
        }
        assert!(got.iter().any(|f| f == "crates/app-meta/build.rs"));
        assert!(
            !got.iter()
                .any(|f| f.starts_with("docs/") || f == "README.md")
        );
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

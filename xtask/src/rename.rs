//! `rename-test`: build the tree under a throwaway name and prove no trace of
//! the real name survives in the binaries.

use crate::lint::{find_forms, name_forms};
use app_meta::derive::Names;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// One word, so its CamelCase form differs from it only by case: the shape
/// that once made the `.app`'s two executables collide on macOS.
pub const DEFAULT_NAME: &str = "zqscribe";

/// Replace the value of the `name = "..."` line under `[workspace.metadata.app]`.
pub fn rewrite_name(src: &str, new_slug: &str) -> Result<String, String> {
    let mut in_section = false;
    let mut done = false;
    let mut out = String::with_capacity(src.len());
    for line in src.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == "[workspace.metadata.app]";
        }
        if in_section && !done && t.starts_with("name") && t.contains('=') {
            let eol = &line[line.trim_end_matches(['\r', '\n']).len()..];
            out.push_str(&format!("name = \"{new_slug}\"{eol}"));
            done = true;
        } else {
            out.push_str(line);
        }
    }
    if done {
        Ok(out)
    } else {
        Err("no `name = ...` line under [workspace.metadata.app]".to_string())
    }
}

/// Read `(slug, id_prefix)` from the root manifest.
pub fn read_manifest_names(root: &Path) -> Result<(String, String), String> {
    let src = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let mut in_section = false;
    let (mut name, mut prefix) = (None, None);
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == "[workspace.metadata.app]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let v = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "name" => name = Some(v),
                "id_prefix" => prefix = Some(v),
                _ => {}
            }
        }
    }
    match (name, prefix) {
        (Some(n), Some(p)) => Ok((n, p)),
        _ => Err("root manifest lacks name/id_prefix under [workspace.metadata.app]".into()),
    }
}

fn tmp_dir() -> PathBuf {
    std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// A scratch directory owned by exactly one run. The name carries the process
/// id, a nanosecond stamp and a per-process counter, and creation is
/// exclusive (`create_dir`), so two runs, even with the same `TMPDIR`, never
/// share or delete each other's tree. Dropping the guard removes the directory
/// on success and on every error path. Stale directories left by a killed run
/// are deliberately not swept, because they might belong to a live run.
#[derive(Debug)]
pub struct WorkDir {
    path: PathBuf,
}

impl WorkDir {
    pub fn create_in(base: &Path) -> Result<WorkDir, String> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(base).map_err(|e| format!("create {}: {e}", base.display()))?;
        for _ in 0..100 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("rename-check-{}-{nanos}-{n}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(WorkDir { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("create {}: {e}", path.display())),
            }
        }
        Err(format!("no free rename-check dir under {}", base.display()))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn capture(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// One old-name hit: the file, the form found, and its byte offset in the
/// file's contents or, when `in_name` is set, in the entry's own name.
#[derive(Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub form: String,
    pub offset: usize,
    pub in_name: bool,
}

/// Unpacks a gzipped tarball into `dest` (created if missing).
pub fn unpack_tarball(tarball: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let ok = Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(dest)
        .status()
        .map_err(|e| format!("tar: {e}"))?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("tar could not unpack {}", tarball.display()))
    }
}

/// Scans `path` (a file, or every file under a directory) for `forms`, in
/// file contents and in the names of every entry below `path`. Symlinks are
/// not followed; their target text is scanned instead. Returns the number
/// of files scanned and the hits.
pub fn scan_tree(path: &Path, forms: &[String]) -> Result<(usize, Vec<Hit>), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    if meta.is_dir() {
        let (mut files, mut hits) = (0, Vec::new());
        let entries = fs::read_dir(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read {}: {e}", path.display()))?;
            let name = entry.file_name();
            hits.extend(find_forms(name.as_encoded_bytes(), forms).into_iter().map(
                |(form, offset)| Hit {
                    path: entry.path(),
                    form,
                    offset,
                    in_name: true,
                },
            ));
            let (f, h) = scan_tree(&entry.path(), forms)?;
            files += f;
            hits.extend(h);
        }
        return Ok((files, hits));
    }
    let bytes = if meta.file_type().is_symlink() {
        let target = fs::read_link(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        target.as_os_str().as_encoded_bytes().to_vec()
    } else {
        fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?
    };
    let hits = find_forms(&bytes, forms)
        .into_iter()
        .map(|(form, offset)| Hit {
            path: path.to_path_buf(),
            form,
            offset,
            in_name: false,
        })
        .collect();
    Ok((1, hits))
}

/// Paths, relative to the package root, that the `.deb` and the `.rpm` must
/// both install (the brief's install layout).
pub fn expected_package_files(n: &Names) -> Vec<String> {
    vec![
        format!("usr/bin/{}", n.slug),
        format!(
            "{}/{}",
            n.linux_libexec_dir.trim_start_matches('/'),
            n.window_bin
        ),
        format!("usr/share/applications/{}.desktop", n.bundle_id),
        format!("usr/share/metainfo/{}.metainfo.xml", n.bundle_id),
        format!("usr/share/icons/hicolor/512x512/apps/{}.png", n.bundle_id),
    ]
}

/// What runs in the check container over the renamed copy: package, unpack
/// both packages, and record the Linux launcher's `--version`.
pub fn linux_script() -> String {
    r#"set -eu
cargo xtask package-linux
t="$CARGO_TARGET_DIR"
ex="$t/extract"
rm -rf "$ex"
mkdir -p "$ex/deb" "$ex/rpm"
for f in "$t"/dist/*.deb; do dpkg-deb -R "$f" "$ex/deb"; done
for f in "$t"/dist/*.rpm; do (cd "$ex/rpm" && rpm2cpio "$f" | cpio -idm --quiet); done
"$t/release/launcher" --version > "$ex/version.txt"
"#
    .to_string()
}

/// One group of artefacts to scan, with a label for the summary.
struct Artefact {
    label: &'static str,
    path: PathBuf,
}

pub fn run(root: &Path, new_slug: &str, linux: bool) -> Result<(), String> {
    let (old_slug, prefix) = read_manifest_names(root)?;
    let old = Names::from_slug(&old_slug, &prefix)?;
    let new = Names::from_slug(new_slug, &prefix)?;
    let forms = name_forms(&old);

    // Copy the tracked tree.
    let guard = WorkDir::create_in(&tmp_dir())?;
    let copy = guard.path().join("src");
    fs::create_dir_all(&copy).map_err(|e| e.to_string())?;
    let listing = Command::new("git")
        .current_dir(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !listing.status.success() {
        return Err("git ls-files failed".into());
    }
    for rel in listing.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let rel = String::from_utf8_lossy(rel).into_owned();
        let to = copy.join(&rel);
        if !root.join(&rel).is_file() {
            continue; // deleted in the working tree
        }
        if let Some(p) = to.parent() {
            fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        fs::copy(root.join(&rel), &to).map_err(|e| format!("copy {rel}: {e}"))?;
    }
    let manifest = copy.join("Cargo.toml");
    let src = fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
    fs::write(&manifest, rewrite_name(&src, new_slug)?).map_err(|e| e.to_string())?;

    // Build with every machine path that could carry the old name remapped.
    let mut flags = vec![format!("--remap-path-prefix={}=/build", copy.display())];
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    if let Some(ch) = cargo_home {
        flags.push(format!("--remap-path-prefix={}=/cargo", ch.display()));
    }
    if let Ok(sysroot) = capture(Command::new("rustc").args(["--print", "sysroot"])) {
        flags.push(format!("--remap-path-prefix={sysroot}=/rustc-sysroot"));
    }
    let target = copy.join("target");
    println!("rename test: building release copy as {new_slug} (this takes a few minutes)");
    let in_copy = |args: &[&str]| -> Result<(), String> {
        let ok = Command::new("cargo")
            .current_dir(&copy)
            .args(args)
            .env("RUSTFLAGS", flags.join(" "))
            .env("CARGO_TARGET_DIR", &target)
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .status()
            .map_err(|e| e.to_string())?
            .success();
        if ok {
            Ok(())
        } else {
            Err(format!("rename test: cargo {} failed", args.join(" ")))
        }
    };
    in_copy(&["build", "--release", "--locked", "--workspace"])?;

    let release = target.join("release");
    let mut artefacts = vec![
        Artefact {
            label: "launcher",
            path: release.join("launcher"),
        },
        Artefact {
            label: "window",
            path: release.join("window"),
        },
    ];
    // The launcher must announce the new name.
    let mut launchers = vec![release.join("launcher")];

    // The `.app`, laid out by the copy's own xtask so its names are the new ones.
    if cfg!(target_os = "macos") {
        println!("rename test: bundling the copy");
        in_copy(&[
            "run",
            "--locked",
            "-p",
            "xtask",
            "--",
            "bundle-mac",
            "--release",
        ])?;
        let app = target.join("dist").join(format!("{}.app", new.bundle_name));
        launchers.push(app.join("Contents/MacOS").join(&new.slug));
        artefacts.push(Artefact {
            label: ".app",
            path: app,
        });
    }

    for exe in &launchers {
        for arg in ["--version", "--help"] {
            let out = capture(Command::new(exe).arg(arg))?;
            println!("{} {arg}: {out}", exe.display());
            if !out.contains(&new.slug) {
                return Err(format!(
                    "{} {arg} does not show {}",
                    exe.display(),
                    new.slug
                ));
            }
        }
    }

    // The Linux packages, built and unpacked in the check container.
    if linux {
        artefacts.extend(linux_packages(root, &copy, &new)?);
    }

    // Scan every output byte for old-name forms.
    let (mut hits, mut files) = (0usize, 0usize);
    for a in &artefacts {
        let (f, h) = scan_tree(&a.path, &forms)?;
        for hit in &h {
            let place = if hit.in_name { "name" } else { "offset" };
            eprintln!(
                "HIT {} {place} {} form {:?}",
                hit.path.display(),
                hit.offset,
                hit.form
            );
        }
        println!("rename test: {}: {} hits in {f} files", a.label, h.len());
        files += f;
        hits += h.len();
    }
    println!("rename test: {hits} hits in {files} files");
    if hits > 0 {
        return Err(format!("rename test: {hits} old-name hit(s)"));
    }
    Ok(())
}

/// Packages the renamed `copy` in the check container, checks both packages
/// hold the install layout under the new names and that the Linux launcher
/// prints the new name, and returns the artefacts to scan.
fn linux_packages(root: &Path, copy: &Path, new: &Names) -> Result<Vec<Artefact>, String> {
    println!("rename test: packaging the copy in the Linux check container");
    let (image, volume) = crate::ci::check_image(root).map_err(|e| format!("rename test: {e}"))?;
    let env = [
        ("CARGO_TARGET_DIR", "/work/target-linux".to_string()),
        (
            "RUSTFLAGS",
            "--remap-path-prefix=/work=/build --remap-path-prefix=/cargo/home=/cargo".to_string(),
        ),
    ];
    crate::ci::run_in_container(copy, &image, &volume, &env, &linux_script())
        .map_err(|e| format!("rename test: Linux packaging failed: {e}"))?;

    let t = copy.join("target-linux");
    let ex = t.join("extract");
    for kind in ["deb", "rpm"] {
        for rel in expected_package_files(new) {
            let p = ex.join(kind).join(&rel);
            if !p.is_file() {
                return Err(format!("rename test: the .{kind} lacks /{rel}"));
            }
        }
    }
    let pkgbuild = t.join("pkg/arch/PKGBUILD");
    if !pkgbuild.is_file() {
        return Err(format!("rename test: no {}", pkgbuild.display()));
    }
    // The source tarball is compressed, so scanning it raw proves nothing:
    // unpack it and scan its contents and paths as a group of its own.
    let tarball = t
        .join("pkg/arch")
        .join(format!("{}-{}.tar.gz", new.kebab, app_meta::VERSION));
    if !tarball.is_file() {
        return Err(format!("rename test: no {}", tarball.display()));
    }
    let tree = ex.join("arch-source");
    unpack_tarball(&tarball, &tree)?;
    let version = fs::read_to_string(ex.join("version.txt")).unwrap_or_default();
    println!("Linux launcher --version: {}", version.trim());
    if !version.contains(&new.slug) {
        return Err(format!(
            "rename test: Linux launcher --version does not show {}",
            new.slug
        ));
    }
    Ok(vec![
        Artefact {
            label: "Linux binaries",
            path: t.join("release/launcher"),
        },
        Artefact {
            label: "Linux window",
            path: t.join("release/window"),
        },
        Artefact {
            label: "staging and PKGBUILD",
            path: t.join("pkg"),
        },
        Artefact {
            label: ".deb and .rpm files",
            path: t.join("dist"),
        },
        Artefact {
            label: ".deb contents",
            path: ex.join("deb"),
        },
        Artefact {
            label: ".rpm contents",
            path: ex.join("rpm"),
        },
        Artefact {
            label: "Arch source tarball contents",
            path: tree,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_base(tag: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("xtask-rename-test-{}-{tag}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn work_dirs_are_unique_per_run() {
        let base = scratch_base("unique");
        let a = WorkDir::create_in(&base).unwrap();
        let b = WorkDir::create_in(&base).unwrap();
        assert_ne!(a.path(), b.path());
        assert!(a.path().is_dir() && b.path().is_dir());
        drop((a, b));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn work_dir_removed_on_drop() {
        let base = scratch_base("drop");
        let w = WorkDir::create_in(&base).unwrap();
        let p = w.path().to_path_buf();
        fs::create_dir_all(p.join("src/target")).unwrap();
        fs::write(p.join("src/target/x"), b"x").unwrap();
        drop(w);
        assert!(!p.exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn work_dir_removed_on_error_path() {
        fn failing(base: &Path) -> (Result<(), String>, PathBuf) {
            let w = WorkDir::create_in(base).unwrap();
            let p = w.path().to_path_buf();
            fs::write(p.join("x"), b"x").unwrap();
            let r = Err(format!("boom in {}", w.path().display()));
            (r, p)
        }
        let base = scratch_base("err");
        let (r, p) = failing(&base);
        assert!(r.is_err());
        assert!(!p.exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn concurrent_allocation_never_collides() {
        let base = scratch_base("conc");
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let b = base.clone();
                std::thread::spawn(move || WorkDir::create_in(&b).unwrap())
            })
            .collect();
        let dirs: Vec<WorkDir> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let mut paths: Vec<_> = dirs.iter().map(|d| d.path().to_path_buf()).collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), 8);
        drop(dirs);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn scan_tree_counts_files_and_hits_recursively() {
        let base = scratch_base("scan");
        let n = Names::from_slug("abc_def", "com.bawdo").unwrap();
        fs::create_dir_all(base.join("a/b")).unwrap();
        fs::write(base.join("a/clean"), b"nothing here").unwrap();
        fs::write(base.join("a/b/dirty"), b"xx AbcDef yy abc-def").unwrap();
        std::os::unix::fs::symlink("/usr/bin/abc_def", base.join("a/link")).unwrap();
        let (files, hits) = scan_tree(&base, &name_forms(&n)).unwrap();
        assert_eq!(files, 3);
        assert_eq!(hits.len(), 3, "two in the file, one in the link target");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn planted_name_in_the_source_tarball_is_reported() {
        use crate::package_linux::{source_files, source_tarball};
        let base = scratch_base("tarball");
        let n = Names::from_slug("abc_def", "com.bawdo").unwrap();
        let tree = base.join("tree");
        for (f, text) in [
            ("Cargo.toml", "[workspace]\n"),
            ("crates/x/src/lib.rs", "// clean\n"),
            ("crates/x/src/planted.rs", "const N: &str = \"AbcDef\";\n"),
            (
                "crates/abc_def/src/lib.rs",
                "// clean contents, named path\n",
            ),
            ("docs/plan.md", "abc_def is not a build input\n"),
        ] {
            let p = tree.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        let tarball = base.join("arch/t-1.tar.gz");
        source_tarball(&tree, &source_files(&tree).unwrap(), &tarball, "t-1").unwrap();

        let out = base.join("unpacked");
        unpack_tarball(&tarball, &out).unwrap();
        let (files, hits) = scan_tree(&out, &name_forms(&n)).unwrap();
        assert_eq!(files, 4, "docs/ stays out of the tarball");
        let in_content = hits.iter().filter(|h| !h.in_name).count();
        let in_names = hits.iter().filter(|h| h.in_name).count();
        assert_eq!(in_content, 1, "the planted constant");
        assert_eq!(in_names, 1, "the crates/abc_def directory");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn scan_tree_of_a_single_file() {
        let base = scratch_base("scan1");
        let n = Names::from_slug("abc_def", "com.bawdo").unwrap();
        fs::write(base.join("f"), b"ABC_DEF_X").unwrap();
        let (files, hits) = scan_tree(&base.join("f"), &name_forms(&n)).unwrap();
        assert_eq!((files, hits.len()), (1, 1));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn expected_package_files_follow_the_install_layout() {
        let n = Names::from_slug("abc_def", "com.bawdo").unwrap();
        assert_eq!(
            expected_package_files(&n),
            [
                "usr/bin/abc_def",
                "usr/lib/abc_def/abc_def-window",
                "usr/share/applications/com.bawdo.abc-def.desktop",
                "usr/share/metainfo/com.bawdo.abc-def.metainfo.xml",
                "usr/share/icons/hicolor/512x512/apps/com.bawdo.abc-def.png",
            ]
        );
    }

    #[test]
    fn linux_script_packages_and_extracts_both_formats() {
        let s = linux_script();
        assert!(s.contains("cargo xtask package-linux"));
        assert!(s.contains("dpkg-deb -R"));
        assert!(s.contains("rpm2cpio"));
        assert!(s.contains("| cpio -idm"));
        assert!(s.contains("--version"));
    }

    #[test]
    fn rewrite_manifest_changes_only_the_name_line() {
        let src = "[workspace.metadata.app]\nname = \"abc_def\"\nid_prefix = \"com.bawdo\"\n";
        assert_eq!(
            rewrite_name(src, "zq_scribe").unwrap(),
            src.replace("\"abc_def\"", "\"zq_scribe\"")
        );
    }

    #[test]
    fn rewrite_refuses_when_name_line_missing() {
        assert!(rewrite_name("[package]\n", "x").is_err());
    }

    #[test]
    fn rewrite_ignores_other_sections() {
        let src = "[package]\nname = \"keep\"\n[workspace.metadata.app]\nname = \"abc_def\"\n";
        let out = rewrite_name(src, "zq_scribe").unwrap();
        assert!(out.contains("name = \"keep\""));
        assert!(out.contains("name = \"zq_scribe\""));
    }
}

//! `rename-test`: build the tree under a throwaway name and prove no trace of
//! the real name survives in the binaries.

use crate::lint::{find_forms, name_forms};
use app_meta::derive::Names;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_NAME: &str = "zq_scribe";

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

pub fn run(root: &Path, new_slug: &str) -> Result<(), String> {
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
    let status = Command::new("cargo")
        .current_dir(&copy)
        .args(["build", "--release", "--locked", "--workspace"])
        .env("RUSTFLAGS", flags.join(" "))
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("rename test: release build failed".into());
    }

    // Scan every output byte for old-name forms.
    let release = target.join("release");
    let outputs = [release.join("launcher"), release.join("window")];
    let mut hits = 0usize;
    let mut files = 0usize;
    for path in &outputs {
        let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        files += 1;
        for (form, off) in find_forms(&bytes, &forms) {
            hits += 1;
            eprintln!("HIT {} offset {off} form {form:?}", path.display());
        }
    }
    println!("rename test: {hits} hits in {files} files");

    // The launcher must announce the new name.
    for arg in ["--version", "--help"] {
        let out = capture(Command::new(release.join("launcher")).arg(arg))?;
        println!("launcher {arg}: {out}");
        if !out.contains(&new.slug) {
            return Err(format!("launcher {arg} does not show {}", new.slug));
        }
    }
    if hits > 0 {
        return Err(format!("rename test: {hits} old-name hit(s)"));
    }
    Ok(())
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

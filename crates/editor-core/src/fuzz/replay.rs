//! Replays every saved fuzz input through its harness, so a plain
//! `cargo test` guards the seeds and the fixed crashes without nightly.
//!
//! Layout, under `<repo>/fuzz/`: `corpus/<target>/` (seeds, one or more
//! per target), `regressions/<target>/` (fixed crashes) and
//! `known/<target>/` (unfixed crashes, replayed only by `replay_known`).
//! Dotfiles such as `.gitkeep` are skipped.

use std::fs;
use std::path::{Path, PathBuf};

use super::TARGETS;

fn fuzz_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz")
}

fn target_fn(name: &str) -> Option<fn(&[u8])> {
    TARGETS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
}

/// The non-dot files directly inside `dir`, sorted.
fn input_files(dir: &Path) -> Vec<PathBuf> {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    let mut files: Vec<PathBuf> = entries
        .map(|e| {
            e.unwrap_or_else(|e| panic!("cannot read an entry of {}: {e}", dir.display()))
                .path()
        })
        .filter(|p| {
            p.is_file()
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    files.sort();
    files
}

/// Replays every file under `<root>/<kind>/<target>/`. Every subdirectory
/// must name a target. With `require_seeds`, every target must have its
/// directory and at least one input in it. A missing `<kind>/` root is
/// tolerated unless `require_seeds`: an empty set of regressions or known
/// crashes may be kept as no directory at all.
fn replay(kind: &str, require_seeds: bool) {
    let root = fuzz_root().join(kind);
    if root.is_dir() {
        for entry in
            fs::read_dir(&root).unwrap_or_else(|e| panic!("cannot read {}: {e}", root.display()))
        {
            let path = entry.expect("directory entry").path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned();
            if name.starts_with('.') {
                continue;
            }
            assert!(
                target_fn(&name).is_some(),
                "{} names no fuzz target",
                path.display()
            );
        }
    } else {
        assert!(!require_seeds, "missing fuzz directory {}", root.display());
        return;
    }
    for (name, f) in TARGETS {
        let dir = root.join(name);
        if !dir.is_dir() {
            assert!(!require_seeds, "missing seed directory {}", dir.display());
            continue;
        }
        let files = input_files(&dir);
        assert!(
            !require_seeds || !files.is_empty(),
            "no seed files in {}",
            dir.display()
        );
        for file in files {
            eprintln!("replaying {}", file.display());
            let data =
                fs::read(&file).unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            f(&data);
        }
    }
}

#[test]
fn every_harness_accepts_empty_and_junk_input() {
    for (_, f) in TARGETS {
        f(&[]);
        f(&[0xff, 0x00, 0xfe, 0x00, 0x01]);
    }
}

#[test]
fn replay_corpus_and_regressions() {
    replay("corpus", true);
    replay("regressions", false);
}

#[test]
#[ignore = "known, unfixed crashes: see fuzz/README.md"]
fn replay_known() {
    replay("known", false);
}

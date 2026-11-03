//! `ci`: the local gate. Stops at the first failing step and names it.

use crate::{lint, rename};
use std::path::Path;
use std::process::Command;

type Step = (&'static str, fn(&Path) -> Result<(), String>);

fn cargo(root: &Path, args: &[&str]) -> Result<(), String> {
    let ok = Command::new("cargo")
        .current_dir(root)
        .args(args)
        .status()
        .map_err(|e| e.to_string())?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("cargo {} failed", args.join(" ")))
    }
}

fn fmt(root: &Path) -> Result<(), String> {
    cargo(root, &["fmt", "--all", "--check"])
}
fn clippy(root: &Path) -> Result<(), String> {
    cargo(
        root,
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )
}
fn test(root: &Path) -> Result<(), String> {
    cargo(root, &["test", "--workspace"])
}
pub fn lint_name(root: &Path) -> Result<(), String> {
    let (slug, prefix) = rename::read_manifest_names(root)?;
    lint::run(root, &app_meta::derive::Names::from_slug(&slug, &prefix)?)
}
fn rename_test(root: &Path) -> Result<(), String> {
    rename::run(root, rename::DEFAULT_NAME)
}

/// Later tasks add one line each (`lint-literals`, `shell-lines`).
const STEPS: &[Step] = &[
    ("fmt", fmt),
    ("clippy", clippy),
    ("test", test),
    ("lint-name", lint_name),
    ("rename-test", rename_test),
];

pub fn run(root: &Path, linux: bool) -> Result<(), String> {
    if linux {
        return Err("--linux: linux-check is not available yet (Task 17 adds it)".into());
    }
    for (name, step) in STEPS {
        println!("== ci: {name}");
        step(root).map_err(|e| format!("ci step `{name}` failed: {e}"))?;
        println!("ci: {name} OK");
    }
    println!("ci: all {} steps OK", STEPS.len());
    Ok(())
}

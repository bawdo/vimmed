//! `lint-name`: the project name may appear in one place only, the root manifest.

use app_meta::derive::Names;
use std::path::Path;
use std::process::Command;

/// Every spelling of the name to look for, matched case-insensitively.
pub fn name_forms(n: &Names) -> Vec<String> {
    let spaced = n.slug.replace('_', " ");
    let mut forms = vec![
        n.slug.clone(),
        n.kebab.clone(),
        n.bundle_name.clone(),
        n.display_name.clone(),
        n.env_prefix.clone(),
        spaced,
    ];
    forms.sort();
    forms.dedup();
    forms
}

/// Case-insensitive search. Returns `(form, offset)`, one hit per offset
/// (the longest form wins when several match at the same place).
pub fn find_forms(haystack: &[u8], forms: &[String]) -> Vec<(String, usize)> {
    let lower = haystack.to_ascii_lowercase();
    let needles: Vec<(&String, Vec<u8>)> = forms
        .iter()
        .filter(|f| !f.is_empty())
        .map(|f| (f, f.to_ascii_lowercase().into_bytes()))
        .collect();
    let mut hits = Vec::new();
    for offset in 0..lower.len() {
        let best = needles
            .iter()
            .filter(|(_, n)| lower[offset..].starts_with(n))
            .max_by_key(|(_, n)| n.len());
        if let Some((form, _)) = best {
            hits.push(((*form).clone(), offset));
        }
    }
    hits
}

const LINT_PATHS: [&str; 6] = [
    "crates/",
    "xtask/",
    "packaging/",
    "docker/",
    "scripts/",
    "README.md",
];

/// A `git grep -n` line is allowed only when it is the `name = ...` line of
/// the root manifest.
pub fn is_allowed_hit(line: &str) -> bool {
    match line.strip_prefix("Cargo.toml:") {
        Some(rest) => rest
            .split_once(':')
            .is_some_and(|(_, text)| text.trim_start().starts_with("name = ")),
        None => false,
    }
}

pub fn run(root: &Path, names: &Names) -> Result<(), String> {
    let forms = name_forms(names);
    let mut cmd = Command::new("git");
    cmd.current_dir(root).args(["grep", "-I", "-i", "-n", "-F"]);
    for f in &forms {
        cmd.args(["-e", f]);
    }
    cmd.arg("--").arg("Cargo.toml").args(LINT_PATHS);
    let out = cmd.output().map_err(|e| format!("git grep: {e}"))?;
    match out.status.code() {
        Some(0) | Some(1) => {}
        _ => {
            return Err(format!(
                "git grep failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let bad: Vec<&str> = text.lines().filter(|l| !is_allowed_hit(l)).collect();
    if bad.is_empty() {
        println!("lint-name: OK");
        Ok(())
    } else {
        for l in &bad {
            eprintln!("{l}");
        }
        Err(format!(
            "lint-name: {} hit(s) outside the root manifest",
            bad.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_form_case_insensitively() {
        let n = Names::from_slug("abc_def", "com.bawdo").unwrap();
        let hits = find_forms(b"xx AbcDef yy ABC_DEF_T0 abc-def", &name_forms(&n));
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn finds_spaced_variant() {
        let n = Names::from_slug("abc_def", "com.bawdo").unwrap();
        assert_eq!(find_forms(b"Abc Def", &name_forms(&n)).len(), 1);
    }

    #[test]
    fn only_the_manifest_name_line_is_allowed() {
        assert!(is_allowed_hit("Cargo.toml:17:name = \"abc_def\""));
        assert!(!is_allowed_hit("Cargo.toml:3:# abc_def"));
        assert!(!is_allowed_hit("crates/x/src/lib.rs:1:name = \"abc_def\""));
    }
}

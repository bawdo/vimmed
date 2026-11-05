//! `lint-name`: the project name may appear in one place only, the root manifest.
//! The root `README.md` may also carry it in its title and in link reference
//! definitions inside a `name-lint:allow` block (the badge URLs).

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

const LINT_PATHS: [&str; 7] = [
    "crates/",
    "xtask/",
    "packaging/",
    "docker/",
    "scripts/",
    "README.md",
    "LICENSE",
];

/// A `git grep -n` line (`path:lineno:text`) is allowed only when it is the
/// `name = ...` line of the root manifest, or the root `README.md` title: a
/// level 1 heading whose whole text is one of the name forms.
pub fn is_allowed_hit(line: &str, forms: &[String]) -> bool {
    if let Some(rest) = line.strip_prefix("Cargo.toml:") {
        return rest.split_once(':').is_some_and(|(_, text)| {
            let text = text.trim_start();
            text.starts_with("name = ") || text.starts_with("repository = ")
        });
    }
    if let Some(rest) = line.strip_prefix("README.md:") {
        return rest.split_once(':').is_some_and(|(_, text)| {
            let text = text.trim_end();
            forms
                .iter()
                .any(|f| text.eq_ignore_ascii_case(&format!("# {f}")))
        });
    }
    false
}

/// Markers around the root `README.md` block of link reference definitions
/// (badge and link URLs) that may carry the name.
const ALLOW_START: &str = "<!-- name-lint:allow:start -->";
const ALLOW_END: &str = "<!-- name-lint:allow:end -->";

/// The 1-based, inclusive line ranges between each pair of allow markers.
/// A marker out of order or left unclosed is an error, so a broken block
/// can never exempt more than it shows.
pub fn readme_allow_ranges(text: &str) -> Result<Vec<(usize, usize)>, String> {
    let mut ranges = Vec::new();
    let mut open: Option<usize> = None;
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        match (line.trim(), open) {
            (ALLOW_START, None) => open = Some(n),
            (ALLOW_START, Some(at)) => {
                return Err(format!(
                    "README.md:{n}: allow block opened again (opened at line {at})"
                ));
            }
            (ALLOW_END, Some(at)) => {
                ranges.push((at + 1, n - 1));
                open = None;
            }
            (ALLOW_END, None) => {
                return Err(format!(
                    "README.md:{n}: allow block closed but never opened"
                ));
            }
            _ => {}
        }
    }
    match open {
        Some(at) => Err(format!("README.md:{at}: allow block never closed")),
        None => Ok(ranges),
    }
}

/// A Markdown link reference definition and nothing else: `[id]: url`, at
/// the start of the line, with a non-empty id and a single URL token.
pub fn is_link_definition(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('[') else {
        return false;
    };
    let Some((id, url)) = rest.split_once("]: ") else {
        return false;
    };
    !id.is_empty()
        && !id.contains(['[', ']'])
        && !url.is_empty()
        && !url.contains(char::is_whitespace)
}

/// A `git grep -n` line from the root `README.md` that is a link reference
/// definition inside an allow block.
pub fn is_allowed_block_hit(line: &str, ranges: &[(usize, usize)]) -> bool {
    let Some(rest) = line.strip_prefix("README.md:") else {
        return false;
    };
    let Some((lineno, text)) = rest.split_once(':') else {
        return false;
    };
    let Ok(n) = lineno.parse::<usize>() else {
        return false;
    };
    ranges.iter().any(|&(a, b)| (a..=b).contains(&n)) && is_link_definition(text.trim_end())
}

pub fn run(root: &Path, names: &Names) -> Result<(), String> {
    let forms = name_forms(names);
    let readme = match std::fs::read_to_string(root.join("README.md")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read README.md: {e}")),
    };
    let ranges = readme_allow_ranges(&readme)?;
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
    let bad: Vec<&str> = text
        .lines()
        .filter(|l| !is_allowed_hit(l, &forms) && !is_allowed_block_hit(l, &ranges))
        .collect();
    if bad.is_empty() {
        println!("lint-name: OK");
        Ok(())
    } else {
        for l in &bad {
            eprintln!("{l}");
        }
        Err(format!(
            "lint-name: {} hit(s) outside the root manifest, README title and README allow block",
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
        assert!(is_allowed_hit("Cargo.toml:17:name = \"abc_def\"", &[]));
        assert!(!is_allowed_hit("Cargo.toml:3:# abc_def", &[]));
        assert!(!is_allowed_hit(
            "crates/x/src/lib.rs:1:name = \"abc_def\"",
            &[]
        ));
    }

    #[test]
    fn readme_title_heading_is_allowed() {
        let forms = vec!["abc_def".to_string(), "Abc_Def".to_string()];
        assert!(is_allowed_hit("README.md:9:# abc_def", &forms));
        assert!(is_allowed_hit("README.md:1:# Abc_Def", &forms));
    }

    #[test]
    fn readme_name_elsewhere_is_not_allowed() {
        let forms = vec!["abc_def".to_string()];
        assert!(!is_allowed_hit("README.md:12:Install abc_def with", &forms));
        assert!(!is_allowed_hit("README.md:3:## abc_def", &forms));
        assert!(!is_allowed_hit("README.md:3:# abc_def editor", &forms));
        assert!(!is_allowed_hit("docs/README.md:1:# abc_def", &forms));
    }

    #[test]
    fn root_manifest_repository_line_is_allowed() {
        let url = "Cargo.toml:14:repository = \"https://github.com/x/abc_def\"";
        assert!(is_allowed_hit(url, &[]));
        assert!(!is_allowed_hit(
            "crates/x/Cargo.toml:5:repository = \"https://github.com/x/abc_def\"",
            &[]
        ));
        assert!(!is_allowed_hit(
            "Cargo.toml:14:homepage = \"https://github.com/x/abc_def\"",
            &[]
        ));
    }

    const BLOCK: &str = "# abc_def\n\
        \n\
        [![CI][ci-badge]][ci]\n\
        \n\
        <!-- name-lint:allow:start -->\n\
        [ci-badge]: https://example.com/abc_def/badge.svg\n\
        [ci]: https://example.com/abc_def\n\
        <!-- name-lint:allow:end -->\n";

    #[test]
    fn allow_block_covers_the_lines_between_the_markers() {
        assert_eq!(readme_allow_ranges(BLOCK), Ok(vec![(6, 7)]));
        assert_eq!(readme_allow_ranges("# abc_def\n"), Ok(vec![]));
    }

    #[test]
    fn allow_block_markers_must_pair() {
        let unclosed = "<!-- name-lint:allow:start -->\n[a]: https://x/abc_def\n";
        assert!(readme_allow_ranges(unclosed).is_err());
        let stray_end = "<!-- name-lint:allow:end -->\n";
        assert!(readme_allow_ranges(stray_end).is_err());
        let nested = "<!-- name-lint:allow:start -->\n<!-- name-lint:allow:start -->\n";
        assert!(readme_allow_ranges(nested).is_err());
    }

    #[test]
    fn link_definition_shape() {
        assert!(is_link_definition(
            "[ci-badge]: https://example.com/abc_def/badge.svg"
        ));
        assert!(is_link_definition("[ci]: https://example.com/abc_def"));
        assert!(!is_link_definition(
            "Install abc_def from [ci]: https://example.com"
        ));
        assert!(!is_link_definition(
            "[ci]: https://example.com/abc_def and more"
        ));
        assert!(!is_link_definition("[]: https://example.com/abc_def"));
        assert!(!is_link_definition("[ci]:https://example.com/abc_def"));
        assert!(!is_link_definition(" [ci]: https://example.com/abc_def"));
    }

    #[test]
    fn link_definitions_inside_the_block_are_allowed() {
        let ranges = readme_allow_ranges(BLOCK).unwrap();
        assert!(is_allowed_block_hit(
            "README.md:6:[ci-badge]: https://example.com/abc_def/badge.svg",
            &ranges
        ));
        assert!(is_allowed_block_hit(
            "README.md:7:[ci]: https://example.com/abc_def",
            &ranges
        ));
    }

    #[test]
    fn block_hits_outside_the_block_or_not_definitions_fail() {
        let ranges = vec![(6, 7)];
        // A definition outside the block.
        assert!(!is_allowed_block_hit(
            "README.md:9:[ci]: https://example.com/abc_def",
            &ranges
        ));
        // Prose inside the block.
        assert!(!is_allowed_block_hit(
            "README.md:6:Install abc_def",
            &ranges
        ));
        // Another file with the same line numbers.
        assert!(!is_allowed_block_hit(
            "docs/README.md:6:[ci]: https://example.com/abc_def",
            &ranges
        ));
        // No block at all.
        assert!(!is_allowed_block_hit(
            "README.md:6:[ci]: https://example.com/abc_def",
            &[]
        ));
    }

    #[test]
    fn manifest_name_line_still_allowed() {
        assert!(is_allowed_hit("Cargo.toml:31:name = \"abc_def\"", &[]));
    }
}

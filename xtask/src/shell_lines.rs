//! `shell-lines`: each shell crate (and the window binary) stays under
//! `LIMIT` counted lines. A counted line is a non-blank line of a `.rs`
//! file that is not only a comment. Crates that don't exist yet are listed
//! as absent and pass.

use std::path::{Path, PathBuf};

pub const LIMIT: usize = 2_500;
const CRATES: [&str; 3] = ["crates/shell-mac", "crates/shell-gtk", "crates/window"];

/// Counted lines in one Rust source. `//` comments (doc comments too) and
/// `/* ... */` blocks are skipped; a line with code before or after a
/// comment counts. Comment markers inside strings are not understood,
/// which is fine for ordinary formatted code.
pub fn count(text: &str) -> usize {
    let mut in_block = false;
    let mut n = 0;
    for line in text.lines() {
        let mut rest = line.trim();
        let mut code = false;
        while !rest.is_empty() {
            if in_block {
                match rest.find("*/") {
                    Some(i) => {
                        in_block = false;
                        rest = rest[i + 2..].trim_start();
                    }
                    None => rest = "",
                }
            } else if rest.starts_with("//") {
                rest = "";
            } else if let Some(r) = rest.strip_prefix("/*") {
                in_block = true;
                rest = r;
            } else {
                code = true;
                let next = [rest.find("//"), rest.find("/*")]
                    .into_iter()
                    .flatten()
                    .min();
                rest = next.map_or("", |i| &rest[i..]);
            }
        }
        n += usize::from(code);
    }
    n
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "target") {
                rust_files(&p, out)?;
            }
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
    Ok(())
}

pub fn run(root: &Path) -> Result<(), String> {
    let mut over = Vec::new();
    for c in CRATES {
        let dir = root.join(c);
        if !dir.is_dir() {
            println!("shell-lines: {c} absent");
            continue;
        }
        let mut files = Vec::new();
        rust_files(&dir, &mut files).map_err(|e| format!("{c}: {e}"))?;
        let mut total = 0;
        for f in &files {
            let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
            total += count(&text);
        }
        let verdict = if total > LIMIT { "OVER" } else { "ok" };
        println!("shell-lines: {c} {total} / {LIMIT} {verdict}");
        if total > LIMIT {
            over.push(c);
        }
    }
    if over.is_empty() {
        Ok(())
    } else {
        Err(format!("over {LIMIT} lines: {}", over.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_comment_lines_do_not_count() {
        let src = "//! doc\n\nuse a;\n    // note\n/// item doc\nfn f() {}\n";
        assert_eq!(count(src), 2);
    }

    #[test]
    fn block_comments_span_lines() {
        let src = "/* one\n two\n three */\nlet x = 1; /* tail */\n/* a */ let y = 2;\n";
        assert_eq!(count(src), 2);
    }

    #[test]
    fn code_with_a_trailing_comment_counts() {
        assert_eq!(count("let a = 1; // why\n"), 1);
        assert_eq!(count("   \n\t\n"), 0);
    }

    #[test]
    fn real_crates_are_measured() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        assert!(run(&root).is_ok());
    }
}

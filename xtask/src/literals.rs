//! `lint-literals`: UI crates may not hard-code colours or font families.
//! Everything comes from `editor-core`'s token layer.
//!
//! Scanned: `crates/editor-render`, `crates/window`, `crates/shell-*` (any that
//! exist). Skipped: `#[cfg(test)]` items and `tests/` directories.
//!
//! This is a line-based heuristic, not a parser. Limits:
//! - Colours: a string literal containing `#` plus six or more hex digits,
//!   the text `Rgba {`, or a `from_rgba8(` call. Colours built any other way
//!   (`[0.1, 0.2, 0.3, 1.0]`, `Color::WHITE`) are not caught; review them.
//! - Font families: a string literal is flagged when the code before it on the
//!   same line mentions `family` (`family: "X"`, `Font::family("X")`,
//!   `FontFamily::Name("X")`), or when the whole literal is a well-known
//!   family name or generic keyword (see `FAMILY_NAMES`). A family passed via
//!   a differently named binding on another line, or an unlisted family name,
//!   gets through. Tokens' `system-ui` / `system-mono` sentinels live in
//!   `editor-core`, outside the scanned crates.
//! - Strings spanning several lines and raw strings (`r#"..."#`) are not
//!   understood. Brace counting for `#[cfg(test)]` skipping ignores braces in
//!   strings and comments, which is right for ordinary formatted code.
//! - Sizes, weights and spacing literals are a review item, not linted.

use std::path::{Path, PathBuf};

/// Whole-literal matches (case-insensitive) that count as a family name.
const FAMILY_NAMES: &[&str] = &[
    "arial",
    "helvetica",
    "helvetica neue",
    "times",
    "times new roman",
    "courier",
    "courier new",
    "menlo",
    "monaco",
    "consolas",
    "georgia",
    "verdana",
    "roboto",
    "inter",
    "jetbrains mono",
    "sf pro",
    "sf mono",
    "segoe ui",
    "dejavu sans",
    "dejavu sans mono",
    "noto sans",
    "system-ui",
    "system-mono",
    "monospace",
    "sans-serif",
    "serif",
];

#[derive(Debug, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
    pub reason: &'static str,
}

/// One line split into its string literals and its code (literal contents and
/// the trailing `//` comment removed).
struct Line {
    /// `(byte offset of the opening quote, literal contents)`
    literals: Vec<(usize, String)>,
    code: String,
    opens: usize,
    closes: usize,
}

fn split_line(line: &str) -> Line {
    let mut out = Line {
        literals: Vec::new(),
        code: String::new(),
        opens: 0,
        closes: 0,
    };
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        match c {
            '/' if chars.get(i + 1).map(|x| x.1) == Some('/') => break,
            '\'' if chars.get(i + 1).map(|x| x.1) == Some('"')
                && chars.get(i + 2).map(|x| x.1) == Some('\'') =>
            {
                i += 3; // the char literal '"'
                continue;
            }
            '"' => {
                let mut text = String::new();
                i += 1;
                while i < chars.len() && chars[i].1 != '"' {
                    if chars[i].1 == '\\' && i + 1 < chars.len() {
                        text.push(chars[i].1);
                        i += 1;
                    }
                    text.push(chars[i].1);
                    i += 1;
                }
                out.literals.push((pos, text));
                out.code.push_str("\"\"");
            }
            _ => {
                match c {
                    '{' => out.opens += 1,
                    '}' => out.closes += 1,
                    _ => {}
                }
                out.code.push(c);
            }
        }
        i += 1;
    }
    out
}

fn has_hex_colour(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len()).any(|i| {
        b[i] == b'#'
            && b[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count()
                >= 6
    })
}

fn has_rgba_literal(code: &str) -> bool {
    code.match_indices("Rgba").any(|(i, _)| {
        let before_ok = !code[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        before_ok && code[i + 4..].trim_start().starts_with('{')
    })
}

fn is_family_literal(before: &str, text: &str) -> bool {
    before.to_ascii_lowercase().contains("family")
        || FAMILY_NAMES.contains(&text.to_ascii_lowercase().as_str())
}

/// Scans one source file's text. Line numbers are 1-based.
pub fn scan_source(src: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut pending_cfg_test = false;
    let mut skip_depth: i64 = 0;
    for (n, raw) in src.lines().enumerate() {
        let line = split_line(raw);
        if skip_depth > 0 {
            skip_depth += line.opens as i64 - line.closes as i64;
            continue;
        }
        if line.code.trim_start().starts_with("#[cfg(test)]") {
            pending_cfg_test = true;
        }
        if pending_cfg_test {
            if line.opens > 0 {
                skip_depth = line.opens as i64 - line.closes as i64;
                pending_cfg_test = false;
            } else if line.code.contains(';') {
                pending_cfg_test = false;
            }
            continue;
        }
        let mut push = |reason| {
            hits.push(Hit {
                line: n + 1,
                reason,
            })
        };
        for (pos, text) in &line.literals {
            if has_hex_colour(text) {
                push("hex colour literal");
            }
            if is_family_literal(&raw[..*pos], text) {
                push("font family literal");
            }
        }
        if has_rgba_literal(&line.code) {
            push("Rgba literal");
        }
        if line.code.contains("from_rgba8(") {
            push("from_rgba8 call");
        }
    }
    hits
}

fn scan_dir(dir: &Path, out: &mut Vec<(PathBuf, Hit)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            let skip = matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some("tests" | "target")
            );
            if !skip {
                scan_dir(&path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            out.extend(scan_source(&text).into_iter().map(|h| (path.clone(), h)));
        }
    }
    Ok(())
}

pub fn run(root: &Path) -> Result<(), String> {
    let crates = root.join("crates");
    let mut dirs = vec![crates.join("editor-render"), crates.join("window")];
    if let Ok(entries) = std::fs::read_dir(&crates) {
        dirs.extend(
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("shell-"))
                }),
        );
    }
    let mut hits = Vec::new();
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        scan_dir(dir, &mut hits)?;
    }
    for (path, hit) in &hits {
        let shown = path.strip_prefix(root).unwrap_or(path);
        eprintln!("{}:{}: {}", shown.display(), hit.line, hit.reason);
    }
    if hits.is_empty() {
        println!("lint-literals: OK");
        Ok(())
    } else {
        Err(format!(
            "lint-literals: {} hard-coded colour or family literal(s); use tokens",
            hits.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reasons(src: &str) -> Vec<&'static str> {
        scan_source(src).into_iter().map(|h| h.reason).collect()
    }

    #[test]
    fn flags_hex_colour_in_string() {
        assert_eq!(reasons("let c = \"#1A2B3C\";"), vec!["hex colour literal"]);
        assert_eq!(
            reasons("let c = \"#1A2B3C80\";"),
            vec!["hex colour literal"]
        );
    }
    #[test]
    fn flags_rgba_struct_literal() {
        assert_eq!(
            reasons("let c = Rgba { r: 1, g: 2, b: 3, a: 4 };"),
            vec!["Rgba literal"]
        );
        assert_eq!(reasons("let c = Rgba{ r: 1 };"), vec!["Rgba literal"]);
    }
    #[test]
    fn flags_from_rgba8() {
        assert_eq!(
            reasons("Color::from_rgba8(1, 2, 3, 4)"),
            vec!["from_rgba8 call"]
        );
    }
    #[test]
    fn flags_family_literals() {
        assert_eq!(
            reasons("let f = Font::family(\"Menlo\");"),
            vec!["font family literal"]
        );
        assert_eq!(
            reasons("let family = \"Whatever\";"),
            vec!["font family literal"]
        );
        assert_eq!(reasons("draw(\"Helvetica\")"), vec!["font family literal"]);
    }
    #[test]
    fn ignores_clean_code_and_comments() {
        let src =
            "let c = tokens.color(TokenId::ColorBg); // #FFFFFF Rgba { }\nlet s = \"hello\";\n";
        assert!(reasons(src).is_empty());
        assert!(reasons("let n = \"#12345\"; let t = SomeRgba { x };").is_empty());
        assert!(reasons("let c = Rgba::from(x); let e = \"Rgba {\";").is_empty());
    }
    #[test]
    fn skips_cfg_test_modules_only() {
        let src = "#[cfg(test)]\nmod tests {\n    fn t() { let _ = \"#FFFFFF\"; }\n}\nfn f() { let _ = \"#000000\"; }\n";
        assert_eq!(
            scan_source(src),
            vec![Hit {
                line: 5,
                reason: "hex colour literal"
            }]
        );
    }
    #[test]
    fn cfg_test_use_statement_skips_one_item_only() {
        let src = "#[cfg(test)]\nuse x::y;\nlet _ = \"#000000\";\n";
        assert_eq!(scan_source(src).len(), 1);
    }
}

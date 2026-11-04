//! Link target classification: decides what a markdown link target points at.
//!
//! Pure apart from the filesystem existence checks. Opening the result is the
//! host's job.

use std::path::{Component, Path, PathBuf};

/// What a link target points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A target with a URL scheme, passed through untouched (fragment kept).
    External(String),
    /// An existing `.md` or `.markdown` file.
    Markdown(PathBuf),
    /// Any other existing path.
    Local(PathBuf),
    /// A path that does not exist, carrying the resolved path.
    Missing(PathBuf),
}

/// True when `target` starts with a scheme matching
/// `^[A-Za-z][A-Za-z0-9+.-]*:`.
///
/// A Windows drive letter such as `C:` matches too, so it is `External`. Only
/// macOS and Linux are targets, so that is accepted.
fn has_scheme(target: &str) -> bool {
    let mut chars = target.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for c in chars {
        match c {
            ':' => return true,
            c if c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-') => {}
            _ => return false,
        }
    }
    false
}

/// Decodes `%XX` escapes (like moon_edit's `Uri.decodeFull` in
/// `link_scanner_service.dart`). Malformed escapes are kept literally, and a
/// result that is not valid UTF-8 falls back to the undecoded input.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Collapses `.` and `..` lexically (moon_edit does the same with `p.normalize`).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !path.is_absolute() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// Classifies `target`, resolving relative paths against `file_dir`.
///
/// Rules:
/// - A scheme makes it `External` (the string is returned as given).
/// - The `#fragment` is stripped, then `%XX` escapes are decoded.
/// - `~` is not expanded: `~/x` is a relative path named `~`.
/// - An absolute path is used as is. A relative one joins onto `file_dir`.
/// - The `.md` / `.markdown` check is case-insensitive.
/// - A bare `#frag` (empty path) refers to the current file, which this
///   function cannot know. It returns `Missing("")`; the caller should treat a
///   target starting with `#` as a no-op before calling.
pub fn classify(target: &str, file_dir: &Path) -> LinkTarget {
    if has_scheme(target) {
        return LinkTarget::External(target.to_string());
    }
    let without_fragment = target.split('#').next().unwrap_or("");
    if without_fragment.is_empty() {
        return LinkTarget::Missing(PathBuf::new());
    }
    let decoded = percent_decode(without_fragment);
    let path = Path::new(&decoded);
    let resolved = if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&file_dir.join(path))
    };
    if !resolved.exists() {
        LinkTarget::Missing(resolved)
    } else if is_markdown(&resolved) {
        LinkTarget::Markdown(resolved)
    } else {
        LinkTarget::Local(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let p =
                std::env::temp_dir().join(format!("abc_def-links-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        fn file(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, "x").unwrap();
            p
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn classify_scheme_md_local_missing() {
        let d = Dir::new("kinds");
        let md = d.file("a.md");
        let mk = d.file("b.markdown");
        let txt = d.file("c.txt");
        assert_eq!(
            classify("https://example.com/x#y", &d.0),
            LinkTarget::External("https://example.com/x#y".into())
        );
        assert_eq!(
            classify("mailto:a@b.c", &d.0),
            LinkTarget::External("mailto:a@b.c".into())
        );
        assert_eq!(classify("a.md", &d.0), LinkTarget::Markdown(md));
        assert_eq!(classify("b.markdown", &d.0), LinkTarget::Markdown(mk));
        assert_eq!(classify("c.txt", &d.0), LinkTarget::Local(txt));
        assert_eq!(
            classify("nope.md", &d.0),
            LinkTarget::Missing(d.0.join("nope.md"))
        );
    }

    #[test]
    fn fragment_stripped() {
        let d = Dir::new("frag");
        let md = d.file("a.md");
        assert_eq!(classify("a.md#section", &d.0), LinkTarget::Markdown(md));
        assert_eq!(
            classify("gone.md#section", &d.0),
            LinkTarget::Missing(d.0.join("gone.md"))
        );
    }

    #[test]
    fn relative_to_file_dir() {
        let d = Dir::new("rel");
        let sub = d.file("sub/n.md");
        let up = d.file("top.txt");
        let sub_dir = d.0.join("sub");
        assert_eq!(
            classify("n.md", &sub_dir),
            LinkTarget::Markdown(sub.clone())
        );
        assert_eq!(
            classify("./n.md", &sub_dir),
            LinkTarget::Markdown(sub.clone())
        );
        assert_eq!(classify("../top.txt", &sub_dir), LinkTarget::Local(up));
        assert_eq!(classify("sub/n.md", &d.0), LinkTarget::Markdown(sub));
    }

    #[test]
    fn absolute_path_ignores_file_dir() {
        let d = Dir::new("abs");
        let md = d.file("a.md");
        let other = Dir::new("abs-other");
        assert_eq!(
            classify(md.to_str().unwrap(), &other.0),
            LinkTarget::Markdown(md)
        );
    }

    #[test]
    fn extension_check_is_case_insensitive() {
        let d = Dir::new("case");
        let md = d.file("A.MD");
        assert_eq!(classify("A.MD", &d.0), LinkTarget::Markdown(md));
    }

    #[test]
    fn percent_encoded_spaces_decode() {
        let d = Dir::new("pct");
        let md = d.file("my note.md");
        assert_eq!(classify("my%20note.md", &d.0), LinkTarget::Markdown(md));
        // A malformed escape stays literal.
        assert_eq!(
            classify("100%.md", &d.0),
            LinkTarget::Missing(d.0.join("100%.md"))
        );
    }

    #[test]
    fn tilde_is_not_expanded() {
        let d = Dir::new("tilde");
        assert_eq!(
            classify("~/x.md", &d.0),
            LinkTarget::Missing(d.0.join("~/x.md"))
        );
    }

    #[test]
    fn bare_fragment_is_missing_empty() {
        let d = Dir::new("bare");
        assert_eq!(classify("#top", &d.0), LinkTarget::Missing(PathBuf::new()));
    }

    #[test]
    fn schemeless_lookalikes_are_paths() {
        let d = Dir::new("look");
        // Space before the colon, or a leading digit, is not a scheme.
        assert!(matches!(classify("a b:c", &d.0), LinkTarget::Missing(_)));
        assert!(matches!(classify("1ab:c", &d.0), LinkTarget::Missing(_)));
        // A drive letter matches the scheme rule, so it is External.
        assert_eq!(
            classify("C:\\x.md", &d.0),
            LinkTarget::External("C:\\x.md".into())
        );
    }
}

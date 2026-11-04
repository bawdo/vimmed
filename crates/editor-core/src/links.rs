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

/// Decodes `%XX` escapes (moon_edit uses `Uri.decodeFull`, but only for image
/// references in `link_scanner_service.dart`; applying it to links is our choice). Malformed escapes are kept literally, and a
/// result that is not valid UTF-8 falls back to the undecoded input.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
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

/// Collapses `.` and `..` lexically, ignoring symlinks (the same as moon_edit's
/// `p.normalize`). A `..` pops only a normal component; otherwise it is kept
/// (relative paths) or dropped at the root (absolute paths).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
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

/// True for a target that points at the current file: empty, or starting
/// with `#`. Callers filter these out before `classify`.
pub fn is_self_fragment(target: &str) -> bool {
    target.is_empty() || target.starts_with('#')
}

/// Classifies `target`, resolving relative paths against `file_dir`.
///
/// Rules:
/// - A scheme makes it `External` (the string is returned as given).
/// - The `#fragment` is stripped, then `%XX` escapes are decoded.
/// - `~` is not expanded: `~/x` is a relative path named `~`.
/// - An absolute path is used as is. A relative one joins onto `file_dir`.
/// - The `.md` / `.markdown` check is case-insensitive.
/// - A self-fragment (see `is_self_fragment`: empty, or a bare `#frag`)
///   refers to the current file, which this function cannot know. Callers must
///   filter it out first; if they do not, it returns `Missing("")`, which must
///   never be shown as "no such file".
/// - The case-insensitive extension check and the percent-decoding are
///   deliberate divergences from moon_edit, not parity: its `.md` checks
///   (app_state.dart, file_operations_service.dart) are case-sensitive, and it
///   uses `Uri.decodeFull` only for image references.
/// - Only a regular file is `Markdown`; a directory named `x.md` is `Local`.
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
    } else if resolved.is_file() && is_markdown(&resolved) {
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

    #[test]
    fn leading_parent_dirs_survive_a_relative_base() {
        assert_eq!(normalize(Path::new("../../x")), PathBuf::from("../../x"));
        assert_eq!(normalize(Path::new("a/../../x")), PathBuf::from("../x"));
        assert_eq!(normalize(Path::new("a/./b/../c")), PathBuf::from("a/c"));
        assert_eq!(normalize(Path::new("/../x")), PathBuf::from("/x"));
        assert!(matches!(
            classify("../../nope.md", Path::new("rel/base")),
            LinkTarget::Missing(p) if p.as_path() == Path::new("nope.md")
        ));
        assert!(matches!(
            classify("../../../nope.md", Path::new("rel/base")),
            LinkTarget::Missing(p) if p.as_path() == Path::new("../nope.md")
        ));
    }

    #[test]
    fn self_fragment_detection() {
        assert!(is_self_fragment(""));
        assert!(is_self_fragment("#x"));
        assert!(!is_self_fragment("a#x"));
        assert!(!is_self_fragment("a.md"));
    }

    #[test]
    fn empty_target_is_missing_empty() {
        let d = Dir::new("empty");
        assert_eq!(classify("", &d.0), LinkTarget::Missing(PathBuf::new()));
    }

    #[test]
    fn percent_decode_needs_two_hex_digits() {
        assert_eq!(percent_decode("%+f"), "%+f");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%20"), " ");
        // %FF alone is not valid UTF-8: keep the input as written.
        assert_eq!(percent_decode("a%FFb"), "a%FFb");
    }

    #[test]
    fn encoded_hash_is_part_of_the_name() {
        let d = Dir::new("hash");
        let md = d.file("a#b.md");
        assert_eq!(classify("a%23b.md", &d.0), LinkTarget::Markdown(md));
    }

    #[test]
    fn directory_named_like_markdown_is_local() {
        let d = Dir::new("dirmd");
        let sub = d.0.join("x.md");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(classify("x.md", &d.0), LinkTarget::Local(sub));
    }
}

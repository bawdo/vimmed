//! Font files on macOS. The body and monospace faces are fixed system
//! paths. The fallback set (symbols, emoji and a monospace backstop) is
//! found through CoreText once, then cached in the same file the Linux
//! shell uses (`<cache>/<kebab>/font-paths`, which `bench --first-run`
//! deletes), so later starts never ask CoreText. The file is keyed on the
//! mtimes of the font directories, so a font install or an OS update
//! expires it, and the renderer's codepoint lines with it.
//!
//! A codepoint none of those faces covers is looked up lazily, on the
//! fallback worker: `CTFontCreateForString` from the system UI font, then
//! that font's `kCTFontURLAttribute`. Core Text is warmed with one throwaway
//! lookup once the first frame is out.

use std::path::{Path, PathBuf};

/// The codepoint of the throwaway warm-up lookup: one no start-up face
/// covers, so Core Text walks its cascade list.
pub const WARM_UP: char = '\u{4E00}';

/// The PostScript name of Apple's catch-all face.
pub const LAST_RESORT: &str = "LastResort";

pub const SANS: &str = "/System/Library/Fonts/SFNS.ttf";
pub const SANS_ITALIC: &str = "/System/Library/Fonts/SFNSItalic.ttf";
pub const MONO: &str = "/System/Library/Fonts/SFNSMono.ttf";
pub const MONO_ITALIC: &str = "/System/Library/Fonts/SFNSMonoItalic.ttf";

/// PostScript names of the fallback faces: Apple Symbols, Apple Color Emoji
/// and Menlo.
pub const FALLBACK_NAMES: [&str; 3] = ["AppleSymbols", "AppleColorEmoji", "Menlo-Regular"];

/// The first line of the cache file. A file without it is ignored.
const CACHE_HEADER: &str = "font-paths v1 macos";

/// `$XDG_CACHE_HOME/<kebab>/font-paths`, or `~/.cache/...` without it.
pub fn cache_path(xdg_cache: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg_cache
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| home.map(|h| h.join(".cache")))?;
    Some(base.join(app_meta::KEBAB).join("font-paths"))
}

/// The font directories whose mtimes key the cache: the system's, the
/// machine's and the user's. Installing or removing a font, or an OS update,
/// changes one of them, which expires the cache and the renderer's codepoint
/// lines with it (the cache is rewritten without them).
pub fn font_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut v = vec![
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/Library/Fonts"),
    ];
    v.extend(home.map(|h| h.join("Library/Fonts")));
    v
}

/// The cache key: one line per directory, with its mtime or `-`.
pub fn cache_key(stamps: &[(PathBuf, Option<u128>)]) -> Vec<String> {
    let stamp = |(p, t): &(PathBuf, Option<u128>)| {
        let t = t.map_or_else(|| "-".to_string(), |t| t.to_string());
        format!("stamp\t{}\t{t}", p.display())
    };
    stamps.iter().map(stamp).collect()
}

pub fn format_cache(key: &[String], fallback: &[PathBuf]) -> String {
    let mut s = format!("{CACHE_HEADER}\n");
    for k in key {
        s.push_str(&format!("key\t{k}\n"));
    }
    for p in fallback {
        s.push_str("fallback\t");
        s.push_str(&p.to_string_lossy());
        s.push('\n');
    }
    s
}

/// The cached fallback paths, or `None` when the file is not ours, its key
/// differs from `key`, it is empty, or it names a file that no longer exists.
/// Other lines (the renderer's codepoint lines) are skipped.
pub fn parse_cache(
    text: &str,
    key: &[String],
    exists: impl Fn(&Path) -> bool,
) -> Option<Vec<PathBuf>> {
    let mut lines = text.lines();
    if lines.next()? != CACHE_HEADER {
        return None;
    }
    let (mut got_key, mut paths) = (Vec::new(), Vec::new());
    for l in lines {
        if let Some(k) = l.strip_prefix("key\t") {
            got_key.push(k.to_string());
        } else if let Some(p) = l.strip_prefix("fallback\t") {
            paths.push(PathBuf::from(p));
        }
    }
    let ok = got_key == key && !paths.is_empty() && paths.iter().all(|p| exists(p));
    ok.then_some(paths)
}

#[cfg(target_os = "macos")]
pub use mac::{MacFonts, cache_file};

#[cfg(target_os = "macos")]
mod mac {
    use std::path::PathBuf;

    use std::time::UNIX_EPOCH;

    use editor_render::fonts::{FontResolver, is_last_resort};
    use objc2_core_foundation::{CFRange, CFString, CFURL};
    use objc2_core_text::{CTFont, CTFontDescriptor, CTFontUIFontType, kCTFontURLAttribute};

    use super::*;

    pub struct MacFonts {
        fallback: Vec<PathBuf>,
    }

    /// The file behind a font name, through CoreText.
    fn file_for(name: &str) -> Option<PathBuf> {
        let name = CFString::from_str(name);
        // SAFETY: `name` is a valid CFString; a size of 0 means the default.
        let desc = unsafe { CTFontDescriptor::with_name_and_size(&name, 0.0) };
        // SAFETY: kCTFontURLAttribute is a valid descriptor attribute key.
        let url = unsafe { desc.attribute(kCTFontURLAttribute) }?;
        url.downcast::<CFURL>().ok()?.to_file_path()
    }

    /// The file of the face Core Text picks for `c`, through the system UI
    /// font's cascade list. Core Text answers a codepoint nothing covers
    /// with LastResort, whose cmap maps everything to one placeholder: that
    /// answer is refused, as fontconfig's charset check refuses a match that
    /// lacks the codepoint.
    fn file_covering(c: char) -> Option<PathBuf> {
        let s = CFString::from_str(c.encode_utf8(&mut [0; 4]));
        let range = CFRange {
            location: 0,
            length: c.len_utf16() as isize,
        };
        // SAFETY: a valid UI font type; size 0 means the default size.
        let base =
            unsafe { CTFont::new_ui_font_for_language(CTFontUIFontType::System, 0.0, None) }?;
        // SAFETY: `range` covers exactly the string's UTF-16 units.
        let font = unsafe { base.for_string(&s, range) };
        // SAFETY: a valid font; the name is a new CFString.
        if unsafe { font.post_script_name() }.to_string() == LAST_RESORT {
            return None;
        }
        // SAFETY: kCTFontURLAttribute is a valid font attribute key.
        let url = unsafe { font.attribute(kCTFontURLAttribute) }?;
        let path = url.downcast::<CFURL>().ok()?.to_file_path()?;
        (!is_last_resort(&path)).then_some(path)
    }

    /// The font cache file under this process's environment.
    pub fn cache_file() -> Option<PathBuf> {
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        cache_path(env("XDG_CACHE_HOME"), env("HOME"))
    }

    /// The cache key as things stand now.
    fn current_key() -> Vec<String> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let stamps: Vec<(PathBuf, Option<u128>)> = font_dirs(home.as_deref())
            .into_iter()
            .map(|d| {
                let t = std::fs::metadata(&d).and_then(|m| m.modified()).ok();
                let t = t.and_then(|t| t.duration_since(UNIX_EPOCH).ok());
                (d, t.map(|t| t.as_nanos()))
            })
            .collect();
        cache_key(&stamps)
    }

    impl MacFonts {
        /// Reads the cache, or resolves the fallback set and writes it.
        pub fn load() -> MacFonts {
            let cache = cache_file();
            let key = current_key();
            let cached = cache
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| parse_cache(&t, &key, |p| p.is_file()));
            if let Some(fallback) = cached {
                return MacFonts { fallback };
            }
            let fallback: Vec<PathBuf> =
                FALLBACK_NAMES.iter().filter_map(|n| file_for(n)).collect();
            if let Some(p) = cache.filter(|_| !fallback.is_empty()) {
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&p, format_cache(&key, &fallback));
            }
            MacFonts { fallback }
        }
    }

    impl FontResolver for MacFonts {
        fn sans(&self) -> PathBuf {
            PathBuf::from(SANS)
        }
        fn mono(&self) -> PathBuf {
            PathBuf::from(MONO)
        }
        fn family(&self, name: &str) -> Option<PathBuf> {
            file_for(name)
        }
        /// The italics first, then the fallback faces.
        fn fallback_set(&self) -> Vec<PathBuf> {
            let mut v = vec![PathBuf::from(SANS_ITALIC), PathBuf::from(MONO_ITALIC)];
            v.extend(self.fallback.iter().cloned());
            v
        }
        fn lookup(&self, c: char) -> Option<PathBuf> {
            file_covering(c)
        }
        fn warm_up(&self) {
            let _ = file_covering(WARM_UP);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<String> {
        cache_key(&[
            (PathBuf::from("/System/Library/Fonts"), Some(17)),
            (PathBuf::from("/h/Library/Fonts"), None),
        ])
    }

    #[test]
    fn cache_round_trips() {
        let paths = vec![PathBuf::from("/a/b.ttf"), PathBuf::from("/c d/e.ttc")];
        let text = format_cache(&key(), &paths);
        assert_eq!(parse_cache(&text, &key(), |_| true), Some(paths.clone()));
        // The renderer appends codepoint lines; they don't spoil the cache.
        let more = format!("{text}glyph\t2610\t/f/Symbols.ttf\n");
        assert_eq!(parse_cache(&more, &key(), |_| true), Some(paths));
    }

    #[test]
    fn stale_or_foreign_cache_is_ignored() {
        let text = format_cache(&key(), &[PathBuf::from("/gone.ttf")]);
        assert_eq!(parse_cache(&text, &key(), |_| false), None);
        assert_eq!(
            parse_cache("something else\nfallback\t/a\n", &key(), |_| true),
            None
        );
        assert_eq!(
            parse_cache(&format_cache(&key(), &[]), &key(), |_| true),
            None
        );
        assert_eq!(parse_cache("", &key(), |_| true), None);
    }

    #[test]
    fn a_changed_font_directory_expires_the_cache() {
        let paths = vec![PathBuf::from("/a/b.ttf")];
        let text = format_cache(&key(), &paths);
        let newer = cache_key(&[
            (PathBuf::from("/System/Library/Fonts"), Some(18)),
            (PathBuf::from("/h/Library/Fonts"), None),
        ]);
        assert_eq!(parse_cache(&text, &newer, |_| true), None);
        // A file from before the key existed is stale too.
        let old = "font-paths v1 macos\nfallback\t/a/b.ttf\n";
        assert_eq!(parse_cache(old, &key(), |_| true), None);
    }

    #[test]
    fn the_key_covers_system_local_and_user_fonts() {
        let d = font_dirs(Some(Path::new("/h")));
        for want in [
            "/System/Library/Fonts",
            "/Library/Fonts",
            "/h/Library/Fonts",
        ] {
            assert!(d.contains(&PathBuf::from(want)), "{want}");
        }
        assert_eq!(font_dirs(None).len(), 2);
        assert!(key()[1].ends_with("\t-"));
    }

    #[test]
    fn cache_lives_under_xdg_or_home() {
        let x = cache_path(Some(PathBuf::from("/x")), Some(PathBuf::from("/h"))).unwrap();
        assert_eq!(x, Path::new("/x").join(app_meta::KEBAB).join("font-paths"));
        let h = cache_path(None, Some(PathBuf::from("/h"))).unwrap();
        assert_eq!(
            h,
            Path::new("/h/.cache")
                .join(app_meta::KEBAB)
                .join("font-paths")
        );
        assert_eq!(cache_path(None, None), None);
    }
}

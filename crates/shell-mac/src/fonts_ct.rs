//! Font files on macOS. The body and monospace faces are fixed system
//! paths. The fallback set (symbols, emoji and a monospace backstop) is
//! found through CoreText once, then cached in the same file the Linux
//! shell uses (`<cache>/<kebab>/font-paths`, which `bench --first-run`
//! deletes), so later starts never ask CoreText.
//!
//! A codepoint none of those faces covers is looked up lazily, on the
//! fallback worker: `CTFontCreateForString` from the system UI font, then
//! that font's `kCTFontURLAttribute`. Core Text is warmed with one throwaway
//! lookup once the first frame is out.

use std::path::{Path, PathBuf};

/// The codepoint of the throwaway warm-up lookup: one no start-up face
/// covers, so Core Text walks its cascade list.
pub const WARM_UP: char = '\u{4E00}';

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

pub fn format_cache(fallback: &[PathBuf]) -> String {
    let mut s = format!("{CACHE_HEADER}\n");
    for p in fallback {
        s.push_str("fallback\t");
        s.push_str(&p.to_string_lossy());
        s.push('\n');
    }
    s
}

/// The cached fallback paths, or `None` when the file is not ours, is empty,
/// or names a file that no longer exists.
pub fn parse_cache(text: &str, exists: impl Fn(&Path) -> bool) -> Option<Vec<PathBuf>> {
    let mut lines = text.lines();
    if lines.next()? != CACHE_HEADER {
        return None;
    }
    let paths: Vec<PathBuf> = lines
        .filter_map(|l| l.strip_prefix("fallback\t"))
        .map(PathBuf::from)
        .collect();
    (!paths.is_empty() && paths.iter().all(|p| exists(p))).then_some(paths)
}

#[cfg(target_os = "macos")]
pub use mac::{MacFonts, cache_file};

#[cfg(target_os = "macos")]
mod mac {
    use std::path::PathBuf;

    use editor_render::fonts::FontResolver;
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
    /// font's cascade list.
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
        // SAFETY: kCTFontURLAttribute is a valid font attribute key.
        let url = unsafe { font.attribute(kCTFontURLAttribute) }?;
        url.downcast::<CFURL>().ok()?.to_file_path()
    }

    /// The font cache file under this process's environment.
    pub fn cache_file() -> Option<PathBuf> {
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        cache_path(env("XDG_CACHE_HOME"), env("HOME"))
    }

    impl MacFonts {
        /// Reads the cache, or resolves the fallback set and writes it.
        pub fn load() -> MacFonts {
            let cache = cache_file();
            let cached = cache
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| parse_cache(&t, |p| p.is_file()));
            if let Some(fallback) = cached {
                return MacFonts { fallback };
            }
            let fallback: Vec<PathBuf> =
                FALLBACK_NAMES.iter().filter_map(|n| file_for(n)).collect();
            if let Some(p) = cache.filter(|_| !fallback.is_empty()) {
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&p, format_cache(&fallback));
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

    #[test]
    fn cache_round_trips() {
        let paths = vec![PathBuf::from("/a/b.ttf"), PathBuf::from("/c d/e.ttc")];
        let text = format_cache(&paths);
        assert_eq!(parse_cache(&text, |_| true), Some(paths.clone()));
        // The renderer appends codepoint lines; they don't spoil the cache.
        let more = format!("{text}glyph\t2610\t/f/Symbols.ttf\n");
        assert_eq!(parse_cache(&more, |_| true), Some(paths));
    }

    #[test]
    fn stale_or_foreign_cache_is_ignored() {
        let text = format_cache(&[PathBuf::from("/gone.ttf")]);
        assert_eq!(parse_cache(&text, |_| false), None);
        assert_eq!(
            parse_cache("something else\nfallback\t/a\n", |_| true),
            None
        );
        assert_eq!(parse_cache(&format_cache(&[]), |_| true), None);
        assert_eq!(parse_cache("", |_| true), None);
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

//! Font files on Linux, through fontconfig, cached between runs.
//!
//! The start-up set is fontconfig's match for the body and monospace
//! families (the tokens' sentinels map to fontconfig's generic names), their
//! bold and italic variants, emoji and symbol. It is written to
//! `$XDG_CACHE_HOME/<kebab>/font-paths` (the file `bench --first-run`
//! deletes) with a key made of the mtimes of fontconfig's cache directories
//! and config files plus the configured families. When the key still
//! matches and every file exists, fontconfig is never called.
//!
//! Only `FcFontMatch` is used: nothing ever lists every system font. A
//! codepoint the start-up set lacks is looked up lazily, on the fallback
//! worker, by matching a pattern holding just that codepoint's charset.

use std::path::{Path, PathBuf};

use editor_core::tokens::family as fam;
use editor_render::fonts::FontPaths;

/// The first line of the cache file. A file without it is ignored (the
/// macOS shell writes a different header to the same path).
const CACHE_HEADER: &str = "font-paths v1 linux";

/// `$XDG_CACHE_HOME/<kebab>/font-paths`, or `~/.cache/...` without it.
pub fn cache_path(xdg_cache: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg_cache
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| home.map(|h| h.join(".cache")))?;
    Some(base.join(app_meta::KEBAB).join("font-paths"))
}

/// Files and directories whose mtimes key the cache: fontconfig's system
/// and user cache directories and its system and user configuration.
pub fn stamp_paths(
    home: Option<&Path>,
    xdg_cache: Option<&Path>,
    xdg_config: Option<&Path>,
) -> Vec<PathBuf> {
    let mut v = vec![
        PathBuf::from("/etc/fonts/fonts.conf"),
        PathBuf::from("/etc/fonts/conf.d"),
        PathBuf::from("/var/cache/fontconfig"),
    ];
    let cache = xdg_cache
        .map(Path::to_path_buf)
        .or_else(|| home.map(|h| h.join(".cache")));
    let config = xdg_config
        .map(Path::to_path_buf)
        .or_else(|| home.map(|h| h.join(".config")));
    if let Some(c) = cache {
        v.push(c.join("fontconfig"));
    }
    if let Some(c) = config {
        v.push(c.join("fontconfig/fonts.conf"));
        v.push(c.join("fontconfig/conf.d"));
    }
    if let Some(h) = home {
        v.push(h.join(".fonts.conf"));
    }
    v
}

/// The cache key: one line per stamp (`None` for a missing path) and one
/// per configured family.
pub fn cache_key(stamps: &[(PathBuf, Option<u128>)], families: &[&str]) -> Vec<String> {
    let stamp = |(p, t): &(PathBuf, Option<u128>)| {
        let t = t.map_or_else(|| "-".to_string(), |t| t.to_string());
        format!("stamp\t{}\t{t}", p.display())
    };
    let names = families.iter().map(|f| format!("name\t{f}"));
    stamps.iter().map(stamp).chain(names).collect()
}

pub fn format_cache(key: &[String], p: &FontPaths) -> String {
    let mut s = format!("{CACHE_HEADER}\n");
    for k in key {
        s.push_str(&format!("key\t{k}\n"));
    }
    let mut file = |tag: &str, path: &Path| s.push_str(&format!("{tag}\t{}\n", path.display()));
    file("sans", &p.sans);
    file("mono", &p.mono);
    for f in &p.fallback {
        file("fallback", f);
    }
    s
}

/// The cached files, or `None` when the file is not ours, its key differs
/// from `key`, or it names a file that no longer exists.
pub fn parse_cache(
    text: &str,
    key: &[String],
    exists: impl Fn(&Path) -> bool,
) -> Option<FontPaths> {
    let mut lines = text.lines();
    if lines.next()? != CACHE_HEADER {
        return None;
    }
    let (mut got_key, mut sans, mut mono, mut fallback) = (Vec::new(), None, None, Vec::new());
    for l in lines {
        let (tag, rest) = l.split_once('\t')?;
        match tag {
            "key" => got_key.push(rest.to_string()),
            "sans" => sans = Some(PathBuf::from(rest)),
            "mono" => mono = Some(PathBuf::from(rest)),
            "fallback" => fallback.push(PathBuf::from(rest)),
            // Codepoints the renderer's lazy fallback found (its own lines).
            "glyph" => {}
            _ => return None,
        }
    }
    let p = FontPaths {
        sans: sans?,
        mono: mono?,
        fallback,
    };
    let all_exist = [&p.sans, &p.mono]
        .into_iter()
        .chain(&p.fallback)
        .all(|f| exists(f));
    (got_key == key && all_exist).then_some(p)
}

/// The name to ask fontconfig for: a token sentinel becomes the generic
/// name it stands for; anything else is a real family.
pub fn query_name(token: &str) -> &str {
    match token {
        fam::SYSTEM_UI => fam::GENERIC_SANS,
        fam::SYSTEM_MONO => fam::GENERIC_MONO,
        other => other,
    }
}

/// One fontconfig match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Query<'a> {
    pub name: &'a str,
    pub bold: bool,
    pub italic: bool,
}

/// The fallback set's matches, in load order: bold and italic body and
/// monospace, then emoji and symbol.
pub fn fallback_queries<'a>(body: &'a str, mono: &'a str) -> Vec<Query<'a>> {
    let q = |name, bold, italic| Query { name, bold, italic };
    vec![
        q(body, true, false),
        q(body, false, true),
        q(mono, true, false),
        q(mono, false, true),
        q(fam::GENERIC_EMOJI, false, false),
        q(fam::GENERIC_SYMBOL, false, false),
    ]
}

/// The matches that found a file, in order, without repeats and without the
/// body and monospace files themselves.
pub fn fallback_files(
    sans: &Path,
    mono: &Path,
    found: impl IntoIterator<Item = Option<PathBuf>>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for f in found.into_iter().flatten() {
        if f != sans && f != mono && !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

#[cfg(target_os = "linux")]
pub use linux::{LinuxFonts, cache_file};

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::{CStr, CString, OsStr, c_char};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::time::UNIX_EPOCH;

    use editor_render::fonts::{FontPaths, FontResolver};
    use fontconfig_sys as fc;
    use fontconfig_sys::constants::{
        FC_CHARSET, FC_FAMILY, FC_FILE, FC_SLANT, FC_SLANT_ITALIC, FC_WEIGHT, FC_WEIGHT_BOLD,
    };

    use super::*;

    pub struct LinuxFonts {
        paths: FontPaths,
    }

    fn env_path(k: &str) -> Option<PathBuf> {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    fn mtime(p: &Path) -> Option<u128> {
        let t = std::fs::metadata(p).ok()?.modified().ok()?;
        Some(t.duration_since(UNIX_EPOCH).ok()?.as_nanos())
    }

    /// The cache key as things stand now.
    fn current_key(names: &[&str]) -> Vec<String> {
        let home = env_path("HOME");
        let stamps: Vec<(PathBuf, Option<u128>)> = stamp_paths(
            home.as_deref(),
            env_path("XDG_CACHE_HOME").as_deref(),
            env_path("XDG_CONFIG_HOME").as_deref(),
        )
        .into_iter()
        .map(|p| {
            let t = mtime(&p);
            (p, t)
        })
        .collect();
        cache_key(&stamps, names)
    }

    /// fontconfig's best file for `q`, through the default configuration.
    fn fc_match(q: Query) -> Option<PathBuf> {
        let name = CString::new(q.name).ok()?;
        // SAFETY: `FcInit` may be called repeatedly. The pattern is created
        // here and `match_file` consumes it.
        unsafe {
            if fc::FcInit() == 0 {
                return None;
            }
            let pat = fc::FcPatternCreate();
            if pat.is_null() {
                return None;
            }
            fc::FcPatternAddString(pat, FC_FAMILY.as_ptr(), name.as_ptr().cast());
            if q.bold {
                fc::FcPatternAddInteger(pat, FC_WEIGHT.as_ptr(), FC_WEIGHT_BOLD);
            }
            if q.italic {
                fc::FcPatternAddInteger(pat, FC_SLANT.as_ptr(), FC_SLANT_ITALIC);
            }
            match_file(pat, None)
        }
    }

    /// The file of fontconfig's best match for a pattern holding only `c`'s
    /// charset, when that match really covers `c`.
    fn fc_covering(c: char) -> Option<PathBuf> {
        // SAFETY: as in `fc_match`. The pattern takes its own reference to
        // the charset, so ours is released straight after adding it.
        unsafe {
            if fc::FcInit() == 0 {
                return None;
            }
            let cs = fc::FcCharSetCreate();
            if cs.is_null() {
                return None;
            }
            fc::FcCharSetAddChar(cs, u32::from(c));
            let pat = fc::FcPatternCreate();
            if !pat.is_null() {
                fc::FcPatternAddCharSet(pat, FC_CHARSET.as_ptr(), cs);
            }
            fc::FcCharSetDestroy(cs);
            if pat.is_null() {
                return None;
            }
            match_file(pat, Some(c))
        }
    }

    /// Fills in `pat`'s defaults, destroys it and returns the file of its
    /// best match; with `need`, only when the match's charset holds it.
    ///
    /// # Safety
    /// `pat` is a valid pattern the caller owns and does not use again.
    unsafe fn match_file(pat: *mut fc::FcPattern, need: Option<char>) -> Option<PathBuf> {
        // SAFETY: a null config means the current one. The file name is
        // copied out of the match before the match is destroyed.
        unsafe {
            fc::FcConfigSubstitute(std::ptr::null_mut(), pat, fc::FcMatchPattern);
            fc::FcDefaultSubstitute(pat);
            let mut res: fc::FcResult = fc::FcResultMatch;
            let found = fc::FcFontMatch(std::ptr::null_mut(), pat, &mut res);
            fc::FcPatternDestroy(pat);
            if found.is_null() {
                return None;
            }
            let covers = need.is_none_or(|c| {
                let mut cs: *mut fc::FcCharSet = std::ptr::null_mut();
                let got = fc::FcPatternGetCharSet(found, FC_CHARSET.as_ptr(), 0, &mut cs);
                got == fc::FcResultMatch
                    && !cs.is_null()
                    && fc::FcCharSetHasChar(cs, u32::from(c)) != 0
            });
            let mut file: *mut fc::FcChar8 = std::ptr::null_mut();
            let got = fc::FcPatternGetString(found, FC_FILE.as_ptr(), 0, &mut file);
            let path = (covers && got == fc::FcResultMatch && !file.is_null()).then(|| {
                let bytes = CStr::from_ptr(file.cast_const().cast::<c_char>()).to_bytes();
                PathBuf::from(OsStr::from_bytes(bytes))
            });
            fc::FcPatternDestroy(found);
            path
        }
    }

    /// The font cache file under this process's environment.
    pub fn cache_file() -> Option<PathBuf> {
        cache_path(env_path("XDG_CACHE_HOME"), env_path("HOME"))
    }

    fn plain(name: &str) -> Query<'_> {
        Query {
            name,
            bold: false,
            italic: false,
        }
    }

    impl LinuxFonts {
        /// Reads the cache, or asks fontconfig and writes it. `body` and
        /// `mono` are the `font.body` and `font.mono` token values.
        pub fn load(body: &str, mono: &str) -> LinuxFonts {
            let names = [body, mono];
            let cache = cache_file();
            let key = current_key(&names);
            let cached = cache
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| parse_cache(&t, &key, Path::is_file));
            if let Some(paths) = cached {
                return LinuxFonts { paths };
            }
            let (b, m) = (query_name(body), query_name(mono));
            let sans = fc_match(plain(b)).unwrap_or_default();
            let mono_file = fc_match(plain(m)).unwrap_or_default();
            let found = fallback_queries(b, m).into_iter().map(fc_match);
            let fallback = fallback_files(&sans, &mono_file, found);
            let paths = FontPaths {
                sans,
                mono: mono_file,
                fallback,
            };
            let usable = !paths.sans.as_os_str().is_empty() && !paths.mono.as_os_str().is_empty();
            if let Some(p) = cache.filter(|_| usable) {
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                // Keyed after matching: a first `FcInit` may have just
                // written fontconfig's own cache.
                let _ = std::fs::write(&p, format_cache(&current_key(&names), &paths));
            }
            LinuxFonts { paths }
        }
    }

    impl FontResolver for LinuxFonts {
        fn sans(&self) -> PathBuf {
            self.paths.sans.clone()
        }
        fn mono(&self) -> PathBuf {
            self.paths.mono.clone()
        }
        fn family(&self, name: &str) -> Option<PathBuf> {
            fc_match(plain(query_name(name)))
        }
        fn fallback_set(&self) -> Vec<PathBuf> {
            self.paths.fallback.clone()
        }
        fn lookup(&self, c: char) -> Option<PathBuf> {
            fc_covering(c)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> FontPaths {
        FontPaths {
            sans: PathBuf::from("/f/Sans.ttf"),
            mono: PathBuf::from("/f/Mono.ttf"),
            fallback: vec![
                PathBuf::from("/f/Sans-Bold.ttf"),
                PathBuf::from("/f d/Emoji.ttf"),
            ],
        }
    }

    fn key() -> Vec<String> {
        cache_key(
            &[
                (PathBuf::from("/etc/fonts/fonts.conf"), Some(17)),
                (PathBuf::from("/var/cache/fontconfig"), None),
            ],
            &["system-ui", "Fira Code"],
        )
    }

    #[test]
    fn cache_round_trips() {
        let text = format_cache(&key(), &paths());
        assert!(text.starts_with("font-paths v1 linux\n"));
        assert_eq!(parse_cache(&text, &key(), |_| true), Some(paths()));
        // The renderer appends codepoint lines; they don't spoil the cache.
        let more = format!("{text}glyph\t2610\t/f/Symbols.ttf\n");
        assert_eq!(parse_cache(&more, &key(), |_| true), Some(paths()));
    }

    #[test]
    fn changed_key_or_missing_file_misses() {
        let text = format_cache(&key(), &paths());
        let mut other = key();
        other[0] = cache_key(&[(PathBuf::from("/etc/fonts/fonts.conf"), Some(18))], &[])[0].clone();
        assert_eq!(parse_cache(&text, &other, |_| true), None);
        let fewer = cache_key(&[], &["system-ui"]);
        assert_eq!(parse_cache(&text, &fewer, |_| true), None);
        assert_eq!(
            parse_cache(&text, &key(), |p| !p.ends_with("Emoji.ttf")),
            None
        );
        assert_eq!(parse_cache("font-paths v1 macos\n", &key(), |_| true), None);
        assert_eq!(parse_cache("", &key(), |_| true), None);
        // No body face recorded: not a usable cache.
        let headless = "font-paths v1 linux\n";
        assert_eq!(parse_cache(headless, &[], |_| true), None);
    }

    #[test]
    fn key_lines_differ_by_stamp_and_family() {
        let a = cache_key(&[(PathBuf::from("/x"), Some(1))], &["a"]);
        let b = cache_key(&[(PathBuf::from("/x"), None)], &["a"]);
        let c = cache_key(&[(PathBuf::from("/x"), Some(1))], &["b"]);
        assert_eq!(a.len(), 2);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn cache_lives_under_xdg_or_home() {
        let x = cache_path(Some(PathBuf::from("/x")), Some(PathBuf::from("/h"))).unwrap();
        assert_eq!(x, Path::new("/x").join(app_meta::KEBAB).join("font-paths"));
        let h = cache_path(Some(PathBuf::new()), Some(PathBuf::from("/h"))).unwrap();
        assert_eq!(
            h,
            Path::new("/h/.cache")
                .join(app_meta::KEBAB)
                .join("font-paths")
        );
        assert_eq!(cache_path(None, None), None);
    }

    #[test]
    fn stamps_cover_system_and_user_fontconfig() {
        let s = stamp_paths(Some(Path::new("/h")), None, None);
        for want in [
            "/etc/fonts/fonts.conf",
            "/etc/fonts/conf.d",
            "/var/cache/fontconfig",
            "/h/.cache/fontconfig",
            "/h/.config/fontconfig/fonts.conf",
            "/h/.config/fontconfig/conf.d",
            "/h/.fonts.conf",
        ] {
            assert!(s.contains(&PathBuf::from(want)), "{want}");
        }
        let x = stamp_paths(
            Some(Path::new("/h")),
            Some(Path::new("/c")),
            Some(Path::new("/g")),
        );
        assert!(x.contains(&PathBuf::from("/c/fontconfig")));
        assert!(x.contains(&PathBuf::from("/g/fontconfig/fonts.conf")));
        assert!(!x.contains(&PathBuf::from("/h/.cache/fontconfig")));
    }

    #[test]
    fn sentinels_map_to_generic_names() {
        assert_eq!(query_name("system-ui"), "sans-serif");
        assert_eq!(query_name("system-mono"), "monospace");
        assert_eq!(query_name("Fira Code"), "Fira Code");
    }

    #[test]
    fn fallback_is_bold_and_italic_then_emoji_and_symbol() {
        let q = fallback_queries("sans-serif", "monospace");
        let q: Vec<(&str, bool, bool)> = q.iter().map(|q| (q.name, q.bold, q.italic)).collect();
        assert_eq!(
            q,
            vec![
                ("sans-serif", true, false),
                ("sans-serif", false, true),
                ("monospace", true, false),
                ("monospace", false, true),
                ("emoji", false, false),
                ("symbol", false, false),
            ]
        );
    }

    #[test]
    fn fallback_files_drop_repeats_and_the_main_faces() {
        let p = |s: &str| Some(PathBuf::from(s));
        let got = fallback_files(
            Path::new("/S.ttf"),
            Path::new("/M.ttf"),
            [
                p("/SB.ttf"),
                p("/S.ttf"),
                None,
                p("/SB.ttf"),
                p("/M.ttf"),
                p("/E.ttf"),
            ],
        );
        assert_eq!(got, vec![PathBuf::from("/SB.ttf"), PathBuf::from("/E.ttf")]);
    }
}

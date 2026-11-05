//! Font files on macOS. The `font.body` and `font.mono` sentinels
//! (`system-ui`, `system-mono`) name fixed system paths, the San Francisco
//! faces, with no Core Text call. Any other name is a family, resolved
//! through Core Text by family name (or, failing that, as a PostScript
//! name) to its regular, italic and bold faces. A name Core Text doesn't
//! know quietly becomes that slot's system face, as fontconfig's nearest
//! match does on Linux.
//!
//! The start-up set (body and monospace faces, their styles, and the
//! fallback faces: symbols, emoji and a monospace backstop) is resolved
//! once, then cached in the same file the Linux shell uses
//! (`<cache>/<kebab>/font-paths`, which `bench --first-run` deletes), so
//! later starts never ask Core Text. The file is keyed on the mtimes of the
//! font directories and on the configured families, so a font install, an
//! OS update or a changed `font.body` or `font.mono` expires it, and the
//! renderer's codepoint lines with it.
//!
//! A codepoint none of those faces covers is looked up lazily, on the
//! fallback worker: `CTFontCreateForString` from the system UI font, then
//! that font's `kCTFontURLAttribute`. Core Text is warmed with one throwaway
//! lookup once the first frame is out.

use std::path::{Path, PathBuf};

use editor_core::tokens::family as fam;
use editor_render::fonts::FontPaths;

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

/// The first line of the cache file. A file without it is ignored. v2 added
/// the configured families to the key and the body and monospace files.
const CACHE_HEADER: &str = "font-paths v2 macos";

/// One family's files: the regular face and, where the family has them,
/// its italic and bold faces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Faces {
    pub regular: PathBuf,
    pub italic: Option<PathBuf>,
    pub bold: Option<PathBuf>,
}

/// The San Francisco faces a sentinel stands for, or `None` for a real
/// family name. The variable SF faces carry bold themselves.
pub fn system_faces(token: &str) -> Option<Faces> {
    let (regular, italic) = match token {
        fam::SYSTEM_UI => (SANS, SANS_ITALIC),
        fam::SYSTEM_MONO => (MONO, MONO_ITALIC),
        _ => return None,
    };
    Some(Faces {
        regular: PathBuf::from(regular),
        italic: Some(PathBuf::from(italic)),
        bold: None,
    })
}

/// The faces for one slot's token: a sentinel's system faces without
/// calling `resolve`, else what `resolve` finds for the name, else the
/// system faces of `slot` (the slot's own sentinel).
pub fn faces_for(token: &str, slot: &str, resolve: impl FnOnce(&str) -> Option<Faces>) -> Faces {
    system_faces(token)
        .or_else(|| resolve(token))
        .or_else(|| system_faces(slot))
        .unwrap_or_else(|| Faces {
            regular: PathBuf::from(SANS),
            italic: None,
            bold: None,
        })
}

/// The start-up files: the two regular faces, then the italics, the bolds
/// and the fallback faces, in that order, without repeats and without the
/// regular faces themselves (a collection holds every style in one file).
pub fn start_up_paths(body: &Faces, mono: &Faces, found: &[PathBuf]) -> FontPaths {
    let styles = [&body.italic, &mono.italic, &body.bold, &mono.bold];
    let all = styles.into_iter().flatten().chain(found);
    let mut fallback: Vec<PathBuf> = Vec::new();
    for f in all {
        if *f != body.regular && *f != mono.regular && !fallback.contains(f) {
            fallback.push(f.clone());
        }
    }
    FontPaths {
        sans: body.regular.clone(),
        mono: mono.regular.clone(),
        fallback,
    }
}

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

/// The cache key: one line per directory, with its mtime or `-`, then one
/// per configured family (body, then monospace).
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
/// from `key`, it has no body, monospace or fallback faces, or it names a
/// file that no longer exists. Other lines (the renderer's codepoint lines)
/// are skipped.
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
        if let Some(k) = l.strip_prefix("key\t") {
            got_key.push(k.to_string());
        } else if let Some(p) = l.strip_prefix("sans\t") {
            sans = Some(PathBuf::from(p));
        } else if let Some(p) = l.strip_prefix("mono\t") {
            mono = Some(PathBuf::from(p));
        } else if let Some(p) = l.strip_prefix("fallback\t") {
            fallback.push(PathBuf::from(p));
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
    (got_key == key && !p.fallback.is_empty() && all_exist).then_some(p)
}

#[cfg(target_os = "macos")]
pub use mac::{MacFonts, cache_file, resolve_family};

#[cfg(target_os = "macos")]
mod mac {
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    use editor_render::fonts::{FontResolver, is_last_resort};
    use objc2_core_foundation::{CFDictionary, CFRange, CFRetained, CFString, CFType, CFURL};
    use objc2_core_text::{
        CTFont, CTFontDescriptor, CTFontSymbolicTraits, CTFontUIFontType,
        kCTFontFamilyNameAttribute, kCTFontURLAttribute,
    };

    use super::*;

    pub struct MacFonts {
        paths: FontPaths,
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

    /// The file a font was loaded from, refusing LastResort.
    fn font_file(font: &CTFont) -> Option<PathBuf> {
        // SAFETY: kCTFontURLAttribute is a valid font attribute key.
        let url = unsafe { font.attribute(kCTFontURLAttribute) }?;
        let path = url.downcast::<CFURL>().ok()?.to_file_path()?;
        (!is_last_resort(&path)).then_some(path)
    }

    /// Core Text's best font for `desc`. It always answers, with a stand-in
    /// when nothing matches, so callers check the name of what came back.
    fn font_from(desc: &CTFontDescriptor) -> CFRetained<CTFont> {
        // SAFETY: a valid descriptor; size 0 means the default; no matrix.
        unsafe { CTFont::with_font_descriptor(desc, 0.0, std::ptr::null()) }
    }

    /// The font of the family called `name` (any case), or `None`.
    fn by_family(name: &str) -> Option<CFRetained<CTFont>> {
        let value = CFString::from_str(name);
        // SAFETY: the key is a valid descriptor attribute key.
        let key: &CFString = unsafe { kCTFontFamilyNameAttribute };
        let attrs = CFDictionary::<CFString, CFType>::from_slices(&[key], &[value.as_ref()]);
        // SAFETY: the dictionary maps a CFString key to a CFString value.
        let desc = unsafe { CTFontDescriptor::with_attributes(attrs.as_opaque()) };
        let font = font_from(&desc);
        // SAFETY: a valid font; the name is a new CFString.
        let family = unsafe { font.family_name() }.to_string();
        family.eq_ignore_ascii_case(name).then_some(font)
    }

    /// The font whose PostScript name is `name` (any case), or `None`.
    fn by_post_script_name(name: &str) -> Option<CFRetained<CTFont>> {
        let cf = CFString::from_str(name);
        // SAFETY: `cf` is a valid CFString; a size of 0 means the default.
        let desc = unsafe { CTFontDescriptor::with_name_and_size(&cf, 0.0) };
        let font = font_from(&desc);
        // SAFETY: a valid font; the name is a new CFString.
        let ps = unsafe { font.post_script_name() }.to_string();
        ps.eq_ignore_ascii_case(name).then_some(font)
    }

    /// `font`'s face in the same family whose `mask` traits are exactly
    /// `traits` (those set, the rest of `mask` clear), when it has one.
    fn styled(
        font: &CTFont,
        traits: CTFontSymbolicTraits,
        mask: CTFontSymbolicTraits,
    ) -> Option<CFRetained<CTFont>> {
        // SAFETY: a valid font; size 0 keeps its size; no matrix.
        let f = unsafe { font.copy_with_symbolic_traits(0.0, std::ptr::null(), traits, mask) }?;
        // SAFETY: valid fonts; the names are new CFStrings.
        let same = unsafe { f.family_name() } == unsafe { font.family_name() };
        // SAFETY: a valid font.
        let got = unsafe { f.symbolic_traits() } & mask;
        (same && got == traits).then_some(f)
    }

    /// The faces of the family called `name`, through Core Text: by family
    /// name (its upright, normal-weight face), else by PostScript name (that
    /// exact face), with the family's italic and bold faces where it has
    /// them. `None` when Core Text knows no such font.
    pub fn resolve_family(name: &str) -> Option<Faces> {
        let (italic, bold) = (
            CTFontSymbolicTraits::ItalicTrait,
            CTFontSymbolicTraits::BoldTrait,
        );
        let plain = |f: CFRetained<CTFont>| {
            styled(&f, CTFontSymbolicTraits::empty(), italic | bold).unwrap_or(f)
        };
        let font = by_family(name)
            .map(plain)
            .or_else(|| by_post_script_name(name))?;
        let regular = font_file(&font)?;
        let file = |t| styled(&font, t, t).and_then(|f| font_file(&f));
        Some(Faces {
            regular,
            italic: file(italic),
            bold: file(bold),
        })
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
        font_file(&font)
    }

    /// The font cache file under this process's environment.
    pub fn cache_file() -> Option<PathBuf> {
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        cache_path(env("XDG_CACHE_HOME"), env("HOME"))
    }

    /// The cache key as things stand now.
    fn current_key(families: &[&str]) -> Vec<String> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let stamps: Vec<(PathBuf, Option<u128>)> = font_dirs(home.as_deref())
            .into_iter()
            .map(|d| {
                let t = std::fs::metadata(&d).and_then(|m| m.modified()).ok();
                let t = t.and_then(|t| t.duration_since(UNIX_EPOCH).ok());
                (d, t.map(|t| t.as_nanos()))
            })
            .collect();
        cache_key(&stamps, families)
    }

    impl MacFonts {
        /// Reads the cache, or resolves the start-up set and writes it.
        /// `body` and `mono` are the `font.body` and `font.mono` token
        /// values; a sentinel costs no Core Text call.
        pub fn load(body: &str, mono: &str) -> MacFonts {
            let names = [body, mono];
            let cache = cache_file();
            let key = current_key(&names);
            let cached = cache
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| parse_cache(&t, &key, |p| p.is_file()));
            if let Some(paths) = cached {
                return MacFonts { paths };
            }
            let b = faces_for(body, fam::SYSTEM_UI, resolve_family);
            let m = faces_for(mono, fam::SYSTEM_MONO, resolve_family);
            let found: Vec<PathBuf> = FALLBACK_NAMES.iter().filter_map(|n| file_for(n)).collect();
            let paths = start_up_paths(&b, &m, &found);
            if let Some(p) = cache.filter(|_| !found.is_empty()) {
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&p, format_cache(&key, &paths));
            }
            MacFonts { paths }
        }
    }

    impl FontResolver for MacFonts {
        fn sans(&self) -> PathBuf {
            self.paths.sans.clone()
        }
        fn mono(&self) -> PathBuf {
            self.paths.mono.clone()
        }
        fn family(&self, name: &str) -> Option<PathBuf> {
            let faces = system_faces(name).or_else(|| resolve_family(name))?;
            Some(faces.regular)
        }
        /// The styles first, then the fallback faces.
        fn fallback_set(&self) -> Vec<PathBuf> {
            self.paths.fallback.clone()
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
        cache_key(
            &[
                (PathBuf::from("/System/Library/Fonts"), Some(17)),
                (PathBuf::from("/h/Library/Fonts"), None),
            ],
            &["system-ui", "Iosevka"],
        )
    }

    fn paths() -> FontPaths {
        FontPaths {
            sans: PathBuf::from("/f/Georgia.ttf"),
            mono: PathBuf::from("/f d/Iosevka.ttc"),
            fallback: vec![PathBuf::from("/a/b.ttf"), PathBuf::from("/c d/e.ttc")],
        }
    }

    fn faces(regular: &str, italic: Option<&str>, bold: Option<&str>) -> Faces {
        Faces {
            regular: PathBuf::from(regular),
            italic: italic.map(PathBuf::from),
            bold: bold.map(PathBuf::from),
        }
    }

    #[test]
    fn cache_round_trips() {
        let text = format_cache(&key(), &paths());
        assert!(text.starts_with("font-paths v2 macos\n"));
        assert!(text.contains("key\tname\tIosevka\n"));
        assert_eq!(parse_cache(&text, &key(), |_| true), Some(paths()));
        // The renderer appends codepoint lines; they don't spoil the cache.
        let more = format!("{text}glyph\t2610\t/f/Symbols.ttf\n");
        assert_eq!(parse_cache(&more, &key(), |_| true), Some(paths()));
    }

    #[test]
    fn stale_or_foreign_cache_is_ignored() {
        let text = format_cache(&key(), &paths());
        assert_eq!(parse_cache(&text, &key(), |_| false), None);
        assert_eq!(
            parse_cache(&text, &key(), |p| !p.ends_with("Iosevka.ttc")),
            None
        );
        assert_eq!(
            parse_cache("something else\nfallback\t/a\n", &key(), |_| true),
            None
        );
        let bare = FontPaths {
            fallback: vec![],
            ..paths()
        };
        assert_eq!(
            parse_cache(&format_cache(&key(), &bare), &key(), |_| true),
            None
        );
        assert_eq!(parse_cache("", &key(), |_| true), None);
        // A v1 file (fallback faces only, no families in its key) misses.
        let v1 = "font-paths v1 macos\nfallback\t/a/b.ttf\n";
        assert_eq!(parse_cache(v1, &key(), |_| true), None);
        // No body face recorded: not a usable cache.
        let headless = "font-paths v2 macos\nfallback\t/a/b.ttf\n";
        assert_eq!(parse_cache(headless, &[], |_| true), None);
    }

    #[test]
    fn a_changed_font_directory_expires_the_cache() {
        let text = format_cache(&key(), &paths());
        let newer = cache_key(
            &[
                (PathBuf::from("/System/Library/Fonts"), Some(18)),
                (PathBuf::from("/h/Library/Fonts"), None),
            ],
            &["system-ui", "Iosevka"],
        );
        assert_eq!(parse_cache(&text, &newer, |_| true), None);
    }

    #[test]
    fn a_changed_family_expires_the_cache() {
        let text = format_cache(&key(), &paths());
        let stamps = [
            (PathBuf::from("/System/Library/Fonts"), Some(17)),
            (PathBuf::from("/h/Library/Fonts"), None),
        ];
        let other = cache_key(&stamps, &["system-ui", "Menlo"]);
        assert_eq!(parse_cache(&text, &other, |_| true), None);
        let swapped = cache_key(&stamps, &["Iosevka", "system-ui"]);
        assert_eq!(parse_cache(&text, &swapped, |_| true), None);
    }

    #[test]
    fn key_lines_are_stamps_then_names() {
        let k = key();
        assert_eq!(k.len(), 4);
        assert!(k[1].ends_with("\t-"));
        assert_eq!(k[2], "name\tsystem-ui");
        assert_eq!(k[3], "name\tIosevka");
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

    #[test]
    fn sentinels_name_the_system_faces() {
        assert_eq!(
            system_faces("system-ui"),
            Some(faces(SANS, Some(SANS_ITALIC), None))
        );
        assert_eq!(
            system_faces("system-mono"),
            Some(faces(MONO, Some(MONO_ITALIC), None))
        );
        assert_eq!(system_faces("Georgia"), None);
        assert_eq!(system_faces("System-UI"), None);
    }

    #[test]
    fn a_sentinel_never_asks_core_text() {
        let never = |_: &str| -> Option<Faces> { panic!("asked Core Text") };
        let b = faces_for("system-ui", fam::SYSTEM_UI, never);
        assert_eq!(b, system_faces("system-ui").unwrap());
        // A sentinel in the other slot still names its own face.
        let m = faces_for("system-ui", fam::SYSTEM_MONO, never);
        assert_eq!(m.regular, PathBuf::from(SANS));
    }

    #[test]
    fn a_family_resolves_or_falls_back_to_the_slots_system_face() {
        let georgia = faces("/g/Georgia.ttf", Some("/g/Georgia Italic.ttf"), None);
        let found = faces_for("Georgia", fam::SYSTEM_UI, |n| {
            assert_eq!(n, "Georgia");
            Some(georgia.clone())
        });
        assert_eq!(found, georgia);
        let unknown = faces_for("No Such Face", fam::SYSTEM_MONO, |_| None);
        assert_eq!(unknown, system_faces("system-mono").unwrap());
    }

    #[test]
    fn the_default_start_up_set_is_unchanged() {
        let ui = system_faces("system-ui").unwrap();
        let mono = system_faces("system-mono").unwrap();
        let found = vec![
            PathBuf::from("/s/Symbols.ttf"),
            PathBuf::from("/s/Menlo.ttc"),
        ];
        let p = start_up_paths(&ui, &mono, &found);
        assert_eq!(p.sans, PathBuf::from(SANS));
        assert_eq!(p.mono, PathBuf::from(MONO));
        let mut want = vec![PathBuf::from(SANS_ITALIC), PathBuf::from(MONO_ITALIC)];
        want.extend(found);
        assert_eq!(p.fallback, want);
    }

    #[test]
    fn a_custom_family_brings_its_own_styles() {
        let body = faces(
            "/g/Georgia.ttf",
            Some("/g/GeorgiaI.ttf"),
            Some("/g/GeorgiaB.ttf"),
        );
        // A collection holds every style: the repeats drop out.
        let mono = faces("/m/Menlo.ttc", Some("/m/Menlo.ttc"), Some("/m/Menlo.ttc"));
        let found = vec![
            PathBuf::from("/s/Symbols.ttf"),
            PathBuf::from("/m/Menlo.ttc"),
        ];
        let p = start_up_paths(&body, &mono, &found);
        assert_eq!(p.sans, PathBuf::from("/g/Georgia.ttf"));
        assert_eq!(p.mono, PathBuf::from("/m/Menlo.ttc"));
        let want: Vec<PathBuf> = ["/g/GeorgiaI.ttf", "/g/GeorgiaB.ttf", "/s/Symbols.ttf"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(p.fallback, want);
        assert!(!p.fallback.contains(&PathBuf::from(SANS_ITALIC)));
    }

    /// Core Text itself, with families every macOS install ships.
    #[cfg(target_os = "macos")]
    #[test]
    fn core_text_resolves_shipped_families() {
        let georgia = resolve_family("Georgia").expect("Georgia");
        assert!(georgia.regular.is_file(), "{}", georgia.regular.display());
        let italic = georgia.italic.expect("Georgia Italic");
        assert!(italic.is_file());
        assert_ne!(italic, georgia.regular);
        let menlo = resolve_family("menlo").expect("Menlo, any case");
        assert!(menlo.regular.is_file());
        // A PostScript name works too.
        let ps = resolve_family("Menlo-Regular").expect("Menlo-Regular");
        assert_eq!(ps.regular, menlo.regular);
        assert_eq!(resolve_family("No Such Face 46"), None);
    }
}

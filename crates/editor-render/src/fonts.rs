//! Fonts, loaded by path only. The renderer never scans the system: a shell's
//! `FontResolver` names the files, and `FontSet::load` reads exactly those.

use std::path::{Path, PathBuf};

use cosmic_text::FontSystem;
use cosmic_text::fontdb::{Database, Source};

/// Finds font files for the renderer. Implemented by the shells.
pub trait FontResolver: Send {
    /// The body face (the `system-ui` sentinel).
    fn sans(&self) -> PathBuf;
    /// The monospace face (the `system-mono` sentinel).
    fn mono(&self) -> PathBuf;
    /// A configured family, by name.
    fn family(&self, name: &str) -> Option<PathBuf>;
    /// The small fixed fallback set loaded at start-up.
    fn fallback_set(&self) -> Vec<PathBuf>;
    /// A face covering `c`, looked up off the start-up path (Task 42).
    fn lookup(&self, c: char) -> Option<PathBuf>;
}

/// The files a `FontSet` loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontPaths {
    pub sans: PathBuf,
    pub mono: PathBuf,
    pub fallback: Vec<PathBuf>,
}

impl FontPaths {
    /// The start-up files a resolver names: sans, mono and the fallback set.
    pub fn from_resolver(r: &dyn FontResolver) -> FontPaths {
        FontPaths {
            sans: r.sans(),
            mono: r.mono(),
            fallback: r.fallback_set(),
        }
    }
}

/// Loaded faces and the font system that shapes with them.
pub struct FontSet {
    pub(crate) system: FontSystem,
    pub(crate) sans: Option<String>,
    pub(crate) mono: Option<String>,
}

/// The locale handed to cosmic-text. It only steers fallback order among the
/// loaded faces, and the set loaded here is fixed.
const LOCALE: &str = "en-US";

impl FontSet {
    /// Loads only the given files, memory-mapped, into a fresh database. A
    /// file that can't be read loads nothing; text in that role then falls
    /// back to whatever faces did load.
    pub fn load(paths: &FontPaths) -> FontSet {
        let mut db = Database::new();
        let sans = load_family(&mut db, &paths.sans);
        let mono = load_family(&mut db, &paths.mono);
        for p in &paths.fallback {
            db.load_font_source(Source::File(p.clone()));
        }
        if let Some(s) = &sans {
            db.set_sans_serif_family(s.clone());
        }
        if let Some(m) = &mono {
            db.set_monospace_family(m.clone());
        }
        let system = FontSystem::new_with_locale_and_db(LOCALE.to_string(), db);
        FontSet { system, sans, mono }
    }

    /// The family name of the loaded body face.
    pub fn sans_family(&self) -> Option<&str> {
        self.sans.as_deref()
    }

    /// The family name of the loaded monospace face.
    pub fn mono_family(&self) -> Option<&str> {
        self.mono.as_deref()
    }

    /// How many faces are loaded.
    pub fn face_count(&self) -> usize {
        self.system.db().len()
    }
}

/// Loads the file and returns its first face's family name.
fn load_family(db: &mut Database, path: &Path) -> Option<String> {
    let ids = db.load_font_source(Source::File(path.to_path_buf()));
    let id = *ids.first()?;
    db.face(id)?.families.first().map(|(name, _)| name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts")
    }

    #[test]
    fn loads_only_the_named_files() {
        let f = FontSet::load(&FontPaths {
            sans: dir().join("NotoSans-Regular.ttf"),
            mono: dir().join("NotoSansMono-Regular.ttf"),
            fallback: Vec::new(),
        });
        assert_eq!(f.face_count(), 2);
        assert!(f.sans_family().is_some_and(|s| s.contains("Sans")));
        assert!(f.mono_family().is_some_and(|s| s.contains("Mono")));
    }

    #[test]
    fn a_missing_file_loads_nothing_and_does_not_panic() {
        let f = FontSet::load(&FontPaths {
            sans: dir().join("missing.ttf"),
            mono: dir().join("NotoSansMono-Regular.ttf"),
            fallback: vec![dir().join("also-missing.ttf")],
        });
        assert_eq!(f.face_count(), 1);
        assert_eq!(f.sans_family(), None);
    }

    struct Fixed;

    impl FontResolver for Fixed {
        fn sans(&self) -> PathBuf {
            dir().join("NotoSans-Regular.ttf")
        }
        fn mono(&self) -> PathBuf {
            dir().join("NotoSansMono-Regular.ttf")
        }
        fn family(&self, _name: &str) -> Option<PathBuf> {
            None
        }
        fn fallback_set(&self) -> Vec<PathBuf> {
            Vec::new()
        }
        fn lookup(&self, _c: char) -> Option<PathBuf> {
            None
        }
    }

    #[test]
    fn paths_from_a_resolver() {
        let p = FontPaths::from_resolver(&Fixed);
        assert_eq!(p.sans, Fixed.sans());
        assert_eq!(p.mono, Fixed.mono());
        assert!(p.fallback.is_empty());
    }
}

//! Fonts, loaded by path only. The renderer never scans the system: a shell's
//! `FontResolver` names the files, and `FontSet::load` reads exactly those.
//!
//! Fallback is in two steps. Shaping tries the span's face, then the fixed
//! fallback set in order (`FixedFallback`), then any other loaded face. A
//! codepoint still missing (shaped as `.notdef`) is queued to a worker
//! (`FontFallback`) that asks `FontResolver::lookup` for one file, reads
//! its faces and wakes the shell; the view then loads the faces and lays out
//! again the lines that showed `.notdef`. Found files are remembered in the
//! shell's font cache file as `glyph<TAB>hex<TAB>path` lines, so a later run
//! skips the lookup.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use cosmic_text::fontdb::{Database, FaceInfo, Source};
use cosmic_text::{Fallback, FontSystem};
use editor_core::host::Waker;
use unicode_script::Script;

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
    /// A face covering `c`. Called on the fallback worker, never on the
    /// start-up path.
    fn lookup(&self, c: char) -> Option<PathBuf>;
    /// One throwaway lookup that readies the platform's lookup machinery,
    /// called on the worker once the first frame is out (Core Text on
    /// macOS). The default does nothing.
    fn warm_up(&self) {}
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
    /// Present when built `with_lookup`.
    pub(crate) fallback: Option<FontFallback>,
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
        let fixed = FixedFallback::of(&db, &paths.fallback);
        let system = FontSystem::new_with_locale_and_db_and_fallback(LOCALE.to_string(), db, fixed);
        FontSet {
            system,
            sans,
            mono,
            fallback: None,
        }
    }

    /// Loads the resolver's start-up files, then hands the resolver to a
    /// lazy fallback worker. `cache` is the shell's font cache file: found
    /// codepoints are appended to it when it exists. `waker` is called from
    /// the worker when a face is ready, and the shell then calls
    /// `DocView::poll_fonts`. No thread starts before
    /// `DocView::start_font_fallback`.
    pub fn with_lookup(
        resolver: Box<dyn FontResolver>,
        cache: Option<PathBuf>,
        waker: Waker,
    ) -> FontSet {
        let paths = FontPaths::from_resolver(resolver.as_ref());
        let mut set = FontSet::load(&paths);
        set.fallback = Some(FontFallback::new(resolver, &paths, cache, waker));
        set
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

/// Family names live as long as the process, because cosmic-text's
/// `Fallback` hands out `&'static str`. Each distinct name is leaked once.
fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(n) = names.get(name) {
        return n;
    }
    let n: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(n);
    n
}

/// The fallback order handed to cosmic-text: after the span's own face, the
/// fixed fallback set's families in load order. Nothing is per script and
/// nothing is forbidden; faces loaded later are still tried after these.
struct FixedFallback {
    families: Vec<&'static str>,
}

impl FixedFallback {
    fn of(db: &Database, files: &[PathBuf]) -> FixedFallback {
        let mut families: Vec<&'static str> = Vec::new();
        for f in files {
            let faces = db.faces().filter(|face| match &face.source {
                Source::File(p) | Source::SharedFile(p, _) => p == f,
                Source::Binary(_) => false,
            });
            for face in faces {
                if let Some((name, _)) = face.families.first() {
                    let name = intern(name);
                    if !families.contains(&name) {
                        families.push(name);
                    }
                }
            }
        }
        FixedFallback { families }
    }
}

impl Fallback for FixedFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &self.families
    }
    fn forbidden_fallback(&self) -> &[&'static str] {
        &[]
    }
    fn script_fallback(&self, _script: Script, _locale: &str) -> &[&'static str] {
        &[]
    }
}

/// The tag of a codepoint line in the font cache file.
const GLYPH_TAG: &str = "glyph";

/// The name of Apple's catch-all face, both as a file stem and a PostScript
/// name. Its cmap maps every codepoint to one placeholder glyph, so once
/// loaded it would answer every later missing codepoint and stop real
/// lookups: it is never loaded or cached.
const LAST_RESORT: &str = "LastResort";

/// True when `path` is the catch-all face's file.
pub fn is_last_resort(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case(LAST_RESORT))
}

/// A font cache file line mapping `c` to the file that covers it, or
/// `None` when the path can't be written as one plain-text line.
pub fn glyph_line(c: char, path: &Path) -> Option<String> {
    let p = path
        .to_str()
        .filter(|p| !p.is_empty() && !p.contains(['\n', '\r', '\t']))?;
    Some(format!("{GLYPH_TAG}\t{:x}\t{p}\n", u32::from(c)))
}

/// The codepoint lines of a font cache file. Other lines belong to the
/// shell and are skipped, as are malformed ones and the catch-all face.
pub fn parse_glyph_lines(text: &str) -> HashMap<char, PathBuf> {
    text.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix(GLYPH_TAG)?.strip_prefix('\t')?;
            let (hex, path) = rest.split_once('\t')?;
            let c = char::from_u32(u32::from_str_radix(hex, 16).ok()?)?;
            let path = PathBuf::from(path);
            (!path.as_os_str().is_empty() && !is_last_resort(&path)).then_some((c, path))
        })
        .collect()
}

/// A file worth reading for a codepoint: it exists and is not the
/// catch-all face.
fn usable(path: &Path) -> bool {
    path.is_file() && !is_last_resort(path)
}

/// The faces in `path`, when one of them really maps `c` to a glyph. The
/// catch-all face is left out by its PostScript name too.
fn covering_faces(path: &Path, c: char) -> Option<Vec<FaceInfo>> {
    let mut db = Database::new();
    db.load_font_source(Source::File(path.to_path_buf()));
    let faces: Vec<FaceInfo> = db
        .faces()
        .filter(|f| f.post_script_name != LAST_RESORT)
        .cloned()
        .collect();
    let maps = |f: &FaceInfo| {
        db.with_face_data(f.id, |data, index| {
            let font = swash::FontRef::from_index(data, index as usize);
            font.is_some_and(|r| r.charmap().map(c) != 0)
        })
    };
    faces.iter().any(|f| maps(f) == Some(true)).then_some(faces)
}

/// What the main thread asks the worker.
enum Ask {
    Char(char),
    WarmUp,
}

/// Before `start`, the resolver and what the worker will need; after, the
/// channel to it.
enum Worker {
    Idle {
        resolver: Box<dyn FontResolver>,
        loaded: HashSet<PathBuf>,
        cache: Option<PathBuf>,
        waker: Waker,
    },
    Running(Sender<Ask>),
    Gone,
}

/// The handle to the lazy fallback worker, owned by the layout. Each
/// codepoint is asked once per process; asks made before `start` wait for it.
pub struct FontFallback {
    worker: Worker,
    asked: HashSet<char>,
    held: Vec<char>,
    found: Option<Receiver<Vec<FaceInfo>>>,
}

impl FontFallback {
    fn new(
        resolver: Box<dyn FontResolver>,
        paths: &FontPaths,
        cache: Option<PathBuf>,
        waker: Waker,
    ) -> FontFallback {
        let loaded = [&paths.sans, &paths.mono]
            .into_iter()
            .chain(&paths.fallback)
            .cloned()
            .collect();
        FontFallback {
            worker: Worker::Idle {
                resolver,
                loaded,
                cache,
                waker,
            },
            asked: HashSet::new(),
            held: Vec::new(),
            found: None,
        }
    }

    /// Queues `c` for a lookup, once.
    pub(crate) fn ask(&mut self, c: char) {
        if !self.asked.insert(c) {
            return;
        }
        match &self.worker {
            Worker::Running(tx) => {
                let _ = tx.send(Ask::Char(c));
            }
            Worker::Idle { .. } => self.held.push(c),
            Worker::Gone => {}
        }
    }

    /// Starts the worker: it warms the resolver up, then takes the held
    /// codepoints. Later calls do nothing.
    pub(crate) fn start(&mut self) {
        let Worker::Idle {
            resolver,
            loaded,
            cache,
            waker,
        } = std::mem::replace(&mut self.worker, Worker::Gone)
        else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let (found_tx, found_rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("font-fallback".into())
            .spawn(move || run(resolver, loaded, cache, waker, rx, found_tx));
        if spawned.is_err() {
            return;
        }
        let _ = tx.send(Ask::WarmUp);
        for c in self.held.drain(..) {
            let _ = tx.send(Ask::Char(c));
        }
        self.worker = Worker::Running(tx);
        self.found = Some(found_rx);
    }

    /// The faces the worker has read since the last call.
    pub(crate) fn take(&mut self) -> Vec<FaceInfo> {
        let Some(rx) = &self.found else {
            return Vec::new();
        };
        rx.try_iter().flatten().collect()
    }
}

/// The worker loop: each codepoint is answered from the cache file or, when
/// that has nothing usable, the resolver. The answer is read and kept only
/// when a face in it really covers the codepoint; a file not loaded yet then
/// has its faces sent.
fn run(
    resolver: Box<dyn FontResolver>,
    mut loaded: HashSet<PathBuf>,
    cache: Option<PathBuf>,
    waker: Waker,
    rx: Receiver<Ask>,
    found: Sender<Vec<FaceInfo>>,
) {
    let mut known = cache
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_glyph_lines(&t))
        .unwrap_or_default();
    for ask in rx {
        let c = match ask {
            Ask::WarmUp => {
                resolver.warm_up();
                continue;
            }
            Ask::Char(c) => c,
        };
        let cached = known.get(&c).filter(|p| usable(p)).cloned();
        let hit = cached.and_then(|p| covering_faces(&p, c).map(|f| (p, f)));
        let (path, faces) = match hit {
            Some(hit) => hit,
            None => {
                let Some(p) = resolver.lookup(c).filter(|p| usable(p)) else {
                    continue;
                };
                let Some(faces) = covering_faces(&p, c) else {
                    continue;
                };
                if let (Some(file), Some(line)) = (&cache, glyph_line(c, &p)) {
                    append(file, &line);
                }
                known.insert(c, p.clone());
                (p, faces)
            }
        };
        if !loaded.insert(path) || found.send(faces).is_err() {
            continue;
        }
        waker();
    }
}

/// Appends `line` to the cache file, only when the shell has written it.
fn append(file: &Path, line: &str) {
    let f = std::fs::OpenOptions::new().append(true).open(file);
    if let Ok(mut f) = f {
        let _ = f.write_all(line.as_bytes());
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
    fn glyph_lines_round_trip_and_skip_the_shells_lines() {
        let p = PathBuf::from("/f d/Symbols.ttf");
        let line = glyph_line('\u{2610}', &p).unwrap();
        assert_eq!(line, "glyph\t2610\t/f d/Symbols.ttf\n");
        let text = format!("font-paths v1 linux\nsans\t/S.ttf\n{line}glyph\t1f600\t/E.ttc\n");
        let m = parse_glyph_lines(&text);
        assert_eq!(m.len(), 2);
        assert_eq!(m[&'\u{2610}'], p);
        assert_eq!(m[&'\u{1F600}'], PathBuf::from("/E.ttc"));
        let bad = "glyph\tzz\t/a\nglyph\td800\t/b\nglyph\t41\t\nglyph 41 /c\n";
        assert!(parse_glyph_lines(bad).is_empty());
    }

    #[test]
    fn the_catch_all_face_is_never_cached() {
        let lr = Path::new("/System/Library/Fonts/LastResort.otf");
        assert!(is_last_resort(lr));
        assert!(!is_last_resort(Path::new("/f/LastResortish.otf")));
        let text = format!(
            "glyph\t4e00\t{}\nglyph\te000\t/x/lastresort.ttf\nglyph\t2610\t/f/S.ttf\n",
            lr.display()
        );
        let m = parse_glyph_lines(&text);
        assert_eq!(m.len(), 1);
        assert!(m.contains_key(&'\u{2610}'));
    }

    #[test]
    fn paths_that_are_not_one_plain_line_are_not_cached() {
        assert_eq!(glyph_line('a', Path::new("/f/a\nb.ttf")), None);
        assert_eq!(glyph_line('a', Path::new("/f/a\tb.ttf")), None);
        assert_eq!(glyph_line('a', Path::new("")), None);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let raw = std::ffi::OsStr::from_bytes(b"/f/\xff.ttf");
            assert_eq!(glyph_line('a', Path::new(raw)), None);
        }
    }

    #[test]
    fn coverage_is_checked_in_the_file_read() {
        let sym = dir().join("NotoSansSymbols2-Regular.ttf");
        assert!(covering_faces(&sym, '\u{2610}').is_some());
        assert!(covering_faces(&dir().join("NotoSans-Regular.ttf"), '\u{2610}').is_none());
        assert!(covering_faces(&dir().join("missing.ttf"), 'a').is_none());
    }

    #[test]
    fn the_fixed_set_is_tried_in_load_order() {
        let files = vec![
            dir().join("NotoSansSymbols2-Regular.ttf"),
            dir().join("NotoSansMono-Regular.ttf"),
        ];
        let f = FontSet::load(&FontPaths {
            sans: dir().join("NotoSans-Regular.ttf"),
            mono: dir().join("NotoSans-Regular.ttf"),
            fallback: files.clone(),
        });
        let fixed = FixedFallback::of(f.system.db(), &files);
        assert_eq!(fixed.families.len(), 2);
        assert!(fixed.families[0].contains("Symbols"));
        assert!(fixed.families[1].contains("Mono"));
        assert!(std::ptr::eq(intern(fixed.families[0]), fixed.families[0]));
    }

    #[test]
    fn paths_from_a_resolver() {
        let p = FontPaths::from_resolver(&Fixed);
        assert_eq!(p.sans, Fixed.sans());
        assert_eq!(p.mono, Fixed.mono());
        assert!(p.fallback.is_empty());
    }
}

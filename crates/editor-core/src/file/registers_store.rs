//! The named registers `"a` to `"z`, shared between every window through
//! `registers.toml` in the state directory (as vim's viminfo shares them).
//!
//! The file holds one table per register:
//!
//! ```toml
//! [a]
//! text = "a line\n"
//! linewise = true
//! ```
//!
//! A write takes an exclusive `flock` on `registers.lock`, reads the file
//! again, merges in its one register, writes a temporary file and renames
//! it over the old one, so two windows writing at once both keep their
//! register. A read goes back to the file only when its modification time
//! (or size) changed. A file that cannot be read as registers is set aside
//! as `registers.toml.bad` and the registers start empty again.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::file::StateDir;
use crate::vim::register::Register;

/// The registers, by name.
type Regs = BTreeMap<char, Register>;

/// What identifies one version of the file: its modification time and
/// size. `None` when there is no file.
type Stamp = Option<(SystemTime, u64)>;

#[derive(Debug)]
pub struct RegisterStore {
    state: StateDir,
    file: PathBuf,
    lock: PathBuf,
    /// The registers as last read or written.
    cache: Regs,
    /// The file `cache` came from; `None` before the first read.
    seen: Option<Stamp>,
    /// A file that could not be read was set aside.
    reset: bool,
}

impl RegisterStore {
    /// The store in `state`. Nothing is read or created until a register
    /// is asked for or written.
    pub fn new(state: &StateDir) -> RegisterStore {
        let root = state.root();
        RegisterStore {
            state: state.clone(),
            file: root.join("registers.toml"),
            lock: root.join("registers.lock"),
            cache: Regs::new(),
            seen: None,
            reset: false,
        }
    }

    /// Register `name` (`a` to `z`), as the file holds it now. The file is
    /// read again only when it changed since the last read. When it cannot
    /// be read at all, the registers last seen stand.
    pub fn get(&mut self, name: char) -> Option<Register> {
        if let Ok(now) = stamp(&self.file)
            && self.seen != Some(now)
        {
            if now.is_none() {
                self.cache.clear();
                self.seen = Some(None);
            } else if let Ok(lock) = self.lock() {
                // Under the lock, so a corrupt file is set aside only while
                // no other window is writing a good one.
                let _ = self.load();
                drop(lock);
            }
        }
        self.cache.get(&name).cloned()
    }

    /// Stores `reg` as register `name` (`a` to `z`), keeping every other
    /// register the file holds. Under an exclusive lock: read the file
    /// again, merge in this one register, write a temporary file and rename
    /// it over the old one. On failure the register is still kept here,
    /// until another window changes the file.
    pub fn set(&mut self, name: char, reg: Register) -> io::Result<()> {
        let result = self.write(name, reg.clone());
        if result.is_err() {
            self.cache.insert(name, reg);
            // The file as it is now counts as read, so the next `get` keeps
            // the cache rather than reading the file back over it.
            self.seen = stamp(&self.file).ok();
        }
        result
    }

    fn write(&mut self, name: char, reg: Register) -> io::Result<()> {
        self.state.ensure()?;
        let lock = self.lock()?;
        self.load()?;
        self.cache.insert(name, reg);
        let tmp = self.file.with_extension("toml.tmp");
        let mut out = user_file(&tmp, true)?;
        out.write_all(serialise(&self.cache).as_bytes())?;
        drop(out);
        fs::rename(&tmp, &self.file)?;
        self.seen = Some(stamp(&self.file)?);
        drop(lock);
        Ok(())
    }

    /// True once after a file that could not be read was set aside.
    pub fn take_reset(&mut self) -> bool {
        std::mem::take(&mut self.reset)
    }

    /// Takes the exclusive lock, which holds until the file is dropped.
    /// Each call opens `registers.lock` afresh: `flock` is per open file,
    /// so two stores (in one process or two) exclude each other.
    fn lock(&self) -> io::Result<File> {
        let f = user_file(&self.lock, false)?;
        f.lock()?;
        Ok(f)
    }

    /// Reads the file into `cache`, under the lock. A file that is not
    /// registers is renamed to `registers.toml.bad`, and the registers
    /// start empty.
    fn load(&mut self) -> io::Result<()> {
        self.cache = match fs::read(&self.file) {
            Ok(bytes) => match parse(&bytes) {
                Some(regs) => regs,
                None => {
                    fs::rename(&self.file, self.file.with_extension("toml.bad"))?;
                    self.reset = true;
                    Regs::new()
                }
            },
            Err(e) if e.kind() == ErrorKind::NotFound => Regs::new(),
            Err(e) => return Err(e),
        };
        self.seen = Some(stamp(&self.file)?);
        Ok(())
    }
}

fn stamp(path: &Path) -> io::Result<Stamp> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some((m.modified()?, m.len()))),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Opens `path` to write, creating it readable only by the user when it
/// is missing. `truncate` empties a file left there.
fn user_file(path: &Path, truncate: bool) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).truncate(truncate).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// The registers in `bytes`, or `None` when they are not UTF-8 TOML of
/// the expected shape: tables named `a` to `z`, each with a string `text`
/// and a boolean `linewise`.
fn parse(bytes: &[u8]) -> Option<Regs> {
    let table: toml::Table = std::str::from_utf8(bytes).ok()?.parse().ok()?;
    table
        .iter()
        .map(|(key, value)| {
            let mut chars = key.chars();
            let name = chars.next().filter(char::is_ascii_lowercase)?;
            if chars.next().is_some() {
                return None;
            }
            let reg = value.as_table()?;
            let text = reg.get("text")?.as_str()?.to_owned();
            let linewise = reg.get("linewise")?.as_bool()?;
            Some((name, Register { text, linewise }))
        })
        .collect()
}

fn serialise(regs: &Regs) -> String {
    let table: toml::Table = regs
        .iter()
        .map(|(name, reg)| {
            let mut t = toml::Table::new();
            t.insert("text".into(), reg.text.clone().into());
            t.insert("linewise".into(), reg.linewise.into());
            (name.to_string(), t.into())
        })
        .collect();
    table.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings;
    use crate::testkit::*;
    use crate::view_model::{MsgKind, StatusLeft};

    fn bad(state: &StateDir) -> PathBuf {
        state.root().join("registers.toml.bad")
    }

    #[test]
    fn named_register_visible_to_another_store() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path());
        let (mut e, _host) = ed_with_state("first\nsecond", &state);
        keys(&mut e, "j\"ayy");
        let mut other = RegisterStore::new(&state);
        assert_eq!(other.get('a'), Some(Register::lines("second\n")));
        assert_eq!(other.get('b'), None);
    }

    #[test]
    fn a_store_sees_a_later_write_from_another() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path());
        let mut a = RegisterStore::new(&state);
        let mut b = RegisterStore::new(&state);
        assert_eq!(b.get('a'), None);
        a.set('a', Register::chars("one")).unwrap();
        assert_eq!(b.get('a'), Some(Register::chars("one")));
        b.set('b', Register::lines("two\n")).unwrap();
        a.set('a', Register::chars("three, longer")).unwrap();
        assert_eq!(b.get('a'), Some(Register::chars("three, longer")));
        assert_eq!(a.get('b'), Some(Register::lines("two\n")));
    }

    #[test]
    fn the_file_has_one_table_per_register() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path());
        let mut s = RegisterStore::new(&state);
        s.set('a', Register::lines("say \"hi\"\n")).unwrap();
        let text = std::fs::read_to_string(state.root().join("registers.toml")).unwrap();
        let table: toml::Table = text.parse().unwrap();
        let a = table["a"].as_table().unwrap();
        assert_eq!(a["text"].as_str(), Some("say \"hi\"\n"));
        assert_eq!(a["linewise"].as_bool(), Some(true));
    }

    #[test]
    fn a_new_store_touches_nothing_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path().join("state"));
        let _ = RegisterStore::new(&state);
        assert!(!state.root().exists());
    }

    #[test]
    fn concurrent_register_writes_keep_both() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path());
        let writers: Vec<_> = ['a', 'b']
            .into_iter()
            .map(|name| {
                let state = state.clone();
                std::thread::spawn(move || {
                    let mut s = RegisterStore::new(&state);
                    for i in 0..100 {
                        s.set(name, Register::chars(format!("{name}{i}"))).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let mut s = RegisterStore::new(&state);
        assert_eq!(s.get('a'), Some(Register::chars("a99")));
        assert_eq!(s.get('b'), Some(Register::chars("b99")));
    }

    #[test]
    fn named_yank_leaves_clipboard_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path());
        let (mut e, host) = ed_with_state("hello world", &state);
        host.set_clipboard("before");
        keys(&mut e, "\"ayy");
        assert_eq!(host.clipboard(), "before");
        assert_eq!(
            e.register(Some('a')),
            Some(Register::lines("hello world\n"))
        );
        // vim: the unnamed register gets the text as well.
        assert_eq!(e.register(None), Some(Register::lines("hello world\n")));
        keys(&mut e, "\"bdw");
        assert_eq!(host.clipboard(), "before");
        assert_eq!(e.register(Some('b')), Some(Register::chars("hello ")));
        // A plain yank still writes the clipboard.
        keys(&mut e, "yy");
        assert_eq!(host.clipboard(), "world\n");
    }

    #[test]
    fn corrupt_file_resets_with_message() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path());
        let file = state.root().join("registers.toml");
        std::fs::write(&file, "[a\ntext = ").unwrap();
        let (mut e, host) = ed_with_state("x", &state);
        host.state().now = 1_000;
        let fx = keys(&mut e, "\"ap");
        assert_eq!(e.text(), "x");
        assert_eq!(std::fs::read_to_string(bad(&state)).unwrap(), "[a\ntext = ");
        assert!(!file.exists());
        assert_eq!(e.message(), Some(strings::NAMED_REGISTERS_RESET));
        assert_eq!(
            e.status().left,
            StatusLeft::Message {
                text: strings::NAMED_REGISTERS_RESET.to_owned(),
                kind: MsgKind::Warning,
            }
        );
        // It fades as any message does.
        assert_eq!(fx.wake_at, Some(1_000 + crate::status::MESSAGE_FOR_NS));
        e.handle(crate::input::Input::Tick {
            now_ns: 1_000 + crate::status::MESSAGE_FOR_NS,
        });
        assert_eq!(e.message(), None);
        // Said once: the registers work again.
        keys(&mut e, "\"ayy\"ap");
        assert_eq!(e.text(), "x\nx");
        assert_eq!(e.message(), None);
    }

    #[test]
    fn a_wrongly_shaped_file_is_set_aside_too() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path());
        std::fs::write(state.root().join("registers.toml"), "[a]\ntext = 3\n").unwrap();
        let mut s = RegisterStore::new(&state);
        assert_eq!(s.get('a'), None);
        assert!(s.take_reset());
        assert!(!s.take_reset());
        assert!(bad(&state).exists());
    }

    #[test]
    fn a_write_over_a_corrupt_file_sets_it_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path());
        std::fs::write(state.root().join("registers.toml"), "not toml [").unwrap();
        let mut s = RegisterStore::new(&state);
        s.set('c', Register::chars("ok")).unwrap();
        assert!(s.take_reset());
        assert!(bad(&state).exists());
        assert_eq!(
            RegisterStore::new(&state).get('c'),
            Some(Register::chars("ok"))
        );
    }

    #[test]
    fn a_write_creates_the_state_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path().join("deep/state"));
        RegisterStore::new(&state)
            .set('z', Register::chars("z"))
            .unwrap();
        assert_eq!(
            RegisterStore::new(&state).get('z'),
            Some(Register::chars("z"))
        );
    }
}

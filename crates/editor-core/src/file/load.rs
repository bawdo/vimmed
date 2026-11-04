//! Reading the document from disk: its text as a `Buffer` (which keeps the
//! file's line endings, BOM and trailing newline) and the `DiskStat` it was
//! read at, so a later write can tell whether the file changed underneath.

use std::fs::{File, Metadata};
use std::io::{self, ErrorKind, Read};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::buffer::Buffer;

/// What identifies a version of a file on disk (`stat`, following links).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskStat {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    /// The modification time in nanoseconds since the epoch.
    pub mtime_ns: i64,
    /// The permission bits and file type, as `st_mode`.
    pub mode: u32,
    pub nlink: u64,
}

impl DiskStat {
    pub fn of(m: &Metadata) -> DiskStat {
        DiskStat {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime_ns: m.mtime().saturating_mul(1_000_000_000) + m.mtime_nsec(),
            mode: m.mode(),
            nlink: m.nlink(),
        }
    }

    /// True when `now` is another version of the file than `self`: it was
    /// replaced (device or inode), resized or modified. A change of mode
    /// or link count alone leaves the text as it was.
    pub fn changed(&self, now: &DiskStat) -> bool {
        (self.dev, self.ino, self.size, self.mtime_ns) != (now.dev, now.ino, now.size, now.mtime_ns)
    }
}

/// What reading a file found.
#[derive(Debug)]
pub enum Loaded {
    /// The file's text, and the stat it was read at.
    Text { buffer: Buffer, stat: DiskStat },
    /// No file at the path: it opens empty and the first write creates it.
    Missing,
    /// The file is not valid UTF-8; it opens read-only and is never
    /// written back.
    NotUtf8 { bytes_len: usize },
    /// The file exists but could not be read.
    Error(io::Error),
}

/// Reads `path` (following symlinks). The stat comes from the open file,
/// so it describes the bytes that were read.
pub fn load(path: &Path) -> Loaded {
    match read(path) {
        Ok((bytes, stat)) => match String::from_utf8(bytes) {
            Ok(text) => Loaded::Text {
                buffer: Buffer::from_text(&text),
                stat,
            },
            Err(e) => Loaded::NotUtf8 {
                bytes_len: e.as_bytes().len(),
            },
        },
        Err(e) if e.kind() == ErrorKind::NotFound => Loaded::Missing,
        Err(e) => Loaded::Error(e),
    }
}

fn read(path: &Path) -> io::Result<(Vec<u8>, DiskStat)> {
    let mut f = File::open(path)?;
    let meta = f.metadata()?;
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    f.read_to_end(&mut bytes)?;
    Ok((bytes, DiskStat::of(&meta)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_keeps_its_format_and_stat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "\u{FEFF}one\r\ntwo\r\n").unwrap();
        let Loaded::Text { buffer, stat } = load(&path) else {
            panic!("expected text");
        };
        assert_eq!(buffer.to_text(), "\u{FEFF}one\r\ntwo\r\n");
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(stat, DiskStat::of(&meta));
        assert_eq!(stat.size, 13);
    }

    #[test]
    fn missing_not_utf8_and_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(load(&dir.path().join("nope")), Loaded::Missing));
        let bad = dir.path().join("bad");
        std::fs::write(&bad, [b'a', 0xff, 0xfe]).unwrap();
        assert!(matches!(load(&bad), Loaded::NotUtf8 { bytes_len: 3 }));
        assert!(matches!(load(dir.path()), Loaded::Error(_)));
    }

    #[test]
    fn changed_ignores_mode_and_links() {
        let a = DiskStat {
            dev: 1,
            ino: 2,
            size: 3,
            mtime_ns: 4,
            mode: 0o100644,
            nlink: 1,
        };
        let relinked = DiskStat {
            mode: 0o100600,
            nlink: 2,
            ..a
        };
        assert!(!a.changed(&relinked));
        assert!(a.changed(&DiskStat { mtime_ns: 5, ..a }));
        assert!(a.changed(&DiskStat { ino: 9, ..a }));
        assert!(a.changed(&DiskStat { size: 0, ..a }));
    }
}

//! Writing the document to disk.
//!
//! The target is the file a symlink points at, so the link stays a link.
//! The usual path is atomic: the text goes to a temporary file beside the
//! target (`.<name>.<pid>.tmp`), which is synced, given the original's
//! permissions and renamed over it, and then the directory is synced. A
//! failure before the rename removes the temporary file and leaves the
//! original as it was.
//!
//! A hard-linked file is written in place (truncated, written and synced),
//! since a rename would split it from its other names. So is a file whose
//! directory cannot be written but which itself can.
//!
//! `force` (`:w!`) skips only the check that the file did not change on
//! disk since it was read. It never changes permissions.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::load::DiskStat;
use crate::strings;

/// Why a write did not happen.
#[derive(Debug)]
pub enum WriteError {
    /// The file on disk is not the version that was read.
    ChangedOnDisk,
    /// The document was read from a file that is not valid UTF-8.
    NotUtf8Source,
    /// The system refused.
    Io(io::Error),
}

impl WriteError {
    /// What the status bar says.
    pub fn message(&self) -> String {
        match self {
            WriteError::ChangedOnDisk => strings::CHANGED_ON_DISK_REFUSED.to_owned(),
            WriteError::NotUtf8Source => strings::NOT_UTF8_REFUSED.to_owned(),
            WriteError::Io(e) => strings::write_failed(&e.to_string()),
        }
    }
}

impl From<io::Error> for WriteError {
    fn from(e: io::Error) -> WriteError {
        WriteError::Io(e)
    }
}

/// The file system calls a write makes. Every method calls the OS; a test
/// double overrides the one it wants to fail.
pub trait WriteOps {
    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }
    /// The stat of `path`, following links.
    fn stat(&mut self, path: &Path) -> io::Result<DiskStat> {
        fs::metadata(path).map(|m| DiskStat::of(&m))
    }
    /// True when this process may write `path` (`access(W_OK)`).
    fn writable(&mut self, path: &Path) -> bool {
        let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a valid NUL-terminated path for the call.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    /// Creates `path`, which must not exist, with `mode` (less the umask).
    fn create_new(&mut self, path: &Path, mode: u32) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
    }
    /// Opens the existing `path` for writing, truncated.
    fn open_truncate(&mut self, path: &Path) -> io::Result<File> {
        OpenOptions::new().write(true).truncate(true).open(path)
    }
    fn write_all(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        file.write_all(bytes)
    }
    fn sync(&mut self, file: &File) -> io::Result<()> {
        file.sync_all()
    }
    /// Sets the permission bits of an open file.
    fn set_mode(&mut self, file: &File, mode: u32) -> io::Result<()> {
        file.set_permissions(Permissions::from_mode(mode))
    }
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }
    fn remove(&mut self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        File::open(dir)?.sync_all()
    }
}

/// The real file system.
pub struct Os;

impl WriteOps for Os {}

/// Writes `text` to `path` and returns the stat it left. `expected` is the
/// stat the document was read at, `None` when there was no file; unless
/// `force`, a file that is now another version (or that appeared since)
/// refuses with `ChangedOnDisk`.
pub fn write(
    path: &Path,
    text: &str,
    expected: Option<&DiskStat>,
    force: bool,
) -> Result<DiskStat, WriteError> {
    write_with(&mut Os, path, text, expected, force)
}

/// `write`, through `ops`.
pub fn write_with(
    ops: &mut dyn WriteOps,
    path: &Path,
    text: &str,
    expected: Option<&DiskStat>,
    force: bool,
) -> Result<DiskStat, WriteError> {
    let target = resolve(ops, path)?;
    let current = match ops.stat(&target) {
        Ok(st) => Some(st),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if !force {
        let changed = match (expected, &current) {
            (Some(was), Some(now)) => was.changed(now),
            // It appeared since it was read as missing.
            (None, Some(_)) => true,
            // Gone since: written back to the same path.
            (_, None) => false,
        };
        if changed {
            return Err(WriteError::ChangedOnDisk);
        }
    }
    let dir = target.parent().unwrap_or(Path::new("/")).to_path_buf();
    if let Some(cur) = &current {
        // A read-only file stays read-only: a rename would get round its
        // permissions, so the write fails as the OS would fail it.
        if !ops.writable(&target) {
            return Err(io::Error::from_raw_os_error(libc::EACCES).into());
        }
        if cur.nlink > 1 || !ops.writable(&dir) {
            write_in_place(ops, &target, text.as_bytes())?;
            return Ok(ops.stat(&target)?);
        }
    }
    write_atomic(ops, &target, &dir, text.as_bytes(), current.as_ref())?;
    Ok(ops.stat(&target)?)
}

/// The file `path` names: a symlink's resolved target, or, for a file
/// that does not exist yet, the name in its canonical parent directory.
fn resolve(ops: &mut dyn WriteOps, path: &Path) -> io::Result<PathBuf> {
    match ops.canonicalize(path) {
        Ok(target) => Ok(target),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let name = path.file_name().ok_or(e)?;
            let parent = match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p,
                _ => Path::new("."),
            };
            Ok(ops.canonicalize(parent)?.join(name))
        }
        Err(e) => Err(e),
    }
}

/// Truncates `target` and writes `bytes` into it, keeping its inode.
fn write_in_place(ops: &mut dyn WriteOps, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = ops.open_truncate(target)?;
    ops.write_all(&mut f, bytes)?;
    ops.sync(&f)
}

/// Writes `bytes` to `.<name>.<pid>.tmp` in `dir` and renames it over
/// `target`. The temporary file is removed if anything before the rename
/// fails. A new file gets the usual mode (0666 less the umask); an
/// existing one keeps its permission bits.
fn write_atomic(
    ops: &mut dyn WriteOps,
    target: &Path,
    dir: &Path,
    bytes: &[u8],
    current: Option<&DiskStat>,
) -> io::Result<()> {
    let name = target.file_name().unwrap_or_default();
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);
    // Left by an earlier run of this process id that died mid-write.
    let _ = ops.remove(&tmp);
    let mode = if current.is_some() { 0o600 } else { 0o666 };
    let mut f = ops.create_new(&tmp, mode)?;
    let staged = (|| {
        ops.write_all(&mut f, bytes)?;
        ops.sync(&f)?;
        if let Some(cur) = current {
            ops.set_mode(&f, cur.mode & 0o7777)?;
        }
        drop(f);
        ops.rename(&tmp, target)
    })();
    if let Err(e) = staged {
        let _ = ops.remove(&tmp);
        return Err(e);
    }
    // The rename is done and the text is in place; a directory that will
    // not sync (some network file systems) does not undo that.
    let _ = ops.sync_dir(dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().mode() & 0o7777
    }

    fn stat_of(path: &Path) -> DiskStat {
        DiskStat::of(&fs::metadata(path).unwrap())
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn round_trip_preserves_crlf_bom_trailing_newline_and_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        let original = "\u{FEFF}# one\r\ntwo\r\n";
        fs::write(&path, original).unwrap();
        set_mode(&path, 0o640);
        let super::super::load::Loaded::Text { mut buffer, stat } = super::super::load::load(&path)
        else {
            panic!("expected text");
        };
        let end = crate::buffer::Pos { line: 1, col: 3 };
        buffer.replace(end..end, "!");
        let after = write(&path, &buffer.to_text(), Some(&stat), false).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\u{FEFF}# one\r\ntwo!\r\n"
        );
        assert_eq!(mode_of(&path), 0o640);
        assert_eq!(after, stat_of(&path));
        // No temporary file is left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn write_through_symlink_keeps_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.md");
        let link = dir.path().join("link.md");
        fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let stat = stat_of(&link);
        write(&link, "new", Some(&stat), false).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    }

    #[test]
    fn hardlinked_file_written_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        fs::write(&a, "old").unwrap();
        fs::hard_link(&a, &b).unwrap();
        let stat = stat_of(&a);
        let after = write(&a, "new text", Some(&stat), false).unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "new text");
        assert_eq!(fs::read_to_string(&b).unwrap(), "new text");
        assert_eq!(after.ino, stat.ino);
        assert_eq!(after.nlink, 2);
    }

    /// Fails the write to the temporary file as a full disk does.
    struct DiskFull;

    impl WriteOps for DiskFull {
        fn write_all(&mut self, _file: &mut File, _bytes: &[u8]) -> io::Result<()> {
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        }
    }

    #[test]
    fn failed_temp_write_leaves_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "keep me").unwrap();
        set_mode(&path, 0o600);
        let stat = stat_of(&path);
        let err = write_with(&mut DiskFull, &path, "lost", Some(&stat), false).unwrap_err();
        assert!(matches!(&err, WriteError::Io(e) if e.raw_os_error() == Some(libc::ENOSPC)));
        assert_eq!(fs::read(&path).unwrap(), b"keep me");
        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(stat_of(&path), stat);
        // The temporary file is gone.
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["doc.md"]);
        let os = io::Error::from_raw_os_error(libc::ENOSPC).to_string();
        assert_eq!(err.message(), format!("write failed: {os}"));
    }

    #[test]
    fn unwritable_dir_writable_file_goes_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("locked");
        fs::create_dir(&sub).unwrap();
        let path = sub.join("doc.md");
        fs::write(&path, "old").unwrap();
        let stat = stat_of(&path);
        set_mode(&sub, 0o555);
        let result = write(&path, "new", Some(&stat), false);
        set_mode(&sub, 0o755);
        let after = result.unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(after.ino, stat.ino);
    }

    #[test]
    fn changed_on_disk_refuses_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "first").unwrap();
        let stat = stat_of(&path);
        // Another program writes a different length, so the stat differs
        // whatever the clock's resolution.
        fs::write(&path, "from elsewhere").unwrap();
        let err = write(&path, "mine", Some(&stat), false).unwrap_err();
        assert!(matches!(err, WriteError::ChangedOnDisk));
        assert_eq!(
            err.message(),
            "file changed on disk: :w! to overwrite, :e! to reload"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "from elsewhere");
        write(&path, "mine", Some(&stat), true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
    }

    #[test]
    fn a_file_that_appeared_since_counts_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "someone else's").unwrap();
        assert!(matches!(
            write(&path, "mine", None, false),
            Err(WriteError::ChangedOnDisk)
        ));
    }

    #[test]
    fn a_deleted_file_is_written_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "first").unwrap();
        let stat = stat_of(&path);
        fs::remove_file(&path).unwrap();
        write(&path, "back", Some(&stat), false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "back");
    }

    #[test]
    fn force_never_changes_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "old").unwrap();
        set_mode(&path, 0o444);
        let stat = stat_of(&path);
        // A read-only file is not replaced by a rename, though its
        // directory would allow one: both writes fail with the OS error.
        let denied = io::Error::from_raw_os_error(libc::EACCES).to_string();
        for force in [false, true] {
            let err = write(&path, "new", Some(&stat), force).unwrap_err();
            assert_eq!(err.message(), format!("write failed: {denied}"));
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
        assert_eq!(mode_of(&path), 0o444);
    }

    #[test]
    fn missing_file_created_on_first_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.md");
        let stat = write(&path, "hello\n", None, false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
        assert_eq!(stat, stat_of(&path));
    }

    #[test]
    fn missing_parent_reports_os_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-dir").join("new.md");
        let err = write(&path, "hello", None, false).unwrap_err();
        let os = io::Error::from_raw_os_error(libc::ENOENT).to_string();
        assert_eq!(err.message(), format!("write failed: {os}"));
    }
}

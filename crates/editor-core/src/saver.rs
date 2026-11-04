//! Where the document goes to disk and comes back from: `Editor::write`,
//! `Editor::reload` and `Editor::close` are thin calls into a `Saver`.
//! Task 31 writes files for real; until then the editor holds `Unwired`,
//! which refuses every write and reload with a reason the editor shows.

use std::io;

use crate::strings;

/// The file behind an editor.
pub trait Saver {
    /// Writes `text` as the document. `force` is `:w!`, which skips only
    /// the check that the file did not change on disk.
    fn write(&mut self, text: &str, force: bool) -> io::Result<()>;
    /// The document's text as it is on disk now, for `:e!`.
    fn reload(&mut self) -> io::Result<String>;
    /// The window is closing with exit code `code`.
    fn close(&mut self, code: i32);
}

/// No file behind the editor yet: writes and reloads fail, saying so.
/// TODO(Task 31): replaced by the real file seam.
pub(crate) struct Unwired;

impl Saver for Unwired {
    fn write(&mut self, _text: &str, _force: bool) -> io::Result<()> {
        Err(io::Error::other(strings::NO_FILE_ACCESS))
    }
    fn reload(&mut self) -> io::Result<String> {
        Err(io::Error::other(strings::NO_FILE_ACCESS))
    }
    fn close(&mut self, _code: i32) {}
}

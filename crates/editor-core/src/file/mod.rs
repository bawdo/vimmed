//! What the editor keeps on disk: the document itself (`load`, `write`),
//! the state directory and the named registers shared between windows.

pub mod load;
pub mod lock;
pub mod recovery;
pub mod registers_store;
pub mod statedir;
pub mod watch;
pub mod write;

pub use load::{DiskStat, Loaded, load};
pub use registers_store::RegisterStore;
pub use statedir::StateDir;
pub use watch::{DiskEvent, Watcher};
pub use write::{WriteError, WriteOps, write, write_with};

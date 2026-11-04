//! What the editor keeps on disk outside the document: the state directory
//! and the named registers shared between windows.

pub mod registers_store;
pub mod statedir;

pub use registers_store::RegisterStore;
pub use statedir::StateDir;

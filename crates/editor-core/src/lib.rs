pub mod buffer;
mod colindex;
pub mod config;
pub mod editor;
pub mod effects;
pub mod ex;
pub mod file;
#[cfg(any(test, feature = "fuzzing"))]
pub mod fuzz;
pub mod grapheme;
pub mod help;
pub mod host;
pub mod input;
#[cfg(any(test, feature = "fuzzing"))]
mod keyseq;
pub mod limits;
pub mod links;
pub mod lists;
pub mod markdown;
pub mod nonvim;
pub mod platform;
pub mod saver;
pub mod search;
pub mod status;
pub mod strings;
pub mod substitute;
pub mod tables;
#[cfg(test)]
pub(crate) mod testkit;
pub mod tokens;
pub mod undo;
pub mod view_model;
pub mod vim;

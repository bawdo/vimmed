//! The Linux shell: GTK 4 with no `GApplication`, a plain window and one
//! custom widget whose snapshot appends one texture per 128 px strip of the
//! frame.
//!
//! `strips`, `timing`, `input`, `env` and the cache format in `fonts_fc` are
//! pure and build and test on every platform; everything that touches GTK or
//! fontconfig is compiled on Linux only.

pub mod env;
pub mod fonts_fc;
pub mod input;
pub mod strips;
pub mod timing;

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod canvas;
#[cfg(target_os = "linux")]
mod clipboard;

#[cfg(target_os = "linux")]
pub use app::run;

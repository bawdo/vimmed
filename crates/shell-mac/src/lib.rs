//! The macOS shell: AppKit through objc2, a layer-hosting view presented
//! from two IOSurfaces, and frame pacing from the view's display link.
//!
//! `present`, `glass`, `input` and the cache format in `fonts_ct` are pure
//! and build and test on every platform; everything that touches AppKit is
//! compiled on macOS only.

pub mod fonts_ct;
pub mod glass;
pub mod input;
pub mod present;
pub mod router;
pub mod startup;

#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod clipboard;
#[cfg(target_os = "macos")]
mod displaylink;
#[cfg(target_os = "macos")]
mod signal;
#[cfg(target_os = "macos")]
mod view;

#[cfg(target_os = "macos")]
pub use app::run;

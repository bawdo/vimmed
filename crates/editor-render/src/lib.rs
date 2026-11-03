//! The headless renderer: lays out source lines with cosmic-text, paints them
//! into a premultiplied RGBA frame and reports which pixel rows changed.
//! Shells copy the damaged rows to the screen; nothing here touches a window.
//!
//! Every colour, size and spacing comes from `editor_core::tokens`.

pub mod damage;
pub mod fonts;
pub mod frame;
pub mod images;
pub mod layout;
pub mod layout_cosmic;
pub mod paint;
mod status_paint;
pub mod view;

pub use editor_core::view_model::{Selection, StatusModel, ViewSource};

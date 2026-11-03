//! The one place the application name is read from. Every other crate takes its
//! names from these constants, which `build.rs` derives from the root manifest.

pub mod derive;

pub const SLUG: &str = env!("APP_META_SLUG");
pub const KEBAB: &str = env!("APP_META_KEBAB");
pub const DISPLAY_NAME: &str = env!("APP_META_DISPLAY_NAME");
pub const BUNDLE_NAME: &str = env!("APP_META_BUNDLE_NAME");
pub const BUNDLE_ID: &str = env!("APP_META_BUNDLE_ID");
pub const ENV_PREFIX: &str = env!("APP_META_ENV_PREFIX");
pub const WINDOW_BIN: &str = env!("APP_META_WINDOW_BIN");
pub const LINUX_LIBEXEC_DIR: &str = env!("APP_META_LINUX_LIBEXEC_DIR");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Wire protocol version. Not derived from the name.
pub const PROTOCOL: u32 = 1;

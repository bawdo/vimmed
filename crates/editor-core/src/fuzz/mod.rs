//! Fuzz harnesses. Each takes raw bytes and must return without
//! panicking. They are driven by the `fuzz/` crate (libFuzzer, nightly)
//! and replayed on stable by `replay`.

use crate::config::Config;
use crate::tokens::Theme;

#[cfg(test)]
mod replay;

/// A harness: raw bytes in, no panic out.
pub type Harness = fn(&[u8]);

/// Every harness by target name. Append new targets here.
pub const TARGETS: &[(&str, Harness)] = &[("config_parse", config_parse)];

/// The config file's text, as `Config::load` reads it: a file that is not
/// UTF-8 is an error there (the defaults), so there is nothing to parse.
pub fn config_parse(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (cfg, _) = Config::parse(text);
    for theme in [Theme::Light, Theme::Dark] {
        let _ = cfg.tokens(theme);
    }
}

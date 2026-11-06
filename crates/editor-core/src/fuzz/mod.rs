//! Fuzz harnesses. Each takes raw bytes and must return without
//! panicking. They are driven by the `fuzz/` crate (libFuzzer, nightly)
//! and replayed on stable by `replay`.

use crate::config::Config;
use crate::file::{recovery, registers_store};
use crate::tokens::Theme;

#[cfg(test)]
mod replay;

/// A harness: raw bytes in, no panic out.
pub type Harness = fn(&[u8]);

/// Every harness by target name. Append new targets here.
pub const TARGETS: &[(&str, Harness)] =
    &[("config_parse", config_parse), ("state_files", state_files)];

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

/// The two state-file parsers. The first byte picks one: `r` for the
/// registers file (the rest is its bytes), `m` for recovery metadata (the
/// rest, decoded lossily). Anything else, or nothing, is ignored.
pub fn state_files(data: &[u8]) {
    match data.split_first() {
        Some((b'r', rest)) => {
            if let Some(regs) = registers_store::parse(rest) {
                let again = registers_store::parse(registers_store::serialise(&regs).as_bytes());
                assert_eq!(again, Some(regs), "registers did not round-trip");
            }
        }
        Some((b'm', rest)) => {
            let text = String::from_utf8_lossy(rest);
            let _ = recovery::parse_meta(&text, "/fuzz.md", 1);
        }
        _ => {}
    }
}

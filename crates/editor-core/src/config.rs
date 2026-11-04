//! The user's settings: `$XDG_CONFIG_HOME/<slug>/config.toml` (or
//! `~/.config/<slug>/config.toml`), read once at open.
//!
//! ```toml
//! vim = true
//! autosave = false
//! theme = "system"      # or "light", "dark"
//! column_width = 660
//!
//! [font]
//! body = "Inter"
//! mono = "JetBrains Mono"
//! size = 16
//!
//! [tokens]              # any token, by its dotted name
//! "color.link" = "#2F5FA8"
//!
//! [tokens.dark]         # only in the dark theme
//! "color.bg" = "#101012"
//! ```
//!
//! Nothing in it stops the editor opening: a missing file is the defaults,
//! bad TOML is the defaults and a `config error`, and an unknown key or a
//! bad value is skipped and named in a message. The shells show the
//! messages in the status bar at open.
//!
//! The token set for a theme (`Config::tokens`) is built in this order:
//! the defaults, then the friendly keys (`font.body`, `font.mono`,
//! `font.size` and `column_width`, which is `space.column`), then
//! `[tokens]`, then `[tokens.light]` or `[tokens.dark]`. So `[tokens]`
//! beats a friendly key that names the same token.
//!
//! The last appearance the system reported is kept in the state directory
//! (`cached_appearance`), so a shell that learns it slowly (over D-Bus)
//! paints its first frame in the right theme.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use crate::file::StateDir;
use crate::strings;
use crate::tokens::{Theme, TokenId, Tokens};

/// Which theme to paint in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemePref {
    /// Follow the system's appearance, live.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    /// The theme this preference fixes, or `None` to follow the system.
    pub fn fixed(self) -> Option<Theme> {
        match self {
            ThemePref::System => None,
            ThemePref::Light => Some(Theme::Light),
            ThemePref::Dark => Some(Theme::Dark),
        }
    }
}

/// `[tokens]`, `[tokens.light]` and `[tokens.dark]`: (token name, value
/// as text) pairs, in file order. Names are checked when they are applied.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenOverrides {
    pub common: Vec<(String, String)>,
    pub light: Vec<(String, String)>,
    pub dark: Vec<(String, String)>,
}

/// The editor's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Vim keybindings.
    pub vim: bool,
    /// Write the file after a short idle pause following each change
    /// (`autosave`). Off by default.
    pub autosave: bool,
    /// `font.body`: the body text's family.
    pub font_body: Option<String>,
    /// `font.mono`: code's family.
    pub font_mono: Option<String>,
    /// `font.size`: the body text's size in points.
    pub font_size: f32,
    /// `column_width`: the text column's width in points (`space.column`).
    pub column_width: f32,
    pub theme: ThemePref,
    pub tokens: TokenOverrides,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            vim: true,
            autosave: false,
            font_body: None,
            font_mono: None,
            font_size: 16.0,
            column_width: 660.0,
            theme: ThemePref::System,
            tokens: TokenOverrides::default(),
        }
    }
}

impl Config {
    /// Where the environment says the config file is, or `None` when it
    /// names no usable `$XDG_CONFIG_HOME` or `$HOME`.
    pub fn path_from_env() -> Option<PathBuf> {
        config_path(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }

    /// `load` on the file the environment names (`path_from_env`); with
    /// none, the defaults.
    pub fn load_from_env() -> (Config, Vec<String>) {
        match Config::path_from_env() {
            Some(path) => Config::load(&path),
            None => {
                marks::mark(marks::names::CONFIG_READ);
                (Config::default(), Vec::new())
            }
        }
    }

    /// Reads the config file at `path`. Never fails: what cannot be used
    /// is left at its default and named in the messages, each said once.
    /// Marks `config_read` when done.
    pub fn load(path: &Path) -> (Config, Vec<String>) {
        let loaded = match std::fs::read_to_string(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Config::default(), Vec::new()),
            Err(e) => (
                Config::default(),
                vec![strings::config_error(&one_line(&e.to_string()))],
            ),
            Ok(text) => Config::parse(&text),
        };
        marks::mark(marks::names::CONFIG_READ);
        loaded
    }

    /// `load`, from the file's text.
    pub fn parse(text: &str) -> (Config, Vec<String>) {
        let table = match text.parse::<toml::Table>() {
            Ok(table) => table,
            Err(e) => {
                let message = strings::config_error(&one_line(e.message()));
                return (Config::default(), vec![message]);
            }
        };
        let mut cfg = Config::default();
        let mut r = Reader::default();
        for (key, value) in &table {
            match key.as_str() {
                "vim" => r.set(&mut cfg.vim, key, value, Reader::bool),
                "autosave" => r.set(&mut cfg.autosave, key, value, Reader::bool),
                "theme" => r.set(&mut cfg.theme, key, value, Reader::theme),
                "column_width" => r.set(&mut cfg.column_width, key, value, Reader::positive),
                "font" => r.font(&mut cfg, value),
                "tokens" => r.tokens(&mut cfg.tokens, value),
                _ => r.unknown(key),
            }
        }
        // The token names and values are checked against both themes.
        let mut messages = r.messages;
        for theme in [Theme::Light, Theme::Dark] {
            messages.extend(cfg.tokens(theme).1);
        }
        let mut seen = Vec::new();
        messages.retain(|m| {
            let new = !seen.contains(m);
            if new {
                seen.push(m.clone());
            }
            new
        });
        (cfg, messages)
    }

    /// The token set for `theme`: the defaults, then the friendly keys,
    /// then `[tokens]`, then the theme's own table. An override that names
    /// no token or has a bad value is skipped and named in the messages.
    pub fn tokens(&self, theme: Theme) -> (Tokens, Vec<String>) {
        let mut t = Tokens::defaults(theme);
        let mut messages = Vec::new();
        let friendly = [
            ("font.body", "font.body", self.font_body.clone()),
            ("font.mono", "font.mono", self.font_mono.clone()),
            ("font.size", "font.size", Some(self.font_size.to_string())),
            (
                "column_width",
                "space.column",
                Some(self.column_width.to_string()),
            ),
        ];
        for (key, token, value) in friendly {
            if let Some(value) = value {
                apply(&mut t, key, token, &value, &mut messages);
            }
        }
        let theme_table = match theme {
            Theme::Light => ("tokens.light.", &self.tokens.light),
            Theme::Dark => ("tokens.dark.", &self.tokens.dark),
        };
        for (prefix, pairs) in [("tokens.", &self.tokens.common), theme_table] {
            for (name, value) in pairs {
                let key = format!("{prefix}{name}");
                if TokenId::from_name(name).is_none() {
                    messages.push(strings::unknown_setting(&key));
                } else {
                    apply(&mut t, &key, name, value, &mut messages);
                }
            }
        }
        (t, messages)
    }
}

/// Sets token `token` to `value`, or says why it cannot under `key`.
fn apply(t: &mut Tokens, key: &str, token: &str, value: &str, messages: &mut Vec<String>) {
    let pair = [(token.to_owned(), value.to_owned())];
    if !t.apply_overrides(&pair).is_empty() {
        messages.push(strings::bad_setting(key, value));
    }
}

/// `text` on one line, for the status bar.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reads values out of the parsed file, noting what it cannot use.
#[derive(Default)]
struct Reader {
    messages: Vec<String>,
}

impl Reader {
    fn unknown(&mut self, key: &str) {
        self.messages.push(strings::unknown_setting(key));
    }

    fn bad(&mut self, key: &str, value: &toml::Value) {
        let shown = one_line(&value.to_string());
        self.messages.push(strings::bad_setting(key, &shown));
    }

    /// Sets `slot` to what `read` makes of `value`, or says it is bad.
    fn set<T>(
        &mut self,
        slot: &mut T,
        key: &str,
        value: &toml::Value,
        read: fn(&toml::Value) -> Option<T>,
    ) {
        match read(value) {
            Some(v) => *slot = v,
            None => self.bad(key, value),
        }
    }

    fn bool(value: &toml::Value) -> Option<bool> {
        value.as_bool()
    }

    fn theme(value: &toml::Value) -> Option<ThemePref> {
        let name = value.as_str()?.trim().to_ascii_lowercase();
        match name.as_str() {
            "system" => Some(ThemePref::System),
            "light" => Some(ThemePref::Light),
            "dark" => Some(ThemePref::Dark),
            _ => None,
        }
    }

    /// A number above zero, whole or not.
    fn positive(value: &toml::Value) -> Option<f32> {
        let n = match value {
            toml::Value::Integer(i) => *i as f32,
            toml::Value::Float(f) => *f as f32,
            _ => return None,
        };
        (n.is_finite() && n > 0.0).then_some(n)
    }

    /// A family name that is not blank.
    fn family(value: &toml::Value) -> Option<Option<String>> {
        let name = value.as_str()?.trim();
        (!name.is_empty()).then(|| Some(name.to_owned()))
    }

    /// `[font]`: `body`, `mono` and `size`.
    fn font(&mut self, cfg: &mut Config, value: &toml::Value) {
        let Some(table) = value.as_table() else {
            return self.bad("font", value);
        };
        for (key, value) in table {
            let full = format!("font.{key}");
            match key.as_str() {
                "body" => self.set(&mut cfg.font_body, &full, value, Reader::family),
                "mono" => self.set(&mut cfg.font_mono, &full, value, Reader::family),
                "size" => self.set(&mut cfg.font_size, &full, value, Reader::positive),
                _ => self.unknown(&full),
            }
        }
    }

    /// `[tokens]`, with `[tokens.light]` and `[tokens.dark]` inside it.
    fn tokens(&mut self, out: &mut TokenOverrides, value: &toml::Value) {
        let Some(table) = value.as_table() else {
            return self.bad("tokens", value);
        };
        for (key, value) in table {
            match (key.as_str(), value.as_table()) {
                ("light", Some(t)) => self.pairs("tokens.light.", "", t, &mut out.light),
                ("dark", Some(t)) => self.pairs("tokens.dark.", "", t, &mut out.dark),
                ("light" | "dark", None) => self.bad(&format!("tokens.{key}"), value),
                _ => self.pair("tokens.", key, value, &mut out.common),
            }
        }
    }

    /// The token (name, value) pairs in `table`, whose names start `name`.
    /// An unquoted dotted name (`color.bg = ...`) is nested tables in
    /// TOML; they are joined back into the one name. `prefix` is where the
    /// table sits in the file, for the messages.
    fn pairs(
        &mut self,
        prefix: &str,
        name: &str,
        table: &toml::Table,
        out: &mut Vec<(String, String)>,
    ) {
        for (key, value) in table {
            let full = if name.is_empty() {
                key.clone()
            } else {
                format!("{name}.{key}")
            };
            self.pair(prefix, &full, value, out);
        }
    }

    /// One token's value (or a nested table of them) as text.
    fn pair(
        &mut self,
        prefix: &str,
        name: &str,
        value: &toml::Value,
        out: &mut Vec<(String, String)>,
    ) {
        let text = match value {
            toml::Value::Table(inner) => return self.pairs(prefix, name, inner, out),
            toml::Value::String(s) => s.clone(),
            toml::Value::Integer(i) => i.to_string(),
            toml::Value::Float(f) => f.to_string(),
            _ if TokenId::from_name(name).is_none() => {
                return self.unknown(&format!("{prefix}{name}"));
            }
            _ => return self.bad(&format!("{prefix}{name}"), value),
        };
        out.push((name.to_owned(), text));
    }
}

/// `$XDG_CONFIG_HOME/<slug>/config.toml`, else `$HOME/.config/<slug>/...`.
/// An empty or relative value is ignored, as the XDG spec says.
fn config_path(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let base = absolute(xdg).or_else(|| Some(absolute(home)?.join(".config")))?;
    Some(base.join(app_meta::SLUG).join("config.toml"))
}

/// The cache file's word for each theme.
fn theme_word(theme: Theme) -> &'static str {
    match theme {
        Theme::Light => "light",
        Theme::Dark => "dark",
    }
}

/// The appearance the system last reported, as kept in `state`; `None`
/// when nothing (or nothing readable) is kept.
pub fn cached_appearance(state: &StateDir) -> Option<Theme> {
    let text = std::fs::read_to_string(state.appearance()).ok()?;
    [Theme::Light, Theme::Dark]
        .into_iter()
        .find(|t| text.trim() == theme_word(*t))
}

/// Keeps `theme` in `state` as the appearance the system last reported.
pub fn cache_appearance(state: &StateDir, theme: Theme) -> io::Result<()> {
    state.ensure()?;
    std::fs::write(state.appearance(), theme_word(theme))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::Redraw;
    use crate::input::Input;
    use crate::testkit::{ViewSource, build};
    use crate::tokens::{TokenId, parse_color};

    fn write(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn load(text: &str) -> (Config, Vec<String>) {
        let tmp = tempfile::tempdir().unwrap();
        Config::load(&write(tmp.path(), text))
    }

    #[test]
    fn missing_file_is_defaults_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let (cfg, messages) = Config::load(&tmp.path().join("config.toml"));
        assert_eq!(cfg, Config::default());
        assert!(messages.is_empty(), "{messages:?}");
        // The defaults from the plan.
        assert!(cfg.vim);
        assert!(!cfg.autosave);
        assert_eq!(cfg.font_body, None);
        assert_eq!(cfg.font_mono, None);
        assert_eq!(cfg.font_size, 16.0);
        assert_eq!(cfg.column_width, 660.0);
        assert_eq!(cfg.theme, ThemePref::System);
        assert_eq!(cfg.tokens, TokenOverrides::default());
        let (t, m) = cfg.tokens(Theme::Dark);
        assert_eq!(t, Tokens::defaults(Theme::Dark));
        assert!(m.is_empty());
    }

    #[test]
    fn bad_toml_falls_back_with_message() {
        let (cfg, messages) = load("vim = false\nautosave = = true\n");
        assert_eq!(cfg, Config::default());
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].starts_with("config error: "), "{}", messages[0]);
        // One line, for the status bar.
        assert!(!messages[0].contains('\n'), "{}", messages[0]);
    }

    #[test]
    fn good_settings_are_read() {
        let (cfg, messages) = load(
            "vim = false\nautosave = true\ntheme = \"dark\"\ncolumn_width = 720\n\
             [font]\nbody = \"Inter\"\nmono = \"Iosevka\"\nsize = 17.5\n",
        );
        assert!(messages.is_empty(), "{messages:?}");
        assert!(!cfg.vim);
        assert!(cfg.autosave);
        assert_eq!(cfg.theme, ThemePref::Dark);
        assert_eq!(cfg.column_width, 720.0);
        assert_eq!(cfg.font_body.as_deref(), Some("Inter"));
        assert_eq!(cfg.font_mono.as_deref(), Some("Iosevka"));
        assert_eq!(cfg.font_size, 17.5);
        let (t, m) = cfg.tokens(Theme::Light);
        assert!(m.is_empty(), "{m:?}");
        assert_eq!(t.family(TokenId::FontBody), "Inter");
        assert_eq!(t.family(TokenId::FontMono), "Iosevka");
        assert_eq!(t.px(TokenId::FontSize), 17.5);
        assert_eq!(t.px(TokenId::SpaceColumn), 720.0);
    }

    #[test]
    fn unknown_setting_reported() {
        let (cfg, messages) = load(
            "vim = false\nwrap = true\n[font]\nfamily = \"x\"\n\
             [tokens]\n\"color.nope\" = \"#000000\"\n\
             [tokens.dark]\n\"color.bogus\" = \"#000000\"\n[colours]\nbg = 1\n",
        );
        // The rest still applies.
        assert!(!cfg.vim);
        for want in [
            "unknown setting: wrap",
            "unknown setting: font.family",
            "unknown setting: tokens.color.nope",
            "unknown setting: tokens.dark.color.bogus",
            "unknown setting: colours",
        ] {
            assert!(messages.iter().any(|m| m == want), "{want}: {messages:?}");
        }
        // Each said once, though both themes were checked.
        assert_eq!(messages.len(), 5, "{messages:?}");
    }

    #[test]
    fn bad_values_reported_and_skipped() {
        let (cfg, messages) = load(
            "vim = \"yes\"\ntheme = \"sepia\"\ncolumn_width = -3\n\
             [font]\nsize = 0\n[tokens]\n\"color.bg\" = \"blue\"\n\"font.size.h1\" = true\n",
        );
        assert_eq!(cfg.vim, Config::default().vim);
        assert_eq!(cfg.theme, ThemePref::System);
        assert_eq!(cfg.column_width, 660.0);
        assert_eq!(cfg.font_size, 16.0);
        for want in [
            "bad value for vim: \"yes\"",
            "bad value for theme: \"sepia\"",
            "bad value for column_width: -3",
            "bad value for font.size: 0",
            "bad value for tokens.color.bg: blue",
            "bad value for tokens.font.size.h1: true",
        ] {
            assert!(messages.iter().any(|m| m == want), "{want}: {messages:?}");
        }
        let (t, _) = cfg.tokens(Theme::Light);
        assert_eq!(t, Tokens::defaults(Theme::Light));
    }

    #[test]
    fn unquoted_dotted_token_names_work() {
        let (cfg, messages) = load("[tokens]\ncolor.link = \"#102030\"\n");
        assert!(messages.is_empty(), "{messages:?}");
        let (t, _) = cfg.tokens(Theme::Light);
        assert_eq!(t.color(TokenId::ColorLink), parse_color("#102030").unwrap());
    }

    #[test]
    fn tokens_table_beats_friendly_keys() {
        let (cfg, messages) = load(
            "column_width = 700\n[font]\nsize = 18\nbody = \"Inter\"\n\
             [tokens]\n\"font.size\" = 20\n\"space.column\" = 800\n",
        );
        assert!(messages.is_empty(), "{messages:?}");
        for theme in [Theme::Light, Theme::Dark] {
            let (t, m) = cfg.tokens(theme);
            assert!(m.is_empty(), "{m:?}");
            assert_eq!(t.px(TokenId::FontSize), 20.0);
            assert_eq!(t.px(TokenId::SpaceColumn), 800.0);
            // A friendly key nothing overrides still applies.
            assert_eq!(t.family(TokenId::FontBody), "Inter");
        }
    }

    #[test]
    fn dark_table_applies_only_in_dark() {
        let (cfg, messages) = load(
            "[tokens]\n\"color.link\" = \"#111111\"\n\
             [tokens.dark]\n\"color.bg\" = \"#000000\"\n\"color.link\" = \"#222222\"\n\
             [tokens.light]\n\"color.caret\" = \"#333333\"\n",
        );
        assert!(messages.is_empty(), "{messages:?}");
        let (dark, _) = cfg.tokens(Theme::Dark);
        assert_eq!(
            dark.color(TokenId::ColorBg),
            parse_color("#000000").unwrap()
        );
        // The theme's table beats `[tokens]`.
        assert_eq!(
            dark.color(TokenId::ColorLink),
            parse_color("#222222").unwrap()
        );
        assert_eq!(
            dark.color(TokenId::ColorCaret),
            Tokens::defaults(Theme::Dark).color(TokenId::ColorCaret)
        );
        let (light, _) = cfg.tokens(Theme::Light);
        assert_eq!(
            light.color(TokenId::ColorBg),
            Tokens::defaults(Theme::Light).color(TokenId::ColorBg)
        );
        assert_eq!(
            light.color(TokenId::ColorLink),
            parse_color("#111111").unwrap()
        );
        assert_eq!(
            light.color(TokenId::ColorCaret),
            parse_color("#333333").unwrap()
        );
    }

    #[test]
    fn appearance_input_switches_tokens_and_redraws_all() {
        let mut e = build("# hi\n", Config::default(), false);
        assert_eq!(e.theme(), Theme::Light);
        assert_eq!(*e.tokens(), Tokens::defaults(Theme::Light));

        let fx = e.handle(Input::Appearance(Theme::Dark));
        assert_eq!(fx.redraw, Redraw::All);
        assert_eq!(e.theme(), Theme::Dark);
        assert_eq!(*e.tokens(), Tokens::defaults(Theme::Dark));

        // The same appearance again changes nothing.
        let fx = e.handle(Input::Appearance(Theme::Dark));
        assert_eq!(fx.redraw, Redraw::None);

        let fx = e.handle(Input::Appearance(Theme::Light));
        assert_eq!(fx.redraw, Redraw::All);
        assert_eq!(*e.tokens(), Tokens::defaults(Theme::Light));
    }

    #[test]
    fn a_fixed_theme_ignores_the_appearance() {
        let cfg = Config {
            theme: ThemePref::Dark,
            ..Config::default()
        };
        let mut e = build("text\n", cfg, false);
        assert_eq!(e.theme(), Theme::Dark);
        assert_eq!(*e.tokens(), Tokens::defaults(Theme::Dark));
        let fx = e.handle(Input::Appearance(Theme::Light));
        assert_eq!(fx.redraw, Redraw::None);
        assert_eq!(e.theme(), Theme::Dark);
    }

    #[test]
    fn the_editor_takes_its_tokens_from_the_config() {
        let (cfg, _) = load("[tokens.dark]\n\"color.bg\" = \"#000000\"\n");
        let mut e = build("text\n", cfg, false);
        let _ = e.handle(Input::Appearance(Theme::Dark));
        assert_eq!(
            e.tokens().color(TokenId::ColorBg),
            parse_color("#000000").unwrap()
        );
    }

    #[test]
    fn xdg_config_home_respected() {
        let file = Path::new(app_meta::SLUG).join("config.toml");
        assert_eq!(
            config_path(Some("/xdg/config".into()), Some("/home/u".into())),
            Some(Path::new("/xdg/config").join(&file))
        );
        let home = Some(Path::new("/home/u/.config").join(&file));
        assert_eq!(config_path(None, Some("/home/u".into())), home);
        // The XDG spec: an empty or relative value is ignored.
        assert_eq!(config_path(Some("".into()), Some("/home/u".into())), home);
        assert_eq!(
            config_path(Some("rel".into()), Some("/home/u".into())),
            home
        );
        assert_eq!(config_path(None, None), None);
    }

    #[test]
    fn appearance_cache_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::at(tmp.path().join("state"));
        assert_eq!(cached_appearance(&state), None);
        cache_appearance(&state, Theme::Dark).unwrap();
        assert_eq!(cached_appearance(&state), Some(Theme::Dark));
        cache_appearance(&state, Theme::Light).unwrap();
        assert_eq!(cached_appearance(&state), Some(Theme::Light));
        // Anything else in the file is no cache at all.
        std::fs::write(state.appearance(), "sepia").unwrap();
        assert_eq!(cached_appearance(&state), None);
    }
}

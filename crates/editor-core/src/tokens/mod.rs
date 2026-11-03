//! The design token layer. Every colour, font family, size, weight, spacing,
//! radius, line height and column width in the UI comes from here, and
//! `cargo xtask lint-literals` enforces it for the render and shell crates.

/// Generates `TokenId` and its name, kind and default lookups from one table.
macro_rules! token_table {
    ($($variant:ident, $name:literal, $kind:ident, $light:literal, $dark:literal;)*) => {
        /// A design token. The dotted names are fixed for good.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum TokenId { $($variant),* }

        impl TokenId {
            /// Every token, in table order (also the index order of `Tokens`).
            pub const ALL: &'static [TokenId] = &[$(TokenId::$variant),*];

            pub fn name(self) -> &'static str {
                match self { $(TokenId::$variant => $name),* }
            }

            pub fn kind(self) -> TokenKind {
                match self { $(TokenId::$variant => TokenKind::$kind),* }
            }

            pub fn from_name(name: &str) -> Option<TokenId> {
                match name { $($name => Some(TokenId::$variant),)* _ => None }
            }

            pub(super) fn default_text(self, theme: Theme) -> &'static str {
                match (self, theme) {
                    $((TokenId::$variant, Theme::Light) => $light,
                      (TokenId::$variant, Theme::Dark) => $dark,)*
                }
            }
        }
    };
}

mod defaults;
pub use defaults::TokenId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Color,
    Px,
    Family,
    Weight,
    Ratio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// An sRGB colour with straight (non-premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Parses `#RRGGBB` or `#RRGGBBAA`.
pub fn parse_color(s: &str) -> Result<Rgba, String> {
    let hex = s
        .strip_prefix('#')
        .ok_or_else(|| format!("colour must start with #: {s}"))?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) || !(hex.len() == 6 || hex.len() == 8) {
        return Err(format!("colour must be #RRGGBB or #RRGGBBAA: {s}"));
    }
    let byte = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| format!("bad hex digits in colour: {s}"))
    };
    Ok(Rgba {
        r: byte(0)?,
        g: byte(2)?,
        b: byte(4)?,
        a: if hex.len() == 8 { byte(6)? } else { 0xFF },
    })
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Color(Rgba),
    Px(f32),
    Family(String),
    Weight(u16),
    Ratio(f32),
}

impl Value {
    #[cfg(test)]
    fn kind(&self) -> TokenKind {
        match self {
            Value::Color(_) => TokenKind::Color,
            Value::Px(_) => TokenKind::Px,
            Value::Family(_) => TokenKind::Family,
            Value::Weight(_) => TokenKind::Weight,
            Value::Ratio(_) => TokenKind::Ratio,
        }
    }

    fn parse(kind: TokenKind, s: &str) -> Result<Value, String> {
        let s = s.trim();
        let number = || {
            s.parse::<f32>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or_else(|| format!("not a number: {s}"))
        };
        match kind {
            TokenKind::Color => parse_color(s).map(Value::Color),
            TokenKind::Px => number().and_then(|n| {
                if n >= 0.0 {
                    Ok(Value::Px(n))
                } else {
                    Err("negative".into())
                }
            }),
            TokenKind::Ratio => number().and_then(|n| {
                if n > 0.0 {
                    Ok(Value::Ratio(n))
                } else {
                    Err("not positive".into())
                }
            }),
            TokenKind::Weight => s
                .parse::<u16>()
                .ok()
                .filter(|w| (1..=1000).contains(w))
                .map(Value::Weight)
                .ok_or_else(|| format!("weight must be 1..=1000: {s}")),
            TokenKind::Family if s.is_empty() => Err("empty family".into()),
            TokenKind::Family => Ok(Value::Family(s.to_string())),
        }
    }
}

/// A resolved set of token values for one theme.
#[derive(Clone, Debug)]
pub struct Tokens {
    values: Vec<Value>,
}

impl Tokens {
    pub fn defaults(theme: Theme) -> Tokens {
        let values = TokenId::ALL
            .iter()
            .map(|id| {
                Value::parse(id.kind(), id.default_text(theme))
                    .unwrap_or_else(|e| panic!("bad default for {}: {e}", id.name()))
            })
            .collect();
        Tokens { values }
    }

    fn value(&self, id: TokenId) -> &Value {
        &self.values[id as usize]
    }

    /// Panics if the stored value's kind differs from the token's kind.
    #[cfg(test)]
    pub(crate) fn check_kind(&self, id: TokenId) {
        assert_eq!(
            self.value(id).kind(),
            id.kind(),
            "kind mismatch for {}",
            id.name()
        );
    }

    pub fn color(&self, id: TokenId) -> Rgba {
        match self.value(id) {
            Value::Color(c) => *c,
            _ => mismatch(id, TokenKind::Color),
        }
    }

    pub fn px(&self, id: TokenId) -> f32 {
        match self.value(id) {
            Value::Px(n) => *n,
            _ => mismatch(id, TokenKind::Px),
        }
    }

    pub fn family(&self, id: TokenId) -> &str {
        match self.value(id) {
            Value::Family(f) => f,
            _ => mismatch(id, TokenKind::Family),
        }
    }

    pub fn weight(&self, id: TokenId) -> u16 {
        match self.value(id) {
            Value::Weight(w) => *w,
            _ => mismatch(id, TokenKind::Weight),
        }
    }

    pub fn ratio(&self, id: TokenId) -> f32 {
        match self.value(id) {
            Value::Ratio(n) => *n,
            _ => mismatch(id, TokenKind::Ratio),
        }
    }

    /// Applies `(name, value)` pairs. Bad pairs are skipped and reported as
    /// `unknown token: <name>` or `bad value for <name>: <value>`.
    pub fn apply_overrides(&mut self, pairs: &[(String, String)]) -> Vec<String> {
        let mut messages = Vec::new();
        for (name, text) in pairs {
            let Some(id) = TokenId::from_name(name) else {
                messages.push(format!("unknown token: {name}"));
                continue;
            };
            match Value::parse(id.kind(), text) {
                Ok(v) => self.values[id as usize] = v,
                Err(_) => messages.push(format!("bad value for {name}: {text}")),
            }
        }
        messages
    }
}

/// A kind mismatch is a programming error: panic always. A wrong-kind read
/// cannot arise from user input, because `apply_overrides` parses by kind.
fn mismatch(id: TokenId, wanted: TokenKind) -> ! {
    panic!(
        "token {} is {:?}, read as {:?}",
        id.name(),
        id.kind(),
        wanted
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_has_light_and_dark_values_of_its_kind() {
        for id in TokenId::ALL {
            for t in [Theme::Light, Theme::Dark] {
                Tokens::defaults(t).check_kind(*id);
            }
        }
    }

    #[test]
    fn names_round_trip() {
        for id in TokenId::ALL {
            assert_eq!(TokenId::from_name(id.name()), Some(*id));
        }
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = TokenId::ALL.iter().map(|i| i.name()).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n);
    }

    #[test]
    fn override_colour() {
        let mut t = Tokens::defaults(Theme::Light);
        assert!(
            t.apply_overrides(&[("color.status.dirty".into(), "#B35C00".into())])
                .is_empty()
        );
        assert_eq!(
            t.color(TokenId::ColorStatusDirty),
            Rgba {
                r: 0xB3,
                g: 0x5C,
                b: 0,
                a: 0xFF
            }
        );
    }

    #[test]
    fn unknown_and_bad_values_are_reported_and_ignored() {
        let mut t = Tokens::defaults(Theme::Light);
        let m = t.apply_overrides(&[
            ("color.nope".into(), "#000000".into()),
            ("space.column".into(), "wide".into()),
        ]);
        assert_eq!(
            m,
            vec![
                "unknown token: color.nope",
                "bad value for space.column: wide"
            ]
        );
        assert_eq!(t.px(TokenId::SpaceColumn), 660.0);
    }

    #[test]
    #[should_panic(expected = "kind mismatch")]
    fn check_kind_panics_on_mismatch() {
        let mut t = Tokens::defaults(Theme::Light);
        t.values[TokenId::SpaceColumn as usize] = Value::Weight(400);
        t.check_kind(TokenId::SpaceColumn);
    }

    #[test]
    fn fixed_defaults_from_the_brief() {
        let t = Tokens::defaults(Theme::Light);
        assert_eq!(t.family(TokenId::FontBody), "system-ui");
        assert_eq!(t.family(TokenId::FontMono), "system-mono");
        assert_eq!(t.px(TokenId::FontSize), 16.0);
        assert_eq!(t.px(TokenId::FontSizeStatus), 11.5);
        assert_eq!(t.weight(TokenId::FontWeightHeading), 650);
        assert_eq!(t.ratio(TokenId::LineHeightCode), 1.4);
        assert_eq!(t.color(TokenId::ColorBg), parse_color("#FAF9F6").unwrap());
    }

    #[test]
    fn parse_color_accepts_rgb_and_rgba_only() {
        assert_eq!(
            parse_color("#102030").unwrap(),
            Rgba {
                r: 0x10,
                g: 0x20,
                b: 0x30,
                a: 255
            }
        );
        assert_eq!(parse_color("#10203040").unwrap().a, 0x40);
        for bad in ["102030", "#12345", "#12345g", "#é23456", "#+1+2+3", ""] {
            assert!(parse_color(bad).is_err(), "{bad}");
        }
    }

    fn luminance(c: Rgba) -> f64 {
        let lin = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    fn contrast(a: Rgba, b: Rgba) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_contrast_meets_wcag_aa_in_both_themes() {
        for theme in [Theme::Light, Theme::Dark] {
            let t = Tokens::defaults(theme);
            for (fg, bg) in [
                (TokenId::ColorText, TokenId::ColorBg),
                (TokenId::ColorStatusText, TokenId::ColorStatusBg),
            ] {
                let ratio = contrast(t.color(fg), t.color(bg));
                assert!(
                    ratio >= 4.5,
                    "{theme:?} {} on {}: {ratio:.2}",
                    fg.name(),
                    bg.name()
                );
            }
        }
    }
}

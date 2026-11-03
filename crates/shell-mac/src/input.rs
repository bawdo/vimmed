//! Turning AppKit key events into editor input. The mapping is pure so it
//! is tested everywhere; only `from_event` touches `NSEvent`.

use editor_core::input::{Input, Key, KeyEvent, Mods};

/// Virtual key codes from `HIToolbox/Events.h` (layout independent).
mod code {
    pub const RETURN: u16 = 36;
    pub const TAB: u16 = 48;
    pub const SPACE: u16 = 49;
    pub const DELETE: u16 = 51;
    pub const ESCAPE: u16 = 53;
    pub const KEYPAD_ENTER: u16 = 76;
    pub const HOME: u16 = 115;
    pub const PAGE_UP: u16 = 116;
    pub const FORWARD_DELETE: u16 = 117;
    pub const END: u16 = 119;
    pub const PAGE_DOWN: u16 = 121;
    pub const LEFT: u16 = 123;
    pub const RIGHT: u16 = 124;
    pub const DOWN: u16 = 125;
    pub const UP: u16 = 126;
}

/// `NSEventModifierFlags` bits.
mod flag {
    pub const SHIFT: usize = 1 << 17;
    pub const CONTROL: usize = 1 << 18;
    pub const OPTION: usize = 1 << 19;
    pub const COMMAND: usize = 1 << 20;
}

pub fn mods_from_flags(bits: usize) -> Mods {
    Mods {
        shift: bits & flag::SHIFT != 0,
        ctrl: bits & flag::CONTROL != 0,
        alt: bits & flag::OPTION != 0,
        cmd: bits & flag::COMMAND != 0,
    }
}

/// The input for one key-down. `chars` is `characters` (with Option and
/// Shift applied) and `bare` is `charactersIgnoringModifiers`, which is what
/// a Ctrl or Cmd shortcut means. Text the keyboard composes into more than
/// one character goes in as `Input::Text`.
pub fn map(code: u16, chars: &str, bare: &str, mods: Mods, time_ns: u64) -> Option<Input> {
    let named = match code {
        code::RETURN | code::KEYPAD_ENTER => Some(Key::Enter),
        code::TAB => Some(Key::Tab),
        code::SPACE => Some(Key::Space),
        code::DELETE => Some(Key::Backspace),
        code::ESCAPE => Some(Key::Esc),
        code::HOME => Some(Key::Home),
        code::PAGE_UP => Some(Key::PageUp),
        code::FORWARD_DELETE => Some(Key::Delete),
        code::END => Some(Key::End),
        code::PAGE_DOWN => Some(Key::PageDown),
        code::LEFT => Some(Key::Left),
        code::RIGHT => Some(Key::Right),
        code::DOWN => Some(Key::Down),
        code::UP => Some(Key::Up),
        _ => None,
    };
    let key = |key| Some(Input::Key(KeyEvent { key, mods, time_ns }));
    if let Some(k) = named {
        return key(k);
    }
    let text = if mods.ctrl || mods.cmd { bare } else { chars };
    let mut it = text.chars();
    match (it.next(), it.next()) {
        (Some(c), None) if !c.is_control() => key(Key::Char(c)),
        (Some(_), Some(_)) if !mods.ctrl && !mods.cmd && !text.chars().any(char::is_control) => {
            Some(Input::Text(text.to_owned()))
        }
        _ => None,
    }
}

#[cfg(target_os = "macos")]
pub fn from_event(e: &objc2_app_kit::NSEvent) -> Option<Input> {
    let chars = e.characters().map(|s| s.to_string()).unwrap_or_default();
    let bare = e
        .charactersIgnoringModifiers()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let mods = mods_from_flags(e.modifierFlags().0);
    let time_ns = (e.timestamp() * 1e9) as u64;
    map(e.keyCode(), &chars, &bare, mods, time_ns)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: Option<Input>) -> Option<(Key, Mods)> {
        match i {
            Some(Input::Key(k)) => Some((k.key, k.mods)),
            _ => None,
        }
    }

    #[test]
    fn named_keys_win_over_characters() {
        let m = Mods::default();
        assert_eq!(key(map(36, "\r", "\r", m, 0)), Some((Key::Enter, m)));
        assert_eq!(
            key(map(51, "\u{7f}", "\u{7f}", m, 0)),
            Some((Key::Backspace, m))
        );
        assert_eq!(
            key(map(123, "\u{f702}", "\u{f702}", m, 0)),
            Some((Key::Left, m))
        );
        assert_eq!(key(map(49, " ", " ", m, 0)), Some((Key::Space, m)));
    }

    #[test]
    fn printable_and_shortcut_characters() {
        let shift = mods_from_flags(1 << 17);
        assert_eq!(
            key(map(0, "A", "a", shift, 0)),
            Some((Key::Char('A'), shift))
        );
        let ctrl = mods_from_flags(1 << 18);
        assert!(ctrl.ctrl && !ctrl.cmd);
        assert_eq!(
            key(map(8, "\u{3}", "c", ctrl, 0)),
            Some((Key::Char('c'), ctrl))
        );
        let alt = mods_from_flags(1 << 19);
        assert_eq!(key(map(35, "π", "p", alt, 0)), Some((Key::Char('π'), alt)));
    }

    #[test]
    fn composed_text_and_nothing() {
        let m = Mods::default();
        assert_eq!(map(0, "´a", "a", m, 0), Some(Input::Text("´a".into())));
        assert_eq!(map(0, "", "", m, 0), None);
        let ev = map(0, "x", "x", m, 42);
        assert!(matches!(ev, Some(Input::Key(KeyEvent { time_ns: 42, .. }))));
    }
}

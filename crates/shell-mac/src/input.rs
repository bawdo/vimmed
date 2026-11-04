//! Turning AppKit key, mouse and scroll events into editor input. The
//! mapping is pure so it is tested everywhere; only `from_event` touches
//! `NSEvent` (and `app`, for the mouse, which needs the view's hit test).

use editor_core::input::{Input, Key, KeyEvent, Mods, MouseKind};

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

/// The named key on virtual key code `code`, if it is one.
fn named(code: u16) -> Option<Key> {
    Some(match code {
        code::RETURN | code::KEYPAD_ENTER => Key::Enter,
        code::TAB => Key::Tab,
        code::SPACE => Key::Space,
        code::DELETE => Key::Backspace,
        code::ESCAPE => Key::Esc,
        code::HOME => Key::Home,
        code::PAGE_UP => Key::PageUp,
        code::FORWARD_DELETE => Key::Delete,
        code::END => Key::End,
        code::PAGE_DOWN => Key::PageDown,
        code::LEFT => Key::Left,
        code::RIGHT => Key::Right,
        code::DOWN => Key::Down,
        code::UP => Key::Up,
        _ => return None,
    })
}

/// The key for one key-down: the named keys by virtual key code, else the
/// one character it types. `chars` is `characters` (with Option and Shift
/// applied) and `bare` is `charactersIgnoringModifiers`, which is what a
/// Ctrl or Cmd shortcut means. `None` for a dead key, a bare modifier or
/// more than one character.
pub fn map_key(code: u16, chars: &str, bare: &str, mods: Mods, time_ns: u64) -> Option<KeyEvent> {
    let key = |key| Some(KeyEvent { key, mods, time_ns });
    if let Some(k) = named(code) {
        return key(k);
    }
    let text = if mods.ctrl || mods.cmd { bare } else { chars };
    let mut it = text.chars();
    match (it.next(), it.next()) {
        (Some(c), None) if !c.is_control() => key(Key::Char(c)),
        _ => None,
    }
}

/// The input for one key-down mapped directly (no input method): the key,
/// or text the keyboard composed into more than one character as
/// `Input::Text`.
pub fn map(code: u16, chars: &str, bare: &str, mods: Mods, time_ns: u64) -> Option<Input> {
    if let Some(k) = map_key(code, chars, bare, mods, time_ns) {
        return Some(Input::Key(k));
    }
    let typed = !mods.ctrl && !mods.cmd && chars.chars().count() > 1;
    (typed && !chars.chars().any(char::is_control)).then(|| Input::Text(chars.to_owned()))
}

/// True when a key-down goes to the input method (`interpretKeyEvents:`)
/// rather than straight to the editor. Only while the editor takes text
/// (`Editor::ime_active`): then every key while text is being composed,
/// and otherwise only keys that could type, so Return, Tab, Esc, Space,
/// the arrows and every Ctrl or Cmd shortcut reach the editor as keys.
pub fn routes_to_ime(ime_active: bool, composing: bool, code: u16, mods: Mods) -> bool {
    ime_active && (composing || (named(code).is_none() && !mods.ctrl && !mods.cmd))
}

/// Byte offset `utf16` UTF-16 units into `text` (AppKit counts in UTF-16),
/// clamped to its length.
pub fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// The editor's kind for a button press with AppKit's `clickCount`: the
/// second click of a double-click is `DoubleClick`; any other press (a
/// third click too) is `Down`.
pub fn press_kind(click_count: isize) -> MouseKind {
    if click_count == 2 {
        MouseKind::DoubleClick
    } else {
        MouseKind::Down
    }
}

/// Points down the document for a scroll event's `scrollingDeltaY`, which
/// is positive towards the document's start. A trackpad's delta
/// (`hasPreciseScrollingDeltas`) is in points already; a wheel's is in
/// lines, which AppKit's scroll views scroll by the line height,
/// `line_pt`.
pub fn scroll_pt(delta_y: f64, precise: bool, line_pt: f32) -> f32 {
    let lines_or_pt = -delta_y as f32;
    if precise {
        lines_or_pt
    } else {
        lines_or_pt * line_pt
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
    fn key_table_gives_the_editors_key_events() {
        let ev = |code, chars: &str, bare: &str, bits| {
            map_key(code, chars, bare, mods_from_flags(bits), 7)
        };
        let ke = |key, mods| {
            Some(KeyEvent {
                key,
                mods,
                time_ns: 7,
            })
        };
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        // `<C-Space>` and `<S-Tab>`, as editor-core's testkit sends them.
        assert_eq!(ev(49, "\0", " ", 1 << 18), ke(Key::Space, ctrl));
        assert_eq!(ev(48, "\u{19}", "\u{19}", 1 << 17), ke(Key::Tab, shift));
        // Cmd+S and Cmd+Shift+R: the primary modifier with the bare key.
        let cmd = Mods {
            cmd: true,
            ..Mods::default()
        };
        assert_eq!(ev(1, "s", "s", 1 << 20), ke(Key::Char('s'), cmd));
        let cmd_shift = Mods { shift: true, ..cmd };
        assert_eq!(
            ev(15, "R", "R", (1 << 20) | (1 << 17)),
            ke(Key::Char('R'), cmd_shift)
        );
        for (code, key) in [
            (36, Key::Enter),
            (76, Key::Enter),
            (53, Key::Esc),
            (117, Key::Delete),
            (115, Key::Home),
            (119, Key::End),
            (116, Key::PageUp),
            (121, Key::PageDown),
            (125, Key::Down),
            (126, Key::Up),
            (124, Key::Right),
        ] {
            assert_eq!(ev(code, "", "", 0), ke(key, Mods::default()), "{code}");
        }
        // A dead key types nothing yet; composed text is no single key.
        assert_eq!(ev(14, "", "e", 1 << 19), None);
        assert_eq!(ev(0, "´a", "a", 0), None);
    }

    #[test]
    fn only_typing_keys_go_to_the_input_method() {
        let m = Mods::default();
        let cmd = mods_from_flags(1 << 20);
        let ctrl = mods_from_flags(1 << 18);
        let alt = mods_from_flags(1 << 19);
        // Normal mode: never.
        assert!(!routes_to_ime(false, false, 38, m));
        assert!(!routes_to_ime(false, true, 38, m));
        // Insert: letters and Option (dead keys) do, shortcuts and named keys
        // do not...
        assert!(routes_to_ime(true, false, 38, m));
        assert!(routes_to_ime(true, false, 14, alt));
        assert!(!routes_to_ime(true, false, 1, cmd));
        assert!(!routes_to_ime(true, false, 49, ctrl));
        assert!(!routes_to_ime(true, false, 36, m));
        assert!(!routes_to_ime(true, false, 53, m));
        // ...unless text is being composed: then the input method has all.
        assert!(routes_to_ime(true, true, 36, m));
        assert!(routes_to_ime(true, true, 53, m));
        assert!(routes_to_ime(true, true, 49, m));
    }

    #[test]
    fn utf16_offsets_become_byte_offsets() {
        assert_eq!(utf16_to_byte("abc", 2), 2);
        assert_eq!(utf16_to_byte("日本", 1), 3);
        assert_eq!(utf16_to_byte("a😀b", 3), 5);
        assert_eq!(utf16_to_byte("a😀b", 2), 5);
        assert_eq!(utf16_to_byte("ab", 9), 2);
        assert_eq!(utf16_to_byte("", 0), 0);
    }

    #[test]
    fn the_second_click_is_a_double_click() {
        assert_eq!(press_kind(1), MouseKind::Down);
        assert_eq!(press_kind(2), MouseKind::DoubleClick);
        // A third click starts over.
        assert_eq!(press_kind(3), MouseKind::Down);
        assert_eq!(press_kind(0), MouseKind::Down);
    }

    #[test]
    fn scroll_deltas_become_points_down_the_document() {
        // A trackpad's precise delta is points, up the document when
        // positive.
        assert_eq!(scroll_pt(3.5, true, 24.0), -3.5);
        assert_eq!(scroll_pt(-2.0, true, 24.0), 2.0);
        // A wheel's delta is lines.
        assert_eq!(scroll_pt(-1.0, false, 24.0), 24.0);
        assert_eq!(scroll_pt(2.0, false, 24.0), -48.0);
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

//! Turning GDK key presses, clicks and scrolls into editor input. The
//! mapping works on plain keysyms, modifier bits and numbers so it is
//! tested everywhere; the shell passes `gdk::Key`'s value and its Unicode
//! character, a press's count and a scroll's delta.

use editor_core::input::{Input, Key, KeyEvent, Mods, MouseKind};

/// X11 keysyms (`gdk/gdkkeysyms.h`), the values of `gdk::Key`.
pub mod keysym {
    pub const SPACE: u32 = 0x0020;
    pub const ISO_LEFT_TAB: u32 = 0xfe20;
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const RETURN: u32 = 0xff0d;
    pub const ESCAPE: u32 = 0xff1b;
    pub const HOME: u32 = 0xff50;
    pub const LEFT: u32 = 0xff51;
    pub const UP: u32 = 0xff52;
    pub const RIGHT: u32 = 0xff53;
    pub const DOWN: u32 = 0xff54;
    pub const PAGE_UP: u32 = 0xff55;
    pub const PAGE_DOWN: u32 = 0xff56;
    pub const END: u32 = 0xff57;
    pub const KP_SPACE: u32 = 0xff80;
    pub const KP_TAB: u32 = 0xff89;
    pub const KP_ENTER: u32 = 0xff8d;
    pub const KP_HOME: u32 = 0xff95;
    pub const KP_LEFT: u32 = 0xff96;
    pub const KP_UP: u32 = 0xff97;
    pub const KP_RIGHT: u32 = 0xff98;
    pub const KP_DOWN: u32 = 0xff99;
    pub const KP_PAGE_UP: u32 = 0xff9a;
    pub const KP_PAGE_DOWN: u32 = 0xff9b;
    pub const KP_END: u32 = 0xff9c;
    pub const KP_DELETE: u32 = 0xff9f;
    pub const DELETE: u32 = 0xffff;
}

/// `GdkModifierType` bits.
pub mod mask {
    pub const SHIFT: u32 = 1 << 0;
    pub const CONTROL: u32 = 1 << 2;
    pub const ALT: u32 = 1 << 3;
    pub const SUPER: u32 = 1 << 26;
}

/// Shift, Ctrl and Alt as named; Super stands in for Cmd.
pub fn mods_from_state(bits: u32) -> Mods {
    Mods {
        shift: bits & mask::SHIFT != 0,
        ctrl: bits & mask::CONTROL != 0,
        alt: bits & mask::ALT != 0,
        cmd: bits & mask::SUPER != 0,
    }
}

/// The key for one key press, as a table from the keysym to `Key`, else its
/// character: `keyval` is the keysym GDK chose for the layout and Shift
/// state, `ch` its character (Ctrl+C arrives as `c`).
pub fn map_key(keyval: u32, ch: Option<char>, mods: Mods, time_ns: u64) -> Option<KeyEvent> {
    use keysym as k;
    let named = match keyval {
        k::RETURN | k::KP_ENTER => Some(Key::Enter),
        k::TAB | k::ISO_LEFT_TAB | k::KP_TAB => Some(Key::Tab),
        k::SPACE | k::KP_SPACE => Some(Key::Space),
        k::BACKSPACE => Some(Key::Backspace),
        k::ESCAPE => Some(Key::Esc),
        k::DELETE | k::KP_DELETE => Some(Key::Delete),
        k::HOME | k::KP_HOME => Some(Key::Home),
        k::END | k::KP_END => Some(Key::End),
        k::PAGE_UP | k::KP_PAGE_UP => Some(Key::PageUp),
        k::PAGE_DOWN | k::KP_PAGE_DOWN => Some(Key::PageDown),
        k::LEFT | k::KP_LEFT => Some(Key::Left),
        k::RIGHT | k::KP_RIGHT => Some(Key::Right),
        k::UP | k::KP_UP => Some(Key::Up),
        k::DOWN | k::KP_DOWN => Some(Key::Down),
        _ => None,
    };
    let key = named.or_else(|| ch.filter(|c| !c.is_control()).map(Key::Char))?;
    Some(KeyEvent { key, mods, time_ns })
}

/// `map_key` as editor input.
pub fn map(keyval: u32, ch: Option<char>, mods: Mods, time_ns: u64) -> Option<Input> {
    map_key(keyval, ch, mods, time_ns).map(Input::Key)
}

/// The editor's kind for a `GtkGestureClick` press with `n_press`: the
/// second press of a double-click is `DoubleClick`; any other (a third
/// too) is `Down`.
pub fn press_kind(n_press: i32) -> MouseKind {
    if n_press == 2 {
        MouseKind::DoubleClick
    } else {
        MouseKind::Down
    }
}

/// Points down the document for a `GtkEventControllerScroll` delta `dy`
/// (positive down the document). A smooth (surface unit) delta is in
/// points already; a wheel's is in notches, each scrolling the page height
/// `page_pt` to the power 2/3, as `GtkScrolledWindow` does.
pub fn scroll_pt(dy: f64, wheel: bool, page_pt: f32) -> f32 {
    if wheel {
        dy as f32 * page_pt.max(1.0).powf(2.0 / 3.0)
    } else {
        dy as f32
    }
}

/// The byte offset of char `index` in `text` (GTK counts a preedit's cursor
/// in chars), clamped to its length.
pub fn char_to_byte(text: &str, index: i32) -> usize {
    let index = usize::try_from(index).unwrap_or(0);
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_press_is_a_double_click() {
        assert_eq!(press_kind(1), MouseKind::Down);
        assert_eq!(press_kind(2), MouseKind::DoubleClick);
        assert_eq!(press_kind(3), MouseKind::Down);
    }

    #[test]
    fn scroll_deltas_become_points_down_the_document() {
        // Smooth (surface) deltas are points already.
        assert_eq!(scroll_pt(3.5, false, 1000.0), 3.5);
        // A wheel notch scrolls the page height to the power 2/3, as a
        // GtkScrolledWindow does: 100 points for a 1000 point page.
        assert!((scroll_pt(1.0, true, 1000.0) - 100.0).abs() < 1e-3);
        assert!((scroll_pt(-2.0, true, 1000.0) + 200.0).abs() < 1e-3);
    }

    fn key(i: Option<Input>) -> Option<(Key, Mods)> {
        match i {
            Some(Input::Key(k)) => Some((k.key, k.mods)),
            _ => None,
        }
    }

    #[test]
    fn named_keys_and_their_keypad_twins() {
        let m = Mods::default();
        for (sym, want) in [
            (keysym::RETURN, Key::Enter),
            (keysym::KP_ENTER, Key::Enter),
            (keysym::BACKSPACE, Key::Backspace),
            (keysym::ESCAPE, Key::Esc),
            (keysym::DELETE, Key::Delete),
            (keysym::KP_DELETE, Key::Delete),
            (keysym::LEFT, Key::Left),
            (keysym::KP_UP, Key::Up),
            (keysym::PAGE_DOWN, Key::PageDown),
            (keysym::KP_HOME, Key::Home),
            (keysym::END, Key::End),
            (keysym::SPACE, Key::Space),
            (keysym::TAB, Key::Tab),
        ] {
            assert_eq!(key(map(sym, None, m, 0)), Some((want, m)), "{sym:#x}");
        }
        // Shift+Tab arrives as ISO_Left_Tab with Shift held.
        let shift = mods_from_state(mask::SHIFT);
        assert_eq!(
            key(map(keysym::ISO_LEFT_TAB, None, shift, 0)),
            Some((Key::Tab, shift))
        );
        // Space has a character too, but stays a named key.
        assert_eq!(
            key(map(keysym::SPACE, Some(' '), m, 0)),
            Some((Key::Space, m))
        );
    }

    #[test]
    fn key_table_gives_the_editors_key_events() {
        let ke = |key, mods| {
            Some(KeyEvent {
                key,
                mods,
                time_ns: 7,
            })
        };
        // `<C-Space>` and `<S-Tab>`, as editor-core's testkit sends them.
        let ctrl = mods_from_state(mask::CONTROL);
        assert_eq!(
            map_key(keysym::SPACE, Some(' '), ctrl, 7),
            ke(Key::Space, ctrl)
        );
        let shift = mods_from_state(mask::SHIFT);
        assert_eq!(
            map_key(keysym::ISO_LEFT_TAB, None, shift, 7),
            ke(Key::Tab, shift)
        );
        // Ctrl+S and Ctrl+Shift+R: the primary modifier on Linux.
        assert_eq!(
            map_key(u32::from('s'), Some('s'), ctrl, 7),
            ke(Key::Char('s'), ctrl)
        );
        let ctrl_shift = mods_from_state(mask::CONTROL | mask::SHIFT);
        assert_eq!(
            map_key(u32::from('R'), Some('R'), ctrl_shift, 7),
            ke(Key::Char('R'), ctrl_shift)
        );
        assert_eq!(
            map(keysym::ESCAPE, None, Mods::default(), 7),
            Some(Input::Key(KeyEvent {
                key: Key::Esc,
                mods: Mods::default(),
                time_ns: 7
            }))
        );
    }

    #[test]
    fn preedit_cursor_chars_become_bytes() {
        assert_eq!(char_to_byte("abc", 2), 2);
        assert_eq!(char_to_byte("日本", 1), 3);
        assert_eq!(char_to_byte("日本", 2), 6);
        assert_eq!(char_to_byte("日本", 9), 6);
        assert_eq!(char_to_byte("ab", -1), 0);
        assert_eq!(char_to_byte("", 0), 0);
    }

    #[test]
    fn modifier_bits() {
        let all = mods_from_state(mask::SHIFT | mask::CONTROL | mask::ALT | mask::SUPER);
        assert!(all.shift && all.ctrl && all.alt && all.cmd);
        let ctrl = mods_from_state(mask::CONTROL);
        assert_eq!(
            ctrl,
            Mods {
                ctrl: true,
                ..Mods::default()
            }
        );
        // Lock and button bits are ignored.
        assert_eq!(mods_from_state(1 << 1 | 1 << 8), Mods::default());
    }

    #[test]
    fn characters_and_shortcuts() {
        let shift = mods_from_state(mask::SHIFT);
        assert_eq!(
            key(map(u32::from('A'), Some('A'), shift, 0)),
            Some((Key::Char('A'), shift))
        );
        let ctrl = mods_from_state(mask::CONTROL);
        assert_eq!(
            key(map(u32::from('c'), Some('c'), ctrl, 0)),
            Some((Key::Char('c'), ctrl))
        );
        let ev = map(u32::from('x'), Some('x'), Mods::default(), 42);
        assert!(matches!(ev, Some(Input::Key(KeyEvent { time_ns: 42, .. }))));
        // Bare modifiers and control characters are not input.
        assert_eq!(map(0xffe1, None, shift, 0), None);
        assert_eq!(map(0x0000, Some('\u{7}'), Mods::default(), 0), None);
    }
}

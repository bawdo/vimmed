//! What a shell feeds the editor: keys, committed and composing text, the
//! mouse, scrolling, focus, ticks, appearance changes and close requests.

use crate::tokens::Theme;

/// A key press. `time_ns` is the event's timestamp on the `marks::now_ns`
/// clock, so key latency can be measured from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Mods,
    pub time_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
}

/// Modifier keys held during an event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub cmd: bool,
}

/// What a mouse event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Drag,
    Up,
}

/// A mouse event in window points. `clicks` counts a multi-click (1 for a
/// single click).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub x_pt: f32,
    pub y_pt: f32,
    pub clicks: u8,
    pub mods: Mods,
}

/// A process signal the shell forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Hangup,
    Interrupt,
    Terminate,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Key(KeyEvent),
    /// Text committed by the input method.
    Text(String),
    /// Text the input method is still composing; `cursor` is a byte offset
    /// into `text`.
    Preedit {
        text: String,
        cursor: usize,
    },
    Mouse(MouseEvent),
    Scroll {
        dy_pt: f32,
    },
    Focus(bool),
    /// The main loop woke (a timer, or a background worker called the
    /// waker).
    Tick {
        now_ns: u64,
    },
    Appearance(Theme),
    CloseRequested,
    Signal(SignalKind),
}

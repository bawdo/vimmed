//! What a shell feeds the editor: keys, committed and composing text, the
//! mouse, scrolling, focus, ticks, appearance changes and close requests.

use std::ops::Range;

use crate::buffer::Pos;
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

/// What a mouse event is. A double-click arrives as `DoubleClick` in
/// place of its second `Down`; the drag and `Up` after it follow as usual.
/// `Move` is the pointer moving with no button held: the shells send it
/// only while the table picker shows, for its hover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Drag,
    Up,
    DoubleClick,
    Move,
}

/// What a point in the document is over (`MouseHit::zone`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Zone {
    /// Text, or the space around it on a painted line.
    Text,
    /// A task item's checkbox, drawn in place of its hidden `- [ ]`.
    Prefix,
    /// An image line's picture.
    Image,
    /// A rendered link's text: the source bytes of the link-styled run
    /// under the point, on the hit's line.
    Link(Range<usize>),
    /// A cell of the table picker, `(col, row)` from 0, while it shows.
    Picker { col: u32, row: u32 },
}

/// Where a mouse event landed in the document, as the renderer's hit test
/// (`DocView::hit_test`) found it: the shell runs the test and hands the
/// result over, since the core has no geometry. Named apart from
/// `search::Hit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseHit {
    /// The caret position nearest the point: before a grapheme when the
    /// point is on its left half, after it on its right half. Where a
    /// click puts an Insert-mode caret.
    pub pos: Pos,
    /// The start of the grapheme under the point (`pos` past the end of
    /// the line's text). Where a click puts a block cursor, and the
    /// grapheme a double-click selects around.
    pub under: Pos,
    pub zone: Zone,
}

impl MouseHit {
    /// A hit on text at `pos`, with the grapheme under the point starting
    /// there too.
    pub fn text(pos: Pos) -> MouseHit {
        MouseHit {
            pos,
            under: pos,
            zone: Zone::Text,
        }
    }
}

/// A mouse event: what happened, where (`None` outside the text area or
/// over a line not painted yet) and the modifiers held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub hit: Option<MouseHit>,
    pub mods: Mods,
}

/// A process signal the shell forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Hangup,
    Interrupt,
    Terminate,
}

impl SignalKind {
    /// The POSIX signal number.
    pub fn number(self) -> i32 {
        match self {
            SignalKind::Hangup => 1,
            SignalKind::Interrupt => 2,
            SignalKind::Terminate => 15,
        }
    }

    /// The exit code after closing on this signal: 128 plus its number.
    /// The editor writes its recovery copy first, if the document is dirty.
    pub fn exit_code(self) -> i32 {
        128 + self.number()
    }
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
    /// The wheel or trackpad: `dy_pt` points down the document (negative
    /// up). A trackpad's smooth delta passes as it is; a wheel notch is
    /// converted to points as the platform does.
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

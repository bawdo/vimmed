//! Keystroke timing. Every time here is in seconds on the clock shared by
//! `NSEvent.timestamp`, `CADisplayLink` and `marks::now_ns` on macOS
//! (`CLOCK_UPTIME_RAW`).
//!
//! `key_ms` runs from the event's timestamp to the commit returning. Its
//! parts: `key_wait_ms` (timestamp to handler entry), `key_draw_ms`
//! (handler entry through staging), the wait for the display-link tick
//! (not logged), and `key_render_ms` (tick to commit returned).
//!
//! `key_glass_est_ms` is an estimate, not a measurement: the display link's
//! `targetTimestamp` at the committing tick (when the frame that carries the
//! commit is due on screen), plus one refresh for the compositor, minus the
//! event's timestamp. `docs/perf/macos-glass.md` describes calibrating it
//! with a 240 fps camera.
//!
//! Beside `key_draw_ms`, the same time is logged under
//! `key_draw_kind_ms.<input>.<redraw>`: what the key was (`input_kind`) and
//! what the editor asked to repaint for it (`redraw_kind`).

use editor_core::effects::Redraw;
use editor_core::input::{Input, Key};
use marks::names;

/// One key, from its event to the end of staging, and what kind it was.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyTimes {
    pub event_s: f64,
    pub handler_s: f64,
    pub staged_s: f64,
    /// From `names::input`.
    pub input: &'static str,
    /// From `names::redraw`.
    pub redraw: &'static str,
}

/// The kind of key `inp` is, from `names::input`. A printable key held
/// with Control or Command is `other` (a shortcut, not text).
pub fn input_kind(inp: &Input) -> &'static str {
    use names::input;
    match inp {
        Input::Text(_) => input::TEXT,
        Input::Key(k) => match k.key {
            Key::Char(_) | Key::Space if !k.mods.ctrl && !k.mods.cmd => input::TEXT,
            Key::Enter => input::ENTER,
            Key::Backspace => input::BACKSPACE,
            Key::Delete => input::DELETE,
            Key::Left | Key::Right | Key::Up | Key::Down => input::ARROW,
            _ => input::OTHER,
        },
        _ => input::OTHER,
    }
}

/// The kind of repaint `r` asks for, from `names::redraw`.
pub fn redraw_kind(r: &Redraw) -> &'static str {
    match r {
        Redraw::None => names::redraw::NONE,
        Redraw::Status => names::redraw::STATUS,
        Redraw::Lines(_) => names::redraw::LINES,
        Redraw::All => names::redraw::ALL,
    }
}

/// `key_draw_ms` again, under the name for this key's kinds.
pub fn kind_value(k: &KeyTimes) -> (String, f64) {
    (
        marks::key_draw_kind(k.input, k.redraw),
        (k.staged_s - k.handler_s) * 1000.0,
    )
}

/// The display-link tick that committed one or more keys.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    pub tick_s: f64,
    pub done_s: f64,
    /// `CADisplayLink.targetTimestamp` at this tick.
    pub target_s: f64,
    /// One refresh period.
    pub period_s: f64,
}

/// The logged values for one key, in milliseconds, as (name, value).
pub fn key_values(k: &KeyTimes, t: &Tick) -> [(&'static str, f64); 5] {
    let ms = |s: f64| s * 1000.0;
    [
        (names::KEY_MS, ms(t.done_s - k.event_s)),
        (names::KEY_WAIT_MS, ms(k.handler_s - k.event_s)),
        (names::KEY_DRAW_MS, ms(k.staged_s - k.handler_s)),
        (names::KEY_RENDER_MS, ms(t.done_s - t.tick_s)),
        (
            names::KEY_GLASS_EST_MS,
            ms(t.target_s + t.period_s - k.event_s),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::input::{Key, KeyEvent, Mods};

    #[test]
    fn splits_add_up_and_glass_adds_a_refresh() {
        let k = KeyTimes {
            event_s: 10.000,
            handler_s: 10.001,
            staged_s: 10.003,
            input: names::input::TEXT,
            redraw: names::redraw::LINES,
        };
        let t = Tick {
            tick_s: 10.008,
            done_s: 10.009,
            target_s: 10.016,
            period_s: 0.008,
        };
        let v = key_values(&k, &t);
        let get = |n: &str| v.iter().find(|(name, _)| *name == n).unwrap().1;
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(close(get(names::KEY_MS), 9.0));
        assert!(close(get(names::KEY_WAIT_MS), 1.0));
        assert!(close(get(names::KEY_DRAW_MS), 2.0));
        assert!(close(get(names::KEY_RENDER_MS), 1.0));
        assert!(close(get(names::KEY_GLASS_EST_MS), 24.0));
        let (name, draw) = kind_value(&k);
        assert_eq!(name, "key_draw_kind_ms.text.lines");
        assert!(close(draw, get(names::KEY_DRAW_MS)), "the same draw time");
    }

    #[test]
    fn key_kinds_name_the_input_and_redraw() {
        let key = |key, mods| {
            Input::Key(KeyEvent {
                key,
                mods,
                time_ns: 0,
            })
        };
        let plain = Mods::default();
        let cmd = Mods {
            cmd: true,
            ..Mods::default()
        };
        assert_eq!(input_kind(&key(Key::Char('a'), plain)), "text");
        assert_eq!(input_kind(&key(Key::Space, plain)), "text");
        assert_eq!(input_kind(&Input::Text("é".into())), "text");
        assert_eq!(input_kind(&key(Key::Char('s'), cmd)), "other");
        assert_eq!(input_kind(&key(Key::Enter, plain)), "enter");
        assert_eq!(input_kind(&key(Key::Backspace, plain)), "backspace");
        assert_eq!(input_kind(&key(Key::Delete, plain)), "delete");
        for k in [Key::Left, Key::Right, Key::Up, Key::Down] {
            assert_eq!(input_kind(&key(k, plain)), "arrow");
        }
        assert_eq!(input_kind(&key(Key::Esc, plain)), "other");
        assert_eq!(redraw_kind(&Redraw::None), "none");
        assert_eq!(redraw_kind(&Redraw::Status), "status");
        assert_eq!(redraw_kind(&Redraw::Lines(2..4)), "lines");
        assert_eq!(redraw_kind(&Redraw::All), "all");
    }
}

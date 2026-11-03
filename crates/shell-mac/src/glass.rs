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

use marks::names;

/// One key, from its event to the end of staging.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyTimes {
    pub event_s: f64,
    pub handler_s: f64,
    pub staged_s: f64,
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

    #[test]
    fn splits_add_up_and_glass_adds_a_refresh() {
        let k = KeyTimes {
            event_s: 10.000,
            handler_s: 10.001,
            staged_s: 10.003,
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
    }
}

//! Start-up and keystroke timing, as pure functions of timestamps on the
//! `marks::now_ns` clock (`CLOCK_MONOTONIC`, which is also GLib's monotonic
//! clock and so the base of `GdkFrameTimings`).
//!
//! A key is split as in the spike: `key_wait_ms` (handler entry to snapshot
//! start), `key_draw_ms` (the snapshot), `key_render_ms` (snapshot end to the
//! frame clock's `after-paint`) and `key_glass_ms` (event to the frame's
//! presentation time). `key_ms` runs from the event to `after-paint`.
//!
//! GDK event times are 32-bit milliseconds. Compositors stamp them from
//! `CLOCK_MONOTONIC`, but a synthetic source (wtype) may not: when the event
//! time is implausibly far from the handler, timing starts at the handler
//! instead and `key_time_fallback` is logged.

use marks::names;

/// Logged (value 1) for a key whose event time could not be trusted.
pub const KEY_TIME_FALLBACK: &str = "key_time_fallback";
/// The largest believable gap from a key event to its handler.
pub const MAX_EVENT_SKEW_MS: u64 = 1_000;

const MS: u64 = 1_000_000;

/// The event's time on the `now_ns` clock, from GDK's 32-bit millisecond
/// timestamp and the handler's `now_ns`. `None` when the two disagree by
/// more than `MAX_EVENT_SKEW_MS` (or the event claims to be in the future).
pub fn event_ns(event_ms: u32, handler_ns: u64) -> Option<u64> {
    let handler_ms = handler_ns / MS;
    // GDK's time is the clock's milliseconds truncated to 32 bits.
    let skew = u64::from((handler_ms as u32).wrapping_sub(event_ms));
    (skew <= MAX_EVENT_SKEW_MS)
        .then(|| handler_ms.checked_sub(skew))
        .flatten()
        .map(|ms| ms * MS)
}

/// One key, from its event to the end of the snapshot that drew it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyTimes {
    /// Where `key_ms` starts: the event, or the handler on fallback.
    pub start_ns: u64,
    pub fallback: bool,
    pub handler_ns: u64,
    /// When the handler returned (after `queue_draw`).
    pub handled_ns: u64,
    pub snap_start_ns: u64,
    pub snap_end_ns: u64,
}

impl KeyTimes {
    /// A key handled at `handler_ns`, whose event carried `event_ms`.
    pub fn new(event_ms: u32, handler_ns: u64) -> KeyTimes {
        let event = event_ns(event_ms, handler_ns);
        KeyTimes {
            start_ns: event.unwrap_or(handler_ns),
            fallback: event.is_none(),
            handler_ns,
            ..KeyTimes::default()
        }
    }
}

/// The values logged for one key at `after-paint`, in milliseconds.
pub fn key_values(k: &KeyTimes, after_paint_ns: u64) -> Vec<(&'static str, f64)> {
    let ms = |from: u64, to: u64| to.saturating_sub(from) as f64 / MS as f64;
    let mut v = vec![
        (names::KEY_MS, ms(k.start_ns, after_paint_ns)),
        (names::KEY_HANDLER_MS, ms(k.handler_ns, k.handled_ns)),
        (names::KEY_WAIT_MS, ms(k.handler_ns, k.snap_start_ns)),
        (names::KEY_DRAW_MS, ms(k.snap_start_ns, k.snap_end_ns)),
        (names::KEY_RENDER_MS, ms(k.snap_end_ns, after_paint_ns)),
    ];
    if k.fallback {
        v.push((KEY_TIME_FALLBACK, 1.0));
    }
    v
}

/// `key_glass_ms` for a key whose frame was presented at
/// `presentation_us` (GLib monotonic microseconds). 0 means the compositor
/// gave no presentation time.
pub fn glass_ms(start_ns: u64, presentation_us: i64) -> Option<f64> {
    let shown_ns = u64::try_from(presentation_us).ok().filter(|&us| us > 0)? * 1_000;
    Some((shown_ns as f64 - start_ns as f64) / MS as f64)
}

/// `refresh_hz` from `gdk::Monitor::refresh_rate` (millihertz); `None` when
/// the monitor doesn't know.
pub fn refresh_hz(millihertz: i32) -> Option<f64> {
    (millihertz > 0).then(|| f64::from(millihertz) / 1000.0)
}

/// When `first_paint` and `first_frame` are marked. `first_paint` is the
/// first snapshot of the canvas; `first_frame` is the first `after-paint`
/// of the frame clock that follows it (the frame that carries that
/// snapshot). GTK has no key-window condition, so these are the only inputs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FirstFrame {
    snapshotted: bool,
    marked: bool,
}

impl FirstFrame {
    /// The canvas was snapshotted. True exactly once: mark `first_paint`.
    pub fn snapshot(&mut self) -> bool {
        !std::mem::replace(&mut self.snapshotted, true)
    }

    /// The frame clock emitted `after-paint`. True exactly once, on the
    /// first one after a snapshot: mark `first_frame`.
    pub fn after_paint(&mut self) -> bool {
        if !self.snapshotted || self.marked {
            return false;
        }
        self.marked = true;
        true
    }

    pub fn is_marked(&self) -> bool {
        self.marked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(v: &[(&str, f64)], n: &str) -> Option<f64> {
        v.iter().find(|(name, _)| *name == n).map(|p| p.1)
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-6)
    }

    #[test]
    fn event_time_is_rebuilt_from_its_low_32_bits() {
        let handler = 5_000_000_123_456_789_u64;
        let handler_ms = handler / MS;
        let event_ms = (handler_ms - 3) as u32;
        assert_eq!(event_ns(event_ms, handler), Some((handler_ms - 3) * MS));
        // Across the 32-bit wrap.
        let wrap = (u64::from(u32::MAX) + 2) * MS + 500;
        assert_eq!(event_ns(u32::MAX, wrap), Some((u64::from(u32::MAX)) * MS));
    }

    #[test]
    fn implausible_event_times_are_refused() {
        let handler = 9_000_000 * MS;
        assert_eq!(event_ns(0, handler), None, "wtype sends 0");
        let future = ((handler / MS) + 5) as u32;
        assert_eq!(event_ns(future, handler), None);
        let late = ((handler / MS) - MAX_EVENT_SKEW_MS - 1) as u32;
        assert_eq!(event_ns(late, handler), None);
    }

    #[test]
    fn key_split_adds_up() {
        let handler = 100 * MS;
        let mut k = KeyTimes::new((handler / MS - 2) as u32, handler);
        assert!(!k.fallback);
        assert_eq!(k.start_ns, 98 * MS);
        k.handled_ns = 101 * MS;
        k.snap_start_ns = 104 * MS;
        k.snap_end_ns = 106 * MS;
        let v = key_values(&k, 110 * MS);
        assert!(close(get(&v, names::KEY_MS), 12.0));
        assert!(close(get(&v, names::KEY_HANDLER_MS), 1.0));
        assert!(close(get(&v, names::KEY_WAIT_MS), 4.0));
        assert!(close(get(&v, names::KEY_DRAW_MS), 2.0));
        assert!(close(get(&v, names::KEY_RENDER_MS), 4.0));
        assert_eq!(get(&v, KEY_TIME_FALLBACK), None);
    }

    #[test]
    fn untrusted_event_time_falls_back_to_the_handler() {
        let handler = 5_000 * MS;
        let k = KeyTimes {
            snap_start_ns: 5_001 * MS,
            snap_end_ns: 5_002 * MS,
            handled_ns: 5_000 * MS,
            ..KeyTimes::new(0, handler)
        };
        assert!(k.fallback);
        assert_eq!(k.start_ns, handler);
        let v = key_values(&k, 5_005 * MS);
        assert!(close(get(&v, names::KEY_MS), 5.0));
        assert!(close(get(&v, KEY_TIME_FALLBACK), 1.0));
    }

    #[test]
    fn glass_needs_a_presentation_time() {
        assert_eq!(glass_ms(10 * MS, 0), None);
        let g = glass_ms(10 * MS, 26_000);
        assert!(close(g, 16.0));
    }

    #[test]
    fn refresh_is_millihertz() {
        assert_eq!(refresh_hz(60_000), Some(60.0));
        assert_eq!(
            refresh_hz(143_856).map(|h| (h * 1000.0).round()),
            Some(143_856.0)
        );
        assert_eq!(refresh_hz(0), None);
    }

    #[test]
    fn first_frame_waits_for_a_snapshot() {
        let mut f = FirstFrame::default();
        // Frames before the canvas drew anything don't count.
        assert!(!f.after_paint());
        assert!(f.snapshot(), "first_paint");
        assert!(!f.snapshot(), "first_paint once");
        assert!(f.after_paint(), "first_frame");
        assert!(f.is_marked());
        assert!(!f.after_paint(), "first_frame once");
        assert!(!f.snapshot());
    }
}

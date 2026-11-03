//! The window's side of the bench protocol: what `<P>BENCH` asks for, and
//! when a benched window exits on its own.
//!
//! The bench can only kill the launcher, and the window runs in its own
//! session, so a benched window must never wait for a person to close it.
//! It exits at `BenchPlan::deadline` with `BenchPlan::exit_code`: 0, except
//! after the startup cap (no `first_frame` at all), which exits
//! `EXIT_NO_FIRST_FRAME` so the bench reports the run as failed.
//!
//! - `start`: `IMAGE_SETTLE` after the first decoded image is on screen, so
//!   `images_painted` is logged. With no image on screen, `START_LINGER`
//!   after `first_frame`.
//! - `type-<source>:N`: `KEY_SETTLE` after the Nth key is committed, so its
//!   timings are logged.
//! - Always, as a backstop: a cap counted from `first_frame` (or from
//!   process start when no frame ever reaches the screen). `manual` gets
//!   `MANUAL_CAP`; the others get the bench's own run timeout,
//!   `AUTO_CAP_BASE` plus `AUTO_CAP_PER_KEY` per key.
//!
//! The shell re-reads the deadline whenever progress changes and checks it
//! from a timer, so nothing here touches a clock.

/// How the keys of a `type` run are produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Manual,
    Wtype,
    Synthetic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchPlan {
    Start,
    Type { source: KeySource, keys: usize },
}

/// What the window has done so far, as `now_ns` timestamps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub start_ns: u64,
    pub first_frame_ns: Option<u64>,
    /// When the first decoded image was committed to the screen.
    pub image_shown_ns: Option<u64>,
    pub keys_done: usize,
    /// When the latest key was committed.
    pub last_key_ns: Option<u64>,
}

const MS: u64 = 1_000_000;
const S: u64 = 1_000 * MS;

pub const IMAGE_SETTLE: u64 = 150 * MS;
pub const START_LINGER: u64 = 2 * S;
pub const KEY_SETTLE: u64 = 250 * MS;
/// Under the bench's own 30 s start timeout, so the window leaves first.
pub const STARTUP_CAP: u64 = 20 * S;
pub const MANUAL_CAP: u64 = 600 * S;
pub const AUTO_CAP_BASE: u64 = 30 * S;
pub const AUTO_CAP_PER_KEY: u64 = 50 * MS;

/// Exit code when a benched window reaches the startup cap without ever
/// marking `first_frame`. Chosen to sit beside the launcher's 220 (window
/// not started) and the window's 221 (protocol mismatch), clear of 0, 1
/// (`:cq`), 2 (usage) and 128 + n (signals).
pub const EXIT_NO_FIRST_FRAME: i32 = 222;

impl BenchPlan {
    /// Parses a `<P>BENCH` value: `start`, or `type-manual:N`,
    /// `type-wtype:N`, `type-synthetic:N`.
    pub fn parse(s: &str) -> Option<BenchPlan> {
        if s == "start" {
            return Some(BenchPlan::Start);
        }
        let (source, keys) = s.strip_prefix("type-")?.split_once(':')?;
        let source = match source {
            "manual" => KeySource::Manual,
            "wtype" => KeySource::Wtype,
            "synthetic" => KeySource::Synthetic,
            _ => return None,
        };
        Some(BenchPlan::Type {
            source,
            keys: keys.parse().ok()?,
        })
    }

    /// The plan in `<P>BENCH`, if it is set and valid.
    pub fn from_env() -> Option<BenchPlan> {
        let v = std::env::var(format!("{}BENCH", app_meta::ENV_PREFIX)).ok()?;
        BenchPlan::parse(&v)
    }

    /// The keys a `type` run expects; 0 for `start`.
    pub fn keys(self) -> usize {
        match self {
            BenchPlan::Start => 0,
            BenchPlan::Type { keys, .. } => keys,
        }
    }

    /// The code to exit with at the deadline.
    pub fn exit_code(self, p: &Progress) -> i32 {
        let _ = self;
        if p.first_frame_ns.is_some() {
            0
        } else {
            EXIT_NO_FIRST_FRAME
        }
    }

    /// When the window should exit, in `now_ns` nanoseconds.
    pub fn deadline(self, p: &Progress) -> u64 {
        let Some(ff) = p.first_frame_ns else {
            return p.start_ns + STARTUP_CAP;
        };
        let cap = ff
            + match self {
                BenchPlan::Start => START_LINGER,
                BenchPlan::Type {
                    source: KeySource::Manual,
                    ..
                } => MANUAL_CAP,
                BenchPlan::Type { keys, .. } => AUTO_CAP_BASE + AUTO_CAP_PER_KEY * keys as u64,
            };
        let done = match self {
            BenchPlan::Start => p.image_shown_ns.map(|t| t + IMAGE_SETTLE),
            BenchPlan::Type { keys, .. } if p.keys_done >= keys => {
                Some(p.last_key_ns.unwrap_or(ff) + KEY_SETTLE)
            }
            BenchPlan::Type { .. } => None,
        };
        done.map_or(cap, |d| d.min(cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_mode() {
        assert_eq!(BenchPlan::parse("start"), Some(BenchPlan::Start));
        assert_eq!(
            BenchPlan::parse("type-wtype:300"),
            Some(BenchPlan::Type {
                source: KeySource::Wtype,
                keys: 300
            })
        );
        assert_eq!(
            BenchPlan::parse("type-manual:500").map(BenchPlan::keys),
            Some(500)
        );
        assert!(matches!(
            BenchPlan::parse("type-synthetic:2"),
            Some(BenchPlan::Type {
                source: KeySource::Synthetic,
                ..
            })
        ));
        assert_eq!(BenchPlan::parse(""), None);
        assert_eq!(BenchPlan::parse("type-wtype"), None);
        assert_eq!(BenchPlan::parse("type-other:3"), None);
        assert_eq!(BenchPlan::parse("type-wtype:x"), None);
    }

    #[test]
    fn no_frame_exits_at_the_startup_cap() {
        let p = Progress {
            start_ns: 7,
            ..Progress::default()
        };
        assert_eq!(BenchPlan::Start.deadline(&p), 7 + STARTUP_CAP);
    }

    #[test]
    fn exit_is_non_zero_only_without_a_first_frame() {
        let mut p = Progress::default();
        assert_eq!(BenchPlan::Start.exit_code(&p), EXIT_NO_FIRST_FRAME);
        let plan = BenchPlan::Type {
            source: KeySource::Wtype,
            keys: 1,
        };
        assert_eq!(plan.exit_code(&p), EXIT_NO_FIRST_FRAME);
        p.first_frame_ns = Some(1);
        assert_eq!(BenchPlan::Start.exit_code(&p), 0);
        assert_eq!(plan.exit_code(&p), 0);
    }

    #[test]
    fn start_waits_for_an_image_then_settles() {
        let mut p = Progress {
            first_frame_ns: Some(100),
            ..Progress::default()
        };
        assert_eq!(BenchPlan::Start.deadline(&p), 100 + START_LINGER);
        p.image_shown_ns = Some(300);
        assert_eq!(BenchPlan::Start.deadline(&p), 300 + IMAGE_SETTLE);
    }

    #[test]
    fn type_exits_after_the_last_key_or_at_the_cap() {
        let plan = BenchPlan::Type {
            source: KeySource::Wtype,
            keys: 3,
        };
        let mut p = Progress {
            first_frame_ns: Some(0),
            keys_done: 2,
            last_key_ns: Some(50),
            ..Progress::default()
        };
        assert_eq!(plan.deadline(&p), AUTO_CAP_BASE + 3 * AUTO_CAP_PER_KEY);
        p.keys_done = 3;
        p.last_key_ns = Some(90);
        assert_eq!(plan.deadline(&p), 90 + KEY_SETTLE);
        let manual = BenchPlan::Type {
            source: KeySource::Manual,
            keys: 3,
        };
        p.keys_done = 0;
        assert_eq!(manual.deadline(&p), MANUAL_CAP);
    }
}

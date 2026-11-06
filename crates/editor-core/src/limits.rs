//! Product budgets: the ceilings that keep one key, or one document, from
//! costing unbounded time or memory.
//!
//! Each constant has a normal value and a lower one under `cfg(fuzzing)` (set
//! by cargo-fuzz and ClusterFuzzLite). Fuzz builds lower the numbers only;
//! the same limit-handling code runs in both.

/// The largest value a count part may reach. Matches vim's own ceiling, so a
/// count never means more than a billion operations. Fuzz builds use 1,000 so
/// a few input bytes cannot mean tens of millions of operations.
#[cfg(not(fuzzing))]
pub const MAX_COUNT: usize = 999_999_999;
#[cfg(fuzzing)]
pub const MAX_COUNT: usize = 1_000;

/// The most lines a document may hold after an edit grows it. Bounds the
/// per-line bookkeeping (line index, render rows) a single edit can force.
#[cfg(not(fuzzing))]
pub const MAX_DOC_LINES: usize = 1 << 22; // 4,194,304
#[cfg(fuzzing)]
pub const MAX_DOC_LINES: usize = 1 << 16; // 65,536

/// The most bytes a document may hold after an edit grows it (256 MiB
/// normally). Bounds memory so a repeated put or substitute cannot exhaust
/// it. Loading is never refused, only growth.
#[cfg(not(fuzzing))]
pub const MAX_DOC_BYTES: usize = 1 << 28; // 268,435,456
#[cfg(fuzzing)]
pub const MAX_DOC_BYTES: usize = 1 << 20; // 1,048,576

/// The most bytes of text the undo history may retain (64 MiB normally).
/// Bounds memory held by saved edits, alongside the group-count limit.
#[cfg(not(fuzzing))]
pub const MAX_UNDO_BYTES: usize = 1 << 26; // 67,108,864
#[cfg(fuzzing)]
pub const MAX_UNDO_BYTES: usize = 1 << 20; // 1,048,576

/// The most changes one `:s` command may make. Bounds the work and the undo
/// record of a single substitution over a large range.
#[cfg(not(fuzzing))]
pub const MAX_SUB_CHANGES: usize = 1 << 18; // 262,144
#[cfg(fuzzing)]
pub const MAX_SUB_CHANGES: usize = 1 << 12; // 4,096

# Layout benchmark: cosmic-text against parley (November 2026)

This settles the open item from the stack design ("Text layout: cosmic-text vs parley"). The
stack design chose cosmic-text behind a thin internal layout trait, subject to this head-to-head.
Task 11's `TextLayout` trait is fixed by the verdict below.

## Verdict

**cosmic-text, with its `shape-run-cache` feature enabled. Owner decision, 2026-11-03.**

Reasoning:

- **On representative prose, parley's lead sits at the bar, not clearly over it.** The rule needs
  parley's per-keystroke p95 to be at least 25% lower. Typing ordinary English prose, parley was
  20.8%, 24.4% and 23.7% lower in the three recorded runs (and 24.2%, 23.5% and 25.3% in an
  earlier set of three with the same workload). That is a coin toss around 25%, not a win.
- **The first pass overstated the lead.** It typed a pangram, whose short words (3.9 letters on
  average against 5.3 for the prose) mean more words per character. cosmic-text shapes word by
  word, so short words cost it more. The pangram is kept as a secondary row: parley leads by 33%
  to 36% there.
- **cosmic-text's stock cache reverses the result.** With the opt-in `shape-run-cache` feature,
  trimmed once per keystroke inside the timed region, cosmic-text's prose p95 is about 29 µs
  against parley's 70 to 74 µs: parley is about 2.4 times slower. The layout output is identical
  with and without the cache (`--compare` gives the same result for both builds).
- **The absolute cost is negligible either way.** The worst keystroke relayout in any run, either
  library, either configuration, was 0.13 ms, against a 16.7 ms frame at 60 Hz (8.3 ms at
  120 Hz).
- **Cold construction doesn't decide anything.** Both are 39 to 58 µs at p50 over 30 processes
  and their ranges overlap heavily. parley's p50 was 11 to 15 µs lower in every run here, so the
  rule's "not worse" condition held for parley on the Mac; it was the keystroke condition that
  fell short. One independent reviewer run reported the opposite ordering, which is consistent
  with this being noise at the scale of microseconds.
- Nothing measured here outweighs the stack design's reasons, so its choice stands.

Where parley is ahead, for the record: the cold first screen (about 0.45 ms against 0.62 to
0.67 ms, a fresh process with nothing cached) and, without the cache, the warm screen. Both are
fractions of a millisecond.

## Configuration for Task 11

- `cosmic-text = { version = "0.19", features = ["shape-run-cache"] }` in `editor-render`, with the
  default features kept.
- Trim the cache once per painted frame with `font_system.shape_run_cache.trim(1)` (keep runs used
  in the current or previous frame). The bench trims once per keystroke and once per full screen,
  inside the timed region, and its numbers include that cost.
- One `Buffer` per source line, `Wrap::WordOrGlyph`, `Shaping::Advanced`, fonts loaded by path
  into a fontdb `Database` passed to `FontSystem::new_with_locale_and_db` (no system scan). This
  is exactly what the bench measured.
- The cache is keyed on run text and attributes, so it never changes layout; it only skips
  reshaping runs that were shaped in the last frame or two.

## Trait shape for Task 11

As in Task 11's Interfaces, plus one per-frame hook for the cache:

```rust
pub trait TextLayout {
    fn layout_line(&mut self, text: &str, spans: &[StyledSpan], width_px: f32) -> LaidLine;
    fn draw(&mut self, line: &LaidLine, sink: &mut dyn FnMut(GlyphMask));
    /// Called once after each painted frame. The cosmic-text implementation trims its
    /// shape-run cache here; other implementations may do nothing.
    fn end_frame(&mut self) {}
}

pub struct StyledSpan {
    pub range: Range<usize>,
    pub role: FontRole, // Body | Mono
    pub size_px: f32,
    pub weight: u16,
    pub italic: bool,
    pub color: Rgba,
    pub strike: bool,
}

pub struct LaidLine { pub height: f32, pub rows: u32, /* .. */ }
// LaidLine::caret(byte) -> Caret { x, top, height }, LaidLine::hit(x, y) -> usize
```

`DocView` calls `end_frame` once after `paint_all` or `paint_lines`. `FontSet::load(paths)` owns
the `FontSystem`; the custom cosmic-text `Fallback` from the stack design plugs in at
construction. The bench's own `Engine` interface (construct, lay out one line into a slot, per
frame housekeeping) mapped onto both libraries without either needing a different call shape,
so the trait stays library-neutral if the choice is ever revisited.

## Versions

| Item | Value |
|---|---|
| cosmic-text | 0.19.0 (the version in the plan's tech stack; it resolved) |
| parley | 0.12.0 (the current release; pulls fontique 0.12, parley_engine 0.12, HarfRust 0.12) |
| cosmic-text's shaper | HarfRust 0.5.2 |
| Rust | 1.99.1, release profile (`opt-level = 3`, fat LTO, `codegen-units = 1`) |
| Machine | Apple silicon (aarch64), macOS 27.2 (build 26B123). The exact chip could not be read from the sandbox (`sysctl` is blocked). |

## How to run

```sh
cargo xtask fixture                      # writes target/fixture/test.md (1,000 lines)
cargo run -p layout-bench --release -- target/fixture/test.md                        # cosmic-text as shipped
cargo run -p layout-bench --release --features run-cache -- target/fixture/test.md   # with shape-run-cache
```

`layout-bench` is a workspace member but is not in `default-members`, so a plain
`cargo build --release` (the shipping build of the launcher and window binaries) never builds it.
`cargo clippy --workspace` does include it.

A diagnostic mode checks that both libraries really did the same work:

```sh
target/release/layout-bench --compare target/fixture/test.md [sans font path]
```

It prints every screen line and every typed keystroke (prose and pangram) where the two
libraries disagree on visual rows or height, with each row's byte range and width, and a count
per typing source.

## Fair-comparison rules

- **Same font files.** Only the platform sans and mono faces are loaded, by path, into both
  libraries; neither scans system fonts. macOS: `/System/Library/Fonts/SFNS.ttf` and
  `/System/Library/Fonts/SFNSMono.ttf`. Both libraries see the files memory-mapped (fontdb's
  `Source::File` for cosmic-text; the bench maps the file with memmap2 and hands parley a `Blob`).
  parley's collection is built with `system_fonts: false`.
- **Same fallback set** for the fallback test only: Apple Symbols, Apple Color Emoji and Menlo on
  macOS (hard-coded in the bench behind `cfg(target_os = "macos")`); on Linux the files from
  `fc-match -f '%{file}'` for `emoji` and `symbol` (sans and mono come from `sans-serif` and
  `monospace`), resolved once at bench start, outside every timed region. cosmic-text falls back
  through every loaded face on its own; parley has no implicit "any loaded face" fallback, so the
  same files are appended to each family stack in the same order.
- **Same column and scale.** 660 pt column at scale 2, so 1320 px. Body 16 pt, so 32 px. Line
  height 1.5 for body and 1.4 for code (the token defaults). Scale is applied by the bench to
  both libraries' sizes (parley's own scale argument is 1.0).
- **Same text and the same styled spans.** One layout object per source line, as Task 11 lays
  text out. A small bench-only classifier gives every line the same spans for both libraries:
  fenced code is mono at 1.4 line height, `#` headings are bold and larger, `**` toggles bold
  and backticks toggle mono. Markers stay in the text. Italic is not styled, because only the
  regular faces are loaded and synthetic italic differs between the libraries. Spans are
  computed outside every timed region.
- **Same wrapping.** cosmic-text `Wrap::WordOrGlyph`; parley `OverflowWrap::Anywhere`. Both break
  at words, and inside a word only when it can't fit.
- **Same reuse.** cosmic-text keeps one `Buffer` per line slot and calls `set_rich_text` then
  `shape_until_scroll`; parley keeps one `LayoutContext` and one `Layout` per slot and calls
  `build_into`, `break_all_lines` and `align`. Without the cache feature nothing is cached across
  calls: every timed call reshapes and re-wraps. With it, cosmic-text reuses runs shaped in the
  previous frame or two, and pays for the per-frame trim inside the timed region.
- **Release build**, the workspace release profile above, one binary for both libraries.
- **Warm-up is excluded.** Before the warm and keystroke measurements, each library shapes the
  first screen 20 times and types 50 characters of each typing source into throwaway copies of
  the line.
- **Interleaving.** Cold child processes alternate cosmic-text, parley, cosmic-text, and so on.
  The warm screen and each keystroke measurement run in 5 alternating rounds, so drift in machine
  load hits both libraries alike.
- **Percentiles are nearest rank.** Sort the samples ascending; the p-th percentile is the sample
  at rank `ceil(p / 100 * n)` (1-based). For 30 cold processes, p50 is the 15th smallest.

## What each measure is

1. **Construction, cold process.** The bench re-executes itself 30 times per library with a
   `--child <library>` flag. The child reads the fixture and computes spans first, then times only
   its own font system construction: for cosmic-text a fontdb `Database`, both faces loaded and
   `FontSystem::new_with_locale_and_db`; for parley a fontique `Collection`, both faces
   registered, the `FontContext` and the `LayoutContext`. Process start is never timed. The table
   shows min, p50 and max, so overlap is visible. The font files are in the OS page cache after
   the first run (purging it needs root), so "cold" means a fresh process, not a cold disk.
2. **First screen, cold.** In the same child, straight after construction: the first 60 source
   lines laid out for the first time (plus one cache trim with the feature on).
3. **First screen, warm.** The same 60 lines, 200 times in the parent after warm-up. With the
   cache this is a best case (an unchanged screen repainted), which is why it halves.
4. **Keystroke relayout.** Typing into source line 3, the first line with `**`, starting just
   after the closing `**`. Each keystroke inserts the next character of the typing source,
   recomputes the spans untimed, then times one relayout of that line (and the cache trim, with
   the feature on). 500 keystrokes; the line grows from 77 to 577 bytes (1 to 6 or 7 visual
   rows), so the percentiles mix short and long lines. Two typing sources:
   - **prose** (the representative case, and the one the rule is applied to): the committed
     passage `crates/layout-bench/src/prose.txt`, ordinary English prose written for the bench,
     average word length 5.3 letters;
   - **pangram** (secondary, for sensitivity): `the quick brown fox jumps over the lazy dog`,
     average word length 3.9 letters.
5. **Fallback quality.** `☐ ☑ ◦ → 😀 中文 ü é` laid out plainly in the sans, counting glyph id 0
   (`.notdef`), first with only the two faces, then with the fallback set added.

## Results: macOS (Apple silicon), release, all recorded runs

Rows that are the same in every run are given once at the end. "Lead" is how much lower
parley's p95 is than cosmic-text's (negative: parley is higher).

### cosmic-text as shipped (no cache), three runs

| Measure | Run | cosmic-text 0.19.0 | parley 0.12.0 |
|---|---|---|---|
| Construction, cold, min / p50 / max of 30 | 1 | 48.8 / 52.8 / 263.0 µs | 32.9 / 39.3 / 157.9 µs |
| | 2 | 52.8 / 56.5 / 108.0 µs | 38.5 / 41.3 / 445.3 µs |
| | 3 | 46.5 / 52.1 / 128.2 µs | 34.5 / 41.1 / 92.3 µs |
| First screen, cold, min / p50 / max of 30 | 1 | 0.58 / 0.64 / 2.07 ms | 0.41 / 0.45 / 1.18 ms |
| | 2 | 0.65 / 0.67 / 1.11 ms | 0.46 / 0.47 / 1.27 ms |
| | 3 | 0.61 / 0.64 / 1.22 ms | 0.42 / 0.44 / 0.80 ms |
| First screen, warm, p50 / p95 / p99 of 200 | 1 | 0.28 / 0.30 / 0.31 ms | 0.23 / 0.26 / 0.27 ms |
| | 2 | 0.28 / 0.30 / 0.33 ms | 0.24 / 0.26 / 0.30 ms |
| | 3 | 0.28 / 0.34 / 0.41 ms | 0.24 / 0.28 / 0.36 ms |
| Keystroke, prose, p50 / p95 / p99 of 500 | 1 | 55.3 / 90.5 / 94.5 µs | 44.5 / 71.7 / 73.1 µs |
| | 2 | 54.9 / 93.1 / 95.9 µs | 43.1 / 70.4 / 72.5 µs |
| | 3 | 56.8 / 92.2 / 99.4 µs | 45.5 / 70.4 / 72.8 µs |
| Keystroke, prose, max | 1 / 2 / 3 | 106.8 / 100.2 / 107.2 µs | 77.0 / 74.9 / 78.5 µs |
| Keystroke, pangram, p50 / p95 / p99 of 500 | 1 | 57.7 / 98.4 / 102.9 µs | 40.4 / 63.0 / 65.0 µs |
| | 2 | 58.8 / 97.4 / 102.9 µs | 39.8 / 65.5 / 68.9 µs |
| | 3 | 58.5 / 98.4 / 105.5 µs | 40.1 / 63.2 / 70.1 µs |
| **Prose p95 lead (rule: 25%)** | 1 / 2 / 3 | | **+20.8% / +24.4% / +23.7%** |
| Pangram p95 lead | 1 / 2 / 3 | | +36.0% / +32.7% / +35.8% |

An earlier set of three runs with the same workload (before the summary line's wording changed)
gave prose leads of +24.2%, +23.5% and +25.3%, and pangram leads of +33.2%, +34.5% and +35.3%.

### cosmic-text with `shape-run-cache` (`--features run-cache`), three runs

| Measure | Run | cosmic-text 0.19.0 + cache | parley 0.12.0 |
|---|---|---|---|
| Construction, cold, min / p50 / max of 30 | 1 | 45.2 / 52.8 / 247.3 µs | 34.4 / 40.9 / 150.5 µs |
| | 2 | 53.1 / 58.0 / 104.1 µs | 38.6 / 45.0 / 94.1 µs |
| | 3 | 47.2 / 55.0 / 166.0 µs | 34.9 / 40.8 / 102.9 µs |
| First screen, cold, min / p50 / max of 30 | 1 | 0.54 / 0.62 / 1.86 ms | 0.41 / 0.45 / 1.12 ms |
| | 2 | 0.62 / 0.64 / 1.12 ms | 0.44 / 0.47 / 0.72 ms |
| | 3 | 0.56 / 0.62 / 1.44 ms | 0.42 / 0.46 / 0.88 ms |
| First screen, warm, p50 / p95 / p99 of 200 | 1 | 0.12 / 0.13 / 0.14 ms | 0.24 / 0.25 / 0.26 ms |
| | 2 | 0.12 / 0.13 / 0.14 ms | 0.24 / 0.26 / 0.28 ms |
| | 3 | 0.12 / 0.13 / 0.13 ms | 0.24 / 0.25 / 0.26 ms |
| Keystroke, prose, p50 / p95 / p99 of 500 | 1 | 18.1 / 29.1 / 30.7 µs | 44.2 / 73.5 / 80.5 µs |
| | 2 | 18.4 / 29.2 / 32.6 µs | 43.7 / 70.9 / 76.0 µs |
| | 3 | 18.6 / 29.0 / 33.1 µs | 42.9 / 69.8 / 73.8 µs |
| Keystroke, prose, max | 1 / 2 / 3 | 34.3 / 41.8 / 48.0 µs | 121.8 / 79.0 / 77.4 µs |
| Keystroke, pangram, p50 / p95 / p99 of 500 | 1 | 18.7 / 31.2 / 32.7 µs | 40.6 / 63.6 / 67.1 µs |
| | 2 | 18.8 / 31.3 / 33.7 µs | 40.8 / 64.5 / 71.5 µs |
| | 3 | 17.8 / 30.8 / 32.1 µs | 40.3 / 62.3 / 68.6 µs |
| **Prose p95 lead** | 1 / 2 / 3 | | **-152.4% / -142.8% / -141.0%** |
| Pangram p95 lead | 1 / 2 / 3 | | -103.6% / -105.7% / -102.4% |

### Rows identical in every run, both builds

| Measure | cosmic-text | parley |
|---|---|---|
| First screen visual rows (cold / warm), total height px | 63 / 63, 3072 | 63 / 63, 3072 |
| Typed line visual rows after the last keystroke (prose / pangram) | 6 / 6 | 6 / 7 |
| `.notdef` glyphs, two faces only | 4 (☐ 😀 中 文) | 4 (☐ 😀 中 文) |
| `.notdef` glyphs, with fallback set | 2 (中 文) | 2 (中 文) |

Other builds were running on the machine during these runs; the interleaving is there so that
load lands on both libraries.

### Did both libraries do the same work?

- The first screen is identical: `--compare` reports no line where rows or height differ.
- The typed line differs by one visual row on 20 of 500 prose keystrokes and 34 of 500 pangram
  keystrokes, always with parley one row taller, near a wrap boundary. parley measures the same
  SF text about 3% wider. This is specific to SF: with
  `--compare ... /System/Library/Fonts/Supplemental/Verdana.ttf` every line and keystroke
  matched exactly (checked with the pangram workload). SF carries an AAT `trak` (tracking) table;
  parley passes the font size to HarfRust 0.12 as the point size, which applies tracking for it,
  while cosmic-text 0.19 shapes with HarfRust 0.5.2. The extra row means parley did slightly more
  work on those keystrokes, so it does not flatter parley. Which one matches Core Text at 16 pt,
  scale 2 is a fidelity question for later.
- `--compare` gives the same counts with and without `--features run-cache`: the cache changes
  speed, not output.
- Fallback: identical. ☐ and 😀 need the fallback set in both; ☑ ◦ → ü é are covered by SF.
  中文 stays missing in both, because the fixed set has no CJK face. That is the job of the
  on-demand `lookup` path (Task 42), not of the start-up set.

## History: the first pass

The first pass typed only the pangram and measured 10 cold processes. It found parley's
keystroke p95 33% to 35% lower (for example 63.3 µs against 95.1 µs) and reported it as a win
by the rule. A fairness review then found the work per call was equal but two input choices
drove the result: the pangram's short words, and leaving out cosmic-text's stock cache. This
version types representative prose (pangram kept as a secondary row), measures both cosmic-text
configurations, and uses 30 cold processes with min, p50 and max shown.

## Linux (pending owner run)

On the Linux laptop, from the repository root, three runs of each build:

```sh
cargo xtask fixture
cargo build -p layout-bench --release
target/release/layout-bench --compare target/fixture/test.md
for i in 1 2 3; do target/release/layout-bench target/fixture/test.md; done
cargo build -p layout-bench --release --features run-cache
target/release/layout-bench --compare target/fixture/test.md
for i in 1 2 3; do target/release/layout-bench target/fixture/test.md; done
```

The bench resolves the faces with `fc-match -f '%{file}'` for `sans-serif`, `monospace`, `emoji`
and `symbol` at start, outside every timed region, and prints the files it used.

**Check the same-work evidence before trusting the timings.** Linux fontconfig defaults are
often static fonts (for example DejaVu Sans), where the regular file has no weight axis. The
bench loads only that one file, so a bold span has no real bold face: one library may synthesise
emboldening and the other may fall back to the regular outlines, giving different advances and
wrap points. Compare the `--compare` output (the per-source "N of 500 keystrokes differ" counts,
and any `line` entries for the first screen) and the "visual rows" rows in the table between the
two libraries. If they differ by much more than the Mac's 20 and 34 keystrokes, or any screen
line differs, the libraries are doing different work and the timings need that caveat.

Paste the outputs here. If the Linux numbers would reverse the verdict, that goes to the owner.

Results: pending.

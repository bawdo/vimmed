# Layout benchmark: cosmic-text against parley (November 2026)

This settles the open item from the stack design ("Text layout: cosmic-text vs parley"). The
stack design chose cosmic-text behind a thin internal layout trait, subject to this head-to-head.
Task 11's `TextLayout` trait is fixed only after the verdict.

## Status

**Verdict: not recorded. On the Mac numbers parley wins by the decision rule, which would reverse
a stack-design choice, so the decision goes to the owner before Task 11.** The measurements, the
method and the rule's arithmetic are below. Nothing in this document commits the editor to
either library.

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
cargo run -p layout-bench --release -- target/fixture/test.md
```

`layout-bench` is a workspace member but is not in `default-members`, so a plain
`cargo build --release` (the shipping build of the launcher and window binaries) never builds it.
`cargo clippy --workspace` does include it.

A diagnostic mode checks that both libraries really did the same work:

```sh
target/release/layout-bench --compare target/fixture/test.md [sans font path]
```

It prints every screen line and every keystroke where the two libraries disagree on visual rows
or height, with each row's byte range and width.

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
  `build_into`, `break_all_lines` and `align`. Neither result is cached across iterations: every
  timed call reshapes and re-wraps.
- **Release build**, the workspace release profile above, one binary for both libraries.
- **Warm-up is excluded.** Before the warm and keystroke measurements, each library shapes the
  first screen 20 times and types 50 characters into a throwaway copy of the line.
- **Interleaving.** Cold child processes alternate cosmic-text, parley, cosmic-text, and so on.
  The warm screen and keystroke measurements run in 5 alternating rounds (40 screens or 100
  keystrokes per library per round), so drift in machine load hits both libraries alike.
- **Percentiles are nearest rank.** Sort the samples ascending; the p-th percentile is the sample
  at rank `ceil(p / 100 * n)` (1-based). For 10 cold processes, p50 is the 5th smallest.

## What each measure is

1. **Construction, cold process.** The bench re-executes itself 10 times per library with a
   `--child <library>` flag. The child reads the fixture and computes spans first, then times only
   its own font system construction: for cosmic-text a fontdb `Database`, both faces loaded and
   `FontSystem::new_with_locale_and_db`; for parley a fontique `Collection`, both faces
   registered, the `FontContext` and the `LayoutContext`. Process start is never timed. The font
   files are in the OS page cache after the first run (purging it needs root), so "cold" means a
   fresh process, not a cold disk.
2. **First screen, cold.** In the same child, straight after construction: the first 60 source
   lines laid out for the first time.
3. **First screen, warm.** The same 60 lines, 200 times in the parent after warm-up.
4. **Keystroke relayout.** Typing into source line 3, the first line with `**`, starting just
   after the closing `**`. Each keystroke inserts the next character of
   `the quick brown fox jumps over the lazy dog ` (prose, so wrapping is at words), recomputes the
   spans untimed, then times one relayout of that line. 500 keystrokes; the line grows from 77 to
   577 bytes (1 to 7 visual rows), so the percentiles mix short and long lines.
5. **Fallback quality.** `☐ ☑ ◦ → 😀 中文 ü é` laid out plainly in the sans, counting glyph id 0
   (`.notdef`), first with only the two faces, then with the fallback set added.

## Results: macOS (Apple silicon), release

| Measure | cosmic-text 0.19.0 | parley 0.12.0 |
|---|---|---|
| Construction, cold process, p50 of 10 | 0.07 ms (min 0.06 ms, max 0.14 ms) | 0.05 ms (min 0.04 ms, max 0.10 ms) |
| First screen (60 lines), cold, p50 of 10 | 0.72 ms (min 0.68 ms, max 1.17 ms) | 0.50 ms (min 0.42 ms, max 0.71 ms) |
| First screen, warm, p50 / p95 / p99 of 200 | 0.28 ms / 0.31 ms / 0.35 ms | 0.24 ms / 0.27 ms / 0.32 ms |
| Keystroke relayout, p50 / p95 / p99 of 500 | 58.1 µs / 95.1 µs / 99.6 µs | 40.3 µs / 63.3 µs / 65.2 µs |
| Keystroke relayout, max | 111.6 µs | 66.9 µs |
| First screen visual rows (cold / warm), total height px | 63 / 63, 3072 | 63 / 63, 3072 |
| Typed line visual rows after the last keystroke | 7 | 7 |
| `.notdef` glyphs, two faces only | 4 (☐ 😀 中 文) | 4 (☐ 😀 中 文) |
| `.notdef` glyphs, with fallback set | 2 (中 文) | 2 (中 文) |

This is the last of four full runs. The three before it gave keystroke p95 of 95.5, 94.7 and
95.2 µs for cosmic-text against 62.3, 62.3 and 61.6 µs for parley (34.8%, 34.2% and 35.2%
lower), and construction p50 of 0.05 to 0.06 ms against 0.04 to 0.05 ms. Other builds were
running on the machine at the time; the interleaving is there so that load lands on both.

### Did both libraries do the same work?

- The first screen is identical: `--compare` reports no line where rows or height differ.
- The typed line differs by one visual row on 37 of the 500 keystrokes, always with parley one
  row taller, near a wrap boundary. parley measures the same SF text about 3% wider. This is
  specific to SF: rerun with `--compare ... /System/Library/Fonts/Supplemental/Verdana.ttf`,
  every line and every keystroke match exactly. SF carries an AAT `trak` (tracking) table;
  parley passes the font size to HarfRust 0.12 as the point size, which applies tracking for
  it, while cosmic-text 0.19 shapes with HarfRust 0.5.2. The extra row means parley did slightly
  more work on those keystrokes, so it does not flatter parley. It is a fidelity question for
  later (which one matches Core Text at 16 pt, scale 2), not a performance one.
- Fallback: identical. ☐ and 😀 need the fallback set in both; ☑ ◦ → ü é are covered by SF.
  中文 stays missing in both, because the fixed set has no CJK face. That is the job of the
  on-demand `lookup` path (Task 42), not of the start-up set.

### The rule, applied to the Mac numbers

Rule (from the plan): stay on cosmic-text unless parley's per-keystroke p95 is at least 25% lower
**and** its cold construction isn't worse on either platform.

- Keystroke p95: 63.3 µs against 95.1 µs, 33.4% lower (33% to 35% across the four runs). Meets
  the 25% bar.
- Cold construction p50: 0.05 ms against 0.07 ms. Not worse. Both are tens of microseconds, so
  the condition holds but carries little weight on this platform.
- So on macOS, **parley wins by the rule**. The Linux half of the construction condition is
  still open (below).

Context for the owner, not part of the rule: both libraries are far inside a frame. The worst
keystroke relayout either produced was 0.11 ms, against 8.3 ms per frame at 120 Hz. parley is
also ahead on every other timed row (cold first screen 0.50 ms against 0.72 ms). Neither result
changes the stack design's concerns about maintenance and churn, which this bench can't measure.

## Linux (pending owner run)

On the Linux laptop, from the repository root:

```sh
cargo xtask fixture && cargo run -p layout-bench --release -- target/fixture/test.md
```

The bench resolves the faces with `fc-match -f '%{file}'` for `sans-serif`, `monospace`, `emoji`
and `symbol` at start, outside every timed region, and prints the files it used. Paste the whole
output here. If the Linux numbers reverse the Mac result (for example, parley's cold
construction is worse there), that also goes to the owner.

Results: pending.

## Verdict and the Task 11 trait shape

Pending the owner's decision. Task 11 should not start until it is made.

For that decision, the trait in Task 11's Interfaces is library-neutral as written
(`TextLayout::layout_line(text, spans, width_px) -> LaidLine`, `draw(line, sink)`, `LaidLine`
with `caret` and `hit`, `StyledSpan` with `FontRole { Body, Mono }`), and the bench's own
`Engine` interface (`construct`, `layout` per line slot, `notdef`) mapped onto both libraries
without either needing a different call shape. What would differ behind it:

- Font loading: cosmic-text uses a fontdb `Database` with faces loaded by path; parley uses a
  fontique `Collection` with `system_fonts: false` and faces registered from memory-mapped
  blobs. `FontSet::load(paths)` hides either.
- Fallback: cosmic-text uses its `Fallback` trait (the stack design's custom fallback) and tries
  every loaded face; parley needs the fallback families in each family stack or registered as
  fontique fallbacks per script. `FontResolver::fallback_set` and `lookup` feed either.
- Drawing: both expose positioned glyph runs with a font and glyph ids, so `draw` rasterises
  through swash in both cases.

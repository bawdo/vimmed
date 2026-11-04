# Checkpoint 1 measurements

Base sha 0337930 (Phase B complete, Tasks 1 to 17). The macOS cold starts in
section 1 were measured later, at sha 8a937bf (grammars linked by Task 27b,
lazy font fallback by Task 42). This page collects every Checkpoint 1 number. Sections marked pending are filled in by the orchestrator
or the owner; do not read an empty cell as a pass.

## Display used

The window now fits the screen's visible frame, so results depend on the
display. Record which one each measurement ran on.

| Measurement | Display | Resolution and scale | Refresh |
|---|---|---|---|
| macOS cold start (8a937bf) | Built-in Liquid Retina XDR (main display, Mac16,6) | 3024 x 1964, Retina | 120 Hz |
| macOS manual typing | pending (owner), same machine expected | pending | pending |
| Linux (all) | deferred to the end of the project (owner decision D8) | | |

Laptop and external-display results differ (different window size, different
scale, different refresh), so compare like with like.

## 1. macOS cold start (GUI run, orchestrator)

Command: `cargo xtask bundle-mac --release && cargo xtask bench start --runs 20`,
then `cargo xtask bench start --first-run --runs 1`. Fixture: `cargo xtask
fixture` (1000 lines).

- Sha 8a937bf (integration tip), with grammars linked (Task 27b) and the lazy
  font fallback (Task 42).
- Machine Mac16,6, macOS 27.2 (Darwin 27.2.0). Display: built-in Liquid Retina
  XDR, 3024 x 1964 Retina, main display, 120 Hz.
- The window is fitted to the laptop's visible frame (Task 15 D11), so these
  results are not directly comparable with a large external display.
- Binaries: window 16,042,896 B, launcher 337,184 B. See section 6.
- Results dir: `bench-results/macos-20261104-125839-start-default` (20 runs),
  `bench-results/macos-20261104-125903-start-first-run-default` (first run).

Result: PASS on both thresholds.

| Measure | Target | Result |
|---|---|---|
| Median to first frame | 250 ms or under | 205.7 ms, PASS |
| p95 to first frame | 400 ms or under | 214.7 ms, PASS |
| First run (empty font cache) | report only | 188.5 ms (one sample, not slower than the warm runs) |

20-run table (`bench start --runs 20`, milliseconds):

| Mark | median | p95 | p99 | max |
|---|---:|---:|---:|---:|
| launcher_main | 5.9 | 26.8 | 36.0 | 36.0 |
| launcher_spawned | 7.1 | 27.3 | 36.4 | 36.4 |
| window_main | 15.8 | 31.1 | 69.7 | 69.7 |
| file_read | 17.2 | 31.8 | 70.4 | 70.4 |
| parsed | 17.4 | 31.8 | 70.5 | 70.5 |
| fonts_loaded | 18.2 | 32.1 | 70.7 | 70.7 |
| toolkit_init | 116.6 | 127.6 | 155.3 | 155.3 |
| window_created | 140.1 | 155.2 | 176.4 | 176.4 |
| first_paint | 143.8 | 159.1 | 180.1 | 180.1 |
| highlight_ready | 179.5 | 205.6 | 211.0 | 211.0 |
| images_painted | 179.4 | 205.6 | 211.0 | 211.0 |
| first_frame | 205.7 | 214.7 | 229.0 | 229.0 |

`refresh_hz` read 120 on every run. The marks are as the bench reports them;
no per-stage subtraction has been derived from them here.

First run (`bench start --first-run --runs 1`, milliseconds): file_read 7.1,
first_frame 188.5, first_paint 127.4, fonts_loaded 11.0, highlight_ready
161.9, images_painted 161.9, launcher_main 2.4, launcher_spawned 2.8, parsed
7.2, toolkit_init 99.8, window_created 123.8, window_main 6.6. Cold start
188.5 ms against the 250 ms threshold: PASS. One sample, noted only.

Comparison with the Task 15 baseline (935ed41, before grammars were linked,
window about 3.0 MB):

| | Task 15 (935ed41) | Now (8a937bf) |
|---|---:|---:|
| first_frame median | 186.8 ms | 205.7 ms (+18.9) |
| first_frame p95 | 203.4 ms | 214.7 ms (+11.3) |

Both are still inside the thresholds. Earlier Task 15 runs at ff1a140 had
toolkit_init 107.4, window_created 129.4 and first_paint 133.1 ms; now they are
116.6, 140.1 and 143.8 ms. The growth sits before the first paint (process
start and toolkit init), which fits a binary about five times larger to map and
relocate. highlight_ready (179.5 ms median) lands before first_frame.

Linux stage budget (a reference for the i7-7700HQ, not a pass mark on the Mac).
The Linux stage runs are deferred (owner decision D8: macOS until the end), so
this table is not filled; the macOS marks above are the data.

| Stage | Linux budget | macOS |
|---|---:|---|
| Launcher to window `main` | 65 ms | see marks above |
| File, parse, config, lock and recovery scan, fonts | 20 ms | see marks above |
| GTK init and window (toolkit init and window on macOS) | 40 ms | see marks above |
| Renderer init to first paint | 80 ms | see marks above |
| First layout and raster, hand-off | 40 ms | see marks above |
| Total | 245 ms | 205.7 ms median to first frame |

## 2. macOS manual typing (owner)

Command: `cargo xtask bench type --manual --keys 500`.

| Measure | Target | Result |
|---|---|---|
| Key to compositor p95 | 16 ms or under | pending (owner) |
| On-glass p95 | 2.5 refresh periods or under (about 42 ms at 60 Hz) | pending (owner) |

Pending item: the owner's manual typing run against the current build, sha
8a937bf (not the earlier 0337930), on the macOS machine. Until it is recorded,
neither row above is a pass.

## 3. Linux (deferred to the end, owner decision D8)

The Linux kit, `scripts/checkpoint-linux.sh 1`, is written and checked
(`bash -n`, shellcheck) but the run is deferred to the end of the project by
owner decision D8. Nothing below is measured yet (macOS is the focus until the end).

| Measure | Target | Result |
|---|---|---|
| Cold start median, default renderer | 250 ms or under | deferred (D8) |
| Cold start p95, default renderer | 400 ms or under | deferred (D8) |
| Cold start median and p95, cairo | report | deferred (D8) |
| First run (empty font cache) | report | deferred (D8) |
| Keystroke p95, default renderer | 16 ms or under | deferred (D8) |
| Keystroke p95, cairo | no worse than default | deferred (D8) |
| Stage table vs budget (65, 20, 40, 80, 40; total 245 ms) | within budget | deferred (D8) |

## 4. Cairo against GL verdict and strip height (deferred, D8)

Rule: cairo stays opt-in unless its keystroke p95 and glass p95 are no worse
than GL's. The strip height comes from `bench type --wtype --keys 300` at the
strip variable set to 64, 128 and 256 (device pixels, 128 is the default).

| Item | Result |
|---|---|
| Cairo keystroke p95 vs GL | deferred (D8) |
| Cairo glass p95 vs GL | deferred (D8) |
| Verdict | deferred (D8) |
| Strip height 64 / 128 / 256 key p95 | deferred (D8) |
| Chosen strip height | deferred (D8), default stays 128 until then |

## 5. Keystroke draw cost, headless (macOS arm64, release)

Budget: our draw at or under 2 ms at p95 (GTK render, at or under 4 ms, is
separate and measured on Linux).

Harness: `crates/editor-render/tests/keystroke_timing.rs`, `#[ignore]`d.
Run `cargo xtask fixture` first, then:

```
cargo test -p editor-render --release --test keystroke_timing -- --ignored --nocapture --test-threads=1
```

Setup: `DocView::paint_lines` through the real markdown display model, the
committed Noto test fonts, 900x700 pt at scale 2, `Fake` source. The timer
wraps `paint_lines` only. Run on 2026-11-04 at sha 0337930 plus the harness.

| Case | Iterations | p50 | p95 | max | vs 2 ms p95 |
|---|---:|---:|---:|---:|---|
| One character typed into a prose line of the fixture (1,000 lines; height unchanged, own rows repainted) | 600 | 0.053 ms | 0.062 ms | 0.098 ms | pass, about 1/30 of budget |
| Height-changing edit at the top of a 10,000-line document (wraps and unwraps line 1, goes through `repaint_to_bottom`) | 200 | 0.675 ms | 0.822 ms | 0.888 ms | pass, about 0.4 of budget |

Notes and limits:

- `repaint_to_bottom` repaints down to the bottom of the visible text area, not
  the whole document, so the 10,000-line case costs a viewport of layout, not
  10,000 lines. That is why it stays under 1 ms.
- The first run of the large case (shaping from cold) is included in the 200
  iterations; max is 0.888 ms, so no cold-cache outlier showed up.
- The test fonts are Noto Sans and Noto Sans Mono, not the user's fonts, and the
  `Fake` source skips the shell, the status model and raster upload. Treat the
  figures as the render crate's own cost. The shell and compositor share of the
  16 ms comes from the manual typing runs (sections 2 and 3).

## 6. Release binary size (macOS arm64)

Two measurements, taken at different points. The first was on the real release
`window` and `launcher` at sha 0337930 (2026-11-04, `cargo build --release`),
before any shell used the highlighter. The later ones are the shipped bundle
once grammars and font fallback were linked.

| Binary | Sha | Bytes | Size |
|---|---|---:|---|
| `window` (before shells used the highlighter) | 0337930 | 3,023,616 | 2.9 MiB |
| `window` (grammars linked, Task 27b) | b7a1355 | 15,943,184 | 15.2 MiB |
| `window` (grammars and lazy font fallback, Task 42) | 8a937bf | 16,042,896 | 15.3 MiB |
| `launcher` | 0337930 and 8a937bf | 337,184 | 329 KiB |

`cargo bloat --release -p window --crates -n 20` at 0337930: `.text` is 2.3 MiB
of a 3.4 MiB file. Top crates: std 414.0 KiB, harfrust 358.4 KiB, skrifa
261.5 KiB, read_fonts 183.6 KiB, zeno 147.1 KiB, tiny_skia 134.5 KiB,
cosmic_text 124.5 KiB, image_webp 119.5 KiB, zune_jpeg 112.0 KiB, image
106.4 KiB, editor_core 88.8 KiB, editor_render 64.6 KiB, shell_mac 47.2 KiB.

The 3.0 MB figure was true only before the shells constructed a `Highlighter`:
the linker dropped the twelve tree-sitter grammars. Once they were wired in
(Task 27b) the window binary grew to 15.9 MB, and Task 42's lazy font fallback
added 99,712 B. Full detail is in `2026-11-binary-size.md`.

Effect on cold start: the window binary is about five times larger than at the
Task 15 baseline, and first_frame median moved from 186.8 to 205.7 ms (p95
203.4 to 214.7 ms), with the growth before first paint (section 1). That is
inside the 250 ms and 400 ms thresholds, but the binary size is now a cost
worth watching.

## 7. Judgement

macOS cold start passes at sha 8a937bf: median 205.7 ms against 250 ms, p95
214.7 ms against 400 ms. The binary grew about five times once the grammars were
linked and first_frame rose by about 19 ms median against the Task 15 baseline,
which still leaves headroom (44 ms at the median, 185 ms at p95). The headless
draw cost is well inside the 2 ms p95 budget.

Still open: the owner's manual typing run at 8a937bf (section 2), and the whole
Linux run (sections 3 and 4), deferred by owner decision D8. Do not read either
as a pass.

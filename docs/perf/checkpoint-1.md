# Checkpoint 1 measurements

Base sha 0337930 (Phase B complete, Tasks 1 to 17). The macOS cold starts in
section 1 were measured later, at sha 8a937bf (grammars linked by Task 27b,
lazy font fallback by Task 42). This page collects every Checkpoint 1 number.
Sections marked pending are filled in by the orchestrator or the owner; do not
read an empty cell as a pass.

## Display used

The window now fits the screen's visible frame, so results depend on the
display. Record which one each measurement ran on.

| Measurement | Display | Resolution and scale | Refresh |
|---|---|---|---|
| macOS cold start (8a937bf) | Built-in Liquid Retina XDR (main display, Mac16,6) | 3024 x 1964, Retina | 120 Hz |
| macOS manual typing (8a937bf) | Built-in Liquid Retina XDR (main display) | 3024 x 1964, Retina | 120 Hz |
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
116.6, 140.1 and 143.8 ms. Most of the growth sits before first paint (about
10.7 of the 18.9 ms). Note the baselines differ: first_paint is compared with
ff1a140, first_frame with 935ed41. Reading that growth as the cost of mapping
and relocating a binary about five times larger is an inference, not something
the marks show. highlight_ready (179.5 ms median) lands about 36 ms after
first_paint, inside the window, so highlighting is part of the later stretch
to first_frame.

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

Command: `cargo xtask bench type --manual --keys 500`. Step 3 is done: the
owner's manual run on the bundle built at sha 8a937bf (not the earlier
0337930), on the built-in 120 Hz Retina display.

- Results dir: `bench-results/macos-20261104-132919-type-manual-default`
  (`summary.txt`, `samples.csv`, `env.txt`).
- 517 key samples. Cold start in the same run: 219.7 ms (one sample, PASS
  against 250 ms).

| Measure | Target | Result |
|---|---|---|
| Key to compositor p95 (`key_ms`) | 16 ms or under | 11.8 ms, PASS |
| Our draw p95 (`key_draw_ms`) | 2 ms or under | 1.4 ms, PASS (p99 is 24.2 ms: see Bug D below) |
| On-glass p95 (`key_glass_est_ms`) | 2.5 refresh periods or under (about 42 ms at 60 Hz) | 27.5 ms, PASS under the absolute reading (owner decision D15) |

Per-stage figures (milliseconds):

| Measure | median | p95 | p99 | max |
|---|---:|---:|---:|---:|
| key_draw_ms | 0.6 | 1.4 | 24.2 | 25.1 |
| key_glass_est_ms | 22.6 | 27.5 | 47.3 | 52.3 |
| key_wait_ms | 1.6 | 3.6 | | |
| key_render_ms | | 0.2 | | |

Owner decision D15: the on-glass target reads as about 42 ms absolute, so
27.5 ms p95 is a PASS. Known gap: the same 27.5 ms is 3.30 refresh periods at
120 Hz, which would fail the "2.5 refresh periods" reading. The target is
recorded under the absolute reading; the gap against the period reading stays
open.

Stage split (computed by the orchestrator from `samples.csv`):

- Event to handler (`key_ms` minus wait, draw and render): median 4.7 ms, p95
  7.7 ms. This time is outside our timed stages.
- `key_glass_est_ms` minus `key_ms` is median 15.7 ms (range 11.5 to 16.5 ms
  per key), about 1.9 refresh periods at 120 Hz (the median). The estimator is the next display-link targetTimestamp
  after the commit, plus one refresh.

Draw tail (Bug D): diagnosed, fix pending the owner's confirmation. The p99 and
max of `key_draw_ms` (24.2 and 25.1 ms, against a 1.4 ms p95) come, most
likely, from Enter or a line join above the image (reproduced headlessly; the
run's log has timings only). `DocView` line slots belong to a line
index and do not move when lines are inserted or removed, so the image is
paired with the wrong slot, loses its cached resampled copy, and is resampled
on the UI thread inside the key handler (26 to 31 ms of a 30 to 35 ms key in
the headless reproduction). Typing below the image is expected not to spike (inferred, not measured). Not fixed.
See `diag-draw-spikes-report.md` in the v0.9 SDD folder.

## Owner GUI checks

All GREEN on 2026-11-04 (owner):

- Task 15 Step 6: type, resize, move between displays, close. Exit 0.
- Task 16 Step 4: Finder, two `.md` files opened together. Two processes (see
  `macos-finder-routing.md`).
- Task 42 Step 3: the glyphs ☐ ☑ 😀 中文 render.

## 3. Linux (deferred to the end, owner decision D8)

The Linux kit, `scripts/checkpoint-linux.sh 1`, is written and checked
(`bash -n`, shellcheck) but the run is deferred to the end of the project by
owner decision D8. Nothing below is measured yet.

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
(Task 27b) the window binary grew to 15.9 MB (15.2 MiB), and Task 42's lazy
font fallback added 99,712 B. Full detail is in `2026-11-binary-size.md`.

Effect on cold start: the window binary is about five times larger than at the
Task 15 baseline, and first_frame median moved from 186.8 to 205.7 ms (p95
203.4 to 214.7 ms), with most of the growth before first paint (section 1; the
baselines differ, see there). Whether the size causes that is an inference. It
is inside the 250 ms and 400 ms thresholds, but the binary size is now a cost
worth watching.

## 7. Judgement

macOS cold start passes at sha 8a937bf: median 205.7 ms against 250 ms, p95
214.7 ms against 400 ms. The binary grew about five times once the grammars were
linked and first_frame rose by about 19 ms median against the Task 15 baseline,
which still leaves headroom (44 ms at the median, 185 ms at p95). The headless
draw cost is well inside the 2 ms p95 budget.

macOS manual typing passes at sha 8a937bf (section 2): key to compositor p95
11.8 ms against 16 ms, and on-glass p95 27.5 ms against about 42 ms under the
absolute reading (owner decision D15). The same 27.5 ms is 3.30 refresh periods
at 120 Hz, a known gap against the period reading. The draw tail (p99 24.2 ms,
max 25.1 ms) is Bug D, diagnosed with the fix pending, not fixed.

Still open: the whole Linux run (sections 3 and 4), deferred by owner decision
D8. Do not read it as a pass.

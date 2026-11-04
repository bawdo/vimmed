# Checkpoint 2 measurements

Tip a716942 (Merged task 29: key mapping and input methods), macOS arm64. This
page collects every Checkpoint 2 number, in the same order as
`checkpoint-1.md` so the two read side by side. Linux is deferred (section 3).

Checkpoint 1's headless draw harness, binary size and owner GUI check sections
were not re-measured at this checkpoint.

## Display used

| Measurement | Display | Resolution and scale | Refresh |
|---|---|---|---|
| macOS cold start (a716942) | Built-in Liquid Retina XDR (main display, Apple M4 Max) | 3024 x 1964, Retina | 120 Hz |
| macOS manual typing (a716942) | Built-in Liquid Retina XDR (main display) | 3024 x 1964, Retina | 120 Hz |
| Linux (all) | deferred to the end of the project (owner decision D8) | | |

Same machine and display as Checkpoint 1 (`env.txt` identical in every result
dir: macOS 27.2, build 26B123, Darwin 27.2.0). `refresh_hz` read 120 on every
run, so the comparison with Checkpoint 1 is like for like.

## 1. macOS cold start (GUI run)

Command: `cargo xtask bench start --runs 20`, then `cargo xtask bench start
--first-run --runs 1`. Fixture: `cargo xtask fixture` (1000 lines).

- Results dir: `bench-results/macos-20261104-193521-start-default` (20 runs),
  `bench-results/macos-20261104-193542-start-first-run-default` (first run).

Result: PASS on both thresholds.

| Measure | Target | Result |
|---|---|---|
| Median to first frame | 250 ms or under | 189.1 ms, PASS |
| p95 to first frame | 400 ms or under | 199.2 ms, PASS |
| First run (empty font cache) | report only | 161.5 ms (one sample, not slower than the warm runs) |

20-run table (`bench start --runs 20`, milliseconds):

| Mark | median | p95 | p99 | max |
|---|---:|---:|---:|---:|
| launcher_main | 5.0 | 6.6 | 6.9 | 6.9 |
| launcher_spawned | 5.8 | 8.0 | 8.3 | 8.3 |
| window_main | 13.0 | 16.7 | 16.7 | 16.7 |
| file_read | 14.2 | 18.1 | 18.1 | 18.1 |
| parsed | 14.4 | 18.3 | 18.3 | 18.3 |
| fonts_loaded | 15.0 | 18.9 | 19.0 | 19.0 |
| toolkit_init | 113.0 | 120.3 | 123.1 | 123.1 |
| window_created | 136.5 | 145.7 | 149.0 | 149.0 |
| first_paint | 140.0 | 149.3 | 152.7 | 152.7 |
| highlight_ready | 169.3 | 186.4 | 188.7 | 188.7 |
| images_painted | 194.4 | 205.6 | 211.0 | 211.0 |
| first_frame | 189.1 | 199.2 | 204.0 | 204.0 |

First run (`bench start --first-run --runs 1`, milliseconds): file_read 5.9,
first_frame 161.5, first_paint 115.2, fonts_loaded 9.8, highlight_ready 138.6,
images_painted 171.3, launcher_main 1.5, launcher_spawned 1.9, parsed 6.0,
toolkit_init 88.0, window_created 111.6, window_main 5.3. Cold start 161.5 ms
against the 250 ms threshold: PASS. One sample, noted only.

Reading the marks: in this run `images_painted` (median 194.4 ms) lands after
`first_frame` (189.1 ms). At Checkpoint 1 it landed at the same time as
`highlight_ready`, before `first_frame`. The bench reports the marks as they
are; why the order moved has not been derived. Both are inside the thresholds
(`first_frame` is the gated mark).

Linux stage budget: not filled, deferred (section 3). The macOS marks above
are the data.

## 2. macOS manual typing (owner)

Command: `cargo xtask bench type --manual --keys 500`. Three runs by the
owner on the bundle at a716942, Insert mode, full vim engine and reveal live,
all three recorded (nothing dropped). Run 1 is the miss; runs 2 and 3 are the
re-runs.

| Run | Results dir (under `bench-results/`) | Keys | Gate |
|---|---|---:|---|
| 1 | `macos-20261104-192815-type-manual-default` | 502 | FAIL, key_ms p95 16.5 ms |
| 2 | `macos-20261104-193739-type-manual-default` | 504 | PASS, 11.3 ms |
| 3 | `macos-20261104-193951-type-manual-default` | 502 | PASS, 13.2 ms |

Cold start in the same runs (one sample each, PASS against 250 ms): 182.7,
175.7, 181.4 ms.

| Measure | Target | Run 1 | Run 2 | Run 3 |
|---|---|---|---|---|
| Key to compositor p95 (`key_ms`) | 16 ms or under | 16.5 ms, FAIL | 11.3 ms, PASS | 13.2 ms, PASS |
| Our draw p95 (`key_draw_ms`) | 2 ms or under | 2.3 ms, over | 1.3 ms, PASS | 1.6 ms, PASS |
| On-glass p95 (`key_glass_est_ms`), absolute reading (D15, about 42 ms) | 2.5 refresh periods or under | 31.1 ms, PASS | 27.3 ms, PASS | 28.8 ms, PASS |

The only gated measure is `key_ms` p95 (the summary prints that one). Run 1's
draw p95 of 2.3 ms is over the 2 ms draw target, which is a reported target
and not part of the summary's PASS or FAIL line. On-glass in refresh periods at
120 Hz (8.33 ms): 3.73, 3.28 and 3.46 periods, so the known gap against the
"2.5 refresh periods" reading (owner decision D15, section 7 of Checkpoint 1)
is still open in all three runs; the absolute reading passes.

Per-stage figures (milliseconds):

| Measure | Run | median | p95 | p99 | max |
|---|---|---:|---:|---:|---:|
| key_ms | 1 | 8.2 | 16.5 | 36.2 | 114.3 |
| key_ms | 2 | 7.5 | 11.3 | 14.5 | 23.1 |
| key_ms | 3 | 7.5 | 13.2 | 16.6 | 26.6 |
| key_wait_ms | 1 | 1.9 | 8.9 | 32.3 | 111.3 |
| key_wait_ms | 2 | 1.6 | 4.0 | 8.6 | 17.9 |
| key_wait_ms | 3 | 2.4 | 8.2 | 9.5 | 16.4 |
| key_draw_ms | 1 | 0.5 | 2.3 | 3.4 | 5.7 |
| key_draw_ms | 2 | 0.6 | 1.3 | 3.5 | 5.2 |
| key_draw_ms | 3 | 0.4 | 1.6 | 3.0 | 6.8 |
| key_render_ms | 1 | 0.2 | 0.2 | 0.3 | 0.4 |
| key_render_ms | 2 | 0.2 | 0.2 | 0.3 | 0.3 |
| key_render_ms | 3 | 0.1 | 0.2 | 0.2 | 0.3 |
| key_glass_est_ms | 1 | 23.9 | 31.1 | 52.2 | 129.0 |
| key_glass_est_ms | 2 | 23.1 | 27.3 | 30.2 | 39.2 |
| key_glass_est_ms | 3 | 22.4 | 28.8 | 31.9 | 38.9 |

### Draw cost by input (`key_draw_ms`, new since Checkpoint 1)

Milliseconds; n is the key count. Rows with n under 5 are noted but carry
little weight.

| Input | Run 1 n, median, p95, max | Run 2 n, median, p95, max | Run 3 n, median, p95, max |
|---|---|---|---|
| arrow | 30, 0.5, 1.5, 1.7 | none | none |
| backspace | 40, 0.5, 2.7, 3.0 | 22, 0.5, 1.2, 1.5 | 24, 0.5, 0.6, 0.7 |
| enter | 18, 3.0, 5.7, 5.7 | 7, 3.6, 4.6, 4.6 | 16, 2.5, 3.1, 3.1 |
| other | 66, 0.2, 0.9, 1.6 | 1, 0.8, 0.8, 0.8 | 2, 0.2, 0.8, 0.8 |
| text | 348, 0.5, 1.2, 4.2 | 474, 0.6, 1.3, 5.2 | 460, 0.4, 1.0, 6.8 |

### Draw cost by redraw kind (`key_draw_ms`)

| Redraw | Run 1 n, median, p95, max | Run 2 n, median, p95, max | Run 3 n, median, p95, max |
|---|---|---|---|
| lines | 479, 0.5, 2.3, 5.7 | 504, 0.6, 1.3, 5.2 | 502, 0.4, 1.6, 6.8 |
| none | 12, 0.0, 0.0, 0.0 | none | none |
| status | 11, 0.1, 1.3, 1.3 | none | none |

### Draw cost by input and redraw (`key_draw_ms`)

| Input and redraw | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| arrow.lines | 30, 0.5, 1.5, 1.7 | none | none |
| backspace.lines | 40, 0.5, 2.7, 3.0 | 22, 0.5, 1.2, 1.5 | 24, 0.5, 0.6, 0.7 |
| enter.lines | 17, 3.0, 5.7, 5.7 | 7, 3.6, 4.6, 4.6 | 16, 2.5, 3.1, 3.1 |
| enter.status | 1, 1.3, 1.3, 1.3 | none | none |
| other.lines | 52, 0.2, 1.2, 1.6 | 1, 0.8, 0.8, 0.8 | 2, 0.2, 0.8, 0.8 |
| other.none | 12, 0.0, 0.0, 0.0 | none | none |
| other.status | 2, 0.0, 0.4, 0.4 | none | none |
| text.lines | 340, 0.6, 1.2, 4.2 | 474, 0.6, 1.3, 5.2 | 460, 0.4, 1.0, 6.8 |
| text.status | 8, 0.0, 0.4, 0.4 | none | none |

Cells are n, median, p95, max in milliseconds. Enter is the dearest input
(median 2.5 to 3.6 ms) but it stays under 6 ms; at Checkpoint 1 the same
inputs cost 30 to 35 ms (Bug D, below).

Run 1 is the only run with arrow keys, `other.none` and `status` redraws (it
had 66 `other` keys against 1 and 2 in runs 2 and 3). Runs 2 and 3 are almost
entirely text, backspace and enter.

### Bug D (draw tail): no draw spike observed in these runs, after the D-b fix

At Checkpoint 1 the `key_draw_ms` p99 was 24.2 ms and the max 25.1 ms, with
Enter or a line join above an image costing 30 to 35 ms (the image was paired
with the wrong `DocView` line slot, lost its cached resampled copy and was
resampled on the UI thread). Now `key_draw_ms` p99 is 3.0 to 3.5 ms and the max
is 5.2 to 6.8 ms across the three runs, consistent with the D-b fix. The
slowest draws in these runs are on text keys (max 4.2, 5.2 and 6.8 ms), not on
Enter. Fix reference: D-b.

## 3. Linux (deferred to the end, owner decision D8)

Same as Checkpoint 1: the Linux laptop run is deferred to the end of the
project by owner decision D8. Nothing below is measured.

| Measure | Target | Result |
|---|---|---|
| Cold start median, default renderer | 250 ms or under | deferred (D8) |
| Cold start p95, default renderer | 400 ms or under | deferred (D8) |
| Cold start median and p95, cairo | report | deferred (D8) |
| First run (empty font cache) | report | deferred (D8) |
| Keystroke p95, default renderer | 16 ms or under | deferred (D8) |
| Keystroke p95, cairo | no worse than default | deferred (D8) |
| Stage table vs budget (65, 20, 40, 80, 40; total 245 ms) | within budget | deferred (D8) |
| Cairo against GL verdict and strip height (64 / 128 / 256) | per Checkpoint 1 rule | deferred (D8) |

Smoke figure only, not a gate: the Docker weston smoke in the most recent
linux-check (at b05d720) reported first_frame 931.2 ms. That is software
rendering in a container, not the i7-7700HQ target, so do not compare it with
the 250 ms threshold.

## 4. Change since Checkpoint 1

Checkpoint 1 figures are from `checkpoint-1.md` (cold start at 8a937bf,
typing at 8a937bf). Deltas are Checkpoint 2 minus Checkpoint 1, in
milliseconds; negative is faster. Startup uses the 20-run medians. For typing,
the delta is run 3 (the last run, and a pass) minus Checkpoint 1; run 1 is
shown beside it because it is the stall run. A dash means Checkpoint 1 did not
record that cell.

Startup stages (median, 20 runs):

| Stage | Checkpoint 1 | Checkpoint 2 | Delta |
|---|---:|---:|---:|
| launcher_main | 5.9 | 5.0 | -0.9 |
| launcher_spawned | 7.1 | 5.8 | -1.3 |
| window_main | 15.8 | 13.0 | -2.8 |
| file_read | 17.2 | 14.2 | -3.0 |
| parsed | 17.4 | 14.4 | -3.0 |
| fonts_loaded | 18.2 | 15.0 | -3.2 |
| toolkit_init | 116.6 | 113.0 | -3.6 |
| window_created | 140.1 | 136.5 | -3.6 |
| first_paint | 143.8 | 140.0 | -3.8 |
| highlight_ready | 179.5 | 169.3 | -10.2 |
| images_painted | 179.4 | 194.4 | +15.0 |
| first_frame (median) | 205.7 | 189.1 | -16.6 |
| first_frame (p95) | 214.7 | 199.2 | -15.5 |
| first run first_frame (1 sample) | 188.5 | 161.5 | -27.0 |

Every stage up to `highlight_ready` is faster. `images_painted` is the one
stage that moved the other way (+15.0 ms median, and it now lands after
`first_frame`). The runs are separate sessions on the same machine, so
differences of a few milliseconds sit inside normal run-to-run spread. The
gain is not attributed to a cause.

Keystroke components (milliseconds):

| Component | Statistic | Checkpoint 1 | Checkpoint 2 run 1 | Checkpoint 2 run 3 | Delta (run 3) |
|---|---|---:|---:|---:|---:|
| key_wait_ms | median | 1.6 | 1.9 | 2.4 | +0.8 |
| key_wait_ms | p95 | 3.6 | 8.9 | 8.2 | +4.6 |
| key_draw_ms | median | 0.6 | 0.5 | 0.4 | -0.2 |
| key_draw_ms | p95 | 1.4 | 2.3 | 1.6 | +0.2 |
| key_draw_ms | p99 | 24.2 | 3.4 | 3.0 | -21.2 |
| key_draw_ms | max | 25.1 | 5.7 | 6.8 | -18.3 |
| key_render_ms | median | 0.1 | 0.2 | 0.1 | 0.0 |
| key_render_ms | p95 | 0.2 | 0.2 | 0.2 | 0.0 |
| key_ms | median | 7.0 | 8.2 | 7.5 | +0.5 |
| key_ms | p95 | 11.8 | 16.5 | 13.2 | +1.4 |
| key_ms | p99 | 31.6 | 36.2 | 16.6 | -15.0 |
| key_glass_est_ms | median | 22.6 | 23.9 | 22.4 | -0.2 |
| key_glass_est_ms | p95 | 27.5 | 31.1 | 28.8 | +1.3 |
| key_glass_est_ms | p99 | 47.3 | 52.2 | 31.9 | -15.4 |

Run 2 for the same rows is in section 2 (key_ms p95 11.3, key_wait p95 4.0,
key_glass p95 27.3), where it sits level with Checkpoint 1. Run 3's higher
`key_wait_ms` p95 (8.2 ms against 3.6) is the likeliest source of its +1.4 ms
in `key_ms` p95 (inferred from the two moving together, not measured per key);
it did not recur in run 2. The draw tail fell by about 21 ms at
p99 and the p95 draw is level. The typing p95s are roughly where Checkpoint 1
was, with the tail much shorter once the Bug D spike is gone.

## 5. Judgement

Checkpoint rule: gate on `key_ms` p95 at or under 16 ms and cold start median
at or under 250 ms, p95 at or under 400 ms.

Cold start passes at a716942: median 189.1 ms (250 ms), p95 199.2 ms (400 ms),
first run 161.5 ms. That is 16.6 ms faster at the median than Checkpoint 1.

Manual typing: gate PASS on 2 of 3 runs (run 2 at 11.3 ms, run 3 at 13.2 ms;
run 1 at 16.5 ms missed). Ruling R53, in substance: run 1's miss was a
transient main-thread stall, not a steady cost. The evidence:

- It sits in keys 0 to 196 only. The slow keys show a backlog shape: one key
  at 111 ms (`key_wait_ms` max 111.3, `key_ms` max 114.3), then 47, 46, 32 and
  30 ms queued behind it.
- The run began straight after a screen unlock and a bundle rebuild.
- It did not recur in the 1006 later keys (504 in run 2, 502 in run 3). Those
  runs have `key_wait_ms` max of 17.9 and 16.4 ms.

So the gate is a PASS under the checkpoint rule on 2 of 3 runs, with run 1
recorded in full and not discarded.

Open carry: runs 2 and 3 contain almost no non-text keys (run 2: 1 `other`,
no arrows; run 3: 2 `other`, no arrows). A stall on a mode switch, such as Esc
deactivating the input context, is therefore not excluded: run 1 had the
non-text keys, but it is also the run with the stall, and the data cannot say
which key caused it (see the tooling gap below). Re-exercise mode switches at
Checkpoint 3.

Draw tail: no Bug D draw spike was observed in these runs, after the D-b fix.
`key_draw_ms` p99 is 3.0 to 3.5 ms and max 5.2 to 6.8 ms, against 30 to 35 ms
on Enter or a join at Checkpoint 1. Run 1's draw p95 (2.3 ms) is over the 2 ms
draw target; the stall is in `key_wait_ms`, not draw, and run 1's draw p95 is
driven by backspace (p95 2.7 ms) and enter (p95 5.7 ms). Runs 2 and 3 are under
2 ms.

On-glass: passes under the absolute reading (D15) in all three runs. The gap
against the "2.5 refresh periods" reading at 120 Hz (3.28 to 3.73 periods)
stays open, unchanged from Checkpoint 1.

Still open: the whole Linux run (section 3), deferred by owner decision D8. Do
not read it as a pass.

## 6. Bench tooling gap

`samples.csv` per-kind samples carry per-kind indexes (`key_ms,0`, `key_draw_ms,0`
count separately), so a slow key cannot be joined to its input kind.

# Checkpoint 1 measurements

Base sha 0337930 (Phase B complete, Tasks 1 to 17). This page collects every
Checkpoint 1 number. Sections marked pending are filled in by the orchestrator
or the owner; do not read an empty cell as a pass.

## Display used

The window now fits the screen's visible frame, so results depend on the
display. Record which one each measurement ran on.

| Measurement | Display | Resolution and scale | Refresh |
|---|---|---|---|
| macOS cold start | pending (orchestrator) | pending | pending |
| macOS manual typing | pending (owner) | pending | pending |
| Linux (all) | deferred, D8 | | |

Laptop and external-display results differ (different window size, different
scale, different refresh), so compare like with like.

## 1. macOS cold start (GUI run, orchestrator)

Command: `cargo xtask bundle-mac --release && cargo xtask bench start --runs 20`,
then `cargo xtask bench start --first-run --runs 1`.

| Measure | Target | Result |
|---|---|---|
| Median to first frame | 250 ms or under | pending (orchestrator) |
| p95 to first frame | 400 ms or under | pending (orchestrator) |
| First run (empty font cache) | report only | pending (orchestrator) |

Stages against the Linux budget (the budget is for the i7-7700HQ, so it is a
reference here, not a pass mark on the Mac):

| Stage | Linux budget | macOS median |
|---|---:|---|
| Launcher to window `main` | 65 ms | pending (orchestrator) |
| File, parse, config, lock and recovery scan, fonts | 20 ms | pending (orchestrator) |
| GTK init and window (toolkit init and window on macOS) | 40 ms | pending (orchestrator) |
| Renderer init to first paint | 80 ms | pending (orchestrator) |
| First layout and raster, hand-off | 40 ms | pending (orchestrator) |
| Total | 245 ms | pending (orchestrator) |

## 2. macOS manual typing (owner)

Command: `cargo xtask bench type --manual --keys 500`.

| Measure | Target | Result |
|---|---|---|
| Key to compositor p95 | 16 ms or under | pending (owner) |
| On-glass p95 | 2.5 refresh periods or under (about 42 ms at 60 Hz) | pending (owner) |

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

Re-measured on the real release `window` and `launcher` (not the throwaway
harness the earlier numbers used). Sha 0337930, 2026-11-04,
`cargo build --release`.

| Binary | Bytes | Size |
|---|---:|---|
| `target/release/window` | 3,023,616 | 2.9 MiB |
| `target/release/launcher` | 337,184 | 329 KiB |

`cargo bloat --release -p window --crates -n 20`: `.text` is 2.3 MiB of a
3.4 MiB file. Top crates: std 414.0 KiB, harfrust 358.4 KiB, skrifa 261.5 KiB,
read_fonts 183.6 KiB, zeno 147.1 KiB, tiny_skia 134.5 KiB, cosmic_text
124.5 KiB, image_webp 119.5 KiB, zune_jpeg 112.0 KiB, image 106.4 KiB,
editor_core 88.8 KiB, editor_render 64.6 KiB, shell_mac 47.2 KiB.

The real binary is 3.0 MB, not the 13.0 MiB the grammar measurement showed. No
shell or window code constructs a `Highlighter` yet, so the linker drops the
twelve tree-sitter grammars. Expect the size to jump towards the earlier figure
once code-block highlighting is wired into the view path; re-measure then. Full
detail is in `2026-11-binary-size.md`.

## 7. Judgement

Pending the GUI numbers (sections 1 and 2) and the Linux run (section 3, D8).
Nothing measured so far misses a threshold: the headless draw cost is well
inside the 2 ms p95 budget, and the binary is small.

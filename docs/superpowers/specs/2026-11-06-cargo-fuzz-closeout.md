# vimmed: fuzzing with cargo-fuzz and ClusterFuzzLite, closeout

Date: 2026-11-06
Design: `2026-11-06-cargo-fuzz-design.md`
Branch: feature/cargo-fuzz (79100e6..HEAD at close)

This statement answers the hypothesis in the design document. The design is left
as it was written.

## Fit

The hypothesis was half right, and much faster than expected.

- **The vim key machine.** It held real defects that the example tests missed. The
  `editor_keys` target found three of them inside its first twelve-minute local runs,
  before any scheduled batch run existed. The hypothesis allowed four weeks.
- **The incremental Markdown reparse.** It held none that we found. The
  `markdown_incremental` target checks the full `Blocks::update` contract against a
  fresh parse. It reached 87.89% of `markdown/block.rs` and found nothing.
- **The kind of defect.** We expected panics, hangs or wrong results. Every defect the
  fuzzer found was a resource defect: a hang, or memory exhaustion. None was a panic
  or a wrong result.

## Proven

- **`editor_keys` reaches defects fast.** It found three:
  - **Quadratic grapheme stepping.** `grapheme::next` and `prev` rescanned the line
    from byte 0 on every call. Fixed in 073f6b9. The minimised input went from 5.8 s
    to 15 ms, and `cargo test` replays it from `fuzz/regressions/editor_keys/`.
  - **Per-key cost proportional to line length.** `display_col` ran two to four times
    on every key, with nothing cached. This is contained, not fixed: a harness guard
    (f7737d7) ends a run once the cursor line passes 2 KiB. The reference input is in
    `fuzz/known/editor_keys/`, and the fix is handed off to its own branch.
  - **Out-of-memory from a counted put.** Puts were capped by bytes while memory is
    spent per line. Fixed in 75eb7cc: a put now adds at most 2^18 lines. The fuzzer's
    input went from a 1,411 MB peak to 45 MB, and it is a regression input.
- **The Markdown reparse held up.** `Blocks::update` met its full contract under an
  hour of local fuzzing, and the oracle is shared with the unit test.
- **Coverage at merge** (the fuzz corpus after one hour, plus the seeds; the bar was
  70%):

  | Target | Area | Coverage |
  |---|---|---|
  | `markdown_incremental` | `markdown/block.rs` | 87.89% |
  | `editor_keys` | `vim/` | 89.27% |
  | `ex_command` | `ex.rs` | 97.87% |

- **The ClusterFuzzLite image works.** It builds, its pinned nightly takes precedence
  over the image's own, and it compiles all five targets with AddressSanitizer. The
  owner confirmed all three with a local Docker run.
- **Every fixed crash replays on stable.** Each one has a minimised input under
  `fuzz/regressions/` that plain `cargo test` replays and that passes after its fix.

## Disproven

- **"Defects concentrate in `Blocks::update`."** It survived an hour of fuzzing at
  87.89% coverage with an oracle covering ids, block info, repaint ranges and the
  rebuild fallback. This is provisional, because the four-week batch window has not
  run yet.
- **"The defects are panics or wrong results."** All three fuzzer findings were
  resource exhaustion. The one wrong result found on the branch came another way:
  `:s` with a newline in the replacement misplaces later matches. Writing the
  `ex_command` harness turned it up, not the fuzzer, and it sits in what the design
  called a cheap target.
- **"ClusterFuzzLite's Rust base image brings its own nightly, so CI is
  unaffected."** False. The pinned image ships `nightly-2025-09-05` (rustc 1.91),
  below the workspace's `rust-version` of 1.92, so every ClusterFuzzLite build would
  have failed. The image now installs the same dated nightly as
  `fuzz/rust-toolchain.toml`.
- **"A key-sequence harness with an input-length cap keeps a run short."** False. A
  count prefix turns 8 bytes of input into millions of operations, so the input cap
  bounds nothing. Runs are now bounded by `cfg(fuzzing)` limits in the product
  (counts stop at 1,000, puts are capped lower) and by two harness run budgets.

## Divergence

- **The coverage run stopped three times on findings.** Each finding was diagnosed
  with the owner before any fix, and fixes landed mid-branch (073f6b9, 75eb7cc). The
  coverage that was finally recorded came from a fourth attempt. That attempt ran in
  libFuzzer fork mode, so timeouts and OOMs in already-diagnosed classes would have
  been recorded rather than stopping the run. None occurred.
- **Five things were added that the plan did not foresee.**
  - The `editor_keys` run budgets: 2 KiB cursor line, 65,536 lines.
  - `cfg(fuzzing)` limits in the product (7c994b4).
  - A per-run oracle budget for `markdown_incremental` (110c6d8). The final review
    measured the harness, not the product, taking 31 s against the 10 s timeout.
  - `.gitattributes` treating fuzz inputs as binary.
  - A `.dockerignore`.
- **The fix for per-key cost was spun off.** After the second recurrence, the owner
  asked for root-cause research. It found two causes:
  - derived metrics recomputed on every key with nothing cached;
  - no budgets on accumulated state: counts, undo bytes, `:s` output, and `.` and
    doubling growth.

  That work is handed off to its own spec and branch rather than built here.
- **Still owed after merge:** the four-week count of crashes, hangs and oracle
  failures per target from the weekly batch runs, and the first Scorecard Fuzzing
  reading. The Scorecard half of the hypothesis is untested until then.

# Fuzzing

Coverage-guided fuzzing of `editor-core` with [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html)
(libFuzzer). The harnesses live in `crates/editor-core/src/fuzz/` behind the
`fuzzing` feature. This directory holds the libFuzzer entry points, the seed
corpus and the saved crash inputs.

`fuzz/` is not part of the workspace. It has its own `Cargo.lock` and its own
toolchain (a dated nightly, pinned in `fuzz/rust-toolchain.toml`), so
`cargo xtask ci` and the stable 1.99.1 toolchain never build it.

## Setup

```sh
rustup toolchain install nightly-2026-10-23 --profile minimal --component llvm-tools-preview
cargo install cargo-fuzz --locked
```

The same nightly is named in three places: the ClusterFuzzLite image
(`.clusterfuzzlite/Dockerfile`), `fuzz/rust-toolchain.toml` and this README.
They move together and are bumped by hand.

Run everything below from `fuzz/`. `rustup` picks the pinned nightly there.

## Running a target

```sh
cd fuzz
cargo fuzz run <target> "$TMPDIR/corpus-<target>" corpus/<target> -- -max_len=<N> -timeout=10
```

libFuzzer writes every new input it finds into the first corpus directory it
is given, so always put a scratch directory first: naming `corpus/<target>`
first fills the committed seeds with generated inputs. Add
`-max_total_time=60` for a short run.

| Target                 | `-max_len` |
| ---------------------- | ---------- |
| `markdown_incremental` | 65536      |
| `ex_command`           | 65536      |
| `editor_keys`          | 4096       |
| `config_parse`         | 65536      |
| `state_files`          | 65536      |

`-timeout=10` is the same for all of them.

## Input formats

Every harness takes raw bytes and must return without panicking. Text parts
are decoded lossily, so invalid UTF-8 is never a reason to bail out (except
`config_parse`, below).

- `config_parse`: the bytes are the config file. Anything that is not valid
  UTF-8 is ignored, as `Config::load` falls back to the defaults for it.
- `state_files`: the first byte picks a parser. `r` is the registers file (the
  rest is its bytes), `m` is recovery metadata (the rest, decoded lossily).
  Anything else, or empty input, is ignored. Registers that parse must
  survive being serialised and parsed again unchanged.
- `ex_command`: `cmd 0x00 text 0x00 tail`. `cmd` is the ex command (`:s` and
  friends), `text` is the buffer, and a missing part is empty. `tail[0..4]` are
  `cursor, vflag, v0, v1` (zero when missing): the cursor line wraps onto the
  buffer, and when `vflag & 1` is set the last Visual selection is
  `(v0 % n, v1 % n)`. `tail[4..]` are confirm answers, one key each. Only a
  `:s` substitute goes past parsing: any other command returns straight after
  `ex::parse`. The applied `run` and the confirm session's result are both
  compared with an oracle that rebuilds each line from strings, without
  working out any position, and the session's current match must hold the
  text it asks about.
- `markdown_incremental`: the initial text up to the first `0x00`, then edit
  records. A record is six bytes `[flags, l0, c0, l1, c1, len]` followed by
  `len` bytes of replacement text. A truncated record ends the sequence.
  Positions wrap onto the buffer and snap down to a char boundary. When
  `flags & 0x0f` is zero the incremental update is handed an edited range that
  cannot fit and must fall back to a full rebuild. The high nibble of `flags`
  places the cursor for cursor-relative reveal. At most 256 edit records are
  applied, and the run also ends once the harness's own checking passes a
  per-run budget (`ORACLE_BUDGET`), so the timeout reflects the product, not
  the oracle.
- `editor_keys`: `text SEP keys`, where `SEP` is the first `0x00` (Mac) or
  `0x01` (Linux) and sets the platform. With neither, all of it is keys on an
  empty text, on the Mac. A leading `R` in the keys opens the document in
  reader mode and is not pressed. The keys use the tests' `<Esc>` notation
  (`keyseq::parse_keys`). The editor has no saver and no state directory, so
  `:w`, `:e!` and closing touch no file. After every key the cursor must sit
  on a char boundary of the buffer, and a key that closes the window ends the
  run.

  Four long-line inputs live in `fuzz/regressions/editor_keys/`.
  `long_line_per_key_cost` is the original reduction and
  `long_non_ascii_line_per_key_cost` its non-ASCII counterpart. Both are
  trivial under `cfg(fuzzing)`: counts clamp at 1,000, so the 51,111,110 count
  builds only a 1,000-byte line and each runs in a millisecond or two under
  ASan. `long_line_1mb_keys` and `long_non_ascii_line_1mb_keys` build a line of
  about 1 MB within the fuzz limits, press `h` 1,500 times back from its end
  and type 1,500 characters there, so they show the long-line per-key cost
  under ASan (about 1 s each, and over a minute before the column index). A
  plain `cargo test` replay of any of the four proves only that the run
  terminates, not that it is fast.

  `long_grapheme_cluster` holds two lines that each carry one grapheme of
  64 KiB (an `e` and 32,768 combining acute accents), and moves and types
  next to it. The column index checkpoints only grapheme boundaries, so each
  step across that grapheme walks all of it: a cost that grows with the
  grapheme, about 250 µs a key at this size in a release build, and no index
  avoids it. Run under ASan with `-timeout`, it catches that cost growing
  faster than the grapheme; under `cargo test`, as with the others, it proves
  only that the run terminates. It was written by hand, not found by the
  fuzzer.

## Replay under `cargo test`

`editor-core` replays `fuzz/corpus/`, `fuzz/regressions/` and `fuzz/known/`
under plain `cargo test` on stable, so it runs in the normal CI with no
nightly. Every subdirectory of those three must name a target, so a stray
directory fails the test, as does a missing or empty target directory under
`fuzz/corpus/`. Stray files beside the target directories are skipped.

## Fuzz-build limits

cargo-fuzz and ClusterFuzzLite build with `cfg(fuzzing)`. Under it vimmed
lowers its own limits, so runs exercise the same limit-handling code without
spending time on huge counts or documents. Normal builds and `cargo test` use
the real limits, so committed regression inputs replay against the product's
behaviour. The limits follow the rustc `cfg(fuzzing)`, not the `fuzzing` cargo
feature: enabling only the feature keeps the real limits.

| Limit                    | Normal build          | Fuzz build |
| ------------------------ | --------------------- | ---------- |
| Count ceiling            | 999,999,999           | 1,000      |
| Document lines           | 4,194,304             | 65,536     |
| Document bytes           | 268,435,456 (256 MiB) | 1,048,576  |
| Undo bytes               | 67,108,864 (64 MiB)   | 1,048,576  |
| Changes in one `:s`      | 262,144               | 4,096      |
| Put bytes                | 67,108,864 (64 MiB)   | 1,048,576  |
| Put lines                | 262,144               | 16,384     |

Document bytes count line contents only, not line breaks.

`MAX_UNDO_GROUPS` (1,000) is the same in both builds.

The `editor_keys` run budgets are these same document limits, not numbers the
harness chose. A put or a `:s` that would pass the document lines or bytes is
refused by the product (`E1240`), but typing, and `.` replaying typed text, is
not checked and can pass them. A run therefore ends, without failing, once the
document passes the line or byte limit of the build it runs in.

## When a run finds a crash

CI (ClusterFuzzLite) uploads the failing input as an artifact. Locally,
libFuzzer writes it to `fuzz/artifacts/<target>/` (untracked).

1. Reproduce: `cargo fuzz run <target> <file>`.
2. Minimise: `cargo fuzz tmin <target> <file>`.
3. Check the input also reproduces, and terminates quickly, under
   `cargo test` (copy it into `regressions/<target>/` and run
   `cargo test -p editor-core replay_corpus_and_regressions`). Fuzz builds
   use the lowered limits under `cfg(fuzzing)`, while the stable replay uses
   the real ones, so an input can behave differently in the two.
4. Fix the bug.
5. Commit the minimised input as `fuzz/regressions/<target>/<short-description>`
   in the same commit as the fix. A crash input never lands in `regressions/`
   without its fix, or `cargo test` goes red.
6. If it cannot be fixed straight away, put it in `fuzz/known/<target>/`
   instead and open an issue. `known/` is replayed only by the `#[ignore]`d
   `replay_known` test (`cargo test -- --ignored replay_known`). Move the file
   to `regressions/` when the fix lands.

A timeout or out-of-memory is a defect too, not noise. A timeout input is
saved the same way, but under `cargo test` it only checks that the harness
terminates; it has no time limit there.

## Coverage at merge

Measured on 2026-11-06 with `nightly-2026-10-23`. Each of the five targets ran for
twelve minutes (one hour of CPU in total, run concurrently) in libFuzzer fork mode:

```sh
cd fuzz
cargo fuzz run <target> "$TMPDIR/corpus4-<target>" corpus/<target> -- \
  -fork=1 -ignore_timeouts=1 -ignore_ooms=1 -max_total_time=720 -max_len=<N> -timeout=10
```

with `-max_len` as in the table above (4096 for `editor_keys`, 65536 for the rest).
Crashes were not ignored. Every run finished with `oom/timeout/crash: 0/0/0`, so no
new finding was recorded and nothing was added to `known/` by this measurement.

Under `cfg(fuzzing)` the limits are lowered (see Fuzz-build limits), so these runs
reached documents within those sizes. This measurement was taken while `editor_keys`
still stopped a run once the cursor line passed 2 KiB or the document passed 65,536
lines; its run budgets are now the product's document limits.

Line coverage of `crates/editor-core/src/` (the `fuzz/` harness module excluded):

| Target                 | Runs       | Final cov / ft  | Line coverage of interest              | Whole of `editor-core` |
| ---------------------- | ---------- | --------------- | -------------------------------------- | ---------------------- |
| `markdown_incremental` | 61,157     | 2718 / 17972    | `markdown/block.rs` 87.89%             | 11.32%                 |
| `ex_command`           | 9,314,383  | 2124 / 10735    | `ex.rs` 97.87%                         | 5.75%                  |
| `editor_keys`          | 2,303,483  | 6529 / 35605    | `vim/` 89.27% (1,556 lines, 8 files)   | 45.50%                 |
| `config_parse`         | 13,504,266 | 3967 / 18104    | not tracked                            | 2.88%                  |
| `state_files`          | 18,677,118 | 3470 / 16464    | not tracked                            | 0.36%                  |

To reproduce a figure, run the target as above, then
`cargo fuzz coverage <target> "$TMPDIR/corpus4-<target>" corpus/<target>` and
`llvm-cov report` from `llvm-tools-preview` on the coverage binary with that
target's `coverage.profdata`, giving it `crates/editor-core/src` as the path to report
on and `--ignore-filename-regex='editor-core/src/fuzz/'`.

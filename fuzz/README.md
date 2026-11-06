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

Run everything below from `fuzz/`. `rustup` picks the pinned nightly there.

## Running a target

```sh
cd fuzz
cargo fuzz run <target> corpus/<target> -- -max_len=<N> -timeout=10
```

Add `-max_total_time=60` for a short run. To keep the committed seeds clean,
give libFuzzer a scratch directory first, so new inputs land there:

```sh
cargo fuzz run <target> "$TMPDIR/corpus-<target>" corpus/<target> -- -max_total_time=60 -max_len=<N> -timeout=10
```

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
  Anything else, or empty input, is ignored.
- `ex_command`: `cmd 0x00 text 0x00 tail`. `cmd` is the ex command (`:s` and
  friends), `text` is the buffer, and a missing part is empty. `tail[0..4]` are
  `cursor, vflag, v0, v1` (zero when missing): the cursor line wraps onto the
  buffer, and when `vflag & 1` is set the last Visual selection is
  `(v0 % n, v1 % n)`. `tail[4..]` are confirm answers, one key each.
- `markdown_incremental`: the initial text up to the first `0x00`, then edit
  records. A record is six bytes `[flags, l0, c0, l1, c1, len]` followed by
  `len` bytes of replacement text. A truncated record ends the sequence.
  Positions wrap onto the buffer and snap down to a char boundary. When
  `flags & 0x0f` is zero the incremental update is handed an edited range that
  cannot fit and must fall back to a full rebuild. The high nibble of `flags`
  places the cursor for cursor-relative reveal.
- `editor_keys`: `text SEP keys`, where `SEP` is the first `0x00` (Mac) or
  `0x01` (Linux) and sets the platform. With neither, all of it is keys on an
  empty text, on the Mac. A leading `R` in the keys opens the document in
  reader mode and is not pressed. The keys use the tests' `<Esc>` notation
  (`keyseq::parse_keys`). The editor has no saver and no state directory, so
  `:w`, `:e!` and closing touch no file.

## Replay under `cargo test`

`editor-core` replays `fuzz/corpus/`, `fuzz/regressions/` and `fuzz/known/`
under plain `cargo test` on stable, so it runs in the normal CI with no
nightly. Every subdirectory of those three must name a target, so keep stray
files and directories out of `fuzz/corpus/`.

## When a run finds a crash

CI (ClusterFuzzLite) uploads the failing input as an artifact. Locally,
libFuzzer writes it to `fuzz/artifacts/<target>/` (untracked).

1. Reproduce: `cargo fuzz run <target> <file>`.
2. Minimise: `cargo fuzz tmin <target> <file>`.
3. Fix the bug.
4. Commit the minimised input as `fuzz/regressions/<target>/<short-description>`
   in the same commit as the fix. A crash input never lands in `regressions/`
   without its fix, or `cargo test` goes red.
5. If it cannot be fixed straight away, put it in `fuzz/known/<target>/`
   instead and open an issue. `known/` is replayed only by the `#[ignore]`d
   `replay_known` test (`cargo test -- --ignored replay_known`). Move the file
   to `regressions/` when the fix lands.

A timeout or out-of-memory is a defect too, not noise. A timeout input is
saved the same way, but under `cargo test` it only checks that the harness
terminates; it has no time limit there.

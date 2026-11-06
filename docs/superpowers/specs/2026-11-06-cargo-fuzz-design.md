# vimmed: fuzzing with cargo-fuzz and ClusterFuzzLite

Date: 2026-11-06
Status: reviewed (builder check done), awaiting owner approval
Scope: fuzz targets over `editor-core`, where they run (ClusterFuzzLite on GitHub Actions), the
seed corpus, and how found crashes become regression tests. No product behaviour changes, beyond
the bug fixes the fuzzers turn up.

## Hypothesis

We believe `editor-core` holds panics, hangs or wrong results that the example-based tests miss,
concentrated in two places: the incremental Markdown reparse (`Blocks::update`) and the vim key
machine driven through `Editor::handle`. Evidence: both are large, stateful and tested only with
hand-picked inputs (`markdown/block.rs` is 1707 lines, the `vim/` module about 5000).
`Blocks::update` also carries a fence re-scan that runs "until the fence state agrees with the
old one again", a table re-scan, and a fallback to a full rebuild. Each is a seam where an
incremental result can quietly drift from a fresh parse. If true, a target that checks
`Blocks::update` against `Blocks::new` after random edits, and a target that feeds random key
sequences to the editor, find at least one real defect between them within the first four weeks
of scheduled batch runs.

We also believe a `.clusterfuzzlite/` setup running on pull requests and on a weekly schedule is
what Scorecard's Fuzzing check detects. If true, Scorecard's Fuzzing check moves from 0 to 10 on
the first Scorecard run after the setup merges to `main`.

This could turn out false in specific ways:
- Four weeks of batch runs find nothing in either target. Then either the code is sounder than we
  think, or the targets are too shallow to reach it, and coverage tells the two apart (see
  Method).
- The defects found are all in the cheap targets (config, ex parsing, state files), and none in
  the two we expect. That would mean the risk sits somewhere other than where we think it does.
- Scorecard does not credit the setup, for example because it looks for something other than the
  `.clusterfuzzlite/` layout we build.

## Method

1. Build the five targets below, the replay test, and the ClusterFuzzLite workflows.
2. Before merging, run `cargo fuzz coverage` locally once per target over the seed corpus plus a
   one hour local run, and record line coverage for `markdown/block.rs`, `vim/` and `ex.rs` in
   the closeout. Under 70% line coverage of `markdown/block.rs` from `markdown_incremental`
   means that target is too shallow to test the hypothesis, and gets reworked before merge.
3. Let the weekly batch job run for four weeks. Count crashes, hangs and oracle failures per
   target, and record each one in `fuzz/regressions/` with its fix.
4. Read the Fuzzing check on the first Scorecard run after merge.

Evidence for: at least one real defect from `markdown_incremental` or `editor_keys` in four
weeks; Fuzzing reads 10. Evidence against: nothing found despite coverage at or above the 70%
bar; defects only in the cheap targets; Fuzzing not credited.

## Desired outcome

- Five fuzz targets build and run under ClusterFuzzLite, on every pull request (short run) and
  weekly (long run).
- Every crash found has a minimised input in `fuzz/regressions/<target>/` that plain
  `cargo test` replays on the pinned stable toolchain, and that test passes after the fix.
- Scorecard's Fuzzing check reads 10.
- The closeout states, per target, coverage and defects found, so the hypothesis can be read
  against the result.

## Intent

The owner wants this for two reasons, both of equal weight: Scorecard credit, and actually
finding bugs in the editor. So the targets carry oracles (checks that the result is right) where
the code offers one, rather than only "did not panic", and the runner is one Scorecard
recognises.

## Goals

- Fuzz the code that parses or reacts to arbitrary input: Markdown, ex commands and `:s`, key
  sequences, the config file, and the on-disk state files.
- Catch panics, hangs and divergence from a stated invariant.
- Keep every crash fixed, through a replayed regression input that runs in ordinary CI.
- Earn Scorecard's Fuzzing credit.

## Non-goals

- Fuzzing the atomic save path. It parses nothing; testing it properly is fault injection, which
  is a different piece of work.
- Fuzzing the renderer (`editor-render`), the shells, the launcher, or third-party crates
  (`toml`, `cosmic-text`, tree-sitter). Third-party code is reached only as far as our own
  targets drive it, and a bug found there is reported upstream, not patched here.
- OSS-Fuzz. Likely not accepted yet for a young project. The ClusterFuzzLite setup carries over
  if we apply later.
- Changing the workspace's pinned stable toolchain.

## Anti-goals

- A required PR check that fails on noise. If the PR fuzz job is flaky or slow enough that people
  learn to ignore it, this has failed even if it technically works.
- Fuzz harnesses that drift from the code they test: harness logic lives next to the code, in
  `editor-core`, and the replay test exercises it on every `cargo test`.
- Any harness touching the real filesystem, the real clipboard, or opening URLs.
- New secrets. The setup runs on the default `GITHUB_TOKEN` only.

## Constraints

- `rust-toolchain.toml` pins stable 1.99.1. cargo-fuzz needs nightly. ClusterFuzzLite's Rust
  base image brings its own nightly, so CI is unaffected. Local fuzzing uses a dated nightly
  pinned in `fuzz/rust-toolchain.toml`, which overrides the root file when run from `fuzz/`.
- Scorecard's Pinned-Dependencies check is already in play: workflow actions are pinned by commit
  SHA, as the existing workflows do, and the Dockerfile base image is pinned by digest.
- The `fuzz/` crate sits outside the workspace (`[workspace]` of its own, own `Cargo.lock`), the
  usual cargo-fuzz layout, so `cargo test --workspace` and the MSRV job never see it.
- Several things a harness needs are private or `pub(crate)` today: `FakeHost` in
  `testkit.rs`, `registers_store::parse`, and `recovery::read_meta` (which also reads the file
  itself rather than taking text).

## Approaches

Considered, for where fuzzing runs:

- **A. ClusterFuzzLite on GitHub Actions (picked by the owner).** Short code-change fuzzing on
  pull requests and a weekly batch with corpus pruning. Brings its own nightly. Detected by
  Scorecard. Costs a Dockerfile and `build.sh` to maintain, and somewhere to keep the grown
  corpus.
- **B. A plain scheduled cargo-fuzz job.** Simplest, but Scorecard does not, as far as we know,
  detect it, so it covers only half the brief.
- **C. OSS-Fuzz.** Best bug-finding (continuous, Google's hardware), but needs Google to accept
  the project, which favours widely used, security-critical software. Not now; A carries over.

Picked: A, because it covers both reasons for doing this without asking for anyone's acceptance.

## Design

### Layout

```
crates/editor-core/
  Cargo.toml              feature `fuzzing` = ["dep:arbitrary"]
  src/fuzz/mod.rs         #[cfg(any(test, feature = "fuzzing"))]: one harness fn per target
  src/fuzz/replay.rs      #[cfg(test)]: replays fuzz/corpus and fuzz/regressions
fuzz/
  Cargo.toml              cargo-fuzz crate, own [workspace], depends on editor-core/fuzzing
  rust-toolchain.toml     dated nightly, for local runs
  fuzz_targets/<target>.rs  each a one-liner calling editor_core::fuzz::<target>(data)
  corpus/<target>/        seed corpus, small hand-picked files, committed
  regressions/<target>/   minimised crash inputs, committed, one file per defect
  README.md               how to run locally, how to triage a crash
.clusterfuzzlite/
  Dockerfile              FROM the OSS-Fuzz Rust base builder, pinned by digest
  build.sh                builds each target with cargo fuzz, copies seed corpora
  project.yaml            language: rust
.github/workflows/
  cflite_pr.yml           code-change mode on pull_request
  cflite_batch.yml        batch and prune modes on a weekly cron, plus workflow_dispatch
```

Each harness function takes `&[u8]` and either returns or panics. The `fuzz_targets` files hold
no logic. That keeps harnesses compiled and checked by clippy on stable through the replay test,
and means a crash input replays identically under `cargo test` and under libFuzzer.

Structured inputs (a sequence of edits, a key sequence) are decoded with the `arbitrary` crate,
an optional dependency enabled only by the `fuzzing` feature and as a dev-dependency.

### The targets

1. **`markdown_incremental`**. Decodes an initial text and a sequence of edits (a start and end
   position and replacement text each, any bytes the buffer accepts: Unicode, tabs, CRLF, long
   lines). Builds `Blocks::new`, then for each edit applies it with `Buffer::replace` and calls
   `Blocks::update`. The oracle is the full `update` contract, which the existing unit test
   `update_matches_fresh_scan_after_edit_sequence` (`markdown/block.rs:1554`) already enforces
   (owner's pick, option 3, after research):
   - **Matches a fresh parse, ids aside.** `canon` (`block.rs:744`) renames ids by first
     appearance and attaches each block's and table's info, so the result must equal
     `Blocks::new` on the same buffer in every per-line kind, code block open, close and
     language, and table header, separator, last row and alignments, with no stale infos left.
     Research found no accepted drift beyond ids: unclosed fences and the outward table re-scan
     are handled, not tolerated. So any other difference is a real defect.
   - **Repaint range.** The returned range covers `edited`; every line outside it keeps its old
     kind exactly, ids included; a table or code block that kept its id but changed its info
     (or, for an unclosed block, its last line) is covered whole. Three of the five commits to
     `block.rs` fixed this range, which is why it is in the oracle.
   - **Contract abuse.** A small share of edits pass `update` deliberately wrong arguments (a
     range or delta that does not fit the old line count). These must return `0..n` and match a
     fresh parse, exercising the full-rebuild fallback the editor never takes today.

   `canon` and the repaint checks move out of `block.rs`'s test module into the fuzz harness
   module, and the existing unit test calls them from there, so the unit test and the fuzz target
   share one oracle and cannot drift apart. Edits that pass correct arguments compute them as the
   test's `edit` helper and the editor (`editor.rs:1776`) do. Every line also goes through `inline::parse_line` and `display::line_display`, and
   the `display_to_src` / `src_to_display` pair is checked against the round-trip property its
   doc comments state.
2. **`ex_command`**. Splits input into a command line and a buffer text. `ex::parse` must not
   panic on any input. When it returns a substitute, `substitute::run` runs over the buffer and
   its edits must apply cleanly; a `confirm_session` driven by an arbitrary answer sequence must
   terminate and keep its positions inside the buffer.
3. **`editor_keys`**. Decodes a starting text and a key sequence (keys, modifiers, typed text),
   opens an `Editor` from text with an in-memory host, a no-op saver and **no state dir**, and
   feeds each key to `Editor::handle`. No state dir means no register sharing, recovery writes
   or locking, and so no background threads or real file IO: the run is deterministic and
   replays identically under `cargo test`. The register and recovery parsers are covered by
   `state_files` instead. `Editor::build` (today `#[cfg(test)]`) becomes available under
   `cfg(any(test, feature = "fuzzing"))` for this. Commands that read or write the file (`:w`,
   `:e!` and similar) must be inert in this setup: with no path and a no-op saver they touch
   nothing, and the builder confirms that. Invariants: no panic; after each key the cursor is a valid
   position in the buffer (`buf.clamp(cursor) == cursor`). Input length is capped (see Hangs)
   so a run stays short.
4. **`config_parse`**. `Config::parse` on arbitrary text, then `tokens` for both themes. Must
   not panic. Warnings are allowed; panics are not.
5. **`state_files`**. The registers parser and the recovery metadata parser on arbitrary bytes.
   Must not panic. For registers, a round trip: anything `parse` accepts must survive
   `serialise` then `parse` unchanged.

### Hangs

libFuzzer's per-input timeout catches hangs. `build.sh` ships an options file per target setting
`max_len` (4096 bytes for `editor_keys`, 64 KiB for the rest) and a `timeout` of 10 seconds. A
timeout in the optimised fuzz build is a defect, whether the loop is infinite or merely
pathological: minimised, committed to `regressions/`, fixed. The replay test asserts only that
the input finishes without panicking. It carries no timing bound, because a debug-build timing
assertion would be flaky. The `timeout` check itself lives in the fuzz build, where
`cargo fuzz run <target> <file>` reproduces it.

### Seed corpus

Small and hand-picked, committed under `fuzz/corpus/<target>/`: the repo's own Markdown (README,
CHANGELOG, a spec or two), a few files covering fences, tables, lists and links; a handful of ex
commands and `:s` forms from the existing tests; a sample `config.toml` covering every key; a
valid registers file and recovery metadata file. A few dozen files in total, not megabytes.

### Where it runs

- **Pull requests** (`cflite_pr.yml`, code-change mode): address sanitizer, a few minutes in
  total across targets. Not a required check at first (see Anti-goals). Revisit once it has run
  for a month without noise.
- **Weekly** (`cflite_batch.yml`): batch mode for about an hour in total, then prune mode to
  keep the corpus small. Also runnable by hand through `workflow_dispatch`.
- **Grown corpus**: kept in GitHub Actions artifacts, ClusterFuzzLite's default when no storage
  repo is set, so no new secret or token is needed. The trade-off: artifacts expire, so a long
  gap between runs loses the grown corpus, and the committed seed corpus is the floor.
- Permissions: each workflow declares the minimum `permissions:` it needs, matching the existing
  workflows' style.

### From crash to regression test

1. A ClusterFuzzLite run fails and uploads the crash input as an artifact.
2. Download it, minimise with `cargo fuzz tmin`, and commit it to
   `fuzz/regressions/<target>/<short-description>`.
3. `replay.rs` already picks up every file in that directory, so `cargo test` now fails.
4. Fix, and the same test passes. `fuzz/README.md` documents these steps.

The replay test runs every file in `fuzz/corpus/` and `fuzz/regressions/` through the matching
harness function, on stable, in the existing test job. No CI change is needed for it.

A crash input only goes into `regressions/` in the same change as its fix, so `main` never goes
red on a known crash. When a crash cannot be fixed straight away, most likely a panic inside a
third-party crate such as `toml`:

- If our code can avoid it (validating or bounding input before handing it over), that is the
  fix, and the input goes into `regressions/` as usual. A panic in `toml` while loading the
  config is still a vimmed crash at start-up, so this is the default route.
- Otherwise the input goes into `fuzz/known/<target>/`, with an issue filed (upstream too where
  the bug is theirs). `replay.rs` runs that directory in a separate `#[ignore]` test, so it is
  kept and runnable but does not fail CI. Moving a file from `known/` to `regressions/` is part
  of fixing it.

(Calls made during the builder check.)

## Left to the builder

- The exact decoding format for each structured input, provided crash inputs stay replayable.
- Which round-trip property `display_to_src` / `src_to_display` actually promise. Read the doc
  comments and assert only what they state.
- How `editor_keys` gets an in-memory host: moving `FakeHost` out of `testkit` or writing a small
  fuzz host. Either way it must never touch the real clipboard, filesystem outside a temp dir,
  or open anything.
- Splitting `recovery::read_meta` so the TOML parsing takes text, with no change to its
  behaviour.
- Exact ClusterFuzzLite action versions (SHA-pinned) and time budgets within the ranges above.

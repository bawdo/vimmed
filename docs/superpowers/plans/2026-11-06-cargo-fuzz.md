# Fuzzing (cargo-fuzz and ClusterFuzzLite) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Five fuzz targets over `editor-core`, replayed on stable by `cargo test`, run by ClusterFuzzLite on pull requests and weekly.

**Architecture:** Harness logic lives in `editor-core` under `src/fuzz/`, compiled for tests and for a `fuzzing` cargo feature. A separate cargo-fuzz crate in `fuzz/` (its own workspace, nightly) wraps each harness in a one-line target. ClusterFuzzLite builds that crate in OSS-Fuzz's Rust image and runs it from two GitHub workflows.

**Tech Stack:** Rust 2024 (workspace pinned to stable 1.99.1), cargo-fuzz and libFuzzer on a dated nightly, ClusterFuzzLite (`google/clusterfuzzlite` actions), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-11-06-cargo-fuzz-design.md`

## Global Constraints

- The workspace toolchain stays stable 1.99.1. Only `fuzz/` uses nightly, through `fuzz/rust-toolchain.toml` with a dated nightly.
- `fuzz/` is outside the workspace: its `Cargo.toml` has its own `[workspace]` and it has its own `Cargo.lock`.
- No new runtime dependencies for `editor-core`. The `fuzzing` feature enables code, not crates (see the decision below).
- Every workflow action is pinned by full commit SHA with a `# vX.Y.Z` comment, as the existing workflows do. The Dockerfile base image is pinned by `@sha256:` digest.
- Workflows declare `permissions: read-all` at the top and the minimum per job. No new secrets: the default `GITHUB_TOKEN` only.
- No harness touches the real filesystem outside a temp dir, the real clipboard, or opens anything.
- libFuzzer options: `max_len` 4096 for `editor_keys`, 65536 for the others. `timeout` is 10 seconds for all of them.
- A crash input goes into `fuzz/regressions/<target>/` only in the same commit as its fix. Unfixable ones go in `fuzz/known/<target>/`, replayed by an `#[ignore]` test.
- Commit subjects are past tense and no longer than 60 characters, with no agent attribution. Subagents stage only; the controller commits and verifies the signature.
- Prose uses Australian English, with no em or en dashes.

**Decision made while planning (inside the spec's builder latitude, "the exact decoding format"):** inputs use a hand-rolled byte format, not the `arbitrary` crate. That keeps seed files plain text you can write by hand (a Markdown file is already a valid `markdown_incremental` seed), and it adds no dependency. Crash inputs replay byte for byte under `cargo test` either way. The spec's mention of `arbitrary` is superseded by this.

## Review Focus

1. **Empty input** (`&[]`). Every harness must return cleanly. Pinned by `every_harness_accepts_empty_and_junk_input` (Task 1). It iterates `TARGETS`, so it covers every target as each one is added.
2. **Invalid UTF-8 and NUL bytes** (`[0xff, 0x00, 0xfe, 0x00, 0x01]`). Every harness must return cleanly; harness decoding must never panic. Pinned by the same test.
3. **A missing or empty seed directory, or a stray directory** in `corpus/`, `regressions/` or `known/` that names no target. Without a check, the replay test would silently replay nothing. It must fail and name the path. Pinned by `replay_corpus_and_regressions` asserting both (Task 1).
4. **CRLF and lone `\r`** in Markdown edits. The buffer's line ending handling meets the incremental parse. Pinned by the `crlf.md` seed and `markdown_incremental_handles_crlf_edits` (Task 3).
5. **`:w`, `:e!`, `:wq` and `:x` in `editor_keys`** must touch no file. Pinned by `editor_keys_touches_no_files` (Task 5).

---

## File Structure

```
crates/editor-core/Cargo.toml                 + [features] fuzzing = []
crates/editor-core/src/lib.rs                 + #[cfg(any(test, feature = "fuzzing"))] pub mod fuzz; and mod keyseq
crates/editor-core/src/keyseq.rs              new: `<Esc>`-notation key parser, moved out of testkit
crates/editor-core/src/fuzz/mod.rs            new: TARGETS, one pub harness fn per target, FuzzHost
crates/editor-core/src/fuzz/replay.rs         new, #[cfg(test)]: replays fuzz/corpus, fuzz/regressions, fuzz/known
crates/editor-core/src/markdown/block.rs      oracle moved out of the test module
crates/editor-core/src/markdown/block/oracle.rs  new: canon, assert_fresh, Snapshot, check_update
crates/editor-core/src/file/recovery.rs       read_meta split into parse_meta(text, key, pid)
crates/editor-core/src/file/registers_store.rs  parse and serialise made pub(crate)
crates/editor-core/src/editor.rs              Editor::build cfg widened to the fuzzing feature
crates/editor-core/src/testkit.rs             keys() uses keyseq
fuzz/Cargo.toml, fuzz/rust-toolchain.toml, fuzz/fuzz_targets/*.rs, fuzz/README.md
fuzz/corpus/<target>/*                        seeds
fuzz/regressions/<target>/.gitkeep            empty until a crash is found
fuzz/known/<target>/.gitkeep
.clusterfuzzlite/{Dockerfile,build.sh,project.yaml}
.github/workflows/{cflite_pr.yml,cflite_batch.yml}
.github/dependabot.yml, typos.toml, CONTRIBUTING.md
```

Target names, used identically everywhere (directories, `TARGETS`, fuzz target binaries, options files): `markdown_incremental`, `ex_command`, `editor_keys`, `config_parse`, `state_files`.

---

### Task 1: The fuzzing feature, the replay test, and `config_parse`

**Files:**
- Modify: `crates/editor-core/Cargo.toml`, `crates/editor-core/src/lib.rs`, `typos.toml`
- Create: `crates/editor-core/src/fuzz/mod.rs`, `crates/editor-core/src/fuzz/replay.rs`, `fuzz/corpus/config_parse/*`, `fuzz/regressions/config_parse/.gitkeep`, `fuzz/known/config_parse/.gitkeep`

**Interfaces:**
- Produces:
  - `pub fn config_parse(data: &[u8])` in `editor_core::fuzz`.
  - `pub const TARGETS: &[(&str, fn(&[u8]))]` in `editor_core::fuzz`. This task lists only `config_parse`; each later task appends its own entry.
  - Feature `fuzzing = []`.
  - `replay.rs` discovers every file under `<repo>/fuzz/{corpus,regressions,known}/<target>/`, skipping dotfiles.

- [ ] **Step 1: Write the failing tests in `fuzz/replay.rs`**

```rust
fn fuzz_root() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz") }

#[test]
fn every_harness_accepts_empty_and_junk_input() {
    for (_, f) in TARGETS {
        f(&[]);
        f(&[0xff, 0x00, 0xfe, 0x00, 0x01]);
    }
}

#[test]
fn replay_corpus_and_regressions() {
    // For each of corpus/ and regressions/: every subdirectory must name a TARGETS entry
    // (panic naming the stray path otherwise). corpus/<target>/ must exist and hold at least
    // one non-dot file for every target. Each file is read and passed to its target's fn,
    // printing the path first (eprintln) so a failure names the input.
}

#[test]
#[ignore = "known, unfixed crashes: see fuzz/README.md"]
fn replay_known() { /* the same walk over known/, without the non-empty requirement */ }
```

- [ ] **Step 2: Run the tests; they fail to compile (`TARGETS` is not defined)**

Run: `cargo test -p editor-core fuzz::`
Expected: compile error, unresolved `TARGETS`.

- [ ] **Step 3: Implement**
  - `Cargo.toml`: `[features] fuzzing = []`.
  - `lib.rs`: `#[cfg(any(test, feature = "fuzzing"))] pub mod fuzz;`.
  - `fuzz/mod.rs`: `config_parse` decodes `data` the same way `Config::load` decodes the file (read `Config::load` and mirror it: if it rejects non-UTF-8, return on a `from_utf8` error). Then it calls `Config::parse(text)`, and `cfg.tokens(theme)` for every `Theme` variant. There is no assertion beyond not panicking. `fuzz/replay.rs` is wired as `#[cfg(test)] mod replay;`.
  - Seeds in `fuzz/corpus/config_parse/`: `full.toml`, every key `config.rs` reads, with valid values; `bad_values.toml`, every key with a wrong type or out-of-range value; `empty.toml`.
  - `typos.toml` `extend-exclude`: add `"fuzz/corpus/**"`, `"fuzz/regressions/**"`, `"fuzz/known/**"`.

- [ ] **Step 4: Run the tests; they pass**

Run: `cargo test -p editor-core fuzz::` then `cargo test -p editor-core fuzz:: -- --ignored`
Expected: both PASS. The ignored run replays an empty `known/` without error.

- [ ] **Step 5: Check the gate**

Run: `cargo xtask ci`
Expected: every step passes. Clippy sees `src/fuzz/` through `--all-targets`.

- [ ] **Step 6: Stage; the controller commits** `Added fuzz harness scaffold and config_parse`

---

### Task 2: `state_files`

**Files:**
- Modify: `crates/editor-core/src/file/recovery.rs:374-383`, `crates/editor-core/src/file/registers_store.rs:173,191`, `crates/editor-core/src/fuzz/mod.rs`
- Create: `fuzz/corpus/state_files/*`, `.gitkeep`s for `state_files`

**Interfaces:**
- Produces:
  - `pub(crate) fn parse_meta(text: &str, key: &str, pid: u32) -> Option<i64>` in `file/recovery.rs`. `read_meta` becomes `fs::read_to_string(meta).ok().and_then(|t| parse_meta(&t, key, pid))`.
  - `registers_store::{parse, serialise}` become `pub(crate)`.
  - `pub fn state_files(data: &[u8])`, appended to `TARGETS`.
- Input format: the first byte selects the parser. `b'r'` means registers, and the rest is the file bytes. `b'm'` means recovery metadata, and the rest is decoded with `String::from_utf8_lossy`. Any other first byte, or empty input, returns.

- [ ] **Step 1: Write the failing tests in `file/recovery.rs` tests**

```rust
#[test]
fn parse_meta_matches_read_meta() {
    let text = "path = \"/a.md\"\npid = 7\nwritten_at_ns = 42\n";
    assert_eq!(parse_meta(text, "/a.md", 7), Some(42));
    assert_eq!(parse_meta(text, "/b.md", 7), None);
    assert_eq!(parse_meta(text, "/a.md", 8), None);
    assert_eq!(parse_meta("not toml", "/a.md", 7), None);
}
```

- [ ] **Step 2: Run it; it fails** (`parse_meta` not found): `cargo test -p editor-core parse_meta`

- [ ] **Step 3: Implement** the split, the visibility changes and the harness.
  - Registers: if `parse(rest)` gives `Some(regs)`, then `assert_eq!(parse(serialise(&regs).as_bytes()), Some(regs))`. Derive `PartialEq` and `Debug` on `Register` and `Regs` if they are missing.
  - Meta: call `parse_meta(&text, "/fuzz.md", 1)`. There is no assertion beyond not panicking.

- [ ] **Step 4: Seeds:** `fuzz/corpus/state_files/regs_valid` (`r` then a two-register TOML file as `serialise` writes it), `regs_wrong_shape` (`r` then a `text = 1` table), `meta_valid` (`m` then the text from Step 1), `meta_wrong_types`.

- [ ] **Step 5: Run** `cargo test -p editor-core`. Expected: PASS, the existing recovery and registers tests included.

- [ ] **Step 6: Stage; the controller commits** `Added state_files fuzz target`

---

### Task 3: `markdown_incremental` and the shared oracle

**Files:**
- Create: `crates/editor-core/src/markdown/block/oracle.rs`, `fuzz/corpus/markdown_incremental/*`, `.gitkeep`s
- Modify: `crates/editor-core/src/markdown/block.rs` (tests module: `canon` at :744, `assert_fresh` at :771, the checks inside `run_edit_sequence` at :1584-1690), `crates/editor-core/src/fuzz/mod.rs`

**Interfaces:**
- Produces, in `markdown/block/oracle.rs`, declared in `block.rs` as `#[cfg(any(test, feature = "fuzzing"))] pub(crate) mod oracle;`. As a child module it may read the private fields of `Blocks`.
  - `pub(crate) fn canon(buf: &Buffer, bl: &Blocks) -> Vec<String>`, moved verbatim.
  - `pub(crate) fn assert_fresh(buf: &Buffer, bl: &Blocks)`, moved verbatim.
  - `pub(crate) struct Snapshot { kinds: Vec<LineKind>, tables: HashMap<TableId, TableInfo>, code_blocks: HashMap<BlockId, CodeBlockInfo> }` and `pub(crate) fn snapshot(bl: &Blocks) -> Snapshot`.
  - `pub(crate) fn check_update(before: &Snapshot, buf: &Buffer, bl: &Blocks, edited: Range<usize>, delta: isize, changed: Range<usize>, label: &str)`. This holds, moved from `run_edit_sequence`: `assert_fresh`; the check that the range covers `edited`; the check that unchanged lines outside the range keep their kinds exactly; and the table and code block repaint checks. The messages carry `label` where they carried `seed {label} step {step}`.
- `run_edit_sequence` keeps its LCG and its `with_code`/`with_table` coverage assertions, and calls `snapshot` and `check_update`. The existing tests must pass unchanged.
- Produces `pub fn markdown_incremental(data: &[u8])`, appended to `TARGETS`.
- Input format: split `data` at the first `0x00`. The part before it is the initial text (`from_utf8_lossy`). With no `0x00`, the whole input is the text and there are no edits. After it come edit records, each `[flags, l0, c0, l1, c1, len]` (six bytes) followed by `len` bytes of replacement text (`from_utf8_lossy`). A truncated record ends the sequence. Positions: line is `l % line_count`, column is `c % (line_len + 1)`, then snapped down to a char boundary. Start and end are ordered as `run_edit_sequence` orders them.
  - `flags & 0x0f == 0` (1 in 16): contract abuse. Pass `update` an `edited` range of `0..line_count + 1 + (l0 as usize)` with the true delta. That range never fits, so it hits the `!fits` fallback. Assert the return is `0..buf.line_count()` and call `assert_fresh`.
  - Otherwise: compute `edited` and `delta` exactly as the test's `edit` helper does, then call `check_update`.
  - After every edit, for every line: call `inline::parse_line(line, kind)`, then `display::line_display` once per `Reveal` variant. For every char boundary `b` of the display text in `0..=len`, `display_to_src(d, b)` must be `<= line.len()` and a char boundary of `line`. For every `c` in `0..=line.len() + 1`, `src_to_display(d, c)` must be `None` when `c > line.len()`. These are only the properties the doc comments at `display.rs:435-457` state.
  - Stop after 256 edit records.

- [ ] **Step 1: Move the oracle; the existing tests are the test**

Run: `cargo test -p editor-core markdown::block`
Expected: PASS before and after the move, with the same test count.

- [ ] **Step 2: Write the failing tests in `fuzz/mod.rs` tests**

```rust
#[test]
fn markdown_incremental_handles_crlf_edits() {
    let mut d = b"# t\r\n```\r\ncode\r\n```\r\n| a |\r\n|---|\r\n".to_vec();
    d.push(0);
    d.extend([1, 1, 0, 1, 3, 2]); d.extend(b"\r\n");        // split the opener
    d.extend([1, 3, 0, 3, 3, 1]); d.extend(b"\r");          // lone CR over the closer
    markdown_incremental(&d);
}

#[test]
fn markdown_incremental_abuse_rebuilds() {
    let mut d = b"a\n```\nb\n".to_vec();
    d.push(0);
    d.extend([0x10, 0, 0, 0, 1, 1]); d.extend(b"x");        // flags & 0x0f == 0
    markdown_incremental(&d);
}
```

- [ ] **Step 3: Run; they fail to compile** (`markdown_incremental` not found).

- [ ] **Step 4: Implement the harness** as specified above.

- [ ] **Step 5: Seeds** in `fuzz/corpus/markdown_incremental/`:
  - copies of `README.md`, `CHANGELOG.md` and `docs/superpowers/specs/2026-11-03-vimmed-v0.9-features-design.md`;
  - `crlf.md` (the text from Step 2);
  - `unclosed_fence.md`;
  - `tables.md` (aligned, ragged, and a pipe row inside a fence);
  - `lists_quotes.md`;
  - `edits_seed` (the `run_edit_sequence` SEED text, `0x00`, then three records inserting `"```rust"`, `"\n| c |\n|:-:|\n"` and `"```"`).

- [ ] **Step 6: Run** `cargo test -p editor-core`. Expected: PASS.

- [ ] **Step 7: Stage; the controller commits** `Added markdown_incremental fuzz target`

---

### Task 4: `ex_command`

**Files:**
- Modify: `crates/editor-core/src/fuzz/mod.rs`
- Create: `fuzz/corpus/ex_command/*`, `.gitkeep`s

**Interfaces:**
- Consumes:
  - `ex::parse(&str) -> Result<ExCmd, String>`;
  - `substitute::lines(SubRange, usize, Option<(usize, usize)>, usize) -> Result<RangeInclusive<usize>, String>`;
  - `substitute::run(&Buffer, &Sub, RangeInclusive<usize>) -> SubResult`;
  - `substitute::confirm_session`, plus `ConfirmSession::{answer, done, current}` and `Answer::from_key`.
- Produces `pub fn ex_command(data: &[u8])`, appended to `TARGETS`.
- Input format: `cmd 0x00 text 0x00 tail`, with each part decoded by `from_utf8_lossy`. A missing part is empty.
  - `tail[0..4]` are `cursor, vflag, v0, v1`, each `0` if missing. Visual is `Some((v0 % n, v1 % n))` when `vflag & 1 == 1`, where `n` is the line count.
  - `tail[4..]` are the confirm answers, one key each, given as `b as char`.

Steps:
1. Call `ex::parse(cmd)`. Anything other than `Ok(ExCmd::Substitute(sub))` returns.
2. Get `lines(sub.range, cursor % n, visual, n)`. On `Err`, return.
3. Run `run(&buf, &sub, r.clone())`. Every edit's range ends must satisfy `buf.clamp(p) == p`. Apply the edits to a clone the same way the editor applies a `:s` result; find the `SubResult` consumer in `editor.rs` and mirror its order. The apply must not panic.
4. `confirm_session(&buf, &sub, r)`: feed answers until `done()`, asserting that `current()`'s line is `< n` before each one. When the answers run out, feed `Answer::from_key('q')`. Then `done()` must be true. Assert at most `answers.len() + 1` answers were fed.

- [ ] **Step 1: Write the failing test in `fuzz/mod.rs` tests**

```rust
#[test]
fn ex_command_substitute_on_last_line() {
    ex_command(b"%s/a/$&$&/gc\0a\naa\na\0\x02\x00\x00\x00yyna"); // cursor on line 2, answers y y n a
    ex_command(b"'<,'>s/x/y/\0x\0\x00\x01\x05\x00");            // visual range clamped by % n
}
```

- [ ] **Step 2: Run; it fails to compile.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Seeds:** one file per `:s` form the existing `ex.rs` and `substitute.rs` tests cover (flags `g i c n`, ranges `%`, `'<,'>`, `N,M`, the `$&` expansion), plus `w`, `q!`, `table 3x4` and an unknown command. A dozen files.
- [ ] **Step 5: Run** `cargo test -p editor-core`. Expected: PASS.
- [ ] **Step 6: Stage; the controller commits** `Added ex_command fuzz target`

---

### Task 5: `editor_keys`

**Files:**
- Create: `crates/editor-core/src/keyseq.rs`, `fuzz/corpus/editor_keys/*`, `.gitkeep`s
- Modify: `crates/editor-core/src/testkit.rs:300-361` (`named` and `keys`), `crates/editor-core/src/editor.rs:374` (`#[cfg(test)]` on `Editor::build`), `crates/editor-core/src/lib.rs`, `crates/editor-core/src/fuzz/mod.rs`

**Interfaces:**
- Produces:
  - `pub(crate) fn parse_keys(seq: &str) -> Vec<(Key, Mods)>` in `keyseq.rs`, declared in `lib.rs` as `#[cfg(any(test, feature = "fuzzing"))] mod keyseq;`. It is the `named` and `keys` escape logic moved verbatim. `testkit::keys` becomes a loop over `parse_keys` that calls `handle(key_with(k, mods))`.
  - `Editor::build` is gated on `#[cfg(any(test, feature = "fuzzing"))]`.
  - `pub(crate) struct FuzzHost { now: Cell<u64> }` in `fuzz/mod.rs`, implementing `Host`. The clipboard is an in-memory `Option<String>`. `open_external` and `spawn_window` do nothing. `now_ns` returns `now`. `waker` returns `Arc::new(|| {})`.
  - `pub fn editor_keys(data: &[u8])`, appended to `TARGETS`.
- Input format: `text SEP keys`.
  - `SEP` is the first `0x00` (Mac) or `0x01` (Linux), whichever comes first. It sets the `Platform`.
  - No separator means all of `data` is keys, on an empty text, on Mac.
  - `keys` is decoded by `from_utf8_lossy` and parsed by `parse_keys`, in the testkit's `<Esc>` notation.
  - If the first key-section byte is `b'R'`, the editor opens in reader mode (`view: true`), and that byte is skipped.
- Setup:
  - `Editor::build(OpenRequest { path: PathBuf::from("/nonexistent-vimmed-fuzz/fuzz.md"), view, display_name: None }, &text, None, Config::default(), platform, Box::new(FuzzHost::default()), false)`.
  - There is no `set_saver` call, so `Unwired` refuses every write and reload, and there is no state dir.
  - Each key is sent as `Input::Key(KeyEvent { key, mods, time_ns })`, with `now` advanced by 16 ms per key.
  - After each key: `let c = ViewSource::cursor(&editor); assert_eq!(editor.buffer().clamp(c), c);`.
  - Stop if the returned `Effects` signal the window closing; find the field in `effects.rs`.

- [ ] **Step 1: Move the key parser; the existing tests are the test**

Run: `cargo test -p editor-core`. Expected: PASS before and after, with the same count.

- [ ] **Step 2: Write the failing tests in `fuzz/mod.rs` tests**

```rust
#[test]
fn editor_keys_touches_no_files() {
    editor_keys(b"# hi\n\0ihello<Esc>:w<CR>:e!<CR>:wq<CR>:x<CR>");
    assert!(!Path::new("/nonexistent-vimmed-fuzz").exists());
}

#[test]
fn editor_keys_reader_mode_and_linux() {
    editor_keys(b"- a\n- b\n\x01Rjjkk<C-d>gg");
}
```

- [ ] **Step 3: Run; they fail to compile.**
- [ ] **Step 4: Implement** `FuzzHost` and the harness.
- [ ] **Step 5: Seeds**, about 15 files, covering:
  - motions with counts (`3w`, `d2j`);
  - text objects (`ci"`, `dap`);
  - registers (`"ayy"ap`);
  - visual and visual-line, with `:s`;
  - undo and redo (`u<C-r>`) and repeat (`.`);
  - list continuation in insert mode;
  - table keys after `:table 2x2<CR>`;
  - `/` search with `n N`;
  - `:h<CR>`;
  - reader mode.

  Each seed is a short Markdown text plus its keys.
- [ ] **Step 6: Run** `cargo test -p editor-core` and `cargo xtask ci`. Expected: PASS.
- [ ] **Step 7: Stage; the controller commits** `Added editor_keys fuzz target`

---

### Task 6: The cargo-fuzz crate and local runs

**Files:**
- Create: `fuzz/Cargo.toml`, `fuzz/rust-toolchain.toml`, `fuzz/fuzz_targets/{markdown_incremental,ex_command,editor_keys,config_parse,state_files}.rs`, `fuzz/README.md`, `fuzz/.gitignore` (`target/`, `artifacts/`, `coverage/`)
- Modify: `CONTRIBUTING.md` (one line pointing to `fuzz/README.md`)

**Interfaces:**
- Consumes: `editor_core::fuzz::<target>(data: &[u8])` from Tasks 1 to 5.
- `fuzz/Cargo.toml`:
  - package `editor-core-fuzz`, `publish = false`, `edition = "2024"`;
  - `[package.metadata] cargo-fuzz = true`;
  - an empty `[workspace]`;
  - dependencies: `libfuzzer-sys = "0.4"` and `editor-core = { path = "../crates/editor-core", features = ["fuzzing"] }`;
  - one `[[bin]]` per target, each with `test = false, doc = false, bench = false`.
- Each target file is `#![no_main] libfuzzer_sys::fuzz_target!(|data: &[u8]| editor_core::fuzz::<target>(data));`.
- `fuzz/rust-toolchain.toml`: `channel = "nightly-YYYY-MM-DD"`. Pick the most recent nightly at least two weeks old on which every target builds.

- [ ] **Step 1: Install the tools** (needs the network: ask the user to approve if the sandbox blocks it). Run `rustup toolchain install nightly-YYYY-MM-DD --profile minimal --component llvm-tools-preview`, then `cargo install cargo-fuzz --locked`.
- [ ] **Step 2: Build.** Run `cd fuzz && cargo fuzz build`. Expected: five binaries, no warnings.
- [ ] **Step 3: Smoke run.** For each target, run `cargo fuzz run <target> "$TMPDIR/corpus-<target>" corpus/<target> -- -max_total_time=60 -max_len=<per Global Constraints> -timeout=10`. libFuzzer writes new inputs to the first directory, so the committed seeds are never touched. Expected: each exits 0 with no crash. If one crashes, follow the README flow (Step 4) in this task, and fix it in a commit of its own before going on.
- [ ] **Step 4: Write `fuzz/README.md`**, covering:
  - installing the pinned nightly and cargo-fuzz;
  - the run command per target, with its `max_len`;
  - the input format for each target (copied from the Interfaces blocks above);
  - triage: download the ClusterFuzzLite artifact, reproduce with `cargo fuzz run <t> <file>`, minimise with `cargo fuzz tmin <t> <file>`, then commit the result as `regressions/<t>/<short-description>` together with the fix (or into `known/<t>/`, with an issue, when it cannot be fixed straight away);
  - that a timeout is a defect too, and replays under `cargo test` only as "terminates".
- [ ] **Step 5: Check the workspace is unaffected.** Run `cargo xtask ci` from the repo root. Expected: PASS, with `fuzz/` built by none of its steps.
- [ ] **Step 6: Stage, including `fuzz/Cargo.lock`; the controller commits** `Added the cargo-fuzz crate`

---

### Task 7: ClusterFuzzLite and the workflows

**Files:**
- Create: `.clusterfuzzlite/Dockerfile`, `.clusterfuzzlite/build.sh`, `.clusterfuzzlite/project.yaml`, `.github/workflows/cflite_pr.yml`, `.github/workflows/cflite_batch.yml`
- Modify: `.github/dependabot.yml` (add cargo for `/fuzz`, and docker for `/.clusterfuzzlite`, with the same schedule, cooldown, grouping and prefix as the existing entries)

**Interfaces:**
- `project.yaml`: `language: rust`.
- `Dockerfile`:
  - `FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:<digest>`, with the digest resolved by `docker buildx imagetools inspect gcr.io/oss-fuzz-base/base-builder-rust:latest` and noted in a comment with the date;
  - `COPY . $SRC/vimmed`, `WORKDIR $SRC/vimmed`, `COPY .clusterfuzzlite/build.sh $SRC/`.
  - The image's `RUSTUP_TOOLCHAIN` env must win over both `rust-toolchain.toml` files; confirm this in Step 2.
- `build.sh`: `cd $SRC/vimmed/fuzz && cargo fuzz build -O`. Then for each target:
  - copy `target/x86_64-unknown-linux-gnu/release/<t>` to `$OUT/`;
  - zip `corpus/<t>/*` into `$OUT/<t>_seed_corpus.zip`;
  - write `$OUT/<t>.options` as `[libfuzzer]\nmax_len = <n>\ntimeout = 10`.
- `cflite_pr.yml`:
  - `on: pull_request` with `permissions: read-all`;
  - one job with `concurrency` per PR (`cancel-in-progress: true`), `timeout-minutes: 30`, and a `sanitizer: [address]` matrix;
  - steps: `google/clusterfuzzlite/actions/build_fuzzers@<sha>` (`language: rust`, `github-token`, `sanitizer`), then `run_fuzzers@<sha>` (`mode: code-change`, `fuzz-seconds: 300`, `github-token`, `sanitizer`).
  - It is not a required check.
- `cflite_batch.yml`:
  - `on: schedule` (cron `17 2 * * 1`, Monday, clear of CI's `23 4` and Scorecard's `30 1`) and `workflow_dispatch`;
  - `permissions: read-all`;
  - job `batch` (`mode: batch`, `fuzz-seconds: 3600`, `timeout-minutes: 90`);
  - job `prune` (`needs: batch`, `mode: prune`, `fuzz-seconds: 600`, `timeout-minutes: 30`).
  - It sets no `storage-repo`, so the corpus lives in Actions artifacts.
- Resolve each action SHA with `gh api repos/google/clusterfuzzlite/git/ref/tags/<tag>` for the current release tag. Pin it as `@<sha> # <tag>`.

- [ ] **Step 1: Write the files above.**
- [ ] **Step 2: Build the image and the fuzzers locally.** Run `docker build -t vimmed-cflite -f .clusterfuzzlite/Dockerfile .`, then `docker run --rm -v "$PWD/build-out:/out" vimmed-cflite compile`. Expected: `/out` holds five binaries, five `_seed_corpus.zip` files and five `.options` files. If the sandbox blocks Docker, ask the user to run these two commands with `!`.
- [ ] **Step 3: Lint the workflows.** Run `actionlint .github/workflows/cflite_pr.yml .github/workflows/cflite_batch.yml`. Expected: no output. Then run `uvx zizmor@1.30.1 .github/workflows/` if the network allows; otherwise rely on CI's zizmor job. Expected: no findings at the CI job's level.
- [ ] **Step 4: Remove `build-out/`** (or add it to `.gitignore`). Run `git status`; nothing from it should be staged.
- [ ] **Step 5: Stage; the controller commits** `Added ClusterFuzzLite PR and weekly fuzzing`

---

### Task 8: Coverage before merge

**Files:**
- Modify: `fuzz/README.md` (a "Coverage at merge" section)

- [ ] **Step 1: Run each target for twelve minutes** (one hour in total, per the spec's Method step 2): `cargo fuzz run <t> "$TMPDIR/corpus-<t>" corpus/<t> -- -max_total_time=720 -max_len=<n> -timeout=10`. Run them in the background. Fix any crash first, per the README flow, in its own commit.
- [ ] **Step 2: Measure coverage.** Run `cargo fuzz coverage <t> "$TMPDIR/corpus-<t>" corpus/<t>` for each target, then `llvm-cov report` (from `llvm-tools-preview`) restricted to `crates/editor-core/src/`. Record the line coverage of `markdown/block.rs` (from `markdown_incremental`), `vim/` (from `editor_keys`) and `ex.rs` (from `ex_command`).
- [ ] **Step 3: Check the bar.** If `markdown/block.rs` is under 70%, stop and report to the user. Per the spec, the target gets reworked before merge.
- [ ] **Step 4: Record** the figures, the nightly used and the date in `fuzz/README.md`. Do not commit the grown local corpus (`fuzz/corpus/` must hold only the seeds; check `git status`).
- [ ] **Step 5: Stage; the controller commits** `Recorded fuzz coverage at merge`

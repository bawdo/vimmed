# Per-key Cost and Resource Budgets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make per-key cost independent of line length and block size, and bound what edits can accumulate, with the fuzz limits derived from the product's own budgets.

**Architecture:** A grapheme-boundary checkpoint index (two lines, both directions, ASCII fast path) owned by `Buffer` and trimmed by `Buffer::replace`; O(1) reveal extents from the block infos; an in-row fast path in `Blocks::update`; a `limits` module of product budgets with lower `cfg(fuzzing)` values, enforced on projected put and `:s` results and on the undo history.

**Tech Stack:** Rust 2024 workspace (stable 1.99.1, `rust-version` 1.92), `unicode-segmentation` 1.13.3, cargo-fuzz on the pinned nightly in `fuzz/rust-toolchain.toml`.

**Spec:** `docs/superpowers/specs/2026-11-06-per-key-cost-and-budgets-design.md` (committed as `2dc5763`). Read it before any task. The design's `Buffer::col_at` is named `grapheme_start` here and returns `Option<usize>`, because its two callers fall back differently (`col_for` to the last grapheme, the vim-off loop to the line end); names were left to the builder.

## Global Constraints

- `cargo xtask ci` green after every commit: fmt, clippy, tests, `lint-name` (the product's name never appears in code or comments), `lint-literals` (user-facing strings live in `crates/editor-core/src/strings.rs`), `shell-lines`, `rename-test`.
- `cfg(fuzzing)` lowers constants only; never a different code path.
- A column answer must always equal `grapheme::display_col`; a reverse answer must always equal today's `motion::col_for` (anti-goal: a column that is ever wrong).
- Budgets refuse growth, never loading. Put and `:s` check the projected result before changing anything (anti-goal: a refusal that half applies).
- Refusal text for size: exactly `E1240: Resulting text too long`.
- Limits (normal / fuzz build): count 999,999,999 / 1,000; document lines 4,194,304 / 65,536; document bytes 268,435,456 (256 MiB) / 1,048,576; undo bytes 67,108,864 (64 MiB) / 1,048,576; changes in one `:s` 262,144 / 4,096. `MAX_UNDO_GROUPS` stays 1,000. `MAX_PUT_BYTES` and `MAX_PUT_LINES` stay.
- Commits: signed through git-guard (message in `.git/AGENT_COMMIT_MSG`, then `git commit -F .git/AGENT_COMMIT_MSG` alone), past tense, subject at most 60 characters, no attribution of any kind. After each: `git verify-commit HEAD`, `git show --name-only HEAD`, clean `git status`. Subagents stage only; the controller commits.
- Never commit to `main`; never push or open a PR.
- Australian English, no em or en dashes.

## Review Focus

1. **Regional-indicator runs across a checkpoint.** A line of 2,000 flags (each 8 bytes) with the cursor past several checkpoints must report the same column as `grapheme::display_col`; restarting inside a pair is the classic bug. Test in Task 2.
2. **An edit that merges a grapheme across the edit start** (typing a combining mark right after a checkpoint byte on a long line). Kept checkpoints must be strictly before the edit's start. Test in Task 3.
3. **Undo and redo restoring a long line**, then a column query. The index must not survive an edit that replaced the line wholesale. Test in Task 3.
4. **Unclosed code block at the end of the buffer** for the O(1) reveal extent (`close == None`). Test in Task 5.
5. **A refused put inside `.` repeat** leaves the document, registers and undo history unchanged, and shows the message once. Test in Task 8.

---

### Task 1: Per-key probe and baseline

**Files:**
- Create: `crates/editor-core/tests/per_key_cost.rs`

**Interfaces:**
- Produces: `cargo test --release -p editor-core --test per_key_cost -- --ignored --nocapture`, printing one line per scenario: `<label> <µs> us/key`.

- [ ] **Step 1: Write the probe** as one `#[test] #[ignore]` fn `per_key_cost`. It opens each document through `Editor::open_with_state(OpenRequest { path, view: false, display_name: None }, Config::default(), Platform::Mac, Box::new(stub), None)` on a file under `std::env::temp_dir()`, with a stub `Host` (no clipboard, `now_ns` fixed, `waker` a no-op `Arc`). It sends `Input::Key(KeyEvent { key, mods: Mods::default(), time_ns: 0 })` and times N keys with `Instant`. Scenarios (labels fixed, they are what the design's table names):
  - `h end 64KiB ascii`, `type end 64KiB ascii`: `"a".repeat(65_536)`, `$` then 300 × `h`; `A` then 300 × `x`.
  - the same two for `"é"` (32,768 of them) and `"漢"` (21,845), labels `e-acute` and `cjk`.
  - `jk end 64KiB e-acute x2`: two such lines, `$`, then 300 alternating `j` and `k`.
  - `l in 200k code block`: ```` ``` ```` + `"code\n" × 200_000` + ```` ``` ````, `100000G`, 300 × `l`.
  - `type in 100k table`: `"| a | b |\n|---|---|\n"` + `"| 1 | 2 |\n" × 100_000`, `50000Gi`, 50 × `x`.
- [ ] **Step 2: Run it** with the command above. Expected: within about 20% of the design's baseline column (1,557 / 735 / 805 / 436 / 573 / 298 / not measured / 183 / 13,403). Paste the output into the commit body.
- [ ] **Step 3: Commit** "Added a per-key cost probe" with the numbers in the body.

### Task 2: `LineCols`, the column index for one line

**Files:**
- Create: `crates/editor-core/src/colindex.rs` (add `mod colindex;` to `lib.rs`, crate-private)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:
  - `pub(crate) const COL_STEP: usize = 1024;`
  - `#[derive(Debug, Default, Clone)] pub(crate) struct LineCols { checkpoints: Vec<(usize, usize)> }`: `(byte, graphemes_before)`, ascending, each a boundary reached by a segmentation run, `(0, 0)` implied.
  - `pub(crate) fn display_col(&mut self, line: &str, col: usize) -> usize`: equals `grapheme::display_col(line, col)` for any char-boundary `col`.
  - `pub(crate) fn grapheme_start(&mut self, line: &str, g: usize) -> Option<usize>`: the byte where grapheme `g` starts, `None` when the line has `g` or fewer graphemes.
  - `pub(crate) fn keep_before(&mut self, col: usize)`: drops checkpoints with `byte >= col`.

- [ ] **Step 1: Write the failing tests**
  - `ascii_is_byte_offset`: `display_col("abc", 2) == 2`; `grapheme_start("abc", 3) == None`; on a 10,000-byte ASCII line no checkpoint is created (assert `checkpoints.is_empty()` after queries).
  - `matches_display_col_everywhere`: for each test string below and every char boundary `c`, `LineCols::default().display_col(s, c) == grapheme::display_col(s, c)`, and the same with one shared `LineCols` queried in random order (a seeded LCG, `x = x.wrapping_mul(6364136223846793005).wrapping_add(1)`, so no new dependency).
  - `grapheme_start_inverts`: for every `g` up to the grapheme count + 2, `grapheme_start(s, g)` equals `s.grapheme_indices(true).nth(g).map(|(i, _)| i)`.
  - `flags_across_checkpoints` (Review Focus 1): `"🇦🇺".repeat(2_000)`, queries at every 8-byte boundary match `grapheme::display_col`.
  - `keep_before_keeps_only_earlier`: after querying the end of a 5,000-byte `é` line, `keep_before(3_000)` leaves only checkpoints with byte `< 3_000`, and later answers still match.
  - Test strings: 5,000 bytes each of `é`, `"e\u{301}"`, flags, `"👩‍👩‍👧"`, Hangul jamo `"\u{1100}\u{1161}\u{11A8}"`, and a mixed one cycling ASCII, `é`, a flag, a ZWJ family and `\r`.
- [ ] **Step 2: Run** `cargo test -p editor-core --lib colindex`. Expected: FAIL (module missing).
- [ ] **Step 3: Implement.** ASCII fast path first: if `line.as_bytes()[..col].is_ascii()`, return `col` (for `grapheme_start`: if the whole line is ASCII, `(g < line.len()).then_some(g)`). Otherwise start from the last checkpoint at or before the target (binary search by byte for `display_col`, by count for `grapheme_start`) and walk `line[start..].grapheme_indices(true)`, offsetting indices by `start`, pushing `(byte, count)` whenever `byte >= last_checkpoint_byte + COL_STEP` and the byte is beyond the last stored checkpoint.
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** "Added a checkpointed grapheme column index".

### Task 3: The index in `Buffer`

**Files:**
- Modify: `crates/editor-core/src/buffer.rs` (struct at `:76-87`, `new` at `:121`, `replace` at `:226-261`, `Clone` at `:89-99`)

**Interfaces:**
- Consumes: `LineCols` (Task 2).
- Produces:
  - `pub fn display_col(&self, pos: Pos) -> usize`
  - `pub fn grapheme_start(&self, line: usize, g: usize) -> Option<usize>`
  - private field `cols: RefCell<[Option<(usize, LineCols)>; 2]>`, most recently used first.

- [ ] **Step 1: Write the failing tests** in `buffer.rs` tests:
  - `display_col_matches_after_random_edits`: an LCG-driven loop of 2,000 steps over a 3-line buffer built from the Task 2 mixed string: each step either queries `display_col` / `grapheme_start` on a random line at a random char boundary and asserts equality with `grapheme::display_col` / `grapheme_indices().nth()`, or makes a random `replace` (inside a line, across lines, inserting `\n`, inserting a combining mark).
  - `combining_mark_at_a_checkpoint` (Review Focus 2): a 4,096-byte `é` line, query the end, insert `"\u{301}"` at the byte of a stored checkpoint, assert the end column still matches.
  - `undo_restores_then_queries` (Review Focus 3): query the end of a long `é` line, `replace` the whole line with ASCII, query, `replace` back with the inverse edit's text, query; each matches `grapheme::display_col`.
  - `line_shift_moves_the_index`: query the end of line 5; insert two lines at line 1; the entry in `cols` now names line 7, and a query of line 7's end matches `grapheme::display_col`.
  - `clone_has_no_index`: clone after a query; the clone's `cols` is empty.
- [ ] **Step 2: Run** `cargo test -p editor-core --lib buffer`. Expected: FAIL.
- [ ] **Step 3: Implement.** Lookups take the entry for the line (moving it to the front) or replace the back entry. In `replace`, after the splice: for each entry, if `start.line == end.line == entry.line` and the inserted text has no `\n`, call `keep_before(start.col)`; else if `entry.line > end.line` (old coordinates), shift it by the line delta; else drop it. `Clone` builds through `Buffer::new`, which starts empty.
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** "Kept a grapheme column index in Buffer".

### Task 4: Callers onto the index

**Files:**
- Modify: `crates/editor-core/src/status.rs:32`, `editor.rs:1012`, `editor.rs:1845`, `editor.rs:1851-1859`, `vim/mod.rs:64`, `vim/visual.rs:131`, `:133`, `:163`, `vim/motion.rs:90`, `:224-231`

**Interfaces:**
- Consumes: `Buffer::display_col`, `Buffer::grapheme_start` (Task 3).
- Produces: `pub fn col_for(buf: &Buffer, line: usize, want: Want) -> usize` in `vim/motion.rs` (was `col_for(line: &str, want)`): `Want::End` as today; `Want::Col(g)` is `buf.grapheme_start(line, g).unwrap_or_else(|| last_col(buf.line(line)))`.

- [ ] **Step 1: Replace the calls.** Each `grapheme::display_col(buf.line(p.line), p.col)` becomes `buf.display_col(p)`. The vim-off loop at `editor.rs:1851-1859` becomes `buf.grapheme_start(target, g).unwrap_or(text.len())` (it may land at the line end, unlike `col_for`). `motion.rs:390` is test code: leave it.
- [ ] **Step 2: Run** `cargo test -p editor-core`. Expected: PASS with no test changed except `col_for` call sites.
- [ ] **Step 3: Run the probe** (Task 1). Expected: the six long-line rows and the `jk` row under 50 µs. If any is not, stop and report the measured numbers before going on.
- [ ] **Step 4: Commit** "Moved column lookups onto the Buffer index", numbers in the body.

### Task 5: O(1) reveal extent

**Files:**
- Modify: `crates/editor-core/src/editor.rs:1891-1906`

**Interfaces:**
- Consumes: `Blocks::code_block(id) -> &CodeBlockInfo { open, close: Option<usize>, .. }`, `Blocks::table(id) -> &TableInfo { header, last, .. }`.

- [ ] **Step 1: Write the failing test** `reveal_extent_of_an_unclosed_block_runs_to_the_end` (Review Focus 4) and `reveal_extent_reads_the_infos` in `editor.rs` tests: `"a\n```\nx\ny"` cursor on `x` gives `1..4`; a closed block gives `open..close + 1`; a table gives `header..last + 1`; a plain line gives `line..line + 1`. (These pass today: this task changes speed, not answers, so they are the guard and the probe is the failing measurement.)
- [ ] **Step 2: Implement** `reveal_extent` with `match self.group(line)`: `Code(id)` → `info.open..info.close.map_or(n, |c| c + 1)`, `Table(id)` → `info.header..info.last + 1`.
- [ ] **Step 3: Run** `cargo test -p editor-core` and the probe. Expected: PASS; `l in 200k code block` under 20 µs.
- [ ] **Step 4: Commit** "Read reveal extents from the block infos".

### Task 6: In-row table fast path

**Files:**
- Modify: `crates/editor-core/src/markdown/block.rs` (`Blocks::update` at `:189`)

**Interfaces:**
- Consumes: `parse_cells`, `TableInfo::aligns.len()` as the table's column count.

- [ ] **Step 1: Write the failing tests** in `block.rs` tests, each comparing against `Blocks::new` (the existing oracle helpers in `markdown/block/oracle.rs`: `check_update`):
  - `editing_a_body_row_in_place_repaints_one_line`: change `| 1 | 2 |` to `| 1 | 22 |` in a 3-row table; `update` returns `s..s + 1`.
  - `a_body_row_losing_a_cell_takes_the_general_path`, `editing_the_header_takes_the_general_path`, `editing_the_separator_takes_the_general_path`, `splitting_a_row_takes_the_general_path`: each still matches a fresh parse.
- [ ] **Step 2: Run** `cargo test -p editor-core --lib markdown::block`. Expected: the first FAILS (returns a wider range), the rest PASS.
- [ ] **Step 3: Implement** the fast path at the top of `update`, after the `fits` check: `delta == 0 && e == s + 1`, `self.kinds[s]` is `TableRow { table }`, and `parse_cells(buf.line(s)).map(|c| c.len()) == Some(self.tables[&table].aligns.len())` → return `s..s + 1` without touching anything else.
- [ ] **Step 4: Run** the tests, then the probe. Expected: PASS; `type in 100k table` under 1,000 µs.
- [ ] **Step 5: Run** `markdown_incremental` for 30 minutes (outside the sandbox; corpus in the session scratchpad): `cd fuzz && cargo fuzz run markdown_incremental <scratch>/corpus-mi corpus/markdown_incremental -- -fork=1 -max_total_time=1800 -max_len=65536 -timeout=10`. Expected: `oom/timeout/crash: 0/0/0`.
- [ ] **Step 6: Commit** "Added an in-row fast path for table edits", fuzz result in the body.

### Task 7: Limits module and count ceiling

**Files:**
- Create: `crates/editor-core/src/limits.rs` (`pub mod limits;` in `lib.rs`)
- Modify: `crates/editor-core/src/vim/count.rs:4-21`

**Interfaces:**
- Produces, each a `pub const usize` with the normal value under `#[cfg(not(fuzzing))]` and the fuzz value under `#[cfg(fuzzing)]`: `MAX_COUNT`, `MAX_DOC_LINES`, `MAX_DOC_BYTES`, `MAX_UNDO_BYTES`, `MAX_SUB_CHANGES` (values in Global Constraints). Doc comment per constant saying what it bounds and why.

- [ ] **Step 1: Write the failing test** in `count.rs`: `count_clamps_at_vim_ceiling`: pushing the digits of 99,999,999,999 gives `effective() == 999_999_999`; `2` × `999999999` (pre and post) also gives `999_999_999`. Mark it `#[cfg(not(fuzzing))]`.
- [ ] **Step 2: Run** `cargo test -p editor-core --lib vim::count`. Expected: FAIL.
- [ ] **Step 3: Implement** `limits.rs`; `count.rs` uses `limits::MAX_COUNT` and drops its own constants and the clippy allow.
- [ ] **Step 4: Run** it again. Expected: PASS.
- [ ] **Step 5: Commit** "Added product limits and vim's count ceiling".

### Task 8: Document budget on put, with a visible refusal

**Files:**
- Modify: `crates/editor-core/src/buffer.rs` (byte count), `vim/operator.rs` (`EditCtx` at `:278`, `put` at `:647`), `editor.rs` (`run` at `:1746`), `strings.rs`

**Interfaces:**
- Produces:
  - `pub fn byte_count(&self) -> usize` on `Buffer`: the sum of line lengths (no line breaks), kept up to date in `replace` and set in `new`.
  - `pub refusal: Option<&'static str>` on `EditCtx`, `None` from `new`.
  - `pub const RESULTING_TEXT_TOO_LONG: &str = "E1240: Resulting text too long";` in `strings.rs`.
  - `pub fn fits(buf: &Buffer, added_lines: usize, added_bytes: usize) -> bool` in `limits.rs`: `line_count + added_lines <= MAX_DOC_LINES && byte_count + added_bytes <= MAX_DOC_BYTES`, with checked arithmetic (overflow is false).

- [ ] **Step 1: Write the failing tests** in `operator.rs` tests (normal build values; build large registers with `"a\n".repeat(..)` rather than typing):
  - `byte_count_tracks_edits` (in `buffer.rs`): after several `replace` calls `byte_count()` equals the sum of `line(i).len()`.
  - `put_past_the_line_budget_is_refused`: a buffer at `MAX_DOC_LINES - 10` lines (via `Buffer::from_text`), `yy` then `20p`: text, registers and `history().can_undo()` unchanged, `message() == Some("E1240: Resulting text too long")`.
  - `put_past_a_per_put_cap_says_e1240`: the existing over-cap tests now also assert the message.
  - `dot_repeat_of_a_refused_put` (Review Focus 5): a put that fits, then `.` until the budget is crossed: the crossing `.` changes nothing and says E1240 once.
- [ ] **Step 2: Run** `cargo test -p editor-core --lib vim::operator buffer`. Expected: FAIL.
- [ ] **Step 3: Implement.** In `put`, after the per-put caps and before `repeat`, refuse when `!limits::fits(ctx.buf, lines * count, bytes * count)`. Each refusal path sets `ctx.refusal = Some(strings::RESULTING_TEXT_TOO_LONG)` and returns `stay`. `Editor::run` calls `self.say(text.to_owned())` after `finish` when `ctx.refusal` is set (read it before `ctx` is dropped).
- [ ] **Step 4: Run** the tests, then `cargo xtask ci`. Expected: PASS.
- [ ] **Step 5: Commit** "Refused puts past the document budget with E1240".

### Task 9: `:s` budgets

**Files:**
- Modify: `crates/editor-core/src/substitute.rs`, `editor/cmdline.rs` (`substitute` at `:277`), `strings.rs`

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)] pub struct Projection { pub changes: usize, pub added_lines: usize, pub added_bytes: usize }` and `pub fn project(buf: &Buffer, sub: &Sub, lines: RangeInclusive<usize>) -> Projection` in `substitute.rs`: counts the matches that would be replaced (all with `g`, else one per line) and the growth if every one is replaced: bytes `max(0, expanded_len - match_len)` summed, lines the `\n` count of each expanded replacement summed. No allocation per match.
  - `pub fn too_many_changes(max: usize) -> String` in `strings.rs`: `format!("E: :s would change more than {max} matches")` (builder may refine the wording; keep the number).

- [ ] **Step 1: Write the failing tests:**
  - `project_counts_without_building` (substitute.rs): `project` of `%s/a/xx/g` on `"aa\na"` is `{ changes: 3, added_lines: 0, added_bytes: 3 }`; with `\r` in the replacement, `added_lines` counts it.
  - `s_past_the_change_cap_is_refused` (cmdline via `ed`): `MAX_SUB_CHANGES + 1` matches, `:%s/a/b/g`: text unchanged, message from `too_many_changes`, no undo step.
  - `s_past_the_byte_budget_is_refused`: a document near `MAX_DOC_BYTES` and a growing `:s`: unchanged, E1240.
  - `confirm_past_a_budget_never_starts`: the same with `gc`: mode stays Normal, no prompt, message set.
- [ ] **Step 2: Run** `cargo test -p editor-core --lib substitute ex::`. Expected: FAIL.
- [ ] **Step 3: Implement** in `Editor::substitute`, after the range resolves and before the `n` / `c` / all branches (the `n` count path is exempt): `let p = substitute::project(..)`; refuse on `p.changes > limits::MAX_SUB_CHANGES` or `!limits::fits(&self.buf, p.added_lines, p.added_bytes)`.
- [ ] **Step 4: Run** the tests, then `cargo xtask ci`. Expected: PASS.
- [ ] **Step 5: Commit** "Bounded :s by a change cap and the document budget".

### Task 10: Undo byte budget

**Files:**
- Modify: `crates/editor-core/src/undo.rs` (`Group` at `:27`, `History` at `:37`, `record` at `:236`, `push` at `:275`, `undo_step`, `redo_step`, `reopen` at `:326`)

**Interfaces:**
- Produces: `pub fn retained_bytes(&self) -> usize` on `History`: the sum over undo and redo groups (not the open one) of, per pair, `inverse.text.len() + forward.text.len() + size_of::<(Edit, Edit)>()`. Groups carry their own `bytes: usize`, summed in `record`.

- [ ] **Step 1: Write the failing tests** in `undo.rs` tests:
  - `retained_bytes_follows_groups`: after recording, undoing, redoing and a new step that clears redo, `retained_bytes()` equals a recomputation from the stacks.
  - `oldest_groups_drop_past_the_byte_budget`: record groups of 1 MiB inserted text each until past `MAX_UNDO_BYTES`; `retained_bytes() <= MAX_UNDO_BYTES` or exactly one group is left; the dropped ones are the oldest (undo order).
  - `a_single_huge_group_is_kept`: one group larger than the budget stays undoable.
  - `reopen_keeps_the_total`: `reopen` then more edits then `end`: total matches a recomputation.
  - The existing `MAX_UNDO_GROUPS` test still passes unchanged.
- [ ] **Step 2: Run** `cargo test -p editor-core --lib undo`. Expected: FAIL.
- [ ] **Step 3: Implement.** `push` adds the group's bytes, subtracts the cleared redo groups', then drops from the front while (`undo.len() > MAX_UNDO_GROUPS` or `retained > MAX_UNDO_BYTES`) and `undo.len() > 1`, setting `base` to each dropped id as today. `reopen` subtracts the reopened group's bytes (its `end` adds them back via `push`).
- [ ] **Step 4: Run** the tests and `cargo xtask ci`. Expected: PASS.
- [ ] **Step 5: Commit** "Budgeted the undo history by bytes".

### Task 11: Harness limits from the product, reference inputs

**Files:**
- Modify: `crates/editor-core/src/fuzz/mod.rs` (`MAX_FUZZ_LINE_BYTES` at `:530`, `MAX_FUZZ_LINES` at `:536`, `drive` loop, the two guard tests), `fuzz/README.md`
- Move: `fuzz/known/editor_keys/long_line_per_key_cost` → `fuzz/regressions/editor_keys/`
- Create: `fuzz/regressions/editor_keys/long_non_ascii_line_per_key_cost`

**Interfaces:**
- Consumes: `limits::MAX_DOC_LINES`, `limits::MAX_DOC_BYTES`, `Buffer::byte_count`.

- [ ] **Step 1: Measure before** (scratch only, not committed): on a throwaway copy with `MAX_FUZZ_LINE_BYTES` set to `usize::MAX`, time `cd fuzz && cargo fuzz run editor_keys ../fuzz/known/editor_keys/long_line_per_key_cost` on `2dc5763`'s product code. Record the time, or "timeout".
- [ ] **Step 2: Change the harness.** Delete `MAX_FUZZ_LINE_BYTES` and its test. The run stops when `line_count() > limits::MAX_DOC_LINES` or `byte_count() > limits::MAX_DOC_BYTES`; rewrite the line-count test to use `limits::MAX_DOC_LINES` (in `cargo test` that is the normal value, so build the over-budget document from text, not keys). Doc comments say these are run budgets taken from the product, because typing is not budget-checked.
- [ ] **Step 3: Create the non-ASCII input**: bytes `é` `0x00` `px5111111` `0p` (same shape as the ASCII one: `printf 'é\0px5111111 0p' > ...`, check with `od -c` against the ASCII file's layout).
- [ ] **Step 4: Run** `cargo test -p editor-core replay_corpus_and_regressions` and `cargo test -p editor-core -- --ignored replay_known`. Expected: PASS; note the replay time of the two long-line inputs.
- [ ] **Step 5: Time both under ASan** with `cargo fuzz run editor_keys <file>`. Expected: each well under 10 s (record the numbers). If either is not, stop and report.
- [ ] **Step 6: Run** `editor_keys` for 30 minutes: `cargo fuzz run editor_keys <scratch>/corpus-ek corpus/editor_keys -- -fork=1 -max_total_time=1800 -max_len=4096 -timeout=10`. Expected: `0/0/0`. Any finding: stop, diagnose, present (CLAUDE.md troubleshooting rules).
- [ ] **Step 7: Update `fuzz/README.md`**: remove the cursor-line limitation paragraph and the `known/` reference to this input; replace the "Fuzz-build limits" section with the limits table from Global Constraints and the run budgets.
- [ ] **Step 8: Commit** "Derived the editor_keys run budgets from the product", with the ASan timings and fuzz result in the body.

### Task 12: Final measurement, changelog

**Files:**
- Modify: `CHANGELOG.md` (`[Unreleased]`)

- [ ] **Step 1: Run the probe** three times; take the median per row. Expected: every row under its target in the design's Desired outcome. Any miss is reported as a miss, with the number.
- [ ] **Step 2: Changelog** under `[Unreleased]`: Fixed (per-key cost on long lines, big code blocks and big tables), Changed (counts clamp at 999,999,999; put and `:s` past the document budget say E1240; the undo history keeps at most 64 MiB plus its newest step; `:s` refuses more than 262,144 changes).
- [ ] **Step 3: Run** `cargo xtask ci`. Expected: all 7 steps OK.
- [ ] **Step 4: Commit** "Recorded the per-key results in the changelog", final probe table in the body.

### Task 13: Merge into the integration branch

- [ ] **Step 1:** Write the merge message (what landed, the before/after table) to `.git/AGENT_COMMIT_MSG`; `git switch integration/v0.9.1`; `git merge --no-ff -F .git/AGENT_COMMIT_MSG feature/per-key-cost-and-budgets`; `git verify-commit HEAD`.
- [ ] **Step 2:** `git branch -d feature/per-key-cost-and-budgets`; check `git config --get-regexp '^branch\.feature/per-key'` and report any leftover (the sandbox cannot edit `.git/config`; the `wombatRnd` opt-out key will remain and needs removing outside the sandbox).
- [ ] **Step 3:** `cargo xtask ci` on the integration branch. Expected: all 7 steps OK.

### Task 14: Bump to v0.9.1

**Files:**
- Modify: `Cargo.toml:30`, `Cargo.lock`, `fuzz/Cargo.lock`, `CHANGELOG.md`

- [ ] **Step 1:** `Cargo.toml` `version = "0.9.1"`; `cargo update --workspace --offline`; `cd fuzz && cargo update -p editor-core -p app-meta -p marks --offline`. Check with `git diff --stat` that only workspace crates changed version.
- [ ] **Step 2:** `CHANGELOG.md`: `## [Unreleased]` stays (empty) above `## [0.9.1] - <today>`, which takes the Unreleased entries; links at the bottom become `[Unreleased]: .../compare/v0.9.1...HEAD` and `[0.9.1]: .../releases/tag/v0.9.1`, `[0.9.0]` kept.
- [ ] **Step 3:** `bash scripts/changelog-version.sh check` and `bash scripts/changelog-version.sh version` (expect `0.9.1`), then `cargo xtask ci`.
- [ ] **Step 4: Commit** "Bumped the version to 0.9.1".

### Task 15: Issue drafts

- [ ] **Step 1:** Write `<scratchpad>/issue-per-key-cost.md` from the hand-off README's draft issue, updated with what landed (link the design doc and the measured table) and `<scratchpad>/issue-*.md` for each non-goal: incremental `Blocks` on undo, `.` and multi-edit; fuzzing with vim off; a rope or piece table.
- [ ] **Step 2:** Hand the owner one `gh issue create --title ... --label ... --body-file ...` command per draft. Do not run them.

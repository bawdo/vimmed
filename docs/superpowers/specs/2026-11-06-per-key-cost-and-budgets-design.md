# Per-key cost and resource budgets: design

Date: 2026-11-06
Branch: `feature/per-key-cost-and-budgets`, off `integration/v0.9.1`
Input: `.superpowers/handoffs/2026-11-06-editor-per-key-cost-and-budgets/`
(`README.md`, `research-codebase.md`, `research-online.md`), the cargo-fuzz
closeout (`2026-11-06-cargo-fuzz-closeout.md`) and a second round of research
summarised under Evidence below.

## Hypothesis

We believe per-key cost grows with line and block size because derived
positions are recomputed from an origin several times a key, with nothing
cached: the display column from byte 0 of the line two to four times a key,
the reveal extent across the whole code block or table twice a move, and the
whole table on every edit inside it. A grapheme-boundary checkpoint index for
the cursor line, an ASCII fast path, reveal extents read from the block infos
and a fast path for in-row table edits will make per-key cost independent of
line length and block size.

If true, `h` and typing at the end of a 64 KiB line, in ASCII, a two-byte
script and a three-byte script, drop below 50 µs a key in release (today 298
to 1,557 µs), and the `editor_keys` harness no longer needs its 2 KiB
cursor-line guard.

It is false if any of these hold after the work:

- typing at the end of a 64 KiB non-ASCII line stays above 100 µs a key in
  release;
- `l` inside a 200,000-line code block stays above 20 µs a key, or typing in
  a 100,000-row table stays above 1 ms a key;
- `fuzz/known/editor_keys/long_line_per_key_cost`, or its non-ASCII
  counterpart, still times out (10 s) under the ASan fuzz build once the
  cursor-line guard is gone.

We also believe the resource findings come from missing budgets on
accumulated state rather than from any single operation, so budgets on the
projected result of put and `:s`, an undo byte budget and a count ceiling
bound a run's memory without a harness guard of its own. That part is false
if a 30-minute `editor_keys` run with the new limits finds a new out-of-memory.

## Method

1. Commit the per-key probe used for the baseline as an `#[ignore]`d test
   (`crates/editor-core/tests/per_key_cost.rs`). It opens a document through
   the public API (`Editor::open_with_state`, a scratch file, a stub `Host`)
   and prints µs per key for each scenario below. Run it in release before
   and after each change:

   ```sh
   cargo test --release -p editor-core --test per_key_cost -- --ignored --nocapture
   ```

2. Make each change below in its own commit, with its tests first, and rerun
   the probe after each.
3. Under the ASan fuzz build (`cd fuzz && cargo fuzz run editor_keys <input>`),
   time `long_line_per_key_cost` and its non-ASCII counterpart before (with
   the cursor-line guard raised on a scratch build only) and after.
4. Run `editor_keys` for 30 minutes in fork mode with the new limits, and
   `markdown_incremental` for 30 minutes (it checks `Blocks::update` against
   a fresh parse, which covers the table fast path).

Evidence for: every scenario under its threshold, both reference inputs
replay well inside the timeout, and the fuzz runs end `oom/timeout/crash:
0/0/0`. Evidence against: any threshold in the Hypothesis missed, a cost that
moves somewhere we did not predict (the per-edit line copy in
`Buffer::replace`, `classify_line` on a long line), or the table fast path
failing the `markdown_incremental` oracle.

## Desired outcome

Checked with the probe in release, against the baseline measured on
2026-11-06 at `0bcdcdf` (macOS arm64):

| Scenario | Baseline µs/key | Target µs/key |
|---|---|---|
| `h` from the end of a 64 KiB ASCII line | 1,557 | < 50 |
| typing at the end of a 64 KiB ASCII line | 735 | < 50 |
| `h` from the end of a 64 KiB `é` line | 805 | < 50 |
| typing at the end of a 64 KiB `é` line | 436 | < 50 |
| `h` from the end of a 64 KiB CJK line | 573 | < 50 |
| typing at the end of a 64 KiB CJK line | 298 | < 50 |
| `j`/`k` between two 64 KiB `é` lines, goal at the end | not measured | < 50 |
| `l` inside a 200,000-line code block | 183 | < 20 |
| typing inside a 100,000-row table | 13,403 | < 1,000 |

And:

- The `editor_keys` cursor-line guard (`MAX_FUZZ_LINE_BYTES`) is gone, and
  the line-count guard is derived from the product's fuzz-build budget.
- `long_line_per_key_cost` has moved from `fuzz/known/` to
  `fuzz/regressions/`, beside a new non-ASCII counterpart.
- A put or `:s` that would take the document past its budget changes nothing
  and says `E1240: Resulting text too long`.
- After any sequence of edits, the undo history holds at most 64 MiB plus its
  newest group.

## Evidence

Baseline, release, throwaway probe (now to be committed): see the table
above. Causes, confirmed against the code at `0bcdcdf`:

- `grapheme::display_col` (`grapheme.rs:81`) walks from byte 0. It runs in
  `status::position` (`status.rs:32`) for the status built before every input
  (`editor.rs:516`) and, when the key asked for no redraw, after it
  (`editor.rs:552`); in `vim_cursor` (`editor.rs:1008`) whenever no goal
  column is set; in `Cursor::at` (`vim/mod.rs:61`) on every motion; in
  `motion.rs:390` and `editor.rs:1845`; and twice in `visual::shape`
  (`vim/visual.rs:131-133`).
- The reverse lookup also walks from byte 0: a vertical motion turns its goal
  column back into a byte offset on the target line with
  `motion::col_for` (`vim/motion.rs:224-231`, `grapheme_indices().nth(g)`)
  or the loop at `editor.rs:1851-1859` (vim off). After `$` or `h` at the
  end of a long line, `j`/`k` between long lines pays O(line) a key.
- `reveal_extent` (`editor.rs:1891`) walks line by line to both edges of the
  code block or table, twice per `move_to` (`editor.rs:1867-1869`) and twice
  per `run` (`editor.rs:1747`, via `finish`). `CodeBlockInfo { open, close }`
  and `TableInfo { header, last }` already hold the edges.
- `Blocks::scan_tables` (`markdown/block.rs:410`) reclassifies every line of
  the run of pipe rows around an edit and reruns `find_tables` over it.
- Counts saturate at `usize::MAX` outside fuzz builds (`vim/count.rs:15`).
  Puts are capped per put (`MAX_PUT_BYTES`, `MAX_PUT_LINES`,
  `vim/operator.rs:31-45`) and refuse silently (`operator.rs:660-675`).
  Undo keeps 1,000 groups with no byte limit (`undo.rs:23`).
- `:s` (since #12) builds one `Match`, one `Edit` and one undo record per
  match, so `:%s/a/b/g` over tens of megabytes of `a` costs gigabytes.

Online research (primary sources, fetched 2026-11-06):

- UAX #29 rev 49 §6.3: the rules "must be evaluated starting at a known
  boundary (such as the start of the text)". A known boundary is as good a
  start as the start of text. unicode-segmentation 1.13.3 treats the start of
  a slice as `sot` (RI parity counted from there). An empirical check found
  no mismatch in 1,475,609 restarts at boundaries. Restarting *inside* a
  regional-indicator pair is wrong, so checkpoints must come out of a
  segmentation run.
- On an ASCII line with no `\r\n` pair, the grapheme index equals the byte
  index (GB3 to GB5, and printable ASCII is Any). Lines here never contain
  `\n`, so no pair can occur inside one. `str::is_ascii` is SIMD or
  word-at-a-time in core.
- Helix, on Ropey, still computes visual columns from the line start and
  carries a `TODO divide long lines into blocks`: a rope does not fix this.
- Emacs alone budgets undo by bytes (`undo-limit` and friends, counting
  saved text and other data), dropping whole change groups oldest first and
  keeping the newest. Vim and Neovim count changes; Helix is unbounded.
- Vim refuses oversized results with `E1240: Resulting text too long` and
  leaves the text unchanged; counts clamp silently at 999,999,999.
- VS Code throws past 256M characters for heap operations
  (`LARGE_FILE_HEAP_OPERATION_THRESHOLD`).

## Goals

- Per-key cost independent of line length for column work, and of block
  size for reveal extents and in-row table edits.
- Budgets on what operations accumulate, with a visible refusal.
- The fuzz-build limits and the `editor_keys` run budget derived from those
  budgets.

## Non-goals

- Incremental `Blocks` rebuilds on undo, `.` and multi-edit operations
  (`editor.rs:1565`, `:1792`, `vim/repeat.rs:260`, `:307`, `:343`).
  Follow-up issue.
- A rope or piece table. Follow-up issue, if ever.
- Fuzzing with vim off. Follow-up issue.
- Render-side walks (`code_bg`, `ordered_display`, `display_to_src`): the
  harness never calls them and they cost per drawn line, not per key.

## Anti-goals

- A column that is ever wrong. A cache that disagrees with
  `grapheme::display_col` by one grapheme is worse than a slow column.
- A budget that refuses to open a file. Budgets refuse growth past them,
  never loading.
- A refusal that half applies an operation. Put and `:s` check the
  projected result before they change anything.
- Moving the fuzz findings out of sight: no harness guard that hides a
  per-key cost the product still has.

## Constraints

- Every change keeps `cargo xtask ci` green (fmt, clippy, tests, the name
  and literal lints: user-facing strings live in `strings.rs`, and the
  product name never appears in code).
- `cfg(fuzzing)` lowers constants only; the code paths are the same.
- `Buffer` is used on one thread (no `Send` or `Sync` requirement found), so
  interior mutability with `Cell` or `RefCell` is acceptable inside it.
- Australian English in comments and messages, no em dashes.

## Approaches considered

1. **ASCII fast path and a per-key memo only.** Cheapest. It fixes the
   reference fuzz input (a line of `e`), but typing on a long non-ASCII line
   still walks the line once a key, and the fuzzer would find that next.
   Rejected.
2. **A rope (Ropey or similar).** Fixes line splices and per-line memory, not
   display columns (Helix shows this), and is a large migration. Rejected
   for this branch.
3. **Boundary checkpoint index for the cursor line, plus an ASCII fast path,
   O(1) reveal extents, a table fast path and product budgets.** Picked by
   the owner on 2026-11-06, with the budget numbers below.

## Design

### 1. Display column

- `grapheme::display_col(line, col)` keeps its meaning and gains a fast
  path: when `line[..col]` is ASCII, the answer is `col`.
- `Buffer` gains a column index, held in a `RefCell`, for up to two lines
  (a vertical motion alternates between the cursor line and its target, as
  does `visual::shape`): per line `{ line, checkpoints: Vec<(byte,
  graphemes_before)> }`, `(0, 0)` implied. Checkpoints are grapheme
  boundaries reached by a segmentation run, at least `COL_STEP` (1,024)
  bytes apart. A query on a third line replaces the least recently used
  entry.
- `Buffer::display_col(pos) -> usize` answers from the fast path, or from the
  last checkpoint at or before `pos.col`: it counts graphemes from there,
  adding a checkpoint each time it passes `COL_STEP` bytes beyond the last
  one, so later queries further along start closer.
- `Buffer::col_at(line, graphemes) -> usize` is the reverse: the byte offset
  of the grapheme with that index (the last grapheme's start when the line
  is shorter, as `col_for` does today). It finds the last checkpoint whose
  count is at most `graphemes` and walks on from there, adding checkpoints
  the same way. On an ASCII line it is `min(graphemes, last_col)`.
- `Buffer::replace` keeps the index honest:
  - an edit confined to the index line keeps the checkpoints strictly before
    the edit's start column and drops the rest (a boundary depends only on
    the text before it and the character at it, so it survives an edit after
    it);
  - an edit wholly above the index line moves it by the change in line
    count;
  - any other edit touching the index line drops the index.
- A cloned `Buffer` starts with no index.
- Every caller moves to the index: `status::position`, `Editor::vim_cursor`,
  `Cursor::at`, `motion.rs:390`, `editor.rs:1845` and `visual::shape` to
  `Buffer::display_col`; `motion::col_for` and the vim-off loop at
  `editor.rs:1851-1859` to `Buffer::col_at`. (Added after the builder
  check: the reverse direction was missing from the first draft.)
- The status is still built before every input and compared after; with the
  index each build costs one lookup of at most about `COL_STEP` bytes, so the
  double build is left as it is. (The proposal said "computed once per key";
  measuring after the index will show whether that is still worth doing.)

### 2. Reveal extent

`reveal_extent` reads the block's edges: a code block's `open..=close` (to the
end of the buffer while unclosed), a table's `header..=last`.

### 3. Table fast path

In `Blocks::update`, before the general rescan: when the edit replaced one
line with one line (`delta == 0`, `e == s + 1`), the old kind was
`TableRow { table }` (a body row, not the header or separator), and the new
text's `parse_cells` count equals the table's column count, the table's
extent and alignment cannot change (`find_tables`: data rows continue while
the cell count matches, and a data row is never a header). The line keeps its
kind, no other line is touched, and the repaint range is the line itself.
Anything else takes the existing path.

### 4. Budgets

A new module (`limits.rs`, say) holds the product limits, each with a
lower `cfg(fuzzing)` value:

| Limit | Normal | Fuzz build |
|---|---|---|
| Count ceiling (`vim/count.rs` `MAX_COUNT`) | 999,999,999 | 1,000 |
| Document lines | 4,194,304 (2^22) | 65,536 |
| Document bytes | 256 MiB (2^28) | 1 MiB |
| Undo bytes | 64 MiB (2^26) | 1 MiB |
| Changes in one `:s` | 262,144 (2^18) | 4,096 |

The per-put caps (`MAX_PUT_BYTES`, `MAX_PUT_LINES`) stay: they bound the
transient copies one put makes (about six times its size).

- **Counts** clamp silently, as vim does.
- **Document budget.** `Buffer` keeps a running byte count (updated in
  `replace`) beside its line count. Put checks the projected lines and bytes
  (`text` lines and bytes times the count, added to the document) before it
  edits, and `:s` checks its projected growth (the sum over matches of the
  expanded replacement's length less the match's) the same way. Past either
  budget, or past a per-put cap, the operation changes nothing and the editor
  says `E1240: Resulting text too long`. Doubling and `.` both go through
  put, so both are bounded. Typing is not checked: it grows a document a key
  at a time.
- **Refusals reach the user** through a new field on `EditCtx` (a refusal
  message the operation sets), which `Editor::run` shows with `say` after the
  operation returns.
- **Undo budget.** `History` keeps a running total of what its groups hold:
  for each recorded pair, both texts' lengths plus `size_of::<(Edit, Edit)>()`.
  Undo and redo groups both count. When `push` files a group and the total
  is over the budget, it drops whole groups from the oldest end until it is
  within budget or only the newest group is left. The save point follows the
  existing rules for a dropped group. Dropping is silent, as in vim and
  Emacs below their outer limit. `MAX_UNDO_GROUPS` (1,000) stays as well, so
  small edits keep vim's step limit; whichever limit is reached first drops
  the oldest group. The totals stay right wherever groups move: `reopen`,
  undo and redo moving a group between stacks, and redo being cleared.
- **`:s` match cap.** `:s` counts its matches before building anything (a
  count is O(document) time and O(1) memory). More than the cap, and it
  refuses with a message from `strings.rs` (wording is the builder's, in the
  `E…:` style the project uses) and changes nothing. The projected growth
  checks lines as well as bytes, since `\r` adds lines.
- **`:s///c`** is checked the same way before the session starts, assuming
  every match is replaced. A session that starts can therefore never be
  refused halfway, whatever is answered.

### 5. Fuzz limits and the harness

- The `cfg(fuzzing)` values above replace the separate fuzz constants
  (`MAX_COUNT` 1,000 stays as is).
- `MAX_FUZZ_LINE_BYTES` goes. `MAX_FUZZ_LINES` becomes the product's
  fuzz-build document line budget, and the harness also stops a run past the
  fuzz-build document byte budget. Those can be passed by typing, which the
  budget does not check, so they stay run budgets rather than assertions.
  (The proposal said "replaced by assertions"; typing and `.` replaying
  typed text can legitimately pass the budget, so an assertion would be
  false.)
- `long_line_per_key_cost` moves to `fuzz/regressions/editor_keys/`, and a
  non-ASCII counterpart (the same shape with a two-byte character) joins it,
  once both replay well inside the timeout under ASan.
- `fuzz/README.md` drops the cursor-line limitation and documents the
  limits table.

### 6. Tests

- Property test: `Buffer::display_col` equals `grapheme::display_col`, and
  `Buffer::col_at` equals today's `col_for`, after random edits and random
  queries on random lines, over strings mixing ASCII, multi-byte
  letters, combining marks, regional indicators, ZWJ sequences and Hangul.
- Unit tests for each index invalidation rule, the ASCII fast path, the
  O(1) reveal extent on unclosed blocks and the table fast path's
  preconditions (header and separator edits, cell-count changes and line
  splits take the general path).
- Budget tests: a put and a `.` repeat refused at the document budget with
  `E1240` and no change; a `:s` refused at the match cap and at the byte
  budget; the undo total staying within budget plus the newest group; a
  count past 999,999,999 clamped.

## Left to the builder

- Names, module boundaries and the exact `:s` cap message.
- Whether the column index lives in `buffer.rs` or a module of its own.
- The probe's exact scenarios beyond the eight in the Desired outcome.

# Incremental `Blocks` on undo, redo, `.` and multi-edit: design

Date: 2026-11-07
Branch: `feature/incremental-blocks`, off `integration/v0.9.2`
Issue: #13. Deferred as a non-goal of
`2026-11-06-per-key-cost-and-budgets-design.md`.

## Background

Single edits keep the Markdown block structure current with
`Blocks::update`, whose cost follows the edit. Four paths throw it away and
call `Blocks::new`, a scan of the whole document:

- `Editor::undo` (undo and redo), once per key, whatever the step touched.
- `Editor::finish` when an operation made more than one edit (`:s` over
  several lines, a list plan, a table command, `.` replaying typed text).
- `vim/repeat.rs`, during `.` replay: once for `ic`, once for a list key and
  once per list or table key in the replayed typing.

## Done when

- None of those paths scans the whole document for an ordinary edit: the
  blocks follow each edit with `Blocks::update`, in the buffer that edit
  left, so each update sees exactly the contract it expects.
- A run of edits whose updates have already re-scanned as many lines as the
  document holds stops following and rebuilds once, when the blocks are next
  needed, so a pathological run (every edit toggling a fence) costs about two
  full scans, not one per edit.
- In tests, every key leaves `Editor::blocks` equal to a fresh scan
  (`oracle::assert_fresh`), so the whole editor test suite is the oracle.
  The `editor_keys` fuzz harness checks the same on small documents.

## The change

### `BlockSync`

A small tracker in `vim/operator.rs`, beside `Touched`, owning a
`&mut Blocks`:

- `edited(buf, t)`: after one edit. While following, calls
  `Blocks::update(buf, t.start_line..t.new_end_line + 1, t.delta)` and adds
  the length of the range it returned to the lines spent. Once the spent
  lines pass the buffer's line count it goes stale and stops updating.
- `blocks(buf)`: the blocks, rebuilt first if stale.
- `changed()`: the range the only edit returned, when there was exactly one,
  for `finish`'s precise repaint.

The returned range is a fair measure of an update's work: the fence rescan
runs while the fence state differs from the old one, which changes the kind
of every line it passes, and the table rescan covers runs of pipe rows that
are repainted whole.

### Callers

- `EditCtx` holds a `BlockSync`; `replace` feeds it each edit. `repeat.rs`
  asks it for the blocks instead of calling `Blocks::new`.
- `History::undo_step` and `redo_step` take a callback run after each edit
  they apply, with that edit's `Touched`. `Editor::undo` feeds it to a
  `BlockSync`. `History::undo` and `redo` (tests) pass a no-op.
- `Editor::finish` no longer updates the blocks: they are current. One edit
  repaints `changed()` as before; several still repaint everything.

## Non-goals

- Repaint ranges for several edits: still `Redraw::All`. The cost here is
  the scan, not the repaint, which is bounded by the window.
- Search highlights on several edits: `Search::refresh` is lazy already.

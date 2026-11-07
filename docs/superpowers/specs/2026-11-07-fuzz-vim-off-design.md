# Fuzzing `editor_keys` with vim off: design

Date: 2026-11-07
Branch: `feature/fuzz-vim-off`, off `integration/v0.9.2`
Issue: #14. Deferred as a non-goal of
`2026-11-06-per-key-cost-and-budgets-design.md`.

## Background

The harness builds every editor with `Config::default()`, which has vim on.
Vim mode is fixed by `Config::vim` when the editor is built and never changes
after, so the vim-off paths are never reached. The `editor_keys` profile from
the merge-time coverage run (`fuzz/README.md`, Coverage at merge) covers
1.12% of `nonvim.rs` (3 of 268 lines) and 2.57% of `editor/nonvim_keys.rs`
(9 of 350), against 89.27% of `vim/`.

## Done when

- A vim-off run is selected by the separator byte, and the harness checks the
  selection's ends and the mode as well as the cursor on every vim-off key.
- Every committed `editor_keys` input (corpus and regressions) means what it
  meant before the change, checked by the replay test passing unchanged.
- Vim-off seeds are in `fuzz/corpus/editor_keys/`.

## The change

### Input format

The separator byte sets vim mode as well as the platform:

| Separator | Platform | Vim |
| --------- | -------- | --- |
| `0x00`    | Mac      | on  |
| `0x01`    | Linux    | on  |
| `0x02`    | Mac      | off |
| `0x03`    | Linux    | off |

The separator is the first byte of `0x00..=0x03`. With none, all of the input
is keys, on an empty text, on the Mac, with vim on, as now. A leading `R` in
the keys still opens the document in reader mode, so reader mode with vim off
is `text 0x02 R keys`.

Why the separator and not a leading marker in the keys (the issue's two
suggestions):

- A marker in the keys takes that key away as a first key, as `R` already
  does to vim's Replace mode. The separator takes no key away.
- A second marker would need an ordering rule with `R`, and tests for the
  wrong order. With the separator, `R` stays where it is.
- One flipped byte turns any vim-on input into its vim-off twin, so every
  existing seed is a single mutation from a vim-off one.
- No committed `editor_keys` input has `0x02` or `0x03` in its text (checked
  over all 25), so all of them keep their meaning.

The cost: a document text can no longer hold `0x02` or `0x03`. They are
control characters with no meaning in markdown.

### Harness

`split_input` returns a small struct in place of its four-value tuple (text,
keys, platform, vim, reader). `drive` builds the editor with
`Config { vim, ..Config::default() }`, as `testkit::plain_with_host` does.

### Invariants

Kept for both modes: after every key the cursor sits on a char boundary, and
the run ends when a key closes the window or the document passes
`limits::MAX_DOC_LINES` or `MAX_DOC_BYTES`.

Added, with vim off, after every key:

- **Selection ends on char boundaries.** Both `anchor` and `head` of
  `ViewSource::selection()`, when there is one. With vim off the selection is
  most of the state besides the cursor that keys move (Shift with the
  arrows, word and line motions, the wrap toggles), and nothing checks it
  today.
- **No vim mode leaks in.** The mode is never `Visual`, `VisualLine`,
  `Search` or `Confirm`, and is `Normal` only in reader mode. The vim-off
  editor does leave `Insert` on purpose: the link prompt is `Command`, the
  close prompt is `Prompt` and reader mode is `Normal` (`editor/link.rs`,
  `editor/prompt.rs`, `editor/reader.rs`). A key that reached the vim engine
  with vim off would show up as one of the forbidden modes.

### Seeds

New files in `fuzz/corpus/editor_keys/`, each with vim off, spread over both
platforms: arrows and Shift selection, word and line motions (`<M-Left>`,
`<D-Right>` on the Mac, `<C-Left>`, `<C-End>` on Linux), Primary+B and
Primary+I over a selection, the find bar (Primary+F, typing, Enter,
Shift+Enter, Esc), the link prompt, undo and redo runs, and one reader-mode
input.

### Docs and tests

- The `editor_keys` and `split_input` doc comments, and the `editor_keys`
  entry under Input formats in `fuzz/README.md`.
- `editor_keys_splits_its_input` and `editor_keys_reader_mode_and_linux`
  updated for the struct, plus a test for each new separator and for a
  vim-off reader run.
- A test that a vim-off run actually takes the vim-off path: the same keys
  give a different document with vim on and with vim off (`ihi<Esc>` types
  `ihi` with vim off).
- A CHANGELOG line.

## Non-goals

- A libFuzzer dictionary of key names.
- Fuzzing mouse input with vim off. The harness drives keys only, in both
  modes.
- Vim-off oracles beyond the two invariants above.

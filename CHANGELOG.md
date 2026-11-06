# Changelog

All notable changes to vimmed are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Pasting with Cmd+V (Ctrl+V on Linux) is checked against the same limits as a put: a paste of more than 64 MiB or 262,144 lines, or one that would take the document past 4,194,304 lines or 256 MiB, changes nothing and says `E1240: Resulting text too long`. A paste over a selection counts only what it adds beyond the selection.

### Fixed

- Typing at the start of a row in a big table, before its leading `|`, no longer takes time in proportion to the table's size: about 1 µs a key in a 100,000-row table (was about 10 ms). Only the first key, which takes the row out of the table, still costs that much.
- `:s///n` counts its matches one at a time, so counting on a very long line no longer takes memory in proportion to the number of matches.

## [0.9.1] - 2026-11-06

### Added

- `\r` in a `:s` replacement inserts a line break, as in vim, and `\\` is a backslash. `\n` stays as typed (vim would insert a NUL).

### Changed

- A put that would add more than 262,144 lines now adds nothing, as one over the existing 64 MiB limit does, so a mistyped count cannot exhaust memory.
- Counts clamp at 999,999,999, as in vim.
- A put, or a `.` repeating one, that would take the document past 4,194,304 lines or 256 MiB changes nothing and says `E1240: Resulting text too long`, as vim does. A put past the per-put limits now says so too, where it used to refuse silently.
- `:s` that would take the document past the same limits changes nothing and says `E1240`, and `:s` refuses more than 262,144 changes at once. Both are checked before anything changes, and before a `:s///c` session starts, so a session is never refused halfway.
- The undo history keeps at most 64 MiB of changes plus its newest step, dropping the oldest steps first, as well as at most 1,000 steps.

### Fixed

- Keys at the end of a long line (moving, typing, `j` and `k` between long lines) no longer take time in proportion to the line's length: about 1 to 10 µs a key on a 64 KiB line, down from 230 to 1,250 µs.
- Moving inside a big code block, and typing inside a cell of a big table, no longer take time in proportion to the block's size: under 1 µs a key in a 200,000-line code block (was 174) and about 1 µs in a 100,000-row table (was about 10 ms).
- Moving by grapheme (`h`, `l`, `b`, `w` and friends) on a long line no longer takes time proportional to the square of the line length.
- `:s` with a line break in the replacement puts later replacements in the right place, with or without `c`, and leaves the cursor where vim does.
- `:s` no longer drops a stray carriage return that sits next to a line break it inserts.

## [0.9.0] - 2026-11-06

### Added

- `vimmed FILE` opens one Markdown file in its own native window on macOS and Linux, and waits until the window closes. `-b` returns straight away and `--view` opens the reader view.
- Vim editing: Normal, Insert, Visual and Visual Line modes, motions, operators, counts, `.` repeat, search, `:s`, and registers shared between windows.
- Markdown rendered in place, with only the element under the cursor showing its syntax: headings, lists, tasks, tables, code blocks with highlighting, links and local images.
- Safe file handling: atomic saves, recovery copies, and detection of changes made on disk by other tools.
- `~/.config/vimmed/config.toml` for vim mode, autosave, fonts, column width and theme, with a light and dark theme that follows the system.

[Unreleased]: https://github.com/bawdo/vimmed/compare/v0.9.1...HEAD
[0.9.1]: https://github.com/bawdo/vimmed/releases/tag/v0.9.1
[0.9.0]: https://github.com/bawdo/vimmed/releases/tag/v0.9.0

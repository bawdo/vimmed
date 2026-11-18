# Changelog

All notable changes to vimmed are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- The text column grows with the window, up to 900 points (was a fixed 660). The side margins scale with the window too: each is 5% of its width or 24 points, whichever is wider, set by the new `space.page.side.ratio` token and the existing `space.page.side`. `column_width` (`space.column`) still sets the most the column can take.
- A table wider than the text column breaks out of it, centred, up to the new `space.column.wide` token (1600 points by default) within the same margins. A table that fits keeps its natural width at the column's left edge and is never stretched.
- `:q`, `:q!` and `:cq` run in reader view, so you can close the document without leaving it first. `:q` still refuses unsaved changes with `E37`. Commands that write (`:w`, `:wq`, `:x`) are still refused, and the message now reads `reader view: only :view (or :reader) and :q run here`.
- The app icon (the macOS dock and the Linux hicolor icons) shows ":VMd" in black inside the green enso (was white).
- When a table still doesn't fit, each column keeps room for its longest word and the space left over goes to the columns that want more, so a short column such as an ID no longer wraps because a long one sits next to it. Words break only when the longest words alone don't fit.

### Fixed

- Images now size to the text column as laid out, not to `space.column`, so they grow and shrink with the window instead of staying at the configured width.

## [0.9.3] - 2026-11-18

### Added

- `:view` (or `:reader`) turns reader view on, and off again. In reader view the command line now opens, but runs only these two commands; anything else says `reader view: only :view (or :reader) runs here`. Unlike vim's `:view`, it takes no file name.
- A page icon at the right end of the status bar toggles reader view when clicked, with a tooltip naming the Cmd+Shift+R (Ctrl+Shift+R on Linux) shortcut. It shows in the new `color.status.reader` token (the accent blue by default) while reader view is on.

## [0.9.2] - 2026-11-07

### Changed

- Pasting with Cmd+V (Ctrl+V on Linux) is checked against the same limits as a put: a paste of more than 64 MiB or 262,144 lines, or one that would take the document past 4,194,304 lines or 256 MiB, changes nothing and says `E1240: Resulting text too long`. A paste over a selection counts only what it adds beyond the selection.

### Fixed

- Typing at the start of a row in a big table, before its leading `|`, no longer takes time in proportion to the table's size: about 1 µs a key in a 100,000-row table (was about 10 ms). Only the first key, which takes the row out of the table, still costs that much.
- `:s///n` counts its matches one at a time, so counting on a very long line no longer takes memory in proportion to the number of matches.
- Undo, redo, `.` and commands that change several places at once (`:s` over many lines, list and table keys) no longer rescan the whole document: in a 200,000-line document, undo and redo take about 0.1 ms a key (were about 3 ms), and `.` repeating a list continuation about 0.2 ms (was about 6 ms).

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

[Unreleased]: https://github.com/bawdo/vimmed/compare/v0.9.3...HEAD
[0.9.3]: https://github.com/bawdo/vimmed/releases/tag/v0.9.3
[0.9.2]: https://github.com/bawdo/vimmed/releases/tag/v0.9.2
[0.9.1]: https://github.com/bawdo/vimmed/releases/tag/v0.9.1
[0.9.0]: https://github.com/bawdo/vimmed/releases/tag/v0.9.0

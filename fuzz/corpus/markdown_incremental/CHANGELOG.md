# Changelog

All notable changes to vimmed are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0] - 2026-11-06

### Added

- `vimmed FILE` opens one Markdown file in its own native window on macOS and Linux, and waits until the window closes. `-b` returns straight away and `--view` opens the reader view.
- Vim editing: Normal, Insert, Visual and Visual Line modes, motions, operators, counts, `.` repeat, search, `:s`, and registers shared between windows.
- Markdown rendered in place, with only the element under the cursor showing its syntax: headings, lists, tasks, tables, code blocks with highlighting, links and local images.
- Safe file handling: atomic saves, recovery copies, and detection of changes made on disk by other tools.
- `~/.config/vimmed/config.toml` for vim mode, autosave, fonts, column width and theme, with a light and dark theme that follows the system.

[Unreleased]: https://github.com/bawdo/vimmed/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/bawdo/vimmed/releases/tag/v0.9.0

//! Every user-visible string. Nothing else in the editor spells out text a
//! person reads.

/// The mode names shown in the status bar.
pub const MODE_NORMAL: &str = "NORMAL";
pub const MODE_INSERT: &str = "INSERT";

/// The title of a document with no file name.
pub const UNTITLED: &str = "Untitled";

/// Shown when a file exists but could not be read, so it opened empty.
pub const READ_FAILED: &str = "Couldn't read the file, so it opened empty";

/// Shown for a second when a search wraps past the end of the document (or,
/// going backward, the start).
pub const SEARCH_WRAPPED: &str = "search wrapped";

/// Shown when a search finds no match.
pub fn pattern_not_found(pattern: &str) -> String {
    format!("E486: Pattern not found: {pattern}")
}

/// Shown in an image placeholder when the file is missing and the image has no alt text.
pub const IMAGE_NOT_FOUND: &str = "image not found";

/// `:q` on a buffer with unsaved changes.
pub const NO_WRITE_SINCE_CHANGE: &str = "E37: No write since last change (add ! to override)";

/// A command line that is no command.
pub fn not_a_command(input: &str) -> String {
    format!("E492: Not a command: {input}")
}

/// Flags after `:s` that are none of `g i c n`, from the first bad one.
pub fn trailing_characters(rest: &str) -> String {
    format!("E488: Trailing characters: {rest}")
}

/// A line range that runs past the end of the document.
pub const INVALID_RANGE: &str = "E16: Invalid range";

/// `:'<,'>` with no selection to take the lines from.
pub const MARK_NOT_SET: &str = "E20: Mark not set";

/// `:s//` with no search to take the pattern from.
pub const NO_PREVIOUS_PATTERN: &str = "E35: No previous regular expression";

/// What a `:s` did: `1 substitution on 1 line`, `<N> substitutions on <L>
/// lines`, with vim's singular for one line (`3 substitutions on 1 line`).
pub fn substitutions(count: usize, lines: usize) -> String {
    counted(count, "substitution", "substitutions", lines)
}

/// What `:s///n` counted: `<N> matches on <L> lines`, `1 match on 1 line`.
pub fn matches(count: usize, lines: usize) -> String {
    counted(count, "match", "matches", lines)
}

fn counted(count: usize, one: &str, many: &str, lines: usize) -> String {
    let noun = if count == 1 { one } else { many };
    let line = if lines == 1 { "line" } else { "lines" };
    format!("{count} {noun} on {lines} {line}")
}

/// The question `:s///c` asks at each match.
pub fn confirm_replace(replacement: &str) -> String {
    format!("replace with {replacement}? [y/n/a/q/l]")
}

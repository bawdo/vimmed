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

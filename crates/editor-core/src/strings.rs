//! Every user-visible string. Nothing else in the editor spells out text a
//! person reads.

/// The mode names shown in the status bar.
pub const MODE_NORMAL: &str = "NORMAL";
pub const MODE_INSERT: &str = "INSERT";
pub const MODE_VISUAL: &str = "VISUAL";
pub const MODE_VISUAL_LINE: &str = "V-LINE";
/// The modes whose line is typed in the status bar, or that ask there, show
/// that instead; these names stand only when there is nothing to show.
pub const MODE_SEARCH: &str = "SEARCH";
pub const MODE_COMMAND: &str = "COMMAND";
pub const MODE_CONFIRM: &str = "CONFIRM";
pub const MODE_PROMPT: &str = "PROMPT";
/// Reader view, in place of Normal (or Insert, with vim off).
pub const MODE_READER: &str = "READER";

/// Shown in the status bar after the file name while there are changes not
/// saved.
pub const CHANGES_NOT_SAVED: &str = "changes not saved";

/// The title of a document with no file name.
pub const UNTITLED: &str = "Untitled";

/// Shown when a file exists but could not be read, so it opened empty.
pub const READ_FAILED: &str = "Couldn't read the file, so it opened empty";

/// Shown when the shared named registers' file could not be read, so it
/// was set aside and the registers start empty.
pub const NAMED_REGISTERS_RESET: &str = "named registers reset: file was unreadable";

/// Shown when a named register could not be written to the state file, so
/// only this window has it.
pub const NAMED_REGISTER_NOT_SHARED: &str =
    "named register not shared: could not write the state file";

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

/// A write that failed, with the system's reason.
pub fn write_failed(reason: &str) -> String {
    format!("write failed: {reason}")
}

/// A reload that failed, with the system's reason.
pub fn reload_failed(reason: &str) -> String {
    format!("reload failed: {reason}")
}

/// Why a write or reload fails in an editor built with no file behind it
/// (the tests' editors).
pub const NO_FILE_ACCESS: &str = "file access is not wired up yet";

/// `ZZ` in reader view with unsaved changes: reader view never writes.
pub const READ_ONLY_ZQ: &str = "read-only: ZQ to discard";

/// A write that succeeded.
pub const WRITTEN: &str = "written";

/// `:w` when the file on disk is not the version that was read.
pub const CHANGED_ON_DISK_REFUSED: &str = "file changed on disk: :w! to overwrite, :e! to reload";

/// The warning beside the file name when the file is not valid UTF-8.
pub const NOT_UTF8_READ_ONLY: &str = "read-only: not valid UTF-8";

/// `:w` or `:w!` on a document read from a file that is not valid UTF-8.
pub const NOT_UTF8_REFUSED: &str = "E: not valid UTF-8; refusing to write";

/// Asked when the window is closed with unsaved changes.
pub const UNSAVED_CHANGES_PROMPT: &str = "unsaved changes: [s]ave [d]iscard [c]ancel";

/// Asked at open when another live window (process `pid`) has the file
/// open.
pub fn already_open(pid: u32) -> String {
    format!("already open in another window (pid {pid}): [o]pen read-only [e]dit anyway [q]uit")
}

/// The warning beside the file name once `o` opened the document
/// read-only, or once the file on disk cannot be read or written.
pub const READ_ONLY: &str = "read-only";

/// The file changed on disk while the document was clean: it was reloaded.
pub const RELOADED_FROM_DISK: &str = "reloaded from disk";

/// The warning beside `changes not saved` when the file changed on disk.
pub const CHANGED_ON_DISK: &str = "file changed on disk";

/// The warning beside the file name once the file was deleted or moved.
pub const NO_LONGER_ON_DISK: &str = "file no longer on disk";

/// `:w`, `:w!` or Primary+S on a document opened read-only.
pub const READ_ONLY_REFUSED: &str = "E: opened read-only; refusing to write";

/// Shown once at open when no lock record could be taken: the file opens
/// as usual, but another window will not see it is open here.
pub fn lock_unavailable(reason: &str) -> String {
    format!("lock unavailable: {reason}")
}

/// Why no lock record was taken when the environment names no state
/// directory (no `$XDG_STATE_HOME` and no `$HOME`).
pub const NO_STATE_DIRECTORY: &str = "no state directory";

/// Asked at open when a window that is gone left a copy of unsaved text
/// newer than the file.
pub const RECOVERY_PROMPT: &str = "recovery copy found: [r]estore [d]iscard [c]ompare";

/// The display name of the reader window `c` at the recovery prompt
/// opens on the copy of `name`.
pub fn recovery_title(name: &str) -> String {
    format!("{name} (recovery)")
}

/// The recovery copy could not be written, with the system's reason.
pub fn recovery_failed(reason: &str) -> String {
    format!("recovery copy not written: {reason}")
}

/// The recovery copy found at open could not be read or removed, with
/// the system's reason.
pub fn recovery_unreadable(reason: &str) -> String {
    format!("recovery copy unusable: {reason}")
}

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

/// Status message prefix when a link target does not exist; the path follows.
pub const NO_SUCH_FILE: &str = "no such file: ";

/// Asked before a link goes to the system handler when it is not a web or
/// mail link and not a markdown file: `target` is exactly what `o` hands
/// the system handler (a local path with its symlinks resolved, or a URL
/// as given), but with control characters and bidi format characters
/// shown escaped (`shown_safely`), so a crafted target cannot make the
/// question misread. `c` or Esc cancels.
pub fn open_link_prompt(target: &str) -> String {
    format!("open {}? [o]pen [c]ancel", shown_safely(target))
}

/// True for the characters that reorder the text around them (the bidi
/// marks, embeddings, overrides and isolates), or show as nothing or as a
/// break (the zero-width space, joiners, byte order mark and the line and
/// paragraph separators).
fn is_unseen(c: char) -> bool {
    matches!(
        c,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// `text` with every control character, bidi format character and
/// invisible character written as a `\u{...}` escape, for showing text a
/// document supplied. A backslash is doubled, so a literal `\u{202e}` in
/// the text cannot pass for an escaped one.
pub fn shown_safely(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\\' {
            out.push_str("\\\\");
        } else if c.is_control() || is_unseen(c) {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

/// The status-line prompt Primary+L opens for a link's URL or path.
pub const LINK_PROMPT: &str = "link: ";

/// The find bar's line in the status row, with vim off (Primary+F).
pub const FIND_PROMPT: &str = "find: ";

/// The find bar's counter: the current match and how many there are
/// (`0 of 0` with none).
pub fn find_counter(at: usize, of: usize) -> String {
    format!("{at} of {of}")
}

/// Asked, with vim off, when Primary+S would write over a file changed on
/// disk: `o` is `:w!`, `r` is `:e!`, `c` cancels.
pub const CHANGED_ON_DISK_PROMPT: &str = "file changed on disk: [o]verwrite [r]eload [c]ancel";

/// With vim off, autosave found the file changed on disk and did not
/// write: saving (Primary+S) asks what to keep. No `:` commands, as there
/// are none with vim off.
pub const CHANGED_ON_DISK_AUTOSAVE: &str = "file changed on disk, not saved; save to choose";

/// The table picker's size label: `<columns> x <rows>`.
pub fn table_size(cols: u32, rows: u32) -> String {
    format!("{cols} x {rows}")
}

/// `:table N M` past the largest table it makes (ruling R50).
pub fn table_too_big(max_cols: u32, max_rows: u32) -> String {
    format!("E474: A table has at most {max_cols} columns and {max_rows} rows")
}

/// Put before the window title while there are unsaved changes, where the
/// toolkit has no edited flag of its own (GTK; macOS shows its dot).
pub const DIRTY_TITLE_MARK: &str = "\u{2022} ";

/// The window title for a toolkit with no edited flag: `name`, marked
/// while `dirty`.
pub fn window_title(name: &str, dirty: bool) -> String {
    if dirty {
        format!("{DIRTY_TITLE_MARK}{name}")
    } else {
        name.to_owned()
    }
}

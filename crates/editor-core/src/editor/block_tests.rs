//! Undo, redo, `.` and operations that make several edits keep the
//! editor's blocks up to date with `Blocks::update`, one edit at a time,
//! rather than scanning the whole document (issue #13). Each test counts
//! the full scans a key sequence makes and checks the blocks against a
//! fresh scan after it.

use crate::markdown::block::full_scans;
use crate::markdown::block::oracle::assert_fresh;
use crate::testkit::{ed, keys};

use super::Editor;

/// A code block and a table at the top, then `lines` lines of prose.
fn sectioned(lines: usize) -> String {
    let mut s =
        String::from("# title\n\n```rust\nlet x = 1;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n");
    for i in 0..lines {
        s.push_str(&format!("line {i}\n"));
    }
    s
}

/// Presses `seq` and returns how many full scans it made, checking the
/// blocks describe the buffer afterwards.
fn scans(e: &mut Editor, seq: &str) -> usize {
    let n = full_scans(|| {
        keys(e, seq);
    });
    assert_fresh(&e.buf, &e.blocks);
    n
}

#[test]
fn undo_and_redo_follow_the_blocks() {
    let mut e = ed(&sectioned(200));
    keys(&mut e, "Gddkddkdd");
    assert_eq!(scans(&mut e, "u"), 0);
    assert_eq!(scans(&mut e, "2u"), 0);
    assert_eq!(scans(&mut e, "<C-r>"), 0);
    assert_eq!(scans(&mut e, "2<C-r>"), 0);
}

#[test]
fn undoing_a_deleted_fence_follows_the_blocks() {
    let mut e = ed(&sectioned(200));
    // The opening fence: the closer opens an unclosed block to the end.
    keys(&mut e, "3Gdd");
    assert_eq!(scans(&mut e, "u"), 0);
    assert_eq!(scans(&mut e, "<C-r>"), 0);
}

#[test]
fn substitute_over_many_lines_follows_the_blocks() {
    let mut e = ed(&sectioned(200));
    assert_eq!(scans(&mut e, ":%s/line/row/<CR>"), 0);
    assert_eq!(scans(&mut e, "u"), 0);
}

#[test]
fn dot_replaying_a_list_continuation_follows_the_blocks() {
    let mut e = ed(&format!("- a\n{}", sectioned(50)));
    keys(&mut e, "A<CR>b<Esc>");
    assert_eq!(scans(&mut e, "."), 0);
}

#[test]
fn dot_replaying_a_list_key_follows_the_blocks() {
    let mut e = ed(&format!("- a\n- b\n{}", sectioned(50)));
    keys(&mut e, "<Tab>j");
    assert_eq!(scans(&mut e, "."), 0);
}

#[test]
fn dot_replaying_a_change_in_code_follows_the_blocks() {
    let mut e = ed(&format!("```\nx\n```\n\n```\ny\n```\n{}", sectioned(50)));
    keys(&mut e, "2Gcicz<Esc>6G");
    assert_eq!(scans(&mut e, "."), 0);
    assert_eq!(e.buf.line(5), "z");
}

#[test]
fn a_run_that_rescans_the_document_rebuilds_once() {
    // Each bare fence the `:s` writes flips every line below it in or
    // out of code, so following each edit would rescan to the end
    // every time.
    let mut e = ed(&"x\n".repeat(200));
    assert_eq!(scans(&mut e, ":%s/x/```/<CR>"), 1);
    assert!(scans(&mut e, "u") <= 1);
}

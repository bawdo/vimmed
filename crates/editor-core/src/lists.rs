//! List, task and fence editing: Enter continuing a list item, indent and
//! outdent of list items, task checkbox toggles and fence auto-close.
//!
//! Each function is pure: it reads the buffer and its block structure and
//! returns an `EditPlan`, or `None` when its behaviour does not apply (not a
//! list item, not a task line, not a fence opener), so the caller falls back
//! to the plain behaviour (a plain line split, or Tab inserting `\t`).

use std::ops::Range;

use crate::buffer::{Buffer, Edit, Pos};
use crate::grapheme;
use crate::markdown::{Blocks, LineKind, ListKind};

/// A set of edits that make one command (one undo step), and where the
/// cursor goes afterwards.
///
/// The edits apply in order, each through `Buffer::replace`: every edit's
/// range refers to the buffer as left by the edits before it (the first one
/// to the buffer the plan was made from). `cursor` refers to the buffer after
/// the last edit and sits on a grapheme boundary there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPlan {
    pub edits: Vec<Edit>,
    pub cursor: Pos,
}

impl EditPlan {
    fn one(range: Range<Pos>, text: impl Into<String>, cursor: Pos) -> EditPlan {
        EditPlan {
            edits: vec![Edit {
                range,
                text: text.into(),
            }],
            cursor,
        }
    }
}

/// What an opening fence's auto-close inserts after it: an empty line for
/// the content, then the closing fence.
const FENCE_CLOSE: &str = "\n\n```";

/// Insert-mode Enter at `cursor`.
///
/// `blocks` must describe `buf`. Where they visibly disagree (an offset past
/// the line or off a char boundary, a prefix that is not there) this declines
/// with `None` rather than panic. The same holds for every function here.
/// `cursor` is clamped to a grapheme boundary first.
///
/// On a list item (outside code):
/// - an item with nothing but whitespace after its prefix becomes one empty
///   line, cursor at col 0, wherever the cursor is on the line;
/// - otherwise, with the cursor at or after the item's content, the line
///   splits and the new line gets the same indent and prefix: the same
///   bullet marker, the next number (`n+1.`), or `- [ ] ` for a task;
/// - with the cursor inside the prefix, `None` (a plain split).
///
/// On the opening fence of an unclosed block, with the cursor at the end of
/// the line, an empty line and a closing fence are inserted below, cursor on
/// the empty line. Any other line gives `None`.
pub fn enter(buf: &Buffer, blocks: &Blocks, cursor: Pos) -> Option<EditPlan> {
    let cursor = buf.clamp(cursor);
    let text = buf.line(cursor.line);
    match *blocks.kind(cursor.line) {
        LineKind::ListItem {
            kind,
            content_offset,
            ..
        } => {
            let line = cursor.line;
            let content = item_content(text, content_offset)?;
            if content.trim().is_empty() {
                return Some(EditPlan::one(
                    Pos { line, col: 0 }..Pos {
                        line,
                        col: text.len(),
                    },
                    "",
                    Pos { line, col: 0 },
                ));
            }
            if cursor.col < content_offset {
                return None;
            }
            let lead = &text[..text.len() - text.trim_start().len()];
            let prefix = match kind {
                ListKind::Bullet(_) => text[..content_offset].to_owned(),
                ListKind::Ordered(n) => format!("{lead}{}. ", n.saturating_add(1)),
                ListKind::Task(_) => format!("{lead}- [ ] "),
            };
            let next = format!("{prefix}{}", &text[cursor.col..]);
            let col = grapheme_ceil(&next, prefix.len());
            Some(EditPlan::one(
                cursor..cursor,
                format!("\n{prefix}"),
                Pos {
                    line: line + 1,
                    col,
                },
            ))
        }
        // Divergence (R43): moon_edit (document_controller.dart:339-341)
        // auto-closes on any opening fence (its `codeBlockId == null` branch
        // is dead), so Enter on a closed block's opener adds a second closer
        // and turns everything below into code. Only unclosed blocks close.
        LineKind::FenceOpen { block, .. }
            if cursor.col == text.len()
                && text.starts_with("```")
                && blocks.code_block(block).close.is_none() =>
        {
            Some(EditPlan::one(
                cursor..cursor,
                FENCE_CLOSE,
                Pos {
                    line: cursor.line + 1,
                    col: 0,
                },
            ))
        }
        _ => None,
    }
}

/// Indents list item `line` by 2 spaces. `blocks` must describe `buf` (see
/// `enter`). `cursor` is clamped, then mapped through the edit (it moves
/// right by 2 when it is on `line`). `None` when `line` is not a list item.
pub fn indent(buf: &Buffer, blocks: &Blocks, line: usize, cursor: Pos) -> Option<EditPlan> {
    if !is_item(buf, blocks, line) {
        return None;
    }
    let cursor = buf.clamp(cursor);
    let at = Pos { line, col: 0 };
    let cursor = if cursor.line == line {
        Pos {
            line,
            col: cursor.col + INDENT.len(),
        }
    } else {
        cursor
    };
    Some(EditPlan::one(at..at, INDENT, cursor))
}

/// Outdents list item `line` by removing up to 2 leading whitespace chars.
/// Each char counts as one column, a tab included: `\t\t\t- a` becomes
/// `\t- a`, not `\t\t- a`. `blocks` must describe `buf` (see `enter`).
/// `cursor` is clamped, then mapped through the edit (a cursor in the removed
/// whitespace goes to col 0). `None` when `line` is not a list item or has no
/// leading whitespace.
pub fn outdent(buf: &Buffer, blocks: &Blocks, line: usize, cursor: Pos) -> Option<EditPlan> {
    if !is_item(buf, blocks, line) {
        return None;
    }
    let cursor = buf.clamp(cursor);
    let text = buf.line(line);
    let removed: usize = text
        .chars()
        .take_while(|c| c.is_whitespace())
        .take(INDENT.len())
        .map(char::len_utf8)
        .sum();
    if removed == 0 {
        return None;
    }
    let cursor = if cursor.line == line {
        Pos {
            line,
            col: cursor.col.saturating_sub(removed),
        }
    } else {
        cursor
    };
    Some(EditPlan::one(
        Pos { line, col: 0 }..Pos { line, col: removed },
        "",
        cursor,
    ))
}

/// Toggles the task checkbox on `line`: `[ ]` becomes `[x]`, and `[x]` or
/// `[X]` becomes `[ ]`. `blocks` must describe `buf` (see `enter`). `cursor`
/// is clamped and otherwise left where it is (the edit keeps the line's
/// length). `None` when `line` is not a task item.
pub fn toggle_task(buf: &Buffer, blocks: &Blocks, line: usize, cursor: Pos) -> Option<EditPlan> {
    if line >= buf.line_count() {
        return None;
    }
    let LineKind::ListItem {
        kind: ListKind::Task(checked),
        content_offset,
        ..
    } = *blocks.kind(line)
    else {
        return None;
    };
    // The prefix ends `[?] `, so the box char sits 3 bytes before the content.
    let col = content_offset.checked_sub(3)?;
    let found = buf.line(line).get(col.checked_sub(1)?..content_offset)?;
    let expected = if checked {
        ["[x] ", "[X] "].contains(&found)
    } else {
        found == "[ ] "
    };
    if !expected {
        return None;
    }
    Some(EditPlan::one(
        Pos { line, col }..Pos { line, col: col + 1 },
        if checked { " " } else { "x" },
        buf.clamp(cursor),
    ))
}

/// What `indent` adds, and the most `outdent` removes (as chars).
const INDENT: &str = "  ";

fn is_item(buf: &Buffer, blocks: &Blocks, line: usize) -> bool {
    line < buf.line_count()
        && matches!(
            *blocks.kind(line),
            LineKind::ListItem { content_offset, .. }
                if item_content(buf.line(line), content_offset).is_some()
        )
}

/// The item text after the prefix, or `None` when `content_offset` does not
/// fit `text` (past the end, off a char boundary, or no list marker after the
/// leading whitespace), which means `blocks` lags the buffer.
fn item_content(text: &str, content_offset: usize) -> Option<&str> {
    let prefix = text.get(..content_offset)?;
    let marker = prefix.trim_start().bytes().next()?;
    (marker == b'-' || marker == b'*' || marker.is_ascii_digit()).then(|| &text[content_offset..])
}

/// The least grapheme boundary at or after `col`.
fn grapheme_ceil(text: &str, col: usize) -> usize {
    let floor = grapheme::floor(text, col);
    if floor == col {
        col
    } else {
        grapheme::next(text, floor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    fn lines(buf: &Buffer) -> Vec<&str> {
        (0..buf.line_count()).map(|i| buf.line(i)).collect()
    }

    /// Applies `plan` edit by edit, then checks the cursor is in range and on
    /// a grapheme boundary of the result.
    fn apply(buf: &mut Buffer, plan: &EditPlan) {
        for e in &plan.edits {
            buf.replace(e.range.clone(), &e.text);
        }
        let c = plan.cursor;
        assert!(c.line < buf.line_count(), "cursor line {c:?} out of range");
        let l = buf.line(c.line);
        assert!(c.col <= l.len(), "cursor col {c:?} past {l:?}");
        assert_eq!(
            grapheme::floor(l, c.col),
            c.col,
            "cursor {c:?} splits {l:?}"
        );
    }

    /// Runs `f` over `text` and applies the plan, returning the new buffer and
    /// cursor, or `None` when `f` declines.
    fn run(
        text: &str,
        f: impl FnOnce(&Buffer, &Blocks) -> Option<EditPlan>,
    ) -> Option<(Buffer, Pos)> {
        let mut buf = Buffer::from_text(text);
        let blocks = Blocks::new(&buf);
        let plan = f(&buf, &blocks)?;
        apply(&mut buf, &plan);
        Some((buf, plan.cursor))
    }

    fn enter_at(text: &str, cursor: Pos) -> Option<(Buffer, Pos)> {
        run(text, |b, k| enter(b, k, cursor))
    }

    // splitLine (document_controller_test.dart): plain lines are not ours,
    // so `enter` declines and the caller does a plain split.

    #[test]
    fn plain_line_split_is_declined() {
        assert!(enter_at("HelloWorld", pos(0, 5)).is_none());
        assert!(enter_at("Hello", pos(0, 0)).is_none());
        assert!(enter_at("Hello", pos(0, 5)).is_none());
    }

    #[test]
    fn does_not_continue_non_list_lines() {
        assert!(enter_at("paragraph", pos(0, 9)).is_none());
    }

    // splitLine with list continuation (document_controller_test.dart).

    #[test]
    fn continues_unordered_list() {
        let (buf, c) = enter_at("- first", pos(0, 7)).unwrap();
        assert_eq!(lines(&buf), ["- first", "- "]);
        assert_eq!(c, pos(1, 2));
    }

    #[test]
    fn continues_star_bullet_with_same_marker() {
        let (buf, c) = enter_at("* first", pos(0, 7)).unwrap();
        assert_eq!(lines(&buf), ["* first", "* "]);
        assert_eq!(c, pos(1, 2));
    }

    #[test]
    fn continues_ordered_list_with_incremented_number() {
        let (buf, c) = enter_at("1. first", pos(0, 8)).unwrap();
        assert_eq!(lines(&buf), ["1. first", "2. "]);
        assert_eq!(c, pos(1, 3));
        let (buf, _) = enter_at("1. first\n9. second", pos(1, 9)).unwrap();
        assert_eq!(buf.line(2), "10. ");
    }

    #[test]
    fn continues_task_list() {
        let (buf, c) = enter_at("- [ ] first", pos(0, 11)).unwrap();
        assert_eq!(lines(&buf), ["- [ ] first", "- [ ] "]);
        assert_eq!(c, pos(1, 6));
    }

    // Divergence (Task 36): moon_edit copies the `[x]`; continuation is
    // always `- [ ] `.
    #[test]
    fn task_continuation_is_unchecked() {
        let (buf, c) = enter_at("- [x] done", pos(0, 10)).unwrap();
        assert_eq!(lines(&buf), ["- [x] done", "- [ ] "]);
        assert_eq!(c, pos(1, 6));
        let (buf, _) = enter_at("  - [X] done", pos(0, 12)).unwrap();
        assert_eq!(buf.line(1), "  - [ ] ");
    }

    #[test]
    fn preserves_nesting_level() {
        let (buf, c) = enter_at("  - nested", pos(0, 10)).unwrap();
        assert_eq!(buf.line(1), "  - ");
        assert_eq!(c, pos(1, 4));
        let (buf, _) = enter_at("    3. deep", pos(0, 11)).unwrap();
        assert_eq!(buf.line(1), "    4. ");
    }

    // Divergence (Task 36): moon_edit's "exits list on empty list item"
    // expects 3 lines (the prefix line emptied plus a new empty line); the
    // empty item becomes one empty line, so 2.
    #[test]
    fn exits_list_on_empty_list_item() {
        let (buf, c) = enter_at("- first\n- ", pos(1, 2)).unwrap();
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.line(1), "");
        assert_eq!(c, pos(1, 0));
    }

    #[test]
    fn empty_item_exit_leaves_one_line() {
        for (text, cursor) in [
            ("- ", pos(0, 2)),
            ("1.  ", pos(0, 4)),
            ("- [x] ", pos(0, 6)),
            ("- [ ]   ", pos(0, 7)),
            ("- ", pos(0, 1)),
        ] {
            let (buf, c) = enter_at(&format!("a\n{text}\nb"), pos(1, cursor.col)).unwrap();
            assert_eq!(lines(&buf), ["a", "", "b"], "{text:?}");
            assert_eq!(c, pos(1, 0), "{text:?}");
        }
    }

    #[test]
    fn exits_nested_list_on_empty_item() {
        let (buf, c) = enter_at("- first\n  - ", pos(1, 4)).unwrap();
        assert_eq!(lines(&buf), ["- first", ""]);
        assert_eq!(c, pos(1, 0));
    }

    #[test]
    fn splits_in_middle_of_list_item() {
        let (buf, c) = enter_at("- hello world", pos(0, 8)).unwrap();
        assert_eq!(lines(&buf), ["- hello ", "- world"]);
        assert_eq!(c, pos(1, 2));
        let (buf, c) = enter_at("- [x] ab", pos(0, 7)).unwrap();
        assert_eq!(lines(&buf), ["- [x] a", "- [ ] b"]);
        assert_eq!(c, pos(1, 6));
    }

    #[test]
    fn cursor_inside_prefix_is_declined() {
        // Not a continuation point: a plain split (col 0 opens a line above).
        assert!(enter_at("- item", pos(0, 0)).is_none());
        assert!(enter_at("  1. item", pos(0, 3)).is_none());
    }

    #[test]
    fn list_syntax_inside_code_is_not_a_list() {
        assert!(enter_at("```\n- item\n```", pos(1, 6)).is_none());
        assert!(enter_at("```\n- \n```", pos(1, 2)).is_none());
    }

    #[test]
    fn continuation_keeps_graphemes_whole() {
        let fam = "👩\u{200d}👩\u{200d}👧";
        let text = format!("- {fam} x");
        let at = 2 + fam.len();
        let (buf, c) = enter_at(&text, pos(0, at)).unwrap();
        assert_eq!(lines(&buf), [format!("- {fam}"), "-  x".to_owned()]);
        assert_eq!(c, pos(1, 2));

        // Decomposed accent: split after the whole cluster.
        let (buf, c) = enter_at("- e\u{301}z", pos(0, 5)).unwrap();
        assert_eq!(lines(&buf), ["- e\u{301}", "- z"]);
        assert_eq!(c, pos(1, 2));

        // After a tab the remainder starts with a lone combining mark, which
        // joins the prefix's space: the cursor moves past that cluster.
        let (buf, c) = enter_at("- a\t\u{301}", pos(0, 4)).unwrap();
        assert_eq!(lines(&buf), ["- a\t", "- \u{301}"]);
        assert_eq!(c, pos(1, 4));
    }

    // code_block_auto_close_test.dart

    #[test]
    fn auto_closes_fence_after_opening_fence() {
        let (buf, c) = enter_at("```bash", pos(0, 7)).unwrap();
        assert_eq!(lines(&buf), ["```bash", "", "```"]);
        assert_eq!(c, pos(1, 0));
        let blocks = Blocks::new(&buf);
        assert!(matches!(
            blocks.kind(0),
            crate::markdown::LineKind::FenceOpen { .. }
        ));
        assert!(matches!(
            blocks.kind(1),
            crate::markdown::LineKind::Code { .. }
        ));
        assert!(matches!(
            blocks.kind(2),
            crate::markdown::LineKind::FenceClose { .. }
        ));
    }

    #[test]
    fn auto_closes_fence_without_language() {
        let (buf, c) = enter_at("```", pos(0, 3)).unwrap();
        assert_eq!(lines(&buf), ["```", "", "```"]);
        assert_eq!(c, pos(1, 0));
    }

    #[test]
    fn no_auto_close_mid_opening_fence() {
        assert!(enter_at("```bash", pos(0, 4)).is_none());
    }

    #[test]
    fn no_auto_close_inside_code_block() {
        assert!(enter_at("```bash\necho hello\n```", pos(1, 10)).is_none());
        // The closing fence is not an opener.
        assert!(enter_at("```bash\necho hello\n```", pos(2, 3)).is_none());
    }

    #[test]
    fn auto_close_above_existing_content() {
        let (buf, c) = enter_at("```bash\n- item 1\n- item 2", pos(0, 7)).unwrap();
        assert_eq!(lines(&buf), ["```bash", "", "```", "- item 1", "- item 2"]);
        assert_eq!(c, pos(1, 0));
        let blocks = Blocks::new(&buf);
        assert!(matches!(
            blocks.kind(3),
            crate::markdown::LineKind::ListItem { .. }
        ));
    }

    // indentLine (document_controller_test.dart)

    #[test]
    fn indents_list_item_by_two_spaces() {
        let (buf, _) = run("- item", |b, k| indent(b, k, 0, pos(0, 0))).unwrap();
        assert_eq!(buf.line(0), "  - item");
        let (buf, _) = run("  - item", |b, k| indent(b, k, 0, pos(0, 0))).unwrap();
        assert_eq!(buf.line(0), "    - item");
        let (buf, _) = run("1. a\n- [x] b", |b, k| indent(b, k, 1, pos(0, 0))).unwrap();
        assert_eq!(lines(&buf), ["1. a", "  - [x] b"]);
    }

    #[test]
    fn indent_declines_non_list_line() {
        assert!(run("paragraph", |b, k| indent(b, k, 0, pos(0, 0))).is_none());
        assert!(run("```\n- item\n```", |b, k| indent(b, k, 1, pos(1, 0))).is_none());
    }

    #[test]
    fn indent_adjusts_cursor_position() {
        let (_, c) = run("- item", |b, k| indent(b, k, 0, pos(0, 3))).unwrap();
        assert_eq!(c, pos(0, 5));
        // A cursor on another line stays put.
        let (_, c) = run("x\n- item", |b, k| indent(b, k, 1, pos(0, 1))).unwrap();
        assert_eq!(c, pos(0, 1));
    }

    // outdentLine (document_controller_test.dart)

    #[test]
    fn outdents_list_item_by_two_spaces() {
        let (buf, _) = run("  - item", |b, k| outdent(b, k, 0, pos(0, 0))).unwrap();
        assert_eq!(buf.line(0), "- item");
        let (buf, _) = run("     1. x", |b, k| outdent(b, k, 0, pos(0, 0))).unwrap();
        assert_eq!(buf.line(0), "   1. x");
    }

    #[test]
    fn outdent_removes_up_to_two_columns() {
        let (buf, _) = run(" - item", |b, k| outdent(b, k, 0, pos(0, 0))).unwrap();
        assert_eq!(buf.line(0), "- item");
        let (buf, _) = run("\t\t- item", |b, k| outdent(b, k, 0, pos(0, 0))).unwrap();
        assert_eq!(buf.line(0), "- item");
    }

    #[test]
    fn outdent_declines_non_indented_list() {
        assert!(run("- item", |b, k| outdent(b, k, 0, pos(0, 0))).is_none());
    }

    #[test]
    fn outdent_declines_non_list_line() {
        assert!(run("paragraph", |b, k| outdent(b, k, 0, pos(0, 0))).is_none());
        assert!(run("  paragraph", |b, k| outdent(b, k, 0, pos(0, 0))).is_none());
    }

    #[test]
    fn outdent_adjusts_cursor_position() {
        let (_, c) = run("  - item", |b, k| outdent(b, k, 0, pos(0, 5))).unwrap();
        assert_eq!(c, pos(0, 3));
    }

    #[test]
    fn outdent_clamps_cursor_in_indent_area_to_zero() {
        let (_, c) = run("  - item", |b, k| outdent(b, k, 0, pos(0, 1))).unwrap();
        assert_eq!(c, pos(0, 0));
    }

    #[test]
    fn indent_and_outdent_keep_graphemes_whole() {
        let text = "  - e\u{301}👩\u{200d}👧";
        let end = text.len();
        let (buf, c) = run(text, |b, k| outdent(b, k, 0, pos(0, end))).unwrap();
        assert_eq!(buf.line(0), "- e\u{301}👩\u{200d}👧");
        assert_eq!(c, pos(0, end - 2));
        let (buf, c) = run(text, |b, k| indent(b, k, 0, pos(0, 7))).unwrap();
        assert_eq!(buf.line(0), "    - e\u{301}👩\u{200d}👧");
        assert_eq!(c, pos(0, 9));
    }

    // gx action (actions_test.dart), toggle logic only.

    #[test]
    fn toggles_unchecked_task_to_checked() {
        let (buf, c) = run("- [ ] todo", |b, k| toggle_task(b, k, 0, pos(0, 6))).unwrap();
        assert_eq!(buf.line(0), "- [x] todo");
        assert_eq!(c, pos(0, 6));
    }

    #[test]
    fn toggles_checked_task_to_unchecked() {
        let (buf, c) = run("- [x] done", |b, k| toggle_task(b, k, 0, pos(0, 6))).unwrap();
        assert_eq!(buf.line(0), "- [ ] done");
        assert_eq!(c, pos(0, 6));
        let (buf, _) = run("  - [X] done", |b, k| toggle_task(b, k, 0, pos(0, 0))).unwrap();
        assert_eq!(buf.line(0), "  - [ ] done");
    }

    #[test]
    fn toggle_does_nothing_on_non_task_line() {
        assert!(run("- item", |b, k| toggle_task(b, k, 0, pos(0, 2))).is_none());
        assert!(run("paragraph", |b, k| toggle_task(b, k, 0, pos(0, 0))).is_none());
        assert!(run("```\n- [ ] x\n```", |b, k| toggle_task(b, k, 1, pos(1, 0))).is_none());
    }

    #[test]
    fn toggle_on_another_line_leaves_cursor() {
        let text = "e\u{301}\n- [ ] todo";
        let (buf, c) = run(text, |b, k| toggle_task(b, k, 1, pos(0, 3))).unwrap();
        assert_eq!(buf.line(1), "- [x] todo");
        assert_eq!(c, pos(0, 3));
    }

    type Planner = Box<dyn Fn(&Buffer, &Blocks) -> Option<EditPlan>>;

    #[test]
    fn plans_apply_cleanly_and_invert() {
        // Each plan applies in order, and the inverse edits restore the text.
        let cases: [(&str, Planner); 5] = [
            ("- a b", Box::new(|b, k| enter(b, k, pos(0, 3)))),
            ("x\n- \ny", Box::new(|b, k| enter(b, k, pos(1, 2)))),
            ("```rs", Box::new(|b, k| enter(b, k, pos(0, 5)))),
            ("  - a", Box::new(|b, k| outdent(b, k, 0, pos(0, 4)))),
            ("- [ ] a", Box::new(|b, k| toggle_task(b, k, 0, pos(0, 0)))),
        ];
        for (text, f) in cases {
            let mut buf = Buffer::from_text(text);
            let plan = f(&buf, &Blocks::new(&buf)).expect(text);
            let mut inverses: Vec<Edit> = plan
                .edits
                .iter()
                .map(|e| buf.replace(e.range.clone(), &e.text))
                .collect();
            assert_ne!(buf.to_text(), text);
            while let Some(inv) = inverses.pop() {
                buf.replace(inv.range, &inv.text);
            }
            assert_eq!(buf.to_text(), text);
        }
    }

    // Round 1 additions.

    // Divergence (R43): moon_edit auto-closes on any opening fence, so Enter
    // at the end of a closed block's opener adds a second closer and turns
    // the rest of the document into code. A closed opener declines.
    #[test]
    fn closed_fence_does_not_auto_close() {
        let text = "```bash\necho\n```\n\npara";
        assert!(enter_at(text, pos(0, 7)).is_none());
        // Unclosed openers still auto-close.
        assert!(enter_at("```bash\necho", pos(0, 7)).is_some());
        assert!(enter_at("para\n```", pos(1, 3)).is_some());
    }

    #[test]
    fn stale_blocks_decline_without_panic() {
        // Blocks made from another text: offsets past or inside the line.
        fn stale(
            from: &str,
            text: &str,
            f: impl Fn(&Buffer, &Blocks) -> Option<EditPlan>,
        ) -> Option<EditPlan> {
            let blocks = Blocks::new(&Buffer::from_text(from));
            f(&Buffer::from_text(text), &blocks)
        }
        assert!(stale("1234. item", "- ", |b, k| enter(b, k, pos(0, 2))).is_none());
        assert!(stale("- [ ] item", "ab", |b, k| toggle_task(b, k, 0, pos(0, 0))).is_none());
        assert!(stale("- [ ] item", "- [ ", |b, k| toggle_task(b, k, 0, pos(0, 0))).is_none());
        // Content offset 3 falls inside the 2-byte `é`.
        assert!(stale("1. x", "- \u{e9}", |b, k| enter(b, k, pos(0, 4))).is_none());
        assert!(stale("- item", "plain", |b, k| indent(b, k, 0, pos(0, 0))).is_none());
        assert!(stale("  - item", "  plain", |b, k| outdent(b, k, 0, pos(0, 0))).is_none());
    }

    #[test]
    fn incoming_cursor_is_clamped() {
        // Past the end of the line, then inside a cluster, then past the end
        // of the buffer.
        let (_, c) = run("- item", |b, k| indent(b, k, 0, pos(0, 99))).unwrap();
        assert_eq!(c, pos(0, 8));
        let (_, c) = run("  - e\u{301}", |b, k| outdent(b, k, 0, pos(0, 6))).unwrap();
        assert_eq!(c, pos(0, 2));
        let (_, c) = run("- [ ] e\u{301}", |b, k| toggle_task(b, k, 0, pos(0, 7))).unwrap();
        assert_eq!(c, pos(0, 6));
        let (_, c) = run("- [ ] x", |b, k| toggle_task(b, k, 0, pos(5, 0))).unwrap();
        assert_eq!(c, pos(0, 0));
    }

    #[test]
    fn outdent_counts_a_tab_as_one_column() {
        let (buf, c) = run("\t\t\t- item", |b, k| outdent(b, k, 0, pos(0, 5))).unwrap();
        assert_eq!(buf.line(0), "\t- item");
        assert_eq!(c, pos(0, 3));
        let (buf, _) = run("\t  - item", |b, k| outdent(b, k, 0, pos(0, 0))).unwrap();
        assert_eq!(buf.line(0), " - item");
    }

    #[test]
    fn multi_edit_plan_applies_in_sequence() {
        // The second range only exists after the first edit (line 1 is new).
        let plan = EditPlan {
            edits: vec![
                Edit {
                    range: pos(0, 1)..pos(0, 1),
                    text: "\n".to_owned(),
                },
                Edit {
                    range: pos(1, 0)..pos(1, 1),
                    text: "c".to_owned(),
                },
            ],
            cursor: pos(1, 1),
        };
        let mut buf = Buffer::from_text("ab");
        apply(&mut buf, &plan);
        assert_eq!(lines(&buf), ["a", "c"]);
    }

    #[test]
    fn crlf_enter_continues_and_keeps_eol() {
        let (buf, c) = enter_at("- a\r\n- b\r\n", pos(1, 3)).unwrap();
        assert_eq!(buf.to_text(), "- a\r\n- b\r\n- \r\n");
        assert_eq!(c, pos(2, 2));
        let (buf, _) = enter_at("1. a\r\n- \r\n", pos(1, 2)).unwrap();
        assert_eq!(buf.to_text(), "1. a\r\n\r\n");
    }

    #[test]
    fn tab_indented_enter_keeps_the_tab() {
        let (buf, c) = enter_at("\t1. b", pos(0, 5)).unwrap();
        assert_eq!(buf.line(1), "\t2. ");
        assert_eq!(c, pos(1, 4));
        let (buf, c) = enter_at("\t- [x] a", pos(0, 8)).unwrap();
        assert_eq!(buf.line(1), "\t- [ ] ");
        assert_eq!(c, pos(1, 7));
        let (buf, _) = enter_at("\t- a", pos(0, 4)).unwrap();
        assert_eq!(buf.line(1), "\t- ");
    }
}

//! Text objects. Only the inner code block (`ic`) so far: `yic dic cic`.

use std::ops::Range;

use crate::buffer::Pos;
use crate::markdown::{Blocks, LineKind};
use crate::vim::operator::{self, EditCtx, OpOutcome, OpRange, Operator};

/// Applies `op` to the inner code of `block` (from `code_block`), linewise:
/// `dic` leaves the fences adjacent, and `cic` leaves one empty line
/// between them and enters Insert on it, so typing never lands on a fence.
/// On an empty block `cic` opens that line and nothing else acts.
pub fn operate(ctx: &mut EditCtx, op: Operator, block: (usize, Range<usize>)) -> OpOutcome {
    let (open, content) = block;
    if content.is_empty() {
        if op != Operator::Change {
            return OpOutcome {
                cursor: ctx.cursor,
                insert: false,
            };
        }
        let at = Pos {
            line: open,
            col: ctx.buf.line(open).len(),
        };
        ctx.replace(at..at, "\n");
        let cursor = Pos {
            line: open + 1,
            col: 0,
        };
        ctx.place(cursor);
        return OpOutcome {
            cursor,
            insert: true,
        };
    }
    let range = OpRange {
        start: Pos {
            line: content.start,
            col: 0,
        },
        end: Pos {
            line: content.end - 1,
            col: 0,
        },
        linewise: true,
    };
    operator::operate(ctx, op, range)
}

/// True when `op` on `block` (from `code_block`) does anything.
pub fn acts(op: Operator, block: Option<&(usize, Range<usize>)>) -> bool {
    block.is_some_and(|(_, content)| !content.is_empty() || op == Operator::Change)
}

/// The content lines of the fenced code block that `line` is in, whether
/// `line` is one of its fences or one of its content lines. `None` for an
/// empty block, or a line outside any block. An unclosed block runs to its
/// last code line: the end of the document, or the line before the next
/// opening fence.
pub fn inner_code(blocks: &Blocks, line: usize) -> Option<Range<usize>> {
    code_block(blocks, line)
        .map(|(_, content)| content)
        .filter(|r| !r.is_empty())
}

/// The code block `line` is in: its opening fence's line and its content
/// lines (empty for an empty block). `None` outside a block. The extent
/// comes from the line kinds, as `display::code_bg` derives it.
pub fn code_block(blocks: &Blocks, line: usize) -> Option<(usize, Range<usize>)> {
    if line >= blocks.line_count() {
        return None;
    }
    let id = match blocks.kind(line) {
        LineKind::FenceOpen { block, .. }
        | LineKind::FenceClose { block }
        | LineKind::Code { block } => *block,
        _ => return None,
    };
    let mut open = line;
    loop {
        match blocks.kind(open) {
            LineKind::FenceOpen { block, .. } if *block == id => break,
            LineKind::Code { block } | LineKind::FenceClose { block }
                if *block == id && open > 0 =>
            {
                open -= 1;
            }
            _ => return None,
        }
    }
    let mut end = open + 1;
    while end < blocks.line_count()
        && matches!(blocks.kind(end), LineKind::Code { block } if *block == id)
    {
        end += 1;
    }
    Some((open, open + 1..end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::testkit::*;
    use crate::vim::Mode;

    fn blocks(text: &str) -> Blocks {
        Blocks::new(&Buffer::from_text(text))
    }

    /// An editor on `text` with the cursor on `line`.
    fn on_line(text: &str, line: usize) -> crate::editor::Editor {
        let mut e = ed(text);
        if line > 0 {
            keys(&mut e, &format!("{}G", line + 1));
        }
        assert_eq!(e.cursor().line, line);
        e
    }

    // inner_code over the block structure.

    #[test]
    fn inner_code_is_the_content_from_any_line_of_the_block() {
        let bl = blocks("x\n```rs\na\nb\n```\ny");
        for line in 1..=4 {
            assert_eq!(inner_code(&bl, line), Some(2..4), "line {line}");
        }
        assert_eq!(inner_code(&bl, 0), None);
        assert_eq!(inner_code(&bl, 5), None);
        assert_eq!(inner_code(&bl, 99), None);
    }

    #[test]
    fn inner_code_of_an_empty_block_is_none() {
        let bl = blocks("```\n```");
        assert_eq!(inner_code(&bl, 0), None);
        assert_eq!(inner_code(&bl, 1), None);
    }

    #[test]
    fn an_unclosed_block_runs_to_the_end() {
        let bl = blocks("x\n```\na\nb");
        assert_eq!(inner_code(&bl, 1), Some(2..4));
        assert_eq!(inner_code(&bl, 3), Some(2..4));
    }

    #[test]
    fn an_unclosed_block_can_end_at_the_next_opening_fence() {
        // Inside a block, a fence with a language opens a new block.
        let bl = blocks("```\na\n```rust\nb\n```");
        assert_eq!(inner_code(&bl, 0), Some(1..2));
        assert_eq!(inner_code(&bl, 1), Some(1..2));
        assert_eq!(inner_code(&bl, 2), Some(3..4));
        assert_eq!(inner_code(&bl, 4), Some(3..4));
    }

    // code_text_object_test.dart: yic.

    #[test]
    fn yic_yanks_the_code_without_fences() {
        let mut e = on_line("```\nline1\nline2\n```", 1);
        keys(&mut e, "yic");
        let r = e.register(None).unwrap();
        assert_eq!(r.text, "line1\nline2\n");
        assert!(r.linewise);
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn yic_works_on_either_fence() {
        let mut e = on_line("```\ncode\n```", 0);
        keys(&mut e, "yic");
        assert_eq!(e.register(None).unwrap().text, "code\n");
        let mut e = on_line("```\ncode\n```", 2);
        keys(&mut e, "yic");
        assert_eq!(e.register(None).unwrap().text, "code\n");
    }

    #[test]
    fn yic_outside_a_block_does_nothing() {
        let (mut e, host) = ed_with_host("plain text");
        host.set_clipboard("before");
        keys(&mut e, "yic");
        assert_eq!(e.register(None), None);
        assert_eq!(host.clipboard(), "before");
        assert_eq!(e.showcmd(), "");
    }

    #[test]
    fn yic_on_an_empty_block_yanks_nothing() {
        let mut e = on_line("```\n```", 0);
        keys(&mut e, "yic");
        assert_eq!(e.register(None), None);
    }

    #[test]
    fn yic_yanks_many_lines() {
        let mut e = on_line("```dart\nclass Foo {\n  void bar() {}\n}\n```", 2);
        keys(&mut e, "yic");
        assert_eq!(
            e.register(None).unwrap().text,
            "class Foo {\n  void bar() {}\n}\n"
        );
    }

    #[test]
    fn ic_at_the_document_start_and_end() {
        let mut e = on_line("```\nfirst\n```\nafter", 1);
        keys(&mut e, "yic");
        assert_eq!(e.register(None).unwrap().text, "first\n");
        let mut e = on_line("before\n```\nlast\n```", 2);
        keys(&mut e, "yic");
        assert_eq!(e.register(None).unwrap().text, "last\n");
    }

    // code_text_object_test.dart: dic.

    #[test]
    fn dic_leaves_the_fences_adjacent() {
        let mut e = on_line("```\ncode\n```", 1);
        keys(&mut e, "dic");
        assert_eq!(e.text(), "```\n```");
        let mut e = on_line("```\nline1\nline2\nline3\n```", 2);
        keys(&mut e, "dic");
        assert_eq!(e.text(), "```\n```");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn dic_outside_a_block_does_nothing() {
        let mut e = ed("plain text");
        keys(&mut e, "dic");
        assert_eq!(e.text(), "plain text");
        assert!(!e.history().can_undo());
    }

    #[test]
    fn dic_stores_the_deleted_lines() {
        let mut e = on_line("```\ndeleted\n```", 1);
        keys(&mut e, "dic");
        let r = e.register(None).unwrap();
        assert_eq!(r.text, "deleted\n");
        assert!(r.linewise);
    }

    #[test]
    fn dic_keeps_the_language_on_the_fence() {
        let mut e = on_line("```python\nprint(\"hi\")\n```", 1);
        keys(&mut e, "dic");
        assert_eq!(e.text(), "```python\n```");
    }

    #[test]
    fn dic_in_an_unclosed_block_takes_the_rest() {
        let mut e = on_line("x\n```\na\nb", 3);
        keys(&mut e, "dic");
        assert_eq!(e.text(), "x\n```");
    }

    #[test]
    fn ic_ignores_counts() {
        let text = "```\na\nb\n```\nafter";
        for seq in ["2dic", "d2ic"] {
            let mut e = on_line(text, 1);
            keys(&mut e, seq);
            assert_eq!(e.text(), "```\n```\nafter", "{seq}");
        }
    }

    #[test]
    fn dic_is_one_undo_step_and_dot_repeats_it() {
        let text = "```\na\nb\n```\n```\nc\n```";
        let mut e = on_line(text, 1);
        keys(&mut e, "dic");
        assert_eq!(e.text(), "```\n```\n```\nc\n```");
        keys(&mut e, "u");
        assert_eq!(e.text(), text);
        keys(&mut e, "<C-r>");
        // Into the second block, then repeat.
        keys(&mut e, "3G.");
        assert_eq!(e.text(), "```\n```\n```\n```");
    }

    // code_text_object_test.dart: cic.

    #[test]
    fn cic_opens_line_between_fences() {
        let mut e = on_line("```\nold\nlines\n```", 1);
        keys(&mut e, "cic");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.text(), "```\n\n```");
        assert_eq!(e.cursor(), pos(1, 0));
        keys(&mut e, "new<Esc>");
        assert_eq!(e.text(), "```\nnew\n```");
        keys(&mut e, "u");
        assert_eq!(e.text(), "```\nold\nlines\n```");
    }

    #[test]
    fn cic_in_an_empty_block_opens_a_line() {
        let mut e = on_line("```\n```", 1);
        keys(&mut e, "cic");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.text(), "```\n\n```");
        assert_eq!(e.cursor(), pos(1, 0));
    }

    #[test]
    fn cic_outside_a_block_stays_in_normal() {
        let mut e = ed("plain text");
        keys(&mut e, "cic");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.text(), "plain text");
    }

    #[test]
    fn cic_is_repeatable() {
        let mut e = on_line("```\na\n```\n```\nb\n```", 1);
        keys(&mut e, "cicz<Esc>");
        keys(&mut e, "4G.");
        assert_eq!(e.text(), "```\nz\n```\n```\nz\n```");
    }

    #[test]
    fn showcmd_shows_the_pending_text_object() {
        let mut e = ed("x");
        keys(&mut e, "di");
        assert_eq!(e.showcmd(), "di");
    }
}

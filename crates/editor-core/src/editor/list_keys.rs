//! The list keys (`crate::lists`): Enter, Tab and Shift+Tab in Insert (and
//! with vim off), Tab and Shift+Tab in Normal, and the task toggles, `gx`
//! and Ctrl+Space.
//!
//! The plan is made from the editor's own blocks, which every edit and
//! undo brings up to date before the next key, so they always describe the
//! buffer. `EditCtx::apply_plan` carries it out as one undo step (inside an
//! Insert session, part of the session's step).

use crate::effects::Effects;
use crate::input::KeyEvent;
use crate::status::Mode;
use crate::vim;
use crate::vim::operator::{self, ListKey, OpOutcome};

use super::Editor;

impl Editor {
    /// Ctrl+Space toggles the cursor line's task in every mode that edits:
    /// in Insert (or with vim off) as part of the typing, in Normal as a
    /// command `.` repeats, and in Visual without leaving it. Elsewhere,
    /// and in reader mode, nothing.
    pub(super) fn ctrl_space(&mut self, k: KeyEvent) -> Effects {
        // The link prompt is a line being typed: the document stays as
        // it is, so the range the link wraps stays right.
        if self.refuses_edits() || self.linking() {
            return Effects::default();
        }
        if !self.cfg.vim {
            return self.insert_list_key(ListKey::Toggle);
        }
        match self.mode {
            Mode::Insert => self.insert_list_key(ListKey::Toggle),
            Mode::Normal => self.normal_key(k),
            Mode::Visual | Mode::VisualLine => self.visual_toggle(),
            _ => Effects::default(),
        }
    }

    /// A list key in Insert, or with vim off: its plan, or what the key
    /// does on a plain line (`operator::insert_list_key`). The Insert
    /// session records the key, so `.` presses it again.
    pub(super) fn insert_list_key(&mut self, key: ListKey) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        let plan = operator::list_plan(key, &self.buf, &self.blocks, self.cursor);
        self.vim.pressed(key);
        self.run(|ctx| OpOutcome {
            cursor: operator::insert_list_key(ctx, key, plan),
            insert: false,
        })
    }

    /// A list key in Normal (`Step::List`). Where it does not apply it does
    /// nothing, and is not the change `.` repeats.
    pub(super) fn normal_list_key(&mut self, key: ListKey) -> Effects {
        let plan = if self.refuses_edits() {
            None
        } else {
            operator::list_plan(key, &self.buf, &self.blocks, self.cursor)
        };
        let Some(plan) = plan else {
            self.vim.discard();
            return Effects::default();
        };
        self.run(|ctx| {
            let at = ctx.apply_plan(plan);
            OpOutcome {
                cursor: vim::rest(ctx.buf, at),
                insert: false,
            }
        })
    }

    /// Ctrl+Space in Visual: toggles the cursor line's task and stays in
    /// Visual. Not a change `.` repeats.
    fn visual_toggle(&mut self) -> Effects {
        self.vim.cancel();
        let Some(plan) = operator::list_plan(ListKey::Toggle, &self.buf, &self.blocks, self.cursor)
        else {
            return Effects::default();
        };
        let before = self.selection_lines();
        let fx = self.run(|ctx| OpOutcome {
            cursor: ctx.apply_plan(plan),
            insert: false,
        });
        fx.merge(self.selection_redraw(before))
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::*;
    use crate::vim::Mode;

    // Insert-mode Enter.

    #[test]
    fn enter_continues_a_list_as_one_undo_step() {
        let mut e = ed("- a\n1. b");
        keys(&mut e, "A<CR>x<Esc>");
        assert_eq!(e.text(), "- a\n- x\n1. b");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), "- a\n1. b");
        keys(&mut e, "jA<CR>");
        assert_eq!(e.text(), "- a\n1. b\n2. ");
        assert_eq!(e.cursor(), pos(2, 3));
    }

    #[test]
    fn empty_item_exit_leaves_one_line() {
        let mut e = ed("- a");
        keys(&mut e, "A<CR><CR>");
        assert_eq!(e.text(), "- a\n");
        assert_eq!(e.cursor(), pos(1, 0));
        assert_eq!(e.mode(), Mode::Insert);
        keys(&mut e, "<Esc>u");
        assert_eq!(e.text(), "- a");
        assert_eq!(e.history().undo_len(), 0);
    }

    #[test]
    fn task_continuation_is_unchecked() {
        let mut e = ed("- [x] done");
        keys(&mut e, "A<CR>next<Esc>");
        assert_eq!(e.text(), "- [x] done\n- [ ] next");
    }

    #[test]
    fn fence_auto_close_is_one_undo_step() {
        let mut e = ed("```rust");
        keys(&mut e, "A<CR>");
        assert_eq!(e.text(), "```rust\n\n```");
        assert_eq!(e.cursor(), pos(1, 0));
        keys(&mut e, "fn f()<Esc>");
        assert_eq!(e.text(), "```rust\nfn f()\n```");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), "```rust");
    }

    #[test]
    fn closed_fence_enter_is_a_plain_split() {
        let mut e = ed("```rust\nx\n```");
        keys(&mut e, "A<CR><Esc>");
        assert_eq!(e.text(), "```rust\n\nx\n```");
    }

    #[test]
    fn o_does_not_auto_close_fence() {
        let mut e = ed("```rust");
        keys(&mut e, "o<Esc>");
        assert_eq!(e.text(), "```rust\n");
        // Nor does it continue a list.
        let mut e = ed("- a");
        keys(&mut e, "ob<Esc>");
        assert_eq!(e.text(), "- a\nb");
    }

    // Insert-mode Tab and Shift+Tab.

    #[test]
    fn insert_tab_indents_a_list_item_as_one_undo_step() {
        let mut e = ed("- a");
        keys(&mut e, "A<Tab>");
        assert_eq!(e.text(), "  - a");
        assert_eq!(e.cursor(), pos(0, 5));
        keys(&mut e, "b<Esc>");
        assert_eq!(e.text(), "  - ab");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), "- a");
    }

    #[test]
    fn insert_shift_tab_outdents_a_list_item_as_one_undo_step() {
        let mut e = ed("    - a");
        keys(&mut e, "A<S-Tab>");
        assert_eq!(e.text(), "  - a");
        assert_eq!(e.cursor(), pos(0, 5));
        keys(&mut e, "<Esc>");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), "    - a");
    }

    #[test]
    fn insert_tab_on_a_plain_line_inserts_a_tab() {
        let mut e = ed("ab");
        keys(&mut e, "a<Tab><S-Tab><Esc>");
        assert_eq!(e.text(), "a\tb");
    }

    // Normal-mode Tab and Shift+Tab.

    #[test]
    fn normal_tab_and_shift_tab_indent_and_outdent() {
        let mut e = ed("- a");
        keys(&mut e, "l<Tab>");
        assert_eq!(e.text(), "  - a");
        assert_eq!(e.cursor(), pos(0, 3));
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "<S-Tab>");
        assert_eq!(e.text(), "- a");
        assert_eq!(e.history().undo_len(), 2);
        keys(&mut e, "u");
        assert_eq!(e.text(), "  - a");
        keys(&mut e, "u");
        assert_eq!(e.text(), "- a");
    }

    #[test]
    fn normal_tab_on_plain_line_is_noop() {
        let mut e = ed("abc");
        keys(&mut e, "lx");
        assert_eq!(e.text(), "ac");
        keys(&mut e, "<Tab><S-Tab>");
        assert_eq!(e.text(), "ac");
        assert_eq!(e.cursor(), pos(0, 1));
        assert_eq!(e.history().undo_len(), 1);
        // The last change is still the `x`.
        keys(&mut e, "0.");
        assert_eq!(e.text(), "c");
    }

    #[test]
    fn dot_repeats_normal_indent() {
        let mut e = ed("- a\n- b");
        keys(&mut e, "<Tab>j.");
        assert_eq!(e.text(), "  - a\n  - b");
    }

    // Task toggles.

    #[test]
    fn toggle_is_undoable() {
        let mut e = ed("- [ ] a");
        keys(&mut e, "gx");
        assert_eq!(e.text(), "- [x] a");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "gx");
        assert_eq!(e.text(), "- [ ] a");
        keys(&mut e, "u");
        assert_eq!(e.text(), "- [x] a");
        keys(&mut e, "u");
        assert_eq!(e.text(), "- [ ] a");
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "- [x] a");
    }

    #[test]
    fn gx_on_a_plain_line_is_a_noop() {
        let mut e = ed("abc");
        keys(&mut e, "gx");
        assert_eq!(e.text(), "abc");
        assert_eq!(e.history().undo_len(), 0);
        assert!(e.last_change().is_none());
    }

    #[test]
    fn dot_repeats_a_toggle() {
        let mut e = ed("- [ ] a\n- [x] b");
        keys(&mut e, "gxj.");
        assert_eq!(e.text(), "- [x] a\n- [ ] b");
    }

    #[test]
    fn ctrl_space_toggles_in_normal() {
        let mut e = ed("- [ ] a");
        keys(&mut e, "ll<C-Space>");
        assert_eq!(e.text(), "- [x] a");
        assert_eq!(e.cursor(), pos(0, 2));
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "u");
        assert_eq!(e.text(), "- [ ] a");
    }

    #[test]
    fn ctrl_space_toggles_in_insert() {
        let mut e = ed("- [ ] a");
        keys(&mut e, "A<C-Space>");
        assert_eq!(e.text(), "- [x] a");
        assert_eq!(e.cursor(), pos(0, 7));
        assert_eq!(e.mode(), Mode::Insert);
        keys(&mut e, "b<Esc>");
        assert_eq!(e.text(), "- [x] ab");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), "- [ ] a");
    }

    #[test]
    fn ctrl_space_toggles_in_visual() {
        let mut e = ed("- [ ] a");
        keys(&mut e, "vl<C-Space>");
        assert_eq!(e.text(), "- [x] a");
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(e.cursor(), pos(0, 1));
        keys(&mut e, "<Esc>V<C-Space>");
        assert_eq!(e.text(), "- [ ] a");
        assert_eq!(e.mode(), Mode::VisualLine);
        keys(&mut e, "<Esc>u");
        assert_eq!(e.text(), "- [x] a");
    }

    #[test]
    fn reader_mode_does_not_toggle() {
        let mut e = build_reader("- [ ] a");
        keys(&mut e, "gx<C-Space><Tab>");
        assert_eq!(e.text(), "- [ ] a");
        assert_eq!(e.history().undo_len(), 0);
    }

    // Dot repeat of an Insert session replays its keys, as vim does: the
    // list keys act on the line they land on (vim 9.1 with `comments=b:-,b:*`
    // and `fo+=r`: `A<CR>b<Esc>` on `- a`, then `j.` on `* c`, gives `* b`).

    #[test]
    fn dot_replays_a_list_enter_where_it_lands() {
        let mut e = ed("- a\n1. c");
        keys(&mut e, "A<CR>b<Esc>j.");
        assert_eq!(e.text(), "- a\n- b\n1. c\n2. b");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn dot_replays_backspace_after_a_list_enter() {
        let mut e = ed("- a\n- c");
        keys(&mut e, "A<CR><BS>x<Esc>");
        assert_eq!(e.text(), "- a\n-x\n- c");
        keys(&mut e, "j.");
        assert_eq!(e.text(), "- a\n-x\n- c\n-x");
    }

    #[test]
    fn dot_replays_insert_tab_and_toggle() {
        let mut e = ed("- [ ] a\nb");
        keys(&mut e, "A<Tab><C-Space><Esc>j.");
        assert_eq!(e.text(), "  - [x] a\nb\t");
    }

    #[test]
    fn dot_replays_a_fence_auto_close() {
        let mut e = ed("```\n\nx");
        keys(&mut e, "A<CR>y<Esc>");
        assert_eq!(e.text(), "```\ny\n```\n\nx");
        // On a plain line the same keys just split it.
        keys(&mut e, "G.");
        assert_eq!(e.text(), "```\ny\n```\n\nx\ny");
    }

    #[test]
    fn dot_replays_a_paste_literally() {
        let (mut e, host) = ed_with_host("- a\n- c");
        host.set_clipboard("p\nq");
        keys(&mut e, "A<D-v><Esc>");
        assert_eq!(e.text(), "- ap\nq\n- c");
        keys(&mut e, "G.");
        assert_eq!(e.text(), "- ap\nq\n- cp\nq");
    }

    // With vim off, Enter, Tab and Shift+Tab act the same, each one step.

    #[test]
    fn plain_mode_continues_indents_and_toggles() {
        let (mut e, _host) = plain_with_host("- [ ] a");
        keys(&mut e, "<End><CR>");
        assert_eq!(e.text(), "- [ ] a\n- [ ] ");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "<Tab>");
        assert_eq!(e.text(), "- [ ] a\n  - [ ] ");
        assert_eq!(e.history().undo_len(), 2);
        keys(&mut e, "<S-Tab>");
        assert_eq!(e.text(), "- [ ] a\n- [ ] ");
        assert_eq!(e.history().undo_len(), 3);
        keys(&mut e, "<C-Space>");
        assert_eq!(e.text(), "- [ ] a\n- [x] ");
        assert_eq!(e.history().undo_len(), 4);
        keys(&mut e, "b<CR>");
        assert_eq!(e.text(), "- [ ] a\n- [x] b\n- [ ] ");
        keys(&mut e, "<CR>");
        assert_eq!(e.text(), "- [ ] a\n- [x] b\n");
        assert_eq!(e.cursor(), pos(2, 0));
    }

    #[test]
    fn plain_mode_tab_on_a_plain_line_inserts_a_tab() {
        let (mut e, _host) = plain_with_host("ab");
        keys(&mut e, "<Right><Tab><S-Tab>");
        assert_eq!(e.text(), "a\tb");
        assert_eq!(e.history().undo_len(), 1);
    }

    // Blocks stay in step: a list typed in this session continues.

    #[test]
    fn a_list_typed_in_the_session_continues() {
        let mut e = ed("");
        keys(&mut e, "i* one<CR>two<CR><CR>after<Esc>");
        assert_eq!(e.text(), "* one\n* two\nafter");
    }
}

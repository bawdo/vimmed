//! The table keys (`crate::tables`): Tab and Shift+Tab between cells in
//! Normal and Insert (and with vim off), Insert-mode Enter down a row, the
//! `t` commands (`ta tA td tD tc tC tx t> t< t=`), and `:table`, which
//! inserts a table of the size it names or opens the grid picker.
//!
//! The plans and moves are made from the editor's own blocks, which every
//! edit and undo brings up to date before the next key, so they always
//! describe the buffer (the `crate::tables` moves and commands do not check
//! beyond line and cell counts). `EditCtx::apply_plan` carries a plan out
//! as one undo step; outside an Insert session the step's cursor is pinned
//! to where the command started (`History::pin_cursor`), so undo puts the
//! cursor back there. A move, or a command that changes nothing (`t>` at
//! the last column), records no step.
//!
//! Reader view refuses all of it, moves included: its keys scroll the view
//! (`reader`). A document opened read-only (`locking`) refuses every edit
//! (a row Tab or Enter would add, the `t` commands, `:table`) but still
//! moves between cells.
//!
//! The grid picker (`TablePicker`) takes every key while it shows: `h l k
//! j` and the arrows resize the selection, Enter inserts it and Esc closes
//! it. The renderer draws it under the cursor line (`editor_render`'s
//! `overlay`).

use crate::effects::{Effects, Redraw};
use crate::input::{Key, KeyEvent};
use crate::status::Mode;
use crate::strings;
use crate::tables::{self, CellMove, TablePicker};
use crate::vim;
use crate::vim::operator::{self, ListKey, OpOutcome};

use super::Editor;

/// The widest table `:table N M` makes (ruling R50). The picker stops at
/// 10, well inside it.
pub(super) const MAX_COLS: u32 = 64;

/// The longest table `:table N M` makes, in data rows (ruling R50).
pub(super) const MAX_ROWS: u32 = 1000;

/// A full repaint: the picker covers lines it does not own.
fn all() -> Effects {
    Effects {
        redraw: Redraw::All,
        ..Effects::default()
    }
}

impl Editor {
    /// Tab, Shift+Tab or Insert-mode Enter with the cursor in a table: the
    /// move to the next or previous cell, or down a row, growing the table
    /// past its end (`operator::table_move`). `None` off a table, for the
    /// caller to treat the key as a list key. In Insert the key joins the
    /// session and what `.` replays.
    pub(super) fn table_nav(&mut self, key: ListKey) -> Option<Effects> {
        // `self.blocks` describe `self.buf`: see the module docs.
        let mv = operator::table_move(key, &self.buf, &self.blocks, self.cursor)?;
        if self.reader {
            return Some(Effects::default());
        }
        let grows = matches!(mv, CellMove::Edit(_));
        if grows && self.refuses_edits() {
            return Some(Effects::default());
        }
        let normal = self.cfg.vim && self.mode == Mode::Normal;
        let session = self.cfg.vim && self.mode == Mode::Insert;
        if !normal {
            self.vim.pressed(key);
        }
        let before = self.cursor;
        Some(self.run(|ctx| {
            let at = operator::apply_move(ctx, mv);
            if grows && !session {
                ctx.history.pin_cursor(before);
            }
            OpOutcome {
                cursor: if normal { vim::rest(ctx.buf, at) } else { at },
                insert: false,
            }
        }))
    }

    /// `t` and `key` in Normal (`Step::Table`): the table command, as one
    /// undo step that puts the cursor back on undo. Off a table, for a key
    /// that is no command, for one that changes nothing, and wherever
    /// edits are refused, nothing.
    pub(super) fn table_command(&mut self, key: char) -> Effects {
        let status = Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        };
        if self.refuses_edits() {
            return status;
        }
        // `self.blocks` describe `self.buf`: see the module docs.
        let plan = tables::table_at(&self.blocks, self.cursor.line)
            .and_then(|info| tables::command(info, &self.buf, self.cursor, key));
        let Some(plan) = plan else {
            return status;
        };
        let before = self.cursor;
        status.merge(self.run(|ctx| {
            let at = ctx.apply_plan(plan);
            ctx.history.pin_cursor(before);
            OpOutcome {
                cursor: vim::rest(ctx.buf, at),
                insert: false,
            }
        }))
    }

    /// `:table` (ruling R38): with two positive numbers, a table that size
    /// at the cursor, up to `MAX_COLS` by `MAX_ROWS` (past that, an error
    /// and nothing inserted); with anything else, the grid picker. Nothing
    /// in a document that refuses edits.
    pub(super) fn table_ex(&mut self, size: Option<(u32, u32)>) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        match size {
            None => {
                self.picker = Some(TablePicker::new());
                all()
            }
            Some((cols, rows)) if cols > MAX_COLS || rows > MAX_ROWS => {
                self.say(strings::table_too_big(MAX_COLS, MAX_ROWS));
                Effects::default()
            }
            Some((cols, rows)) => self.insert_table(cols, rows),
        }
    }

    /// A `cols` by `rows` table at the cursor (`tables::insert`), as one
    /// undo step that puts the cursor back on undo.
    pub(super) fn insert_table(&mut self, cols: u32, rows: u32) -> Effects {
        let plan = tables::insert(&self.buf, self.cursor, cols, rows);
        let before = self.cursor;
        let normal = self.cfg.vim && self.mode == Mode::Normal;
        self.run(|ctx| {
            let at = ctx.apply_plan(plan);
            ctx.history.pin_cursor(before);
            OpOutcome {
                cursor: if normal { vim::rest(ctx.buf, at) } else { at },
                insert: false,
            }
        })
    }

    /// A key while the grid picker shows: `h`/Left, `l`/Right, `k`/Up and
    /// `j`/Down move the selection (growing the grid), Enter inserts the
    /// selected size and Esc closes it. Every other key does nothing. The
    /// mouse hovers and clicks in it too (`mouse`).
    pub(super) fn picker_key(&mut self, k: KeyEvent) -> Effects {
        let plain = !(k.mods.ctrl || k.mods.alt || self.platform.primary(k.mods));
        let Some(p) = self.picker.as_mut() else {
            return Effects::default();
        };
        match k.key {
            Key::Esc => {
                self.picker = None;
                return all();
            }
            Key::Enter if plain => {
                let (cols, rows) = p.size();
                self.picker = None;
                return all().merge(self.insert_table(cols, rows));
            }
            Key::Char('h') | Key::Left if plain => p.left(),
            Key::Char('l') | Key::Right if plain => p.right(),
            Key::Char('k') | Key::Up if plain => p.up(),
            Key::Char('j') | Key::Down if plain => p.down(),
            _ => return Effects::default(),
        }
        all()
    }

    /// The grid picker, while `:table` shows it.
    pub fn table_picker(&self) -> Option<&TablePicker> {
        self.picker.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use crate::effects::Redraw;
    use crate::input::Input;
    use crate::strings;
    use crate::tables::TablePicker;
    use crate::testkit::*;
    use crate::view_model::{MsgKind, StatusLeft};
    use crate::vim::Mode;

    use super::super::Editor;

    /// Header, separator and two data rows. Cell starts: `a` and `c` at 2,
    /// `b` and `d` at 6.
    const T: &str = "| h1 | h2 |\n| --- | --- |\n| a | b |\n| c | d |";

    fn at(e: &mut Editor, line: usize, col: usize) {
        e.cursor = pos(line, col);
    }

    // Tab and Shift+Tab in Normal.

    #[test]
    fn normal_tab_and_shift_tab_move_between_cells() {
        let mut e = ed(T);
        at(&mut e, 2, 0);
        keys(&mut e, "<Tab>");
        assert_eq!(e.cursor(), pos(2, 6));
        keys(&mut e, "<Tab>");
        assert_eq!(e.cursor(), pos(3, 2));
        keys(&mut e, "<S-Tab>");
        assert_eq!(e.cursor(), pos(2, 6));
        // From the header the separator is skipped.
        at(&mut e, 0, 7);
        keys(&mut e, "<Tab>");
        assert_eq!(e.cursor(), pos(2, 2));
        keys(&mut e, "<S-Tab>");
        assert_eq!(e.cursor(), pos(0, 7));
        assert_eq!(e.text(), T);
        assert_eq!(e.history().undo_len(), 0);
        assert!(e.last_change().is_none());
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn shift_tab_at_the_first_cell_stays() {
        let mut e = ed(T);
        keys(&mut e, "<S-Tab>");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.text(), T);
        assert_eq!(e.history().undo_len(), 0);
    }

    #[test]
    fn tab_created_row_is_undoable() {
        let mut e = ed(T);
        at(&mut e, 3, 8);
        keys(&mut e, "<Tab>");
        let grown = format!("{T}\n|  |  |");
        assert_eq!(e.text(), grown);
        assert_eq!(e.cursor(), pos(4, 2));
        assert_eq!(e.history().undo_len(), 1);
        // The blocks know the new row: Tab goes on along it.
        keys(&mut e, "<Tab>");
        assert_eq!(e.cursor(), pos(4, 5));
        keys(&mut e, "u");
        assert_eq!(e.text(), T);
        assert_eq!(e.cursor(), pos(3, 8));
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), grown);
        // A Tab is not a change `.` repeats.
        assert!(e.last_change().is_none());
    }

    // Insert-mode Tab, Shift+Tab and Enter.

    #[test]
    fn insert_tab_and_enter_move_between_cells() {
        let mut e = ed(T);
        at(&mut e, 2, 0);
        keys(&mut e, "i<Tab>");
        assert_eq!(e.cursor(), pos(2, 6));
        assert_eq!(e.mode(), Mode::Insert);
        keys(&mut e, "<S-Tab>");
        assert_eq!(e.cursor(), pos(2, 2));
        keys(&mut e, "<CR>");
        assert_eq!(e.cursor(), pos(3, 2));
        assert_eq!(e.text(), T);
        // Enter on the last row grows the table, in the same column.
        keys(&mut e, "<Tab><CR>");
        assert_eq!(e.text(), format!("{T}\n|  |  |"));
        assert_eq!(e.cursor(), pos(4, 5));
        keys(&mut e, "x<Esc>");
        assert_eq!(e.text(), format!("{T}\n|  | x |"));
        // The row and the typing are the one Insert session's step.
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), T);
    }

    #[test]
    fn dot_replays_a_table_tab_where_it_lands() {
        let mut e = ed(T);
        at(&mut e, 2, 0);
        keys(&mut e, "i<Tab>x<Esc>");
        assert_eq!(
            e.text(),
            "| h1 | h2 |\n| --- | --- |\n| a | xb |\n| c | d |"
        );
        keys(&mut e, "j0.");
        assert_eq!(
            e.text(),
            "| h1 | h2 |\n| --- | --- |\n| a | xb |\n| c | xd |"
        );
    }

    // With vim off, the same keys do the same, each its own step.

    #[test]
    fn plain_mode_tab_shift_tab_and_enter_in_a_table() {
        let (mut e, _host) = plain_with_host(T);
        at(&mut e, 2, 0);
        keys(&mut e, "<Tab>");
        assert_eq!(e.cursor(), pos(2, 6));
        keys(&mut e, "<S-Tab>");
        assert_eq!(e.cursor(), pos(2, 2));
        keys(&mut e, "<CR>");
        assert_eq!(e.cursor(), pos(3, 2));
        assert_eq!(e.text(), T);
        assert_eq!(e.history().undo_len(), 0);
        keys(&mut e, "<End><Tab>");
        assert_eq!(e.text(), format!("{T}\n|  |  |"));
        assert_eq!(e.cursor(), pos(4, 2));
        assert_eq!(e.history().undo_len(), 1);
    }

    // The `t` commands.

    #[test]
    fn t_commands_are_one_undo_step_and_restore_the_cursor() {
        for (cmd, from) in [
            ("ta", pos(2, 6)),
            ("tA", pos(2, 6)),
            ("td", pos(2, 6)),
            ("tD", pos(2, 6)),
            ("tc", pos(2, 6)),
            ("tC", pos(2, 6)),
            ("tx", pos(2, 6)),
            ("t>", pos(3, 2)),
            ("t<lt>", pos(3, 6)),
            ("t=", pos(2, 6)),
        ] {
            let mut e = ed(T);
            at(&mut e, from.line, from.col);
            keys(&mut e, cmd);
            let after = e.text();
            assert_ne!(after, T, "{cmd}");
            assert_eq!(e.history().undo_len(), 1, "{cmd}");
            assert_eq!(e.mode(), Mode::Normal, "{cmd}");
            assert_eq!(e.showcmd(), "", "{cmd}");
            keys(&mut e, "u");
            assert_eq!(e.text(), T, "{cmd}");
            assert_eq!(e.cursor(), from, "{cmd}");
            keys(&mut e, "<C-r>");
            assert_eq!(e.text(), after, "{cmd}");
        }
    }

    #[test]
    fn t_commands_edit_the_table() {
        let mut e = ed(T);
        at(&mut e, 2, 6);
        keys(&mut e, "ta");
        assert_eq!(
            e.text(),
            "| h1 | h2 |\n| --- | --- |\n| a | b |\n|  |  |\n| c | d |"
        );
        assert_eq!(e.cursor(), pos(3, 2));
        // The blocks follow: the new row is in the table.
        keys(&mut e, "jt>");
        assert_eq!(
            e.text(),
            "| h2 | h1 |\n| --- | --- |\n| b | a |\n|  |  |\n| d | c |"
        );
        keys(&mut e, "tD");
        assert_eq!(e.text(), "");
    }

    #[test]
    fn ta_on_header_inserts_after_separator_as_one_step() {
        let mut e = ed(T);
        keys(&mut e, "ta");
        assert_eq!(
            e.text(),
            "| h1 | h2 |\n| --- | --- |\n|  |  |\n| a | b |\n| c | d |"
        );
        assert_eq!(e.cursor(), pos(2, 2));
        keys(&mut e, "u");
        assert_eq!(e.text(), T);
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn edge_swap_creates_no_undo_entry() {
        let mut e = ed(T);
        at(&mut e, 2, 6);
        keys(&mut e, "t>");
        at(&mut e, 3, 2);
        keys(&mut e, "t<lt>");
        assert_eq!(e.text(), T);
        assert_eq!(e.history().undo_len(), 0);
        assert!(!e.dirty());
        assert_eq!(e.showcmd(), "");
        assert_eq!(e.cursor(), pos(3, 2));
    }

    #[test]
    fn t_is_a_prefix_only_in_a_table() {
        let mut e = ed(T);
        keys(&mut e, "t");
        assert_eq!(e.showcmd(), "t");
        // An unknown second key clears it and does nothing.
        keys(&mut e, "z");
        assert_eq!(e.showcmd(), "");
        assert_eq!(e.text(), T);
        assert_eq!(e.mode(), Mode::Normal);
        // Off a table `t` means nothing, so `a` appends.
        let mut e = ed("abc");
        keys(&mut e, "ta");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(0, 1));
        // After an operator it is not a table command either.
        let mut e = ed(T);
        keys(&mut e, "dta");
        assert_eq!(e.text(), T);
        assert_eq!(e.mode(), Mode::Insert);
    }

    // `:table`.

    #[test]
    fn table_with_a_size_inserts_it_as_one_step() {
        let mut e = ed("");
        keys(&mut e, ":table 3 2<CR>");
        assert_eq!(
            e.text(),
            "| Column 1 | Column 2 | Column 3 |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |"
        );
        assert_eq!(e.cursor(), pos(2, 2));
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.history().undo_len(), 1);
        assert!(e.table_picker().is_none());
        // On a line with text it goes below, with a blank line each side.
        let mut e = ed("intro");
        keys(&mut e, ":table 1 1<CR>");
        assert_eq!(e.text(), "intro\n\n| Column 1 |\n| --- |\n|  |\n");
        assert_eq!(e.cursor(), pos(4, 2));
        keys(&mut e, "u");
        assert_eq!(e.text(), "intro");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn table_past_the_cap_is_refused() {
        let refused = strings::table_too_big(64, 1000);
        for cmd in [
            ":table 65 2<CR>",
            ":table 2 1001<CR>",
            ":table 99999999999 1<CR>",
        ] {
            let mut e = ed("x");
            keys(&mut e, cmd);
            assert_eq!(e.text(), "x", "{cmd}");
            assert_eq!(
                e.status().left,
                StatusLeft::Message {
                    text: refused.clone(),
                    kind: MsgKind::Error,
                },
                "{cmd}"
            );
            assert_eq!(e.history().undo_len(), 0, "{cmd}");
            assert!(e.table_picker().is_none(), "{cmd}");
        }
        // The cap itself is fine.
        let mut e = ed("");
        keys(&mut e, ":table 64 1<CR>");
        assert_eq!(e.buffer().line_count(), 3);
        assert_eq!(e.message(), None);
    }

    // The grid picker.

    #[test]
    fn table_without_a_size_opens_the_picker() {
        for cmd in [":table<CR>", ":table 0 3<CR>", ":table 2<CR>"] {
            let mut e = ed("");
            keys(&mut e, cmd);
            assert_eq!(e.table_picker(), Some(&TablePicker::new()), "{cmd}");
            assert_eq!(e.mode(), Mode::Normal, "{cmd}");
            assert_eq!(e.text(), "", "{cmd}");
        }
    }

    #[test]
    fn picker_keys_resize_insert_and_close() {
        let mut e = ed("");
        let fx = keys(&mut e, ":table<CR>");
        assert_eq!(fx.redraw, Redraw::All);
        keys(&mut e, "lljjj");
        assert_eq!(e.table_picker().unwrap().selected(), (2, 3));
        keys(&mut e, "<Left><Up>");
        assert_eq!(e.table_picker().unwrap().selected(), (1, 2));
        keys(&mut e, "<Right><Down>hk");
        assert_eq!(e.table_picker().unwrap().selected(), (1, 2));
        assert_eq!(e.table_picker().unwrap().label(), "2 x 3");
        // Other keys do nothing while it shows.
        keys(&mut e, "xdd:<D-v>");
        assert_eq!(e.text(), "");
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "<CR>");
        assert!(e.table_picker().is_none());
        assert_eq!(
            e.text(),
            "| Column 1 | Column 2 |\n| --- | --- |\n|  |  |\n|  |  |\n|  |  |"
        );
        assert_eq!(e.cursor(), pos(2, 2));
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), "");
        // Esc closes it without inserting.
        keys(&mut e, ":table<CR>l<Esc>");
        assert!(e.table_picker().is_none());
        assert_eq!(e.text(), "");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn picker_grows_to_ten_and_inserts_ten_columns() {
        let mut e = ed("");
        keys(&mut e, ":table<CR>");
        for _ in 0..12 {
            keys(&mut e, "l");
        }
        assert_eq!(e.table_picker().unwrap().cols(), 10);
        keys(&mut e, "<CR>");
        assert_eq!(e.buffer().line(0).matches("Column").count(), 10);
        assert_eq!(e.buffer().line_count(), 3);
    }

    #[test]
    fn picker_takes_committed_text_as_keys() {
        let mut e = ed("");
        keys(&mut e, ":table<CR>");
        e.handle(Input::Text("ljx".into()));
        assert_eq!(e.table_picker().unwrap().selected(), (1, 1));
        assert_eq!(e.text(), "");
    }

    // Reader view and read-only refuse every table edit.

    #[test]
    fn reader_refuses_table_keys() {
        let mut e = build_reader(T);
        at(&mut e, 3, 8);
        keys(&mut e, "<Tab>tatd<S-Tab>t=");
        assert_eq!(e.text(), T);
        assert_eq!(e.cursor(), pos(3, 8));
        assert_eq!(e.history().undo_len(), 0);
        keys(&mut e, ":table 2 2<CR>:table<CR>");
        assert_eq!(e.text(), T);
        assert!(e.table_picker().is_none());
        // `t` is no prefix there, so a scroll key after it still scrolls.
        keys(&mut e, "<Esc>");
        let fx = keys(&mut e, "tj");
        assert!(fx.scroll.is_some());
        // With vim off too.
        let (mut e, _host) = plain_with_host(T);
        e.reader = true;
        at(&mut e, 3, 8);
        keys(&mut e, "<Tab><CR><S-Tab>");
        assert_eq!(e.text(), T);
        assert_eq!(e.cursor(), pos(3, 8));
    }

    #[test]
    fn read_only_refuses_table_edits_but_moves() {
        let mut e = ed(T);
        e.open_read_only();
        at(&mut e, 3, 8);
        keys(&mut e, "<Tab>");
        assert_eq!(e.text(), T);
        for cmd in [
            "ta",
            "tA",
            "td",
            "tD",
            "tc",
            "tC",
            "tx",
            "t<lt>",
            "t=",
            "i<CR><Esc>",
        ] {
            at(&mut e, 3, 6);
            keys(&mut e, cmd);
            assert_eq!(e.text(), T, "{cmd}");
        }
        keys(&mut e, ":table 2 2<CR>:table<CR>");
        assert_eq!(e.text(), T);
        assert!(e.table_picker().is_none());
        assert!(!e.dirty());
        assert_eq!(e.history().undo_len(), 0);
        // Moving between cells still works.
        at(&mut e, 2, 0);
        keys(&mut e, "<Tab>");
        assert_eq!(e.cursor(), pos(2, 6));
    }
}

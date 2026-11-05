//! The mouse and the wheel. The shell hit-tests each event against what
//! it painted (`DocView::hit_test`) and hands the result over as a
//! `MouseHit`: the caret position nearest the point, the grapheme under
//! it and what it is over (`Zone`).
//!
//! - A click (`Down`) places the cursor: on the grapheme under the point in
//!   Normal (never past the line's last one), at the nearest caret position
//!   in Insert and with vim off. In Insert it starts a new undo step, as an
//!   arrow key does. Reveal (the hidden syntax shown around the cursor)
//!   stays where it was until `Up`, so the text does not move under the
//!   pointer while the button is held. A click never scrolls the view.
//! - A drag enters charwise Visual from where the press landed (with vim
//!   off, the selection the arrow keys make, its end open). From Insert,
//!   the Insert session ends first, as Esc ends it.
//! - A double-click selects the word under the point (letters, digits and
//!   `_`, in any script), the run of whitespace, or the single punctuation
//!   grapheme, in Visual with the cursor on its last grapheme (moon_edit);
//!   with vim off the selection ends after it. A drag after it keeps the
//!   selection. Past a line's end it is a click; on a link a click opens,
//!   nothing (its first click opened it).
//! - Every press the editor takes (on a link or checkbox too) leaves
//!   Visual, drops the vim-off selection, cancels pending keys (`d`
//!   waiting for its motion) and drops an input method's composition, so
//!   it never commits where the caret went. A drag off the text turns a
//!   press on a link or checkbox into a drag: `Up` then does nothing.
//! - A held press holds back a reload of the file changed on disk until
//!   `Up` (`disk`).
//! - A click on a task's checkbox (`Zone::Prefix`) toggles it on `Up`
//!   (`lists::toggle_task`, as Ctrl+Space does), as an undo step of its
//!   own, the cursor staying where it is.
//! - A click on a link (`Zone::Link`) opens it on `Up` through `open_link`
//!   (which decides what a target does): a plain click in Normal, with vim
//!   off and in reader view, Primary+click in Insert. The cursor stays.
//!   A drag from a link selects instead.
//! - While the table picker shows, hovering (`Move`) or pressing on a cell
//!   selects that size and releasing on one inserts it, as Enter does;
//!   other clicks do nothing.
//! - Reader view and a read-only document take link clicks and the wheel
//!   only: no checkbox toggles, no picker.
//! - Clicks and drags do nothing while a line is typed in the status bar
//!   (`:`, `/`, the link prompt, the find bar), while `:s///c` asks, while
//!   any prompt asks and while the help overlay shows (R68). A press held
//!   when one of those starts still lets its reveal go on `Up`.
//! - The wheel and trackpad scroll the view by whole points, carrying the
//!   fraction to the next event, and leave the cursor where it is; they
//!   scroll during prompts too, but not under the help overlay.

use std::ops::Range;

use crate::buffer::Pos;
use crate::effects::{Effects, Redraw, Scroll};
use crate::grapheme;
use crate::input::{Mods, MouseEvent, MouseHit, MouseKind, Zone};
use crate::lists;
use crate::markdown::{InlineKind, parse_line};
use crate::status::Mode;
use crate::vim::{self, operator::OpOutcome};

use super::Editor;

/// What `Up` does when the press did not become a drag.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OnUp {
    /// The press placed the cursor; nothing more.
    Nothing,
    /// Open this link target.
    Open(String),
    /// Toggle the task on this line.
    Toggle(usize),
}

/// A button held since a press the editor took.
pub(super) struct Pressed {
    /// Where the cursor was before the press: reveal stays around it
    /// until `Up` (`Editor::reveal_at`).
    reveal: Pos,
    /// Where the press landed: a drag's selection starts here.
    anchor: Pos,
    on_up: OnUp,
    /// The press moved: `Up` only ends it.
    dragged: bool,
    /// It was a double-click: a drag keeps its selection.
    word: bool,
}

/// The class a grapheme falls in for a double-click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(g: &str) -> Class {
    match g.chars().next() {
        Some(c) if c.is_alphanumeric() || c == '_' => Class::Word,
        Some(c) if c.is_whitespace() => Class::Space,
        _ => Class::Other,
    }
}

/// What a double-click at `col` (a grapheme start) selects on `text`: the
/// word, the whitespace run or the one other grapheme there, as byte
/// offsets. `None` at or past the end.
fn word_at(text: &str, col: usize) -> Option<Range<usize>> {
    if col >= text.len() {
        return None;
    }
    let at = grapheme::floor(text, col);
    let first = &text[at..grapheme::next(text, at)];
    let k = class(first);
    if k == Class::Other {
        return Some(at..grapheme::next(text, at));
    }
    let mut start = at;
    while start > 0 {
        let p = grapheme::prev(text, start);
        if class(&text[p..start]) != k {
            break;
        }
        start = p;
    }
    let mut end = at;
    while end < text.len() {
        let n = grapheme::next(text, end);
        if class(&text[end..n]) != k {
            break;
        }
        end = n;
    }
    Some(start..end)
}

impl Editor {
    /// A mouse event (see the module notes).
    pub(super) fn mouse(&mut self, m: MouseEvent) -> Effects {
        if self.picker.is_some() && !self.mouse_refused() {
            let fx = self.release_press();
            return fx.merge(self.picker_mouse(m));
        }
        if m.kind == MouseKind::Up {
            return self.lift();
        }
        if self.mouse_refused() {
            return Effects::default();
        }
        match m.kind {
            MouseKind::Down => self.press(m.hit, m.mods),
            MouseKind::DoubleClick => self.double_click(m.hit, m.mods),
            MouseKind::Drag => self.drag(m.hit),
            MouseKind::Up | MouseKind::Move => Effects::default(),
        }
    }

    /// The wheel or trackpad, `dy_pt` points down the document: the view
    /// scrolls by whole points, the fraction kept for the next event.
    pub(super) fn wheel(&mut self, dy_pt: f32) -> Effects {
        if self.help.is_some() || !dy_pt.is_finite() {
            return Effects::default();
        }
        let total = self.wheel_rem + dy_pt;
        let whole = total.trunc();
        self.wheel_rem = total - whole;
        if whole == 0.0 {
            return Effects::default();
        }
        Effects {
            scroll: Some(Scroll::Points(whole as i32)),
            ..Effects::default()
        }
    }

    /// Where reveal is around: the cursor, or, while a press holds it, the
    /// cursor from before the press.
    pub(super) fn reveal_at(&self) -> Pos {
        match &self.pressed {
            Some(p) => self.buf.clamp(p.reveal),
            None => self.cursor,
        }
    }

    /// Ends a press: reveal moves to the cursor, repainting the lines it
    /// left and the ones it reaches.
    pub(super) fn release_press(&mut self) -> Effects {
        let Some(p) = self.pressed.take() else {
            return Effects::default();
        };
        let n = self.buf.line_count();
        let was = self.buf.clamp(p.reveal).line.min(n.saturating_sub(1));
        Effects {
            redraw: Redraw::Lines(self.reveal_extent(was))
                .merge(Redraw::Lines(self.reveal_extent(self.cursor.line))),
            ..Effects::default()
        }
    }

    /// True while clicks and drags do nothing (R68): a line typed in the
    /// status bar, `:s///c`'s question, a prompt, the help overlay.
    fn mouse_refused(&self) -> bool {
        matches!(
            self.mode,
            Mode::Command | Mode::Search | Mode::Confirm | Mode::Prompt
        ) || self.linking()
            || self.finding()
            || self.help.is_some()
    }

    /// True when a click on a link opens it in the current mode: a plain
    /// click in Normal, with vim off and in reader view, Primary+click in
    /// Insert. Never in Visual (the click leaves it).
    fn click_opens(&self, mods: Mods) -> bool {
        if self.reader || !self.cfg.vim {
            return true;
        }
        match self.mode {
            Mode::Normal => true,
            Mode::Insert => self.platform.primary(mods),
            _ => false,
        }
    }

    /// The target of the link on `line` whose text holds source byte
    /// `at`, trimmed.
    fn link_target(&self, line: usize, at: usize) -> Option<String> {
        let text = self.buf.line(line);
        parse_line(text, self.blocks.kind(line))
            .into_iter()
            .find_map(|e| match e.kind {
                InlineKind::Link { url } if e.syntax.contains(&at) => {
                    text.get(url).map(|u| u.trim().to_owned())
                }
                _ => None,
            })
    }

    /// The line whose task a click on `hit` toggles, if it is a task's
    /// checkbox and the document takes edits.
    fn toggle_line(&self, hit: &MouseHit) -> Option<usize> {
        let line = hit.pos.line;
        (hit.zone == Zone::Prefix
            && !self.refuses_edits()
            && line < self.buf.line_count()
            && lists::toggle_task(&self.buf, &self.blocks, line, self.cursor).is_some())
        .then_some(line)
    }

    /// What `Up` does after a press on `hit` that does not drag.
    fn on_up(&self, hit: &MouseHit, mods: Mods) -> OnUp {
        // The grapheme under the point picks the link, so two links side
        // by side (one run of link spans) open the one clicked.
        let at = match &hit.zone {
            Zone::Link(r) if r.contains(&hit.under.col) => hit.under.col,
            Zone::Link(r) => r.start,
            _ => 0,
        };
        if matches!(hit.zone, Zone::Link(_))
            && self.click_opens(mods)
            && hit.pos.line < self.buf.line_count()
            && let Some(target) = self.link_target(hit.pos.line, at)
        {
            return OnUp::Open(target);
        }
        match self.toggle_line(hit) {
            Some(line) => OnUp::Toggle(line),
            None => OnUp::Nothing,
        }
    }

    /// True when the cursor is a block resting on a grapheme (vim, out of
    /// Insert).
    fn block_cursor(&self) -> bool {
        self.cfg.vim && self.mode != Mode::Insert
    }

    /// Where a press on `hit` puts the cursor, and a drag from it starts.
    fn press_pos(&self, hit: &MouseHit) -> Pos {
        if self.block_cursor() {
            vim::rest(&self.buf, hit.under)
        } else {
            self.buf.clamp(hit.pos)
        }
    }

    /// Ends an Insert session as Esc does, without moving the cursor.
    fn end_insert_session(&mut self) {
        if self.cfg.vim && self.mode == Mode::Insert {
            self.mode = Mode::Normal;
            self.vim.cancel();
            self.history.end();
            self.vim.commit();
        }
    }

    /// Puts the cursor at `to` while the button holds reveal back, and
    /// repaints the caret's old and new lines.
    fn place(&mut self, to: Pos) -> Effects {
        let old = self.cursor.line;
        self.goal = None;
        self.cursor = self.buf.clamp(to);
        Effects {
            redraw: Redraw::Lines(old..old + 1)
                .merge(Redraw::Lines(self.cursor.line..self.cursor.line + 1)),
            ..Effects::default()
        }
    }

    /// What every press the editor takes does first: a composition in
    /// progress is dropped (the input method is told by the shell), and
    /// Visual, the vim-off selection and any pending keys (`d` waiting for
    /// its motion) go.
    fn take_press(&mut self) -> Effects {
        let fx = self.release_press().merge(self.clear_preedit());
        if self.visual() {
            self.mode = Mode::Normal;
            self.anchor = None;
        }
        if !self.cfg.vim {
            self.anchor = None;
        }
        if !(self.cfg.vim && self.mode == Mode::Insert) {
            self.vim.cancel();
        }
        fx
    }

    /// `Down`: a link or checkbox waits for `Up`; anything else places the
    /// cursor. Either way Visual (and, with vim off, the selection) goes.
    fn press(&mut self, hit: Option<MouseHit>, mods: Mods) -> Effects {
        let Some(hit) = hit else {
            return Effects::default();
        };
        if matches!(hit.zone, Zone::Picker { .. }) {
            return Effects::default();
        }
        // Worked out before Visual goes: a link click in Visual places the
        // cursor rather than opening.
        let on_up = self.on_up(&hit, mods);
        // Reader view takes link clicks only.
        if self.reader && !matches!(on_up, OnUp::Open(_)) {
            return self.release_press();
        }
        let before = self.selection_lines();
        let reveal = self.reveal_at();
        let mut fx = self.take_press();
        if on_up == OnUp::Nothing {
            if self.cfg.vim && self.mode == Mode::Insert {
                // As an arrow key: the typing after it is a new step.
                self.history.end();
                self.history.begin();
                self.vim.arrowed();
            }
            fx = fx.merge(self.place(self.press_pos(&hit)));
        }
        self.pressed = Some(Pressed {
            reveal,
            anchor: self.press_pos(&hit),
            on_up,
            dragged: false,
            word: false,
        });
        fx.merge(self.selection_redraw(before))
    }

    /// `DoubleClick`: selects the word, whitespace run or punctuation
    /// under the point (over a link a click would not open, too). On a
    /// link a click opens, it does nothing: its first click opened it. A
    /// checkbox takes it as another click.
    fn double_click(&mut self, hit: Option<MouseHit>, mods: Mods) -> Effects {
        let Some(hit) = hit else {
            return Effects::default();
        };
        let opens = matches!(hit.zone, Zone::Link(_)) && self.click_opens(mods);
        if opens {
            return Effects::default();
        }
        let line = hit.under.line.min(self.buf.line_count().saturating_sub(1));
        let range = word_at(self.buf.line(line), hit.under.col);
        let wordy = matches!(hit.zone, Zone::Text | Zone::Link(_)) && !self.reader;
        let (Some(range), true) = (range, wordy) else {
            return self.press(Some(hit), mods);
        };
        let before = self.selection_lines();
        let reveal = self.reveal_at();
        let fx = self.take_press();
        let last = grapheme::prev(self.buf.line(line), range.end);
        let start = Pos {
            line,
            col: range.start,
        };
        let head = if self.cfg.vim {
            self.end_insert_session();
            self.vim.cancel();
            self.mode = Mode::Visual;
            Pos { line, col: last }
        } else {
            Pos {
                line,
                col: range.end,
            }
        };
        self.anchor = Some(start);
        let fx = fx.merge(self.place(head));
        self.pressed = Some(Pressed {
            reveal,
            anchor: start,
            on_up: OnUp::Nothing,
            dragged: true,
            word: true,
        });
        fx.merge(self.selection_redraw(before))
    }

    /// `Drag`: from the press, a selection to the point.
    fn drag(&mut self, hit: Option<MouseHit>) -> Effects {
        // Off the text the selection stays as it was, but the press is no
        // longer a click: its link or checkbox does nothing on `Up`.
        let (Some(hit), Some(p)) = (hit, self.pressed.as_ref()) else {
            if let Some(p) = self.pressed.as_mut() {
                p.on_up = OnUp::Nothing;
            }
            return Effects::default();
        };
        if p.word || self.reader || matches!(hit.zone, Zone::Picker { .. }) {
            return Effects::default();
        }
        let anchor = p.anchor;
        // Leaving Insert first, so the head rests as Visual's does.
        let leaving = self.cfg.vim && self.mode == Mode::Insert;
        let head = if self.cfg.vim {
            vim::rest(&self.buf, hit.under)
        } else {
            self.buf.clamp(hit.pos)
        };
        let dragged = p.dragged;
        if !dragged && head == anchor {
            return Effects::default();
        }
        if let Some(p) = self.pressed.as_mut() {
            p.dragged = true;
            p.on_up = OnUp::Nothing;
        }
        let before = self.selection_lines();
        if self.cfg.vim {
            if leaving {
                self.end_insert_session();
            }
            if !self.visual() {
                self.vim.cancel();
                self.mode = Mode::Visual;
                self.anchor = Some(anchor);
            }
        } else if !dragged {
            self.anchor = Some(anchor);
        }
        let fx = self.place(head);
        fx.merge(self.selection_redraw(before))
    }

    /// `Up`: reveal catches up with the cursor, then a press that did not
    /// drag opens its link or toggles its checkbox. While clicks are
    /// refused only the reveal moves.
    fn lift(&mut self) -> Effects {
        let on_up = match &self.pressed {
            Some(p) if !p.dragged => p.on_up.clone(),
            _ => OnUp::Nothing,
        };
        let fx = self.release_press();
        if self.mouse_refused() {
            return fx;
        }
        match on_up {
            OnUp::Nothing => fx,
            OnUp::Open(target) => fx.merge(self.open_link(&target)),
            OnUp::Toggle(line) => fx.merge(self.toggle_by_click(line)),
        }
    }

    /// Toggles the task on `line` as an undo step of its own, the cursor
    /// staying put and the view with it.
    fn toggle_by_click(&mut self, line: usize) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        let Some(plan) = lists::toggle_task(&self.buf, &self.blocks, line, self.cursor) else {
            return Effects::default();
        };
        let insert = self.cfg.vim && self.mode == Mode::Insert;
        if insert {
            self.history.end();
        }
        self.vim.cancel();
        let before = self.selection_lines();
        let fx = self.run(|ctx| OpOutcome {
            cursor: ctx.apply_plan(plan),
            insert: false,
        });
        if insert {
            // The Insert session goes on as after an arrow key.
            self.history.begin();
            self.vim.arrowed();
        }
        Effects {
            scroll_to_cursor: false,
            ..fx.merge(self.selection_redraw(before))
        }
    }

    /// The mouse while the table picker shows: hovering, pressing or
    /// dragging on a cell selects that size, releasing on one inserts it
    /// as Enter does. Nothing else does anything.
    fn picker_mouse(&mut self, m: MouseEvent) -> Effects {
        let Some(MouseHit {
            zone: Zone::Picker { col, row },
            ..
        }) = m.hit
        else {
            return Effects::default();
        };
        if self.refuses_edits() {
            return Effects::default();
        }
        let Some(p) = self.picker.as_mut() else {
            return Effects::default();
        };
        let was = p.clone();
        p.select(col, row);
        if m.kind == MouseKind::Up {
            let (cols, rows) = p.size();
            self.picker = None;
            return Effects {
                redraw: Redraw::All,
                ..Effects::default()
            }
            .merge(self.insert_table(cols, rows));
        }
        if *p != was {
            Effects {
                redraw: Redraw::All,
                ..Effects::default()
            }
        } else {
            Effects::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::buffer::Pos;
    use crate::effects::{Effects, Scroll};
    use crate::input::{Input, Mods, MouseEvent, MouseHit, MouseKind, Zone};
    use crate::status::Mode;
    use crate::testkit::{ViewSource, build_reader, ed, ed_with_host, keys, plain_with_host, pos};
    use crate::view_model::{Selection, StatusLeft};

    use super::super::Editor;

    fn send(e: &mut Editor, kind: MouseKind, hit: Option<MouseHit>, mods: Mods) -> Effects {
        e.handle(Input::Mouse(MouseEvent { kind, hit, mods }))
    }

    /// A hit on text, `under` the grapheme at `under` and nearest the
    /// caret position `at`.
    fn on(at: Pos, under: Pos) -> Option<MouseHit> {
        Some(MouseHit {
            pos: at,
            under,
            zone: Zone::Text,
        })
    }

    fn zoned(at: Pos, zone: Zone) -> Option<MouseHit> {
        Some(MouseHit {
            pos: at,
            under: at,
            zone,
        })
    }

    /// A plain click (Down then Up) on the grapheme at `p`.
    fn click(e: &mut Editor, p: Pos) -> Effects {
        let fx = send(e, MouseKind::Down, on(p, p), Mods::default());
        fx.merge(send(e, MouseKind::Up, on(p, p), Mods::default()))
    }

    fn cmd() -> Mods {
        Mods {
            cmd: true,
            ..Mods::default()
        }
    }

    fn sel(e: &Editor) -> Option<(Pos, Pos)> {
        e.selection().map(|s: Selection| (s.anchor, s.head))
    }

    #[test]
    fn a_click_places_the_cursor_on_the_grapheme_under_it() {
        let mut e = ed("hello world\nsecond");
        click(&mut e, pos(1, 3));
        assert_eq!(e.cursor(), pos(1, 3));
        assert_eq!(e.mode(), Mode::Normal);
        // Normal rests on a grapheme: never past the line's last one.
        let fx = send(
            &mut e,
            MouseKind::Down,
            on(pos(0, 11), pos(0, 11)),
            Mods::default(),
        );
        assert_eq!(e.cursor(), pos(0, 10));
        assert!(!fx.scroll_to_cursor, "a click never scrolls the view");
    }

    #[test]
    fn insert_clicks_take_the_nearest_caret_position() {
        let mut e = ed("hello");
        keys(&mut e, "i");
        send(
            &mut e,
            MouseKind::Down,
            on(pos(0, 3), pos(0, 2)),
            Mods::default(),
        );
        assert_eq!(e.cursor(), pos(0, 3));
        assert_eq!(e.mode(), Mode::Insert);
        // Typing after it is a new undo step, as after an arrow key.
        keys(&mut e, "X<Esc>");
        assert_eq!(e.text(), "helXlo");
    }

    #[test]
    fn reveal_waits_for_mouse_up() {
        let mut e = ed("plain\n**bold** x");
        let hidden = e.display(1);
        // Down on "bold": the cursor moves, but line 1 keeps its rendered
        // display until Up, so the text does not shift under the pointer.
        let fx = send(
            &mut e,
            MouseKind::Down,
            on(pos(1, 3), pos(1, 3)),
            Mods::default(),
        );
        assert_eq!(e.cursor(), pos(1, 3));
        assert_eq!(e.display(1), hidden);
        assert_eq!(e.display(1).spans[0].src.start, 2, "`**` still hidden");
        assert!(matches!(fx.redraw, crate::effects::Redraw::Lines(_)));
        let fx = send(
            &mut e,
            MouseKind::Up,
            on(pos(1, 3), pos(1, 3)),
            Mods::default(),
        );
        assert_eq!(e.display(1).spans[0].src.start, 0, "`**` revealed on Up");
        match fx.redraw {
            crate::effects::Redraw::Lines(r) => assert!(r.contains(&1)),
            other => panic!("expected lines, got {other:?}"),
        }
    }

    #[test]
    fn drag_enters_visual() {
        let mut e = ed("hello world");
        send(
            &mut e,
            MouseKind::Down,
            on(pos(0, 1), pos(0, 1)),
            Mods::default(),
        );
        assert_eq!(e.mode(), Mode::Normal);
        send(
            &mut e,
            MouseKind::Drag,
            on(pos(0, 8), pos(0, 7)),
            Mods::default(),
        );
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(sel(&e), Some((pos(0, 1), pos(0, 7))));
        send(
            &mut e,
            MouseKind::Drag,
            on(pos(0, 9), pos(0, 9)),
            Mods::default(),
        );
        assert_eq!(sel(&e), Some((pos(0, 1), pos(0, 9))));
        send(
            &mut e,
            MouseKind::Up,
            on(pos(0, 9), pos(0, 9)),
            Mods::default(),
        );
        assert_eq!(e.mode(), Mode::Visual);
        keys(&mut e, "d");
        assert_eq!(e.text(), "hd");
    }

    #[test]
    fn a_drag_from_insert_selects_in_visual() {
        let mut e = ed("hello world");
        keys(&mut e, "ixy");
        send(
            &mut e,
            MouseKind::Down,
            on(pos(0, 3), pos(0, 3)),
            Mods::default(),
        );
        send(
            &mut e,
            MouseKind::Drag,
            on(pos(0, 6), pos(0, 6)),
            Mods::default(),
        );
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(sel(&e), Some((pos(0, 3), pos(0, 6))));
        // The Insert session ended as Esc ends it: one undo step.
        keys(&mut e, "<Esc>u");
        assert_eq!(e.text(), "hello world");
    }

    #[test]
    fn double_click_selects_word_cursor_on_last_char() {
        let mut e = ed("foo   bar, baz");
        send(
            &mut e,
            MouseKind::DoubleClick,
            on(pos(0, 7), pos(0, 7)),
            Mods::default(),
        );
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(sel(&e), Some((pos(0, 6), pos(0, 8))));
        assert_eq!(e.cursor(), pos(0, 8));
        send(
            &mut e,
            MouseKind::Up,
            on(pos(0, 7), pos(0, 7)),
            Mods::default(),
        );
        assert_eq!(e.mode(), Mode::Visual);
        // A run of whitespace.
        let mut e = ed("foo   bar, baz");
        send(
            &mut e,
            MouseKind::DoubleClick,
            on(pos(0, 4), pos(0, 4)),
            Mods::default(),
        );
        assert_eq!(sel(&e), Some((pos(0, 3), pos(0, 5))));
        // A single punctuation char.
        let mut e = ed("foo   bar,, baz");
        send(
            &mut e,
            MouseKind::DoubleClick,
            on(pos(0, 9), pos(0, 9)),
            Mods::default(),
        );
        assert_eq!(sel(&e), Some((pos(0, 9), pos(0, 9))));
        // Past the end of a line, nothing to select: the cursor goes there.
        let mut e = ed("ab\n\ncd");
        send(
            &mut e,
            MouseKind::DoubleClick,
            on(pos(1, 0), pos(1, 0)),
            Mods::default(),
        );
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(1, 0));
    }

    #[test]
    fn a_word_is_letters_digits_and_underscores_in_any_script() {
        let mut e = ed("x été_2 y");
        send(
            &mut e,
            MouseKind::DoubleClick,
            on(pos(0, 3), pos(0, 3)),
            Mods::default(),
        );
        // "été_2" is bytes 2..9 (é is two bytes); its last char starts at 8.
        assert_eq!(sel(&e), Some((pos(0, 2), pos(0, 8))));
    }

    #[test]
    fn a_click_in_visual_exits_visual() {
        let mut e = ed("hello world");
        keys(&mut e, "vll");
        assert_eq!(e.mode(), Mode::Visual);
        click(&mut e, pos(0, 8));
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.selection(), None);
        assert_eq!(e.cursor(), pos(0, 8));
    }

    #[test]
    fn a_checkbox_or_link_click_in_visual_exits_visual_too() {
        let mut e = ed("- [ ] a\nb c");
        keys(&mut e, "jvl");
        assert_eq!(e.mode(), Mode::Visual);
        let p = zoned(pos(0, 6), Zone::Prefix);
        send(&mut e, MouseKind::Down, p.clone(), Mods::default());
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.selection(), None);
        send(&mut e, MouseKind::Up, p, Mods::default());
        assert_eq!(e.text(), "- [x] a\nb c");
        assert_eq!(e.mode(), Mode::Normal);
        // A link click in Visual leaves it, and places the cursor.
        let (mut e, host) = ed_with_host(LINKED);
        keys(&mut e, "vl");
        send(&mut e, MouseKind::Down, on_link(), Mods::default());
        send(&mut e, MouseKind::Up, on_link(), Mods::default());
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.selection(), None);
        assert!(host.state().opened.is_empty());
    }

    #[test]
    fn a_click_cancels_a_pending_operator() {
        // `d` waits for a motion; a click on a link (which does not move
        // the cursor) must not leave it waiting.
        let (mut e, _) = ed_with_host(LINKED);
        keys(&mut e, "d");
        assert_eq!(e.showcmd(), "d");
        send(&mut e, MouseKind::Down, on_link(), Mods::default());
        send(&mut e, MouseKind::Up, on_link(), Mods::default());
        assert_eq!(e.showcmd(), "");
        keys(&mut e, "l");
        assert_eq!(e.text(), LINKED, "`l` is a motion, not `dl`");
        // A checkbox click too.
        let mut e = ed("- [ ] a");
        keys(&mut e, "d");
        let p = zoned(pos(0, 6), Zone::Prefix);
        send(&mut e, MouseKind::Down, p.clone(), Mods::default());
        send(&mut e, MouseKind::Up, p, Mods::default());
        keys(&mut e, "l");
        assert_eq!(e.text(), "- [x] a");
    }

    #[test]
    fn a_drag_off_the_text_from_a_link_opens_nothing() {
        let (mut e, host) = ed_with_host(LINKED);
        send(&mut e, MouseKind::Down, on_link(), Mods::default());
        send(&mut e, MouseKind::Drag, None, Mods::default());
        send(&mut e, MouseKind::Up, None, Mods::default());
        assert!(host.state().opened.is_empty());
        // Nor a checkbox.
        let mut e = ed("- [ ] a");
        let p = zoned(pos(0, 6), Zone::Prefix);
        send(&mut e, MouseKind::Down, p, Mods::default());
        send(&mut e, MouseKind::Drag, None, Mods::default());
        send(&mut e, MouseKind::Up, None, Mods::default());
        assert_eq!(e.text(), "- [ ] a");
    }

    #[test]
    fn a_click_drops_a_composition_rather_than_moving_it() {
        let mut e = ed("hello");
        keys(&mut e, "i");
        e.handle(Input::Preedit {
            text: "に".into(),
            cursor: 3,
        });
        assert!(e.preedit().is_some());
        click(&mut e, pos(0, 4));
        assert_eq!(e.preedit(), None);
        assert_eq!(e.cursor(), pos(0, 4));
        assert_eq!(e.text(), "hello");
        // With vim off too.
        let (mut e, _) = plain_with_host("hello");
        e.handle(Input::Preedit {
            text: "に".into(),
            cursor: 3,
        });
        send(
            &mut e,
            MouseKind::DoubleClick,
            on(pos(0, 1), pos(0, 1)),
            Mods::default(),
        );
        assert_eq!(e.preedit(), None);
        assert_eq!(e.text(), "hello");
    }

    #[test]
    fn scrolling_does_not_split_a_vim_off_typing_run() {
        let (mut e, _) = plain_with_host("");
        keys(&mut e, "ab");
        e.handle(Input::Scroll { dy_pt: 30.0 });
        keys(&mut e, "cd");
        assert_eq!(e.text(), "abcd");
        keys(&mut e, "<D-z>");
        assert_eq!(e.text(), "", "one undo step");
    }

    #[test]
    fn prefix_click_toggles_task() {
        let mut e = ed("- [ ] a\nb");
        keys(&mut e, "j");
        let p = zoned(pos(0, 6), Zone::Prefix);
        send(&mut e, MouseKind::Down, p.clone(), Mods::default());
        let fx = send(&mut e, MouseKind::Up, p.clone(), Mods::default());
        assert_eq!(e.text(), "- [x] a\nb");
        assert_eq!(e.cursor(), pos(1, 0), "the cursor stays where it was");
        assert!(!fx.scroll_to_cursor);
        assert_eq!(e.history().undo_len(), 1);
        send(&mut e, MouseKind::Down, p.clone(), Mods::default());
        send(&mut e, MouseKind::Up, p, Mods::default());
        assert_eq!(e.text(), "- [ ] a\nb");
        keys(&mut e, "u");
        assert_eq!(e.text(), "- [x] a\nb");
    }

    #[test]
    fn prefix_clicks_toggle_with_vim_off_and_never_in_reader_view() {
        let (mut e, _) = plain_with_host("- [ ] a");
        let p = zoned(pos(0, 6), Zone::Prefix);
        send(&mut e, MouseKind::Down, p.clone(), Mods::default());
        send(&mut e, MouseKind::Up, p.clone(), Mods::default());
        assert_eq!(e.text(), "- [x] a");
        let mut r = build_reader("- [ ] a");
        send(&mut r, MouseKind::Down, p.clone(), Mods::default());
        send(&mut r, MouseKind::Up, p, Mods::default());
        assert_eq!(r.text(), "- [ ] a");
    }

    /// `see [docs](https://example.com/d) now`: the link text `docs` is
    /// source bytes 5..9.
    const LINKED: &str = "see [docs](https://example.com/d) now";

    fn on_link() -> Option<MouseHit> {
        zoned(pos(0, 6), Zone::Link(5..9))
    }

    #[test]
    fn a_plain_click_on_a_link_opens_it_in_normal() {
        let (mut e, host) = ed_with_host(LINKED);
        send(&mut e, MouseKind::Down, on_link(), Mods::default());
        assert!(host.state().opened.is_empty(), "nothing opens on Down");
        send(&mut e, MouseKind::Up, on_link(), Mods::default());
        assert_eq!(host.state().opened, vec!["https://example.com/d"]);
        assert_eq!(e.cursor(), pos(0, 0), "opening leaves the cursor alone");
    }

    #[test]
    fn the_link_under_the_point_opens_of_two_side_by_side() {
        let (mut e, host) = ed_with_host("[a](http://x)[b](http://y)");
        // One run of link spans, 1..15, from the renderer; `b` is byte 14.
        let hit = Some(MouseHit {
            pos: pos(0, 14),
            under: pos(0, 14),
            zone: Zone::Link(1..15),
        });
        send(&mut e, MouseKind::Down, hit.clone(), Mods::default());
        send(&mut e, MouseKind::Up, hit, Mods::default());
        assert_eq!(host.state().opened, vec!["http://y"]);
    }

    #[test]
    fn a_double_click_on_a_link_opens_it_once() {
        let (mut e, host) = ed_with_host(LINKED);
        send(&mut e, MouseKind::Down, on_link(), Mods::default());
        send(&mut e, MouseKind::Up, on_link(), Mods::default());
        send(&mut e, MouseKind::DoubleClick, on_link(), Mods::default());
        send(&mut e, MouseKind::Up, on_link(), Mods::default());
        assert_eq!(host.state().opened, vec!["https://example.com/d"]);
        assert_eq!(e.mode(), Mode::Normal);
        // In Insert, where a plain click places the caret, it selects the
        // word.
        let (mut e, _) = ed_with_host(LINKED);
        keys(&mut e, "i");
        send(&mut e, MouseKind::DoubleClick, on_link(), Mods::default());
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(sel(&e), Some((pos(0, 5), pos(0, 8))));
    }

    #[test]
    fn insert_mode_needs_primary_to_open_a_link() {
        let (mut e, host) = ed_with_host(LINKED);
        keys(&mut e, "i");
        send(&mut e, MouseKind::Down, on_link(), Mods::default());
        send(&mut e, MouseKind::Up, on_link(), Mods::default());
        assert!(host.state().opened.is_empty());
        assert_eq!(e.cursor(), pos(0, 6), "a plain click places the caret");
        send(&mut e, MouseKind::Down, on_link(), cmd());
        send(&mut e, MouseKind::Up, on_link(), cmd());
        assert_eq!(host.state().opened, vec!["https://example.com/d"]);
    }

    #[test]
    fn a_plain_click_opens_a_link_with_vim_off_and_in_reader_view() {
        let (mut e, host) = plain_with_host(LINKED);
        send(&mut e, MouseKind::Down, on_link(), Mods::default());
        send(&mut e, MouseKind::Up, on_link(), Mods::default());
        assert_eq!(host.state().opened, vec!["https://example.com/d"]);
        let dir = std::env::temp_dir();
        let (mut r, host) = crate::testkit::ed_at(&dir.join("abc_def-reader.md"), LINKED, true);
        send(&mut r, MouseKind::Down, on_link(), Mods::default());
        send(&mut r, MouseKind::Up, on_link(), Mods::default());
        assert_eq!(host.state().opened, vec!["https://example.com/d"]);
    }

    #[test]
    fn a_drag_from_a_link_selects_and_opens_nothing() {
        let (mut e, host) = ed_with_host(LINKED);
        send(&mut e, MouseKind::Down, on_link(), Mods::default());
        send(
            &mut e,
            MouseKind::Drag,
            on(pos(0, 2), pos(0, 2)),
            Mods::default(),
        );
        send(
            &mut e,
            MouseKind::Up,
            on(pos(0, 2), pos(0, 2)),
            Mods::default(),
        );
        assert!(host.state().opened.is_empty());
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(sel(&e), Some((pos(0, 6), pos(0, 2))));
    }

    #[test]
    fn vim_off_drag_and_double_click_select_without_visual() {
        let (mut e, _) = plain_with_host("hello world");
        send(
            &mut e,
            MouseKind::Down,
            on(pos(0, 1), pos(0, 1)),
            Mods::default(),
        );
        send(
            &mut e,
            MouseKind::Drag,
            on(pos(0, 4), pos(0, 3)),
            Mods::default(),
        );
        send(
            &mut e,
            MouseKind::Up,
            on(pos(0, 4), pos(0, 3)),
            Mods::default(),
        );
        assert_eq!(e.mode(), Mode::Insert);
        // The selection runs to the caret position, its end open.
        assert_eq!(sel(&e), Some((pos(0, 1), pos(0, 3))));
        keys(&mut e, "X");
        assert_eq!(e.text(), "hXo world");
        let (mut e, _) = plain_with_host("hello world");
        send(
            &mut e,
            MouseKind::DoubleClick,
            on(pos(0, 8), pos(0, 8)),
            Mods::default(),
        );
        assert_eq!(sel(&e), Some((pos(0, 6), pos(0, 10))));
        assert_eq!(e.cursor(), pos(0, 11));
        // A click drops the selection.
        click(&mut e, pos(0, 2));
        assert_eq!(e.selection(), None);
        assert_eq!(e.cursor(), pos(0, 2));
    }

    #[test]
    fn clicks_do_nothing_while_a_line_is_typed_or_a_prompt_asks() {
        // `:`, `/` and the link prompt.
        for seq in [":s", "/x", "<D-l>"] {
            let mut e = ed("hello world");
            keys(&mut e, seq);
            let mode = e.mode();
            let fx = click(&mut e, pos(0, 6));
            assert_eq!(e.cursor(), pos(0, 0), "{seq}");
            assert_eq!(e.mode(), mode, "{seq}");
            assert_eq!(fx, Effects::default(), "{seq}");
            send(
                &mut e,
                MouseKind::Down,
                on(pos(0, 1), pos(0, 1)),
                Mods::default(),
            );
            send(
                &mut e,
                MouseKind::Drag,
                on(pos(0, 5), pos(0, 5)),
                Mods::default(),
            );
            assert_eq!(e.selection(), None, "{seq}");
        }
        // `:s///c`'s confirm.
        let mut e = ed("a a");
        keys(&mut e, ":s/a/b/gc<CR>");
        assert_eq!(e.mode(), Mode::Confirm);
        let at = e.cursor();
        click(&mut e, pos(0, 1));
        assert_eq!(e.cursor(), at);
        // A prompt (the open-link question).
        let (mut e, host) = ed_with_host("see [x](ftp://h/f) now");
        send(
            &mut e,
            MouseKind::Down,
            zoned(pos(0, 5), Zone::Link(5..6)),
            Mods::default(),
        );
        send(
            &mut e,
            MouseKind::Up,
            zoned(pos(0, 5), Zone::Link(5..6)),
            Mods::default(),
        );
        assert_eq!(e.mode(), Mode::Prompt);
        click(&mut e, pos(0, 1));
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.mode(), Mode::Prompt);
        assert!(host.state().opened.is_empty());
        // The wheel still scrolls.
        let fx = e.handle(Input::Scroll { dy_pt: 12.0 });
        assert_eq!(fx.scroll, Some(Scroll::Points(12)));
    }

    #[test]
    fn the_wheel_scrolls_whole_points_and_keeps_the_rest() {
        let mut e = ed("a");
        let fx = e.handle(Input::Scroll { dy_pt: 0.6 });
        assert_eq!(fx.scroll, None);
        let fx = e.handle(Input::Scroll { dy_pt: 0.6 });
        assert_eq!(fx.scroll, Some(Scroll::Points(1)));
        let fx = e.handle(Input::Scroll { dy_pt: -3.4 });
        assert_eq!(fx.scroll, Some(Scroll::Points(-3)));
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn the_wheel_scrolls_reader_view_and_not_the_help() {
        let mut r = build_reader("a\nb");
        let fx = r.handle(Input::Scroll { dy_pt: 5.0 });
        assert_eq!(fx.scroll, Some(Scroll::Points(5)));
        let mut e = ed("a");
        keys(&mut e, ":help<CR>");
        let fx = e.handle(Input::Scroll { dy_pt: 5.0 });
        assert_eq!(fx.scroll, None);
    }

    #[test]
    fn picker_hover_selects_and_a_click_picks() {
        let mut e = ed("");
        keys(&mut e, ":table<CR>");
        let cell = |col, row| zoned(pos(0, 0), Zone::Picker { col, row });
        let fx = send(&mut e, MouseKind::Move, cell(2, 1), Mods::default());
        assert_eq!(e.table_picker().unwrap().selected(), (2, 1));
        assert_eq!(fx.redraw, crate::effects::Redraw::All);
        // Moving within the same cell repaints nothing.
        let fx = send(&mut e, MouseKind::Move, cell(2, 1), Mods::default());
        assert_eq!(fx.redraw, crate::effects::Redraw::None);
        // Clicks off the grid do nothing while it shows.
        click(&mut e, pos(0, 0));
        assert!(e.table_picker().is_some());
        send(&mut e, MouseKind::Down, cell(1, 1), Mods::default());
        assert!(e.table_picker().is_some(), "it picks on Up");
        send(&mut e, MouseKind::Up, cell(1, 1), Mods::default());
        assert!(e.table_picker().is_none());
        assert_eq!(
            e.text(),
            "| Column 1 | Column 2 |\n| --- | --- |\n|  |  |\n|  |  |"
        );
        assert_eq!(e.history().undo_len(), 1);
    }

    #[test]
    fn no_picker_in_reader_view() {
        let mut r = build_reader("");
        let cell = zoned(pos(0, 0), Zone::Picker { col: 1, row: 1 });
        send(&mut r, MouseKind::Down, cell.clone(), Mods::default());
        send(&mut r, MouseKind::Up, cell, Mods::default());
        assert_eq!(r.text(), "");
    }

    #[test]
    fn a_key_while_the_button_is_held_reveals_first() {
        let mut e = ed("plain\n**bold** x");
        send(
            &mut e,
            MouseKind::Down,
            on(pos(1, 3), pos(1, 3)),
            Mods::default(),
        );
        assert_eq!(e.display(1).spans[0].src.start, 2);
        keys(&mut e, "l");
        assert_eq!(e.cursor(), pos(1, 4));
        assert_eq!(e.display(1).spans[0].src.start, 0);
    }

    #[test]
    fn a_click_outside_the_text_does_nothing() {
        let mut e = ed("hello");
        let fx = send(&mut e, MouseKind::Down, None, Mods::default());
        assert_eq!(fx, Effects::default());
        let fx = send(&mut e, MouseKind::Up, None, Mods::default());
        assert_eq!(fx, Effects::default());
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn status_is_unchanged_by_an_ignored_click() {
        let mut e = ed("x");
        keys(&mut e, ":");
        click(&mut e, pos(0, 0));
        assert!(matches!(e.status().left, StatusLeft::Input { prefix, .. } if prefix == ":"));
    }
}

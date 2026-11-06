//! Operators (`d c y`) over a range, plus `x` and putting (`p P`).
//!
//! The engine works out the range (`OpRange`) from a motion or a doubled
//! operator key; `operate` and `put` carry the edit out through an
//! `EditCtx`, which records every replacement in the history and keeps the
//! unnamed register and the system clipboard in step.
//!
//! Every position is snapped with `Buffer::clamp` and moved with the
//! grapheme helpers before it reaches `Buffer::replace`, so no operator
//! splits a cluster.

use std::ops::Range;

use crate::buffer::{Buffer, Edit, Pos};
use crate::grapheme;
use crate::host::Host;
use crate::lists::{self, EditPlan};
use crate::markdown::Blocks;
use crate::tables::{self, CellMove};
use crate::undo::{self, History};
use crate::vim::motion::{self, Motion, MotionKind};
use crate::vim::register::{Register, Registers};
use crate::vim::{Cursor, rest};

/// The most a single put may insert. A count that would go past it puts
/// nothing rather than exhausting memory.
///
/// Fuzz builds (`cfg(fuzzing)`) use a smaller limit (1 MiB) so a run cannot
/// spend its time on huge puts.
#[cfg(not(fuzzing))]
const MAX_PUT_BYTES: usize = 1 << 26;
#[cfg(fuzzing)]
const MAX_PUT_BYTES: usize = 1 << 20;

/// The most lines a single put may add. Each line costs about 175 bytes of
/// memory on top of its text (the `String`, the Markdown line kind and the
/// undo record), so the byte cap alone lets one counted put of a tiny line
/// allocate gigabytes. A put past this adds nothing, like the byte cap.
///
/// Fuzz builds (`cfg(fuzzing)`) use a smaller limit (16,384 lines) so a run
/// cannot spend its time on huge puts.
#[cfg(not(fuzzing))]
const MAX_PUT_LINES: usize = 1 << 18;
#[cfg(fuzzing)]
const MAX_PUT_LINES: usize = 1 << 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

impl Operator {
    /// The operator a Normal-mode key starts.
    pub fn from_key(c: char) -> Option<Operator> {
        match c {
            'd' => Some(Operator::Delete),
            'c' => Some(Operator::Change),
            'y' => Some(Operator::Yank),
            _ => None,
        }
    }

    pub fn key(self) -> char {
        match self {
            Operator::Delete => 'd',
            Operator::Change => 'c',
            Operator::Yank => 'y',
        }
    }
}

/// The text an operator acts on. Charwise, `start..end` with `end`
/// exclusive. Linewise, every line from `start.line` to `end.line`; the
/// columns are not used to cut, but `start` is the cursor vim has at the
/// change (what undo restores): the motion's landing for `k` and `gg`, the
/// first non-blank for `dd`, the cursor for forward motions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpRange {
    pub start: Pos,
    pub end: Pos,
    pub linewise: bool,
}

impl OpRange {
    /// `count` whole lines from the cursor's `line` and `col`, as far as
    /// the buffer goes (`dd cc yy`). As in vim, where the operator's range
    /// starts at the lesser of the cursor and where `dd` leaves it (the
    /// first non-blank of the last line), `start.col` is the first
    /// non-blank for one line (or the cursor's column if that is less), and
    /// the cursor's column for more.
    pub fn lines(buf: &Buffer, line: usize, col: usize, count: usize) -> OpRange {
        let last = buf.line_count() - 1;
        let line = line.min(last);
        let end = line.saturating_add(count.max(1) - 1).min(last);
        let col = if end == line {
            col.min(motion::caret_col(buf.line(line)))
        } else {
            col
        };
        OpRange {
            start: Pos { line, col },
            end: Pos { line: end, col: 0 },
            linewise: true,
        }
    }

    /// Up to `count` graphemes from `pos`, stopping at the line end (`x`,
    /// and `l` under an operator). `None` when there is nothing under the
    /// cursor.
    pub fn chars(buf: &Buffer, pos: Pos, count: usize) -> Option<OpRange> {
        let start = buf.clamp(pos);
        let line = buf.line(start.line);
        if start.col >= line.len() {
            return None;
        }
        let mut col = start.col;
        for _ in 0..count.max(1) {
            if col >= line.len() {
                break;
            }
            col = grapheme::next(line, col);
        }
        Some(OpRange {
            start,
            end: Pos { col, ..start },
            linewise: false,
        })
    }

    /// What `op` acts on when motion `m` (repeated `count` times) moves
    /// from `cur`. `None` when the motion fails, so the operator does
    /// nothing.
    ///
    /// Inclusive motions (`e $`) take the grapheme they land on; linewise
    /// ones (`j k gg G`) take whole lines. `l` may reach the line end, so
    /// `dl` takes the last grapheme. `cw` on a word acts as `ce`. `w` stops
    /// at the end of the line the last word it moved over is on. An
    /// exclusive range ending at the start of a later line ends at the end
    /// of the line before instead, and becomes linewise when it starts at
    /// or before the first non-blank (vim's `o_v` rules).
    pub fn from_motion(
        op: Operator,
        m: Motion,
        buf: &Buffer,
        cur: Cursor,
        count: usize,
    ) -> Option<OpRange> {
        let count = count.max(1);
        let p = buf.clamp(cur.pos);
        let cur = Cursor { pos: p, ..cur };
        match m {
            Motion::Right => return OpRange::chars(buf, p, count),
            Motion::WordFwd if op == Operator::Change => {
                if !motion::on_blank(buf, p) {
                    return Some(change_word(buf, p, count));
                }
                if count == 1 {
                    // Vim changes just the one blank. On an empty line
                    // there is none: change nothing and type.
                    return OpRange::chars(buf, p, 1).or(Some(OpRange {
                        start: p,
                        end: p,
                        linewise: false,
                    }));
                }
                return word_range(buf, cur, count);
            }
            Motion::WordFwd => return word_range(buf, cur, count),
            _ => {}
        }
        let r = motion::apply(m, buf, cur, count);
        r.moved.then(|| span(buf, p, r.cursor.pos, r.kind))
    }

    /// The exclusive charwise range between `from` and `to` (in either
    /// order), as an operator over a search match takes it, with vim's
    /// `o_v` rules at a later line's start. `None` when they are the same
    /// place.
    pub fn exclusive(buf: &Buffer, from: Pos, to: Pos) -> Option<OpRange> {
        let (from, to) = (buf.clamp(from), buf.clamp(to));
        (from != to).then(|| span(buf, from, to, MotionKind::Exclusive))
    }
}

/// `w` under an operator: the last step stops at its line's end.
fn word_range(buf: &Buffer, cur: Cursor, count: usize) -> Option<OpRange> {
    let mid = if count > 1 {
        motion::apply(Motion::WordFwd, buf, cur, count - 1)
            .cursor
            .pos
    } else {
        cur.pos
    };
    let end = motion::word_fwd_for_operator(buf, mid);
    (end != cur.pos).then(|| span(buf, cur.pos, end, MotionKind::Exclusive))
}

/// `cw` on a word: to the end of the word, where a cursor already on a
/// word's last grapheme counts as one word done.
fn change_word(buf: &Buffer, p: Pos, count: usize) -> OpRange {
    let n = if motion::at_word_end(buf, p) {
        count - 1
    } else {
        count
    };
    let e = if n == 0 {
        p
    } else {
        motion::apply(Motion::WordEnd, buf, Cursor::at(buf, p), n)
            .cursor
            .pos
            .max(p)
    };
    span(buf, p, e, MotionKind::Inclusive)
}

/// The range between `from` and `to` (in either order) for a motion of
/// `kind`.
fn span(buf: &Buffer, from: Pos, to: Pos, kind: MotionKind) -> OpRange {
    let (a, b) = if to < from { (to, from) } else { (from, to) };
    match kind {
        MotionKind::Linewise => OpRange {
            start: a,
            end: Pos {
                line: b.line,
                col: 0,
            },
            linewise: true,
        },
        MotionKind::Inclusive => OpRange {
            start: a,
            end: Pos {
                col: grapheme::next(buf.line(b.line), b.col),
                ..b
            },
            linewise: false,
        },
        MotionKind::Exclusive if b.col == 0 && b.line > a.line => {
            let last = b.line - 1;
            let in_indent = buf.line(a.line)[..a.col]
                .chars()
                .all(|c| c == ' ' || c == '\t');
            OpRange {
                start: if in_indent { Pos { col: 0, ..a } } else { a },
                end: Pos {
                    line: last,
                    col: buf.line(last).len(),
                },
                linewise: in_indent,
            }
        }
        MotionKind::Exclusive => OpRange {
            start: a,
            end: b,
            linewise: false,
        },
    }
}

/// The lines an `EditCtx::replace` touched, for the editor to bring its
/// block structure and repaint up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Touched {
    /// The first line replaced.
    pub start_line: usize,
    /// The last line of the replaced range, in the buffer before the edit.
    pub old_end_line: usize,
    /// The last line of the inserted text, in the buffer after the edit.
    pub new_end_line: usize,
    /// The change in line count.
    pub delta: isize,
}

/// What an edit works with. Every change to the buffer goes through
/// `replace`, which records it in the history.
pub struct EditCtx<'a> {
    pub buf: &'a mut Buffer,
    /// The cursor: where the command started, then where it leaves it.
    pub cursor: Pos,
    pub history: &'a mut History,
    pub registers: &'a mut Registers,
    pub host: &'a mut dyn Host,
    /// The named register (`"a` to `"z`) the command writes or puts, if it
    /// names one.
    pub register: Option<char>,
    touched: Vec<Touched>,
}

impl<'a> EditCtx<'a> {
    pub fn new(
        buf: &'a mut Buffer,
        cursor: Pos,
        history: &'a mut History,
        registers: &'a mut Registers,
        host: &'a mut dyn Host,
    ) -> EditCtx<'a> {
        EditCtx {
            buf,
            cursor,
            history,
            registers,
            host,
            register: None,
            touched: Vec::new(),
        }
    }

    /// Replaces `r` with `text`, records the change and leaves the cursor
    /// after the inserted text. `r` must already sit on grapheme
    /// boundaries (R12).
    pub fn replace(&mut self, r: Range<Pos>, text: &str) -> Edit {
        let before = self.buf.line_count();
        let old_end_line = r.end.line.min(before - 1);
        let inverse = self.buf.replace(r, text);
        let delta = self.buf.line_count() as isize - before as isize;
        self.history
            .record(self.cursor, inverse.clone(), undo::forward(&inverse, text));
        self.cursor = inverse.range.end;
        self.touched.push(Touched {
            start_line: inverse.range.start.line,
            old_end_line,
            new_end_line: inverse.range.end.line,
            delta,
        });
        inverse
    }

    /// Carries out a plan from `crate::lists` (or any other planner) as one
    /// undo step, and returns where it leaves the cursor.
    ///
    /// The edits go through `replace` in order, so each is recorded and
    /// logged as a line shift, and each one's range is read in the buffer
    /// the edits before it left (the `EditPlan` contract). They make one
    /// group: a group of their own, or part of the one already open (an
    /// Insert session). Undo puts the cursor back where it was before the
    /// plan. Every edit is in `touched`, so the editor brings its blocks up
    /// to date (several edits rebuild them). The cursor goes to
    /// `plan.cursor`, clamped onto a grapheme; an empty plan only moves it.
    pub fn apply_plan(&mut self, plan: EditPlan) -> Pos {
        self.history.begin();
        for e in &plan.edits {
            self.replace(e.range.clone(), &e.text);
        }
        self.history.end();
        self.cursor = self.buf.clamp(plan.cursor);
        self.cursor
    }

    /// Moves the cursor to where the command leaves it.
    pub fn place(&mut self, pos: Pos) {
        self.cursor = pos;
    }

    /// The edits made through this context, oldest first.
    pub fn touched(&self) -> &[Touched] {
        &self.touched
    }

    /// Writes the unnamed register and the system clipboard or, when the
    /// command names a register, that register and the unnamed one (as vim
    /// does), leaving the clipboard alone.
    pub fn write_register(&mut self, reg: Register) {
        match self.register {
            Some(name) => {
                self.registers.set(Some(name), reg.clone());
                self.registers.set(None, reg);
            }
            None => write_register(self.host, self.registers, reg),
        }
    }

    /// What `p` and `P` put: the named register, or else the clipboard,
    /// read as `Registers::from_clipboard` says.
    pub fn register_to_put(&mut self) -> Option<Register> {
        if let Some(name) = self.register {
            return self.registers.get(Some(name));
        }
        let clip = self.host.clipboard_get();
        self.registers.from_clipboard(clip)
    }
}

/// Writes `reg` to the unnamed register and the system clipboard, which
/// mirrors it.
pub fn write_register(host: &mut dyn Host, registers: &mut Registers, reg: Register) {
    host.clipboard_set(&reg.text);
    registers.wrote_clipboard(&reg);
    registers.set(None, reg);
}

/// A key that acts on list items, tasks and fences (`crate::lists`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKey {
    /// Enter in Insert: continue a list, leave an empty item, or close a
    /// fence.
    Enter,
    /// Tab: indent a list item.
    Tab,
    /// Shift+Tab: outdent a list item.
    BackTab,
    /// `gx` or Ctrl+Space: toggle a task's checkbox.
    Toggle,
}

/// The plan `key` makes with the cursor at `cursor`, or `None` when the
/// line is not one it acts on. `blocks` must describe `buf` as it is now:
/// `crate::lists` only notices a mismatch it can see.
pub fn list_plan(key: ListKey, buf: &Buffer, blocks: &Blocks, cursor: Pos) -> Option<EditPlan> {
    let line = buf.clamp(cursor).line;
    match key {
        ListKey::Enter => lists::enter(buf, blocks, cursor),
        ListKey::Tab => lists::indent(buf, blocks, line, cursor),
        ListKey::BackTab => lists::outdent(buf, blocks, line, cursor),
        ListKey::Toggle => lists::toggle_task(buf, blocks, line, cursor),
    }
}

/// Where `key` goes in a table (`crate::tables`) with the cursor at
/// `cursor`: Tab to the next cell, Shift+Tab to the previous one (staying
/// put at the first) and Enter, in Insert, down a row; past the last cell
/// or row they grow the table. `None` off a table, and for the task
/// toggle, which tables do not take. `blocks` must describe `buf` as it is
/// now: the `crate::tables` moves do not check.
pub fn table_move(key: ListKey, buf: &Buffer, blocks: &Blocks, cursor: Pos) -> Option<CellMove> {
    let cursor = buf.clamp(cursor);
    let info = tables::table_at(blocks, cursor.line)?;
    Some(match key {
        ListKey::Tab => tables::next_cell(info, buf, cursor),
        ListKey::BackTab => CellMove::Move(tables::prev_cell(info, buf, cursor).unwrap_or(cursor)),
        ListKey::Enter => tables::next_row(info, buf, cursor),
        ListKey::Toggle => return None,
    })
}

/// Carries out a `table_move`: a move only places the cursor (nothing to
/// undo); a grown table is a plan, one undo step (`EditCtx::apply_plan`).
/// Returns where the cursor goes.
pub fn apply_move(ctx: &mut EditCtx, mv: CellMove) -> Pos {
    match mv {
        CellMove::Move(p) => {
            let p = ctx.buf.clamp(p);
            ctx.place(p);
            p
        }
        CellMove::Edit(plan) => ctx.apply_plan(plan),
    }
}

/// `key` pressed in Insert: `plan` (from `list_plan`) when there is one,
/// or else what the key does on a plain line: Enter splits it, Tab puts in
/// a tab, Shift+Tab and Ctrl+Space do nothing. Returns where the cursor
/// goes.
pub fn insert_list_key(ctx: &mut EditCtx, key: ListKey, plan: Option<EditPlan>) -> Pos {
    if let Some(plan) = plan {
        return ctx.apply_plan(plan);
    }
    let at = ctx.buf.clamp(ctx.cursor);
    let cursor = match key {
        ListKey::Enter => ctx.replace(at..at, "\n").range.end,
        ListKey::Tab => ctx.replace(at..at, "\t").range.end,
        ListKey::BackTab | ListKey::Toggle => at,
    };
    ctx.place(cursor);
    cursor
}

/// What Backspace in Insert deletes with the cursor at `cursor`: the
/// grapheme before it, or at a line's start the line break before it.
/// `None` at the very start.
pub fn backspace_range(buf: &Buffer, cursor: Pos) -> Option<Range<Pos>> {
    let c = buf.clamp(cursor);
    let from = if c.col > 0 {
        Pos {
            line: c.line,
            col: grapheme::prev(buf.line(c.line), c.col),
        }
    } else if c.line > 0 {
        Pos {
            line: c.line - 1,
            col: buf.line(c.line - 1).len(),
        }
    } else {
        return None;
    };
    Some(from..c)
}

/// What a command left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpOutcome {
    pub cursor: Pos,
    /// True when the command enters Insert (`c`).
    pub insert: bool,
}

/// Applies `op` to `range`. The register (and the clipboard) get exactly
/// the text the range covers; a linewise range's text is its lines, each
/// ending in `\n`. An empty charwise range changes nothing, though `c`
/// still enters Insert.
pub fn operate(ctx: &mut EditCtx, op: Operator, range: OpRange) -> OpOutcome {
    if range.linewise {
        linewise(ctx, op, range)
    } else {
        charwise(ctx, op, range)
    }
}

fn charwise(ctx: &mut EditCtx, op: Operator, range: OpRange) -> OpOutcome {
    let start = ctx.buf.clamp(range.start);
    let end = ctx.buf.clamp(range.end).max(start);
    if start == end {
        return OpOutcome {
            cursor: ctx.cursor,
            insert: op == Operator::Change,
        };
    }
    let text = ctx.buf.text_range(start..end);
    ctx.write_register(Register::chars(text));
    match op {
        Operator::Yank => {}
        Operator::Delete | Operator::Change => {
            // Vim moves the cursor to the start of the range first, which
            // is where undo puts it back.
            ctx.cursor = start;
            ctx.replace(start..end, "");
        }
    }
    let cursor = if op == Operator::Change {
        start
    } else {
        rest(ctx.buf, start)
    };
    ctx.place(cursor);
    OpOutcome {
        cursor,
        insert: op == Operator::Change,
    }
}

fn linewise(ctx: &mut EditCtx, op: Operator, range: OpRange) -> OpOutcome {
    let last = ctx.buf.line_count() - 1;
    let a = range.start.line.min(last);
    let b = range.end.line.min(last).max(a);
    if op != Operator::Yank {
        // Undo and redo restore the cursor vim had at the change.
        ctx.cursor = Pos {
            line: a,
            col: range.start.col,
        };
        if op == Operator::Change && b > a {
            // Vim's linewise change first deletes the lines after the
            // first with the cursor moved down onto them, and that is the
            // cursor undo restores.
            ctx.cursor.line = a + 1;
        }
    }
    let line_end = |buf: &Buffer, line: usize| Pos {
        line,
        col: buf.line(line).len(),
    };
    let mut text = ctx
        .buf
        .text_range(Pos { line: a, col: 0 }..line_end(ctx.buf, b));
    text.push('\n');
    ctx.write_register(Register::lines(text));
    let cursor = match op {
        Operator::Yank => rest(
            ctx.buf,
            Pos {
                line: a,
                col: if a == ctx.cursor.line {
                    ctx.cursor.col
                } else {
                    motion::caret_col(ctx.buf.line(a))
                },
            },
        ),
        Operator::Change => {
            ctx.replace(Pos { line: a, col: 0 }..line_end(ctx.buf, b), "");
            Pos { line: a, col: 0 }
        }
        Operator::Delete => {
            let line = if b < last {
                ctx.replace(
                    Pos { line: a, col: 0 }..Pos {
                        line: b + 1,
                        col: 0,
                    },
                    "",
                );
                a
            } else if a > 0 {
                // The last lines: take the line break before them, so no
                // empty line is left behind.
                let from = line_end(ctx.buf, a - 1);
                ctx.replace(from..line_end(ctx.buf, b), "");
                a - 1
            } else {
                ctx.replace(Pos::default()..line_end(ctx.buf, b), "");
                0
            };
            Pos {
                line,
                col: motion::caret_col(ctx.buf.line(line)),
            }
        }
    };
    ctx.place(cursor);
    OpOutcome {
        cursor,
        insert: op == Operator::Change,
    }
}

/// `o` (below the cursor's line) and `O` (above): opens a plain empty line
/// and leaves the cursor on it, in Insert.
pub fn open_line(ctx: &mut EditCtx, above: bool) -> OpOutcome {
    let line = ctx.buf.clamp(ctx.cursor).line;
    let at = if above {
        Pos { line, col: 0 }
    } else {
        Pos {
            line,
            col: ctx.buf.line(line).len(),
        }
    };
    ctx.replace(at..at, "\n");
    let cursor = if above {
        Pos { line, col: 0 }
    } else {
        Pos {
            line: line + 1,
            col: 0,
        }
    };
    ctx.place(cursor);
    OpOutcome {
        cursor,
        insert: true,
    }
}

/// `p` (after the cursor, or below its line) and `P` (before, or above),
/// `count` times, from the clipboard.
pub fn put(ctx: &mut EditCtx, before: bool, count: usize) -> OpOutcome {
    let stay = OpOutcome {
        cursor: ctx.cursor,
        insert: false,
    };
    let Some(reg) = ctx.register_to_put() else {
        return stay;
    };
    let count = count.max(1);
    let mut text = reg.text.replace("\r\n", "\n");
    if reg.linewise && !text.ends_with('\n') {
        text.push('\n');
    }
    if text
        .len()
        .checked_mul(count)
        .is_none_or(|n| n > MAX_PUT_BYTES)
    {
        return stay;
    }
    // A put splits its text at each `\n`, so that is how many lines it adds.
    if text
        .matches('\n')
        .count()
        .checked_mul(count)
        .is_none_or(|n| n > MAX_PUT_LINES)
    {
        return stay;
    }
    let text = text.repeat(count);
    let p = ctx.buf.clamp(ctx.cursor);
    let cursor = if reg.linewise {
        let first = if before {
            ctx.replace(
                Pos {
                    line: p.line,
                    col: 0,
                }..Pos {
                    line: p.line,
                    col: 0,
                },
                &text,
            );
            p.line
        } else if p.line + 1 < ctx.buf.line_count() {
            let at = Pos {
                line: p.line + 1,
                col: 0,
            };
            ctx.replace(at..at, &text);
            p.line + 1
        } else {
            // Below the last line: the break goes first.
            let at = Pos {
                line: p.line,
                col: ctx.buf.line(p.line).len(),
            };
            let body = &text[..text.len() - 1];
            ctx.replace(at..at, &format!("\n{body}"));
            p.line + 1
        };
        Pos {
            line: first,
            col: motion::caret_col(ctx.buf.line(first)),
        }
    } else {
        let line = ctx.buf.line(p.line);
        let col = if before || line.is_empty() {
            p.col
        } else {
            grapheme::next(line, p.col)
        };
        let at = Pos { col, ..p };
        let inverse = ctx.replace(at..at, &text);
        let r = inverse.range;
        if r.start.line == r.end.line {
            // On the last grapheme put.
            Pos {
                col: grapheme::prev(ctx.buf.line(r.end.line), r.end.col),
                ..r.end
            }
        } else {
            r.start
        }
    };
    let cursor = rest(ctx.buf, cursor);
    ctx.place(cursor);
    OpOutcome {
        cursor,
        insert: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::vim::Mode;

    // `EditCtx::apply_plan`.

    /// Applies `plan` to `text` with the cursor at `cursor`, inside a group
    /// opened round it when `nested`, and returns the buffer, the history,
    /// where the plan left the cursor and the lines it touched.
    fn plan_on(
        text: &str,
        cursor: Pos,
        plan: EditPlan,
        nested: bool,
    ) -> (Buffer, History, Pos, Vec<Touched>) {
        let mut buf = Buffer::from_text(text);
        let mut history = History::new();
        let mut registers = Registers::new();
        let mut host = FakeHost::default();
        if nested {
            history.begin();
        }
        let mut ctx = EditCtx::new(&mut buf, cursor, &mut history, &mut registers, &mut host);
        let at = ctx.apply_plan(plan);
        assert_eq!(ctx.cursor, at);
        let touched = ctx.touched().to_vec();
        if nested {
            // The plan joined the outer group, which is still open.
            assert_eq!(history.undo_len(), 0);
            history.end();
        }
        (buf, history, at, touched)
    }

    fn edit(range: Range<Pos>, text: &str) -> Edit {
        Edit {
            range,
            text: text.into(),
        }
    }

    #[test]
    fn apply_plan_applies_edits_in_sequence_as_one_undo_step() {
        // The second edit's line exists only once the first has run.
        let plan = EditPlan {
            edits: vec![
                edit(pos(0, 2)..pos(0, 2), "\nxy"),
                edit(pos(1, 2)..pos(1, 2), "!"),
                edit(pos(2, 0)..pos(2, 1), ""),
            ],
            cursor: pos(1, 3),
        };
        let (mut buf, mut history, at, touched) = plan_on("ab\ncd", pos(0, 1), plan, false);
        assert_eq!(buf.to_text(), "ab\nxy!\nd");
        assert_eq!(at, pos(1, 3));
        assert_eq!(touched.len(), 3);
        assert_eq!(history.undo_len(), 1);
        // Undo puts the text and the cursor back.
        assert_eq!(history.undo(&mut buf), Some(pos(0, 1)));
        assert_eq!(buf.to_text(), "ab\ncd");
        history.redo(&mut buf);
        assert_eq!(buf.to_text(), "ab\nxy!\nd");
    }

    #[test]
    fn apply_plan_joins_an_open_group() {
        let plan = EditPlan {
            edits: vec![edit(pos(0, 0)..pos(0, 0), "  ")],
            cursor: pos(0, 2),
        };
        let (mut buf, mut history, _, _) = plan_on("- a", pos(0, 0), plan, true);
        assert_eq!(buf.to_text(), "  - a");
        assert_eq!(history.undo_len(), 1);
        history.undo(&mut buf);
        assert_eq!(buf.to_text(), "- a");
    }

    #[test]
    fn apply_plan_keeps_the_cursor_on_a_grapheme() {
        // A cursor inside the family emoji, or past the line, is clamped.
        let fam = "👩\u{200d}👩\u{200d}👧";
        let plan = EditPlan {
            edits: vec![edit(pos(0, 0)..pos(0, 0), "x")],
            cursor: pos(0, 5),
        };
        let (_, _, at, _) = plan_on(fam, pos(0, 0), plan, false);
        assert_eq!(at, pos(0, 1));
        let plan = EditPlan {
            edits: vec![edit(pos(0, 0)..pos(0, 0), "x")],
            cursor: pos(3, 99),
        };
        let (buf, _, at, _) = plan_on(fam, pos(0, 0), plan, false);
        assert_eq!(at, pos(0, buf.line(0).len()));
    }

    #[test]
    fn an_empty_plan_changes_nothing() {
        let plan = EditPlan {
            edits: Vec::new(),
            cursor: pos(0, 1),
        };
        let (buf, history, at, touched) = plan_on("ab", pos(0, 0), plan, false);
        assert_eq!(buf.to_text(), "ab");
        assert_eq!(history.undo_len(), 0);
        assert!(touched.is_empty());
        assert_eq!(at, pos(0, 1));
    }

    // The brief's cases.

    #[test]
    fn d_dollar_is_inclusive() {
        let mut e = ed("abc def");
        keys(&mut e, "wd$");
        assert_eq!(e.text(), "abc ");
    }

    #[test]
    fn dj_is_linewise() {
        let mut e = ed("a\nb\nc");
        keys(&mut e, "dj");
        assert_eq!(e.text(), "c");
    }

    #[test]
    fn compound_count_multiplies() {
        let mut e = ed("a b c d e f g h");
        keys(&mut e, "2d3w");
        assert_eq!(e.text(), "g h");
    }

    #[test]
    fn dw_at_line_end_records_exactly_what_it_removed() {
        let mut e = ed("foo bar\nbaz");
        keys(&mut e, "wdw");
        assert_eq!(e.text(), "foo \nbaz");
        assert_eq!(e.register(None).unwrap().text, "bar");
    }

    #[test]
    fn cc_yanks_linewise() {
        let mut e = ed("one\ntwo");
        keys(&mut e, "cc<Esc>");
        assert_eq!(
            e.register(None).unwrap(),
            Register {
                text: "one\n".into(),
                linewise: true
            }
        );
    }

    #[test]
    fn dd_last_line_then_p_adds_no_blank() {
        let mut e = ed("a\nb");
        keys(&mut e, "jddkp");
        assert_eq!(e.text(), "a\nb");
    }

    #[test]
    fn nx_register_holds_all() {
        let mut e = ed("abcd");
        keys(&mut e, "3x");
        assert_eq!(e.register(None).unwrap().text, "abc");
    }

    #[test]
    fn x_on_last_char_clamps() {
        let mut e = ed("ab");
        keys(&mut e, "$x");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn x_steps_by_grapheme() {
        let mut e = ed("👩‍👩‍👧b");
        keys(&mut e, "x");
        assert_eq!(e.text(), "b");
    }

    #[test]
    fn yank_writes_clipboard_and_p_reads_it() {
        let (mut e, host) = ed_with_host("abc");
        keys(&mut e, "yw");
        assert_eq!(host.clipboard(), "abc");
        host.set_clipboard("XY");
        keys(&mut e, "P");
        assert_eq!(e.text(), "XYabc");
    }

    #[test]
    fn external_clipboard_with_newline_pastes_linewise() {
        let (mut e, host) = ed_with_host("a\nb");
        host.set_clipboard("z\n");
        keys(&mut e, "p");
        assert_eq!(e.text(), "a\nz\nb");
    }

    // operators_test.dart, at the operator level.

    struct Run {
        text: String,
        regs: Registers,
        host: FakeHost,
        out: OpOutcome,
    }

    fn run(text: &str, cursor: Pos, f: impl FnOnce(&mut EditCtx) -> OpOutcome) -> Run {
        let mut buf = Buffer::from_text(text);
        let mut history = History::new();
        let mut regs = Registers::new();
        let host = FakeHost::default();
        let mut h = host.clone();
        let out = {
            let mut ctx = EditCtx::new(&mut buf, cursor, &mut history, &mut regs, &mut h);
            f(&mut ctx)
        };
        Run {
            text: buf.to_text(),
            regs,
            host,
            out,
        }
    }

    fn range(start: Pos, end: Pos, linewise: bool) -> OpRange {
        OpRange {
            start,
            end,
            linewise,
        }
    }

    fn op_on(text: &str, op: Operator, r: OpRange) -> Run {
        run(text, r.start, |ctx| operate(ctx, op, r))
    }

    #[test]
    fn delete_removes_the_range_and_yanks_it() {
        let r = op_on(
            "hello world",
            Operator::Delete,
            range(pos(0, 0), pos(0, 5), false),
        );
        assert_eq!(r.text, " world");
        assert_eq!(r.regs.unnamed(), &Register::chars("hello"));
        assert_eq!(r.host.clipboard(), "hello");
        assert!(!r.out.insert);
    }

    #[test]
    fn linewise_delete_takes_whole_lines() {
        let r = op_on(
            "line one\nline two\nline three",
            Operator::Delete,
            range(pos(1, 3), pos(1, 5), true),
        );
        assert_eq!(r.text, "line one\nline three");
        assert_eq!(r.regs.unnamed(), &Register::lines("line two\n"));
        assert_eq!(r.out.cursor, pos(1, 0));
    }

    #[test]
    fn linewise_delete_of_the_last_line_registers_the_line() {
        // moon_edit registered "\nline two". The register holds the line.
        let r = op_on(
            "line one\nline two",
            Operator::Delete,
            range(pos(1, 0), pos(1, 8), true),
        );
        assert_eq!(r.text, "line one");
        assert_eq!(r.regs.unnamed(), &Register::lines("line two\n"));
    }

    #[test]
    fn change_deletes_like_delete_and_enters_insert() {
        let r = op_on(
            "hello world",
            Operator::Change,
            range(pos(0, 0), pos(0, 5), false),
        );
        assert_eq!(r.text, " world");
        assert_eq!(r.regs.unnamed().text, "hello");
        assert!(r.out.insert);
        assert_eq!(r.out.cursor, pos(0, 0));
    }

    #[test]
    fn yank_leaves_the_text_alone() {
        let r = op_on(
            "hello world",
            Operator::Yank,
            range(pos(0, 0), pos(0, 5), false),
        );
        assert_eq!(r.text, "hello world");
        assert_eq!(r.regs.unnamed(), &Register::chars("hello"));
        assert!(!r.out.insert);

        let r = op_on(
            "line one\nline two",
            Operator::Yank,
            range(pos(0, 3), pos(0, 5), true),
        );
        assert_eq!(r.text, "line one\nline two");
        assert_eq!(r.regs.unnamed(), &Register::lines("line one\n"));
    }

    #[test]
    fn operator_keys() {
        assert_eq!(Operator::from_key('d'), Some(Operator::Delete));
        assert_eq!(Operator::from_key('c'), Some(Operator::Change));
        assert_eq!(Operator::from_key('y'), Some(Operator::Yank));
        for c in ['z', 'h', 'w', 'j'] {
            assert_eq!(Operator::from_key(c), None, "{c}");
        }
        for op in [Operator::Delete, Operator::Change, Operator::Yank] {
            assert_eq!(Operator::from_key(op.key()), Some(op));
        }
    }

    // actions_test.dart: x, p and P.

    #[test]
    fn x_deletes_the_char_under_the_cursor() {
        let mut e = ed("hello");
        keys(&mut e, "llx");
        assert_eq!(e.text(), "helo");
        assert_eq!(e.register(None).unwrap().text, "l");
    }

    #[test]
    fn x_on_an_empty_line_or_past_the_end_does_nothing() {
        let mut e = ed("");
        keys(&mut e, "x");
        assert_eq!(e.text(), "");
        assert_eq!(e.register(None), None);
        let buf = Buffer::from_text("hi");
        assert_eq!(OpRange::chars(&buf, pos(0, 2), 1), None);
    }

    fn put_with(text: &str, at: Pos, reg: Register, before: bool) -> Run {
        run(text, at, |ctx| {
            ctx.write_register(reg);
            put(ctx, before, 1)
        })
    }

    #[test]
    fn p_puts_after_the_cursor() {
        let r = put_with("hello", pos(0, 2), Register::chars("world"), false);
        assert_eq!(r.text, "helworldlo");
        assert_eq!(r.out.cursor, pos(0, 7));
    }

    #[test]
    fn p_and_big_p_with_nothing_to_put_do_nothing() {
        for before in [false, true] {
            let r = run("hello", pos(0, 2), |ctx| put(ctx, before, 1));
            assert_eq!(r.text, "hello");
            assert_eq!(r.out.cursor, pos(0, 2));
        }
    }

    #[test]
    fn linewise_p_puts_below_the_line() {
        let r = put_with(
            "line one\nline two",
            pos(0, 3),
            Register::lines("new line\n"),
            false,
        );
        assert_eq!(r.text, "line one\nnew line\nline two");
        assert_eq!(r.out.cursor, pos(1, 0));
    }

    #[test]
    fn linewise_p_on_the_last_line_adds_the_break_first() {
        let r = put_with("only line", pos(0, 3), Register::lines("new line\n"), false);
        assert_eq!(r.text, "only line\nnew line");
        assert_eq!(r.out.cursor, pos(1, 0));
    }

    #[test]
    fn big_p_puts_before_the_cursor() {
        let r = put_with("hello", pos(0, 2), Register::chars("world"), true);
        assert_eq!(r.text, "heworldllo");
    }

    #[test]
    fn linewise_big_p_puts_above_the_line() {
        let r = put_with(
            "line one\nline two",
            pos(1, 3),
            Register::lines("new line\n"),
            true,
        );
        assert_eq!(r.text, "line one\nnew line\nline two");
        assert_eq!(r.out.cursor, pos(1, 0));
    }

    // vim_engine_test.dart: operators with motions, simple actions, counts
    // and paste.

    const DOC: &str = "hello world\nline two\nline three";

    #[test]
    fn dw_deletes_a_word_into_the_register() {
        let mut e = ed(DOC);
        keys(&mut e, "dw");
        assert_eq!(e.register(None), Some(Register::chars("hello ")));
        assert_eq!(e.text(), "world\nline two\nline three");
    }

    #[test]
    fn dd_deletes_the_line_linewise() {
        let mut e = ed(DOC);
        keys(&mut e, "dd");
        assert_eq!(e.register(None), Some(Register::lines("hello world\n")));
        assert_eq!(e.text(), "line two\nline three");
    }

    #[test]
    fn yy_yanks_the_line() {
        let mut e = ed(DOC);
        keys(&mut e, "yy");
        assert_eq!(e.register(None), Some(Register::lines("hello world\n")));
        assert_eq!(e.text(), DOC);
    }

    #[test]
    fn x_deletes_into_the_register() {
        let mut e = ed(DOC);
        keys(&mut e, "x");
        assert_eq!(e.register(None).unwrap().text, "h");
        assert_eq!(e.text(), "ello world\nline two\nline three");
    }

    #[test]
    fn counts_with_operators() {
        for (text, seq, want) in [
            ("one two three four", "2dw", "three four"),
            (
                "Line 1\nLine 2\nLine 3\nLine 4\nLine 5",
                "3dd",
                "Line 4\nLine 5",
            ),
            (
                "- Item 1\n- Item 2\n- Item 3\n- Item 4",
                "2dd",
                "- Item 3\n- Item 4",
            ),
            (
                "- Item 1\n  - Nested 1\n  - Nested 2\n- Item 2",
                "2dd",
                "  - Nested 2\n- Item 2",
            ),
            (
                "- Item 1\n- Item 2\n- Item 3\n- Item 4",
                "w2dd",
                "- Item 3\n- Item 4",
            ),
            (
                "- One\n- Two\n- Three\n- Four\n- Five",
                "3dd",
                "- Four\n- Five",
            ),
            (
                "- [ ] Todo 1\n- [x] Done\n- [ ] Todo 2\n- [ ] Todo 3",
                "2dd",
                "- [ ] Todo 2\n- [ ] Todo 3",
            ),
            ("- Item 1\n- Item 2\n- Item 3", "20dd", ""),
            ("- Item 1\n- Item 2\n- Item 3\n- Item 4", "20dd", ""),
            (
                "# Header\n- Item 1\n- Item 2\n- Item 3",
                "j20dd",
                "# Header",
            ),
        ] {
            let mut e = ed(text);
            keys(&mut e, seq);
            assert_eq!(e.text(), want, "{seq} on {text:?}");
        }
    }

    #[test]
    fn two_yy_yanks_two_lines() {
        let mut e = ed("Line 1\nLine 2\nLine 3");
        keys(&mut e, "2yy");
        assert_eq!(e.register(None), Some(Register::lines("Line 1\nLine 2\n")));
    }

    #[test]
    fn counts_with_x_and_puts() {
        for (text, seq, want) in [
            ("Hello World", "3x", "lo World"),
            ("Hello", "yl3p", "HHHHello"),
            ("World", "yl$2P", "WorlWWd"),
        ] {
            let mut e = ed(text);
            keys(&mut e, seq);
            assert_eq!(e.text(), want, "{seq}");
        }
    }

    #[test]
    fn p_and_big_p_put_a_yanked_word() {
        for (seq, want) in [
            ("ywllllllp", "hello whello orld"),
            ("ywllllllP", "hello hello world"),
        ] {
            let mut e = ed(DOC);
            keys(&mut e, seq);
            assert_eq!(e.register(None).unwrap().text, "hello ");
            assert_eq!(e.text().lines().next(), Some(want), "{seq}");
            // The put went into the history (moon_edit: canUndo).
            assert_eq!(e.history().undo_len(), 1);
        }
    }

    // The divergences and rulings this task owns.

    #[test]
    fn e_and_dollar_are_inclusive_under_an_operator() {
        let mut e = ed("abc def");
        keys(&mut e, "de");
        assert_eq!(e.text(), " def");
        let mut e = ed("abc");
        keys(&mut e, "y$");
        assert_eq!(e.register(None).unwrap().text, "abc");
    }

    #[test]
    fn k_g_and_gg_are_linewise_under_an_operator() {
        for (seq, want) in [("jdk", "c"), ("jdG", "a"), ("jdgg", "c"), ("ldj", "c")] {
            let mut e = ed("a\nb\nc");
            keys(&mut e, seq);
            assert_eq!(e.text(), want, "{seq}");
        }
    }

    #[test]
    fn an_operator_over_a_failed_motion_does_nothing() {
        for (text, seq) in [("a\nb", "jdj"), ("a\nb", "dk"), ("ab", "dh"), ("ab", "$de")] {
            let mut e = ed(text);
            keys(&mut e, seq);
            assert_eq!(e.text(), text, "{seq}");
            assert_eq!(e.register(None), None, "{seq}");
            assert_eq!(e.showcmd(), "", "{seq}");
        }
    }

    #[test]
    fn dl_takes_the_last_char_and_nx_stops_at_the_line_end() {
        let mut e = ed("ab");
        keys(&mut e, "$dl");
        assert_eq!(e.text(), "a");
        let mut e = ed("ab\ncd");
        keys(&mut e, "5x");
        assert_eq!(e.text(), "\ncd");
        assert_eq!(e.register(None).unwrap().text, "ab");
    }

    #[test]
    fn x_on_the_last_char_of_a_longer_line_rests_on_the_new_last() {
        let mut e = ed("abc");
        keys(&mut e, "$x");
        assert_eq!(e.text(), "ab");
        assert_eq!(e.cursor(), pos(0, 1));
    }

    #[test]
    fn dw_across_lines_follows_vim() {
        for (text, seq, want, reg) in [
            ("foo\nbar baz", "2dw", "baz", "foo\nbar "),
            ("foo bar\nbaz", "2dw", "\nbaz", "foo bar"),
            ("foo\nbar", "dw", "\nbar", "foo"),
            ("hello", "$dw", "hell", "o"),
        ] {
            let mut e = ed(text);
            keys(&mut e, seq);
            assert_eq!(e.text(), want, "{seq} on {text:?}");
            assert_eq!(e.register(None).unwrap().text, reg, "{seq} on {text:?}");
        }
    }

    #[test]
    fn exclusive_ranges_ending_at_a_line_start_follow_o_v() {
        // dw on an empty line deletes the line.
        let mut e = ed("a\n\nb");
        keys(&mut e, "jdw");
        assert_eq!(e.text(), "a\nb");
        assert_eq!(e.register(None), Some(Register::lines("\n")));
        // db from a line start, starting in the indent: linewise.
        let mut e = ed("ab\ncd");
        keys(&mut e, "jdb");
        assert_eq!(e.text(), "cd");
        // Not in the indent: up to the end of the line before.
        let mut e = ed("x ab\ncd");
        keys(&mut e, "jdb");
        assert_eq!(e.text(), "x \ncd");
        assert_eq!(e.register(None).unwrap().text, "ab");
    }

    #[test]
    fn cw_acts_as_ce() {
        for (text, seq, want) in [
            ("foo bar", "cwX<Esc>", "X bar"),
            ("foo bar", "llcwX<Esc>", "foX bar"),
            ("a b c", "c2wX<Esc>", "X c"),
            ("a  b", "lcwX<Esc>", "aX b"),
            ("a\n\nb", "jcwX<Esc>", "a\nX\nb"),
        ] {
            let mut e = ed(text);
            keys(&mut e, seq);
            assert_eq!(e.text(), want, "{seq} on {text:?}");
        }
    }

    #[test]
    fn c_enters_insert_and_typing_goes_through_the_insert_path() {
        let mut e = ed("one two");
        keys(&mut e, "cw");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.text(), " two");
        keys(&mut e, "1<Esc>");
        assert_eq!(e.text(), "1 two");
        assert_eq!(e.mode(), Mode::Normal);
        // The change and the typing are one undo step (owner decision 3).
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), "one two");
    }

    #[test]
    fn cc_empties_the_lines_and_types() {
        let mut e = ed("a\nb\nc");
        keys(&mut e, "2ccX<Esc>");
        assert_eq!(e.text(), "X\nc");
        assert_eq!(e.register(None), Some(Register::lines("a\nb\n")));
    }

    #[test]
    fn big_c_and_big_d_go_to_the_line_end() {
        let mut e = ed("abc def");
        keys(&mut e, "wD");
        assert_eq!(e.text(), "abc ");
        assert_eq!(e.cursor(), pos(0, 3));
        let mut e = ed("abc def");
        keys(&mut e, "wCX<Esc>");
        assert_eq!(e.text(), "abc X");
        let mut e = ed("ab\ncd\nef");
        keys(&mut e, "2D");
        assert_eq!(e.text(), "\nef");
        let mut e = ed("");
        keys(&mut e, "CX<Esc>");
        assert_eq!(e.text(), "X");
    }

    #[test]
    fn dd_lands_on_the_first_non_blank() {
        let mut e = ed("a\n  b");
        keys(&mut e, "dd");
        assert_eq!(e.cursor(), pos(0, 2));
    }

    #[test]
    fn mismatched_operator_keys_cancel() {
        let mut e = ed("ab\ncd");
        keys(&mut e, "dyj");
        assert_eq!(e.text(), "ab\ncd");
        assert_eq!(e.cursor(), pos(1, 0));
    }

    #[test]
    fn every_delete_and_change_writes_the_clipboard() {
        for (seq, clip) in [
            ("dd", "ab\n"),
            ("x", "a"),
            ("D", "ab"),
            ("cw<Esc>", "ab"),
            ("yy", "ab\n"),
        ] {
            let (mut e, host) = ed_with_host("ab\ncd");
            keys(&mut e, seq);
            assert_eq!(host.clipboard(), clip, "{seq}");
        }
    }

    #[test]
    fn our_own_linewise_text_round_trips_through_the_clipboard() {
        let mut e = ed("a\nb");
        keys(&mut e, "yyp");
        assert_eq!(e.text(), "a\na\nb");
        assert_eq!(e.cursor(), pos(1, 0));
        // Text from elsewhere without a newline puts charwise.
        let (mut e, host) = ed_with_host("ab");
        host.set_clipboard("q");
        keys(&mut e, "p");
        assert_eq!(e.text(), "aqb");
    }

    #[test]
    fn puts_never_split_a_cluster() {
        let mut e = ed("👩‍👩‍👧b");
        keys(&mut e, "ylp");
        assert_eq!(e.text(), "👩‍👩‍👧👩‍👩‍👧b");
        assert_eq!(e.cursor(), pos(0, 18));
    }

    #[test]
    fn multiline_charwise_put_lands_on_its_start() {
        let (mut e, host) = ed_with_host("ab");
        host.set_clipboard("x\ny");
        keys(&mut e, "p");
        assert_eq!(e.text(), "ax\nyb");
        assert_eq!(e.cursor(), pos(0, 1));
    }

    #[cfg(not(fuzzing))]
    #[test]
    fn a_huge_put_count_puts_nothing() {
        let mut e = ed("a");
        keys(&mut e, "yl99999999999p");
        assert_eq!(e.text(), "a");
    }

    #[cfg(fuzzing)]
    #[test]
    fn fuzz_builds_lower_the_put_caps() {
        assert_eq!(MAX_PUT_BYTES, 1 << 20);
        assert_eq!(MAX_PUT_LINES, 1 << 14);
        // The fuzz count ceiling (1,000) is below the line cap, so reach the
        // cap with a multi-line yank: 17 lines x 1,000 = 17,000 > 16,384.
        let text = "a\n".repeat(16);
        let mut e = ed(&format!("{text}a"));
        keys(&mut e, "17yy");
        keys(&mut e, "99999p");
        assert_eq!(e.buffer().line_count(), 17);
        // Under the cap still puts: 17 lines x 900 = 15,300.
        keys(&mut e, "900p");
        assert_eq!(e.buffer().line_count(), 17 + 15_300);
    }

    #[cfg(not(fuzzing))]
    #[test]
    fn a_put_past_the_line_cap_puts_nothing() {
        let mut e = ed("a");
        keys(&mut e, "yy");
        let count = MAX_PUT_LINES + 1;
        keys(&mut e, &format!("{count}p"));
        assert_eq!(e.text(), "a");
        assert_eq!(e.buffer().line_count(), 1);
    }

    #[cfg(not(fuzzing))]
    #[test]
    fn a_put_at_the_line_cap_is_accepted() {
        let mut e = ed("a");
        keys(&mut e, "yy");
        keys(&mut e, &format!("{MAX_PUT_LINES}p"));
        assert_eq!(e.buffer().line_count(), MAX_PUT_LINES + 1);
    }

    #[test]
    fn edits_are_recorded_in_the_history() {
        let mut e = ed("abc");
        keys(&mut e, "lx");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "yy");
        assert_eq!(e.history().undo_len(), 1);
        keys(&mut e, "u");
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), pos(0, 1));
    }
}

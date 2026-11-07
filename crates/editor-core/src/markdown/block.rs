//! Per-line block classification with fenced code and table tracking.

use std::collections::HashMap;
use std::ops::Range;

use crate::buffer::Buffer;

use super::table::{TableId, TableInfo, find_tables, parse_cells};

#[cfg(any(test, feature = "fuzzing"))]
pub(crate) mod oracle;

// How many times `Blocks::update` has returned from a fast path on this
// thread, so a test can tell a fast path from the general one when both
// return the same lines. Per thread, so tests running alongside each other
// do not see each other's updates.
#[cfg(test)]
thread_local! {
    static FAST_PATHS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

// How many times `Blocks::new` has scanned a whole buffer on this thread, so
// a test can tell an edit that followed the blocks from one that rebuilt
// them.
#[cfg(test)]
thread_local! {
    static FULL_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Runs `f` and returns how many times `Blocks::new` scanned a whole buffer
/// meanwhile, on this thread.
#[cfg(test)]
pub(crate) fn full_scans(f: impl FnOnce()) -> usize {
    let before = FULL_SCANS.with(|n| n.get());
    f();
    FULL_SCANS.with(|n| n.get()) - before
}

/// Counts one return from a fast path in `Blocks::update`.
fn took_fast_path() {
    #[cfg(test)]
    FAST_PATHS.with(|n| n.set(n.get() + 1));
}

/// Identifies one fenced code block within a `Blocks` value. Stable between
/// updates that do not touch the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// `- ` or `* `; the char is the marker.
    Bullet(char),
    /// `N. `; the number as written (saturating).
    Ordered(u64),
    /// `- [ ] ` or `- [x] `; true when checked.
    Task(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineKind {
    Blank,
    Paragraph,
    Heading(u8),
    Rule,
    /// `indent` is the nesting level (leading whitespace chars / 2);
    /// `content_offset` is the byte offset where the item text starts.
    ListItem {
        indent: usize,
        kind: ListKind,
        content_offset: usize,
    },
    /// `content_offset` is the byte offset after the `>` markers and spaces.
    Quote {
        depth: u8,
        content_offset: usize,
    },
    FenceOpen {
        block: BlockId,
        lang: Option<String>,
    },
    FenceClose {
        block: BlockId,
    },
    Code {
        block: BlockId,
    },
    TableHeader {
        table: TableId,
    },
    TableSep {
        table: TableId,
    },
    TableRow {
        table: TableId,
    },
    /// A line that is exactly one `![alt](src)`; byte ranges into the line.
    Image {
        alt: Range<usize>,
        src: Range<usize>,
    },
}

/// Where a fenced code block sits. `close` is `None` when the block runs to
/// the end of the document (or to another opening fence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlockInfo {
    pub open: usize,
    pub close: Option<usize>,
    pub lang: Option<String>,
}

/// The block structure of a buffer: one `LineKind` per line, plus the extent
/// of every fenced code block and table.
#[derive(Debug, Clone)]
pub struct Blocks {
    kinds: Vec<LineKind>,
    code_blocks: HashMap<BlockId, CodeBlockInfo>,
    tables: HashMap<TableId, TableInfo>,
    next_block: u32,
    next_table: u32,
}

/// What `kind` returns for a line past the end, mirroring `Buffer::line`.
static OUT_OF_RANGE: LineKind = LineKind::Blank;

/// An old info line number whose line was deleted by an edit (see `shift`).
const GONE: usize = usize::MAX;

/// Fence state between two lines: `None` outside code, or the open block.
type State = Option<BlockId>;

fn state_after(kind: &LineKind) -> State {
    match kind {
        LineKind::FenceOpen { block, .. } | LineKind::Code { block } => Some(*block),
        _ => None,
    }
}

fn block_id(kind: &LineKind) -> Option<BlockId> {
    match kind {
        LineKind::FenceOpen { block, .. }
        | LineKind::FenceClose { block }
        | LineKind::Code { block } => Some(*block),
        _ => None,
    }
}

fn is_code(kind: &LineKind) -> bool {
    matches!(
        kind,
        LineKind::FenceOpen { .. } | LineKind::FenceClose { .. } | LineKind::Code { .. }
    )
}

fn is_table(kind: &LineKind) -> bool {
    matches!(
        kind,
        LineKind::TableHeader { .. } | LineKind::TableSep { .. } | LineKind::TableRow { .. }
    )
}

/// Bookkeeping for one rescan: ids whose lines were rewritten (their infos
/// are dropped or rebuilt), and the previous kind of every surviving line that
/// was rewritten, which yields the range of lines whose kind changed. `repaint`
/// holds the lines of tables that kept their id but whose info changed.
#[derive(Default)]
struct Rescan {
    dead_blocks: Vec<BlockId>,
    dead_tables: Vec<TableId>,
    prev: HashMap<usize, LineKind>,
    repaint: Vec<Range<usize>>,
}

impl Rescan {
    fn bury(&mut self, kind: &LineKind) {
        match kind {
            LineKind::FenceOpen { block, .. }
            | LineKind::FenceClose { block }
            | LineKind::Code { block } => self.dead_blocks.push(*block),
            LineKind::TableHeader { table }
            | LineKind::TableSep { table }
            | LineKind::TableRow { table } => self.dead_tables.push(*table),
            _ => {}
        }
    }
}

impl Blocks {
    pub fn new(buf: &Buffer) -> Blocks {
        #[cfg(test)]
        FULL_SCANS.with(|n| n.set(n.get() + 1));
        Blocks::scan(buf)
    }

    /// `new` without counting the scan (`full_scans`): for the oracle,
    /// whose own scans are not the product's.
    fn scan(buf: &Buffer) -> Blocks {
        let n = buf.line_count();
        let mut blocks = Blocks {
            kinds: vec![LineKind::Blank; n],
            code_blocks: HashMap::new(),
            tables: HashMap::new(),
            next_block: 0,
            next_table: 0,
        };
        let mut r = Rescan::default();
        blocks.scan_fences(buf, 0, n, None, &mut r);
        blocks.scan_tables(buf, 0, n, &mut r);
        blocks
    }

    /// Brings the classification up to date after an edit and returns the
    /// lines (in `buf`) to repaint: every line whose kind changed, plus every
    /// line of a block or table whose info changed while it kept its id. A
    /// table is covered from header to last row when its extent or alignment
    /// changed; a code block from its opener to the later of its old and new
    /// last lines (the closer, or for an unclosed block the line before the
    /// next opener or the end of the document) when its open, close, lang or
    /// last line changed. The returned range always covers `edited`.
    ///
    /// The contract: `buf` is the buffer after the edit. `edited` is the range
    /// of lines in `buf` whose text changed (it may be empty for a pure
    /// deletion). `delta` is the change in line count, so the lines that were
    /// replaced in the old buffer are `edited.start..edited.end - delta`, and
    /// every line at or after `edited.end` is old line `i - delta`.
    ///
    /// The fence scan restarts at `edited.start` and runs until the fence
    /// state agrees with the old one again, so opening or closing a fence
    /// re-scans to the next fence or the end. Tables are re-scanned over the
    /// whole run of pipe rows touching the re-scanned lines. Ids of blocks and
    /// tables outside those lines are kept.
    ///
    /// Arguments that do not fit the old line count fall back to a full
    /// rebuild, returning every line.
    pub fn update(&mut self, buf: &Buffer, edited: Range<usize>, delta: isize) -> Range<usize> {
        let n = buf.line_count();
        let old_n = self.kinds.len();
        let (s, e) = (edited.start, edited.end);
        let old_e = e as isize - delta;
        let fits = s <= e
            && e <= n
            && old_n as isize + delta == n as isize
            && old_e >= s as isize
            && old_e <= old_n as isize;
        if !fits {
            *self = Blocks::new(buf);
            return 0..n;
        }
        let old_e = old_e as usize;

        // In-row fast path: a one-line edit to a body row that keeps its cell
        // count changes nothing else. `find_tables` extends a table while the
        // cell count matches the header's, and `TableInfo` holds only the
        // extent and alignments, so the table's extent, alignments, ids and
        // every other line's kind stay as they are. The line cannot open a
        // fence (it must start with `|` once trimmed, fences start with
        // backticks) and it follows a table line, so no fence state carries in.
        if delta == 0
            && e == s + 1
            && let LineKind::TableRow { table } = &self.kinds[s]
            && parse_cells(buf.line(s)).map(|c| c.len()) == Some(self.tables[table].aligns.len())
        {
            took_fast_path();
            return s..s + 1;
        }

        // Out-of-table fast path: a one-line edit to a line in no table and
        // no code block that leaves it unable to join either changes only
        // its own kind. Tables are built from runs of pipe rows
        // (`parse_cells` finds cells), and a separator needs cells too, so
        // a line without them is no header, separator or row, and
        // `find_tables` treats it as it treated the old line, which joined
        // no table either. Not being code, the line had no fence state
        // carried into it, and not opening a fence, it carries none out.
        if delta == 0
            && e == s + 1
            && !is_code(&self.kinds[s])
            && !is_table(&self.kinds[s])
            && parse_cells(buf.line(s)).is_none()
            && opening_fence(buf.line(s), false).is_none()
        {
            self.kinds[s] = classify_line(buf.line(s));
            took_fast_path();
            return s..s + 1;
        }

        let mut r = Rescan::default();
        let removed: Vec<LineKind> = self
            .kinds
            .splice(s..old_e, std::iter::repeat_n(LineKind::Blank, e - s))
            .collect();
        for kind in &removed {
            r.bury(kind);
        }
        // The last replaced line in the block carried in from above, mapped
        // like `shift` maps info lines: same offset while inside the edit.
        let carried = s.checked_sub(1).and_then(|p| state_after(&self.kinds[p]));
        let removed_last = carried
            .and_then(|id| removed.iter().rposition(|k| block_id(k) == Some(id)))
            .map(|j| if s + j < e { s + j } else { GONE });
        self.shift(old_e, e, delta);
        let f = self.scan_fences(buf, s, e, removed_last, &mut r);
        self.scan_tables(buf, s, f, &mut r);

        let (mut lo, mut hi) = (s, e);
        for (&i, old) in &r.prev {
            if self.kinds[i] != *old {
                lo = lo.min(i);
                hi = hi.max(i + 1);
            }
        }
        for lines in &r.repaint {
            lo = lo.min(lines.start);
            hi = hi.max(lines.end);
        }
        lo..hi
    }

    /// The kind of `line`, or `Blank` past the end.
    pub fn kind(&self, line: usize) -> &LineKind {
        self.kinds.get(line).unwrap_or(&OUT_OF_RANGE)
    }

    /// The number of lines classified (the buffer's line count).
    pub fn line_count(&self) -> usize {
        self.kinds.len()
    }

    /// The extent of a code block. `id` must come from this value's current
    /// kinds; a stale id panics.
    pub fn code_block(&self, id: BlockId) -> &CodeBlockInfo {
        &self.code_blocks[&id]
    }

    /// The extent of a table. `id` must come from this value's current kinds;
    /// a stale id panics.
    pub fn table(&self, id: TableId) -> &TableInfo {
        &self.tables[&id]
    }

    /// Maps info line numbers into the new buffer: lines at or after old line
    /// `from` (the old end of the edit) move by `delta`; lines inside the edit
    /// keep their offset while it is still inside `..to` (the new end of the
    /// edit), and otherwise become `GONE`, so they never compare equal to a new
    /// line. Only infos about to be rebuilt or compared hold such values.
    fn shift(&mut self, from: usize, to: usize, delta: isize) {
        if delta == 0 {
            return;
        }
        let mv = |x: &mut usize| {
            if *x >= from {
                *x = (*x as isize + delta) as usize;
            } else if *x >= to {
                *x = GONE;
            }
        };
        for info in self.code_blocks.values_mut() {
            mv(&mut info.open);
            if let Some(c) = info.close.as_mut() {
                mv(c);
            }
        }
        for info in self.tables.values_mut() {
            mv(&mut info.header);
            mv(&mut info.sep);
            mv(&mut info.last);
        }
    }

    /// Classifies lines from `s` on, tracking fences. Lines before `e` are new
    /// (their kinds are placeholders); from `e` on the stored kinds are the old
    /// ones, and the scan stops at the first line where the fence state before
    /// it matches the old state. Returns that line (or the line count).
    fn scan_fences(
        &mut self,
        buf: &Buffer,
        s: usize,
        e: usize,
        removed_last: Option<usize>,
        r: &mut Rescan,
    ) -> usize {
        let n = self.kinds.len();
        let carried: State = s.checked_sub(1).and_then(|p| state_after(&self.kinds[p]));
        let carried_close = carried
            .and_then(|id| self.code_blocks.get_mut(&id))
            .map(|info| info.close.take());
        let mut state = carried;
        let mut old_state: Option<State> = None;
        let mut agreed = false;
        // The last line known to carry the carried block's id (its opener or
        // code above `s` when the scan finds none).
        let mut carried_last = s.saturating_sub(1);
        // The same for the old kinds, in new line numbers: from the replaced
        // lines (`removed_last`, see `update`), then from old lines the scan
        // rewrites.
        let mut old_last = removed_last.unwrap_or(carried_last);
        let mut i = s;
        while i < n {
            if old_state == Some(state) {
                agreed = true;
                break;
            }
            let text = buf.line(i);
            let open = opening_fence(text, state.is_some());
            // An opener resets the state, so an old line that opened the same
            // way means everything from here on is unchanged, ids included.
            if i >= e
                && let (Some(lang), LineKind::FenceOpen { lang: old, .. }) = (&open, &self.kinds[i])
                && lang == old
            {
                break;
            }
            let kind = if let Some(lang) = open {
                let id = BlockId(self.next_block);
                self.next_block = self.next_block.wrapping_add(1);
                self.code_blocks.insert(
                    id,
                    CodeBlockInfo {
                        open: i,
                        close: None,
                        lang: lang.clone(),
                    },
                );
                LineKind::FenceOpen { block: id, lang }
            } else if let Some(id) = state {
                if text.trim_end() == "```" {
                    if let Some(info) = self.code_blocks.get_mut(&id) {
                        info.close = Some(i);
                    }
                    LineKind::FenceClose { block: id }
                } else {
                    LineKind::Code { block: id }
                }
            } else {
                classify_line(text)
            };
            state = state_after(&kind);
            if carried.is_some() && block_id(&kind) == carried {
                carried_last = i;
            }
            if i >= e {
                let old = std::mem::replace(&mut self.kinds[i], kind);
                if carried.is_some() && block_id(&old) == carried {
                    old_last = i;
                }
                old_state = Some(state_after(&old));
                r.bury(&old);
                r.prev.insert(i, old);
            } else {
                self.kinds[i] = kind;
            }
            i += 1;
        }

        // Stopped because the states agree while still inside the carried
        // block: the rest of it is unchanged, so its old close stands. Stopping
        // at a same opener instead ends the carried block there, unclosed.
        if agreed
            && let (Some(id), Some(close)) = (carried, carried_close)
            && state == Some(id)
            && let Some(info) = self.code_blocks.get_mut(&id)
        {
            info.close = close;
        }
        // The carried block is the only one that keeps its id through a
        // rescan (its open and lang lie above `s`), so only its close and its
        // last line can change under a kept id. An unclosed block ends at the
        // next opener, so its last line can move while `close` stays `None`.
        // When either changes, repaint opener to the later of the old and new
        // last lines. If the scan stopped by agreement inside the block, both
        // are unchanged (its close was restored above), so nothing is added.
        if let (Some(id), Some(old_close)) = (carried, carried_close)
            && let Some(info) = self.code_blocks.get(&id)
            && (info.close != old_close || old_last != carried_last)
        {
            let end = if old_last == GONE {
                carried_last
            } else {
                old_last.max(carried_last)
            };
            r.repaint.push(info.open..end + 1);
        }
        for id in r.dead_blocks.drain(..) {
            if Some(id) != carried {
                self.code_blocks.remove(&id);
            }
        }
        i
    }

    /// Rebuilds the tables over every run of pipe rows touching `s..f`.
    fn scan_tables(&mut self, buf: &Buffer, s: usize, f: usize, r: &mut Rescan) {
        let n = self.kinds.len();
        let candidate = |kinds: &[LineKind], i: usize| {
            !is_code(&kinds[i]) && parse_cells(buf.line(i)).is_some()
        };
        let mut ts = s;
        while ts > 0 && candidate(&self.kinds, ts - 1) {
            ts -= 1;
        }
        let mut te = f.max(s);
        while te < n && candidate(&self.kinds, te) {
            te += 1;
        }
        // Lines the fence scan did not rewrite still hold old table kinds.
        for i in (ts..s).chain(f.max(s)..te) {
            let old = std::mem::replace(&mut self.kinds[i], classify_line(buf.line(i)));
            r.bury(&old);
            r.prev.insert(i, old);
        }
        // Old infos (already shifted) of the tables being rebuilt, so a kept id
        // whose extent or alignment changed can be repainted whole.
        let mut old_infos = HashMap::new();
        for id in r.dead_tables.drain(..) {
            if let Some(info) = self.tables.remove(&id) {
                old_infos.insert(id, info);
            }
        }

        let kinds = &self.kinds;
        let found = find_tables(ts..te, |i| (!is_code(&kinds[i])).then(|| buf.line(i)));
        for t in found {
            let reuse = match r.prev.get(&t.header) {
                Some(LineKind::TableHeader { table }) if !self.tables.contains_key(table) => {
                    Some(*table)
                }
                _ => None,
            };
            let table = reuse.unwrap_or_else(|| {
                let id = TableId(self.next_table);
                self.next_table = self.next_table.wrapping_add(1);
                id
            });
            self.kinds[t.header] = LineKind::TableHeader { table };
            self.kinds[t.sep] = LineKind::TableSep { table };
            for row in t.sep + 1..=t.last {
                self.kinds[row] = LineKind::TableRow { table };
            }
            let info = TableInfo {
                header: t.header,
                sep: t.sep,
                last: t.last,
                aligns: t.aligns,
            };
            if reuse.is_some() && old_infos.get(&table) != Some(&info) {
                r.repaint.push(info.header..info.last + 1);
            }
            self.tables.insert(table, info);
        }
    }
}

/// `Some(lang)` when `text` opens a fence: three backticks at column 0. Inside
/// a block, three backticks with only trailing whitespace closes instead, so
/// only three backticks plus a language opens a new block there. (moon_edit
/// opens a new block on trailing whitespace; whitespace is not a language.)
fn opening_fence(text: &str, inside: bool) -> Option<Option<String>> {
    if !text.starts_with("```") || (inside && text.trim_end() == "```") {
        return None;
    }
    let lang = text[3..].trim();
    Some((!lang.is_empty()).then(|| lang.to_owned()))
}

/// The kind of a line outside code, before tables are applied.
fn classify_line(text: &str) -> LineKind {
    if text.trim().is_empty() {
        return LineKind::Blank;
    }
    if let Some(level) = heading_level(text) {
        return LineKind::Heading(level);
    }
    if text.starts_with('>') {
        return quote(text);
    }
    // Before lists, so `* * *` and `- - -` are rules.
    if is_rule(text) {
        return LineKind::Rule;
    }
    if let Some(item) = list_item(text) {
        return item;
    }
    image(text).unwrap_or(LineKind::Paragraph)
}

/// `^(#{1,6})\s+`
fn heading_level(text: &str) -> Option<u8> {
    let hashes = text.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let next = text[hashes..].chars().next()?;
    next.is_whitespace().then_some(hashes as u8)
}

/// Depth counts `>` with spaces allowed between them; the content starts
/// after the last `>` and the spaces that follow it.
fn quote(text: &str) -> LineKind {
    let mut depth: u8 = 0;
    let mut offset = 0;
    for (i, b) in text.bytes().enumerate() {
        match b {
            b'>' => {
                depth = depth.saturating_add(1);
                offset = i + 1;
            }
            b' ' if offset == i => offset = i + 1,
            b' ' => {}
            _ => {
                return LineKind::Quote {
                    depth,
                    content_offset: offset,
                };
            }
        }
    }
    LineKind::Quote {
        depth,
        content_offset: text.len(),
    }
}

/// `^(\*[ ]*\*[ ]*\*[\* ]*|-[ ]*-[ ]*-[- ]*|_[ ]*_[ ]*_[_ ]*)$`: the first byte
/// is the marker, every byte is the marker or a space, at least three markers.
fn is_rule(text: &str) -> bool {
    let Some(marker) = text.bytes().next() else {
        return false;
    };
    matches!(marker, b'*' | b'-' | b'_')
        && text.bytes().all(|b| b == marker || b == b' ')
        && text.bytes().filter(|&b| b == marker).count() >= 3
}

/// After leading whitespace: `- [ ] `/`- [x] ` (task), `- ` or `* ` (bullet),
/// or `^\d+\.\s+` (ordered). Nesting is leading whitespace chars / 2.
fn list_item(text: &str) -> Option<LineKind> {
    let body = text.trim_start();
    let lead = text.len() - body.len();
    let indent = text[..lead].chars().count() / 2;
    let item = |kind, prefix: usize| LineKind::ListItem {
        indent,
        kind,
        content_offset: lead + prefix,
    };
    let b = body.as_bytes();
    if b.len() >= 6
        && b.starts_with(b"- [")
        && matches!(b[3], b'x' | b'X' | b' ')
        && b[4] == b']'
        && b[5] == b' '
    {
        return Some(item(ListKind::Task(b[3] != b' '), 6));
    }
    if body.starts_with("- ") || body.starts_with("* ") {
        return Some(item(ListKind::Bullet(char::from(b[0])), 2));
    }
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || b.get(digits) != Some(&b'.') {
        return None;
    }
    let rest = &body[digits + 1..];
    let gap = rest.len() - rest.trim_start().len();
    if gap == 0 {
        return None;
    }
    let number = body[..digits].parse().unwrap_or(u64::MAX);
    Some(item(ListKind::Ordered(number), digits + 1 + gap))
}

/// A line that is exactly `![alt](src)`: the first `]` after `![` must be
/// followed by `(`, and the first `)` after that must end the line.
fn image(text: &str) -> Option<LineKind> {
    let close = 2 + text.strip_prefix("![")?.find(']')?;
    if text.as_bytes().get(close + 1) != Some(&b'(') {
        return None;
    }
    let paren = close + 2 + text[close + 2..].find(')')?;
    (paren + 1 == text.len()).then(|| LineKind::Image {
        alt: 2..close,
        src: close + 2..paren,
    })
}

/// The marker shown for an ordered list item: its position among the ordered
/// items at the same nesting level since the nearest shallower item, as `N.`
/// (level 0), `a.` (level 1) or `i.` (level 2), repeating every three levels.
/// Deeper items and unordered siblings are skipped; any line that is not a
/// list item ends the list. Empty for lines that are not ordered items.
pub fn ordered_display(blocks: &Blocks, buf: &Buffer, line: usize) -> String {
    if line >= buf.line_count() {
        return String::new();
    }
    let LineKind::ListItem {
        indent,
        kind: ListKind::Ordered(_),
        ..
    } = *blocks.kind(line)
    else {
        return String::new();
    };
    let mut count: u64 = 1;
    for i in (0..line).rev() {
        let LineKind::ListItem {
            indent: level,
            kind,
            ..
        } = blocks.kind(i)
        else {
            break;
        };
        if *level < indent {
            break;
        }
        if *level == indent && matches!(kind, ListKind::Ordered(_)) {
            count += 1;
        }
    }
    let marker = match indent % 3 {
        0 => count.to_string(),
        1 => alpha(count),
        _ => roman(count),
    };
    format!("{marker}.")
}

/// 1 is a, 26 is z, 27 is aa.
fn alpha(mut n: u64) -> String {
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(char::from(b'a' + (n % 26) as u8));
        n /= 26;
    }
    out.iter().rev().collect()
}

/// Lower-case roman numerals.
fn roman(mut n: u64) -> String {
    const NUMERALS: [(u64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::oracle::{assert_fresh, block_of, check_update, snapshot, table_of};
    use super::*;
    use crate::buffer::Pos;
    use crate::markdown::table::Align;

    fn blocks(text: &str) -> (Buffer, Blocks) {
        let b = Buffer::from_text(text);
        let bl = Blocks::new(&b);
        (b, bl)
    }

    fn kinds(text: &str) -> Vec<LineKind> {
        let (b, bl) = blocks(text);
        (0..b.line_count()).map(|i| bl.kind(i).clone()).collect()
    }

    fn k(line: &str) -> LineKind {
        kinds(line).remove(0)
    }

    /// (indent, kind, content_offset) of a list line; panics otherwise.
    fn list(line: &str) -> (usize, ListKind, usize) {
        match k(line) {
            LineKind::ListItem {
                indent,
                kind,
                content_offset,
            } => (indent, kind, content_offset),
            other => panic!("{line:?} is {other:?}, not a list item"),
        }
    }

    /// (depth, content_offset) of a quote line; panics otherwise.
    fn quote(line: &str) -> (u8, usize) {
        match k(line) {
            LineKind::Quote {
                depth,
                content_offset,
            } => (depth, content_offset),
            other => panic!("{line:?} is {other:?}, not a quote"),
        }
    }

    /// Applies a buffer replace, then `update` with the contract's arguments.
    fn edit(buf: &mut Buffer, bl: &mut Blocks, r: Range<Pos>, text: &str) -> Range<usize> {
        let before = buf.line_count();
        let e = buf.replace(r, text);
        let delta = buf.line_count() as isize - before as isize;
        bl.update(buf, e.range.start.line..e.range.end.line + 1, delta)
    }

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    /// Replaces the whole of line `l` with `text`.
    fn set_line(buf: &mut Buffer, bl: &mut Blocks, l: usize, text: &str) -> Range<usize> {
        let len = buf.line(l).len();
        edit(buf, bl, p(l, 0)..p(l, len), text)
    }

    // ---- document_line_test.dart: listType ----

    #[test]
    fn detects_unordered_list_with_dash() {
        assert_eq!(list("- item").1, ListKind::Bullet('-'));
    }

    #[test]
    fn detects_unordered_list_with_asterisk() {
        assert_eq!(list("* item").1, ListKind::Bullet('*'));
    }

    #[test]
    fn detects_ordered_list() {
        assert_eq!(list("1. item").1, ListKind::Ordered(1));
    }

    #[test]
    fn detects_ordered_list_with_multi_digit_number() {
        assert_eq!(list("42. item").1, ListKind::Ordered(42));
    }

    #[test]
    fn detects_task_lists() {
        assert_eq!(list("- [ ] todo").1, ListKind::Task(false));
        assert_eq!(list("- [x] done").1, ListKind::Task(true));
        assert_eq!(list("- [X] done").1, ListKind::Task(true));
    }

    #[test]
    fn non_list_lines_are_not_list_items() {
        assert_eq!(k("paragraph"), LineKind::Paragraph);
        assert_eq!(k("# Header"), LineKind::Heading(1));
        assert_eq!(k("1."), LineKind::Paragraph);
        assert_eq!(k("-"), LineKind::Paragraph);
    }

    // ---- nestingLevel ----

    #[test]
    fn nesting_is_leading_whitespace_over_two() {
        assert_eq!(list("- item").0, 0);
        assert_eq!(list("  - item").0, 1);
        assert_eq!(list("    - item").0, 2);
        assert_eq!(list("      - item").0, 3);
        assert_eq!(list(" - item").0, 0);
    }

    // ---- listContentOffset (and getListPrefix, which is the text before it) ----

    #[test]
    fn list_content_offsets() {
        assert_eq!(list("- item").2, 2);
        assert_eq!(list("1. item").2, 3);
        assert_eq!(list("- [ ] item").2, 6);
        assert_eq!(list("  - item").2, 4);
        assert_eq!(list("12.   item").2, 6);
    }

    #[test]
    fn list_prefix_is_text_before_content_offset() {
        for (line, prefix) in [
            ("- item", "- "),
            ("1. item", "1. "),
            ("- [ ] item", "- [ ] "),
            ("  - item", "  - "),
        ] {
            assert_eq!(&line[..list(line).2], prefix);
        }
    }

    #[test]
    fn star_task_syntax_is_a_plain_bullet() {
        assert_eq!(list("* [ ] x"), (0, ListKind::Bullet('*'), 2));
    }

    // ---- blockquote blockType, depth, content offset, content ----

    #[test]
    fn detects_blockquotes() {
        for line in [
            "> text",
            ">> text",
            ">>> text",
            "> > text",
            ">",
            ">>no space",
        ] {
            quote(line);
        }
    }

    #[test]
    fn blockquote_depths() {
        assert_eq!(quote("> quoted").0, 1);
        assert_eq!(quote(">> nested").0, 2);
        assert_eq!(quote(">>> deeply nested").0, 3);
        assert_eq!(quote("> > spaced").0, 2);
        assert_eq!(quote("> >> mixed").0, 3);
        assert_eq!(quote(">").0, 1);
        assert_eq!(quote(">>").0, 2);
    }

    #[test]
    fn blockquote_content_offsets() {
        assert_eq!(quote("> text").1, 2);
        assert_eq!(quote(">> text").1, 3);
        assert_eq!(quote(">>> text").1, 4);
        assert_eq!(quote("> > text").1, 4);
        assert_eq!(quote(">").1, 1);
        assert_eq!(quote(">>").1, 2);
    }

    #[test]
    fn blockquote_content_is_text_after_offset() {
        for (line, content) in [
            ("> text", "text"),
            (">> nested", "nested"),
            ("> > spaced", "spaced"),
            (">", ""),
        ] {
            assert_eq!(&line[quote(line).1..], content);
        }
    }

    // ---- table properties: a lone row is not a table ----

    #[test]
    fn lone_table_row_is_paragraph() {
        assert_eq!(k("| a | b |"), LineKind::Paragraph);
        assert_eq!(k("|---|---|"), LineKind::Paragraph);
    }

    // ---- horizontal rule detection ----

    #[test]
    fn detects_rules() {
        for line in ["***", "---", "___", "****", "* * *", "- - -", "***  "] {
            assert_eq!(k(line), LineKind::Rule, "{line:?}");
        }
    }

    #[test]
    fn dashes_with_text_are_not_a_rule() {
        assert_eq!(k("--- text"), LineKind::Paragraph);
        assert_eq!(k(" ***"), LineKind::Paragraph);
    }

    #[test]
    fn two_dashes_is_not_rule() {
        assert_eq!(k("--"), LineKind::Paragraph);
        assert_eq!(k("**"), LineKind::Paragraph);
    }

    #[test]
    fn hr_beats_list_for_star_star_star() {
        assert_eq!(k("* * *"), LineKind::Rule);
        assert_eq!(k("- - -"), LineKind::Rule);
        assert_eq!(list("* item").1, ListKind::Bullet('*'));
    }

    // ---- image block type (brief wins: an image-only line is Image) ----

    #[test]
    fn image_only_line_is_image() {
        let line = "![alt text](images/file.png)";
        let LineKind::Image { alt, src } = k(line) else {
            panic!("not an image");
        };
        assert_eq!(&line[alt], "alt text");
        assert_eq!(&line[src], "images/file.png");
        assert!(matches!(
            k("![photo](images/photo.png)"),
            LineKind::Image { .. }
        ));
    }

    #[test]
    fn line_with_text_around_image_is_paragraph() {
        assert_eq!(k("See ![photo](images/photo.png)"), LineKind::Paragraph);
        assert_eq!(k("![a](b) after"), LineKind::Paragraph);
        assert_eq!(k("![a](b)![c](d)"), LineKind::Paragraph);
    }

    // ---- document_controller_test.dart: block types and headings ----

    #[test]
    fn detects_block_types() {
        assert_eq!(
            kinds("# Header\nParagraph\n- List")[..2],
            [LineKind::Heading(1), LineKind::Paragraph]
        );
        assert!(matches!(
            kinds("# Header\nParagraph\n- List")[2],
            LineKind::ListItem { .. }
        ));
    }

    #[test]
    fn heading_levels() {
        assert_eq!(
            kinds("# H1\n## H2\nParagraph\n###### H6\n####### H7\n#nospace\n#"),
            [
                LineKind::Heading(1),
                LineKind::Heading(2),
                LineKind::Paragraph,
                LineKind::Heading(6),
                LineKind::Paragraph,
                LineKind::Paragraph,
                LineKind::Paragraph,
            ]
        );
    }

    #[test]
    fn whitespace_only_lines_are_blank() {
        assert_eq!(
            kinds("a\n\n   \nb")[1..3],
            [LineKind::Blank, LineKind::Blank]
        );
    }

    // ---- getOrderedListDisplayNumber ----

    fn displays(text: &str) -> Vec<String> {
        let (b, bl) = blocks(text);
        (0..b.line_count())
            .map(|i| ordered_display(&bl, &b, i))
            .collect()
    }

    #[test]
    fn sequential_numbers_for_top_level_items() {
        assert_eq!(
            displays("1. First\n1. Second\n1. Third"),
            ["1.", "2.", "3."]
        );
    }

    #[test]
    fn nested_level_resets_when_parent_appears() {
        assert_eq!(
            displays("1. First\n  1. Nested A\n  1. Nested B\n1. Second\n  1. Nested C"),
            ["1.", "a.", "b.", "2.", "a."]
        );
    }

    #[test]
    fn three_levels_of_nesting() {
        assert_eq!(
            displays("1. L0\n  1. L1\n    1. L2a\n    1. L2b\n  1. L1b\n    1. L2c"),
            ["1.", "a.", "i.", "ii.", "b.", "i."]
        );
    }

    #[test]
    fn non_ordered_lines_have_no_display() {
        assert_eq!(
            displays("- unordered\nparagraph\n1. ordered"),
            ["", "", "1."]
        );
    }

    #[test]
    fn non_list_content_restarts_numbering() {
        assert_eq!(displays("1. First\nparagraph\n1. Second"), ["1.", "", "1."]);
    }

    #[test]
    fn ordered_numbering_by_level() {
        assert_eq!(
            displays(
                "1. a\n  1. b\n    1. c\n    1. c\n    1. c\n    1. c\n      1. d\n  - skip\n  1. e"
            ),
            ["1.", "a.", "i.", "ii.", "iii.", "iv.", "1.", "", "b."]
        );
        // Raw numbers are ignored; unordered siblings neither count nor break.
        assert_eq!(displays("7. a\n- b\n3. c"), ["1.", "", "2."]);
        assert_eq!(alpha(26), "z");
        assert_eq!(alpha(27), "aa");
        assert_eq!(roman(1994), "mcmxciv");
    }

    // ---- fenced code block tracking ----

    #[test]
    fn marks_opening_and_closing_fence() {
        let (_, bl) = blocks("```dart\ncode\n```");
        let LineKind::FenceOpen { block, lang } = bl.kind(0) else {
            panic!("{:?}", bl.kind(0));
        };
        assert_eq!(lang.as_deref(), Some("dart"));
        assert_eq!(bl.kind(1), &LineKind::Code { block: *block });
        assert_eq!(bl.kind(2), &LineKind::FenceClose { block: *block });
        assert_eq!(
            bl.code_block(*block),
            &CodeBlockInfo {
                open: 0,
                close: Some(2),
                lang: Some("dart".into())
            }
        );
    }

    #[test]
    fn content_lines_share_block_id() {
        let (_, bl) = blocks("```\nline1\nline2\n```");
        let id = block_of(bl.kind(0)).unwrap();
        for i in 1..4 {
            assert_eq!(block_of(bl.kind(i)), Some(id));
        }
        assert!(matches!(bl.kind(1), LineKind::Code { .. }));
    }

    #[test]
    fn detects_language_and_empty_language() {
        assert!(matches!(
            &k("```javascript"),
            LineKind::FenceOpen { lang: Some(l), .. } if l == "javascript"
        ));
        assert!(matches!(k("```"), LineKind::FenceOpen { lang: None, .. }));
        assert!(matches!(
            k("```   "),
            LineKind::FenceOpen { lang: None, .. }
        ));
    }

    #[test]
    fn unclosed_fence_runs_to_end() {
        let ks = kinds("```\ncode\n# more code\n| a |\n|---|");
        let id = block_of(&ks[0]).unwrap();
        for k in &ks[1..] {
            assert_eq!(k, &LineKind::Code { block: id });
        }
        let (_, bl) = blocks("```\ncode\nmore code");
        assert_eq!(bl.code_block(id).close, None);
    }

    #[test]
    fn fence_with_language_inside_block_starts_new_block() {
        let (_, bl) = blocks("```\na\n```rust\nb\n```\nafter");
        let first = block_of(bl.kind(0)).unwrap();
        let second = block_of(bl.kind(2)).unwrap();
        assert_ne!(first, second);
        assert!(matches!(bl.kind(2), LineKind::FenceOpen { .. }));
        assert_eq!(bl.code_block(first).close, None);
        assert_eq!(bl.code_block(second).close, Some(4));
        assert_eq!(bl.kind(5), &LineKind::Paragraph);
    }

    #[test]
    fn fence_close_ignores_trailing_whitespace() {
        // Diverges from moon_edit, which opens a new block on "```  " inside
        // one: whitespace is not a language, so the line closes the block.
        let ks = kinds("```\ncode\n```  \nafter\n# h");
        let id = block_of(&ks[0]).unwrap();
        assert_eq!(ks[2], LineKind::FenceClose { block: id });
        assert_eq!(ks[3], LineKind::Paragraph);
        assert_eq!(ks[4], LineKind::Heading(1));
        assert!(matches!(
            kinds("```\na\n```\t")[2],
            LineKind::FenceClose { .. }
        ));
        // A real language still opens a new block inside one.
        assert!(matches!(
            kinds("```\na\n``` rust ")[2],
            LineKind::FenceOpen { .. }
        ));

        let (mut b, mut bl) = blocks("```\ncode\nx\nafter");
        let changed = set_line(&mut b, &mut bl, 2, "``` ");
        assert_eq!(bl.kind(2), &LineKind::FenceClose { block: id });
        assert_eq!(bl.code_block(id).close, Some(2));
        assert_eq!(bl.kind(3), &LineKind::Paragraph);
        assert_eq!(changed, 0..4);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn fence_must_start_at_column_zero() {
        assert_eq!(k(" ```"), LineKind::Paragraph);
    }

    // ---- code_block_auto_close_test.dart (detection only) ----

    #[test]
    fn auto_closed_fence_detection() {
        let ks = kinds("```bash\n\n```");
        assert!(matches!(ks[0], LineKind::FenceOpen { .. }));
        assert!(matches!(ks[1], LineKind::Code { .. }));
        assert!(matches!(ks[2], LineKind::FenceClose { .. }));

        let ks = kinds("```bash\necho hello\n\n```");
        assert!(matches!(ks[2], LineKind::Code { .. }));
        assert!(matches!(ks[3], LineKind::FenceClose { .. }));

        let ks = kinds("```bash\n\n```\n- item 1\n- item 2");
        assert!(matches!(ks[3], LineKind::ListItem { .. }));
        assert!(matches!(ks[4], LineKind::ListItem { .. }));
    }

    #[test]
    fn inserting_closing_fence_above_list_releases_it() {
        let (mut b, mut bl) = blocks("```bash\n- item 1\n- item 2");
        assert!(matches!(bl.kind(1), LineKind::Code { .. }));
        let changed = edit(&mut b, &mut bl, p(0, 7)..p(0, 7), "\n\n```");
        assert_eq!(b.to_text(), "```bash\n\n```\n- item 1\n- item 2");
        assert!(matches!(bl.kind(3), LineKind::ListItem { .. }));
        assert!(matches!(bl.kind(4), LineKind::ListItem { .. }));
        assert_eq!(changed, 0..5);
        assert_fresh(&b, &bl);
    }

    // ---- table state tracking ----

    #[test]
    fn assigns_table_to_header_separator_and_rows() {
        let (_, bl) = blocks("| A | B |\n|---|---|\n| 1 | 2 |");
        let LineKind::TableHeader { table } = *bl.kind(0) else {
            panic!("{:?}", bl.kind(0));
        };
        assert_eq!(bl.kind(1), &LineKind::TableSep { table });
        assert_eq!(bl.kind(2), &LineKind::TableRow { table });
        assert_eq!(
            bl.table(table),
            &TableInfo {
                header: 0,
                sep: 1,
                last: 2,
                aligns: vec![Align::None, Align::None]
            }
        );
    }

    #[test]
    fn three_column_table() {
        let (_, bl) = blocks("| A | B | C |\n|---|---|---|\n| 1 | 2 | 3 |");
        let t = table_of(bl.kind(0)).unwrap();
        assert_eq!(bl.table(t).aligns.len(), 3);
    }

    #[test]
    fn alignments_from_separator_row() {
        let (_, bl) = blocks("| L | C | R |\n|:---|:---:|---:|\n| a | b | c |");
        let t = table_of(bl.kind(2)).unwrap();
        assert_eq!(
            bl.table(t).aligns,
            [Align::Left, Align::Centre, Align::Right]
        );
    }

    #[test]
    fn header_only_table_is_valid() {
        let (_, bl) = blocks("| A | B |\n|---|---|");
        let t = table_of(bl.kind(0)).unwrap();
        assert!(matches!(bl.kind(1), LineKind::TableSep { .. }));
        assert_eq!(bl.table(t).last, 1);
    }

    #[test]
    fn no_separator_means_no_table() {
        assert_eq!(
            kinds("| A | B |\n| 1 | 2 |"),
            [LineKind::Paragraph, LineKind::Paragraph]
        );
    }

    #[test]
    fn mismatched_column_count_breaks_table() {
        let ks = kinds("| A | B |\n|---|---|\n| 1 | 2 | 3 |");
        assert!(table_of(&ks[0]).is_some());
        assert!(table_of(&ks[1]).is_some());
        assert_eq!(ks[2], LineKind::Paragraph);
        // The separator's cell count must match the header's too.
        assert_eq!(kinds("| A | B |\n|---|")[0], LineKind::Paragraph);
    }

    #[test]
    fn adjacent_tables_get_different_ids() {
        let ks = kinds("| A |\n|---|\n| 1 |\n\n| B |\n|---|\n| 2 |");
        let (a, b) = (table_of(&ks[0]).unwrap(), table_of(&ks[4]).unwrap());
        assert_ne!(a, b);
        assert_eq!(ks[3], LineKind::Blank);
    }

    #[test]
    fn table_inside_code_is_not_table() {
        let ks = kinds("```\n| A | B |\n|---|---|\n| 1 | 2 |\n```\n| C |\n|---|");
        for k in &ks[1..4] {
            assert!(matches!(k, LineKind::Code { .. }), "{k:?}");
        }
        assert!(matches!(ks[5], LineKind::TableHeader { .. }));
        // A header just above a fence cannot use the fence line as separator.
        assert_eq!(kinds("| A |\n```\n|---|")[0], LineKind::Paragraph);
    }

    #[test]
    fn single_column_table_is_valid() {
        let (_, bl) = blocks("| A |\n|---|\n| 1 |");
        let t = table_of(bl.kind(0)).unwrap();
        assert_eq!(bl.table(t).aligns.len(), 1);
    }

    #[test]
    fn deleting_separator_row_clears_table() {
        let (mut b, mut bl) = blocks("| A | B |\n|---|---|\n| 1 | 2 |");
        let changed = edit(&mut b, &mut bl, p(1, 0)..p(2, 0), "");
        assert_eq!(b.to_text(), "| A | B |\n| 1 | 2 |");
        assert_eq!(bl.kind(0), &LineKind::Paragraph);
        assert_eq!(bl.kind(1), &LineKind::Paragraph);
        assert_eq!(changed, 0..2);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn breaking_separator_text_clears_table() {
        let (mut b, mut bl) = blocks("| A | B |\n|---|---|\n| 1 | 2 |");
        set_line(&mut b, &mut bl, 1, "not a separator");
        assert_eq!(bl.kind(0), &LineKind::Paragraph);
        assert_eq!(bl.kind(2), &LineKind::Paragraph);
        assert_fresh(&b, &bl);
    }

    // ---- incremental update ----

    #[test]
    fn update_insert_lines_keeps_untouched_ids_and_shifts_infos() {
        let (mut b, mut bl) = blocks("# h\npara\n```\ncode\n```\n| a |\n|---|\n- x");
        let code = block_of(bl.kind(2)).unwrap();
        let table = table_of(bl.kind(5)).unwrap();
        let changed = edit(&mut b, &mut bl, p(1, 0)..p(1, 0), "new\n- lines\n");
        assert_eq!(changed, 1..4);
        assert_eq!(block_of(bl.kind(4)), Some(code));
        assert_eq!(bl.code_block(code).open, 4);
        assert_eq!(bl.code_block(code).close, Some(6));
        assert_eq!(table_of(bl.kind(7)), Some(table));
        assert_eq!(bl.table(table).header, 7);
        assert_eq!(bl.table(table).last, 8);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn update_delete_lines_shifts_infos() {
        let (mut b, mut bl) = blocks("a\nb\nc\n```\ncode\n```");
        let code = block_of(bl.kind(3)).unwrap();
        let changed = edit(&mut b, &mut bl, p(0, 1)..p(2, 1), "");
        assert_eq!(b.to_text(), "a\n```\ncode\n```");
        assert_eq!(changed, 0..1);
        assert_eq!(bl.code_block(code).open, 1);
        assert_eq!(bl.code_block(code).close, Some(3));
        assert_fresh(&b, &bl);
    }

    #[test]
    fn update_opening_a_fence_rescans_to_end() {
        let (mut b, mut bl) = blocks("a\nb\n- c\n| t |\n|---|");
        let changed = set_line(&mut b, &mut bl, 0, "```rust");
        assert_eq!(changed, 0..5);
        let id = block_of(bl.kind(0)).unwrap();
        for i in 1..5 {
            assert_eq!(bl.kind(i), &LineKind::Code { block: id });
        }
        assert_eq!(bl.code_block(id).close, None);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn update_opening_a_fence_rescans_to_next_fence() {
        let (mut b, mut bl) = blocks("a\nb\n```\nc\n```\nd");
        let changed = set_line(&mut b, &mut bl, 0, "```");
        // Lines 0..3 are now a block closed by the old opener; "c" is outside
        // and the old closer opens an unclosed block running to the end.
        assert_eq!(changed, 0..6);
        assert!(matches!(bl.kind(2), LineKind::FenceClose { .. }));
        assert_eq!(bl.kind(3), &LineKind::Paragraph);
        assert!(matches!(bl.kind(4), LineKind::FenceOpen { .. }));
        assert!(matches!(bl.kind(5), LineKind::Code { .. }));
        assert_fresh(&b, &bl);
    }

    #[test]
    fn update_fence_rescan_stops_once_states_agree() {
        let (mut b, mut bl) = blocks("```\nx\n```\npara\n```\ny\n```");
        let later = block_of(bl.kind(4)).unwrap();
        let changed = set_line(&mut b, &mut bl, 1, "x2");
        assert_eq!(changed, 1..2);
        assert_eq!(block_of(bl.kind(4)), Some(later));
        assert_fresh(&b, &bl);
    }

    #[test]
    fn removing_close_lets_block_run_into_next_opener() {
        let (mut b, mut bl) = blocks("```\na\n```\nb\n```rust\nc\n```");
        let first = block_of(bl.kind(0)).unwrap();
        let second = block_of(bl.kind(4)).unwrap();
        let changed = set_line(&mut b, &mut bl, 2, "x");
        // The first block's close changed under its kept id: repaint all of it.
        assert_eq!(changed, 0..4);
        assert_eq!(bl.kind(3), &LineKind::Code { block: first });
        assert_eq!(bl.code_block(first).close, None);
        assert_eq!(block_of(bl.kind(4)), Some(second));
        assert_eq!(bl.code_block(second).close, Some(6));
        assert_fresh(&b, &bl);
    }

    #[test]
    fn closing_unclosed_fence_far_below_repaints_block() {
        let (mut b, mut bl) = blocks("para\n```\na\nb\nc\nd\ne\nf");
        let id = block_of(bl.kind(1)).unwrap();
        let changed = set_line(&mut b, &mut bl, 7, "```");
        assert_eq!(bl.code_block(id).close, Some(7));
        assert_eq!(changed, 1..8, "every line of the block, opener to closer");
        assert_fresh(&b, &bl);
    }

    #[test]
    fn unclosed_block_growing_past_removed_opener_repaints_block() {
        let (mut b, mut bl) = blocks("```\na\nb\n```rust\nc");
        let id = block_of(bl.kind(0)).unwrap();
        let changed = set_line(&mut b, &mut bl, 3, "x");
        assert_eq!(bl.kind(4), &LineKind::Code { block: id });
        assert_eq!(bl.code_block(id).close, None);
        // Last line moved from 2 to 4 with close still None.
        assert_eq!(changed, 0..5);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn unclosed_block_cut_short_by_new_opener_repaints_block() {
        let (mut b, mut bl) = blocks("```\na\nb\nc\nd");
        let id = block_of(bl.kind(0)).unwrap();
        let changed = set_line(&mut b, &mut bl, 2, "```go");
        assert_eq!(bl.kind(1), &LineKind::Code { block: id });
        assert_eq!(bl.code_block(id).close, None);
        // Last line moved from 4 to 1: line 1 is the new last line.
        assert_eq!(changed, 0..5);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn editing_last_line_of_unclosed_block_keeps_range_tight() {
        let (mut b, mut bl) = blocks("```\na\nb");
        let changed = set_line(&mut b, &mut bl, 2, "bc");
        assert_eq!(changed, 2..3);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn removing_close_repaints_block_to_end() {
        let (mut b, mut bl) = blocks("```\na\nb\n```\nc");
        let id = block_of(bl.kind(0)).unwrap();
        let changed = set_line(&mut b, &mut bl, 3, "x");
        assert_eq!(bl.code_block(id).close, None);
        assert_eq!(changed, 0..5);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn changing_fence_language_repaints_block() {
        let (mut b, mut bl) = blocks("x\n```rust\na\nb\n```\ny");
        let changed = set_line(&mut b, &mut bl, 1, "```go");
        let id = block_of(bl.kind(1)).unwrap();
        assert_eq!(bl.code_block(id).lang.as_deref(), Some("go"));
        assert_eq!(changed, 1..5);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn edit_inside_fence_reclassifies_to_close() {
        let (mut b, mut bl) = blocks("```\ncode\n``x\n# after\n- item");
        assert!(matches!(bl.kind(2), LineKind::Code { .. }));
        assert!(matches!(bl.kind(3), LineKind::Code { .. }));
        let id = block_of(bl.kind(0)).unwrap();
        let changed = set_line(&mut b, &mut bl, 2, "```");
        assert_eq!(changed, 0..5);
        assert_eq!(bl.kind(2), &LineKind::FenceClose { block: id });
        assert_eq!(bl.code_block(id).close, Some(2));
        assert_eq!(bl.kind(3), &LineKind::Heading(1));
        assert!(matches!(bl.kind(4), LineKind::ListItem { .. }));
        assert_fresh(&b, &bl);
    }

    #[test]
    fn removing_closing_fence_extends_block_to_end() {
        let (mut b, mut bl) = blocks("```\nx\n```\n# h\n- l");
        let id = block_of(bl.kind(0)).unwrap();
        let changed = edit(&mut b, &mut bl, p(1, 1)..p(2, 3), "");
        assert_eq!(b.to_text(), "```\nx\n# h\n- l");
        assert_eq!(changed, 0..4);
        assert_eq!(bl.kind(2), &LineKind::Code { block: id });
        assert_eq!(bl.code_block(id).close, None);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn table_alignment_change_repaints_whole_table() {
        let (mut b, mut bl) = blocks("intro\n| a | b |\n|---|---|\n| 1 | 2 |\nend");
        let t = table_of(bl.kind(1)).unwrap();
        let changed = set_line(&mut b, &mut bl, 2, "|:-:|--:|");
        assert_eq!(table_of(bl.kind(1)), Some(t), "id kept");
        assert_eq!(bl.table(t).aligns, [Align::Centre, Align::Right]);
        assert_eq!(changed, 1..4);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn table_gaining_a_row_repaints_whole_table() {
        let (mut b, mut bl) = blocks("| a | b |\n|---|---|\n| 1 | 2 |\nx");
        let t = table_of(bl.kind(0)).unwrap();
        let changed = set_line(&mut b, &mut bl, 3, "| a much longer cell | 4 |");
        assert_eq!(table_of(bl.kind(0)), Some(t), "id kept");
        assert_eq!(bl.table(t).last, 3);
        assert_eq!(changed, 0..4);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn update_line_becomes_table_separator() {
        let (mut b, mut bl) = blocks("intro\n| a | b |\ntext\n| 1 | 2 |\n| 3 | 4 |\nend");
        let changed = set_line(&mut b, &mut bl, 2, "|:--|--:|");
        assert_eq!(changed, 1..5);
        let t = table_of(bl.kind(1)).unwrap();
        assert_eq!(
            bl.table(t),
            &TableInfo {
                header: 1,
                sep: 2,
                last: 4,
                aligns: vec![Align::Left, Align::Right]
            }
        );
        assert_eq!(bl.kind(5), &LineKind::Paragraph);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn update_pure_deletion_with_empty_edited_range() {
        let (_, mut bl) = blocks("a\n```\nb\n| x |\n|---|");
        let b = Buffer::from_text("a\nb\n| x |\n|---|");
        let changed = bl.update(&b, 1..1, -1);
        assert_eq!(changed, 1..4);
        assert_fresh(&b, &bl);
    }

    #[test]
    fn update_with_broken_contract_rebuilds() {
        let (_, mut bl) = blocks("a\nb");
        let b = Buffer::from_text("```\nx\ny");
        assert_eq!(bl.update(&b, 0..1, 0), 0..3);
        assert_fresh(&b, &bl);
    }

    /// Deterministic pseudo-random edits (an LCG, no dependency); after each
    /// one `update` must agree with a fresh scan, and every line outside the
    /// returned range must keep its old kind exactly, ids included.
    #[test]
    fn update_matches_fresh_scan_after_edit_sequence() {
        const SEED: &str =
            "# t\n```\ncode\n```\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- a\n  1. b\n> q\ntext\n";
        const PIECES: [&str; 19] = [
            "``` ",
            "```",
            "```rust",
            "| a | b |",
            "|---|---|",
            "| 1 | 2 |",
            "- ",
            "1. ",
            "\n",
            "> ",
            "x",
            "",
            "# ",
            "***",
            "\n```\n",
            "\n| c |\n|:-:|\n",
            "![i](s)",
            "\n- a\n  1. b\n",
            "\n| 3 | 4 |\n",
        ];
        for seed in [0x5eed, 7, 1_234_567] {
            run_edit_sequence(seed, SEED, &PIECES);
        }
    }

    fn run_edit_sequence(mut seed: u64, start_text: &str, pieces: &[&str]) {
        let label = seed;
        let mut next = |n: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % n.max(1)
        };
        let (mut b, mut bl) = blocks(start_text);
        let (mut with_code, mut with_table) = (0, 0);
        for step in 0..1000 {
            let l0 = next(b.line_count());
            let l1 = if next(4) == 0 {
                (l0 + 1).min(b.line_count() - 1)
            } else {
                l0
            };
            let c0 = next(b.line(l0).len() + 1);
            let c1 = next(b.line(l1).len() + 1);
            let (start, end) = if (l0, c0) <= (l1, c1) {
                (p(l0, c0), p(l1, c1))
            } else {
                (p(l1, c1), p(l0, c0))
            };
            let (start, end) = if next(3) == 0 {
                (start, start)
            } else {
                (start, end)
            };
            // Keep the document from collapsing into a line or two.
            let text = if b.line_count() < 8 {
                start_text
            } else {
                pieces[next(pieces.len())]
            };

            let before_snap = snapshot(&b, &bl);
            let before = b.line_count();
            let e = b.replace(start..end, text);
            let delta = b.line_count() as isize - before as isize;
            let edited = e.range.start.line..e.range.end.line + 1;
            let changed = bl.update(&b, edited.clone(), delta);
            check_update(
                &before_snap,
                &b,
                &bl,
                edited,
                delta,
                changed,
                &format!("seed {label} step {step}"),
            );
            with_code += usize::from(!bl.code_blocks.is_empty());
            with_table += usize::from(!bl.tables.is_empty());
        }
        // The sequence must actually exercise fences and tables.
        assert!(
            with_code > 300 && with_table > 300,
            "seed {label}: {with_code} {with_table}"
        );
    }

    /// Replaces line `l` with `text` and checks `update` against a fresh parse.
    fn checked_set_line(b: &mut Buffer, bl: &mut Blocks, l: usize, text: &str) -> Range<usize> {
        let snap = snapshot(b, bl);
        let before = b.line_count();
        let len = b.line(l).len();
        let e = b.replace(p(l, 0)..p(l, len), text);
        let delta = b.line_count() as isize - before as isize;
        let edited = e.range.start.line..e.range.end.line + 1;
        let changed = bl.update(b, edited.clone(), delta);
        check_update(&snap, b, bl, edited, delta, changed.clone(), text);
        changed
    }

    const TABLE: &str = "intro\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |\nafter";

    /// Runs `f` and returns how many times `Blocks::update` took a fast path
    /// meanwhile.
    fn fast_paths(f: impl FnOnce()) -> usize {
        let before = FAST_PATHS.with(|n| n.get());
        f();
        FAST_PATHS.with(|n| n.get()) - before
    }

    #[test]
    fn editing_a_body_row_in_place_repaints_one_line() {
        let (mut b, mut bl) = blocks(TABLE);
        let mut changed = 0..0;
        let fast = fast_paths(|| changed = checked_set_line(&mut b, &mut bl, 4, "| 3 | 44 |"));
        assert_eq!(fast, 1);
        assert_eq!(changed, 4..5);
    }

    #[test]
    fn a_body_row_losing_a_cell_takes_the_general_path() {
        let (mut b, mut bl) = blocks(TABLE);
        let mut changed = 0..0;
        let fast = fast_paths(|| changed = checked_set_line(&mut b, &mut bl, 4, "| 3 |"));
        assert_eq!(fast, 0);
        assert!(changed.contains(&4));
    }

    #[test]
    fn editing_the_header_takes_the_general_path() {
        let (mut b, mut bl) = blocks(TABLE);
        let fast = fast_paths(|| {
            checked_set_line(&mut b, &mut bl, 1, "| a | bb |");
            checked_set_line(&mut b, &mut bl, 1, "| a |");
        });
        assert_eq!(fast, 0);
    }

    #[test]
    fn editing_the_separator_takes_the_general_path() {
        let (mut b, mut bl) = blocks(TABLE);
        let fast = fast_paths(|| {
            checked_set_line(&mut b, &mut bl, 2, "|:--|--:|");
            checked_set_line(&mut b, &mut bl, 2, "|---|");
        });
        assert_eq!(fast, 0);
    }

    #[test]
    fn splitting_a_row_takes_the_general_path() {
        let (mut b, mut bl) = blocks(TABLE);
        let snap = snapshot(&b, &bl);
        let before = b.line_count();
        let e = b.replace(p(4, 5)..p(4, 5), "|\n|");
        let delta = b.line_count() as isize - before as isize;
        let edited = e.range.start.line..e.range.end.line + 1;
        let mut changed = 0..0;
        let fast = fast_paths(|| changed = bl.update(&b, edited.clone(), delta));
        assert_eq!(fast, 0);
        check_update(&snap, &b, &bl, edited, delta, changed, "split");
    }

    #[test]
    fn a_body_row_that_looks_like_a_separator_keeps_its_cell_count() {
        let (mut b, mut bl) = blocks(TABLE);
        let mut changed = 0..0;
        let fast = fast_paths(|| {
            changed = checked_set_line(&mut b, &mut bl, 4, "|---|---|");
            checked_set_line(&mut b, &mut bl, 5, "  | ``` | x |  ");
        });
        assert_eq!(fast, 2);
        assert_eq!(changed, 4..5);
    }

    #[test]
    fn editing_a_line_that_stays_out_of_every_table_repaints_one_line() {
        let (mut b, mut bl) = blocks(TABLE);
        // The first key splits the table: the general path.
        let fast = fast_paths(|| {
            checked_set_line(&mut b, &mut bl, 4, "x| 3 | 4 |");
        });
        assert_eq!(fast, 0);
        // Later keys leave the line out of every table and every fence.
        let mut changed = 0..0;
        let fast = fast_paths(|| {
            changed = checked_set_line(&mut b, &mut bl, 4, "xy| 3 | 4 |");
        });
        assert_eq!(fast, 1);
        assert_eq!(changed, 4..5);
        // Its own kind may change, and is still right.
        let fast = fast_paths(|| {
            checked_set_line(&mut b, &mut bl, 4, "# xy| 3 | 4 |");
            checked_set_line(&mut b, &mut bl, 0, "- intro");
            checked_set_line(&mut b, &mut bl, 6, "");
        });
        assert_eq!(fast, 3);
    }

    #[test]
    fn a_pipe_line_outside_any_table_losing_its_pipes_takes_the_fast_path() {
        let text = "| a | b |\n|---|---|\n| 1 | 2 |\n| x |\n| c | d |\n|---|---|\n| 3 | 4 |";
        let (mut b, mut bl) = blocks(text);
        let fast = fast_paths(|| {
            checked_set_line(&mut b, &mut bl, 3, "x");
        });
        assert_eq!(fast, 1);
    }

    #[test]
    fn a_line_joining_a_table_or_a_fence_takes_the_general_path() {
        let (mut b, mut bl) = blocks(TABLE);
        checked_set_line(&mut b, &mut bl, 4, "x| 3 | 4 |");
        let fast = fast_paths(|| {
            // Back into the table. The line is no body row before the
            // edit, so the in-row fast path does not apply either.
            checked_set_line(&mut b, &mut bl, 4, "| 3 | 4 |");
            // Opening a fence.
            checked_set_line(&mut b, &mut bl, 6, "```");
            // Inside the fence it opened.
            checked_set_line(&mut b, &mut bl, 6, "```rust");
        });
        assert_eq!(fast, 0);
    }
}

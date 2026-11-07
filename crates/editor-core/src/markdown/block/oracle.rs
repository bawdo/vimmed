//! The incremental-reparse oracle: checks that `Blocks::update` agrees with
//! a fresh scan and repaints everything its edit could have changed. Shared
//! by the block tests and the `markdown_incremental` fuzz harness.

use std::collections::HashMap;
use std::ops::Range;

use crate::buffer::Buffer;
use crate::markdown::table::{TableId, TableInfo};

use super::{BlockId, Blocks, CodeBlockInfo, LineKind};

pub(crate) fn block_of(kind: &LineKind) -> Option<BlockId> {
    match kind {
        LineKind::FenceOpen { block, .. }
        | LineKind::FenceClose { block }
        | LineKind::Code { block } => Some(*block),
        _ => None,
    }
}

pub(crate) fn table_of(kind: &LineKind) -> Option<TableId> {
    match kind {
        LineKind::TableHeader { table }
        | LineKind::TableSep { table }
        | LineKind::TableRow { table } => Some(*table),
        _ => None,
    }
}

/// The kinds with ids renamed by first appearance, each id-bearing line
/// carrying its block or table info, so two `Blocks` compare equal when
/// they describe the same structure. Also checks no stale infos linger.
pub(crate) fn canon(buf: &Buffer, bl: &Blocks) -> Vec<String> {
    let mut bids: HashMap<BlockId, usize> = HashMap::new();
    let mut tids: HashMap<TableId, usize> = HashMap::new();
    let out = (0..buf.line_count())
        .map(|i| {
            let kind = bl.kind(i);
            if let Some(b) = block_of(kind) {
                let n = bids.len();
                let c = *bids.entry(b).or_insert(n);
                let mut s = format!("{kind:?}");
                s = s.replace(&format!("{b:?}"), &format!("B{c}"));
                format!("{s} {:?}", bl.code_block(b))
            } else if let Some(t) = table_of(kind) {
                let n = tids.len();
                let c = *tids.entry(t).or_insert(n);
                let s = format!("{kind:?}").replace(&format!("{t:?}"), &format!("T{c}"));
                format!("{s} {:?}", bl.table(t))
            } else {
                format!("{kind:?}")
            }
        })
        .collect();
    assert_eq!(bl.code_blocks.len(), bids.len(), "stale code block infos");
    assert_eq!(bl.tables.len(), tids.len(), "stale table infos");
    out
}

pub(crate) fn assert_fresh(buf: &Buffer, bl: &Blocks) {
    assert_eq!(
        canon(buf, bl),
        canon(buf, &Blocks::scan(buf)),
        "{:?}",
        buf.to_text()
    );
}

/// The parts of a `Blocks` that `check_update` compares against, taken
/// before the edit.
pub(crate) struct Snapshot {
    kinds: Vec<LineKind>,
    tables: HashMap<TableId, TableInfo>,
    code_blocks: HashMap<BlockId, CodeBlockInfo>,
}

/// `buf` is the buffer `bl` describes, before the edit.
pub(crate) fn snapshot(buf: &Buffer, bl: &Blocks) -> Snapshot {
    Snapshot {
        kinds: (0..buf.line_count()).map(|i| bl.kind(i).clone()).collect(),
        tables: bl.tables.clone(),
        code_blocks: bl.code_blocks.clone(),
    }
}

/// After `changed = bl.update(buf, edited, delta)`: `update` must agree
/// with a fresh scan, every line outside `changed` must keep its old kind
/// exactly (ids included), and any table or code block whose extent changed
/// under a kept id must be repainted whole. `label` names the case in the
/// failure messages.
pub(crate) fn check_update(
    before: &Snapshot,
    buf: &Buffer,
    bl: &Blocks,
    edited: Range<usize>,
    delta: isize,
    changed: Range<usize>,
    label: &str,
) {
    let old = &before.kinds;
    let old_tables = &before.tables;
    let old_code = &before.code_blocks;
    let b = buf;

    assert_fresh(b, bl);
    assert!(
        changed.start <= edited.start && changed.end >= edited.end,
        "{label}"
    );
    for i in (0..b.line_count()).filter(|i| !changed.contains(i)) {
        let o = if i < edited.start {
            i
        } else {
            (i as isize - delta) as usize
        };
        assert_eq!(bl.kind(i), &old[o], "{label} line {i}");
    }
    // A table whose info changed (after mapping old lines into the new
    // buffer) must be repainted whole, even when its id was kept. A
    // line inside the edit keeps its offset while still inside
    // `edited`, and is otherwise gone.
    let old_e = (edited.end as isize - delta) as usize;
    let map = |x: usize| {
        if x >= old_e {
            Some((x as isize + delta) as usize)
        } else if x < edited.end {
            Some(x)
        } else {
            None
        }
    };
    for (id, info) in &bl.tables {
        let Some(was) = old_tables.get(id) else {
            continue;
        };
        let same = map(was.header) == Some(info.header)
            && map(was.sep) == Some(info.sep)
            && map(was.last) == Some(info.last)
            && was.aligns == info.aligns;
        if !same {
            assert!(
                changed.start <= info.header && changed.end > info.last,
                "{label}: table {info:?} was {was:?}, changed {changed:?}"
            );
        }
    }
    // Likewise a code block whose open, close, lang or last line (from
    // the kinds, which is how an unclosed block's end is known) changed
    // under a kept id: opener to the later of its old and new last
    // lines is in range.
    for (id, info) in &bl.code_blocks {
        let Some(was) = old_code.get(id) else {
            continue;
        };
        let mut last = info.open;
        while block_of(bl.kind(last + 1)) == Some(*id) {
            last += 1;
        }
        let old_last = old
            .iter()
            .rposition(|k| block_of(k) == Some(*id))
            .and_then(map);
        let same = map(was.open) == Some(info.open)
            && was.close.map(map) == info.close.map(Some)
            && was.lang == info.lang
            && old_last == Some(last);
        if !same {
            let end = old_last.map_or(last, |o| o.max(last).min(b.line_count() - 1));
            assert!(
                changed.start <= info.open && changed.end > end,
                "{label}: block {info:?} last {last} was {was:?} last {old_last:?}, changed {changed:?}"
            );
        }
    }
}

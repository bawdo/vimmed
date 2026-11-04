//! Code block highlighting, render side.
//!
//! `ViewSource` has no block ids, so the view finds each painted code block's
//! lines through `display` (the `CodeBg` decorations), gives it an id of its
//! own and feeds the `Highlighter` the block's content lines joined with
//! `\n`. `request` is idempotent, but the text is only sent when it changed.
//!
//! Spans land on a later frame and are indexed by line within the block, so
//! after an edit the landed spans belong to the old text. Until the new
//! result lands, each line keeps the spans of the identical line in the old
//! text when it lies in the unchanged run before or after the edit, and has
//! none otherwise: a line count change never paints a line with its old
//! neighbour's colours.

use std::ops::Range;

use editor_core::host::Waker;
use editor_core::markdown::{BlockDeco, BlockId, LineDisplay};
use editor_core::view_model::ViewSource;

use crate::highlight::{Highlighter, LineSpans, language_for};
use crate::paint::fence_lang;

/// The most blocks tracked at once. Past it, the block painted longest ago is
/// dropped and its id reused, which bounds the highlighter's memory.
const MAX_TRACKED: usize = 32;

/// A code block's lines, found this frame.
struct Extent {
    /// Every line of the block, fences included.
    lines: Range<usize>,
    /// The lines between the fences.
    content: Range<usize>,
    /// The tracked block, when the language is one we highlight.
    id: Option<BlockId>,
}

struct Track {
    id: BlockId,
    /// The first content line, as last painted.
    start: usize,
    lang: String,
    /// The text last sent to the highlighter.
    text: String,
    /// The text whose spans the highlighter holds, once a result has landed.
    landed: Option<String>,
    /// While a newer text is pending: for each current line, the landed line
    /// whose spans it may show.
    remap: Option<Vec<Option<usize>>>,
    /// The frame the block was last painted in.
    used: u64,
}

pub(crate) struct CodeTracks {
    hl: Option<Highlighter>,
    tracks: Vec<Track>,
    extents: Vec<Extent>,
    next_id: u32,
    frame: u64,
}

/// For each line of `new`, the line of `old` with the same text in the
/// unchanged run before or after the edit.
fn remap(old: &str, new: &str) -> Vec<Option<usize>> {
    let o: Vec<&str> = old.split('\n').collect();
    let n: Vec<&str> = new.split('\n').collect();
    let pre = o.iter().zip(&n).take_while(|(a, b)| a == b).count();
    let room = o.len().min(n.len()) - pre;
    let post = o
        .iter()
        .rev()
        .zip(n.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    (0..n.len())
        .map(|k| {
            if k < pre {
                Some(k)
            } else if k >= n.len() - post {
                Some(k + o.len() - n.len())
            } else {
                None
            }
        })
        .collect()
}

impl CodeTracks {
    pub(crate) fn new() -> CodeTracks {
        CodeTracks {
            hl: None,
            tracks: Vec::new(),
            extents: Vec::new(),
            next_id: 0,
            frame: 0,
        }
    }

    pub(crate) fn enable(&mut self, waker: Waker) {
        self.hl = Some(Highlighter::new(waker));
    }

    /// Starts a frame: block extents are found afresh.
    pub(crate) fn begin_frame(&mut self) {
        self.frame += 1;
        self.extents.clear();
    }

    /// The block and the highlight classes for code line `i` (displayed as
    /// `d`), requesting the block when its text changed. `None` when
    /// highlighting is off, the line is not block content, or the language is
    /// unknown.
    pub(crate) fn classes(
        &mut self,
        src: &dyn ViewSource,
        i: usize,
        d: &LineDisplay,
    ) -> Option<(BlockId, usize, LineSpans)> {
        self.hl.as_ref()?;
        let e = match self.extents.iter().position(|e| e.lines.contains(&i)) {
            Some(k) => k,
            None => {
                let e = self.find(src, i, d)?;
                self.extents.push(e);
                self.extents.len() - 1
            }
        };
        let e = &self.extents[e];
        if !e.content.contains(&i) {
            return None;
        }
        let id = e.id?;
        let k = i - e.content.start;
        Some((id, k, self.line_classes(id, k)))
    }

    /// The classes of line `k` of tracked block `id` as things stand.
    pub(crate) fn line_classes(&self, id: BlockId, k: usize) -> LineSpans {
        let (Some(hl), Some(tr)) = (&self.hl, self.tracks.iter().find(|t| t.id == id)) else {
            return Vec::new();
        };
        if tr.landed.is_none() {
            return Vec::new();
        }
        let from = match &tr.remap {
            None => Some(k),
            Some(m) => m.get(k).copied().flatten(),
        };
        from.map_or_else(Vec::new, |o| hl.spans(id, o).to_vec())
    }

    /// Takes landed results. Returns each changed block's id and its content
    /// lines as last painted.
    pub(crate) fn poll(&mut self) -> Vec<(BlockId, Range<usize>)> {
        let Some(hl) = self.hl.as_mut() else {
            return Vec::new();
        };
        let changed = hl.poll();
        let mut out = Vec::new();
        for tr in &mut self.tracks {
            if changed.contains(&tr.id) {
                tr.landed = Some(tr.text.clone());
                tr.remap = None;
                let n = tr.text.split('\n').count();
                out.push((tr.id, tr.start..tr.start + n));
            }
        }
        out
    }

    /// Finds the block holding line `i` and makes sure its current text is
    /// requested.
    fn find(&mut self, src: &dyn ViewSource, i: usize, d: &LineDisplay) -> Option<Extent> {
        let BlockDeco::CodeBg {
            first,
            last,
            unclosed,
        } = d.deco
        else {
            return None;
        };
        let deco = |j: usize| src.display(j).deco;
        let (mut open, mut top) = (None, i);
        if first {
            open = Some(i);
        } else {
            while top > 0 {
                match deco(top - 1) {
                    BlockDeco::CodeBg { first: true, .. } => {
                        open = Some(top - 1);
                        break;
                    }
                    BlockDeco::CodeBg { .. } => top -= 1,
                    _ => break,
                }
            }
        }
        let (mut end, mut closed) = (i, last && !unclosed);
        if !last {
            closed = false;
            let mut j = i + 1;
            while j < src.line_count() {
                match deco(j) {
                    BlockDeco::CodeBg {
                        last: true,
                        unclosed,
                        ..
                    } => {
                        end = j;
                        closed = !unclosed;
                        break;
                    }
                    BlockDeco::CodeBg { .. } => j += 1,
                    _ => {
                        end = j - 1;
                        break;
                    }
                }
            }
            if j >= src.line_count() {
                end = src.line_count() - 1;
            }
        }
        let start = open.map_or(top, |o| o + 1);
        let content_end = if closed { end } else { end + 1 };
        let content = start..content_end.max(start);
        let lines = open.unwrap_or(top)..end + 1;
        let lang = open.map_or("", |o| fence_lang(src.line(o))).to_owned();
        let id = (language_for(&lang).is_some() && !content.is_empty())
            .then(|| self.track(src, &content, lang));
        Some(Extent { lines, content, id })
    }

    /// The tracked block for `content`, requesting its text when it changed.
    fn track(&mut self, src: &dyn ViewSource, content: &Range<usize>, lang: String) -> BlockId {
        let text = content
            .clone()
            .map(|j| src.line(j).strip_suffix('\r').unwrap_or(src.line(j)))
            .collect::<Vec<_>>()
            .join("\n");
        let start = content.start;
        let frame = self.frame;
        let free = |t: &Track| t.used != frame;
        let found = self
            .tracks
            .iter()
            .position(|t| t.start == start && t.text == text && t.lang == lang)
            .or_else(|| {
                self.tracks
                    .iter()
                    .position(|t| free(t) && t.text == text && t.lang == lang)
            })
            .or_else(|| {
                self.tracks
                    .iter()
                    .position(|t| free(t) && t.start == start && t.lang == lang)
            });
        let hl = self.hl.as_mut().expect("highlighting is on");
        let k = match found {
            Some(k) => {
                let tr = &mut self.tracks[k];
                if tr.text != text {
                    hl.request(tr.id, &lang, &text);
                    tr.remap = tr.landed.as_deref().map(|old| remap(old, &text));
                    tr.text = text;
                }
                tr.start = start;
                k
            }
            None => {
                let id = if self.tracks.len() >= MAX_TRACKED {
                    let (k, _) = self
                        .tracks
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, t)| t.used)
                        .expect("tracks is full");
                    self.tracks.swap_remove(k).id
                } else {
                    self.next_id += 1;
                    BlockId(self.next_id)
                };
                hl.request(id, &lang, &text);
                self.tracks.push(Track {
                    id,
                    start,
                    lang,
                    text,
                    // A reused id still holds the dropped block's spans: show
                    // none until this block's land.
                    landed: None,
                    remap: None,
                    used: frame,
                });
                self.tracks.len() - 1
            }
        };
        self.tracks[k].used = frame;
        self.tracks[k].id
    }
}

#[cfg(test)]
mod tests {
    use super::remap;

    #[test]
    fn remap_keeps_the_unchanged_runs() {
        assert_eq!(remap("a\nb", "x\na\nb"), vec![None, Some(0), Some(1)]);
        assert_eq!(remap("a\nb\nc", "a\nc"), vec![Some(0), Some(2)]);
        assert_eq!(remap("a\nb", "a\nB"), vec![Some(0), None]);
        assert_eq!(remap("a", "a"), vec![Some(0)]);
    }
}

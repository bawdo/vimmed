//! Search: a literal query with smartcase, its matches per line for the
//! highlights, and stepping from a position to the next match (`/ ? n N`).
//!
//! Matches on a line are found as vim finds them: from the line start, each
//! match after the end of the one before, so they never overlap. A match
//! whose start or end would split a grapheme cluster is no match (vim does
//! not match `e` in a decomposed `é`), so every match is a range of whole
//! graphemes and a cursor put on its start rests on a grapheme boundary
//! (R12).

use std::cell::OnceCell;
use std::ops::Range;

use unicode_segmentation::GraphemeCursor;

use crate::buffer::{Buffer, Pos};
use crate::vim::Cursor;
use crate::vim::operator::Touched;

/// How long `search wrapped` shows: one second.
pub const WRAPPED_FOR_NS: u64 = 1_000_000_000;

/// A search: what is searched for, which way, and where it started, with
/// the matches on each line for the highlights.
///
/// The highlights are worked out a line at a time, when a line is asked
/// for (`matches_on`), so a key on the search line costs what the renderer
/// shows rather than the whole document. Each line's matches sit in a
/// `OnceCell`: `ViewSource` reads through `&self`, a cell is filled once
/// and never changed while it is shared, so the slice it lends stays
/// valid, and only `&mut self` (a new query, an edit) empties cells.
/// There is no `PartialEq`: which cells are filled is not part of a
/// search's value, and nothing compares searches.
#[derive(Debug, Clone)]
pub struct Search {
    pub query: String,
    /// True for `/`, false for `?`. `n` goes this way, `N` the other.
    pub forward: bool,
    /// The cursor when the search started, where Esc puts it back.
    pub origin: Cursor,
    /// The matches on each line, as `matches` gives them, once asked for.
    lines: Vec<OnceCell<Vec<Range<usize>>>>,
    /// The match the search last jumped to.
    current: Option<(usize, Range<usize>)>,
}

/// A match found from a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
    /// Source byte range on the line, on grapheme boundaries.
    pub range: Range<usize>,
    /// True when the search went past the end of the buffer (or, going
    /// backward, the start) to find it.
    pub wrapped: bool,
}

impl Hit {
    /// Where the cursor goes: the match's start.
    pub fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            col: self.range.start,
        }
    }
}

impl Search {
    /// An empty search starting at `origin`.
    pub fn new(forward: bool, origin: Cursor) -> Search {
        Search {
            query: String::new(),
            forward,
            origin,
            lines: Vec::new(),
            current: None,
        }
    }

    /// The matches in `buf`, line by line, worked out afresh.
    pub fn matches(&self, buf: &Buffer) -> Vec<(usize, Range<usize>)> {
        let m = Matcher::new(&self.query);
        (0..buf.line_count())
            .flat_map(|i| m.line(buf.line(i)).into_iter().map(move |r| (i, r)))
            .collect()
    }

    /// The matches on `line` of `buf`, for `ViewSource::matches`, worked
    /// out the first time the line is asked for since the search was last
    /// brought up to date (`refresh`, `edited`). `buf` must be the buffer
    /// it was brought up to date with.
    pub fn matches_on(&self, buf: &Buffer, line: usize) -> &[Range<usize>] {
        match self.lines.get(line) {
            Some(cell) if line < buf.line_count() => {
                cell.get_or_init(|| line_matches(buf.line(line), &self.query))
            }
            _ => &[],
        }
    }

    /// The match the search last jumped to.
    pub fn current(&self) -> Option<(usize, Range<usize>)> {
        self.current.clone()
    }

    /// Records the match the search jumped to (`None`: it found none).
    pub fn set_current(&mut self, hit: Option<&Hit>) {
        self.current = hit.map(|h| (h.line, h.range.clone()));
    }

    /// Forgets every line's matches, to be worked out again when asked for.
    pub fn refresh(&mut self, buf: &Buffer) {
        self.current = None;
        self.lines.clear();
        if !self.query.is_empty() {
            self.lines.resize(buf.line_count(), OnceCell::new());
        }
    }

    /// Brings the matches up to date after the one edit `t`: only the
    /// lines it replaced are forgotten, the others move with the edit.
    pub fn edited(&mut self, buf: &Buffer, t: &Touched) {
        self.current = None;
        if self.query.is_empty() {
            return;
        }
        let old_end = (t.old_end_line + 1).min(self.lines.len());
        let start = t.start_line.min(old_end);
        let new_end = (t.new_end_line + 1).min(buf.line_count());
        let fresh = (start..new_end.max(start)).map(|_| OnceCell::new());
        self.lines.splice(start..old_end, fresh);
        if self.lines.len() != buf.line_count() {
            self.refresh(buf);
        }
    }
}

/// True when `query` holds an uppercase letter, so the search is case
/// sensitive (smartcase).
pub fn case_sensitive(query: &str) -> bool {
    query.chars().any(char::is_uppercase)
}

/// The matches of `query` on `line`, in order, as vim finds them: each
/// one after the end of the one before, and only on grapheme boundaries.
/// Without an uppercase letter in `query`, case is ignored char by char,
/// so the ranges stay in the line's own bytes.
pub fn line_matches(line: &str, query: &str) -> Vec<Range<usize>> {
    Matcher::new(query).line(line)
}

/// The matches of `pattern` on `line` as `line_matches` finds them, but
/// case sensitive unless `ignore_case`, whatever the pattern holds (`:s`
/// and its `i` flag, not smartcase).
pub fn literal_matches(line: &str, pattern: &str, ignore_case: bool) -> Vec<Range<usize>> {
    Matcher::with_case(pattern, !ignore_case).line(line)
}

/// `literal_matches` made ready for line after line, giving each line's
/// matches one at a time rather than collected, so counting them (`:s`
/// before it builds anything) holds no memory per match.
pub struct Literal<'q>(Matcher<'q>);

impl<'q> Literal<'q> {
    pub fn new(pattern: &'q str, ignore_case: bool) -> Literal<'q> {
        Literal(Matcher::with_case(pattern, !ignore_case))
    }

    /// The matches on `line`, in order, as `literal_matches` gives them.
    pub fn matches<'a>(&'a self, line: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
        self.0.iter(line)
    }
}

/// U+212A KELVIN SIGN, the only char beyond ASCII that folds to an ASCII
/// one (`k`); `only_the_kelvin_sign_folds_to_ascii` checks that.
const KELVIN: char = '\u{212A}';

/// A query made ready for matching line after line.
struct Matcher<'q> {
    query: &'q str,
    sensitive: bool,
    /// Ignoring case, the query is all ASCII.
    ascii: bool,
    /// Ignoring case, the query holds `k` (or `K`), which the Kelvin sign
    /// matches.
    has_k: bool,
    /// Up to two pairs of bytes a line holding a match must hold one of
    /// each: the query's rarest bytes (by a fixed guess at how common they
    /// are in text). Ignoring case, the pairs are the two cases of an
    /// ASCII char other than `k`, which no other char folds to;
    /// otherwise a byte twice.
    needs: Vec<(u8, u8)>,
    /// The query's rarest byte, where the ASCII window path looks first:
    /// its offset in the query and its two cases.
    anchor: (usize, u8, u8),
}

/// A rough guess at how common an ASCII byte is in prose, higher being
/// more common, so a prefilter looks for the rarest bytes first.
fn commonness(b: u8) -> usize {
    const BY_FREQUENCY: &[u8] = b"etaoinshrdlcumwfgypbvkjxqz";
    match b.to_ascii_lowercase() {
        b' ' => 60,
        c if c.is_ascii_lowercase() => 50 - BY_FREQUENCY.iter().position(|&x| x == c).unwrap_or(0),
        c if c.is_ascii_digit() => 20,
        c if c.is_ascii() => 10,
        // A byte beyond ASCII.
        _ => 5,
    }
}

impl<'q> Matcher<'q> {
    /// Smartcase: case matters once `query` holds an uppercase letter.
    fn new(query: &'q str) -> Matcher<'q> {
        Matcher::with_case(query, case_sensitive(query))
    }

    /// Case matters only when `sensitive`. Ignoring it, the query may hold
    /// capitals too (`:s///i`), so `K` is treated as `k` throughout.
    fn with_case(query: &'q str, sensitive: bool) -> Matcher<'q> {
        let bytes = query.as_bytes();
        let mut ranked: Vec<u8> = bytes
            .iter()
            .copied()
            .filter(|&b| sensitive || (b.is_ascii() && !b.eq_ignore_ascii_case(&b'k')))
            .collect();
        ranked.sort_by_key(|&b| commonness(b));
        ranked.dedup();
        let needs = ranked
            .iter()
            .take(2)
            .map(|&b| {
                if sensitive {
                    (b, b)
                } else {
                    (b.to_ascii_lowercase(), b.to_ascii_uppercase())
                }
            })
            .collect();
        let anchor = (0..bytes.len())
            .min_by_key(|&i| commonness(bytes[i]))
            .map_or((0, 0, 0), |i| {
                (
                    i,
                    bytes[i].to_ascii_lowercase(),
                    bytes[i].to_ascii_uppercase(),
                )
            });
        Matcher {
            query,
            sensitive,
            ascii: query.is_ascii(),
            has_k: query.contains(['k', 'K']),
            needs,
            anchor,
        }
    }

    /// The matches on `line`, each checked for grapheme boundaries at its
    /// two ends only (R12).
    fn line(&self, line: &str) -> Vec<Range<usize>> {
        self.iter(line).collect()
    }

    /// The matches on `line` one at a time, as `line` gives them.
    fn iter<'a>(&'a self, line: &'a str) -> LineMatches<'a, 'q> {
        let done = self.query.is_empty() || !self.may_match(line.as_bytes());
        // Ignoring case, an ASCII query matches only ASCII bytes unless
        // the line holds a Kelvin sign, so whole bytes can be compared.
        let bytewise =
            !done && !self.sensitive && self.ascii && !(self.has_k && line.contains(KELVIN));
        LineMatches {
            m: self,
            line,
            at: if done { line.len() } else { 0 },
            bytewise,
        }
    }

    /// False when no match can be on `line`: it lacks a byte every match
    /// holds.
    fn may_match(&self, line: &[u8]) -> bool {
        // Ignoring case, a char beyond ASCII folds only to chars beyond it.
        (self.sensitive || self.ascii || !line.is_ascii())
            && self
                .needs
                .iter()
                .all(|&(lo, up)| scan(line, |b| b == lo || b == up).is_some())
    }

    /// The first window of `line` from `at` equal to the ASCII query,
    /// ignoring ASCII case, found from where its rarest byte is. It holds
    /// only ASCII, so both its ends are char boundaries.
    fn ascii_match(&self, line: &[u8], at: usize) -> Option<Range<usize>> {
        let query = self.query.as_bytes();
        let (off, lo, up) = self.anchor;
        let mut p = at + off;
        while p < line.len() {
            p += scan(&line[p..], |b| b == lo || b == up)?;
            let s = p - off;
            if s + query.len() > line.len() {
                return None;
            }
            if line[s..s + query.len()].eq_ignore_ascii_case(query) {
                return Some(s..s + query.len());
            }
            p += 1;
        }
        None
    }

    /// The first match in `line` starting at or after `at` (a char
    /// boundary). Ignoring case, chars are folded one by one, tried only
    /// where the query's first char could start: one of its two cases (or
    /// a Kelvin sign for `k`), or, for a first char beyond ASCII, a char
    /// beyond ASCII.
    fn next_match(&self, line: &str, at: usize) -> Option<Range<usize>> {
        let rest = &line[at..];
        if self.sensitive {
            return rest
                .find(self.query)
                .map(|o| at + o..at + o + self.query.len());
        }
        let first = self.query.as_bytes()[0];
        let (lo, up) = (first.to_ascii_lowercase(), first.to_ascii_uppercase());
        let kelvin = lo == b'k';
        let bytes = rest.as_bytes();
        let mut o = 0;
        while o < bytes.len() {
            o += if first.is_ascii() {
                scan(&bytes[o..], |b| {
                    b == lo || b == up || (kelvin && b == KELVIN_LEAD)
                })?
            } else {
                // 0xC0 and up starts a char of two or more bytes.
                scan(&bytes[o..], |b| b >= 0xC0)?
            };
            if let Some(n) = folded_len(&rest[o..], self.query) {
                return Some(at + o..at + o + n);
            }
            o += 1;
            while o < bytes.len() && !rest.is_char_boundary(o) {
                o += 1;
            }
        }
        None
    }
}

/// A line's matches in order, found as they are asked for: `Matcher::iter`.
struct LineMatches<'a, 'q> {
    m: &'a Matcher<'q>,
    line: &'a str,
    /// Where the next search starts; the line's length once done.
    at: usize,
    bytewise: bool,
}

impl Iterator for LineMatches<'_, '_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Range<usize>> {
        let line = self.line;
        while self.at < line.len() {
            let found = if self.bytewise {
                self.m.ascii_match(line.as_bytes(), self.at)
            } else {
                self.m.next_match(line, self.at)
            };
            let Some(r) = found else {
                self.at = line.len();
                return None;
            };
            if grapheme_boundary(line, r.start) && grapheme_boundary(line, r.end) {
                self.at = r.end;
                return Some(r);
            }
            // It splits a grapheme: look again from the next char.
            self.at = r.start + line[r.start..].chars().next().map_or(1, char::len_utf8);
        }
        None
    }
}

/// The first byte of the Kelvin sign.
const KELVIN_LEAD: u8 = 0xE2;

/// The index of the first byte of `hay` that `hit` picks. It tests 32
/// bytes at a time without stopping early inside them, which the compiler
/// can vectorise, then finds the byte in the block that had one.
#[inline]
fn scan(hay: &[u8], hit: impl Fn(u8) -> bool + Copy) -> Option<usize> {
    let mut base = 0;
    for block in hay.chunks(32) {
        if block.iter().fold(false, |any, &b| any | hit(b)) {
            return block.iter().position(|&b| hit(b)).map(|i| base + i);
        }
        base += block.len();
    }
    None
}

/// True when byte `i` (a char boundary) of `line` is a grapheme boundary,
/// looking only around `i`.
fn grapheme_boundary(line: &str, i: usize) -> bool {
    let b = line.as_bytes();
    if i == 0 || i >= b.len() {
        return true;
    }
    // Between two ASCII bytes there is a boundary, but inside CR LF.
    if b[i - 1].is_ascii() && b[i].is_ascii() {
        return !(b[i - 1] == b'\r' && b[i] == b'\n');
    }
    // The whole line is the chunk, so the cursor never asks for more.
    GraphemeCursor::new(i, line.len(), true)
        .is_boundary(line, 0)
        .unwrap_or_else(|_| {
            use unicode_segmentation::UnicodeSegmentation;
            line.grapheme_indices(true).any(|(g, _)| g == i)
        })
}

/// The length in bytes of the start of `text` that matches `query`, case
/// ignored char by char, or `None` when it does not match.
fn folded_len(text: &str, query: &str) -> Option<usize> {
    let mut chars = text.chars();
    let mut len = 0;
    for q in query.chars() {
        let c = chars.next()?;
        let same = c == q
            || if c.is_ascii() && q.is_ascii() {
                c.eq_ignore_ascii_case(&q)
            } else {
                c.to_lowercase().eq(q.to_lowercase())
            };
        if !same {
            return None;
        }
        len += c.len_utf8();
    }
    Some(len)
}

/// The `count`th match of `query` from `from` (0 counts as 1), going
/// forward or backward and wrapping round the buffer. A match at `from`
/// itself is not counted. `None` when there is no match at all.
///
/// Each line's matches are found once and stepped through by index, as
/// many as the count needs. Coming back round to the first match found
/// gives the number of matches in the document, and only the remainder
/// of the count is stepped after that, so any count costs at most two
/// passes over the buffer.
pub fn find(buf: &Buffer, query: &str, from: Pos, forward: bool, count: usize) -> Option<Hit> {
    if query.is_empty() {
        return None;
    }
    let m = Matcher::new(query);
    let first = first_hit(buf, &m, buf.clamp(from), forward)?;
    if count <= 1 {
        return Some(first);
    }
    // Each line's matches in the order the search meets them.
    let matches_on = |l: usize| {
        let mut v = m.line(buf.line(l));
        if !forward {
            v.reverse();
        }
        v
    };
    let n = buf.line_count();
    let mut line = first.line;
    let mut ms = matches_on(line);
    let at_first = ms.iter().position(|r| *r == first.range)?;
    let mut idx = at_first;
    let mut left = count - 1;
    let mut wrapped = first.wrapped;
    let mut taken = 0;
    let mut round_known = false;
    while left > 0 {
        let here = ms.len() - 1 - idx;
        if left <= here {
            idx += left;
            break;
        }
        left -= here;
        taken += here;
        // On to the next line with a match; the first one's line has one.
        loop {
            let (next, past_end) = if forward {
                ((line + 1) % n, line + 1 >= n)
            } else {
                ((line + n - 1) % n, line == 0)
            };
            line = next;
            wrapped |= past_end;
            ms = matches_on(line);
            if !ms.is_empty() {
                break;
            }
        }
        idx = 0;
        left -= 1;
        taken += 1;
        if !round_known && line == first.line {
            // Back on the first match's line, `at_first` steps short of
            // it: that many more make the round.
            round_known = true;
            if left >= at_first {
                left = (left - at_first) % (taken + at_first);
                idx = at_first;
            }
        }
    }
    Some(Hit {
        line,
        range: ms[idx].clone(),
        wrapped,
    })
}

/// The nearest match after `from` (forward) or before it, wrapping.
fn first_hit(buf: &Buffer, m: &Matcher, from: Pos, forward: bool) -> Option<Hit> {
    let n = buf.line_count();
    let here = m.line(buf.line(from.line));
    let near = if forward {
        here.into_iter().find(|r| r.start > from.col)
    } else {
        here.into_iter().rev().find(|r| r.start < from.col)
    };
    if let Some(range) = near {
        return Some(Hit {
            line: from.line,
            range,
            wrapped: false,
        });
    }
    // Then line by line, round to the cursor's line again (its matches on
    // the far side of the cursor come last).
    (1..=n).find_map(|k| {
        let (line, wrapped) = if forward {
            ((from.line + k) % n, from.line + k >= n)
        } else {
            ((from.line + n - k) % n, k > from.line)
        };
        let ms = m.line(buf.line(line));
        let range = if forward {
            ms.into_iter().next()
        } else {
            ms.into_iter().next_back()
        }?;
        Some(Hit {
            line,
            range,
            wrapped,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::Redraw;
    use crate::input::Input;
    use crate::testkit::*;
    use crate::vim::Mode;
    use crate::vim::operator::Operator;
    use crate::vim::repeat::Change;

    fn lines(text: &str) -> Buffer {
        Buffer::from_text(text)
    }

    /// One match range, to compare with `ViewSource::matches`.
    fn just(r: Range<usize>) -> Vec<Range<usize>> {
        vec![r]
    }

    fn all(text: &str, query: &str) -> Vec<(usize, Range<usize>)> {
        let mut s = Search::new(true, Cursor::at(&lines(text), pos(0, 0)));
        s.query = query.to_owned();
        s.matches(&lines(text))
    }

    fn hit(text: &str, query: &str, from: Pos, forward: bool) -> Option<(Pos, bool)> {
        find(&lines(text), query, from, forward, 1).map(|h| (h.pos(), h.wrapped))
    }

    // search_engine_test.dart

    #[test]
    fn empty_query_matches_nothing() {
        assert!(all("hello world\ntest line", "").is_empty());
        assert_eq!(hit("hello", "", pos(0, 0), true), None);
    }

    #[test]
    fn finds_a_single_match() {
        assert_eq!(all("hello world", "world"), vec![(0, 6..11)]);
    }

    #[test]
    fn finds_several_matches_on_a_line() {
        assert_eq!(
            all("hello hello hello", "hello"),
            vec![(0, 0..5), (0, 6..11), (0, 12..17)]
        );
    }

    #[test]
    fn finds_matches_across_lines() {
        assert_eq!(
            all("hello world\nworld hello\ntest", "hello"),
            vec![(0, 0..5), (1, 6..11)]
        );
    }

    #[test]
    fn lowercase_query_ignores_case() {
        assert_eq!(all("Hello HELLO hello", "hello").len(), 3);
    }

    #[test]
    fn uppercase_in_the_query_makes_it_case_sensitive() {
        assert_eq!(all("Hello HELLO hello", "Hello"), vec![(0, 0..5)]);
    }

    #[test]
    fn empty_lines_have_no_matches() {
        assert_eq!(all("\nhello\n", "hello"), vec![(1, 0..5)]);
    }

    #[test]
    fn the_query_is_literal() {
        assert_eq!(all("a.b\na*b\na[b", "a.b"), vec![(0, 0..3)]);
        assert_eq!(all("a.b\naxb", "."), vec![(0, 1..2)]);
    }

    #[test]
    fn smartcase_is_sensitive_only_with_an_uppercase_letter() {
        assert!(!case_sensitive("hello"));
        assert!(!case_sensitive("hello world"));
        assert!(!case_sensitive("123 ._"));
        assert!(case_sensitive("Hello"));
        assert!(case_sensitive("HELLO"));
        assert!(case_sensitive("heLLo"));
        assert!(case_sensitive("École"));
    }

    // Beyond moon_edit: ranges in source bytes, on grapheme boundaries.

    #[test]
    fn ignoring_case_keeps_source_byte_ranges() {
        // É is two bytes, as é is.
        assert_eq!(line_matches("ÉCOLE école", "école"), vec![0..6, 7..13]);
    }

    #[test]
    fn a_match_never_splits_a_grapheme() {
        // vim: `e` does not match the e of a decomposed é.
        assert_eq!(line_matches("xe\u{301} e", "e"), vec![5..6]);
        assert_eq!(line_matches("xe\u{301} e", "e\u{301}"), vec![1..4]);
        let zwj = "👩\u{200d}👩\u{200d}👧";
        assert_eq!(line_matches(&format!("{zwj} 👩"), "👩"), vec![19..23]);
    }

    #[test]
    fn only_the_kelvin_sign_folds_to_ascii() {
        // The candidate filter for a query starting with an ASCII char
        // rests on this.
        for c in ('\u{80}'..=char::MAX).filter(|c| !c.is_ascii()) {
            let mut lower = c.to_lowercase();
            if let (Some(l), None) = (lower.next(), lower.next())
                && l.is_ascii()
            {
                assert_eq!(c, '\u{212A}', "{c:?} folds to {l:?}");
            }
        }
        assert_eq!(line_matches("x\u{212A}m km", "km"), vec![1..5, 6..8]);
    }

    #[test]
    fn matches_do_not_overlap() {
        // vim: `aa` in `aaaa` matches at 0 and 2.
        assert_eq!(line_matches("aaaa", "aa"), vec![0..2, 2..4]);
    }

    // search_state_test.dart, from the cursor rather than a match index.

    #[test]
    fn find_goes_to_the_first_match_after_the_position() {
        let text = "xxxxxhello\n\nhello\n\n\nxxxhello";
        assert_eq!(
            hit(text, "hello", pos(1, 0), true),
            Some((pos(2, 0), false))
        );
        // A match at the position itself is not "after" it.
        assert_eq!(
            hit(text, "hello", pos(2, 0), true),
            Some((pos(5, 3), false))
        );
    }

    #[test]
    fn find_wraps_past_the_end() {
        let text = "xxxxxhello\n\nhello\nx\nx\nx";
        assert_eq!(hit(text, "hello", pos(5, 0), true), Some((pos(0, 5), true)));
        // The only match, under the cursor: found again after wrapping.
        assert_eq!(
            hit("hello", "hello", pos(0, 0), true),
            Some((pos(0, 0), true))
        );
    }

    #[test]
    fn find_backward_goes_to_the_match_before() {
        let text = "xxxxxhello\n\nhello\nx\nx\nxxxhello";
        assert_eq!(
            hit(text, "hello", pos(3, 0), false),
            Some((pos(2, 0), false))
        );
        assert_eq!(
            hit(text, "hello", pos(2, 0), false),
            Some((pos(0, 5), false))
        );
        assert_eq!(
            hit(text, "hello", pos(0, 5), false),
            Some((pos(5, 3), true))
        );
    }

    #[test]
    fn find_with_a_count_steps_that_many_matches() {
        let buf = lines("a1 a2 a3 a4 a5");
        let at = |count, from| find(&buf, "a", from, true, count).map(|h| (h.pos(), h.wrapped));
        assert_eq!(at(3, pos(0, 0)), Some((pos(0, 9), false)));
        assert_eq!(at(2, pos(0, 9)), Some((pos(0, 0), true)));
        assert_eq!(at(1_000_000_000, pos(0, 0)), Some((pos(0, 0), true)));
        // More than one round of the five matches.
        assert_eq!(at(6, pos(0, 0)), Some((pos(0, 3), true)));
        assert_eq!(at(11, pos(0, 0)), Some((pos(0, 3), true)));
        assert_eq!(
            find(&buf, "a", pos(0, 0), false, 7).map(|h| (h.pos(), h.wrapped)),
            Some((pos(0, 9), true))
        );
        assert_eq!(
            find(&buf, "a", pos(0, 0), false, 2).map(|h| h.pos()),
            Some(pos(0, 9))
        );
    }

    /// `find` as it was first built: every match collected, then indexed.
    fn find_by_index(
        buf: &Buffer,
        query: &str,
        from: Pos,
        forward: bool,
        count: usize,
    ) -> Option<(Pos, bool)> {
        let first = find(buf, query, from, forward, 1)?;
        let all: Vec<(usize, Range<usize>)> = (0..buf.line_count())
            .flat_map(|i| {
                line_matches(buf.line(i), query)
                    .into_iter()
                    .map(move |r| (i, r))
            })
            .collect();
        let n = all.len();
        let i = all
            .iter()
            .position(|(l, r)| *l == first.line && *r == first.range)?;
        let steps = count.max(1) - 1;
        let (j, wrapped) = if forward {
            ((i + steps % n) % n, steps >= n - i)
        } else {
            ((i + n - steps % n) % n, steps > i)
        };
        let (line, range) = all[j].clone();
        Some((
            Pos {
                line,
                col: range.start,
            },
            first.wrapped || wrapped,
        ))
    }

    #[test]
    fn counted_find_agrees_with_collecting_every_match() {
        let long = "ab ".repeat(2_000);
        let mixed = "a1 a2\n\nb a\nx\na a a\n\u{212A}a é a";
        for (text, query, froms) in [
            (
                long.as_str(),
                "a",
                vec![pos(0, 0), pos(0, 3000), pos(0, 5997)],
            ),
            (mixed, "a", vec![pos(0, 0), pos(2, 2), pos(4, 4), pos(5, 0)]),
            (mixed, "ka", vec![pos(0, 0), pos(5, 0)]),
        ] {
            let buf = lines(text);
            for &from in &froms {
                for forward in [true, false] {
                    for count in [
                        1,
                        2,
                        3,
                        7,
                        13,
                        1_999,
                        2_000,
                        2_001,
                        4_001,
                        30_000,
                        1_000_000_000,
                    ] {
                        assert_eq!(
                            find(&buf, query, from, forward, count).map(|h| (h.pos(), h.wrapped)),
                            find_by_index(&buf, query, from, forward, count),
                            "{query:?} from {from:?} forward {forward} count {count}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_bytewise_path_matches_the_char_by_char_one() {
        // ASCII queries ignoring case, on lines with and without text
        // beyond ASCII, a Kelvin sign and graphemes a match must not split.
        for (line, query, want) in [
            ("The THE the", "the", vec![0..3, 4..7, 8..11]),
            // é is two bytes, and not an e.
            ("café the Thé", "the", just(6..9)),
            ("e\u{301}e", "e", just(3..4)),
            ("\u{212A}M km KM", "km", vec![0..4, 5..7, 8..10]),
            ("ab", "abc", vec![]),
        ] {
            assert_eq!(line_matches(line, query), want, "{line:?} {query:?}");
        }
    }

    #[test]
    fn find_without_a_match_is_none() {
        assert_eq!(hit("abc\ndef", "zzz", pos(0, 1), true), None);
        assert_eq!(hit("abc\ndef", "zzz", pos(0, 1), false), None);
    }

    // vim_engine_search_test.dart

    const DOC: &str = "hello world\ntest hello\nfinal line";

    #[test]
    fn slash_enters_search_forward() {
        let mut e = ed(DOC);
        keys(&mut e, "/");
        assert_eq!(e.mode(), Mode::Search);
        assert!(e.search().unwrap().forward);
    }

    #[test]
    fn question_mark_enters_search_backward() {
        let mut e = ed(DOC);
        keys(&mut e, "?");
        assert_eq!(e.mode(), Mode::Search);
        assert!(!e.search().unwrap().forward);
    }

    #[test]
    fn typing_in_search_mode_updates_the_query() {
        let mut e = ed(DOC);
        keys(&mut e, "/hel");
        assert_eq!(e.search().unwrap().query, "hel");
        // Committed text types into the query too.
        e.handle(Input::Text("lo w".into()));
        assert_eq!(e.search().unwrap().query, "hello w");
    }

    #[test]
    fn incremental_search_highlights_matches_as_you_type() {
        let mut e = ed(DOC);
        keys(&mut e, "/hello");
        assert_eq!(e.search().unwrap().matches(e.buffer()).len(), 2);
        assert_eq!(e.matches(0), just(0..5));
        assert_eq!(e.matches(1), just(5..10));
        assert!(e.matches(2).is_empty());
    }

    #[test]
    fn every_key_jumps_from_the_origin() {
        let mut e = ed(DOC);
        keys(&mut e, "/w");
        assert_eq!(e.cursor(), pos(0, 6));
        keys(&mut e, "<BS>t");
        assert_eq!(e.cursor(), pos(1, 0));
        keys(&mut e, "es");
        assert_eq!(e.cursor(), pos(1, 0));
        // No match: the cursor goes back to the origin.
        keys(&mut e, "q");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.search().unwrap().current(), None);
    }

    #[test]
    fn enter_confirms_jumps_and_keeps_the_highlights() {
        let mut e = ed(DOC);
        keys(&mut e, "/hello<CR>");
        assert_eq!(e.mode(), Mode::Normal);
        // vim: the first match after the cursor (moon_edit took the one at it).
        assert_eq!(e.cursor(), pos(1, 5));
        assert_eq!(e.matches(0), just(0..5));
        assert_eq!(e.search().unwrap().current(), Some((1, 5..10)));
    }

    #[test]
    fn esc_cancels_restoring_the_cursor_and_the_highlights() {
        let mut e = ed(DOC);
        keys(&mut e, "/hello<Esc>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 0));
        assert!(e.search().is_none());
        assert!(e.matches(0).is_empty());

        // After a search, Esc on the next one brings the first one back.
        keys(&mut e, "/final<CR>");
        assert_eq!(e.cursor(), pos(2, 0));
        keys(&mut e, "/test");
        assert_eq!(e.cursor(), pos(1, 0));
        assert!(e.matches(2).is_empty());
        keys(&mut e, "<Esc>");
        assert_eq!(e.cursor(), pos(2, 0));
        assert_eq!(e.matches(2), just(0..5));
        assert_eq!(e.search().unwrap().query, "final");
    }

    #[test]
    fn esc_restores_the_wanted_column() {
        let mut e = ed("abcdef\nab\nabcdef");
        keys(&mut e, "$j/a<Esc>j");
        assert_eq!(e.cursor(), pos(2, 5));
    }

    #[test]
    fn n_moves_to_the_next_match() {
        let mut e = ed(DOC);
        keys(&mut e, "/hello<CR>");
        assert_eq!(e.cursor(), pos(1, 5));
        keys(&mut e, "n");
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "n");
        assert_eq!(e.cursor(), pos(1, 5));
    }

    #[test]
    fn big_n_moves_to_the_previous_match() {
        let mut e = ed(DOC);
        keys(&mut e, "/hello<CR>N");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn question_mark_searches_backward_and_n_follows_it() {
        let mut e = ed("foo foo foo");
        keys(&mut e, "5l?foo<CR>");
        assert_eq!(e.cursor(), pos(0, 4));
        keys(&mut e, "n");
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "N");
        assert_eq!(e.cursor(), pos(0, 4));
    }

    #[test]
    fn n_searches_from_cursor_not_index() {
        let mut e = ed("foo x\nfoo x\nfoo x\nfoo x\nfoo x");
        keys(&mut e, "/foo<CR>");
        assert_eq!(e.cursor(), pos(1, 0));
        // Past the matches on lines 2 and 3.
        keys(&mut e, "jj$");
        keys(&mut e, "n");
        assert_eq!(e.cursor(), pos(4, 0));
    }

    #[test]
    fn counts_step_over_matches() {
        let mut e = ed("a1 a2 a3 a4 a5");
        keys(&mut e, "/a<CR>");
        assert_eq!(e.cursor(), pos(0, 3));
        keys(&mut e, "0");
        keys(&mut e, "3n");
        assert_eq!(e.cursor(), pos(0, 9));
        keys(&mut e, "2/a<CR>");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn wrap_sets_message() {
        let (mut e, host) = ed_with_host(DOC);
        host.state().now = 5_000;
        keys(&mut e, "/hello<CR>");
        assert_eq!(e.message(), None);
        keys(&mut e, "n");
        assert_eq!(e.message(), Some("search wrapped"));
        // It shows for one second.
        e.handle(Input::Tick {
            now_ns: 5_000 + 999_999_999,
        });
        assert_eq!(e.message(), Some("search wrapped"));
        e.handle(Input::Tick {
            now_ns: 5_000 + 1_000_000_000,
        });
        assert_eq!(e.message(), None);
        // `?` wrapping past the start says so too.
        keys(&mut e, "gg?final<CR>");
        assert_eq!(e.cursor(), pos(2, 0));
        assert_eq!(e.message(), Some("search wrapped"));
    }

    #[test]
    fn no_match_shows_pattern_not_found() {
        let mut e = ed("abc");
        keys(&mut e, "l/zzz<CR>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 1));
        assert_eq!(e.message(), Some("E486: Pattern not found: zzz"));
        // It fades after six seconds, as every message does.
        e.handle(Input::Tick {
            now_ns: crate::status::MESSAGE_FOR_NS - 1,
        });
        assert_eq!(e.message(), Some("E486: Pattern not found: zzz"));
        e.handle(Input::Tick {
            now_ns: crate::status::MESSAGE_FOR_NS,
        });
        assert_eq!(e.message(), None);
        // vim: the failed pattern is still the one `n` looks for.
        keys(&mut e, "n");
        assert_eq!(e.cursor(), pos(0, 1));
    }

    #[test]
    fn backspace_in_search_mode_removes_the_last_char() {
        let mut e = ed(DOC);
        keys(&mut e, "/hello<BS>");
        assert_eq!(e.search().unwrap().query, "hell");
        keys(&mut e, "<Esc>/e\u{301}<BS>");
        assert_eq!(e.search().unwrap().query, "");
    }

    #[test]
    fn backspace_on_empty_query_exits_search() {
        let mut e = ed(DOC);
        keys(&mut e, "/<BS>");
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "/te<BS><BS>");
        assert_eq!(e.mode(), Mode::Search);
        assert_eq!(e.cursor(), pos(0, 0));
        keys(&mut e, "<BS>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn empty_query_repeats_the_last_one_in_the_new_direction() {
        let mut e = ed("xa xb xa xb");
        keys(&mut e, "/xb<CR>0/<CR>");
        assert_eq!(e.cursor(), pos(0, 3));
        keys(&mut e, "?<CR>");
        assert_eq!(e.cursor(), pos(0, 9));
        keys(&mut e, "n");
        assert_eq!(e.cursor(), pos(0, 3));
    }

    #[test]
    fn search_from_visual_mode_extends_the_selection() {
        let mut e = ed(DOC);
        keys(&mut e, "v/test<CR>");
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(e.cursor(), pos(1, 0));
        assert_eq!(e.selection().unwrap().anchor, pos(0, 0));
        // vim: `n` moves the head too, and `d` takes it all.
        let mut e = ed("ab foo cd foo ef");
        keys(&mut e, "v/foo<CR>nd");
        assert_eq!(e.text(), "oo ef");
        let mut e = ed("ab foo cd foo");
        keys(&mut e, "/foo<Esc>");
        e.handle(Input::Text("/foo".into()));
        keys(&mut e, "<CR>0vnd");
        assert_eq!(e.text(), "oo cd foo");
    }

    #[test]
    fn esc_from_a_visual_search_puts_the_head_back() {
        let mut e = ed(DOC);
        keys(&mut e, "vl/final");
        assert_eq!(e.cursor(), pos(2, 0));
        keys(&mut e, "<Esc>");
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(e.cursor(), pos(0, 1));
    }

    #[test]
    fn smartcase_in_the_editor() {
        let mut e = ed("Hello hello");
        keys(&mut e, "/hello");
        assert_eq!(e.matches(0).len(), 2);
        keys(&mut e, "<Esc>/Hello");
        assert_eq!(e.matches(0), just(0..5));
    }

    #[test]
    fn highlights_follow_edits_and_undo() {
        let mut e = ed("foo\nbar");
        keys(&mut e, "/foo<CR>");
        keys(&mut e, "jIfoo <Esc>");
        assert_eq!(e.matches(1), just(0..3));
        keys(&mut e, "u");
        assert!(e.matches(1).is_empty());
        keys(&mut e, "ggdd");
        assert!(e.matches(0).is_empty());
        assert_eq!(e.search().unwrap().matches(e.buffer()), vec![]);
    }

    // n and N under an operator: exclusive, charwise (vim).

    #[test]
    fn dn_deletes_up_to_the_next_match() {
        let mut e = ed("x foo y foo z");
        keys(&mut e, "/foo<CR>0dn");
        assert_eq!(e.text(), "foo y foo z");
        assert_eq!(e.cursor(), pos(0, 0));
        // `.` repeats it with the search as it is now.
        keys(&mut e, ".");
        assert_eq!(e.text(), "foo z");
    }

    #[test]
    fn d_slash_deletes_up_to_the_match_and_repeats() {
        let mut e = ed("x foo y foo z");
        keys(&mut e, "d/foo<CR>");
        assert_eq!(e.text(), "foo y foo z");
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, ".");
        assert_eq!(e.text(), "foo z");
        // One undo step each.
        keys(&mut e, "u");
        assert_eq!(e.text(), "foo y foo z");
    }

    #[test]
    fn dot_after_d_slash_searches_its_own_pattern_again() {
        // vim: `.` replays `d/foo<CR>`, whatever was searched since, and
        // `foo` is the last search again.
        let mut e = ed("x foo y bar z foo w bar");
        keys(&mut e, "0d/foo<CR>");
        assert_eq!(e.text(), "foo y bar z foo w bar");
        keys(&mut e, "0/bar<CR>");
        assert_eq!(e.cursor(), pos(0, 6));
        keys(&mut e, "0.");
        assert_eq!(e.text(), "foo w bar");
        let s = e.search().unwrap();
        assert_eq!((s.query.as_str(), s.forward), ("foo", true));
        assert_eq!(e.matches(0), just(0..3));
    }

    #[test]
    fn dot_after_d_question_mark_keeps_its_direction() {
        let mut e = ed("w bar foo z bar y foo x");
        keys(&mut e, "$d?foo<CR>");
        assert_eq!(e.text(), "w bar foo z bar y x");
        assert_eq!(e.cursor(), pos(0, 18));
        keys(&mut e, "$/bar<CR>");
        assert_eq!(e.cursor(), pos(0, 2));
        keys(&mut e, "$.");
        assert_eq!(e.text(), "w bar x");
        let s = e.search().unwrap();
        assert_eq!((s.query.as_str(), s.forward), ("foo", false));
    }

    #[test]
    fn dot_after_dn_uses_the_search_as_it_is_now() {
        // vim: the redo of `dn` is `n`, with whatever the last search is.
        let mut e = ed("x foo y bar z foo w bar");
        keys(&mut e, "/foo<CR>0dn");
        assert_eq!(e.text(), "foo y bar z foo w bar");
        keys(&mut e, "0/bar<CR>0.");
        assert_eq!(e.text(), "bar z foo w bar");
        assert_eq!(e.search().unwrap().query, "bar");
    }

    #[test]
    fn d_slash_esc_and_no_match_change_nothing() {
        let mut e = ed("x foo y");
        keys(&mut e, "d/foo<Esc>");
        assert_eq!(e.text(), "x foo y");
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "d/zzz<CR>");
        assert_eq!(e.text(), "x foo y");
        assert_eq!(e.message(), Some("E486: Pattern not found: zzz"));
        assert_eq!(e.last_change(), None);
    }

    #[test]
    fn big_dn_deletes_back_to_the_match() {
        let mut e = ed("foo bar baz");
        keys(&mut e, "/foo<CR>8ldN");
        assert_eq!(e.text(), "baz");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn cn_changes_up_to_the_match() {
        let mut e = ed("x foo y foo z");
        keys(&mut e, "/foo<CR>0cnQQ<Esc>");
        assert_eq!(e.text(), "QQfoo y foo z");
        assert_eq!(e.cursor(), pos(0, 1));
    }

    #[test]
    fn dn_to_the_start_of_a_later_line_follows_the_exclusive_rules() {
        // From the line start the range becomes linewise.
        let mut e = ed("ab\nfoo cd");
        keys(&mut e, "/foo<CR>ggdn");
        assert_eq!(e.text(), "foo cd");
        let mut e = ed("  ab\nfoo");
        keys(&mut e, "/foo<CR>gg0ldn");
        assert_eq!(e.text(), "foo");
        // From mid-line it stops at the line end.
        let mut e = ed("ab cd\nfoo cd");
        keys(&mut e, "/foo<CR>gg3ldn");
        assert_eq!(e.text(), "ab \nfoo cd");
    }

    #[test]
    fn dn_without_a_match_or_onto_itself_does_nothing() {
        let mut e = ed("abc");
        keys(&mut e, "/zzz<CR>dn");
        assert_eq!(e.text(), "abc");
        assert_eq!(e.last_change(), None);
        let mut e = ed("foo abc");
        keys(&mut e, "/foo<CR>dn");
        assert_eq!(e.text(), "foo abc");
        assert_eq!(e.last_change(), None);
        // With no search yet, n is nothing too.
        let mut e = ed("abc");
        keys(&mut e, "ndn");
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn dn_records_a_search_change() {
        let mut e = ed("x foo y foo");
        keys(&mut e, "/foo<CR>02dn");
        assert_eq!(
            e.last_change(),
            Some(&Change::Search {
                op: Operator::Delete,
                reverse: false,
                count: 2,
                pattern: None,
                inserted: None,
            })
        );
        assert_eq!(e.text(), "foo");
        // `d?foo` records what it searched for.
        let mut e = ed("foo x foo");
        keys(&mut e, "$d?foo<CR>");
        assert_eq!(
            e.last_change(),
            Some(&Change::Search {
                op: Operator::Delete,
                reverse: false,
                count: 1,
                pattern: Some(("foo".into(), false)),
                inserted: None,
            })
        );
    }

    #[test]
    fn yn_yanks_up_to_the_match_and_stays() {
        let mut e = ed("x foo y");
        keys(&mut e, "/foo<CR>0yn");
        assert_eq!(e.register(None).unwrap().text, "x ");
        assert_eq!(e.cursor(), pos(0, 0));
        assert_eq!(e.text(), "x foo y");
    }

    #[test]
    fn ctrl_c_on_the_search_line_acts_as_esc() {
        let mut e = ed(DOC);
        keys(&mut e, "/final<C-c>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn typing_a_query_repaints_everything() {
        let mut e = ed(DOC);
        assert_eq!(keys(&mut e, "/").redraw, Redraw::Status);
        let fx = keys(&mut e, "f");
        assert_eq!(fx.redraw, Redraw::All);
        assert!(fx.scroll_to_cursor);
        assert_eq!(keys(&mut e, "<CR>").redraw, Redraw::All);
    }

    #[test]
    fn the_cursor_lands_on_grapheme_boundaries() {
        let mut e = ed("xe\u{301} e");
        keys(&mut e, "/e<CR>");
        assert_eq!(e.cursor(), pos(0, 5));
        let mut e = ed("ÉCOLE école");
        keys(&mut e, "/école<CR>");
        assert_eq!(e.cursor(), pos(0, 7));
        keys(&mut e, "n");
        assert_eq!(e.cursor(), pos(0, 0));
    }
}

/// Timing harness for the per-key cost of Search mode, not run in CI:
/// `cargo test -p editor-core --release --lib search::timing -- --ignored --nocapture`
#[cfg(test)]
mod timing {
    use std::time::Instant;

    use super::find;
    use crate::buffer::Buffer;
    use crate::editor::Editor;
    use crate::input::{Input, Key, KeyEvent, Mods};
    use crate::testkit::*;

    /// 10,000 lines in a 4-line cycle: ASCII with `café`, a French task
    /// line with a ZWJ emoji, a decomposed accent, plain ASCII.
    fn doc() -> String {
        let mut text = String::new();
        for i in 0..10_000 {
            match i % 4 {
                0 => text.push_str(&format!(
                    "Line {i}: the quick brown fox jumps over the lazy dog, café\n"
                )),
                1 => text.push_str(&format!(
                    "- [ ] tâche numéro {i} pour l'école, naïve façade 👩\u{200d}👩\u{200d}👧\n"
                )),
                2 => text.push_str(&format!(
                    "**bold** and `code` with e\u{301}cole décomposée {i}\n"
                )),
                _ => text.push_str("plain ascii text line with a few words in it and some more\n"),
            }
        }
        text
    }

    fn press(e: &mut Editor, key: Key) {
        e.handle(Input::Key(KeyEvent {
            key,
            mods: Mods::default(),
            time_ns: 0,
        }));
    }

    /// One key, plus the highlights a renderer asks for: 60 lines from
    /// the cursor's. Milliseconds.
    fn timed(e: &mut Editor, key: Key) -> f64 {
        let t = Instant::now();
        press(e, key);
        let top = e.cursor().line.saturating_sub(30);
        let mut n = 0;
        for i in top..(top + 60).min(e.line_count()) {
            n += e.matches(i).len();
        }
        std::hint::black_box(n);
        t.elapsed().as_secs_f64() * 1000.0
    }

    fn summary(label: &str, mut times: Vec<f64>) {
        let all = format!("{times:.3?}");
        times.sort_by(f64::total_cmp);
        println!(
            "{label}: median {:.3} max {:.3} per key {all}",
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }

    #[test]
    #[ignore = "timing harness, run by hand in release"]
    fn search_keys_and_n() {
        let text = doc();
        // From the middle of the document, so a hit can be either side.
        for query in [
            "e",
            "lazy d",
            "école",
            "Quick",
            "numéro 9",
            "zzzzzz",
            "elephant",
            "the cat",
            "résumé",
            "wombat",
            "Elephant",
            "ezzzz",
            "éxyz",
            "👩zz",
            "👩",
        ] {
            let mut e = ed(&text);
            keys(&mut e, "5000G");
            press(&mut e, Key::Char('/'));
            let times = query
                .chars()
                .map(|c| timed(&mut e, if c == ' ' { Key::Space } else { Key::Char(c) }))
                .collect();
            summary(&format!("type {query:?}"), times);
        }
        for (query, label) in [
            ("fox", "n, common match"),
            ("numéro 5001", "n, one match (wraps the document)"),
            (
                "école, naïve façade 👩\u{200d}👩\u{200d}👧",
                "n, every 4th line, long query beyond ASCII",
            ),
            ("ÉCOLE", "n, case sensitive, no match"),
        ] {
            let mut e = ed(&text);
            keys(&mut e, "5000G");
            keys(&mut e, &format!("/{query}<CR>"));
            let times = (0..10).map(|_| timed(&mut e, Key::Char('n'))).collect();
            summary(&format!("n {query:?} ({label})"), times);
            let times = (0..10).map(|_| timed(&mut e, Key::Char('N'))).collect();
            summary(&format!("N {query:?}"), times);
        }
        let mut e = ed(&text);
        keys(&mut e, "5000G/fox<CR>");
        let times = (0..5)
            .map(|_| {
                let t = Instant::now();
                keys(&mut e, "1000n");
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        summary("1000n \"fox\"", times);
    }

    #[test]
    #[ignore = "timing harness, run by hand in release"]
    fn counts_on_a_long_line() {
        // One 60 KB line with 20,000 matches.
        let buf = Buffer::from_text(&"ab ".repeat(20_000));
        for count in [1, 100, 1000, 30_000, 1_000_000_000] {
            let t = Instant::now();
            let hit = find(&buf, "a", pos(0, 0), true, count).map(|h| h.pos());
            println!(
                "count {count} on a long line: {:.3} ms, at {hit:?}",
                t.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}

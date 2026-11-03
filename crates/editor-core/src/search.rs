//! Search: a literal query with smartcase, its matches per line for the
//! highlights, and stepping from a position to the next match (`/ ? n N`).
//!
//! Matches on a line are found as vim finds them: from the line start, each
//! match after the end of the one before, so they never overlap. A match
//! whose start or end would split a grapheme cluster is no match (vim does
//! not match `e` in a decomposed `é`), so every match is a range of whole
//! graphemes and a cursor put on its start rests on a grapheme boundary
//! (R12).

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{Buffer, Pos};
use crate::vim::Cursor;
use crate::vim::operator::Touched;

/// How long `search wrapped` shows: one second.
pub const WRAPPED_FOR_NS: u64 = 1_000_000_000;

/// A search: what is searched for, which way, and where it started, with
/// the matches on each line for the highlights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    pub query: String,
    /// True for `/`, false for `?`. `n` goes this way, `N` the other.
    pub forward: bool,
    /// The cursor when the search started, where Esc puts it back.
    pub origin: Cursor,
    /// The matches on each line, as `matches` gives them.
    lines: Vec<Vec<Range<usize>>>,
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
        (0..buf.line_count())
            .flat_map(|i| {
                line_matches(buf.line(i), &self.query)
                    .into_iter()
                    .map(move |r| (i, r))
            })
            .collect()
    }

    /// The matches on `line` when the search was last brought up to date
    /// (`refresh`, `edited`), for `ViewSource::matches`.
    pub fn matches_on(&self, line: usize) -> &[Range<usize>] {
        self.lines.get(line).map_or(&[], Vec::as_slice)
    }

    /// The match the search last jumped to.
    pub fn current(&self) -> Option<(usize, Range<usize>)> {
        self.current.clone()
    }

    /// Records the match the search jumped to (`None`: it found none).
    pub fn set_current(&mut self, hit: Option<&Hit>) {
        self.current = hit.map(|h| (h.line, h.range.clone()));
    }

    /// Works out the matches on every line afresh.
    pub fn refresh(&mut self, buf: &Buffer) {
        self.current = None;
        self.lines = if self.query.is_empty() {
            Vec::new()
        } else {
            (0..buf.line_count())
                .map(|i| line_matches(buf.line(i), &self.query))
                .collect()
        };
    }

    /// Brings the matches up to date after the one edit `t`: only the
    /// lines it replaced are searched again.
    pub fn edited(&mut self, buf: &Buffer, t: &Touched) {
        self.current = None;
        if self.query.is_empty() {
            return;
        }
        let old_end = (t.old_end_line + 1).min(self.lines.len());
        let start = t.start_line.min(old_end);
        let new_end = (t.new_end_line + 1).min(buf.line_count());
        let fresh: Vec<_> = (start..new_end)
            .map(|i| line_matches(buf.line(i), &self.query))
            .collect();
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
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let sensitive = case_sensitive(query);
    // On an ASCII line every byte starts a grapheme.
    let bounds: Option<Vec<usize>> = (!line.is_ascii()).then(|| {
        line.grapheme_indices(true)
            .map(|(i, _)| i)
            .chain([line.len()])
            .collect()
    });
    let on_boundary = |i: usize| bounds.as_ref().is_none_or(|b| b.binary_search(&i).is_ok());
    let mut at = 0;
    while at < line.len() {
        let Some(r) = next_match(line, at, query, sensitive) else {
            break;
        };
        if on_boundary(r.start) && on_boundary(r.end) {
            at = r.end;
            out.push(r);
        } else {
            // It splits a grapheme: look again from the next char.
            at = r.start + line[r.start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// The first match of `query` in `line` starting at or after `at` (a char
/// boundary).
fn next_match(line: &str, at: usize, query: &str, sensitive: bool) -> Option<Range<usize>> {
    let rest = &line[at..];
    if sensitive {
        return rest.find(query).map(|o| at + o..at + o + query.len());
    }
    rest.char_indices()
        .find_map(|(o, _)| folded_len(&rest[o..], query).map(|n| at + o..at + o + n))
}

/// The length in bytes of the start of `text` that matches `query`, case
/// ignored char by char, or `None` when it does not match.
fn folded_len(text: &str, query: &str) -> Option<usize> {
    let mut chars = text.chars();
    let mut len = 0;
    for q in query.chars() {
        let c = chars.next()?;
        if c != q && !c.to_lowercase().eq(q.to_lowercase()) {
            return None;
        }
        len += c.len_utf8();
    }
    Some(len)
}

/// The `count`th match of `query` from `from` (0 counts as 1), going
/// forward or backward and wrapping round the buffer. A match at `from`
/// itself is not counted. `None` when there is no match at all.
pub fn find(buf: &Buffer, query: &str, from: Pos, forward: bool, count: usize) -> Option<Hit> {
    if query.is_empty() {
        return None;
    }
    let from = buf.clamp(from);
    let first = first_hit(buf, query, from, forward)?;
    if count <= 1 {
        return Some(first);
    }
    // Step over the rest through the list of every match, so a huge count
    // costs one pass over the buffer.
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
    let steps = count - 1;
    let (j, wrapped) = if forward {
        ((i + steps % n) % n, steps >= n - i)
    } else {
        ((i + n - steps % n) % n, steps > i)
    };
    let (line, range) = all[j].clone();
    Some(Hit {
        line,
        range,
        wrapped: first.wrapped || wrapped,
    })
}

/// The nearest match after `from` (forward) or before it, wrapping.
fn first_hit(buf: &Buffer, query: &str, from: Pos, forward: bool) -> Option<Hit> {
    let n = buf.line_count();
    let here = line_matches(buf.line(from.line), query);
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
        let ms = line_matches(buf.line(line), query);
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
        assert_eq!(
            find(&buf, "a", pos(0, 0), false, 2).map(|h| h.pos()),
            Some(pos(0, 9))
        );
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
        // The message does not time out (Task 26 fades it).
        e.handle(Input::Tick { now_ns: u64::MAX });
        assert_eq!(e.message(), Some("E486: Pattern not found: zzz"));
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
                inserted: None,
            })
        );
        assert_eq!(e.text(), "foo");
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

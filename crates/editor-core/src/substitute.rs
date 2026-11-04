//! `:s`: literal matching and replacing over a range of lines, all at once
//! (`run`) or asking at each match (`ConfirmSession`).
//!
//! Matching is `search`'s literal matcher, case sensitive unless the `i`
//! flag is given (not smartcase), so no match splits a grapheme (R12).
//! Matches are found in each line as it was before the command, as vim
//! does, so a replacement is never matched again. Without `g` only a
//! line's first match counts.
//!
//! The range is resolved first (`lines`), because `'<,'>` needs the
//! selection the editor remembers and a bad range is an error to show;
//! `run` and `confirm_session` take the resolved lines rather than the
//! cursor's line.

use std::ops::{Range, RangeInclusive};

use crate::buffer::{Buffer, Edit, Pos};
use crate::ex::{Sub, SubRange};
use crate::search;
use crate::strings;
use crate::vim::motion::caret_col;

/// What `run` found: the edits to make (none for `n`), how many matches
/// they replace and on how many lines.
///
/// There is one edit per line, top to bottom, from the line's first
/// replaced match to the end of its last. None changes the line count, so
/// each applies as given, in any order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubResult {
    pub edits: Vec<Edit>,
    pub count: usize,
    pub lines: usize,
}

/// The lines `range` covers in a buffer of `line_count` lines, with the
/// cursor on `cursor_line` and `visual` the first and last lines of the
/// selection `:` left Visual with, in either order.
///
/// A range past the end is `E16: Invalid range` and `'<,'>` with no
/// selection `E20: Mark not set`. A backwards range is swapped: vim asks
/// first, and with no prompt for it here it does what answering `y` does.
pub fn lines(
    range: SubRange,
    cursor_line: usize,
    visual: Option<(usize, usize)>,
    line_count: usize,
) -> Result<RangeInclusive<usize>, String> {
    let (a, b) = match range {
        SubRange::Line => (cursor_line, cursor_line),
        SubRange::All => (0, line_count - 1),
        SubRange::Visual => visual.ok_or_else(|| strings::MARK_NOT_SET.to_owned())?,
        SubRange::Lines(a, b) => (a, b),
    };
    let (a, b) = (a.min(b), a.max(b));
    if b >= line_count {
        return Err(strings::INVALID_RANGE.to_owned());
    }
    Ok(a..=b)
}

/// The matches `sub` replaces on `line`: all of them with `g`, else the
/// first.
fn line_matches(line: &str, sub: &Sub) -> Vec<Range<usize>> {
    let mut ms = search::literal_matches(line, &sub.pattern, sub.flags.i);
    if !sub.flags.g {
        ms.truncate(1);
    }
    ms
}

/// `replacement` with each `$&` turned into `matched`.
fn expand(replacement: &str, matched: &str) -> String {
    replacement.replace("$&", matched)
}

/// Every replacement `sub` makes on `lines` of `buf`, or with `n` only the
/// count.
pub fn run(buf: &Buffer, sub: &Sub, lines: RangeInclusive<usize>) -> SubResult {
    let mut out = SubResult::default();
    for l in lines {
        let text = buf.line(l);
        let ms = line_matches(text, sub);
        let (Some(first), Some(last)) = (ms.first(), ms.last()) else {
            continue;
        };
        out.count += ms.len();
        out.lines += 1;
        if sub.flags.n {
            continue;
        }
        let mut new = String::new();
        let mut at = first.start;
        for m in &ms {
            new.push_str(&text[at..m.start]);
            new.push_str(&expand(&sub.replacement, &text[m.clone()]));
            at = m.end;
        }
        out.edits.push(Edit {
            range: Pos {
                line: l,
                col: first.start,
            }..Pos {
                line: l,
                col: last.end,
            },
            text: new,
        });
    }
    out
}

/// An answer to `replace with <r>? [y/n/a/q/l]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Replace this match and go on.
    Yes,
    /// Skip it and go on.
    No,
    /// Replace this one and all the rest.
    All,
    /// Stop (Esc too).
    Quit,
    /// Replace this one and stop.
    Last,
}

impl Answer {
    /// The answer a key gives: `y n a q l`, in either case.
    pub fn from_key(c: char) -> Option<Answer> {
        Some(match c.to_ascii_lowercase() {
            'y' => Answer::Yes,
            'n' => Answer::No,
            'a' => Answer::All,
            'q' => Answer::Quit,
            'l' => Answer::Last,
            _ => return None,
        })
    }
}

/// One match a session asks about.
#[derive(Debug, Clone)]
struct Match {
    line: usize,
    /// Where it is in the buffer as it stands, after the replacements so
    /// far.
    range: Range<usize>,
    /// The text it matched, for `$&`.
    text: String,
}

/// Where the cursor ends when a session does, as vim 9.1 leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// On the match last asked about (after `n`, `q` or Esc).
    At(Pos),
    /// At the start of the line just replaced on (after `y` or `l`).
    LineStart(usize),
    /// On the first non-blank of the last line replaced on (after `a`,
    /// which stops asking, so the session ends as a plain `:s` does).
    FirstNonBlank(usize),
}

/// A `:s///c` in progress: the matches, the one being asked about, and
/// what has been replaced. The editor shows `prompt` with the cursor on
/// `current`, passes each answer to `answer` and makes the edits it gives,
/// in order, before the next.
#[derive(Debug, Clone)]
pub struct ConfirmSession {
    replacement: String,
    matches: Vec<Match>,
    /// The match being asked about; `matches.len()` once done.
    at: usize,
    count: usize,
    lines: usize,
    last_line: Option<usize>,
    /// The first line replaced on, and the cursor vim's undo goes back to
    /// for the session: see `undo_cursor`.
    first_line: Option<usize>,
    undo_cursor: Option<Pos>,
    end: Option<End>,
}

/// A session over the matches `sub` has on `lines` of `buf`. With none it
/// is done from the start.
pub fn confirm_session(buf: &Buffer, sub: &Sub, lines: RangeInclusive<usize>) -> ConfirmSession {
    let matches = lines
        .flat_map(|line| {
            let text = buf.line(line);
            line_matches(text, sub).into_iter().map(move |range| Match {
                line,
                text: text[range.clone()].to_owned(),
                range,
            })
        })
        .collect();
    ConfirmSession {
        replacement: sub.replacement.clone(),
        matches,
        at: 0,
        count: 0,
        lines: 0,
        last_line: None,
        first_line: None,
        undo_cursor: None,
        end: None,
    }
}

impl ConfirmSession {
    /// The match being asked about: its line and range in the buffer as it
    /// stands.
    pub fn current(&self) -> Option<(usize, Range<usize>)> {
        self.matches.get(self.at).map(|m| (m.line, m.range.clone()))
    }

    /// `replace with <r>? [y/n/a/q/l]`, with the replacement as typed.
    pub fn prompt(&self) -> String {
        strings::confirm_replace(&self.replacement)
    }

    /// Takes the answer to the current match and returns the edits it
    /// makes, to be applied in order before anything else changes the
    /// buffer.
    pub fn answer(&mut self, a: Answer) -> Vec<Edit> {
        let Some(m) = self.matches.get(self.at) else {
            return Vec::new();
        };
        let asked = Pos {
            line: m.line,
            col: m.range.start,
        };
        let mut edits = Vec::new();
        match a {
            Answer::Yes | Answer::Last => {
                edits.push(self.replace());
                self.end = Some(End::LineStart(asked.line));
                if a == Answer::Last {
                    self.at = self.matches.len();
                }
            }
            Answer::No => {
                self.asked_on_first_line(asked);
                self.at += 1;
                self.end = Some(End::At(asked));
            }
            Answer::Quit => {
                self.asked_on_first_line(asked);
                self.at = self.matches.len();
                self.end = Some(End::At(asked));
            }
            Answer::All => {
                while self.at < self.matches.len() {
                    edits.push(self.replace());
                }
                self.end = self.last_line.map(End::FirstNonBlank);
            }
        }
        edits
    }

    /// Replaces the current match and moves to the next: the edit, with
    /// the matches after it on its line moved by the change in length.
    fn replace(&mut self) -> Edit {
        let m = &self.matches[self.at];
        let line = m.line;
        let text = expand(&self.replacement, &m.text);
        let edit = Edit {
            range: Pos {
                line,
                col: m.range.start,
            }..Pos {
                line,
                col: m.range.end,
            },
            text,
        };
        let (old, new) = (m.range.len(), edit.text.len());
        for later in self.matches[self.at + 1..]
            .iter_mut()
            .take_while(|l| l.line == line)
        {
            later.range = later.range.start + new - old..later.range.end + new - old;
        }
        self.count += 1;
        if self.last_line != Some(line) {
            self.lines += 1;
            self.last_line = Some(line);
        }
        let first = *self.first_line.get_or_insert(line);
        if first == line {
            self.undo_cursor = Some(Pos { line, col: 0 });
        }
        self.at += 1;
        edit
    }

    /// Notes a match skipped or quit at, for `undo_cursor`.
    fn asked_on_first_line(&mut self, asked: Pos) {
        if self.first_line == Some(asked.line) {
            self.undo_cursor = Some(asked);
        }
    }

    /// True once every match is answered or the session was quit.
    pub fn done(&self) -> bool {
        self.at >= self.matches.len()
    }

    /// How many matches were replaced.
    pub fn count(&self) -> usize {
        self.count
    }

    /// On how many lines.
    pub fn lines(&self) -> usize {
        self.lines
    }

    /// Where the cursor goes once the session is done, in `buf` after its
    /// edits.
    pub fn end_cursor(&self, buf: &Buffer) -> Pos {
        match self.end {
            Some(End::At(p)) => p,
            Some(End::LineStart(line)) => Pos { line, col: 0 },
            Some(End::FirstNonBlank(line)) => Pos {
                line,
                col: caret_col(buf.line(line)),
            },
            None => self
                .current()
                .map_or(Pos::default(), |(line, r)| Pos { line, col: r.start }),
        }
    }

    /// Where undoing the session puts the cursor, as vim 9.1 does: vim
    /// writes each line's replacements when it leaves the line, and the
    /// undo step keeps the cursor from the first of those writes. That is
    /// column 0 of the first line replaced on when its last answer
    /// replaced, else the match last skipped (or quit at) there. `None`
    /// when nothing was replaced.
    pub fn undo_cursor(&self) -> Option<Pos> {
        self.undo_cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::ex::SubFlags;
    use crate::testkit::*;
    use crate::vim::Mode;

    fn flags(s: &str) -> SubFlags {
        SubFlags {
            g: s.contains('g'),
            i: s.contains('i'),
            c: s.contains('c'),
            n: s.contains('n'),
        }
    }

    fn sub(pattern: &str, replacement: &str, f: &str) -> Sub {
        Sub {
            range: SubRange::All,
            pattern: pattern.to_owned(),
            replacement: replacement.to_owned(),
            flags: flags(f),
        }
    }

    /// Runs `sub` over `range` of `text` and returns the text after and
    /// the result.
    fn apply(text: &str, s: &Sub, range: RangeInclusive<usize>) -> (String, SubResult) {
        let mut buf = Buffer::from_text(text);
        let r = run(&buf, s, range);
        for e in &r.edits {
            buf.replace(e.range.clone(), &e.text);
        }
        (buf.to_text(), r)
    }

    // substitute_engine_test.dart: SubstituteRange.

    #[test]
    fn current_line_range_is_the_cursor_line() {
        assert_eq!(lines(SubRange::Line, 5, None, 10), Ok(5..=5));
    }

    #[test]
    fn whole_document_range() {
        assert_eq!(lines(SubRange::All, 5, None, 10), Ok(0..=9));
    }

    #[test]
    fn line_range_is_as_given() {
        assert_eq!(lines(SubRange::Lines(3, 7), 0, None, 10), Ok(3..=7));
    }

    #[test]
    fn visual_range_is_the_selection_in_order() {
        assert_eq!(lines(SubRange::Visual, 0, Some((5, 2)), 10), Ok(2..=5));
        assert_eq!(
            lines(SubRange::Visual, 0, None, 10),
            Err("E20: Mark not set".to_owned())
        );
    }

    #[test]
    fn a_range_past_the_end_is_e16() {
        // vim 9.1: `:1,9s` on three lines is E16.
        assert_eq!(
            lines(SubRange::Lines(0, 8), 0, None, 3),
            Err("E16: Invalid range".to_owned())
        );
    }

    #[test]
    fn a_backwards_range_is_swapped() {
        // vim asks `Backwards range given, OK to swap (y/n)?`; with no
        // prompt here it swaps, as answering y does.
        assert_eq!(lines(SubRange::Lines(2, 1), 0, None, 3), Ok(1..=2));
    }

    // substitute_engine_test.dart: SubstituteEngine.findMatches, as counts.

    #[test]
    fn finds_literal_matches_in_range() {
        let (text, r) = apply("foo bar foo\nbaz foo\nqux", &sub("foo", "x", "gn"), 0..=2);
        assert_eq!(text, "foo bar foo\nbaz foo\nqux");
        assert_eq!((r.count, r.lines), (3, 2));
        assert!(r.edits.is_empty());
    }

    #[test]
    fn respects_the_line_range_when_counting() {
        let (_, r) = apply("foo\nfoo\nfoo\nfoo", &sub("foo", "x", "gn"), 1..=2);
        assert_eq!((r.count, r.lines), (2, 2));
    }

    #[test]
    fn case_sensitive_without_i() {
        let (text, r) = apply("FOO foo Foo", &sub("foo", "x", "g"), 0..=0);
        assert_eq!(text, "FOO x Foo");
        assert_eq!(r.count, 1);
        // Not smartcase: a pattern without capitals is still exact.
        let (text, _) = apply("FOO foo Foo", &sub("Foo", "x", "g"), 0..=0);
        assert_eq!(text, "FOO foo x");
    }

    #[test]
    fn case_insensitive_with_i() {
        let (text, r) = apply("FOO foo Foo", &sub("foo", "x", "gi"), 0..=0);
        assert_eq!(text, "x x x");
        assert_eq!(r.count, 3);
        // A capital K matches the Kelvin sign, as k does.
        let (text, _) = apply("foo \u{212A}ey", &sub("FOO KEY", "x", "gi"), 0..=0);
        assert_eq!(text, "x");
    }

    // substitute_engine_test.dart: SubstituteEngine.replaceAll.

    #[test]
    fn replaces_all_matches_with_g() {
        let (text, r) = apply("foo bar foo", &sub("foo", "baz", "g"), 0..=0);
        assert_eq!(text, "baz bar baz");
        assert_eq!((r.count, r.lines), (2, 1));
    }

    #[test]
    fn replaces_only_the_first_match_per_line_without_g() {
        let (text, r) = apply("foo bar foo\nfoo foo", &sub("foo", "baz", ""), 0..=1);
        assert_eq!(text, "baz bar foo\nbaz foo");
        assert_eq!((r.count, r.lines), (2, 2));
    }

    #[test]
    fn dollar_ampersand_is_the_match() {
        let (text, _) = apply("Foo foo", &sub("foo", "[$&]", "gi"), 0..=0);
        assert_eq!(text, "[Foo] [foo]");
        // Anything else is literal, `&` and `$1` included.
        let (text, _) = apply("foo", &sub("foo", r"&\0$1", ""), 0..=0);
        assert_eq!(text, r"&\0$1");
    }

    #[test]
    fn replaces_only_in_the_line_range() {
        let (text, _) = apply("foo\nfoo\nfoo", &sub("foo", "bar", "g"), 1..=1);
        assert_eq!(text, "foo\nbar\nfoo");
    }

    #[test]
    fn a_replacement_never_matches_again() {
        let (text, r) = apply("aa", &sub("a", "aa", "g"), 0..=0);
        assert_eq!(text, "aaaa");
        assert_eq!(r.count, 2);
    }

    #[test]
    fn edits_are_one_per_line_in_order() {
        let (_, r) = apply("a foo b foo c\nfoo", &sub("foo", "X", "g"), 0..=1);
        let ranges: Vec<_> = r.edits.iter().map(|e| e.range.clone()).collect();
        assert_eq!(ranges, [pos(0, 2)..pos(0, 11), pos(1, 0)..pos(1, 3)]);
        assert_eq!(r.edits[0].text, "X b X");
    }

    #[test]
    fn matches_never_split_a_grapheme() {
        // R12: the `e` of a decomposed `é` is no match.
        let (text, r) = apply("e\u{301} e", &sub("e", "X", "g"), 0..=0);
        assert_eq!(text, "e\u{301} X");
        assert_eq!(r.count, 1);
    }

    // ConfirmSession on its own.

    #[test]
    fn answers_from_keys_in_either_case() {
        for (c, a) in [
            ('y', Answer::Yes),
            ('N', Answer::No),
            ('a', Answer::All),
            ('Q', Answer::Quit),
            ('l', Answer::Last),
        ] {
            assert_eq!(Answer::from_key(c), Some(a));
        }
        assert_eq!(Answer::from_key('z'), None);
    }

    #[test]
    fn the_session_walks_the_matches_adjusting_for_replacements() {
        let mut buf = Buffer::from_text("foo bar foo");
        let mut s = confirm_session(&buf, &sub("foo", "bazz", "gc"), 0..=0);
        assert_eq!(s.prompt(), "replace with bazz? [y/n/a/q/l]");
        assert_eq!(s.current(), Some((0, 0..3)));
        for e in s.answer(Answer::Yes) {
            buf.replace(e.range, &e.text);
        }
        assert_eq!(buf.line(0), "bazz bar foo");
        assert_eq!(s.current(), Some((0, 9..12)));
        assert!(!s.done());
        assert!(s.answer(Answer::No).is_empty());
        assert!(s.done());
        assert_eq!((s.count(), s.lines()), (1, 1));
    }

    // vim_engine_test.dart: confirm mode (:s///c).

    fn confirm(text: &str, cmd: &str) -> Editor {
        let mut e = ed(text);
        keys(&mut e, &format!(":{cmd}<CR>"));
        e
    }

    #[test]
    fn c_flag_enters_confirm_mode() {
        let e = confirm("foo bar foo", "s/foo/baz/gc");
        assert_eq!(e.mode(), Mode::Confirm);
        assert_eq!(e.prompt().as_deref(), Some("replace with baz? [y/n/a/q/l]"));
        assert_eq!(e.cursor(), pos(0, 0));
        let current = e.search().and_then(|s| s.current());
        assert_eq!(current, Some((0, 0..3)));
    }

    #[test]
    fn y_replaces_and_moves_to_the_next() {
        let mut e = confirm("foo bar foo", "s/foo/baz/gc");
        keys(&mut e, "y");
        assert_eq!(e.text(), "baz bar foo");
        assert_eq!(e.mode(), Mode::Confirm);
        assert_eq!(e.cursor(), pos(0, 8));
        let current = e.search().and_then(|s| s.current());
        assert_eq!(current, Some((0, 8..11)));
    }

    #[test]
    fn n_skips_the_match() {
        let mut e = confirm("foo bar foo", "s/foo/baz/gc");
        keys(&mut e, "n");
        assert_eq!(e.text(), "foo bar foo");
        assert_eq!(e.mode(), Mode::Confirm);
        assert_eq!(e.cursor(), pos(0, 8));
    }

    #[test]
    fn a_replaces_all_the_rest() {
        let mut e = confirm("foo bar foo baz foo", "s/foo/XXX/gc");
        keys(&mut e, "na");
        assert_eq!(e.text(), "foo bar XXX baz XXX");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.message(), Some("2 substitutions on 1 line"));
        assert_eq!(e.prompt(), None);
    }

    #[test]
    fn q_quits_without_replacing_more() {
        let mut e = confirm("foo bar foo", "s/foo/baz/gc");
        keys(&mut e, "yq");
        assert_eq!(e.text(), "baz bar foo");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.message(), Some("1 substitution on 1 line"));
    }

    #[test]
    fn l_replaces_this_one_and_quits() {
        let mut e = confirm("foo bar foo baz foo", "s/foo/XXX/gc");
        keys(&mut e, "nl");
        assert_eq!(e.text(), "foo bar XXX baz foo");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.message(), Some("1 substitution on 1 line"));
    }

    #[test]
    fn escape_quits_without_replacing() {
        let mut e = confirm("foo bar foo", "s/foo/baz/gc");
        keys(&mut e, "<Esc>");
        assert_eq!(e.text(), "foo bar foo");
        assert_eq!(e.mode(), Mode::Normal);
        // vim: nothing replaced, nothing said.
        assert_eq!(e.message(), None);
        assert!(!e.history().can_undo());
    }

    #[test]
    fn completes_when_every_match_is_answered() {
        let mut e = confirm("foo", "s/foo/bar/gc");
        keys(&mut e, "y");
        assert_eq!(e.text(), "bar");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.message(), Some("1 substitution on 1 line"));
    }

    #[test]
    fn answers_take_either_case_and_other_keys_wait() {
        let mut e = confirm("foo foo foo", "s/foo/x/gc");
        keys(&mut e, "zY");
        assert_eq!(e.text(), "x foo foo");
        keys(&mut e, "N<Space>L");
        assert_eq!(e.text(), "x foo x");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn confirm_without_a_match_says_so() {
        let e = confirm("foo", "s/xyz/bar/gc");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.message(), Some("E486: Pattern not found: xyz"));
    }

    #[test]
    fn n_takes_priority_over_c() {
        // vim 9.1: counts, leaves the cursor on its line's first non-blank.
        let mut e = ed(GOLD);
        e_at(&mut e, 1, 1);
        keys(&mut e, ":%s/foo/X/gcn<CR>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.text(), GOLD);
        assert_eq!(e.message(), Some("3 matches on 2 lines"));
        assert_eq!(e.cursor(), pos(1, 0));
    }

    // Undo: every `:s` is one step.

    #[test]
    fn substitute_is_one_undo_step() {
        let mut e = ed("foo\nfoo foo\nbar foo");
        keys(&mut e, "x:%s/o/0/g<CR>");
        assert_eq!(e.text(), "00\nf00 f00\nbar f00");
        keys(&mut e, "u");
        assert_eq!(e.text(), "oo\nfoo foo\nbar foo");
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "00\nf00 f00\nbar f00");
    }

    #[test]
    fn confirm_session_is_one_undo_step() {
        let mut e = ed("foo\nfoo foo\nbar foo");
        keys(&mut e, "x:%s/o/0/gc<CR>yynyy");
        assert_eq!(e.mode(), Mode::Confirm);
        keys(&mut e, "a");
        assert_eq!(e.text(), "00\nfo0 f00\nbar f00");
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "u");
        assert_eq!(e.text(), "oo\nfoo foo\nbar foo");
        keys(&mut e, "u");
        assert_eq!(e.text(), "foo\nfoo foo\nbar foo");
    }

    #[test]
    fn substitute_is_not_what_dot_repeats() {
        let mut e = ed("abc\nfoo");
        keys(&mut e, "x:2s/foo/bar/<CR>gg.");
        assert_eq!(e.text(), "c\nbar");
    }

    // Golden cases from vim 9.1 (`vim -N -u NONE -i NONE`), each command in
    // its own undo block: the cursor after the command, after `u` and
    // after Ctrl+R.

    fn e_at(e: &mut Editor, line: usize, col: usize) {
        keys(e, "gg0");
        for _ in 0..line {
            keys(e, "j");
        }
        for _ in 0..col {
            keys(e, "l");
        }
        assert_eq!(e.cursor(), pos(line, col));
    }

    #[test]
    fn vim_golden_percent_s_cursor_undo_redo() {
        let mut e = ed("one\ntwo\n    pre foo\nx foo y\nend");
        e_at(&mut e, 0, 1);
        keys(&mut e, ":%s/foo/BAR/<CR>");
        assert_eq!(e.text(), "one\ntwo\n    pre BAR\nx BAR y\nend");
        // The last line changed, at its first non-blank.
        assert_eq!(e.cursor(), pos(3, 0));
        assert_eq!(e.message(), Some("2 substitutions on 2 lines"));
        keys(&mut e, "u");
        // The first line changed, at column 0 (not its first non-blank).
        assert_eq!(e.cursor(), pos(2, 0));
        keys(&mut e, "<C-r>");
        assert_eq!(e.cursor(), pos(2, 0));
    }

    #[test]
    fn vim_golden_s_g_cursor_and_undo() {
        let mut e = ed("  foo foo foo");
        e_at(&mut e, 0, 11);
        keys(&mut e, ":s/foo/q/g<CR>");
        assert_eq!(e.text(), "  q q q");
        assert_eq!(e.cursor(), pos(0, 2));
        assert_eq!(e.message(), Some("3 substitutions on 1 line"));
        keys(&mut e, "u");
        assert_eq!(e.cursor(), pos(0, 0));
    }

    #[test]
    fn vim_golden_n_flag_cursor() {
        let mut e = ed(GOLD);
        e_at(&mut e, 0, 7);
        keys(&mut e, ":s/foo/X/n<CR>");
        assert_eq!(e.message(), Some("1 match on 1 line"));
        assert_eq!(e.cursor(), pos(0, 2));
        // No match: the cursor stays.
        e_at(&mut e, 1, 1);
        keys(&mut e, ":s/foo/X/n<CR>");
        assert_eq!(e.message(), Some("E486: Pattern not found: foo"));
        assert_eq!(e.cursor(), pos(1, 1));
    }

    #[test]
    fn vim_golden_visual_range_cursor() {
        let mut e = ed(GOLD);
        e_at(&mut e, 1, 1);
        keys(&mut e, "Vj:s/foo/X/<CR>");
        assert_eq!(e.text(), "  foo foo\nbar\n  x X");
        assert_eq!(e.cursor(), pos(2, 2));
        assert_eq!(e.message(), Some("1 substitution on 1 line"));
        keys(&mut e, "u");
        assert_eq!(e.cursor(), pos(2, 0));
    }

    #[test]
    fn vim_golden_empty_pattern_is_the_last_search() {
        let mut e = ed(GOLD);
        e_at(&mut e, 1, 1);
        keys(&mut e, "/foo<CR>:s//X/<CR>");
        assert_eq!(e.text(), "  foo foo\nbar\n  x X");
        assert_eq!(e.cursor(), pos(2, 2));
        let mut e = ed(GOLD);
        keys(&mut e, ":s//X/<CR>");
        assert_eq!(e.message(), Some("E35: No previous regular expression"));
        assert_eq!(e.text(), GOLD);
    }

    #[test]
    fn vim_golden_ranges() {
        let mut e = ed("a\na\na");
        keys(&mut e, ":1,9s/a/b/<CR>");
        assert_eq!(e.message(), Some("E16: Invalid range"));
        keys(&mut e, ":0,2s/a/b/<CR>");
        assert_eq!(e.text(), "b\nb\na");
        assert_eq!(e.cursor(), pos(1, 0));
        keys(&mut e, ":3,2s/b/c/<CR>");
        assert_eq!(e.text(), "b\nc\na");
    }

    #[test]
    fn substitute_makes_the_pattern_the_last_search() {
        let mut e = ed("foo\nbar foo");
        keys(&mut e, ":s/foo/x/<CR>n");
        assert_eq!(e.cursor(), pos(1, 4));
    }

    const GOLD: &str = "  foo foo\nbar\n  x foo";

    /// Runs `:<cmd>` from (1, 1) of `GOLD`, then `answers`, and checks the
    /// text and cursor, then the cursor after `u` and after Ctrl+R.
    fn golden_confirm(cmd: &str, answers: &str, text: &str, at: Pos, undo: Pos) {
        let mut e = ed(GOLD);
        e_at(&mut e, 1, 1);
        keys(&mut e, &format!(":{cmd}<CR>{answers}"));
        assert_eq!(e.mode(), Mode::Normal, "{answers}");
        assert_eq!(e.text(), text, "{answers}");
        assert_eq!(e.cursor(), at, "{answers}: after");
        if text == GOLD {
            assert!(!e.history().can_undo(), "{answers}");
            return;
        }
        keys(&mut e, "u");
        assert_eq!(e.text(), GOLD, "{answers}");
        assert_eq!(e.cursor(), undo, "{answers}: undo");
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), text, "{answers}");
        assert_eq!(e.cursor(), undo, "{answers}: redo");
    }

    #[test]
    fn vim_golden_confirm_yny() {
        let t = "  X foo\nbar\n  x X";
        golden_confirm("%s/foo/X/gc", "yny", t, pos(2, 0), pos(0, 4));
    }

    #[test]
    fn vim_golden_confirm_ynn() {
        let t = "  X foo\nbar\n  x foo";
        golden_confirm("%s/foo/X/gc", "ynn", t, pos(2, 4), pos(0, 4));
    }

    #[test]
    fn vim_golden_confirm_yq() {
        let t = "  X foo\nbar\n  x foo";
        golden_confirm("%s/foo/X/gc", "yq", t, pos(0, 4), pos(0, 4));
    }

    #[test]
    fn vim_golden_confirm_yn_one_line() {
        let t = "  X foo\nbar\n  x foo";
        golden_confirm("1s/foo/X/gc", "yn", t, pos(0, 4), pos(0, 4));
    }

    #[test]
    fn vim_golden_confirm_ny_one_line() {
        let t = "  foo X\nbar\n  x foo";
        golden_confirm("1s/foo/X/gc", "ny", t, pos(0, 0), pos(0, 0));
    }

    #[test]
    fn vim_golden_confirm_nyq() {
        let t = "  foo X\nbar\n  x foo";
        golden_confirm("%s/foo/X/gc", "nyq", t, pos(2, 4), pos(0, 0));
    }

    #[test]
    fn vim_golden_confirm_nnn() {
        golden_confirm("%s/foo/X/gc", "nnn", GOLD, pos(2, 4), pos(0, 0));
    }

    #[test]
    fn vim_golden_confirm_n_esc() {
        golden_confirm("%s/foo/X/gc", "n<Esc>", GOLD, pos(0, 6), pos(0, 0));
    }

    #[test]
    fn vim_golden_confirm_yl() {
        let t = "  X X\nbar\n  x foo";
        golden_confirm("%s/foo/X/gc", "yl", t, pos(0, 0), pos(0, 0));
    }

    #[test]
    fn vim_golden_confirm_na() {
        let t = "  foo X\nbar\n  x X";
        golden_confirm("%s/foo/X/gc", "na", t, pos(2, 2), pos(0, 0));
    }

    #[test]
    fn vim_golden_confirm_without_g() {
        // Without g, skipping a line's first match skips the line.
        let t = "  foo foo\nbar\n  x X";
        golden_confirm("%s/foo/X/c", "ny", t, pos(2, 0), pos(2, 0));
    }

    #[test]
    fn vim_golden_confirm_ignores_other_keys() {
        let t = "  X foo\nbar\n  x foo";
        golden_confirm("%s/foo/X/gc", "zyq", t, pos(0, 4), pos(0, 4));
    }
}

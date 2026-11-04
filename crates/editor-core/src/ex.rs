//! The command line's language: what `:` commands mean.
//!
//! `parse` turns the text typed after `:` into an `ExCmd`, or the message
//! to show instead. The editor carries the command out (`Editor` in
//! `editor.rs`); `:s` does its matching and replacing in `substitute`.
//!
//! The commands are moon_edit's: `:w`, `:w!`, `:q`, `:q!`, `:wq`, `:x`,
//! `:cq`, `:e!`, `:help` (or `:h`), `:table [cols rows]` and
//! `[range]s/pattern/replacement/[flags]`, with `:qa`, `:qa!` and `:wqa`
//! as aliases of `:q`, `:q!` and `:wq`.

use crate::strings;

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExCmd {
    /// `:w`, or `:w!` (`force`).
    Write { force: bool },
    /// `:q` or `:qa`, or `:q!` or `:qa!` (`force`).
    Quit { force: bool },
    /// `:wq` or `:wqa`.
    WriteQuit,
    /// `:x`: write if changed, then close.
    Exit,
    /// `:cq`: close with exit code 1.
    CQuit,
    /// `:e!`: reload from disk, dropping the changes.
    ReloadForce,
    /// `:help` or `:h`.
    Help,
    /// `:table`, with the columns and rows when both are given and
    /// positive.
    Table(Option<(u32, u32)>),
    /// `:s`.
    Substitute(Sub),
}

/// A `:s` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sub {
    pub range: SubRange,
    /// Matched literally. Empty means the last search's pattern.
    pub pattern: String,
    /// Put in literally, but for `$&`, which stands for the match.
    pub replacement: String,
    pub flags: SubFlags,
}

/// The lines a `:s` acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubRange {
    /// No range: the cursor's line.
    Line,
    /// `%`.
    All,
    /// `'<,'>`: the lines of the selection `:` left Visual with.
    Visual,
    /// `N,M`, as 0-based line indexes (`1,10` is `Lines(0, 9)`). Line 0 is
    /// taken as line 1, as vim does.
    Lines(usize, usize),
}

/// `:s` flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubFlags {
    /// Every match on a line, not just the first.
    pub g: bool,
    /// Ignore case.
    pub i: bool,
    /// Ask before each replacement.
    pub c: bool,
    /// Only count the matches; takes priority over `c`.
    pub n: bool,
}

/// Parses the text typed after `:`. Unknown commands give `E492: Not a
/// command: <input>`.
pub fn parse(input: &str) -> Result<ExCmd, String> {
    let input = input.trim();
    let cmd = match input {
        "w" => ExCmd::Write { force: false },
        "w!" => ExCmd::Write { force: true },
        "q" | "qa" => ExCmd::Quit { force: false },
        "q!" | "qa!" => ExCmd::Quit { force: true },
        "wq" | "wqa" => ExCmd::WriteQuit,
        "x" => ExCmd::Exit,
        "cq" => ExCmd::CQuit,
        "e!" => ExCmd::ReloadForce,
        "h" | "help" => ExCmd::Help,
        _ => {
            if let Some(args) = input.strip_prefix("table")
                && (args.is_empty() || args.starts_with(char::is_whitespace))
            {
                return Ok(ExCmd::Table(table_size(args)));
            }
            return match substitute(input) {
                Some(sub) => sub.map(ExCmd::Substitute),
                None => Err(strings::not_a_command(input)),
            };
        }
    };
    Ok(cmd)
}

/// `:table`'s columns and rows: exactly two positive numbers, or none. A
/// number too big for a `u32` is `u32::MAX`, still a size (one the editor
/// refuses as too big).
fn table_size(args: &str) -> Option<(u32, u32)> {
    // Words from `split_whitespace` are never empty, so all digits is a
    // number, and one that does not parse is too big.
    let num = |a: &str| {
        a.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| a.parse::<u32>().unwrap_or(u32::MAX))
    };
    let mut nums = args.split_whitespace().map(num);
    match (nums.next(), nums.next(), nums.next()) {
        (Some(Some(cols)), Some(Some(rows)), None) if cols > 0 && rows > 0 => Some((cols, rows)),
        _ => None,
    }
}

/// `[range]s<d>pattern<d>replacement<d>[flags]`, where the delimiter `<d>`
/// is any char but a letter, a digit or `\`, and `\<d>` stands for it. The
/// closing delimiters may be left off. `None` when `input` is no `:s`.
fn substitute(input: &str) -> Option<Result<Sub, String>> {
    let (range, rest) = sub_range(input)?;
    let mut rest = rest.strip_prefix('s')?.chars();
    let delim = rest.next().filter(|&d| !d.is_alphanumeric() && d != '\\')?;
    let rest = rest.as_str();
    let (pattern, rest) = until_delim(rest, delim);
    let (replacement, flags) = until_delim(rest, delim);
    let mut f = SubFlags::default();
    for (i, c) in flags.char_indices() {
        let on = match c {
            'g' => &mut f.g,
            'i' => &mut f.i,
            'c' => &mut f.c,
            'n' => &mut f.n,
            _ => return Some(Err(strings::trailing_characters(&flags[i..]))),
        };
        *on = true;
    }
    Some(Ok(Sub {
        range,
        pattern,
        replacement,
        flags: f,
    }))
}

/// The range before `s` and what follows it: `%`, `'<,'>`, `N,M`, `N` or
/// none.
fn sub_range(input: &str) -> Option<(SubRange, &str)> {
    if let Some(rest) = input.strip_prefix('%') {
        return Some((SubRange::All, rest));
    }
    if let Some(rest) = input.strip_prefix("'<,'>") {
        return Some((SubRange::Visual, rest));
    }
    let Some((first, rest)) = number(input) else {
        return Some((SubRange::Line, input));
    };
    let (last, rest) = match rest.strip_prefix(',') {
        Some(rest) => number(rest)?,
        None => (first, rest),
    };
    Some((SubRange::Lines(first, last), rest))
}

/// A line number at the start of `s`, as a 0-based index (0 counts as 1),
/// and what follows it.
fn number(s: &str) -> Option<(usize, &str)> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n = s[..end].parse::<usize>().ok()?;
    Some((n.saturating_sub(1), &s[end..]))
}

/// The text of `s` up to an unescaped `delim` (or the end), with `\<delim>`
/// turned into `delim`, and what follows the delimiter.
fn until_delim(s: &str, delim: char) -> (String, &str) {
    let mut out = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == delim {
            return (out, &s[i + c.len_utf8()..]);
        }
        if c == '\\' {
            match chars.next() {
                Some((_, d)) if d == delim => out.push(d),
                Some((_, d)) => {
                    out.push('\\');
                    out.push(d);
                }
                None => out.push('\\'),
            }
            continue;
        }
        out.push(c);
    }
    (out, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Pos;
    use crate::editor::Editor;
    use crate::effects::{Effects, Redraw};
    use crate::input::Input;
    use crate::testkit::*;
    use crate::view_model::{MsgKind, StatusLeft, ViewSource};
    use crate::vim::Mode;

    fn sub(input: &str) -> Sub {
        match parse(input) {
            Ok(ExCmd::Substitute(s)) => s,
            other => panic!("{input}: {other:?}"),
        }
    }

    // command_parser_test.dart: basic commands.

    #[test]
    fn parses_w_as_write() {
        assert_eq!(parse("w"), Ok(ExCmd::Write { force: false }));
        assert_eq!(parse("w!"), Ok(ExCmd::Write { force: true }));
    }

    #[test]
    fn parses_q_as_quit() {
        assert_eq!(parse("q"), Ok(ExCmd::Quit { force: false }));
    }

    #[test]
    fn parses_q_bang_as_forced_quit() {
        assert_eq!(parse("q!"), Ok(ExCmd::Quit { force: true }));
    }

    #[test]
    fn parses_wq_as_write_quit() {
        assert_eq!(parse("wq"), Ok(ExCmd::WriteQuit));
    }

    #[test]
    fn parses_x_as_exit() {
        assert_eq!(parse("x"), Ok(ExCmd::Exit));
    }

    #[test]
    fn parses_help() {
        assert_eq!(parse("help"), Ok(ExCmd::Help));
        assert_eq!(parse("h"), Ok(ExCmd::Help));
    }

    #[test]
    fn parses_cq_and_reload() {
        assert_eq!(parse("cq"), Ok(ExCmd::CQuit));
        assert_eq!(parse("e!"), Ok(ExCmd::ReloadForce));
    }

    #[test]
    fn wqa_alias_parses() {
        assert_eq!(parse("wqa"), Ok(ExCmd::WriteQuit));
        assert_eq!(parse("qa"), Ok(ExCmd::Quit { force: false }));
        assert_eq!(parse("qa!"), Ok(ExCmd::Quit { force: true }));
    }

    #[test]
    fn surrounding_spaces_are_ignored() {
        assert_eq!(parse("  wq "), Ok(ExCmd::WriteQuit));
    }

    #[test]
    fn unknown_command_is_e492() {
        assert_eq!(parse("foo"), Err("E492: Not a command: foo".to_owned()));
        assert_eq!(parse("e"), Err("E492: Not a command: e".to_owned()));
        assert_eq!(parse(""), Err("E492: Not a command: ".to_owned()));
    }

    // command_parser_test.dart: substitute commands.

    #[test]
    fn parses_s_on_the_current_line() {
        let s = sub("s/old/new/");
        assert_eq!(s.pattern, "old");
        assert_eq!(s.replacement, "new");
        assert_eq!(s.range, SubRange::Line);
        assert_eq!(s.flags, SubFlags::default());
    }

    #[test]
    fn parses_percent_s_as_global() {
        let s = sub("%s/old/new/g");
        assert_eq!(s.range, SubRange::All);
        assert!(s.flags.g);
    }

    #[test]
    fn parses_a_line_range_zero_based() {
        assert_eq!(sub("1,10s/old/new/").range, SubRange::Lines(0, 9));
        // Line 0 is line 1, as in vim.
        assert_eq!(sub("0,2s/a/b/").range, SubRange::Lines(0, 1));
        assert_eq!(sub("3s/a/b/").range, SubRange::Lines(2, 2));
    }

    #[test]
    fn parses_the_visual_range() {
        assert_eq!(sub("'<,'>s/old/new/").range, SubRange::Visual);
    }

    #[test]
    fn parses_all_flags() {
        assert_eq!(
            sub("s/old/new/gicn").flags,
            SubFlags {
                g: true,
                i: true,
                c: true,
                n: true
            }
        );
    }

    #[test]
    fn supports_alternate_delimiters() {
        let s = sub("s#old#new#g");
        assert_eq!(s.pattern, "old");
        assert_eq!(s.replacement, "new");
        assert!(s.flags.g);
    }

    #[test]
    fn handles_escaped_delimiters() {
        assert_eq!(sub(r"s/foo\/bar/baz/").pattern, "foo/bar");
        assert_eq!(sub(r"s/a/x\/y/").replacement, "x/y");
        // Any other escape stays as typed.
        assert_eq!(sub(r"s/a\b/c/").pattern, r"a\b");
    }

    #[test]
    fn closing_delimiters_are_optional() {
        let s = sub("s/foo/bar");
        assert_eq!((s.pattern.as_str(), s.replacement.as_str()), ("foo", "bar"));
        let s = sub("s/foo");
        assert_eq!((s.pattern.as_str(), s.replacement.as_str()), ("foo", ""));
    }

    #[test]
    fn alphanumeric_or_backslash_delimiter_is_no_command() {
        assert_eq!(
            parse("sxfooxbarx"),
            Err("E492: Not a command: sxfooxbarx".to_owned())
        );
        assert!(parse(r"s\a\b\").is_err());
        assert_eq!(parse("s"), Err("E492: Not a command: s".to_owned()));
        assert_eq!(parse("1,2x"), Err("E492: Not a command: 1,2x".to_owned()));
    }

    #[test]
    fn an_unknown_flag_is_e488() {
        assert_eq!(
            parse("s/a/b/gq"),
            Err("E488: Trailing characters: q".to_owned())
        );
    }

    // command_parser_test.dart: table command.

    #[test]
    fn parses_table_with_dimensions() {
        assert_eq!(parse("table 3 4"), Ok(ExCmd::Table(Some((3, 4)))));
    }

    #[test]
    fn table_dimensions_past_u32_saturate() {
        // Too big to hold is still a size, so the editor refuses it as too
        // big rather than opening the picker.
        assert_eq!(
            parse("table 99999999999 2"),
            Ok(ExCmd::Table(Some((u32::MAX, 2))))
        );
    }

    #[test]
    fn parses_table_without_valid_dimensions() {
        for input in [
            "table",
            "table 3",
            "table 0 3",
            "table abc def",
            "table 1 2 3",
        ] {
            assert_eq!(parse(input), Ok(ExCmd::Table(None)), "{input}");
        }
    }

    // vim_engine_command_test.dart: the command line.

    /// Types `typed` on the command line one key at a time.
    fn colon(e: &mut Editor, typed: &str) -> Effects {
        keys(e, &format!(":{typed}<CR>"))
    }

    #[test]
    fn colon_enters_command_mode_from_normal() {
        let mut e = ed("hello world");
        keys(&mut e, ":");
        assert_eq!(e.mode(), Mode::Command);
        assert_eq!(e.cmdline(), "");
    }

    #[test]
    fn keys_append_to_the_command_line() {
        let mut e = ed("hello world");
        keys(&mut e, ":w");
        assert_eq!(e.cmdline(), "w");
        keys(&mut e, "q");
        assert_eq!(e.cmdline(), "wq");
        keys(&mut e, " x");
        assert_eq!(e.cmdline(), "wq x");
        e.handle(Input::Text("y\nz".into()));
        assert_eq!(e.cmdline(), "wq xyz");
        assert_eq!(e.text(), "hello world");
    }

    #[test]
    fn escape_returns_to_normal_and_clears() {
        let mut e = ed("hello world");
        keys(&mut e, ":w<Esc>");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.cmdline(), "");
    }

    #[test]
    fn backspace_removes_the_last_grapheme() {
        let mut e = ed("hello world");
        keys(&mut e, ":wq<BS>");
        assert_eq!(e.cmdline(), "w");
        e.handle(Input::Text("e\u{301}".into()));
        keys(&mut e, "<BS>");
        assert_eq!(e.cmdline(), "w");
    }

    #[test]
    fn backspace_on_an_empty_line_returns_to_normal() {
        // vim 9.1: `:<BS>` and `:a<BS><BS>` both leave the command line.
        let mut e = ed("hello world");
        keys(&mut e, ":<BS>");
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, ":a<BS>");
        assert_eq!(e.mode(), Mode::Command);
        keys(&mut e, "<BS>");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn committed_text_after_colon_goes_to_the_command_line() {
        let mut e = ed("hello world");
        e.handle(Input::Text(":wq".into()));
        assert_eq!(e.mode(), Mode::Command);
        assert_eq!(e.cmdline(), "wq");
    }

    #[test]
    fn enter_on_an_empty_line_does_nothing() {
        let mut e = ed("hello world");
        let fx = colon(&mut e, "");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.message(), None);
        assert_eq!(fx.close, None);
    }

    // vim_engine_command_test.dart: command execution.

    #[test]
    fn w_calls_write() {
        let (mut e, files) = ed_with_files("hello world");
        colon(&mut e, "w");
        assert_eq!(files.calls(), [FileCall::Write { force: false }]);
        assert_eq!(e.mode(), Mode::Normal);
        colon(&mut e, "w!");
        assert_eq!(
            files.calls(),
            [
                FileCall::Write { force: false },
                FileCall::Write { force: true }
            ]
        );
    }

    #[test]
    fn a_good_write_clears_dirty() {
        let (mut e, _files) = ed_with_files("hello world");
        keys(&mut e, "x");
        assert!(e.dirty());
        let fx = colon(&mut e, "w");
        assert!(!e.dirty());
        assert_eq!(fx.dirty, Some(false));
    }

    #[test]
    fn a_failed_write_keeps_dirty() {
        let (mut e, files) = ed_with_files("hello world");
        files.fail_writes();
        keys(&mut e, "x");
        colon(&mut e, "w");
        assert!(e.dirty());
    }

    #[test]
    fn a_failed_write_says_why_as_an_error() {
        let (mut e, files) = ed_with_files("hello world");
        files.fail_writes();
        let fx = colon(&mut e, "w");
        assert_eq!(e.message(), Some("write failed: No space left on device"));
        assert_eq!(
            e.status().left,
            StatusLeft::Message {
                text: "write failed: No space left on device".into(),
                kind: MsgKind::Error,
            }
        );
        assert_ne!(fx.redraw, Redraw::None);
    }

    #[test]
    fn a_write_hands_the_saver_the_text() {
        let (mut e, files) = ed_with_files("hello world");
        keys(&mut e, "x");
        colon(&mut e, "w");
        assert_eq!(files.disk(), "ello world");
    }

    #[test]
    fn with_no_saver_a_write_fails_out_loud() {
        let mut e = ed("hello world");
        keys(&mut e, "x");
        colon(&mut e, "w");
        assert!(e.dirty());
        assert_eq!(
            e.message(),
            Some("write failed: file access is not wired up yet")
        );
    }

    #[test]
    fn q_closes_when_clean() {
        let (mut e, files) = ed_with_files("hello world");
        assert_eq!(colon(&mut e, "q").close, Some(0));
        assert_eq!(files.calls(), [FileCall::Close { code: 0 }]);
    }

    #[test]
    fn q_warns_when_dirty() {
        let mut e = ed("hello world");
        keys(&mut e, "x");
        let fx = colon(&mut e, "q");
        assert_eq!(fx.close, None);
        assert_eq!(
            e.message(),
            Some("E37: No write since last change (add ! to override)")
        );
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(colon(&mut e, "qa").close, None);
    }

    #[test]
    fn q_bang_force_closes() {
        let mut e = ed("hello world");
        keys(&mut e, "x");
        assert_eq!(colon(&mut e, "q!").close, Some(0));
        let mut e = ed("hello world");
        keys(&mut e, "x");
        assert_eq!(colon(&mut e, "qa!").close, Some(0));
    }

    #[test]
    fn wq_writes_and_closes() {
        let wrote_and_closed = [
            FileCall::Write { force: false },
            FileCall::Close { code: 0 },
        ];
        let (mut e, files) = ed_with_files("hello world");
        assert_eq!(colon(&mut e, "wq").close, Some(0));
        assert_eq!(files.calls(), wrote_and_closed);
        let (mut e, files) = ed_with_files("hello world");
        assert_eq!(colon(&mut e, "wqa").close, Some(0));
        assert_eq!(files.calls(), wrote_and_closed);
    }

    #[test]
    fn wq_does_not_close_when_the_write_fails() {
        let (mut e, files) = ed_with_files("hello world");
        files.fail_writes();
        assert_eq!(colon(&mut e, "wq").close, None);
        assert_eq!(files.calls(), [FileCall::Write { force: false }]);
    }

    #[test]
    fn x_writes_only_when_changed() {
        let (mut e, files) = ed_with_files("hello world");
        assert_eq!(colon(&mut e, "x").close, Some(0));
        assert_eq!(files.calls(), [FileCall::Close { code: 0 }]);
        let (mut e, files) = ed_with_files("hello world");
        keys(&mut e, "x");
        assert_eq!(colon(&mut e, "x").close, Some(0));
        assert_eq!(
            files.calls(),
            [
                FileCall::Write { force: false },
                FileCall::Close { code: 0 }
            ]
        );
        let (mut e, files) = ed_with_files("hello world");
        files.fail_writes();
        keys(&mut e, "x");
        assert_eq!(colon(&mut e, "x").close, None);
    }

    #[test]
    fn cq_closes_with_1() {
        let mut e = ed("hello world");
        keys(&mut e, "x");
        assert_eq!(colon(&mut e, "cq").close, Some(1));
    }

    #[test]
    fn e_bang_reloads() {
        let (mut e, files) = ed_with_files("hello world");
        colon(&mut e, "e!");
        assert_eq!(files.calls(), [FileCall::Reload]);
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn e_bang_takes_the_text_on_disk_as_one_undo_step() {
        let (mut e, files) = ed_with_files("hello world");
        keys(&mut e, "x");
        files.set_disk("from disk\nsecond");
        let fx = colon(&mut e, "e!");
        assert_eq!(e.text(), "from disk\nsecond");
        assert!(!e.dirty());
        assert_eq!(fx.dirty, Some(false));
        keys(&mut e, "u");
        assert_eq!(e.text(), "ello world");
    }

    #[test]
    fn e_bang_on_a_clean_buffer_reports_nothing() {
        let (mut e, files) = ed_with_files("hello world");
        assert!(!e.dirty());
        files.set_disk("changed on disk");
        let fx = colon(&mut e, "e!");
        assert_eq!(e.text(), "changed on disk");
        // Clean before and after: no change of the dirty state to report,
        // and the edit that replaced the text never says dirty.
        assert_eq!(fx.dirty, None);
        assert!(!e.dirty());
    }

    #[test]
    fn closing_tells_the_saver() {
        let (mut e, files) = ed_with_files("hello world");
        assert_eq!(colon(&mut e, "cq").close, Some(1));
        assert_eq!(files.calls(), [FileCall::Close { code: 1 }]);
        let (mut e, files) = ed_with_files("hello world");
        e.handle(Input::CloseRequested);
        assert_eq!(files.calls(), [FileCall::Close { code: 0 }]);
    }

    #[test]
    fn help_returns_to_normal_quietly() {
        // `:table` is tested with the table keys (`editor::table_keys`).
        let mut e = ed("hello world");
        let fx = colon(&mut e, "help");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.message(), None);
        assert_eq!(fx.close, None);
        assert_eq!(e.text(), "hello world");
    }

    #[test]
    fn unknown_command_shows_e492() {
        let mut e = ed("hello world");
        colon(&mut e, "foo");
        assert_eq!(e.message(), Some("E492: Not a command: foo"));
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn reader_mode_refuses_the_command_line() {
        let mut e = build_reader("foo");
        colon(&mut e, "s/foo/bar/");
        assert_eq!(e.text(), "foo");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(colon(&mut e, "q").close, None);
    }

    // vim_engine_command_test.dart: substitute command execution.

    const SUBST: &str = "foo bar foo\nbaz foo\nqux";

    #[test]
    fn s_replaces_the_first_match_on_the_current_line() {
        let mut e = ed(SUBST);
        colon(&mut e, "s/foo/bar/");
        assert_eq!(e.text(), "bar bar foo\nbaz foo\nqux");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn percent_s_g_replaces_every_match() {
        let mut e = ed(SUBST);
        colon(&mut e, "%s/foo/bar/g");
        assert_eq!(e.text(), "bar bar bar\nbaz bar\nqux");
        assert_eq!(e.message(), Some("3 substitutions on 2 lines"));
        assert!(e.dirty());
    }

    #[test]
    fn s_g_replaces_every_match_on_the_current_line() {
        let mut e = ed(SUBST);
        colon(&mut e, "s/foo/bar/g");
        assert_eq!(e.text(), "bar bar bar\nbaz foo\nqux");
        assert_eq!(e.message(), Some("2 substitutions on 1 line"));
    }

    #[test]
    fn gn_counts_without_replacing() {
        let mut e = ed(SUBST);
        colon(&mut e, "%s/foo/bar/gn");
        assert_eq!(e.text(), SUBST);
        assert_eq!(e.message(), Some("3 matches on 2 lines"));
        assert!(!e.dirty());
        assert!(!e.history().can_undo());
    }

    #[test]
    fn s_without_a_match_says_so() {
        let mut e = ed(SUBST);
        keys(&mut e, "l");
        colon(&mut e, "s/xyz/bar/");
        assert_eq!(e.message(), Some("E486: Pattern not found: xyz"));
        assert_eq!(e.cursor(), Pos { line: 0, col: 1 });
        assert_eq!(e.mode(), Mode::Normal);
    }

    // vim_engine_test.dart: visual mode command entry.

    #[test]
    fn substitute_on_a_visual_selection_replaces_only_its_lines() {
        let mut e = ed("foo\nfoo\nfoo");
        keys(&mut e, "jV:s/foo/bar/g<CR>");
        assert_eq!(e.text(), "foo\nbar\nfoo");
    }

    #[test]
    fn the_visual_range_is_remembered_after_normal_colon() {
        let mut e = ed("foo\nfoo\nfoo");
        keys(&mut e, "jV:<Esc>gg");
        colon(&mut e, "'<,'>s/foo/bar/");
        assert_eq!(e.text(), "foo\nbar\nfoo");
    }

    #[test]
    fn the_visual_range_without_a_selection_is_e20() {
        let mut e = ed("foo");
        colon(&mut e, "'<,'>s/foo/bar/");
        assert_eq!(e.text(), "foo");
        assert_eq!(e.message(), Some("E20: Mark not set"));
    }
}

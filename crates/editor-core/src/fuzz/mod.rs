//! Fuzz harnesses. Each takes raw bytes and must return without
//! panicking. They are driven by the `fuzz/` crate (libFuzzer, nightly)
//! and replayed on stable by `replay`.

use std::ops::Range;

use crate::buffer::{Buffer, Pos};
use crate::config::Config;
use crate::ex::{self, ExCmd};
use crate::file::{recovery, registers_store};
use crate::markdown::block::oracle::{assert_fresh, check_update, snapshot};
use crate::markdown::{Blocks, Reveal, display_to_src, line_display, parse_line, src_to_display};
use crate::substitute::{self, Answer};
use crate::tokens::Theme;

#[cfg(test)]
mod replay;

/// A harness: raw bytes in, no panic out.
pub type Harness = fn(&[u8]);

/// Every harness by target name. Append new targets here.
pub const TARGETS: &[(&str, Harness)] = &[
    ("config_parse", config_parse),
    ("state_files", state_files),
    ("markdown_incremental", markdown_incremental),
    ("ex_command", ex_command),
];

/// The config file's text, as `Config::load` reads it: a file that is not
/// UTF-8 is an error there (the defaults), so there is nothing to parse.
pub fn config_parse(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (cfg, _) = Config::parse(text);
    for theme in [Theme::Light, Theme::Dark] {
        let _ = cfg.tokens(theme);
    }
}

/// The two state-file parsers. The first byte picks one: `r` for the
/// registers file (the rest is its bytes), `m` for recovery metadata (the
/// rest, decoded lossily). Anything else, or nothing, is ignored.
pub fn state_files(data: &[u8]) {
    match data.split_first() {
        Some((b'r', rest)) => {
            if let Some(regs) = registers_store::parse(rest) {
                let again = registers_store::parse(registers_store::serialise(&regs).as_bytes());
                assert_eq!(again, Some(regs), "registers did not round-trip");
            }
        }
        Some((b'm', rest)) => {
            let text = String::from_utf8_lossy(rest);
            let _ = recovery::parse_meta(&text, "/fuzz.md", 1);
        }
        _ => {}
    }
}

/// `:s` through `ex::parse`, `substitute::lines`, `run` and a confirm
/// session. The input is `cmd 0x00 text 0x00 tail`, each part decoded
/// lossily and a missing part empty. `text` is the buffer. `tail[0..4]`
/// are `cursor, vflag, v0, v1` (zero when missing): the cursor line wraps
/// onto the buffer and, when `vflag & 1` is set, the last Visual selection
/// is `(v0 % n, v1 % n)`. `tail[4..]` are confirm answers, one key each.
///
/// The edits of `run` are applied top to bottom, as `substitute_all` in
/// the editor does; those each `answer` returns are applied in order, as
/// `confirm_key` does. A key that gives no `Answer` is skipped, as the
/// editor waits on it. When the answers run out the session is quit.
pub fn ex_command(data: &[u8]) {
    let mut parts = data.splitn(3, |&b| b == 0);
    let cmd = String::from_utf8_lossy(parts.next().unwrap_or(&[]));
    let text = String::from_utf8_lossy(parts.next().unwrap_or(&[]));
    let tail = parts.next().unwrap_or(&[]);
    let byte = |i: usize| tail.get(i).copied().unwrap_or(0);
    let answers = tail.get(4..).unwrap_or(&[]);

    let Ok(ExCmd::Substitute(sub)) = ex::parse(&cmd) else {
        return;
    };
    let buf = Buffer::from_text(&text);
    let n = buf.line_count();
    let visual = (byte(1) & 1 == 1).then(|| (usize::from(byte(2)) % n, usize::from(byte(3)) % n));
    let Ok(r) = substitute::lines(sub.range, usize::from(byte(0)) % n, visual, n) else {
        return;
    };

    let result = substitute::run(&buf, &sub, r.clone());
    let mut applied = buf.clone();
    for e in &result.edits {
        assert_eq!(
            buf.clamp(e.range.start),
            e.range.start,
            "edit start off a boundary"
        );
        assert_eq!(
            buf.clamp(e.range.end),
            e.range.end,
            "edit end off a boundary"
        );
        applied.replace(e.range.clone(), &e.text);
    }

    let mut session = substitute::confirm_session(&buf, &sub, r);
    let mut confirmed = buf.clone();
    let mut fed = 0;
    let mut keys = answers.iter();
    while !session.done() {
        if let Some((line, _)) = session.current() {
            assert!(line < n, "current match on line {line} of {n}");
        }
        assert!(
            fed < answers.len() + 1,
            "session not done after {fed} answers fed (answers.len() = {})",
            answers.len()
        );
        let answer = match keys.next() {
            Some(&b) => match Answer::from_key(b as char) {
                Some(a) => a,
                None => continue,
            },
            None => {
                for e in session.answer(Answer::from_key('q').expect("q is an answer")) {
                    confirmed.replace(e.range, &e.text);
                }
                assert!(session.done(), "session not done after quit");
                break;
            }
        };
        fed += 1;
        for e in session.answer(answer) {
            confirmed.replace(e.range, &e.text);
        }
    }
}

/// The most edit records `markdown_incremental` applies.
const MAX_EDITS: usize = 256;

/// Incremental block reparsing against the oracle. The input is the initial
/// text (decoded lossily) up to the first `0x00`, then edit records: six
/// bytes `[flags, l0, c0, l1, c1, len]` and `len` bytes of replacement text
/// (decoded lossily). A truncated record ends the sequence. Positions wrap
/// onto the buffer and snap down to a char boundary.
///
/// When `flags & 0x0f` is zero, `update` is handed an `edited` range that
/// cannot fit, and must fall back to a full rebuild. Otherwise it gets the
/// contract's arguments and `check_update` judges the result. After every
/// edit the display mappings of every line `update` repainted are checked
/// too (every line of the initial text first); the high nibble of `flags`
/// places the cursor for `Reveal::Cursor`.
pub fn markdown_incremental(data: &[u8]) {
    let (text, mut rest) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &[][..]),
    };
    let mut buf = Buffer::from_text(&String::from_utf8_lossy(text));
    let mut bl = Blocks::new(&buf);
    check_display(&buf, &bl, 0..buf.line_count(), 0);
    for step in 0..MAX_EDITS {
        let Some((&[flags, l0, c0, l1, c1, len], tail)) = rest.split_first_chunk::<6>() else {
            break;
        };
        let Some((repl, tail)) = tail.split_at_checked(usize::from(len)) else {
            break;
        };
        rest = tail;
        let repl = String::from_utf8_lossy(repl);

        let a = fuzz_pos(&buf, l0, c0);
        let b = fuzz_pos(&buf, l1, c1);
        let (start, end) = if (a.line, a.col) <= (b.line, b.col) {
            (a, b)
        } else {
            (b, a)
        };
        let abuse = flags & 0x0f == 0;
        let before = (!abuse).then(|| snapshot(&buf, &bl));
        let old_n = buf.line_count();
        let e = buf.replace(start..end, &repl);
        let delta = buf.line_count() as isize - old_n as isize;
        let changed = if let Some(before) = before {
            let edited = e.range.start.line..e.range.end.line + 1;
            let changed = bl.update(&buf, edited.clone(), delta);
            check_update(
                &before,
                &buf,
                &bl,
                edited,
                delta,
                changed.clone(),
                &format!("edit {step}"),
            );
            changed
        } else {
            let changed = bl.update(&buf, 0..buf.line_count() + 1 + usize::from(l0), delta);
            assert_eq!(
                changed,
                0..buf.line_count(),
                "edit {step}: abuse did not rebuild"
            );
            assert_fresh(&buf, &bl);
            changed
        };
        check_display(&buf, &bl, changed, usize::from(flags >> 4));
    }
}

/// A buffer position from fuzz bytes: the line wraps onto the buffer, the
/// column onto the line (end included), snapped down to a char boundary.
fn fuzz_pos(buf: &Buffer, l: u8, c: u8) -> Pos {
    let line = usize::from(l) % buf.line_count();
    let text = buf.line(line);
    let mut col = usize::from(c) % (text.len() + 1);
    while !text.is_char_boundary(col) {
        col -= 1;
    }
    Pos { line, col }
}

/// The most mapping calls times spans one line's display check may cost.
/// Each `display_to_src` or `src_to_display` call walks every span, so
/// probing every offset of a long, span-dense line is quadratic.
const DISPLAY_BUDGET: usize = 1_000_000;

/// Every element of `v`, or when that is over the budget (each probe
/// costing `per_probe`), a deterministic subset: the first and last, the
/// span `edges`, and evenly strided elements of `v`, each set strided to
/// half the probes the budget allows.
fn probes(v: Vec<usize>, edges: Vec<usize>, per_probe: usize) -> Vec<usize> {
    if v.len().saturating_mul(per_probe) <= DISPLAY_BUDGET {
        return v;
    }
    let half = (DISPLAY_BUDGET / per_probe / 2).max(1);
    let stride = |x: &[usize]| x.len().div_ceil(half).max(1);
    let mut out: Vec<usize> = v.first().into_iter().chain(v.last()).copied().collect();
    out.extend(edges.iter().step_by(stride(&edges)));
    out.extend(v.iter().step_by(stride(&v)));
    out.sort_unstable();
    out.dedup();
    out
}

/// The display of each line in `lines`, under every reveal, keeps the
/// mapping properties `display_to_src` and `src_to_display` document.
/// `cursor` wraps onto each line for `Reveal::Cursor`. Offsets are probed
/// within `DISPLAY_BUDGET` (see `probes`).
fn check_display(buf: &Buffer, bl: &Blocks, lines: Range<usize>, cursor: usize) {
    for i in lines {
        let line = buf.line(i);
        let kind = bl.kind(i);
        let inline = parse_line(line, kind);
        let mut c = cursor % (line.len() + 1);
        while !line.is_char_boundary(c) {
            c -= 1;
        }
        for reveal in [Reveal::None, Reveal::Cursor(c), Reveal::Block] {
            let d = line_display(line, kind, &inline, reveal);
            let per_probe = d.spans.len() + 1;
            let shown: String = d.spans.iter().map(|s| &line[s.src.clone()]).collect();
            let bounds = (0..=shown.len())
                .filter(|&b| shown.is_char_boundary(b))
                .collect();
            let edges = d
                .spans
                .iter()
                .scan(0, |at, s| {
                    *at += s.src.len();
                    Some(*at)
                })
                .collect();
            for b in probes(bounds, edges, per_probe) {
                let src = display_to_src(&d, b);
                assert!(
                    src <= line.len() && line.is_char_boundary(src),
                    "line {i} {line:?} {reveal:?}: display {b} maps to {src}"
                );
            }
            let cols = (0..=line.len() + 1).collect();
            let edges = d
                .spans
                .iter()
                .flat_map(|s| [s.src.start, s.src.end])
                .chain([line.len()])
                .collect();
            for col in probes(cols, edges, per_probe) {
                if col > line.len() {
                    assert_eq!(
                        src_to_display(&d, col),
                        None,
                        "line {i} {line:?} {reveal:?}: src {col} past the end"
                    );
                } else {
                    let _ = src_to_display(&d, col);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ex_command_substitute_on_last_line() {
        for cmd in ["%s/a/$&$&/gc", "'<,'>s/x/y/"] {
            assert!(matches!(ex::parse(cmd), Ok(ExCmd::Substitute(_))));
        }
        ex_command(b"%s/a/$&$&/gc\0a\naa\na\0\x02\x00\x00\x00yyna"); // cursor on line 2, answers y y n a
        ex_command(b"'<,'>s/x/y/\0x\0\x00\x01\x05\x00"); // visual range clamped by % n
    }

    #[test]
    fn markdown_incremental_handles_crlf_edits() {
        let mut d = b"# t\r\n```\r\ncode\r\n```\r\n| a |\r\n|---|\r\n".to_vec();
        d.push(0);
        d.extend([1, 1, 0, 1, 3, 2]);
        d.extend(b"\r\n"); // replace the opener line's "```" with "\r\n"
        d.extend([1, 3, 0, 3, 3, 1]);
        d.extend(b"\r"); // a lone "\r" over line 3
        markdown_incremental(&d);
    }

    #[test]
    fn markdown_incremental_abuse_rebuilds() {
        let mut d = b"a\n```\nb\n".to_vec();
        d.push(0);
        d.extend([0x10, 0, 0, 0, 1, 1]);
        d.extend(b"x"); // flags & 0x0f == 0
        markdown_incremental(&d);
    }

    /// A long, span-dense line must stay inside `DISPLAY_BUDGET` rather
    /// than probe every offset (quadratic in spans), which would hit the
    /// libFuzzer timeout.
    #[test]
    fn markdown_incremental_bounds_long_dense_lines() {
        let mut d = b"*a*".repeat(20 * 1024);
        d.push(0);
        d.extend([1, 0, 3, 0, 3, 1]);
        d.extend(b"x");
        markdown_incremental(&d);
    }
}

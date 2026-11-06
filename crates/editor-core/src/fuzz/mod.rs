//! Fuzz harnesses. Each takes raw bytes and must return without
//! panicking. They are driven by the `fuzz/` crate (libFuzzer, nightly)
//! and replayed on stable by `replay`.

use std::borrow::Cow;
use std::cell::Cell;
use std::ops::{Range, RangeInclusive};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::buffer::{Buffer, Pos};
use crate::config::Config;
use crate::editor::{Editor, OpenRequest};
use crate::ex::{self, ExCmd};
use crate::file::{recovery, registers_store};
use crate::host::{Host, Waker, WindowRequest};
use crate::input::{Input, KeyEvent};
use crate::keyseq::parse_keys;
use crate::limits;
use crate::markdown::block::oracle::{assert_fresh, check_update, snapshot};
use crate::markdown::{Blocks, Reveal, display_to_src, line_display, parse_line, src_to_display};
use crate::platform::Platform;
use crate::search;
use crate::substitute::{self, Answer};
use crate::tokens::Theme;
use crate::view_model::ViewSource;

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
    ("editor_keys", editor_keys),
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
///
/// Both results are checked against `sub_oracle`, which rebuilds the text
/// line by line from strings and never works out a position, and the
/// session's current match must be the text it asks about.
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
    // Each edit is placed in the buffer as the edits before it leave it.
    for e in &result.edits {
        assert_eq!(
            applied.clamp(e.range.start),
            e.range.start,
            "edit start off a boundary"
        );
        assert_eq!(
            applied.clamp(e.range.end),
            e.range.end,
            "edit end off a boundary"
        );
        applied.replace(e.range.clone(), &e.text);
    }
    let matches = oracle_matches(&buf, &sub, r.clone());
    let all = vec![!sub.flags.n; matches.len()];
    assert_eq!(
        lines_text(&applied),
        sub_oracle(&buf, &sub, &matches, &all),
        "run's edits disagree with the oracle"
    );

    // Whether each match, in the order the session asks, was replaced.
    let mut replaced = vec![false; matches.len()];
    let mut asked = 0;
    let mut session = substitute::confirm_session(&buf, &sub, r);
    let mut confirmed = buf.clone();
    let mut fed = 0;
    let mut keys = answers.iter();
    while !session.done() {
        if let Some((line, range)) = session.current() {
            let now = confirmed.line_count();
            assert!(line < now, "current match on line {line} of {now}");
            let (l, m) = &matches[asked];
            let at = |col| Pos { line, col };
            assert_eq!(
                confirmed.text_range(at(range.start)..at(range.end)),
                &buf.line(*l)[m.clone()],
                "current match is not the text asked about"
            );
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
        match answer {
            Answer::Yes | Answer::Last => replaced[asked] = true,
            Answer::All => replaced[asked..].fill(true),
            Answer::No | Answer::Quit => {}
        }
        asked = match answer {
            Answer::Yes | Answer::No => asked + 1,
            _ => matches.len(),
        };
        for e in session.answer(answer) {
            confirmed.replace(e.range, &e.text);
        }
    }
    assert_eq!(
        lines_text(&confirmed),
        sub_oracle(&buf, &sub, &matches, &replaced),
        "the confirm session's edits disagree with the oracle"
    );
}

/// Every match `sub` can replace on `lines` of `buf`, top to bottom and in
/// the order a confirm session asks: all of a line's with `g`, else its
/// first.
fn oracle_matches(
    buf: &Buffer,
    sub: &ex::Sub,
    lines: RangeInclusive<usize>,
) -> Vec<(usize, Range<usize>)> {
    lines
        .flat_map(|l| {
            let mut ms = search::literal_matches(buf.line(l), &sub.pattern, sub.flags.i);
            if !sub.flags.g {
                ms.truncate(1);
            }
            ms.into_iter().map(move |m| (l, m))
        })
        .collect()
}

/// The `:s` oracle: `buf`'s lines joined by `\n`, with each of `matches`
/// whose `replaced` is true swapped for the replacement (`$&` the match,
/// and `\r\n` a line break, as `Buffer::replace` takes it). It builds each
/// line from its original text, so it shares no position arithmetic with
/// `substitute`.
fn sub_oracle(
    buf: &Buffer,
    sub: &ex::Sub,
    matches: &[(usize, Range<usize>)],
    replaced: &[bool],
) -> String {
    let mut out = Vec::new();
    let mut next = matches.iter().zip(replaced).peekable();
    for l in 0..buf.line_count() {
        let line = buf.line(l);
        let mut new = String::new();
        let mut at = 0;
        while let Some(((_, m), &yes)) = next.next_if(|((ml, _), _)| *ml == l) {
            if yes {
                new.push_str(&line[at..m.start]);
                let text = sub.replacement.replace("$&", &line[m.clone()]);
                new.push_str(&text.replace("\r\n", "\n"));
                at = m.end;
            }
        }
        new.push_str(&line[at..]);
        out.push(new);
    }
    out.join("\n")
}

/// `buf`'s lines joined by `\n`, whatever its line endings.
fn lines_text(buf: &Buffer) -> String {
    (0..buf.line_count())
        .map(|l| buf.line(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The most edit records `markdown_incremental` applies.
const MAX_EDITS: usize = 256;

/// The most oracle work, in steps, one `markdown_incremental` run may do.
/// It bounds the harness's own checking, so that the fuzzer's timeout
/// reflects the product rather than the oracle: the oracle's cost per edit
/// grows with the whole buffer (`snapshot`, `check_update`, `assert_fresh`)
/// and with every repainted line (`check_display`), so a large document and
/// many edits would otherwise time out on a correct product. The run ends
/// cleanly before the oracle work for an edit would pass it; that edit's
/// `update` is still applied, so the product's own per-edit cost stays
/// fully exercised up to that point.
///
/// Measured under cargo-fuzz (ASan, debug assertions) on inputs near
/// `-max_len`, this keeps a run under about two seconds, a fifth of the
/// `-timeout=10`, while still covering several edits of a 64 KiB
/// span-dense line (one line's display check costs about six million).
const ORACLE_BUDGET: usize = 32_000_000;

/// The steps a line costs in one whole-buffer oracle pass (a snapshot, or
/// `check_update` with its fresh rebuild) and in `check_display` before
/// its probes. A probe step is one span walked by a mapping call; a line in
/// a whole-buffer pass formats and compares its kind, which costs about
/// this many of those.
const LINE_STEPS: usize = 64;

/// Incremental block reparsing against the oracle. The input is the initial
/// text (decoded lossily) up to the first `0x00`, then edit records: six
/// bytes `[flags, l0, c0, l1, c1, len]` and `len` bytes of replacement text
/// (decoded lossily). A truncated record ends the sequence, and at most
/// `MAX_EDITS` are applied. Positions wrap onto the buffer and snap down to
/// a char boundary.
///
/// When `flags & 0x0f` is zero, `update` is handed an `edited` range that
/// cannot fit, and must fall back to a full rebuild. Otherwise it gets the
/// contract's arguments and `check_update` judges the result. After every
/// edit the display mappings of every line `update` repainted are checked
/// too (every line of the initial text first); the high nibble of `flags`
/// places the cursor for `Reveal::Cursor`. The run ends once the oracle
/// would pass `ORACLE_BUDGET`.
pub fn markdown_incremental(data: &[u8]) {
    let _ = incremental_checked(data);
}

/// `markdown_incremental`, returning how many edits the oracle checked in
/// full before the input or `ORACLE_BUDGET` ran out.
fn incremental_checked(data: &[u8]) -> usize {
    let (text, mut rest) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &[][..]),
    };
    let mut buf = Buffer::from_text(&String::from_utf8_lossy(text));
    let mut bl = Blocks::new(&buf);
    let pass = |buf: &Buffer| buf.line_count().saturating_mul(LINE_STEPS);
    let Some(mut spent) = check_display(&buf, &bl, 0..buf.line_count(), 0, ORACLE_BUDGET) else {
        return 0;
    };
    for step in 0..MAX_EDITS {
        let Some((&[flags, l0, c0, l1, c1, len], tail)) = rest.split_first_chunk::<6>() else {
            return step;
        };
        let Some((repl, tail)) = tail.split_at_checked(usize::from(len)) else {
            return step;
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
        // The snapshot is a whole-buffer pass over the text before the edit.
        let mut in_budget = abuse || spent + pass(&buf) <= ORACLE_BUDGET;
        let before = (!abuse && in_budget).then(|| {
            spent += pass(&buf);
            snapshot(&buf, &bl)
        });
        let old_n = buf.line_count();
        let e = buf.replace(start..end, &repl);
        let delta = buf.line_count() as isize - old_n as isize;
        let edited = if abuse {
            0..buf.line_count() + 1 + usize::from(l0)
        } else {
            e.range.start.line..e.range.end.line + 1
        };
        let changed = bl.update(&buf, edited.clone(), delta);
        // `check_update` and `assert_fresh` are passes over the edited text.
        in_budget = in_budget && spent + pass(&buf) <= ORACLE_BUDGET;
        if !in_budget {
            return step;
        }
        spent += pass(&buf);
        if let Some(before) = before {
            check_update(
                &before,
                &buf,
                &bl,
                edited,
                delta,
                changed.clone(),
                &format!("edit {step}"),
            );
        } else {
            assert_eq!(
                changed,
                0..buf.line_count(),
                "edit {step}: abuse did not rebuild"
            );
            assert_fresh(&buf, &bl);
        }
        let cursor = usize::from(flags >> 4);
        let Some(s) = check_display(&buf, &bl, changed, cursor, ORACLE_BUDGET - spent) else {
            return step;
        };
        spent += s;
    }
    MAX_EDITS
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
///
/// Returns the steps spent (`LINE_STEPS` a line, plus each probe times the
/// spans it walks), or `None` when a line's probes would take the total
/// past `budget`; that line and those after it are then left unchecked.
fn check_display(
    buf: &Buffer,
    bl: &Blocks,
    lines: Range<usize>,
    cursor: usize,
    budget: usize,
) -> Option<usize> {
    let mut spent = 0;
    for i in lines {
        let line = buf.line(i);
        let kind = bl.kind(i);
        let inline = parse_line(line, kind);
        let mut c = cursor % (line.len() + 1);
        while !line.is_char_boundary(c) {
            c -= 1;
        }
        let mut cost = LINE_STEPS;
        let plans: Vec<_> = [Reveal::None, Reveal::Cursor(c), Reveal::Block]
            .into_iter()
            .map(|reveal| {
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
                let bounds = probes(bounds, edges, per_probe);
                let cols = (0..=line.len() + 1).collect();
                let edges = d
                    .spans
                    .iter()
                    .flat_map(|s| [s.src.start, s.src.end])
                    .chain([line.len()])
                    .collect();
                let cols = probes(cols, edges, per_probe);
                cost += (bounds.len() + cols.len()) * per_probe;
                (reveal, d, bounds, cols)
            })
            .collect();
        spent += cost;
        if spent > budget {
            return None;
        }
        for (reveal, d, bounds, cols) in plans {
            for b in bounds {
                let src = display_to_src(&d, b);
                assert!(
                    src <= line.len() && line.is_char_boundary(src),
                    "line {i} {line:?} {reveal:?}: display {b} maps to {src}"
                );
            }
            for col in cols {
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
    Some(spent)
}

/// A host that touches nothing outside the process: the clipboard is held
/// in memory, links and windows go nowhere, and the clock is `now`, which
/// `editor_keys` moves on by hand through its own handle on it (the editor
/// owns the host, behind `dyn Host`).
#[derive(Default)]
pub(crate) struct FuzzHost {
    now: Rc<Cell<u64>>,
    clipboard: Option<String>,
}

impl Host for FuzzHost {
    fn clipboard_get(&mut self) -> Option<String> {
        self.clipboard.clone()
    }
    fn clipboard_set(&mut self, text: &str) {
        self.clipboard = Some(text.to_owned());
    }
    fn open_external(&mut self, _target: &str) {}
    fn spawn_window(&mut self, _req: WindowRequest) {}
    fn now_ns(&self) -> u64 {
        self.now.get()
    }
    fn waker(&self) -> Waker {
        Arc::new(|| {})
    }
}

/// The path `editor_keys` names its document,
/// `/nonexistent-<slug>-fuzz/fuzz.md`. Its directory does not exist, and
/// the editor has no `Saver` wired to it, so nothing reaches it. Built
/// from the slug, which `lint-name` keeps out of the source.
fn fuzz_path() -> PathBuf {
    PathBuf::from(format!("/nonexistent-{}-fuzz/fuzz.md", app_meta::SLUG))
}

/// How far the clock moves between two keys: a frame at 60 Hz.
const KEY_STEP_NS: u64 = 16_000_000;

/// A whole `Editor` driven by keys. The input is `text SEP keys`, where
/// `SEP` is the first `0x00` (the Mac) or `0x01` (Linux) and sets the
/// platform; with neither, all of it is keys, on an empty text, on the
/// Mac. Both parts are decoded lossily. A leading `R` in the keys opens the
/// document in reader mode and is not pressed. The keys are the tests'
/// `<Esc>` notation (`keyseq::parse_keys`), each a frame after the last.
///
/// The editor has no `Saver` (so `:w`, `:e!` and closing reach no file)
/// and no state directory (no registers file, lock or recovery copy).
/// After every key the cursor must sit on a char boundary of the buffer.
/// A key that closes the window ends the run.
pub fn editor_keys(data: &[u8]) {
    drive(fuzz_path(), data);
}

/// `editor_keys`'s input split into its parts: the text, the keys, the
/// platform and whether the document opens in reader mode.
fn split_input(data: &[u8]) -> (Cow<'_, str>, Cow<'_, str>, Platform, bool) {
    let (text, keys, platform) = match data.iter().position(|&b| b == 0 || b == 1) {
        Some(i) => {
            let platform = if data[i] == 0 {
                Platform::Mac
            } else {
                Platform::Linux
            };
            (&data[..i], &data[i + 1..], platform)
        }
        None => (&[][..], data, Platform::Mac),
    };
    let (view, keys) = match keys.split_first() {
        Some((b'R', rest)) => (true, rest),
        _ => (false, keys),
    };
    (
        String::from_utf8_lossy(text),
        String::from_utf8_lossy(keys),
        platform,
        view,
    )
}

/// `editor_keys` on a document named `path`, returning the editor as the
/// keys left it. Only the tests name any other path than `fuzz_path()`.
fn drive(path: PathBuf, data: &[u8]) -> Editor {
    let (text, keys, platform, view) = split_input(data);
    let host = FuzzHost::default();
    let clock = Rc::clone(&host.now);
    let mut editor = Editor::build(
        OpenRequest {
            path,
            view,
            display_name: None,
        },
        &text,
        None,
        Config::default(),
        platform,
        Box::new(host),
        false,
    );
    for (key, mods) in parse_keys(&keys) {
        let now = clock.get().saturating_add(KEY_STEP_NS);
        clock.set(now);
        let fx = editor.handle(Input::Key(KeyEvent {
            key,
            mods,
            time_ns: now,
        }));
        let c = ViewSource::cursor(&editor);
        assert_eq!(editor.buffer().clamp(c), c, "cursor off a boundary");
        // The run budgets are the product's own document budgets
        // (`limits`), not ones the harness chose. The product refuses a put
        // or a `:s` that would pass them, but typing (and `.` replaying
        // typed text) is not checked and can pass them, so the run ends
        // there instead of working on a document the product would not
        // have grown to.
        if fx.close.is_some()
            || editor.buffer().line_count() > limits::MAX_DOC_LINES
            || editor.buffer().byte_count() > limits::MAX_DOC_BYTES
        {
            break;
        }
    }
    editor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings;
    use crate::view_model::StatusLeft;

    #[test]
    fn editor_keys_touches_no_files() {
        editor_keys(b"# hi\n\0ihello<Esc>:w<CR>:e!<CR>:wq<CR>:x<CR>");
        let path = fuzz_path();
        let dir = path.parent().expect("the fuzz path has a directory");
        assert!(!dir.exists(), "{} exists", dir.display());
    }

    /// `editor_keys_touches_no_files` cannot fail: a write to a missing
    /// directory fails without creating it. Here the directory exists, so
    /// a wired `Saver` would leave `fuzz.md` (or its temporary file) in it.
    #[test]
    fn editor_keys_writes_nothing_to_an_existing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let e = drive(
            dir.path().join("fuzz.md"),
            b"# hi\n\0ihello<Esc>:w<CR>:e!<CR>:wq<CR>:x<CR>",
        );
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|f| f.unwrap().file_name())
            .collect();
        assert!(left.is_empty(), "the harness wrote {left:?}");
        // The writes went to `Unwired` and were refused.
        assert!(e.dirty(), "a write succeeded");
        let StatusLeft::Message { text, .. } = e.status().left else {
            panic!("no refusal showing: {:?}", e.status().left);
        };
        assert!(text.contains(strings::NO_FILE_ACCESS), "{text}");
    }

    /// A document past `limits::MAX_DOC_LINES` ends the run after the first
    /// key: the keys after it are not applied. The document is built from
    /// text, because typing is the only way past the budget and a put or a
    /// `:s` is refused there.
    // The normal-build budget; the fuzz build has a lower one.
    #[cfg(not(fuzzing))]
    #[test]
    fn editor_keys_stops_on_too_many_lines() {
        let _big = crate::testkit::big_doc_lock();
        let mut data = "\n".repeat(limits::MAX_DOC_LINES + 1).into_bytes();
        data.extend_from_slice(b"\0ix<Esc>");
        let e = drive(fuzz_path(), &data);
        assert_eq!(e.buffer().line_count(), limits::MAX_DOC_LINES + 1);
        assert!(!e.text().contains('x'), "a key ran after the budget");

        // At the budget exactly, the run goes on.
        let mut data = "\n".repeat(limits::MAX_DOC_LINES).into_bytes();
        data.extend_from_slice(b"\0ix<Esc>");
        let e = drive(fuzz_path(), &data);
        assert_eq!(e.buffer().line_count(), limits::MAX_DOC_LINES);
        assert!(e.text().contains('x'));
    }

    /// A document past `limits::MAX_DOC_BYTES` ends the run after the first
    /// key, built from one long line of text. There is no at-budget case:
    /// it would be a second 256 MiB document in a debug test.
    // The normal-build budget; the fuzz build has a lower one.
    #[cfg(not(fuzzing))]
    #[test]
    fn editor_keys_stops_on_too_many_bytes() {
        let _big = crate::testkit::big_doc_lock();
        let mut data = vec![b'a'; limits::MAX_DOC_BYTES + 1];
        data.extend_from_slice(b"\0ix<Esc>");
        let e = drive(fuzz_path(), &data);
        assert_eq!(e.buffer().byte_count(), limits::MAX_DOC_BYTES + 1);
        assert!(
            !e.buffer().line(0).contains('x'),
            "a key ran after the budget"
        );
    }

    #[test]
    fn editor_keys_reader_mode_and_linux() {
        let data = b"- a\n- b\n\x01Rjjkk<C-d>gg";
        let (text, keys, platform, view) = split_input(data);
        assert_eq!(
            (&*text, &*keys, platform, view),
            ("- a\n- b\n", "jjkk<C-d>gg", Platform::Linux, true)
        );
        editor_keys(data);
    }

    #[test]
    fn editor_keys_splits_its_input() {
        // No separator: all keys, on an empty text, on the Mac.
        let (text, keys, platform, view) = split_input(b"ihi<Esc>");
        assert_eq!(
            (&*text, &*keys, platform, view),
            ("", "ihi<Esc>", Platform::Mac, false)
        );
        // The first separator wins; a later one is a key.
        let (text, keys, platform, view) = split_input(b"a\0R\x01b");
        assert_eq!(
            (&*text, &*keys, platform, view),
            ("a", "\x01b", Platform::Mac, true)
        );
    }

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

    /// A long table and `MAX_EDITS` one-byte edits in its header: each edit
    /// repaints the whole table, so checking every one costs the oracle
    /// about lines times edits and timed the fuzzer out on a correct
    /// product. `ORACLE_BUDGET` ends the run after a few edits instead.
    #[test]
    fn markdown_incremental_budget_ends_heavy_runs() {
        let mut d = b"|a|\n|-|\n".to_vec();
        d.extend(b"|a|\n".repeat(14_000));
        d.push(0);
        for _ in 0..MAX_EDITS {
            d.extend([1, 0, 1, 0, 1, 1]);
            d.extend(b"x");
        }
        let checked = incremental_checked(&d);
        assert!(
            (1..MAX_EDITS).contains(&checked),
            "checked {checked} of {MAX_EDITS} edits"
        );

        // A light run checks every edit.
        let mut d = b"|a|\n|-|\n|a|\n".to_vec();
        d.push(0);
        for _ in 0..MAX_EDITS {
            d.extend([1, 0, 1, 0, 1, 1]);
            d.extend(b"x");
        }
        assert_eq!(incremental_checked(&d), MAX_EDITS);
    }
}

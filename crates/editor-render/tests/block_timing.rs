//! Keystroke draw cost around blocks (ignored by default; timing is
//! meaningful only in a release build):
//!
//! `cargo test -p editor-render --release --test block_timing -- --ignored --nocapture`
//!
//! Each case types characters into one line and times `paint_lines` for
//! that line, as the shell does per keystroke. The budget is a 2 ms p95 draw.
//!
//! - `paragraph`: plain prose, the baseline.
//! - `code line, <lang> x<n>`: inside an n-line block (40 to 5,000 lines) with
//!   highlighting on. For `rust` every keystroke finds the block's lines and
//!   sends its text (the colour lands later, off this path); `text` has no
//!   highlighter, so only the block's extent is found.
//! - `table row, cursor outside`: an edit to a row of a 30-row table while the
//!   cursor is elsewhere (a substitution), so the whole table is laid out and
//!   repainted.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Fake, view};
use editor_core::buffer::Pos;
use editor_core::tokens::{Theme, Tokens};

const KEYS: usize = 300;

fn report(name: &str, mut times: Vec<Duration>) {
    times.sort_unstable();
    let pct = |p: f64| times[((times.len() as f64 - 1.0) * p).round() as usize];
    println!(
        "{name:<28} p50 {:>8.3} ms  p95 {:>8.3} ms  max {:>8.3} ms",
        pct(0.5).as_secs_f64() * 1e3,
        pct(0.95).as_secs_f64() * 1e3,
        times.last().unwrap().as_secs_f64() * 1e3,
    );
}

/// Types `KEYS` characters at the end of line `edit`, timing each paint.
fn run(name: &str, lines: Vec<String>, edit: usize, cursor_line: usize, highlight: bool) {
    let t = Tokens::defaults(Theme::Light);
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut src = Fake::new(&refs, t.clone());
    src.cursor = Pos {
        line: cursor_line,
        col: 0,
    };
    let mut v = view(&t, 2.0);
    if highlight {
        v.set_highlighting(Arc::new(|| {}));
    }
    v.paint_all(&src);
    let mut times = Vec::with_capacity(KEYS);
    let mut text = src.lines[edit].clone();
    for k in 0..KEYS {
        // Keep the row a table row: insert before the closing pipe.
        let at = if text.ends_with('|') {
            text.len() - 1
        } else {
            text.len()
        };
        text.insert(at, if k % 6 == 5 { ' ' } else { 'x' });
        src.set_line(edit, &text);
        if cursor_line == edit {
            src.cursor = Pos {
                line: edit,
                col: at + 1,
            };
        }
        let start = Instant::now();
        v.paint_lines(&src, edit..edit + 1);
        times.push(start.elapsed());
        v.poll_async();
        v.take_damage();
    }
    report(name, times);
}

#[test]
#[ignore = "timing; run by hand in release"]
fn block_keystroke_timing() {
    let prose: Vec<String> = (0..40)
        .map(|i| format!("Paragraph {i} of plain text."))
        .collect();
    run("paragraph", prose, 5, 5, false);

    for (lang, n) in [
        ("rust", 40),
        ("rust", 1000),
        ("rust", 5000),
        ("text", 1000),
        ("text", 5000),
    ] {
        let mut code = vec!["Intro".to_string(), format!("```{lang}")];
        code.extend((0..n).map(|i| format!("    let v{i} = compute({i}) + 1; // note")));
        code.push("```".to_string());
        run(&format!("code line, {lang} x{n}"), code, 10, 10, true);
    }

    let mut table = vec![
        "Intro".to_string(),
        "| Name | Count | Note |".to_string(),
        "|:-----|------:|:----:|".to_string(),
    ];
    table.extend((0..30).map(|i| format!("| row {i} | {i} | text |")));
    run("table row, cursor outside", table, 6, 0, false);
}

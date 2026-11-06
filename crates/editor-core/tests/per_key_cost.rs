//! Per-key cost probe. Run on a quiet machine with:
//! `cargo test --release -p editor-core --test per_key_cost -- --ignored --nocapture`
//! It prints one `<label> <micros> us/key` line per scenario.

use std::fs;
use std::hint::black_box;
use std::time::Instant;

use editor_core::editor::{Config, Editor, OpenRequest};
use editor_core::host::{Host, Waker, WindowRequest};
use editor_core::input::{Input, Key, KeyEvent, Mods};
use editor_core::platform::Platform;

struct StubHost;

impl Host for StubHost {
    fn clipboard_get(&mut self) -> Option<String> {
        None
    }
    fn clipboard_set(&mut self, _text: &str) {}
    fn open_external(&mut self, _target: &str) {}
    fn spawn_window(&mut self, _req: WindowRequest) {}
    fn now_ns(&self) -> u64 {
        0
    }
    fn waker(&self) -> Waker {
        std::sync::Arc::new(|| {})
    }
}

fn key(c: char) -> Input {
    Input::Key(KeyEvent {
        key: Key::Char(c),
        mods: Mods::default(),
        time_ns: 0,
    })
}

/// Opens `text` as a document, sends `setup` untimed, then times `timed`.
fn scenario(label: &str, text: &str, setup: &str, timed: &[char]) {
    let path = std::env::temp_dir().join(format!(
        "per_key_cost_{}_{}.md",
        std::process::id(),
        label.replace(' ', "_")
    ));
    fs::write(&path, text).unwrap();
    let req = OpenRequest {
        path: path.clone(),
        view: false,
        display_name: None,
    };
    let mut e = Editor::open_with_state(
        req,
        Config::default(),
        Platform::Mac,
        Box::new(StubHost),
        None,
    );
    for c in setup.chars() {
        black_box(e.handle(key(c)));
    }
    let inputs: Vec<Input> = timed.iter().map(|&c| key(c)).collect();
    let start = Instant::now();
    for input in inputs {
        black_box(e.handle(input));
    }
    let us = start.elapsed().as_secs_f64() * 1e6 / timed.len() as f64;
    println!("{label} {us:.0} us/key");
    black_box(&e);
    drop(e);
    let _ = fs::remove_file(&path);
}

fn repeat(c: char, n: usize) -> Vec<char> {
    vec![c; n]
}

#[test]
#[ignore]
fn per_key_cost() {
    let hs = repeat('h', 300);
    let xs = repeat('x', 300);
    for (name, ch, n) in [
        ("ascii", "a", 65_536),
        ("e-acute", "é", 32_768),
        ("cjk", "漢", 21_845),
    ] {
        let text = ch.repeat(n);
        scenario(&format!("h end 64KiB {name}"), &text, "$", &hs);
        scenario(&format!("type end 64KiB {name}"), &text, "A", &xs);
    }

    let line = "é".repeat(32_768);
    let two = format!("{line}\n{line}\n");
    let jk: Vec<char> = (0..300)
        .map(|i| if i % 2 == 0 { 'j' } else { 'k' })
        .collect();
    scenario("jk end 64KiB e-acute x2", &two, "$", &jk);

    let code = format!("```\n{}```\n", "code\n".repeat(200_000));
    scenario("l in 200k code block", &code, "100000G", &repeat('l', 300));

    let table = format!("| a | b |\n|---|---|\n{}", "| 1 | 2 |\n".repeat(100_000));
    // `l` first: typing must land inside a cell. At column 0, before the
    // leading pipe, the first key makes the line stop being a table row.
    scenario("type in 100k table", &table, "50000Gli", &repeat('x', 50));
    // At column 0 the first key (in the setup) splits the table; the timed
    // keys edit a line in no table.
    scenario(
        "type col 0 in 100k table",
        &table,
        "50000Gix",
        &repeat('x', 50),
    );
}

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use editor_core::host::Waker;
use editor_core::markdown::BlockId;
use editor_core::tokens::TokenId;
use editor_render::highlight::{Highlighter, SyntaxClass};
use tree_sitter::{InputEdit, Point};

const TIMEOUT: Duration = Duration::from_secs(20);

fn counting_waker() -> (Waker, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    let w: Waker = Arc::new(move || {
        n2.fetch_add(1, Ordering::SeqCst);
    });
    (w, n)
}

/// Poll until `want` has been reported coloured, or fail clearly on timeout.
fn wait_for(h: &mut Highlighter, want: BlockId) {
    let start = Instant::now();
    loop {
        if h.poll().contains(&want) {
            return;
        }
        assert!(
            start.elapsed() < TIMEOUT,
            "timed out after {TIMEOUT:?} waiting for block {want:?} to be highlighted"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn first_request_returns_plain_then_coloured_after_poll() {
    let (waker, wakes) = counting_waker();
    let mut h = Highlighter::new(waker);
    let b = BlockId(1);
    h.request(b, "rust", "fn main() {}");
    // Nothing is coloured on the requesting call: first paint is plain.
    assert!(h.spans(b, 0).is_empty());
    wait_for(&mut h, b);
    assert!(!h.spans(b, 0).is_empty());
    assert!(
        wakes.load(Ordering::SeqCst) >= 1,
        "the waker was never called"
    );
    // A second poll has nothing new to report.
    assert!(h.poll().is_empty());
}

#[test]
fn keyword_class_for_fn() {
    let (waker, _) = counting_waker();
    let mut h = Highlighter::new(waker);
    let b = BlockId(7);
    h.request(b, "rust", "fn main() {}");
    wait_for(&mut h, b);
    let spans = h.spans(b, 0);
    assert!(
        spans
            .iter()
            .any(|(r, c)| *r == (0..2) && *c == SyntaxClass::Keyword),
        "expected `fn` as Keyword, got {spans:?}"
    );
    assert_eq!(SyntaxClass::Keyword.token(), TokenId::ColorSyntaxKeyword);
}

#[test]
fn spans_are_per_line_with_line_relative_ranges() {
    let (waker, _) = counting_waker();
    let mut h = Highlighter::new(waker);
    let b = BlockId(2);
    h.request(b, "rust", "// hi\nlet x = 1;");
    wait_for(&mut h, b);
    assert!(
        h.spans(b, 0)
            .iter()
            .any(|(r, c)| *r == (0..5) && *c == SyntaxClass::Comment)
    );
    assert!(
        h.spans(b, 1)
            .iter()
            .any(|(r, c)| *r == (0..3) && *c == SyntaxClass::Keyword)
    );
    assert!(h.spans(b, 99).is_empty());
}

#[test]
fn unknown_lang_stays_plain() {
    let (waker, wakes) = counting_waker();
    let mut h = Highlighter::new(waker);
    let b = BlockId(3);
    h.request(b, "klingon", "qapla'");
    // Give any (wrongly started) worker a chance, bounded.
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(100) {
        assert!(h.poll().is_empty());
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(h.spans(b, 0).is_empty());
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
}

#[test]
fn unrequested_blocks_do_no_work() {
    let (waker, wakes) = counting_waker();
    let mut h = Highlighter::new(waker);
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(50) {
        assert!(h.poll().is_empty());
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
    assert!(h.spans(BlockId(9), 0).is_empty());
}

#[test]
fn incremental_edit_keeps_tree() {
    let (waker, _) = counting_waker();
    let mut h = Highlighter::new(waker);
    let b = BlockId(4);
    let mut text = String::new();
    for i in 0..200 {
        text.push_str(&format!("fn f{i}() -> u32 {{ {i} }}\n"));
    }
    h.request(b, "rust", &text);
    wait_for(&mut h, b);
    let first = h.last_reparse(b).expect("initial parse recorded");
    assert!(!first.incremental, "the first parse has no old tree");

    // Change `f0` to `g0`: one byte replaced at offset 3, on row 0.
    let start = 3;
    let mut new_text = text.clone();
    new_text.replace_range(start..start + 1, "g");
    let edit = InputEdit {
        start_byte: start,
        old_end_byte: start + 1,
        new_end_byte: start + 1,
        start_position: Point {
            row: 0,
            column: start,
        },
        old_end_position: Point {
            row: 0,
            column: start + 1,
        },
        new_end_position: Point {
            row: 0,
            column: start + 1,
        },
    };
    h.edit(b, edit, &new_text);
    wait_for(&mut h, b);

    let info = h.last_reparse(b).expect("reparse recorded");
    assert!(
        info.incremental,
        "the old tree must be passed to the parser"
    );
    assert!(
        info.changed_bytes < new_text.len() / 20,
        "changed ranges must be small: {} of {} bytes",
        info.changed_bytes,
        new_text.len()
    );
    // Output is still right, including at the far end of the block.
    assert!(
        h.spans(b, 0)
            .iter()
            .any(|(r, c)| *r == (0..2) && *c == SyntaxClass::Keyword)
    );
    assert!(
        h.spans(b, 199)
            .iter()
            .any(|(r, c)| *r == (0..2) && *c == SyntaxClass::Keyword)
    );
}

/// Poll for a bounded window and report whether anything was ever returned.
fn quiet_for(h: &mut Highlighter, window: Duration) -> bool {
    let start = Instant::now();
    let mut quiet = true;
    while start.elapsed() < window {
        quiet &= h.poll().is_empty();
        std::thread::sleep(Duration::from_millis(1));
    }
    quiet
}

#[test]
fn repeat_request_with_same_lang_and_text_is_a_no_op() {
    let (waker, wakes) = counting_waker();
    let mut h = Highlighter::new(waker);
    let b = BlockId(5);
    h.request(b, "rust", "fn main() {}");
    wait_for(&mut h, b);
    let wakes_before = wakes.load(Ordering::SeqCst);
    let info_before = h.last_reparse(b);

    // A host that requests on every paint must not cause work or wakes.
    for _ in 0..5 {
        h.request(b, "rust", "fn main() {}");
    }
    assert!(quiet_for(&mut h, Duration::from_millis(150)), "a job ran");
    assert_eq!(wakes.load(Ordering::SeqCst), wakes_before, "a wake fired");
    assert_eq!(h.last_reparse(b), info_before);
    // The colour is still there.
    assert!(
        h.spans(b, 0)
            .iter()
            .any(|(r, c)| *r == (0..2) && *c == SyntaxClass::Keyword)
    );

    // Changed text still re-highlights.
    h.request(b, "rust", "let x = 1;");
    wait_for(&mut h, b);
    assert!(
        h.spans(b, 0)
            .iter()
            .any(|(r, c)| *r == (0..3) && *c == SyntaxClass::Keyword)
    );
    assert!(wakes.load(Ordering::SeqCst) > wakes_before);
}

#[test]
fn request_after_edit_with_matching_text_is_a_no_op() {
    let (waker, wakes) = counting_waker();
    let mut h = Highlighter::new(waker);
    let b = BlockId(6);
    h.request(b, "rust", "fn a() {}");
    wait_for(&mut h, b);

    let edit = InputEdit {
        start_byte: 3,
        old_end_byte: 4,
        new_end_byte: 4,
        start_position: Point { row: 0, column: 3 },
        old_end_position: Point { row: 0, column: 4 },
        new_end_position: Point { row: 0, column: 4 },
    };
    h.edit(b, edit, "fn b() {}");
    wait_for(&mut h, b);
    let wakes_before = wakes.load(Ordering::SeqCst);
    let info_before = h.last_reparse(b);
    assert!(info_before.unwrap().incremental);

    h.request(b, "rust", "fn b() {}");
    assert!(quiet_for(&mut h, Duration::from_millis(150)), "a job ran");
    assert_eq!(wakes.load(Ordering::SeqCst), wakes_before, "a wake fired");
    assert_eq!(h.last_reparse(b), info_before);
}

#[test]
fn every_class_has_a_distinct_syntax_token() {
    let mut seen = std::collections::HashSet::new();
    for c in SyntaxClass::ALL {
        assert!(c.token().name().starts_with("color.syntax."));
        assert!(seen.insert(c.token()), "{c:?} shares a token");
    }
    assert_eq!(seen.len(), 13);
}

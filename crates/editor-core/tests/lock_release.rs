//! A shell that exits without dropping its editor (a bench run at its
//! deadline, the router quitting) lets the lock record go first.

use std::fs;

use editor_core::editor::{Config, Editor, OpenRequest};
use editor_core::file::StateDir;
use editor_core::host::{Host, Waker, WindowRequest};
use editor_core::platform::Platform;

struct NoHost;

impl Host for NoHost {
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

fn records(state: &StateDir) -> usize {
    fs::read_dir(state.locks()).map_or(0, |d| d.count())
}

#[test]
fn release_lock_lets_the_record_go_before_exit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.md");
    fs::write(&path, "hello").unwrap();
    let state = StateDir::at(dir.path().join("state"));
    let req = OpenRequest {
        path,
        view: false,
        display_name: None,
    };
    let mut e = Editor::open_with_state(
        req,
        Config::default(),
        Platform::Mac,
        Box::new(NoHost),
        Some(&state),
    );
    assert_eq!(records(&state), 1);
    e.release_lock();
    assert_eq!(records(&state), 0);
    // Releasing again, or closing after, is harmless.
    e.release_lock();
    assert_eq!(e.close(0).close, Some(0));
    // The editor is never dropped, as when the process exits under it.
    std::mem::forget(e);
    assert_eq!(records(&state), 0);
}

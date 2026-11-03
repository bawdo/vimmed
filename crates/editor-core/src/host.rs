//! What the editor asks of the shell that hosts it.

use std::path::PathBuf;
use std::sync::Arc;

/// Wakes the shell's main loop. Background workers (images, highlighting,
/// file watch, recovery writes) call it when they have a result; the shell
/// then calls `editor.handle(Input::Tick { .. })` and `view.poll_async()`,
/// and repaints the returned damage.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// A request to open another window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRequest {
    pub path: PathBuf,
    /// True to open in reader mode.
    pub view: bool,
    pub display_name: Option<String>,
}

pub trait Host {
    fn clipboard_get(&mut self) -> Option<String>;
    fn clipboard_set(&mut self, text: &str);
    /// Opens a URL or path with the system's default handler.
    fn open_external(&mut self, target: &str);
    fn spawn_window(&mut self, req: WindowRequest);
    /// Monotonic nanoseconds on the `marks::now_ns` clock.
    fn now_ns(&self) -> u64;
    fn waker(&self) -> Waker;
}

/// A host that records what it is asked. Task 19 moves it into `testkit`.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    #[derive(Default)]
    pub(crate) struct FakeHost {
        pub clipboard: Option<String>,
        pub opened: Vec<String>,
        pub spawned: Vec<WindowRequest>,
        pub now: u64,
    }

    impl Host for FakeHost {
        fn clipboard_get(&mut self) -> Option<String> {
            self.clipboard.clone()
        }
        fn clipboard_set(&mut self, text: &str) {
            self.clipboard = Some(text.to_owned());
        }
        fn open_external(&mut self, target: &str) {
            self.opened.push(target.to_owned());
        }
        fn spawn_window(&mut self, req: WindowRequest) {
            self.spawned.push(req);
        }
        fn now_ns(&self) -> u64 {
            self.now
        }
        fn waker(&self) -> Waker {
            Arc::new(|| {})
        }
    }
}

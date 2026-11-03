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

/// What the window binary was started with, handed to the platform shell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowArgs {
    /// The file to open. `None` starts the window in router mode (Task 16).
    pub path: Option<PathBuf>,
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

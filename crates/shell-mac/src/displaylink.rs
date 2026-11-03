//! Frame pacing with `NSView.displayLink(target:selector:)` (macOS 14). The
//! link follows the view's screen, so moving the window between displays
//! keeps the right rate. It runs only while something is staged.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::NSView;
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;

pub struct Link(Retained<CADisplayLink>);

impl Link {
    /// A paused link that sends `sel` to `target` on each tick.
    pub fn new(view: &NSView, target: &AnyObject, sel: Sel) -> Link {
        // SAFETY: `target` implements `sel`, taking the link as its argument.
        let link = unsafe { view.displayLinkWithTarget_selector(target, sel) };
        link.setPaused(true);
        // SAFETY: the main run loop and a framework mode constant.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Link(link)
    }

    pub fn resume(&self) {
        self.0.setPaused(false);
    }

    pub fn pause(&self) {
        self.0.setPaused(true);
    }

    /// When the frame being prepared at this tick is due on screen.
    pub fn target_s(&self) -> f64 {
        self.0.targetTimestamp()
    }

    /// One refresh period, from the current tick.
    pub fn period_s(&self) -> f64 {
        self.0.targetTimestamp() - self.0.timestamp()
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.0.invalidate();
    }
}

//! The two Objective-C classes: the layer-hosting document view and the
//! application and window delegate. Both only forward to `app`.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSApplicationTerminateReply, NSEvent, NSView, NSWindow,
    NSWindowDelegate,
};
use objc2_core_graphics::CGColor;
use objc2_foundation::{NSArray, NSNotification, NSRect, NSURL};
use objc2_quartz_core::{CADisplayLink, CALayer, kCAGravityTopLeft};

use crate::{app, router};

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "EditorDocumentView"]
    pub struct DocumentView;

    impl DocumentView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            app::on_key(event);
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _link: &CADisplayLink) {
            app::on_tick();
        }

        #[unsafe(method(benchCheck:))]
        fn bench_check(&self, _sender: Option<&AnyObject>) {
            app::on_bench_check();
        }
    }
);

impl DocumentView {
    /// A layer-hosting view: the root layer is set before `wantsLayer`, so
    /// AppKit leaves its contents to us. The layer is filled with `bg`
    /// wherever no surface covers it (during a live resize) and never
    /// stretches the surface.
    pub fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        bg: &CGColor,
    ) -> (Retained<Self>, Retained<CALayer>) {
        // SAFETY: `initWithFrame:` is NSView's designated initialiser.
        let view: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), initWithFrame: frame] };
        let root = CALayer::new();
        // SAFETY: reading a framework constant.
        root.setContentsGravity(unsafe { kCAGravityTopLeft });
        root.setOpaque(true);
        root.setBackgroundColor(Some(bg));
        view.setLayer(Some(&root));
        view.setWantsLayer(true);
        (view, root)
    }

    pub fn tick_sel() -> Sel {
        sel!(tick:)
    }

    /// Calls `benchCheck:` after `delay_s` seconds.
    pub fn schedule_bench_check(&self, delay_s: f64) {
        let none: Option<&AnyObject> = None;
        // SAFETY: the view implements `benchCheck:`, which takes an object.
        let _: () = unsafe {
            msg_send![self, performSelector: sel!(benchCheck:), withObject: none, afterDelay: delay_s]
        };
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "EditorAppDelegate"]
    pub struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            app::launch(self);
            router::did_launch();
        }

        /// Finder, `open` and the Dock: each URL goes to a new window process.
        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            router::open_urls(urls);
        }

        /// Cmd+Q: the editor decides, as for the close button.
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            app::request_close();
            NSApplicationTerminateReply::TerminateCancel
        }
    }

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _w: &NSWindow) -> bool {
            app::request_close();
            false
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _n: &NSNotification) {
            app::on_geometry();
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing(&self, _n: &NSNotification) {
            app::on_geometry();
        }

        #[unsafe(method(windowDidChangeScreen:))]
        fn window_did_change_screen(&self, _n: &NSNotification) {
            app::on_geometry();
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _n: &NSNotification) {
            app::on_became_key();
        }
    }
);

impl Delegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: NSObject's `init`.
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

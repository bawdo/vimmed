//! The two Objective-C classes: the layer-hosting document view and the
//! application and window delegate. Both only forward to `app`.
//!
//! The view is an `NSTextInputClient`. While the editor takes text, a key
//! that could type goes through `interpretKeyEvents:`, so dead keys and
//! input methods compose (`setMarkedText:`) and commit (`insertText:`);
//! every other key, and every key in Normal and Visual, is mapped directly
//! (`input::routes_to_ime`). The composition lives in the editor; the view
//! only reports it.

use editor_core::input::Input;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSApplicationTerminateReply, NSEvent, NSTextInputClient,
    NSView, NSWindow, NSWindowDelegate,
};
use objc2_core_graphics::CGColor;
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound, NSNotification, NSPoint,
    NSRange, NSRect, NSString, NSUInteger, NSURL,
};
use objc2_quartz_core::{CADisplayLink, CALayer, kCAGravityTopLeft};

use crate::{app, input, router};

/// The text in an `insertText:` or `setMarkedText:` argument, which is an
/// `NSString` or an `NSAttributedString`.
fn text_of(obj: &AnyObject) -> String {
    if let Some(a) = obj.downcast_ref::<NSAttributedString>() {
        a.string().to_string()
    } else if let Some(s) = obj.downcast_ref::<NSString>() {
        s.to_string()
    } else {
        String::new()
    }
}

/// `NSNotFound` as an unsigned location.
fn not_found() -> NSUInteger {
    NSNotFound as NSUInteger
}

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
            if app::takes_ime(event) {
                app::begin_ime_key(event);
                self.interpretKeyEvents(&NSArray::from_slice(&[event]));
                app::end_ime_key();
            } else {
                app::on_key(event);
            }
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _link: &CADisplayLink) {
            app::on_tick();
        }

        #[unsafe(method(benchCheck:))]
        fn bench_check(&self, _sender: Option<&AnyObject>) {
            app::on_bench_check();
        }

        #[unsafe(method(wake:))]
        fn wake(&self, _sender: Option<&AnyObject>) {
            app::on_wake();
        }
    }

    unsafe impl NSObjectProtocol for DocumentView {}

    unsafe impl NSTextInputClient for DocumentView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, _replacement: NSRange) {
            app::on_ime(Input::Text(text_of(string)));
        }

        /// A key the input method passed on (it composed nothing).
        #[unsafe(method(doCommandBySelector:))]
        fn do_command(&self, _selector: Sel) {
            app::on_ime_command();
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, string: &AnyObject, selected: NSRange, _replacement: NSRange) {
            let text = text_of(string);
            let cursor = input::utf16_to_byte(&text, selected.location);
            app::on_ime(Input::Preedit { text, cursor });
        }

        /// Accept the composition as typed (an input source switch, focus
        /// loss). After `insertText:` there is none left, so nothing.
        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            app::on_unmark();
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            NSRange::new(not_found(), 0)
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            match app::marked_utf16_len() {
                Some(n) => NSRange::new(0, n),
                None => NSRange::new(not_found(), 0),
            }
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            app::marked_utf16_len().is_some()
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(
            &self,
            _range: NSRange,
            _actual: *mut NSRange,
        ) -> Option<Retained<NSAttributedString>> {
            None
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            NSArray::new()
        }

        /// The caret, on screen: where the candidate window goes.
        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, _range: NSRange, _actual: *mut NSRange) -> NSRect {
            app::caret_screen_rect(self)
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, _point: NSPoint) -> NSUInteger {
            not_found()
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

    /// Calls `wake:` (an `Input::Tick` for the editor) after `delay_s`
    /// seconds.
    pub fn schedule_wake(&self, delay_s: f64) {
        let none: Option<&AnyObject> = None;
        // SAFETY: the view implements `wake:`, which takes an object.
        let _: () = unsafe {
            msg_send![self, performSelector: sel!(wake:), withObject: none, afterDelay: delay_s]
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

//! The general pasteboard, as `public.utf8-plain-text`.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

pub fn get() -> Option<String> {
    let pb = NSPasteboard::generalPasteboard();
    // SAFETY: reading a framework constant.
    let ty = unsafe { NSPasteboardTypeString };
    pb.stringForType(ty).map(|s| s.to_string())
}

pub fn set(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    // SAFETY: reading a framework constant.
    let ty = unsafe { NSPasteboardTypeString };
    pb.setString_forType(&NSString::from_str(text), ty);
}

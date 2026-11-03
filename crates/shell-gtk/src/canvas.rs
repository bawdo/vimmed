//! The document widget: a plain `gtk::Widget` subclass whose snapshot
//! appends one texture per strip of the frame. All the work happens in
//! `app::on_snapshot`; this file is the GObject plumbing and the texture
//! upload.

use editor_render::frame::Frame;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};

use crate::strips::{StripSet, StripTexture};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Canvas;

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "ShellStripCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Canvas {}

    impl WidgetImpl for Canvas {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            crate::app::on_snapshot(self.obj().upcast_ref::<gtk::Widget>(), snapshot);
        }
    }
}

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// A strip is an immutable memory texture of the frame's rows. The frame is
/// premultiplied RGBA, which GDK takes as is.
impl StripTexture for gdk::Texture {
    fn upload(frame: &Frame, top: u32, rows: u32) -> gdk::Texture {
        let stride = frame.width_px as usize * 4;
        let start = top as usize * stride;
        let data = &frame.pixmap.data()[start..start + rows as usize * stride];
        let bytes = glib::Bytes::from(data);
        gdk::MemoryTexture::new(
            frame.width_px as i32,
            rows as i32,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &bytes,
            stride,
        )
        .upcast()
    }
}

/// Appends every strip at its place in points: device pixels over `scale`.
pub fn append(snapshot: &gtk::Snapshot, strips: &StripSet<gdk::Texture>, frame: &Frame) {
    let s = frame.scale;
    let w = frame.width_px as f32 / s;
    for strip in strips.strips() {
        let r = graphene::Rect::new(0.0, strip.top as f32 / s, w, strip.rows as f32 / s);
        snapshot.append_texture(&strip.texture, &r);
    }
}

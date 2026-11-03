//! The clipboard. `gdk::Clipboard` reads are asynchronous and the editor
//! asks synchronously, so the shell keeps a copy of the text, refreshed
//! when the clipboard changes and when the window gains focus. Writes go
//! through at once and update the copy.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio};

pub struct Clipboard {
    board: gdk::Clipboard,
    text: Rc<RefCell<Option<String>>>,
}

fn refresh(board: &gdk::Clipboard, text: &Rc<RefCell<Option<String>>>) {
    let text = text.clone();
    board.read_text_async(None::<&gio::Cancellable>, move |r| {
        // A read fails when the clipboard holds no text: nothing to paste.
        *text.borrow_mut() = r.ok().flatten().map(|s| s.to_string());
    });
}

impl Clipboard {
    pub fn new(display: &gdk::Display) -> Clipboard {
        let board = display.clipboard();
        let text = Rc::new(RefCell::new(None));
        let t = text.clone();
        board.connect_changed(move |b| refresh(b, &t));
        refresh(&board, &text);
        Clipboard { board, text }
    }

    /// Re-reads the clipboard (the window gained focus).
    pub fn refresh(&self) {
        refresh(&self.board, &self.text);
    }

    pub fn get(&self) -> Option<String> {
        self.text.borrow().clone()
    }

    pub fn set(&self, s: &str) {
        self.board.set_text(s);
        *self.text.borrow_mut() = Some(s.to_string());
    }
}

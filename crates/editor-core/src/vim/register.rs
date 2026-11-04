//! Registers: the unnamed register that every yank, delete and change
//! writes, and the named registers `"a` to `"z`.
//!
//! The system clipboard mirrors the unnamed register. That sync lives in
//! `EditCtx` (which owns the host); this module only remembers the last
//! text written to the clipboard so a put can tell our own text from text
//! another app copied. The named registers never touch the clipboard; with
//! a `RegisterStore` they are shared with every other window.

use crate::file::RegisterStore;

/// What a register holds. A linewise register's text is whole lines, each
/// ending in `\n`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

impl Register {
    pub fn chars(text: impl Into<String>) -> Register {
        Register {
            text: text.into(),
            linewise: false,
        }
    }

    pub fn lines(text: impl Into<String>) -> Register {
        Register {
            text: text.into(),
            linewise: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct Registers {
    unnamed: Register,
    /// `"a` to `"z`, by letter, while no store shares them. TODO(later
    /// tasks): appending with `"A`, the numbered and small-delete
    /// registers.
    named: [Option<Register>; 26],
    /// The store that shares `"a` to `"z` with other windows.
    store: Option<RegisterStore>,
    /// What we last wrote to the system clipboard, text and shape.
    clipboard: Option<Register>,
    /// A named register the store could not write since the last
    /// `take_unshared`.
    unshared: bool,
}

impl Registers {
    pub fn new() -> Registers {
        Registers::default()
    }

    /// Keeps `"a` to `"z` in `store` from now on.
    pub fn share(&mut self, store: RegisterStore) {
        self.store = Some(store);
    }

    /// The unnamed register: empty until something is yanked or deleted.
    pub fn unnamed(&self) -> &Register {
        &self.unnamed
    }

    /// Stores `reg` in the unnamed register (`name` is `None`) or in a
    /// named one. Other names are ignored. A store that cannot write keeps
    /// the register for this window only, and `take_unshared` says so.
    pub fn set(&mut self, name: Option<char>, reg: Register) {
        match name.map(slot) {
            None => self.unnamed = reg,
            Some(None) => {}
            Some(Some(i)) => match self.store.as_mut() {
                Some(store) => {
                    if store.set(letter(i), reg).is_err() {
                        self.unshared = true;
                    }
                }
                None => self.named[i] = Some(reg),
            },
        }
    }

    /// The register's contents, or `None` when it holds nothing.
    pub fn get(&mut self, name: Option<char>) -> Option<Register> {
        let reg = match name.map(slot) {
            None => Some(self.unnamed.clone()),
            Some(None) => None,
            Some(Some(i)) => match self.store.as_mut() {
                Some(store) => store.get(letter(i)),
                None => self.named[i].clone(),
            },
        };
        reg.filter(|r| !r.text.is_empty())
    }

    /// True once after the store found its file unreadable and reset the
    /// named registers.
    pub fn take_reset(&mut self) -> bool {
        self.store.as_mut().is_some_and(RegisterStore::take_reset)
    }

    /// True once after a named register could not be written to the
    /// store, so other windows do not have it.
    pub fn take_unshared(&mut self) -> bool {
        std::mem::take(&mut self.unshared)
    }

    /// Remembers the register just written to the system clipboard, text
    /// and shape.
    pub fn wrote_clipboard(&mut self, reg: &Register) {
        self.clipboard = Some(reg.clone());
    }

    /// What a put should use given the clipboard's current text: our own
    /// text keeps the linewise flag it was written with (a named write since
    /// may have given the unnamed register other text); text from elsewhere
    /// is linewise when it ends with a newline. With no clipboard, the
    /// unnamed register. `None` when there is nothing to put.
    pub fn from_clipboard(&self, clip: Option<String>) -> Option<Register> {
        let Some(text) = clip else {
            return Some(self.unnamed.clone()).filter(|r| !r.text.is_empty());
        };
        if text.is_empty() {
            return None;
        }
        let linewise = match &self.clipboard {
            Some(ours) if ours.text == text => ours.linewise,
            _ => text.ends_with('\n'),
        };
        Some(Register { text, linewise })
    }
}

/// The index of a named register, `a` to `z` in either case.
fn slot(c: char) -> Option<usize> {
    c.is_ascii_alphabetic()
        .then(|| (c.to_ascii_lowercase() as u8 - b'a') as usize)
}

/// The name of the register at `slot`.
fn letter(slot: usize) -> char {
    char::from(b'a' + slot as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    // registers_test.dart: VimRegister.

    #[test]
    fn a_register_stores_text_and_the_linewise_flag() {
        let r = Register::lines("hello world");
        assert_eq!(r.text, "hello world");
        assert!(r.linewise);
        assert!(!Register::chars("test").linewise);
        assert!(Register::default().text.is_empty());
    }

    // registers_test.dart: VimRegisters.

    #[test]
    fn the_unnamed_register_starts_empty() {
        let mut regs = Registers::new();
        assert!(regs.unnamed().text.is_empty());
        assert_eq!(regs.get(None), None);
    }

    #[test]
    fn set_stores_in_the_unnamed_register_and_overwrites() {
        let mut regs = Registers::new();
        regs.set(None, Register::chars("yanked text"));
        assert_eq!(regs.unnamed().text, "yanked text");
        regs.set(None, Register::lines("line content\n"));
        assert!(regs.unnamed().linewise);
        regs.set(None, Register::chars("first"));
        regs.set(None, Register::chars("second"));
        assert_eq!(regs.get(None), Some(Register::chars("second")));
    }

    #[test]
    fn clearing_resets_to_empty() {
        let mut regs = Registers::new();
        regs.set(None, Register::chars("some content"));
        regs.set(None, Register::default());
        assert_eq!(regs.get(None), None);
    }

    // Without a store, the named registers live in memory, apart from the
    // unnamed one.

    #[test]
    fn named_registers_are_separate_from_the_unnamed_one() {
        let mut regs = Registers::new();
        regs.set(Some('a'), Register::chars("x"));
        assert_eq!(regs.get(Some('a')), Some(Register::chars("x")));
        assert_eq!(regs.get(Some('A')), Some(Register::chars("x")));
        assert_eq!(regs.get(None), None);
        assert_eq!(regs.get(Some('b')), None);
        regs.set(Some('1'), Register::chars("ignored"));
        assert_eq!(regs.get(Some('1')), None);
    }

    #[test]
    fn our_own_clipboard_text_keeps_the_linewise_flag() {
        let mut regs = Registers::new();
        regs.set(None, Register::lines("a\n"));
        regs.wrote_clipboard(&Register::lines("a\n"));
        assert_eq!(
            regs.from_clipboard(Some("a\n".into())),
            Some(Register::lines("a\n"))
        );
        regs.set(None, Register::chars("b\n"));
        regs.wrote_clipboard(&Register::chars("b\n"));
        assert_eq!(
            regs.from_clipboard(Some("b\n".into())),
            Some(Register::chars("b\n"))
        );
    }

    #[test]
    fn other_clipboard_text_is_linewise_when_it_ends_with_a_newline() {
        let regs = Registers::new();
        assert_eq!(
            regs.from_clipboard(Some("z\n".into())),
            Some(Register::lines("z\n"))
        );
        assert_eq!(
            regs.from_clipboard(Some("z".into())),
            Some(Register::chars("z"))
        );
        assert_eq!(regs.from_clipboard(Some(String::new())), None);
    }

    // `"a` to `"z` before the operators and puts (checked against vim 9.1:
    // a named write fills the unnamed register too).

    use crate::file::StateDir;
    use crate::testkit::*;

    #[test]
    fn a_named_register_comes_before_every_operator_and_put() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut e, host) = ed_with_state("one two\nthree four\nfive", &StateDir::at(tmp.path()));
        host.set_clipboard("clip");
        keys(&mut e, "\"ayy\"bx\"cdw\"dyw");
        assert_eq!(e.register(Some('a')), Some(Register::lines("one two\n")));
        assert_eq!(e.register(Some('b')), Some(Register::chars("o")));
        assert_eq!(e.register(Some('c')), Some(Register::chars("ne ")));
        assert_eq!(e.register(Some('d')), Some(Register::chars("two")));
        assert_eq!(e.register(None), Some(Register::chars("two")));
        keys(&mut e, "j0\"eD");
        assert_eq!(e.register(Some('e')), Some(Register::chars("three four")));
        keys(&mut e, "j0\"fCnew<Esc>");
        assert_eq!(e.register(Some('f')), Some(Register::chars("five")));
        assert_eq!(e.text(), "two\n\nnew");
        // `"ap` and `"aP` put the register, not the clipboard.
        keys(&mut e, "gg\"ap");
        assert_eq!(e.text(), "two\none two\n\nnew");
        keys(&mut e, "\"bP");
        assert_eq!(e.text(), "two\noone two\n\nnew");
        assert_eq!(host.clipboard(), "clip");
    }

    #[test]
    fn counts_go_either_side_of_the_register() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut e, _host) = ed_with_state("a\nb\nc", &StateDir::at(tmp.path()));
        keys(&mut e, "2\"ayy\"b2yy");
        assert_eq!(e.register(Some('a')), Some(Register::lines("a\nb\n")));
        assert_eq!(e.register(Some('b')), Some(Register::lines("a\nb\n")));
        keys(&mut e, "\"a");
        assert_eq!(e.showcmd(), "\"a");
        keys(&mut e, "2d");
        assert_eq!(e.showcmd(), "\"a2d");
    }

    #[test]
    fn a_named_register_works_in_visual_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut e, host) = ed_with_state("hello\nworld", &StateDir::at(tmp.path()));
        host.set_clipboard("clip");
        keys(&mut e, "vl\"ay");
        assert_eq!(e.register(Some('a')), Some(Register::chars("he")));
        keys(&mut e, "jV\"bd");
        assert_eq!(e.register(Some('b')), Some(Register::lines("world\n")));
        assert_eq!(e.text(), "hello");
        assert_eq!(host.clipboard(), "clip");
    }

    #[test]
    fn a_register_with_nothing_after_it_is_forgotten() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut e, host) = ed_with_state("ab\ncd", &StateDir::at(tmp.path()));
        keys(&mut e, "\"a<Esc>yy");
        assert_eq!(e.register(Some('a')), None);
        assert_eq!(host.clipboard(), "ab\n");
        keys(&mut e, "\"aj");
        keys(&mut e, "yy");
        assert_eq!(e.register(Some('a')), None);
        assert_eq!(host.clipboard(), "cd\n");
        // Only `a` to `z` name a register.
        keys(&mut e, "\"1yy");
        assert_eq!(e.showcmd(), "");
        assert_eq!(e.register(Some('1')), None);
    }

    #[test]
    fn a_named_delete_before_a_search_goes_to_the_register() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut e, host) = ed_with_state("one two three", &StateDir::at(tmp.path()));
        host.set_clipboard("clip");
        keys(&mut e, "\"ad/thr<CR>");
        assert_eq!(e.text(), "three");
        assert_eq!(e.register(Some('a')), Some(Register::chars("one two ")));
        assert_eq!(host.clipboard(), "clip");
    }

    // Fix round 1: a named write moves the unnamed register on but leaves
    // the clipboard, so a plain put must take the clipboard's own shape.

    #[test]
    fn a_put_after_a_named_charwise_write_keeps_the_clipboard_linewise() {
        let (mut e, _host) = ed_with_host("foo\nbar baz");
        keys(&mut e, "yyj\"ayw0p");
        assert_eq!(e.text(), "foo\nbar baz\nfoo");
    }

    #[test]
    fn a_put_after_a_named_linewise_write_keeps_the_clipboard_charwise() {
        let (mut e, _host) = ed_with_host("foo\nbar baz");
        keys(&mut e, "ywj\"ayyp");
        assert_eq!(e.text(), "foo\nbfooar baz");
    }

    // vim 9.1: `"add.` on one/two/three leaves `a` holding "two\n".

    #[test]
    fn repeating_a_named_delete_writes_the_register_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut e, host) = ed_with_state("one\ntwo\nthree", &StateDir::at(tmp.path()));
        host.set_clipboard("clip");
        keys(&mut e, "\"add.");
        assert_eq!(e.text(), "three");
        assert_eq!(e.register(Some('a')), Some(Register::lines("two\n")));
        assert_eq!(host.clipboard(), "clip");
        // A plain delete after it records no register.
        keys(&mut e, "x.");
        assert_eq!(e.register(Some('a')), Some(Register::lines("two\n")));
        assert_eq!(host.clipboard(), "h");
    }

    #[test]
    fn with_no_state_directory_named_registers_last_the_session() {
        use crate::editor::{Config, Editor};
        use crate::platform::Platform;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("new.md");
        let mut e = Editor::open_with_state(
            req(path.to_str().unwrap()),
            Config::default(),
            Platform::Mac,
            Box::new(FakeHost::default()),
            None,
        );
        keys(&mut e, "ihi<Esc>\"ayy\"ap");
        assert_eq!(e.text(), "hi\nhi");
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_named_write_the_store_cannot_make_warns_and_still_works_here() {
        use crate::view_model::{MsgKind, StatusLeft};
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        // A regular file sits where the state dir must be made, so creation fails even for root.
        std::fs::write(locked.join("state"), b"").unwrap();
        let (mut e, host) = ed_with_state("hello", &StateDir::at(locked.join("state")));
        host.state().now = 1_000;
        let fx = keys(&mut e, "\"ayy");
        assert_eq!(
            e.status().left,
            StatusLeft::Message {
                text: crate::strings::NAMED_REGISTER_NOT_SHARED.to_owned(),
                kind: MsgKind::Warning,
            }
        );
        assert_eq!(fx.wake_at, Some(1_000 + crate::status::MESSAGE_FOR_NS));
        assert_eq!(e.register(Some('a')), Some(Register::lines("hello\n")));
        keys(&mut e, "\"ap");
        assert_eq!(e.text(), "hello\nhello");
    }

    #[test]
    fn without_a_clipboard_the_unnamed_register_is_used() {
        let mut regs = Registers::new();
        assert_eq!(regs.from_clipboard(None), None);
        regs.set(None, Register::lines("q\n"));
        assert_eq!(regs.from_clipboard(None), Some(Register::lines("q\n")));
    }
}

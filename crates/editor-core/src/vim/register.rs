//! Registers: the unnamed register that every yank, delete and change
//! writes, and the named registers `"a` to `"z` (a stub until Task 23).
//!
//! The system clipboard mirrors the unnamed register. That sync lives in
//! `EditCtx` (which owns the host); this module only remembers the last
//! text written to the clipboard so a put can tell our own text from text
//! another app copied.

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
    /// `"a` to `"z`, by letter. TODO(Task 23): appending with `"A`, the
    /// numbered and small-delete registers.
    named: [Option<Register>; 26],
    /// The text we last wrote to the system clipboard.
    clipboard: Option<String>,
}

impl Registers {
    pub fn new() -> Registers {
        Registers::default()
    }

    /// The unnamed register: empty until something is yanked or deleted.
    pub fn unnamed(&self) -> &Register {
        &self.unnamed
    }

    /// Stores `reg` in the unnamed register (`name` is `None`) or in a
    /// named one. Other names are ignored.
    pub fn set(&mut self, name: Option<char>, reg: Register) {
        match name {
            None => self.unnamed = reg,
            Some(c) => {
                if let Some(slot) = slot(c) {
                    self.named[slot] = Some(reg);
                }
            }
        }
    }

    /// The register's contents, or `None` when it holds nothing.
    pub fn get(&self, name: Option<char>) -> Option<Register> {
        let reg = match name {
            None => Some(&self.unnamed),
            Some(c) => slot(c).and_then(|i| self.named[i].as_ref()),
        };
        reg.filter(|r| !r.text.is_empty()).cloned()
    }

    /// Remembers the text just written to the system clipboard.
    pub fn wrote_clipboard(&mut self, text: &str) {
        self.clipboard = Some(text.to_owned());
    }

    /// What a put should use given the clipboard's current text: our own
    /// text keeps the unnamed register's linewise flag; text from elsewhere
    /// is linewise when it ends with a newline. With no clipboard, the
    /// unnamed register. `None` when there is nothing to put.
    pub fn from_clipboard(&self, clip: Option<String>) -> Option<Register> {
        let Some(text) = clip else {
            return self.get(None);
        };
        if text.is_empty() {
            return None;
        }
        let linewise = if self.clipboard.as_deref() == Some(text.as_str()) {
            self.unnamed.linewise
        } else {
            text.ends_with('\n')
        };
        Some(Register { text, linewise })
    }
}

/// The index of a named register, `a` to `z` in either case.
fn slot(c: char) -> Option<usize> {
    c.is_ascii_alphabetic()
        .then(|| (c.to_ascii_lowercase() as u8 - b'a') as usize)
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
        let regs = Registers::new();
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

    // Named registers are a stub, but they keep what they are given apart
    // from the unnamed one.

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
        regs.wrote_clipboard("a\n");
        assert_eq!(
            regs.from_clipboard(Some("a\n".into())),
            Some(Register::lines("a\n"))
        );
        regs.set(None, Register::chars("b\n"));
        regs.wrote_clipboard("b\n");
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

    #[test]
    fn without_a_clipboard_the_unnamed_register_is_used() {
        let mut regs = Registers::new();
        assert_eq!(regs.from_clipboard(None), None);
        regs.set(None, Register::lines("q\n"));
        assert_eq!(regs.from_clipboard(None), Some(Register::lines("q\n")));
    }
}

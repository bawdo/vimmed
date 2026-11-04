//! Links: opening one (`open_link`) and the link prompt Primary+L opens.
//!
//! `open_link` resolves a target against the document's directory
//! (`crate::links`): a URL, or a local file that is not markdown, goes to
//! the system handler (`Host::open_external`); a markdown file opens in a
//! new window of its own (`Host::spawn_window`), tied to no launcher; a
//! file that is not there says so. A bare `#fragment` points into this
//! document and does nothing in v0.9 (no jump to the heading yet). Links
//! open in reader view too.
//!
//! Primary+L, in Normal, Insert, Visual and Visual Line (and with vim
//! off, Primary+K too), opens the command line as the link prompt
//! `link: `. It reuses the command line's typing (`cmdline`): the line is
//! `self.cmdline`, in Command mode, with `linking` saying what Enter does.
//! Enter puts in `[<selection>](<input>)` over a selection, back in Normal
//! with the cursor on the `)`, or `[](<input>)` at the cursor with no
//! selection, in Insert with the cursor inside the `[]`. With vim off
//! there is no Normal: over a selection the cursor goes after the `)`,
//! still typing. The input is trimmed. The insertion is one undo step
//! (text typed into the `[]` afterwards is the Insert session's own). Esc,
//! Backspace on an empty line, or Enter on a blank one cancel, back to the
//! mode, cursor and selection the prompt came from. Reader view is
//! read-only, so Primary+L does nothing there.

use std::ops::Range;
use std::path::PathBuf;

use crate::buffer::Pos;
use crate::effects::{Effects, Redraw};
use crate::host::WindowRequest;
use crate::links::{self, LinkTarget};
use crate::status::{self, Mode};
use crate::strings;
use crate::view_model::MsgKind;
use crate::vim::operator::OpOutcome;
use crate::vim::{self, visual};

use super::Editor;

/// The link prompt being typed, and what Enter or Esc goes back to.
pub(super) struct Linking {
    /// What the link wraps: the selection's text, or an empty range at the
    /// cursor.
    range: Range<Pos>,
    /// True when the prompt was opened over a selection.
    selected: bool,
    /// The mode, cursor and selection anchor Esc restores.
    ret: Mode,
    cursor: Pos,
    anchor: Option<Pos>,
}

impl Editor {
    /// Opens a link's target (the `(...)` of `[text](target)`), as a click
    /// on a link does. A bare `#fragment` does nothing. A target that is
    /// not there shows `no such file: <path>`; the effects then ask for the
    /// status repaint and the timer that fades it, so this can be called
    /// from outside `handle`.
    pub fn open_link(&mut self, target: &str) -> Effects {
        if links::is_self_fragment(target) {
            return Effects::default();
        }
        match links::classify(target, &self.link_dir()) {
            LinkTarget::External(url) => self.host.open_external(&url),
            LinkTarget::Local(path) => self.host.open_external(&path.to_string_lossy()),
            LinkTarget::Markdown(path) => self.host.spawn_window(WindowRequest {
                path,
                view: false,
                display_name: None,
            }),
            LinkTarget::Missing(path) => {
                let text = format!("{}{}", strings::NO_SUCH_FILE, path.display());
                self.say_as(text, MsgKind::Error, status::MESSAGE_FOR_NS);
                return Effects {
                    redraw: Redraw::Status,
                    wake_at: self.message_until,
                    ..Effects::default()
                };
            }
        }
        Effects::default()
    }

    /// The directory links resolve against: the file's, made absolute
    /// against the process's cwd (so a spawned window or the system handler
    /// gets a path that does not depend on its own cwd), or the cwd itself
    /// for an unnamed buffer.
    fn link_dir(&self) -> PathBuf {
        let parent = self.path.parent().unwrap_or(&self.path);
        if parent.is_absolute() {
            return parent.to_owned();
        }
        let cwd = std::env::current_dir().unwrap_or_default();
        cwd.join(parent)
    }

    /// Primary+L: the link prompt, over the selection in Visual, else at
    /// the cursor. Only from Normal, Insert and Visual: a line already
    /// being typed, a question being asked and reader view keep the key.
    pub(super) fn start_link(&mut self) -> Effects {
        let editing = matches!(
            self.mode,
            Mode::Normal | Mode::Insert | Mode::Visual | Mode::VisualLine
        );
        if self.refuses_edits() || !editing {
            return Effects::default();
        }
        let before = self.selection_lines();
        let selection = self.shown_selection().filter(|_| self.visual());
        // Vim off, the selection Shift and the arrows made.
        let plain = self.plain_range().filter(|_| !self.cfg.vim);
        let selected = selection.is_some() || plain.is_some();
        let range = match (selection, plain) {
            (Some(sel), _) => visual::paste_range(&self.buf, sel, &mut String::new()),
            (None, Some(r)) => r,
            (None, None) => {
                let at = self.buf.clamp(self.cursor);
                at..at
            }
        };
        self.vim.cancel();
        let fx = self.clear_preedit();
        self.linking = Some(Linking {
            range,
            selected,
            ret: self.mode,
            cursor: self.cursor,
            anchor: self.anchor,
        });
        self.anchor = None;
        self.cmdline.clear();
        self.unsay();
        self.mode = Mode::Command;
        fx.merge(self.selection_redraw(before)).merge(Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        })
    }

    /// True while the command line is the link prompt.
    pub(super) fn linking(&self) -> bool {
        self.linking.is_some()
    }

    /// Esc on the link prompt (or Backspace on its empty line, or Enter on
    /// a blank one): back to the mode, cursor and selection it came from.
    pub(super) fn cancel_link(&mut self) -> Effects {
        let Some(l) = self.linking.take() else {
            return Effects::default();
        };
        self.cmdline.clear();
        self.mode = l.ret;
        self.cursor = l.cursor;
        self.anchor = l.anchor;
        self.selection_redraw(None).merge(Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        })
    }

    /// Enter on the link prompt: the link goes in as one undo step.
    pub(super) fn finish_link(&mut self) -> Effects {
        let input = self.cmdline.trim().to_owned();
        if input.is_empty() {
            return self.cancel_link();
        }
        let Some(l) = self.linking.take() else {
            return Effects::default();
        };
        self.cmdline.clear();
        let range = self.buf.clamp(l.range.start)..self.buf.clamp(l.range.end);
        let label = self.buf.text_range(range.clone());
        let text = format!("[{label}]({input})");
        // In a vim Insert session the link is a step of its own, as an
        // arrow key makes one; the session goes on after it.
        if self.cfg.vim && l.ret == Mode::Insert {
            self.history.end();
            self.vim.arrowed();
        }
        let insert = !l.selected;
        let vim_on = self.cfg.vim;
        self.mode = if insert || !vim_on {
            Mode::Insert
        } else {
            Mode::Normal
        };
        let fx = self.run(|ctx| {
            let start = range.start;
            let end = ctx.replace(range, &text).range.end;
            let cursor = if insert {
                // Inside the `[]`: `[` is one byte.
                Pos {
                    col: start.col + 1,
                    ..start
                }
            } else if !vim_on {
                // After the `)`, typing on.
                end
            } else {
                // On the `)` (one byte), where Normal mode rests after a put.
                vim::rest(
                    ctx.buf,
                    Pos {
                        col: end.col.saturating_sub(1),
                        ..end
                    },
                )
            };
            OpOutcome {
                cursor,
                insert: false,
            }
        });
        // The Insert session the link leaves the cursor in, which Esc ends.
        // No change is pending for it (`start_link` cancelled the engine),
        // so the link and the label typed into it are not what `.`
        // repeats: `.` still repeats the change before the link. Recording
        // the label as an `i` change would make `.` type the bare label
        // with no link round it, which is worse.
        if insert && self.cfg.vim {
            self.history.begin();
        }
        fx.merge(Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::effects::Effects;
    use crate::host::WindowRequest;
    use crate::input::Input;
    use crate::status::Mode;
    use crate::strings;
    use crate::testkit::{
        ViewSource, build_reader, ed, ed_at, ed_linux_with_host, keys, plain_with_host, pos,
    };
    use crate::view_model::{MsgKind, StatusLeft};

    /// A directory of its own for one test, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("abc_def-links-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        /// Writes an empty file `name` in the directory, returning its path.
        fn file(&self, name: &str) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, "").unwrap();
            p
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn prompt_line(e: &crate::editor::Editor) -> Option<String> {
        match e.status().left {
            StatusLeft::Input { prefix, text, .. } if prefix == strings::LINK_PROMPT => Some(text),
            _ => None,
        }
    }

    #[test]
    fn md_link_spawns_untied_window() {
        let dir = Scratch::new("md");
        let other = dir.file("other.md");
        let (mut e, host) = ed_at(&dir.path().join("notes.md"), "", false);
        e.open_link("other.md#part");
        assert_eq!(
            host.state().spawned,
            [WindowRequest {
                path: other,
                view: false,
                display_name: None,
            }]
        );
        assert!(host.state().opened.is_empty());
        assert_eq!(e.message(), None);
    }

    #[test]
    fn external_and_local_go_to_open_external() {
        let dir = Scratch::new("ext");
        let data = dir.file("data.txt");
        let (mut e, host) = ed_at(&dir.path().join("notes.md"), "", false);
        e.open_link("https://example.com/a#b");
        e.open_link("data.txt");
        assert_eq!(
            host.state().opened,
            [
                "https://example.com/a#b".to_owned(),
                data.to_string_lossy().into_owned()
            ]
        );
        assert!(host.state().spawned.is_empty());
        assert_eq!(e.message(), None);
    }

    #[test]
    fn missing_target_says_no_such_file() {
        let dir = Scratch::new("missing");
        let (mut e, host) = ed_at(&dir.path().join("notes.md"), "", false);
        let fx = e.open_link("gone.md");
        let want = format!(
            "{}{}",
            strings::NO_SUCH_FILE,
            dir.path().join("gone.md").display()
        );
        assert_eq!(e.message(), Some(want.as_str()));
        assert_eq!(
            e.status().left,
            StatusLeft::Message {
                text: want,
                kind: MsgKind::Error,
            }
        );
        // Called from outside `handle` (a click), it still asks for the
        // status repaint and the timer that fades the message.
        assert_ne!(fx.redraw, crate::effects::Redraw::None);
        assert!(fx.wake_at.is_some());
        assert!(host.state().opened.is_empty());
        assert!(host.state().spawned.is_empty());
    }

    #[test]
    fn self_fragment_is_a_no_op() {
        let dir = Scratch::new("frag");
        let (mut e, host) = ed_at(&dir.path().join("notes.md"), "", false);
        assert_eq!(e.open_link("#heading"), Effects::default());
        assert_eq!(e.open_link(""), Effects::default());
        assert_eq!(e.message(), None);
        assert!(host.state().opened.is_empty());
        assert!(host.state().spawned.is_empty());
    }

    #[test]
    fn open_link_from_an_unnamed_buffer_resolves_against_cwd() {
        // cargo runs the tests in the crate's directory, which has a
        // Cargo.toml.
        let (mut e, host) = ed_at(Path::new(""), "", false);
        e.open_link("Cargo.toml");
        let want = std::env::current_dir().unwrap().join("Cargo.toml");
        assert_eq!(host.state().opened, [want.to_string_lossy().into_owned()]);
    }

    #[test]
    fn a_relative_file_path_resolves_to_an_absolute_one() {
        // A window started as `x docs/notes.md`: the spawned window and the
        // system handler get an absolute path, whatever their cwd.
        let (mut e, host) = ed_at(Path::new("src/notes.md"), "", false);
        e.open_link("lib.rs");
        let want = std::env::current_dir().unwrap().join("src/lib.rs");
        assert_eq!(host.state().opened, [want.to_string_lossy().into_owned()]);
    }

    #[test]
    fn links_open_in_reader_view() {
        let dir = Scratch::new("reader");
        let other = dir.file("other.md");
        let (mut e, host) = ed_at(&dir.path().join("notes.md"), "", true);
        e.open_link("other.md");
        assert_eq!(host.state().spawned.len(), 1);
        assert_eq!(host.state().spawned[0].path, other);
    }

    #[test]
    fn link_prompt_wraps_selection_and_is_one_undo_step() {
        let mut e = ed("hello world");
        keys(&mut e, "ve<D-l>");
        assert_eq!(prompt_line(&e), Some(String::new()));
        assert_eq!(e.text(), "hello world");
        keys(&mut e, "https://x.y");
        assert_eq!(prompt_line(&e), Some("https://x.y".into()));
        keys(&mut e, "<CR>");
        assert_eq!(e.text(), "[hello](https://x.y) world");
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(prompt_line(&e), None);
        // On the link's last character, as a put leaves it.
        assert_eq!(e.cursor(), pos(0, 19));
        assert!(e.dirty());
        keys(&mut e, "u");
        assert_eq!(e.text(), "hello world");
        assert!(!e.dirty());
        keys(&mut e, "<C-r>");
        assert_eq!(e.text(), "[hello](https://x.y) world");
    }

    #[test]
    fn link_prompt_wraps_whole_graphemes() {
        let mut e = ed("a\u{1F44D}\u{1F3FD}b");
        keys(&mut e, "lv<D-l>u<CR>");
        assert_eq!(e.text(), "a[\u{1F44D}\u{1F3FD}](u)b");
    }

    #[test]
    fn link_prompt_wraps_a_line_selection() {
        let mut e = ed("one\ntwo\nthree");
        keys(&mut e, "Vj<D-l>u<CR>");
        assert_eq!(e.text(), "[one\ntwo](u)\nthree");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn empty_selection_form_puts_the_cursor_inside_brackets_in_insert() {
        let mut e = ed("ab");
        keys(&mut e, "l<D-l>u<CR>");
        assert_eq!(e.text(), "a[](u)b");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), pos(0, 2));
        keys(&mut e, "t<Esc>");
        assert_eq!(e.text(), "a[t](u)b");
        // The link was one step and the text typed in it another.
        keys(&mut e, "u");
        assert_eq!(e.text(), "a[](u)b");
        keys(&mut e, "u");
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn link_prompt_from_insert_stays_in_insert() {
        let mut e = ed("ab");
        keys(&mut e, "Axy<D-l>u<CR>z<Esc>");
        assert_eq!(e.text(), "abxy[z](u)");
        keys(&mut e, "u");
        assert_eq!(e.text(), "abxy[](u)");
        keys(&mut e, "u");
        assert_eq!(e.text(), "abxy");
    }

    #[test]
    fn esc_cancels_the_link_prompt() {
        let mut e = ed("hello world");
        keys(&mut e, "ve<D-l>abc<Esc>");
        assert_eq!(e.text(), "hello world");
        assert_eq!(prompt_line(&e), None);
        // Back to the selection it came from.
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(e.cursor(), pos(0, 4));
        keys(&mut e, "<Esc>i<D-l>x<Esc>");
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.text(), "hello world");
        assert!(!e.dirty());
    }

    #[test]
    fn backspace_on_an_empty_link_prompt_cancels() {
        let mut e = ed("ab");
        keys(&mut e, "<D-l>x<BS><BS>");
        assert_eq!(prompt_line(&e), None);
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn enter_on_an_empty_link_prompt_cancels() {
        let mut e = ed("ab");
        keys(&mut e, "<D-l>  <CR>");
        assert_eq!(prompt_line(&e), None);
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.text(), "ab");
        assert!(!e.dirty());
    }

    #[test]
    fn primary_l_in_reader_does_nothing() {
        let mut e = build_reader("ab");
        keys(&mut e, "<D-l>");
        assert_eq!(prompt_line(&e), None);
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "u<CR>");
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn primary_l_works_with_vim_off() {
        let (mut e, _host) = plain_with_host("ab");
        keys(&mut e, "<D-l>");
        assert_eq!(prompt_line(&e), Some(String::new()));
        assert!(e.ime_active());
        e.handle(Input::Text("u".into()));
        assert_eq!(prompt_line(&e), Some("u".into()));
        keys(&mut e, "<CR>");
        assert_eq!(e.text(), "[](u)ab");
        assert_eq!(e.cursor(), pos(0, 1));
        keys(&mut e, "t");
        assert_eq!(e.text(), "[t](u)ab");
    }

    #[test]
    fn a_composition_shows_on_the_link_prompt_with_vim_off() {
        let (mut e, _host) = plain_with_host("ab");
        keys(&mut e, "<D-l>u");
        let fx = e.handle(Input::Preedit {
            text: "\u{e9}".into(),
            cursor: 2,
        });
        assert_eq!(fx.redraw, crate::effects::Redraw::Status);
        assert_eq!(prompt_line(&e), Some("u\u{e9}".into()));
        assert_eq!(ViewSource::preedit(&e), None);
    }

    #[test]
    fn primary_v_pastes_into_the_link_prompt() {
        let (mut e, host) = crate::testkit::ed_with_host("ab");
        host.set_clipboard("https://x.y/\r\n");
        keys(&mut e, "<D-l>u<D-v>");
        assert_eq!(prompt_line(&e), Some("uhttps://x.y/".into()));
        assert_eq!(e.text(), "ab");
        keys(&mut e, "<CR>");
        assert_eq!(e.text(), "[](uhttps://x.y/)ab");
    }

    #[test]
    fn dot_after_a_link_repeats_the_change_before_it() {
        let mut e = ed("abcd");
        keys(&mut e, "x<D-l>u<CR>t<Esc>");
        assert_eq!(e.text(), "[t](u)bcd");
        keys(&mut e, "$.");
        assert_eq!(e.text(), "[t](u)bc");
    }

    #[test]
    fn ctrl_space_does_not_edit_under_the_link_prompt() {
        // Vim off: Ctrl+Space would toggle the task as typing.
        let (mut e, _host) = plain_with_host("- [ ] a");
        keys(&mut e, "<D-l><C-Space>");
        assert_eq!(e.text(), "- [ ] a");
        assert_eq!(prompt_line(&e), Some(String::new()));
        keys(&mut e, "u<CR>");
        assert_eq!(e.text(), "[](u)- [ ] a");
        // Vim on, from Insert.
        let mut e = ed("- [ ] a");
        keys(&mut e, "i<D-l><C-Space>");
        assert_eq!(e.text(), "- [ ] a");
        assert_eq!(prompt_line(&e), Some(String::new()));
        keys(&mut e, "u<CR>");
        assert_eq!(e.text(), "[](u)- [ ] a");
    }

    #[test]
    fn ctrl_l_is_the_link_key_on_linux() {
        let (mut e, _host) = ed_linux_with_host("ab");
        keys(&mut e, "<C-l>u<CR>");
        assert_eq!(e.text(), "[](u)ab");
    }

    #[test]
    fn link_prompt_input_is_trimmed_and_committed_text_types_into_it() {
        let mut e = ed("ab");
        keys(&mut e, "<D-l>");
        e.handle(Input::Text(" u v\n".into()));
        assert_eq!(prompt_line(&e), Some(" u v".into()));
        keys(&mut e, "<CR>");
        assert_eq!(e.text(), "[](u v)ab");
    }

    #[test]
    fn primary_l_on_the_command_line_does_nothing() {
        let mut e = ed("ab");
        keys(&mut e, ":<D-l>");
        assert_eq!(e.mode(), Mode::Command);
        assert_eq!(prompt_line(&e), None);
    }
}

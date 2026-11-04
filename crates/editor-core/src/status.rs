//! The editing mode and how the status bar shows it, the cursor and how
//! long a message stays.

use crate::buffer::{Buffer, Pos};
use crate::grapheme;
use crate::strings;

pub use crate::vim::Mode;

impl Mode {
    /// The name shown in the status bar. Search, Command, Confirm and
    /// Prompt show their line or question there instead (`StatusLeft`).
    pub fn name(self) -> &'static str {
        match self {
            Mode::Normal => strings::MODE_NORMAL,
            Mode::Insert => strings::MODE_INSERT,
            Mode::Visual => strings::MODE_VISUAL,
            Mode::VisualLine => strings::MODE_VISUAL_LINE,
            Mode::Search => strings::MODE_SEARCH,
            Mode::Command => strings::MODE_COMMAND,
            Mode::Confirm => strings::MODE_CONFIRM,
            Mode::Prompt => strings::MODE_PROMPT,
        }
    }
}

/// How long a message shows before it fades: six seconds.
pub const MESSAGE_FOR_NS: u64 = 6_000_000_000;

/// The cursor as shown, `line:column`, both 1-based, counting graphemes.
pub fn position(buf: &Buffer, cursor: Pos) -> String {
    let col = grapheme::display_col(buf.line(cursor.line), cursor.col);
    format!("{}:{}", cursor.line + 1, col + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::Redraw;
    use crate::input::Input;
    use crate::testkit::{ed, ed_with_host, keys};
    use crate::view_model::{CaretShape, MsgKind, StatusLeft, ViewSource};

    fn left(e: &crate::editor::Editor) -> StatusLeft {
        e.status().left
    }

    #[test]
    fn mode_labels() {
        assert_eq!(Mode::Normal.name(), "NORMAL");
        assert_eq!(Mode::Insert.name(), "INSERT");
        assert_eq!(Mode::Visual.name(), "VISUAL");
        assert_eq!(Mode::VisualLine.name(), "V-LINE");
        for m in [Mode::Search, Mode::Command, Mode::Confirm, Mode::Prompt] {
            assert!(!m.name().is_empty(), "{m:?}");
        }
    }

    #[test]
    fn position_uses_display_column() {
        // Wide CJK and an emoji count one column each, as graphemes do,
        // not the two cells a terminal gives them (R21).
        let buf = Buffer::from_text("\u{65e5}\u{672c}x\n\u{1f600}\u{1f600}y");
        assert_eq!(position(&buf, Pos { line: 0, col: 6 }), "1:3");
        assert_eq!(position(&buf, Pos { line: 1, col: 8 }), "2:3");
    }

    #[test]
    fn the_status_names_each_mode_the_editor_is_in() {
        let mut e = ed("abc\ndef");
        assert_eq!(left(&e), StatusLeft::Mode("NORMAL"));
        keys(&mut e, "v");
        assert_eq!(left(&e), StatusLeft::Mode("VISUAL"));
        keys(&mut e, "V");
        assert_eq!(left(&e), StatusLeft::Mode("V-LINE"));
        keys(&mut e, "<Esc>i");
        assert_eq!(left(&e), StatusLeft::Mode("INSERT"));
    }

    #[test]
    fn message_beats_mode_and_fades_after_6s() {
        let (mut e, host) = ed_with_host("abc");
        host.state().now = 1_000;
        let fx = keys(&mut e, ":foo<CR>");
        assert_eq!(
            left(&e),
            StatusLeft::Message {
                text: "E492: Not a command: foo".into(),
                kind: MsgKind::Error,
            }
        );
        // The shell is asked to tick when it is due to fade.
        assert_eq!(fx.wake_at, Some(1_000 + MESSAGE_FOR_NS));
        let fx = e.handle(Input::Tick {
            now_ns: 1_000 + MESSAGE_FOR_NS - 1,
        });
        assert_eq!(fx.redraw, Redraw::None);
        assert!(matches!(left(&e), StatusLeft::Message { .. }));
        let fx = e.handle(Input::Tick {
            now_ns: 1_000 + MESSAGE_FOR_NS,
        });
        assert_eq!(fx.redraw, Redraw::Status);
        assert_eq!(left(&e), StatusLeft::Mode("NORMAL"));
    }

    #[test]
    fn an_ordinary_message_is_info() {
        let mut e = ed("foo foo");
        keys(&mut e, ":s/foo/bar/g<CR>");
        assert_eq!(
            left(&e),
            StatusLeft::Message {
                text: "2 substitutions on 1 line".into(),
                kind: MsgKind::Info,
            }
        );
    }

    #[test]
    fn esc_clears_a_message() {
        let mut e = ed("abc");
        keys(&mut e, ":foo<CR>");
        let fx = keys(&mut e, "<Esc>");
        assert_eq!(e.message(), None);
        assert_eq!(fx.redraw, Redraw::Status);
        assert_eq!(left(&e), StatusLeft::Mode("NORMAL"));
    }

    #[test]
    fn the_command_line_and_search_are_typed_in_the_status_bar() {
        let mut e = ed("abc abc");
        keys(&mut e, ":s/a");
        assert_eq!(
            left(&e),
            StatusLeft::Input {
                prefix: ':',
                text: "s/a".into(),
                cursor: 3,
            }
        );
        keys(&mut e, "<Esc>?b");
        assert_eq!(
            left(&e),
            StatusLeft::Input {
                prefix: '?',
                text: "b".into(),
                cursor: 1,
            }
        );
        keys(&mut e, "<Esc>/");
        assert_eq!(
            left(&e),
            StatusLeft::Input {
                prefix: '/',
                text: String::new(),
                cursor: 0,
            }
        );
    }

    #[test]
    fn typing_a_command_clears_the_message_it_would_hide() {
        let mut e = ed("abc");
        keys(&mut e, ":foo<CR>:");
        assert_eq!(e.message(), None);
        assert!(matches!(left(&e), StatusLeft::Input { prefix: ':', .. }));
    }

    #[test]
    fn confirm_asks_in_the_status_bar() {
        let mut e = ed("foo foo");
        keys(&mut e, ":s/foo/x/gc<CR>");
        assert_eq!(
            left(&e),
            StatusLeft::Prompt("replace with x? [y/n/a/q/l]".into())
        );
    }

    #[test]
    fn showcmd_and_position_are_in_the_model() {
        let mut e = ed("abc\ndef");
        keys(&mut e, "jl2d");
        let s = e.status();
        assert_eq!(s.showcmd, "2d");
        assert_eq!(s.position, "2:2");
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn status_only_changes_redraw_only_the_status_bar() {
        let mut e = ed("abc\ndef");
        // A pending count, a key on the command line, a message fading.
        assert_eq!(keys(&mut e, "2").redraw, Redraw::Status);
        assert_eq!(keys(&mut e, "<Esc>:").redraw, Redraw::Status);
        assert_eq!(keys(&mut e, "x").redraw, Redraw::Status);
        assert_eq!(keys(&mut e, "<BS>").redraw, Redraw::Status);
    }

    #[test]
    fn caret_is_a_block_in_normal_and_a_bar_in_insert() {
        let mut e = ed("abc");
        assert_eq!(e.caret_shape(), CaretShape::Block);
        keys(&mut e, "v");
        assert_eq!(e.caret_shape(), CaretShape::Block);
        keys(&mut e, "<Esc>i");
        assert_eq!(e.caret_shape(), CaretShape::Bar);
        // On the command line the caret is in the status bar.
        keys(&mut e, "<Esc>:");
        assert_eq!(e.caret_shape(), CaretShape::Hidden);
    }

    #[test]
    fn a_fresh_visual_selection_covers_the_grapheme_under_the_cursor() {
        let mut e = ed("ae\u{301}x");
        keys(&mut e, "lv");
        let sel = e.selection().unwrap();
        assert_eq!(sel.on_line(0, e.line(0)), Some(1..4));
        keys(&mut e, "l");
        let sel = e.selection().unwrap();
        assert_eq!(sel.on_line(0, e.line(0)), Some(1..5));
    }

    #[test]
    fn the_current_match_moving_redraws_its_old_and_new_lines() {
        let mut e = ed("foo\nbar\nfoo\nbar\nfoo");
        keys(&mut e, "/foo<CR>");
        assert_eq!(e.current_match(), Some((2, 0..3)));
        // The cursor leaves the match, then `n` jumps from line 3.
        keys(&mut e, "j");
        let fx = keys(&mut e, "n");
        assert_eq!(e.current_match(), Some((4, 0..3)));
        let Redraw::Lines(r) = fx.redraw else {
            panic!("{:?}", fx.redraw);
        };
        assert!(r.contains(&2) && r.contains(&4), "{r:?}");
    }

    #[test]
    fn the_current_match_follows_the_confirm_session() {
        // Smartcase highlights only `FOO`; `i` makes `:s` ask at `foo` too.
        let mut e = ed("foo FOO");
        keys(&mut e, ":s/FOO/x/gci<CR>");
        assert_eq!(e.current_match(), Some((0, 0..3)));
        keys(&mut e, "n");
        assert_eq!(e.current_match(), Some((0, 4..7)));
    }

    #[test]
    fn position_counts_graphemes_from_one() {
        let buf = Buffer::from_text("a\ne\u{301}x");
        assert_eq!(position(&buf, Pos { line: 1, col: 3 }), "2:2");
        assert_eq!(position(&buf, Pos { line: 0, col: 0 }), "1:1");
    }
}

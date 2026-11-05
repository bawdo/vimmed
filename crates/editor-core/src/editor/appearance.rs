//! The theme and the config file's messages.
//!
//! Under `ThemePref::System` the shell reports the system's appearance
//! (`Input::Appearance`) at start and whenever it changes; each change
//! builds the token set for the new theme from the config and repaints
//! everything. A fixed `light` or `dark` ignores it.
//!
//! What the config file's read found wrong (`Config::load`) is said once
//! in the status bar at open, with any other warning from opening. A
//! prompt at open (already open, recovery) holds it back until answered,
//! as it does the lock warning.

use crate::effects::{Effects, Redraw};
use crate::status;
use crate::strings;
use crate::tokens::Theme;
use crate::view_model::MsgKind;

use super::Editor;

impl Editor {
    /// The system's appearance is `theme`.
    pub(super) fn appearance(&mut self, theme: Theme) -> Effects {
        if self.cfg.theme.fixed().is_some() || theme == self.theme {
            return Effects::default();
        }
        self.theme = theme;
        // The messages were said when the config was read.
        self.tokens = self.cfg.tokens(theme).0;
        Effects {
            redraw: Redraw::All,
            ..Effects::default()
        }
    }

    /// Says what the config file's read found wrong (`messages` from
    /// `Config::load`), once the prompts at open, if any, are answered.
    /// The shell calls it after `open` and before `open_effects`.
    pub fn say_config(&mut self, messages: &[String]) {
        if messages.is_empty() {
            return;
        }
        let text = messages.join(strings::MESSAGE_JOIN);
        if self.asking.is_some() {
            self.config_warning = Some(text);
        } else {
            self.say_open_warning(text);
        }
    }

    /// Once the prompts at open are answered: says the config warning held
    /// back, if any.
    pub(super) fn say_held_config_warning(&mut self) {
        if let Some(text) = self.config_warning.take() {
            self.say_open_warning(text);
        }
    }

    /// Shows `text` as a warning after any warning opening already shows,
    /// so neither hides the other.
    fn say_open_warning(&mut self, text: String) {
        let text = match &self.message {
            Some((before, MsgKind::Warning)) => {
                format!("{before}{}{text}", strings::MESSAGE_JOIN)
            }
            _ => text,
        };
        self.say_as(text, MsgKind::Warning, status::MESSAGE_FOR_NS);
    }
}

#[cfg(test)]
mod tests {
    use crate::editor::Config;
    use crate::testkit::{ViewSource, build};
    use crate::view_model::StatusLeft;

    #[test]
    fn config_messages_show_in_the_status_bar_at_open() {
        let mut e = build("text\n", Config::default(), false);
        e.say_config(&[
            "unknown setting: wrap".into(),
            "bad value for vim: 1".into(),
        ]);
        assert_eq!(
            e.message(),
            Some("unknown setting: wrap; bad value for vim: 1")
        );
        assert!(e.open_effects().wake_at.is_some(), "it fades");
        assert!(matches!(e.status().left, StatusLeft::Message { .. }));
    }

    #[test]
    fn no_config_messages_say_nothing() {
        let mut e = build("text\n", Config::default(), false);
        e.say_config(&[]);
        assert_eq!(e.message(), None);
    }

    #[test]
    fn config_messages_join_a_warning_already_shown() {
        let mut e = build("text\n", Config::default(), false);
        e.say_open_warning("no state directory".into());
        e.say_config(&["config error: x".into()]);
        assert_eq!(e.message(), Some("no state directory; config error: x"));
    }
}

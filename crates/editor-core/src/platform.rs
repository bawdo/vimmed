//! The platform the editor runs on, for shortcuts that differ between them.

use crate::input::Mods;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Linux,
}

impl Platform {
    /// The platform this build targets.
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Linux
        }
    }

    /// True when the primary shortcut modifier is held: Cmd on the Mac,
    /// Ctrl on Linux.
    pub fn primary(self, m: Mods) -> bool {
        match self {
            Platform::Mac => m.cmd,
            Platform::Linux => m.ctrl,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_is_cmd_on_mac_and_ctrl_on_linux() {
        let cmd = Mods {
            cmd: true,
            ..Mods::default()
        };
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        assert!(Platform::Mac.primary(cmd));
        assert!(!Platform::Mac.primary(ctrl));
        assert!(Platform::Linux.primary(ctrl));
        assert!(!Platform::Linux.primary(cmd));
    }
}

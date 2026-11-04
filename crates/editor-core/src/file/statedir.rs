//! The state directory: `$XDG_STATE_HOME/<slug>`, or `~/.local/state/<slug>`
//! when that is unset. It holds the shared registers, the recovery copies
//! and the lock records.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    /// The state directory the environment names, or `None` when it names
    /// none (no usable `$XDG_STATE_HOME` or `$HOME`).
    pub fn from_env() -> Option<StateDir> {
        StateDir::from_vars(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
    }

    /// The state directory for these values of `$XDG_STATE_HOME` and
    /// `$HOME`. An empty or relative `$XDG_STATE_HOME` is ignored, as the
    /// XDG spec says, and so is an empty or relative `$HOME`.
    fn from_vars(xdg: Option<OsString>, home: Option<OsString>) -> Option<StateDir> {
        let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
        let base = absolute(xdg).or_else(|| Some(absolute(home)?.join(".local/state")))?;
        Some(StateDir {
            root: base.join(app_meta::SLUG),
        })
    }

    /// A state directory at `path`, for tests.
    pub fn at(path: impl Into<PathBuf>) -> StateDir {
        StateDir { root: path.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where recovery copies go.
    pub fn recovery(&self) -> PathBuf {
        self.root.join("recovery")
    }

    /// Where the last appearance the system reported is kept
    /// (`config::cached_appearance`).
    pub fn appearance(&self) -> PathBuf {
        self.root.join("appearance")
    }

    /// Where lock records go.
    pub fn locks(&self) -> PathBuf {
        self.root.join("locks")
    }

    /// Creates the directory and its `recovery` and `locks` folders, each
    /// readable only by the user (mode 0700).
    pub fn ensure(&self) -> io::Result<()> {
        for dir in [self.root.clone(), self.recovery(), self.locks()] {
            private_dir(&dir)?;
        }
        Ok(())
    }
}

/// Creates `dir` (and any missing parents) with mode 0700, and makes an
/// existing one 0700.
#[cfg(unix)]
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_state_home_respected() {
        let s = StateDir::from_vars(Some("/xdg/state".into()), Some("/home/u".into())).unwrap();
        assert_eq!(s.root(), Path::new("/xdg/state").join(app_meta::SLUG));
        let s = StateDir::from_vars(Some("/xdg/state".into()), None).unwrap();
        assert_eq!(s.root(), Path::new("/xdg/state").join(app_meta::SLUG));
    }

    #[test]
    fn without_xdg_state_home_the_home_directory_is_used() {
        let want = Path::new("/home/u/.local/state").join(app_meta::SLUG);
        let root = |xdg: Option<&str>| {
            StateDir::from_vars(xdg.map(Into::into), Some("/home/u".into()))
                .unwrap()
                .root()
                .to_owned()
        };
        assert_eq!(root(None), want);
        // The XDG spec: an empty or relative value is ignored.
        assert_eq!(root(Some("")), want);
        assert_eq!(root(Some("rel/state")), want);
    }

    #[test]
    fn without_any_home_there_is_no_state_directory() {
        // Not a shared temporary directory, where another user could plant
        // the registers file.
        assert_eq!(StateDir::from_vars(None, None), None);
        assert_eq!(StateDir::from_vars(Some("".into()), Some("".into())), None);
        assert_eq!(StateDir::from_vars(None, Some("rel/home".into())), None);
    }

    #[test]
    fn recovery_and_locks_sit_under_the_root() {
        let s = StateDir::at("/s");
        assert_eq!(s.recovery(), Path::new("/s/recovery"));
        assert_eq!(s.locks(), Path::new("/s/locks"));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_creates_private_directories() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let s = StateDir::at(tmp.path().join("a/b"));
        s.ensure().unwrap();
        s.ensure().unwrap();
        for dir in [s.root().to_owned(), s.recovery(), s.locks()] {
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{dir:?}");
        }
    }
}

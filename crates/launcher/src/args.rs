//! Command-line parsing, by hand: the launcher links only libc.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum Cmd {
    Open {
        path: PathBuf,
        background: bool,
        view: bool,
    },
    Help,
    Version,
}

/// A usage error. The message says what was wrong; the caller prints it with
/// the usage line and exits 2.
#[derive(Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

/// Parse the arguments after the program name.
///
/// Flags: `-b` (return once the window has started), `--view`, `-h`/`--help`,
/// `-V`/`--version`. `--` ends the flags, so a file named `-b` can be opened.
/// Exactly one file is required. Help and version win over any file.
pub fn parse(argv: &[OsString]) -> Result<Cmd, UsageError> {
    let mut background = false;
    let mut view = false;
    let mut files: Vec<&OsString> = Vec::new();
    let mut flags_done = false;

    for arg in argv {
        if flags_done {
            files.push(arg);
            continue;
        }
        match arg.to_str() {
            Some("--") => flags_done = true,
            Some("-b") => background = true,
            Some("--view") => view = true,
            Some("-h" | "--help") => return Ok(Cmd::Help),
            Some("-V" | "--version") => return Ok(Cmd::Version),
            // A lone `-` is an ordinary name, not a flag.
            _ if is_flag(arg) => {
                return Err(UsageError(format!(
                    "unknown option {}",
                    arg.to_string_lossy()
                )));
            }
            _ => files.push(arg),
        }
    }

    match files.as_slice() {
        [] => Err(UsageError("no file given".into())),
        [path] => Ok(Cmd::Open {
            path: PathBuf::from(path),
            background,
            view,
        }),
        _ => Err(UsageError("only one file can be opened".into())),
    }
}

fn is_flag(arg: &OsString) -> bool {
    let bytes = arg.as_encoded_bytes();
    bytes.len() > 1 && bytes[0] == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn no_args_is_usage() {
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn b_and_view_combine() {
        let want = Cmd::Open {
            path: PathBuf::from("a.md"),
            background: true,
            view: true,
        };
        assert_eq!(parse(&args(&["-b", "--view", "a.md"])), Ok(want));
        let want = Cmd::Open {
            path: PathBuf::from("a.md"),
            background: true,
            view: true,
        };
        assert_eq!(parse(&args(&["a.md", "--view", "-b"])), Ok(want));
    }

    #[test]
    fn plain_file_opens_in_foreground() {
        let want = Cmd::Open {
            path: PathBuf::from("a.md"),
            background: false,
            view: false,
        };
        assert_eq!(parse(&args(&["a.md"])), Ok(want));
    }

    #[test]
    fn unknown_flag_is_usage() {
        assert!(parse(&args(&["--frobnicate", "a.md"])).is_err());
        assert!(parse(&args(&["-x", "a.md"])).is_err());
    }

    #[test]
    fn two_files_is_usage() {
        assert!(parse(&args(&["a.md", "b.md"])).is_err());
    }

    #[test]
    fn double_dash_allows_dash_file() {
        let want = Cmd::Open {
            path: PathBuf::from("-b"),
            background: false,
            view: false,
        };
        assert_eq!(parse(&args(&["--", "-b"])), Ok(want));
        let want = Cmd::Open {
            path: PathBuf::from("--view"),
            background: true,
            view: false,
        };
        assert_eq!(parse(&args(&["-b", "--", "--view"])), Ok(want));
    }

    #[test]
    fn help_and_version() {
        assert_eq!(parse(&args(&["--help"])), Ok(Cmd::Help));
        assert_eq!(parse(&args(&["-h"])), Ok(Cmd::Help));
        assert_eq!(parse(&args(&["--version"])), Ok(Cmd::Version));
        assert_eq!(parse(&args(&["-V"])), Ok(Cmd::Version));
        // Help wins even alongside a file.
        assert_eq!(parse(&args(&["a.md", "--help"])), Ok(Cmd::Help));
    }
}

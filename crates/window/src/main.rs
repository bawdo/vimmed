//! The window process. The launcher starts it as `window [--view] PATH`
//! with `<P>PROTOCOL` set; it checks the protocol, parses its arguments and
//! hands them to the platform shell.
//!
//! With no path the window is the macOS router (Finder and `open` start it
//! that way): it creates no window, starts one sibling window process per
//! file it is asked to open, and exits 0. Elsewhere no path is a usage
//! error.
//!
//! Exit codes: the shell's own (0, 1 for `:cq`, or 128 + n after signal n);
//! 0 from the router; 2 for bad arguments; 220 when no window can come up,
//! because no shell exists for this platform or the toolkit cannot start
//! (R34: the launcher's 220 means the same, so it passes through unchanged);
//! 221 when the launcher speaks a different protocol; 223 when a bench run
//! never marked `first_frame` (`marks::bench::EXIT_NO_FIRST_FRAME`). 222 is
//! the launcher's own code when it loses track of the window (D7), so the
//! window never exits with it. 220 to 223 are the reserved block.

use std::ffi::OsString;
use std::path::PathBuf;

use app_meta::SLUG;
use editor_core::host::WindowArgs;

/// Only the macOS shell has a router (no path: route Finder's opens).
const HAS_ROUTER: bool = cfg!(target_os = "macos");
const EXIT_USAGE: i32 = 2;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const EXIT_NOT_STARTED: i32 = 220;
const EXIT_PROTOCOL: i32 = 221;

fn main() {
    marks::init();
    marks::mark(marks::names::WINDOW_MAIN);
    let protocol = std::env::var(format!("{}PROTOCOL", app_meta::ENV_PREFIX)).ok();
    if !protocol_ok(protocol.as_deref()) {
        eprintln!(
            "{SLUG}: launcher protocol {} does not match window protocol {}",
            protocol.unwrap_or_default(),
            app_meta::PROTOCOL
        );
        std::process::exit(EXIT_PROTOCOL);
    }
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let parsed = parse(&argv).and_then(|a| path_check(&a, HAS_ROUTER).map(|()| a));
    let args = match parsed {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{SLUG}: {msg}");
            std::process::exit(EXIT_USAGE);
        }
    };
    std::process::exit(run(args));
}

#[cfg(target_os = "macos")]
fn run(args: WindowArgs) -> i32 {
    shell_mac::run(args)
}

#[cfg(target_os = "linux")]
fn run(args: WindowArgs) -> i32 {
    shell_gtk::run(args)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn run(_args: WindowArgs) -> i32 {
    eprintln!("{SLUG}: window binary could not be started: unsupported platform");
    EXIT_NOT_STARTED
}

/// No path is router mode, which only a platform with a router accepts.
fn path_check(args: &WindowArgs, has_router: bool) -> Result<(), String> {
    if args.path.is_none() && !has_router {
        return Err("no file given".into());
    }
    Ok(())
}

/// An absent `<P>PROTOCOL` is fine (the window started by hand or by the
/// router); a present one must match exactly.
fn protocol_ok(value: Option<&str>) -> bool {
    value.is_none_or(|v| v.trim().parse::<u32>().ok() == Some(app_meta::PROTOCOL))
}

/// `[--view] [--display-name NAME] [--] [PATH]`. No path means router mode.
fn parse(argv: &[OsString]) -> Result<WindowArgs, String> {
    let mut out = WindowArgs::default();
    let mut it = argv.iter();
    let mut flags_done = false;
    while let Some(arg) = it.next() {
        let flag = if flags_done { None } else { arg.to_str() };
        match flag {
            Some("--") => flags_done = true,
            Some("--view") => out.view = true,
            Some("--display-name") => {
                let name = it.next().ok_or("--display-name needs a value")?;
                out.display_name = Some(name.to_string_lossy().into_owned());
            }
            Some(f) if f.len() > 1 && f.starts_with('-') => {
                return Err(format!("unknown option {f}"));
            }
            _ if out.path.is_some() => return Err("only one file can be opened".into()),
            _ => out.path = Some(PathBuf::from(arg)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn launcher_contract_parses() {
        let got = parse(&a(&["--view", "/x/a.md"])).unwrap();
        assert_eq!(got.path, Some(PathBuf::from("/x/a.md")));
        assert!(got.view);
        let got = parse(&a(&["/x/a.md"])).unwrap();
        assert!(!got.view);
        assert_eq!(got.display_name, None);
    }

    #[test]
    fn router_mode_needs_a_router() {
        let none = WindowArgs::default();
        assert!(path_check(&none, true).is_ok());
        assert_eq!(path_check(&none, false), Err("no file given".to_string()));
        let some = parse(&a(&["/x/a.md"])).unwrap();
        assert!(path_check(&some, false).is_ok());
    }

    #[test]
    fn no_path_is_router_mode() {
        assert_eq!(parse(&[]).unwrap(), WindowArgs::default());
    }

    #[test]
    fn display_name_and_dash_files() {
        let got = parse(&a(&["--display-name", "Notes", "--", "-b"])).unwrap();
        assert_eq!(got.display_name.as_deref(), Some("Notes"));
        assert_eq!(got.path, Some(PathBuf::from("-b")));
        assert!(parse(&a(&["--display-name"])).is_err());
        assert!(parse(&a(&["--bogus", "a.md"])).is_err());
        assert!(parse(&a(&["a.md", "b.md"])).is_err());
    }

    #[test]
    fn protocol_mismatch_is_refused() {
        assert!(protocol_ok(None));
        assert!(protocol_ok(Some(&app_meta::PROTOCOL.to_string())));
        assert!(!protocol_ok(Some(&(app_meta::PROTOCOL + 1).to_string())));
        assert!(!protocol_ok(Some("nonsense")));
    }
}

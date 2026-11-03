//! The window process. The launcher starts it as `window [--view] PATH`
//! with `<P>PROTOCOL` set; it checks the protocol, parses its arguments and
//! hands them to the platform shell.
//!
//! Exit codes: the shell's own (0, or 128 + n after signal n); 2 for bad
//! arguments; 220 when no shell exists for this platform yet; 221 when the
//! launcher speaks a different protocol; 222 when a bench run never marked
//! `first_frame` (`marks::bench::EXIT_NO_FIRST_FRAME`).

use std::ffi::OsString;
use std::path::PathBuf;

use app_meta::SLUG;
use editor_core::host::WindowArgs;

const EXIT_USAGE: i32 = 2;
#[cfg(not(target_os = "macos"))]
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
    let args = match parse(&argv) {
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
fn run(_args: WindowArgs) -> i32 {
    // TODO(Task 17): call `shell_gtk::run(args)` once the GTK shell exists.
    eprintln!("{SLUG}: window binary could not be started: no GTK shell yet");
    EXIT_NOT_STARTED
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn run(_args: WindowArgs) -> i32 {
    eprintln!("{SLUG}: window binary could not be started: unsupported platform");
    EXIT_NOT_STARTED
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

//! Test stand-in for the window binary. Its stdio is `/dev/null` (the launcher
//! sees to that), so it reports through `FAKE_OUT` and its exit status only.
//!
//! `FAKE_MODE` is a comma-separated list of steps, run in order:
//! - `exit:N`: exit with code N.
//! - `sleep:MS`: sleep for MS milliseconds.
//! - `signal:N`: send signal N to itself.
//! - `write-sid`: write `<sid> <pid>` to the file named by `FAKE_OUT`.
//!
//! Exits 0 after the last step.

use std::time::Duration;

fn write_sid() {
    let Some(out) = std::env::var_os("FAKE_OUT") else {
        std::process::exit(90);
    };
    // SAFETY: getsid(0) and getpid() only read the calling process's ids.
    let (sid, pid) = unsafe { (libc::getsid(0), libc::getpid()) };
    // Write then rename, so a polling reader never sees a partial line.
    let mut tmp = out.clone();
    tmp.push(".tmp");
    if std::fs::write(&tmp, format!("{sid} {pid}\n")).is_err()
        || std::fs::rename(&tmp, &out).is_err()
    {
        std::process::exit(91);
    }
}

fn main() {
    let mode = std::env::var("FAKE_MODE").unwrap_or_default();
    for step in mode.split(',').filter(|s| !s.is_empty()) {
        let (name, arg) = step.split_once(':').unwrap_or((step, ""));
        let num = || {
            arg.parse::<i64>()
                .unwrap_or_else(|_| std::process::exit(92))
        };
        match name {
            "exit" => std::process::exit(num() as i32),
            "sleep" => std::thread::sleep(Duration::from_millis(num() as u64)),
            "signal" => {
                // SAFETY: sending a signal to our own pid.
                unsafe { libc::kill(libc::getpid(), num() as libc::c_int) };
                // A caught or ignored signal would fall through to here.
                std::thread::sleep(Duration::from_secs(5));
                std::process::exit(93);
            }
            "write-sid" => write_sid(),
            _ => std::process::exit(94),
        }
    }
}

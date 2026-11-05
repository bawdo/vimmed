//! `ci`: the local gate. Stops at the first failing step and names it.

use crate::{lint, rename};
use std::path::Path;
use std::process::Command;

type Step = (&'static str, Box<dyn Fn(&Path) -> Result<(), String>>);

fn cargo(root: &Path, args: &[&str]) -> Result<(), String> {
    let ok = Command::new("cargo")
        .current_dir(root)
        .args(args)
        .status()
        .map_err(|e| e.to_string())?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("cargo {} failed", args.join(" ")))
    }
}

fn fmt(root: &Path) -> Result<(), String> {
    cargo(root, &["fmt", "--all", "--check"])
}
fn clippy(root: &Path) -> Result<(), String> {
    cargo(
        root,
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )
}
/// The cargo arguments of the `test` step. With `coverage` the one test
/// run also writes an lcov report, so CI tests once rather than twice.
pub fn test_step_args(coverage: bool) -> Vec<&'static str> {
    if coverage {
        vec![
            "llvm-cov",
            "--workspace",
            "--lcov",
            "--output-path",
            "target/lcov.info",
        ]
    } else {
        vec!["test", "--workspace"]
    }
}
fn test(root: &Path, coverage: bool) -> Result<(), String> {
    cargo(root, &test_step_args(coverage))
}
pub fn lint_name(root: &Path) -> Result<(), String> {
    let (slug, prefix) = rename::read_manifest_names(root)?;
    lint::run(root, &app_meta::derive::Names::from_slug(&slug, &prefix)?)
}
fn rename_test(root: &Path) -> Result<(), String> {
    rename::run(root, rename::DEFAULT_NAME, false)
}

fn steps(coverage: bool) -> Vec<Step> {
    vec![
        ("fmt", Box::new(fmt)),
        ("clippy", Box::new(clippy)),
        ("test", Box::new(move |root| test(root, coverage))),
        ("lint-name", Box::new(lint_name)),
        ("lint-literals", Box::new(crate::literals::run)),
        ("shell-lines", Box::new(crate::shell_lines::run)),
        ("rename-test", Box::new(rename_test)),
    ]
}

pub fn run(root: &Path, linux: bool, coverage: bool) -> Result<(), String> {
    let steps = steps(coverage);
    for (name, step) in &steps {
        println!("== ci: {name}");
        step(root).map_err(|e| format!("ci step `{name}` failed: {e}"))?;
        println!("ci: {name} OK");
    }
    println!("ci: all {} steps OK", steps.len());
    if linux {
        println!("== ci: linux-check");
        linux_check(root)?;
    }
    Ok(())
}

// `linux-check`: the workspace built and tested in a Linux container
// (docker/Dockerfile.check), then a smoke run of the launcher under a
// headless weston. The workspace is mounted at /work; the cargo registry,
// the target directory and the fixture live on a named volume, so repeat
// runs are incremental and the host's own target directory is untouched.
// The smoke run leaves its evidence in target/linux-check on the host.

/// Where the workspace is mounted in the check container.
pub const WORK: &str = "/work";
/// Where the smoke run leaves its evidence, relative to the workspace.
const CHECK_OUT: &str = "target/linux-check";

/// The `channel` rust-toolchain.toml pins.
pub fn toolchain_channel(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "channel").then(|| v.trim().trim_matches('"').to_string())
    })
}

/// Libraries the launcher may link: the vDSO, libc, libgcc_s and the
/// dynamic loader (named differently on each architecture).
fn ldd_allowed(lib: &str) -> bool {
    let name = lib.rsplit('/').next().unwrap_or(lib);
    [
        "linux-vdso.so",
        "linux-gate.so",
        "libc.so",
        "libgcc_s.so",
        "ld-linux",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

/// Checks `ldd` output for the launcher: only libc, libgcc_s, the loader
/// and the vDSO. Returns the unexpected lines.
pub fn ldd_ok(text: &str) -> Result<(), Vec<String>> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return Err(vec!["ldd printed nothing".into()]);
    }
    let bad: Vec<String> = lines
        .iter()
        .filter(|l| !l.split_whitespace().next().is_some_and(ldd_allowed))
        .map(|l| l.to_string())
        .collect();
    if bad.is_empty() { Ok(()) } else { Err(bad) }
}

/// The smoke run's verdict from its marks log, the launcher's exit code
/// and `ldd`: `first_frame` in milliseconds, or what went wrong.
pub fn smoke_verdict(marks: &str, exit_code: &str, ldd: &str) -> Result<f64, String> {
    let mut m = std::collections::BTreeMap::new();
    crate::bench::parse_log(marks, &mut m);
    let first = m
        .get(marks::names::FIRST_FRAME)
        .and_then(|v| v.first().copied());
    let first = first.ok_or("first_frame was never logged")?;
    match exit_code.trim() {
        "0" => {}
        "" => return Err("the launcher's exit code was not recorded".into()),
        code => return Err(format!("the launcher exited with {code}")),
    }
    ldd_ok(ldd).map_err(|bad| format!("launcher links more than libc: {}", bad.join("; ")))?;
    Ok(first)
}

/// The one-line summary of a passing run.
pub fn summary(first_frame_ms: f64) -> String {
    format!(
        "linux-check: build OK, clippy OK, tests OK, weston smoke first_frame={first_frame_ms:.1}, launcher ldd OK"
    )
}

/// The smoke script run in the container, for a given env prefix.
fn smoke_script(prefix: &str) -> String {
    format!(
        r#"set -u
out={WORK}/{CHECK_OUT}
mkdir -p "$out"
cargo xtask fixture /cargo/fixture > /dev/null || exit 11
export XDG_RUNTIME_DIR=/tmp/xdg-run
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"
weston --backend=headless --socket=wl-check --idle-time=0 > "$out/weston.log" 2>&1 &
wpid=$!
for _ in $(seq 100); do [ -S "$XDG_RUNTIME_DIR/wl-check" ] && break; sleep 0.1; done
[ -S "$XDG_RUNTIME_DIR/wl-check" ] || {{ cat "$out/weston.log"; exit 12; }}
ldd /cargo/target/debug/launcher > "$out/ldd.txt" 2>&1
WAYLAND_DISPLAY=wl-check GDK_BACKEND=wayland GTK_A11Y=none NO_AT_BRIDGE=1 \
  {prefix}BENCH=start {prefix}MARKS="$out/marks.log" \
  timeout 60 /cargo/target/debug/launcher /cargo/fixture/test.md > "$out/launcher.log" 2>&1
echo $? > "$out/exit-code"
kill "$wpid" 2> /dev/null
exit 0
"#
    )
}

fn docker(root: &Path, args: &[&str]) -> Result<(), String> {
    let ok = Command::new("docker")
        .current_dir(root)
        .args(args)
        .status()
        .map_err(|e| format!("docker: {e}"))?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("docker {} failed", args.first().unwrap_or(&"")))
    }
}

/// Runs `script` with bash in the check container.
fn in_container(root: &Path, image: &str, volume: &str, script: &str) -> Result<(), String> {
    let env = [("CARGO_TARGET_DIR", "/cargo/target".to_string())];
    run_in_container(root, image, volume, &env, script)
}

/// Runs `script` with bash in the check container, with `work` mounted at
/// `/work`, the named `volume` at `/cargo`, `CARGO_HOME=/cargo/home`, and
/// the extra `env`.
pub fn run_in_container(
    work: &Path,
    image: &str,
    volume: &str,
    env: &[(&str, String)],
    script: &str,
) -> Result<(), String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        format!("{}:{WORK}", work.display()),
        "-v".into(),
        format!("{volume}:/cargo"),
        "-w".into(),
        WORK.into(),
        "-e".into(),
        "CARGO_HOME=/cargo/home".into(),
    ];
    for (k, v) in env {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    args.extend([image.into(), "bash".into(), "-c".into(), script.into()]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    docker(work, &args)
}

/// Builds the check image from `docker/Dockerfile.check` and returns its
/// name and the name of its cargo volume.
pub fn check_image(root: &Path) -> Result<(String, String), String> {
    let pinned = std::fs::read_to_string(root.join("rust-toolchain.toml"))
        .map_err(|e| format!("rust-toolchain.toml: {e}"))?;
    let channel = toolchain_channel(&pinned).ok_or("rust-toolchain.toml: no `channel` found")?;
    let image = format!("{}-check", app_meta::KEBAB);
    let volume = format!("{}-check-cargo", app_meta::KEBAB);
    let arg = format!("RUST_TOOLCHAIN={channel}");
    let build_image = [
        "build",
        "-f",
        "docker/Dockerfile.check",
        "--build-arg",
        &arg,
        "-t",
        &image,
        "docker",
    ];
    docker(root, &build_image).map_err(|e| format!("image build failed: {e}"))?;
    Ok((image, volume))
}

pub fn linux_check(root: &Path) -> Result<(), String> {
    let (image, volume) = check_image(root).map_err(|e| format!("linux-check: {e}"))?;
    let run = |script: &str| in_container(root, &image, &volume, script);
    run("cargo build --workspace --all-targets")
        .map_err(|e| format!("linux-check: build failed: {e}"))?;
    run("cargo xtask validate-packaging")
        .map_err(|e| format!("linux-check: packaging validation failed: {e}"))?;
    run("cargo clippy --workspace --all-targets -- -D warnings")
        .map_err(|e| format!("linux-check: clippy failed: {e}"))?;
    run("cargo test --workspace").map_err(|e| format!("linux-check: tests failed: {e}"))?;
    let out = root.join(CHECK_OUT);
    let _ = std::fs::remove_dir_all(&out);
    run(&smoke_script(app_meta::ENV_PREFIX))
        .map_err(|e| format!("linux-check: smoke setup failed: {e}"))?;
    let read = |f: &str| std::fs::read_to_string(out.join(f)).unwrap_or_default();
    match smoke_verdict(&read("marks.log"), &read("exit-code"), &read("ldd.txt")) {
        Ok(ms) => {
            println!("{}", summary(ms));
            Ok(())
        }
        Err(e) => Err(format!(
            "linux-check: smoke failed: {e}\n--- launcher.log\n{}--- weston.log (tail)\n{}",
            read("launcher.log"),
            tail(&read("weston.log"), 20)
        )),
    }
}

fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut s = lines[lines.len().saturating_sub(n)..].join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const LDD_GOOD: &str = "\tlinux-vdso.so.1 (0x0000ffff8a9c0000)
\tlibgcc_s.so.1 => /lib/aarch64-linux-gnu/libgcc_s.so.1 (0x0000ffff8a900000)
\tlibc.so.6 => /lib/aarch64-linux-gnu/libc.so.6 (0x0000ffff8a750000)
\t/lib/ld-linux-aarch64.so.1 (0x0000ffff8a983000)
";

    #[test]
    fn test_step_runs_plain_tests_without_coverage() {
        assert_eq!(test_step_args(false), ["test", "--workspace"]);
    }

    #[test]
    fn test_step_writes_lcov_with_coverage() {
        assert_eq!(
            test_step_args(true),
            [
                "llvm-cov",
                "--workspace",
                "--lcov",
                "--output-path",
                "target/lcov.info"
            ]
        );
    }

    #[test]
    fn toolchain_channel_is_read_from_the_pin() {
        let t = "[toolchain]\nchannel = \"1.99.1\"\ncomponents = [\"rustfmt\"]\n";
        assert_eq!(toolchain_channel(t).as_deref(), Some("1.99.1"));
        assert_eq!(toolchain_channel("[toolchain]\n"), None);
    }

    #[test]
    fn ldd_allows_only_libc_libgcc_and_the_loader() {
        assert_eq!(ldd_ok(LDD_GOOD), Ok(()));
        let x86 = "\tlinux-vdso.so.1 (0x7ffd)\n\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x1)\n\t/lib64/ld-linux-x86-64.so.2 (0x2)\n";
        assert_eq!(ldd_ok(x86), Ok(()));
        let bad = format!("{LDD_GOOD}\tlibgtk-4.so.1 => /lib/libgtk-4.so.1 (0x3)\n");
        let err = ldd_ok(&bad).unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err[0].contains("libgtk-4"));
        assert!(ldd_ok("").is_err(), "no output is not a pass");
    }

    #[test]
    fn smoke_needs_first_frame_exit_zero_and_clean_ldd() {
        let marks = "window_main,10.000\nfirst_paint,80.000\nfirst_frame,95.250\n";
        assert_eq!(smoke_verdict(marks, "0\n", LDD_GOOD), Ok(95.25));
        let e = smoke_verdict("first_paint,80.0\n", "0\n", LDD_GOOD).unwrap_err();
        assert!(e.contains("first_frame"), "{e}");
        let e = smoke_verdict(marks, "223\n", LDD_GOOD).unwrap_err();
        assert!(e.contains("223"), "{e}");
        let e = smoke_verdict(marks, "", LDD_GOOD).unwrap_err();
        assert!(e.contains("exit"), "{e}");
        let bad = format!("{LDD_GOOD}\tlibm.so.6 => /lib/libm.so.6 (0x3)\n");
        let e = smoke_verdict(marks, "0", &bad).unwrap_err();
        assert!(e.contains("libm"), "{e}");
    }

    #[test]
    fn summary_line_matches_the_brief() {
        assert_eq!(
            summary(95.25),
            "linux-check: build OK, clippy OK, tests OK, weston smoke first_frame=95.2, launcher ldd OK"
        );
    }

    #[test]
    fn smoke_script_uses_the_prefix_and_the_fixture() {
        let s = smoke_script("ABC_DEF_");
        assert!(s.contains("ABC_DEF_BENCH=start"));
        assert!(s.contains("ABC_DEF_MARKS="));
        assert!(s.contains("weston --backend=headless"));
        assert!(s.contains("cargo xtask fixture /cargo/fixture"));
        assert!(s.contains("ldd /cargo/target/debug/launcher"));
    }
}

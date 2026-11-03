//! `cargo xtask bench`: cold-start and keystroke measurement runner.

pub mod report;
pub mod stats;

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Linux,
    Macos,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Macos
        } else {
            Platform::Linux
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::Macos => "macos",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Renderer {
    Default,
    Gl,
    Cairo,
}

impl Renderer {
    fn name(self) -> &'static str {
        match self {
            Renderer::Default => "default",
            Renderer::Gl => "gl",
            Renderer::Cairo => "cairo",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyMode {
    Manual,
    Wtype,
    Synthetic,
}

impl KeyMode {
    fn name(self) -> &'static str {
        match self {
            KeyMode::Manual => "manual",
            KeyMode::Wtype => "wtype",
            KeyMode::Synthetic => "synthetic",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    Start { runs: usize, first_run: bool },
    Type { mode: KeyMode, keys: usize },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Args {
    pub kind: Kind,
    pub renderer: Renderer,
    pub out: Option<PathBuf>,
    pub bin: Option<PathBuf>,
}

pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    let (sub, rest) = argv.split_first().ok_or("missing `start` or `type`")?;
    let mut renderer = Renderer::Default;
    let mut out = None;
    let mut bin = None;
    let mut runs = 20usize;
    let mut keys = 500usize;
    let mut first_run = false;
    let mut mode = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match (sub.as_str(), a.as_str()) {
            (_, "--renderer") => {
                renderer = match val("--renderer")?.as_str() {
                    "default" => Renderer::Default,
                    "gl" => Renderer::Gl,
                    "cairo" => Renderer::Cairo,
                    o => return Err(format!("unknown renderer `{o}`")),
                }
            }
            (_, "--out") => out = Some(PathBuf::from(val("--out")?)),
            (_, "--bin") => bin = Some(PathBuf::from(val("--bin")?)),
            ("start", "--runs") => {
                runs = val("--runs")?
                    .parse()
                    .map_err(|_| "--runs needs a number")?
            }
            ("start", "--first-run") => first_run = true,
            ("type", "--keys") => {
                keys = val("--keys")?
                    .parse()
                    .map_err(|_| "--keys needs a number")?
            }
            ("type", "--manual") => mode = Some(KeyMode::Manual),
            ("type", "--wtype") => mode = Some(KeyMode::Wtype),
            ("type", "--synthetic") => mode = Some(KeyMode::Synthetic),
            (_, other) => return Err(format!("unknown argument `{other}`")),
        }
    }
    let kind = match sub.as_str() {
        "start" => {
            if runs == 0 {
                return Err("--runs must be at least 1".into());
            }
            Kind::Start { runs, first_run }
        }
        "type" => Kind::Type {
            mode: mode.ok_or("type needs --manual, --wtype or --synthetic")?,
            keys,
        },
        o => return Err(format!("unknown bench mode `{o}`")),
    };
    Ok(Args {
        kind,
        renderer,
        out,
        bin,
    })
}

impl Args {
    fn label(&self) -> String {
        let base = match &self.kind {
            Kind::Start {
                first_run: true, ..
            } => "start-first-run".to_string(),
            Kind::Start { .. } => "start".to_string(),
            Kind::Type { mode, .. } => format!("type-{}", mode.name()),
        };
        format!("{base}-{}", self.renderer.name())
    }

    /// Value of `<P>BENCH` for one run.
    fn bench_env(&self) -> String {
        match &self.kind {
            Kind::Start { .. } => "start".into(),
            Kind::Type { mode, keys } => format!("type-{}:{keys}", mode.name()),
        }
    }
}

/// `<platform>-<YYYYMMDD-HHMMSS>-<label>`, placed under `bench-results/`.
pub fn out_dir_name(platform: Platform, stamp: &str, label: &str) -> String {
    format!("{}-{stamp}-{label}", platform.name())
}

/// Parse `name,ms` lines into the map; malformed lines are skipped.
pub fn parse_log(text: &str, into: &mut BTreeMap<String, Vec<f64>>) {
    for line in text.lines() {
        if let Some((name, v)) = line.split_once(',')
            && let Ok(ms) = v.trim().parse::<f64>()
        {
            into.entry(name.trim().to_string()).or_default().push(ms);
        }
    }
}

pub fn samples_csv(samples: &BTreeMap<String, Vec<f64>>) -> String {
    let mut s = String::from("metric,index,ms\n");
    for (name, v) in samples {
        for (i, ms) in v.iter().enumerate() {
            s.push_str(&format!("{name},{i},{ms:.3}\n"));
        }
    }
    s
}

fn capture(cmd: &str, args: &[&str]) -> String {
    match Command::new(cmd).args(args).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unavailable".into(),
    }
}

fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
        })
        .unwrap_or_else(|| "unavailable".into())
}

fn env_or_unavailable(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| "unavailable".into())
}

pub fn env_report(platform: Platform) -> String {
    let mut s = String::new();
    let mut add = |k: &str, v: String| s.push_str(&format!("{k}: {v}\n"));
    match platform {
        Platform::Macos => {
            add("uname", capture("uname", &["-a"]));
            add("sw_vers", capture("sw_vers", &[]));
            add(
                "display",
                capture("system_profiler", &["SPDisplaysDataType"]),
            );
        }
        Platform::Linux => {
            add("kernel", capture("uname", &["-a"]));
            add("cpu", cpu_model());
            add("gtk4", capture("pkg-config", &["--modversion", "gtk4"]));
            add("session", env_or_unavailable("XDG_SESSION_TYPE"));
            add("scale", env_or_unavailable("GDK_SCALE"));
        }
    }
    s
}

/// Seam for Task 15: on macOS the launcher will live inside the app bundle, and
/// this is where that path gets wired in. `--bin PATH` overrides it.
fn default_launcher(platform: Platform) -> PathBuf {
    match platform {
        Platform::Linux | Platform::Macos => PathBuf::from("target/release/launcher"),
    }
}

/// Seam: the font path cache location, deleted by `--first-run`. Must match the
/// location the window crate uses once that cache exists.
fn font_cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join(app_meta::KEBAB).join("font-paths"))
}

fn env_name(suffix: &str) -> String {
    format!("{}{suffix}", app_meta::ENV_PREFIX)
}

fn run_once(
    args: &Args,
    platform: Platform,
    bin: &Path,
    log: &Path,
) -> Result<BTreeMap<String, Vec<f64>>, String> {
    let _ = std::fs::remove_file(log);
    if let Kind::Type {
        mode: KeyMode::Manual,
        keys,
    } = args.kind
    {
        println!(
            "Type {keys} characters into the window; it closes itself when done. Press Enter to start."
        );
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
    }
    let mut cmd = Command::new(bin);
    cmd.env(env_name("MARKS"), log)
        .env(env_name("BENCH"), args.bench_env());
    if platform == Platform::Linux {
        match args.renderer {
            Renderer::Default => cmd.env("GDK_DISABLE", "vulkan"),
            Renderer::Gl => cmd.env("GSK_RENDERER", "gl"),
            Renderer::Cairo => cmd.env("GSK_RENDERER", "cairo"),
        };
    }
    // T0 is taken as close to the spawn as possible.
    cmd.env(env_name("T0"), marks::now_ns().to_string());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
    if let Kind::Type {
        mode: KeyMode::Wtype,
        keys,
    } = args.kind
    {
        std::thread::sleep(Duration::from_millis(1500));
        let text = "a".repeat(keys);
        let _ = Command::new("wtype").args(["-d", "5", &text]).status();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("launcher exited with {status}"));
    }
    let mut m = BTreeMap::new();
    parse_log(&std::fs::read_to_string(log).unwrap_or_default(), &mut m);
    Ok(m)
}

fn stamp() -> String {
    capture("date", &["+%Y%m%d-%H%M%S"])
}

fn usage() {
    eprintln!(
        "usage: cargo xtask bench start [--runs 20] [--renderer default|gl|cairo] [--first-run] [--out DIR] [--bin PATH]"
    );
    eprintln!(
        "       cargo xtask bench type (--manual | --wtype | --synthetic) [--keys 500] [--renderer ...] [--out DIR] [--bin PATH]"
    );
}

pub fn run(argv: Vec<String>) -> ExitCode {
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("xtask bench: {e}");
            usage();
            return ExitCode::from(2);
        }
    };
    let platform = Platform::current();
    let bin = args
        .bin
        .clone()
        .unwrap_or_else(|| default_launcher(platform));
    let dir = args.out.clone().unwrap_or_else(|| {
        Path::new("bench-results").join(out_dir_name(platform, &stamp(), &args.label()))
    });
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("xtask bench: {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    let log = dir.join("run.marks");
    let manual = matches!(
        args.kind,
        Kind::Type {
            mode: KeyMode::Manual,
            ..
        }
    );
    let (runs, first_run) = match args.kind {
        Kind::Start { runs, first_run } => (if first_run { 1 } else { runs }, first_run),
        Kind::Type { .. } => (1, false),
    };

    if first_run {
        if let Some(p) = font_cache_path() {
            let _ = std::fs::remove_file(p);
        }
    } else if !manual {
        eprintln!("warm-up run (discarded)");
        if let Err(e) = run_once(&args, platform, &bin, &log) {
            eprintln!("xtask bench: {e}");
            return ExitCode::FAILURE;
        }
    }

    let mut samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for i in 0..runs {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(400));
        }
        eprintln!("run {}/{runs}", i + 1);
        match run_once(&args, platform, &bin, &log) {
            Ok(m) => {
                for (k, mut v) in m {
                    samples.entry(k).or_default().append(&mut v);
                }
            }
            Err(e) => {
                eprintln!("xtask bench: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    let _ = std::fs::remove_file(&log);

    let summary = report::summarise(&samples, platform);
    let write = |name: &str, body: &str| std::fs::write(dir.join(name), body);
    if write("samples.csv", &samples_csv(&samples)).is_err()
        || write("summary.txt", &summary).is_err()
        || write("env.txt", &env_report(platform)).is_err()
    {
        eprintln!("xtask bench: could not write results to {}", dir.display());
        return ExitCode::FAILURE;
    }
    print!("{summary}");
    println!("results: {}", dir.display());
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_start_defaults_and_flags() {
        let x = parse_args(&a("start")).unwrap();
        assert_eq!(
            x.kind,
            Kind::Start {
                runs: 20,
                first_run: false
            }
        );
        let x = parse_args(&a("start --runs 5 --renderer gl --first-run --out o")).unwrap();
        assert_eq!(
            x.kind,
            Kind::Start {
                runs: 5,
                first_run: true
            }
        );
        assert_eq!(x.renderer, Renderer::Gl);
        assert_eq!(x.out, Some(PathBuf::from("o")));
    }

    #[test]
    fn parses_type_modes() {
        let x = parse_args(&a("type --wtype --keys 300")).unwrap();
        assert_eq!(
            x.kind,
            Kind::Type {
                mode: KeyMode::Wtype,
                keys: 300
            }
        );
        assert_eq!(x.bench_env(), "type-wtype:300");
        assert!(parse_args(&a("type")).is_err());
        assert!(parse_args(&a("start --bogus")).is_err());
        assert!(parse_args(&a("start --runs 0")).is_err());
        assert!(parse_args(&a("start --renderer x")).is_err());
    }

    #[test]
    fn out_dir_naming() {
        assert_eq!(
            out_dir_name(Platform::Linux, "20261103-101500", "start-default"),
            "linux-20261103-101500-start-default"
        );
    }

    #[test]
    fn log_parse_and_csv() {
        let mut m = BTreeMap::new();
        parse_log(
            "first_frame,12.345\nbad line\nfirst_frame,13.000\nrefresh_hz,60.000\n",
            &mut m,
        );
        assert_eq!(m["first_frame"], vec![12.345, 13.0]);
        let csv = samples_csv(&m);
        assert!(csv.starts_with("metric,index,ms\n"));
        assert!(csv.contains("first_frame,1,13.000\n"));
    }
}

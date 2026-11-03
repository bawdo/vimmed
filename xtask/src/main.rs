mod bench;
mod ci;
mod fixture;
mod lint;
mod literals;
mod rename;

use std::path::PathBuf;
use std::process::ExitCode;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = &args[args.len().min(1)..];
    let result = match args.first().map(String::as_str) {
        Some("ci") => ci::run(&root(), rest.iter().any(|a| a == "--linux")),
        Some("lint-name") => ci::lint_name(&root()),
        Some("lint-literals") => literals::run(&root()),
        Some("rename-test") => {
            let name = rest
                .iter()
                .position(|a| a == "--name")
                .and_then(|i| rest.get(i + 1))
                .map(String::as_str)
                .unwrap_or(rename::DEFAULT_NAME);
            rename::run(&root(), name)
        }
        Some("fixture") => return fixture::run(rest.first().cloned()),
        Some("bench") => return bench::run(rest.to_vec()),
        Some(other) => {
            eprintln!("xtask: unknown subcommand `{other}`");
            usage();
            return ExitCode::from(2);
        }
        None => {
            usage();
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "usage: cargo xtask <ci [--linux] | lint-name | lint-literals | rename-test [--name SLUG] | fixture [DIR] | bench ...>"
    );
}

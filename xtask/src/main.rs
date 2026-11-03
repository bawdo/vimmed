use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        // Later tasks add one arm per subcommand here.
        Some(other) => {
            eprintln!("xtask: unknown subcommand `{other}`");
            usage();
            ExitCode::from(2)
        }
        None => {
            usage();
            ExitCode::from(2)
        }
    }
}

fn usage() {
    eprintln!("usage: cargo xtask <subcommand>");
    eprintln!("no subcommands exist yet");
}

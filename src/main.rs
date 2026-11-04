use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = dubcheck::cli::Cli::parse();
    match dubcheck::cli::run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = claude_router::cli::Cli::parse();
    match claude_router::cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let style = claude_router::cli::Style::detect();
            eprintln!("{} {error:#}", style.err("error:"));
            ExitCode::FAILURE
        }
    }
}

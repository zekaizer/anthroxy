use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = anthroxy::cli::Cli::parse();
    match anthroxy::cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Display only: every error type here already tells the whole
            // story, so the alternate form would print the cause twice.
            let style = anthroxy::cli::Style::detect();
            eprintln!("{} {error}", style.err("error:"));
            ExitCode::FAILURE
        }
    }
}

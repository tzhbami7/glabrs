//! glabrs binary entry point: parse arguments, dispatch, map errors to exit codes.

use std::process::ExitCode;

use glabrs::cli;
use glabrs::cmd;

fn main() -> ExitCode {
    let cli = cli::parse();

    match cmd::dispatch(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(e.exit_code())
        }
    }
}

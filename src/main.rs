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

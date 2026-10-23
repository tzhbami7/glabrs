//! glabrs binary entry point: parse arguments, dispatch, map errors to exit codes.

use std::process::ExitCode;

use glabrs::cli;
use glabrs::cmd;

fn main() -> ExitCode {
    restore_sigpipe();

    let cli = cli::parse();

    match cmd::dispatch(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(e.exit_code())
        }
    }
}

/// Restore the default SIGPIPE disposition so closed pipes (`glabrs mr list |
/// head`) terminate the process silently with signal 141, like every other
/// Unix CLI. Rust ignores SIGPIPE by default, which turns an early `head`
/// exit into a noisy broken-pipe panic from `println!`.
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: `signal` with SIG_DFL is the canonical, async-signal-safe way
    // to restore a default disposition; no handler runs in this process.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

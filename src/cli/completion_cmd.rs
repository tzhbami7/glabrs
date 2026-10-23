//! clap definitions for the `completion` command group.

use clap::Subcommand;

/// Generate shell completion scripts.
#[derive(Subcommand, Debug)]
pub enum CompletionArgs {
    /// Generate completion script for Bash
    Bash,
    /// Generate completion script for Zsh
    Zsh,
    /// Generate completion script for Fish
    Fish,
    /// Generate completion script for PowerShell
    PowerShell,
}

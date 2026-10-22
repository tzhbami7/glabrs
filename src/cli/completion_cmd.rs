use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum CompletionArgs {
    /// Generate completion script for a shell
    Bash,
    Zsh,
    Fish,
    PowerShell,
}

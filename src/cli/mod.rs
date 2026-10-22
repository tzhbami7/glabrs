pub mod api_cmd;
pub mod auth_cmd;
pub mod ci_cmd;
pub mod completion_cmd;
pub mod config_cmd;
pub mod issue_cmd;
pub mod mr_cmd;
pub mod release_cmd;
pub mod repo_cmd;

use clap::{Parser, Subcommand};

/// Lean, fast CLI for self-hosted GitLab
#[derive(Parser, Debug)]
#[command(
    name = "glabrs",
    version,
    about,
    propagate_version = true,
    arg_required_else_help = true
)]
pub struct Root {
    /// Instance hostname or owner/repo path (overrides git remote detection)
    #[arg(short = 'R', long, global = true)]
    pub repo: Option<String>,

    /// Instance hostname (overrides git remote detection)
    #[arg(long, global = true)]
    pub hostname: Option<String>,

    /// Personal access token (takes precedence over env and stored tokens)
    #[arg(long, global = true, env = "GITLAB_TOKEN", hide_env_values = true)]
    pub token: Option<String>,

    /// Skip TLS certificate verification (prints a loud warning)
    #[arg(long, global = true)]
    pub insecure: bool,

    /// Output raw JSON (where supported)
    #[arg(long, global = true)]
    pub json: bool,

    /// Disable pager for long output
    #[arg(long, global = true)]
    pub no_pager: bool,

    /// Print API method and path (never headers)
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage authentication
    Auth {
        #[command(subcommand)]
        command: auth_cmd::AuthArgs,
    },
    /// Manage configuration
    Config {
        #[command(subcommand)]
        command: config_cmd::ConfigArgs,
    },
    /// Make an authenticated API request
    Api {
        #[command(flatten)]
        args: api_cmd::ApiArgs,
    },
    /// Work with repositories and projects
    Repo {
        #[command(subcommand)]
        command: repo_cmd::RepoArgs,
    },
    /// Work with issues
    Issue {
        #[command(subcommand)]
        command: issue_cmd::IssueArgs,
    },
    /// Work with merge requests
    Mr {
        #[command(subcommand)]
        command: mr_cmd::MrArgs,
    },
    /// Work with CI/CD pipelines and jobs
    Ci {
        #[command(subcommand)]
        command: ci_cmd::CiArgs,
    },
    /// Work with releases
    Release {
        #[command(subcommand)]
        command: release_cmd::ReleaseArgs,
    },
    /// Generate shell completion scripts
    Completion {
        #[command(subcommand)]
        command: completion_cmd::CompletionArgs,
    },
}

pub fn parse() -> Root {
    Root::parse()
}

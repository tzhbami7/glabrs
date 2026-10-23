//! clap definitions: argument structs, the command tree, and `parse`.
//!
//! Subcommand modules hold the per-group argument enums; this module holds
//! the global flags ([`Root`]), shared [`ListOpts`], and the [`Command`] tree.

/// Arguments for the `api` passthrough command.
pub mod api_cmd;
/// Arguments for the `auth` command group.
pub mod auth_cmd;
/// Arguments for the `ci` command group.
pub mod ci_cmd;
/// Arguments for the `completion` command group.
pub mod completion_cmd;
/// Arguments for the `config` command group.
pub mod config_cmd;
/// Arguments for the `issue` command group.
pub mod issue_cmd;
/// Arguments for the `mr` command group.
pub mod mr_cmd;
/// Arguments for the `release` command group.
pub mod release_cmd;
/// Arguments for the `repo` command group.
pub mod repo_cmd;

use clap::{Args, Parser, Subcommand};

/// Lean, fast CLI for self-hosted GitLab.
#[derive(Parser, Debug)]
#[command(
    name = "glabrs",
    version,
    about,
    propagate_version = true,
    arg_required_else_help = true
)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "global CLI flags are naturally independent booleans; each maps to one -x/--flag"
)]
pub struct Root {
    /// Project path or URL (numeric ID, group/sub/repo, full URL; overrides
    /// the git remote detection)
    #[arg(short = 'R', long, global = true)]
    pub repo: Option<String>,

    /// Instance hostname (overrides git remote detection)
    #[arg(long, global = true)]
    pub hostname: Option<String>,

    /// Personal access token (shorthand for `GITLAB_TOKEN`; beats
    /// `CI_JOB_TOKEN` and stored tokens)
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

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Shared paging options for list commands.
#[derive(Args, Debug)]
pub struct ListOpts {
    /// Results per page (max 100)
    #[arg(long, default_value = "20", value_parser = clap::value_parser!(u32).range(1..=100))]
    pub per_page: u32,
    /// Page to fetch; with --paginate, the page to start from
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub page: Option<u32>,
    /// Follow X-Next-Page until exhausted
    #[arg(long)]
    pub paginate: bool,
}

/// The glabrs command tree.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage authentication
    Auth {
        /// The `auth` subcommand to run.
        #[command(subcommand)]
        command: auth_cmd::AuthArgs,
    },
    /// Manage configuration
    Config {
        /// The `config` subcommand to run.
        #[command(subcommand)]
        command: config_cmd::ConfigArgs,
    },
    /// Make an authenticated API request
    Api {
        /// The `api` arguments.
        #[command(flatten)]
        args: api_cmd::ApiArgs,
    },
    /// Work with repositories and projects
    Repo {
        /// The `repo` subcommand to run.
        #[command(subcommand)]
        command: repo_cmd::RepoArgs,
    },
    /// Work with issues
    Issue {
        /// The `issue` subcommand to run.
        #[command(subcommand)]
        command: issue_cmd::IssueArgs,
    },
    /// Work with merge requests
    Mr {
        /// The `mr` subcommand to run.
        #[command(subcommand)]
        command: mr_cmd::MrArgs,
    },
    /// Work with CI/CD pipelines and jobs
    Ci {
        /// The `ci` subcommand to run.
        #[command(subcommand)]
        command: ci_cmd::CiArgs,
    },
    /// Work with releases
    Release {
        /// The `release` subcommand to run.
        #[command(subcommand)]
        command: release_cmd::ReleaseArgs,
    },
    /// Generate shell completion scripts
    Completion {
        /// The `completion` subcommand to run.
        #[command(subcommand)]
        command: completion_cmd::CompletionArgs,
    },
}

/// Parse command-line arguments into the clap [`Root`] tree.
#[must_use]
pub fn parse() -> Root {
    Root::parse()
}

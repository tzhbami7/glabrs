//! One module per command group, plus host/token precedence resolution.
//!
//! [`dispatch`] routes the clap tree; [`resolve`] merges settings per
//! precedence (flags > env > per-host > global) and builds the client.

/// The `api` passthrough command.
pub mod api;
/// The `auth` command group.
pub mod auth;
/// The `ci` command group.
pub mod ci;
/// The `completion` command group.
pub mod completion;
/// The `config` command group.
pub mod config;
/// Host/token resolution and the TLS agent builder.
pub mod context;
/// Shared per-command-group helpers (project path, user/milestone lookup).
pub mod helpers;
/// The `issue` command group.
pub mod issue;
/// The `mr` command group.
pub mod mr;
/// The `release` command group.
pub mod release;
/// The `repo` command group.
pub mod repo;

use crate::api::client::TokenKind;
use crate::cli::{Command, Root};
use crate::error::Error;

pub use context::{
    build_agent, normalize_host, resolve, target_hostname, token_flag, warn_insecure,
};

/// Route the parsed command tree to its command group's `run`.
///
/// # Errors
///
/// Returns [`Error`] from the dispatched command; `main` maps it to a
/// process exit code.
pub fn dispatch(root: &Root) -> Result<(), Error> {
    match &root.command {
        Command::Auth { command } => auth::run(root, command),
        Command::Config { command } => config::run(root, command),
        Command::Api { args } => api::run(root, args),
        Command::Repo { command } => repo::run(root, command),
        Command::Issue { command } => issue::run(root, command),
        Command::Mr { command } => mr::run(root, command),
        Command::Ci { command } => ci::run(root, command),
        Command::Release { command } => release::run(root, command),
        Command::Completion { command } => completion::generate(command),
    }
}

/// Resolved per-invocation context: host settings, token, client.
pub struct Ctx {
    /// The API client built for this invocation.
    pub client: crate::api::Client,
    /// The normalized target hostname.
    pub hostname: String,
    /// `--json`: emit raw JSON where supported.
    pub json: bool,
    /// `--no-pager`: bypass paging for long output.
    pub no_pager: bool,
    /// The instance web base (`https://host[:port]/prefix`).
    pub web_url: String,
}

impl Ctx {
    /// The API client for this invocation.
    #[must_use]
    pub fn client(&self) -> &crate::api::Client {
        &self.client
    }

    /// `GET /user` fails under CI job tokens (they authenticate
    /// project-scoped endpoints only); callers needing "the current user"
    /// should degrade gracefully instead of failing the whole command.
    #[must_use]
    pub fn can_resolve_current_user(&self) -> bool {
        !matches!(self.client.token_kind(), Some(TokenKind::Job))
    }
}

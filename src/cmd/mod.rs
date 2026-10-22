pub mod api;
pub mod auth;
pub mod ci;
pub mod completion;
pub mod config;
pub mod context;
pub mod issue;
pub mod mr;
pub mod release;
pub mod repo;

use crate::api::client::TokenKind;
use crate::cli::{Command, Root};
use crate::error::Error;

pub use context::{build_agent, normalize_host, resolve, warn_insecure};

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
    pub client: crate::api::Client,
    pub hostname: String,
    pub json: bool,
    pub no_pager: bool,
    pub web_url: String,
}

impl Ctx {
    pub fn client(&self) -> &crate::api::Client {
        &self.client
    }

    /// `GET /user` fails under CI job tokens (they authenticate
    /// project-scoped endpoints only); callers needing "the current user"
    /// should degrade gracefully instead of failing the whole command.
    pub fn can_resolve_current_user(&self) -> bool {
        !matches!(
            self.client.token_kind(),
            Some(TokenKind::Job)
        )
    }
}

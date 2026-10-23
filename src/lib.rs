//! glabrs — a lean, fast CLI for self-hosted GitLab (17.0+).
//!
//! One small blocking binary, instant startup, no telemetry: the daily-driver
//! command set with script-friendly JSON output everywhere. This lib target
//! exposes the same units the binary uses; the CLI itself is documented in
//! the README (commands, exit codes, configuration).
//!
//! # Module map
//!
//! - [`api`] — blocking GitLab REST v4 client (`Client`, URL building,
//!   percent-encoding, pagination).
//! - [`cli`] — clap definitions: argument structs and the command tree.
//! - [`cmd`] — one module per command group; resolves host/token precedence
//!   ([`cmd::resolve`]) and dispatches.
//! - [`config`] — `config.toml` / `hosts.toml` loading and persistence.
//! - [`git`] — git remote-URL parsing (https, ssh, SCP syntax) and helpers.
//! - [`render`] — output policy: markdown tables, pagers, raw JSON.
//! - [`error`] — crate error type and script-friendly exit codes.

pub mod api;
pub mod cli;
pub mod cmd;
pub mod config;
/// The crate error type and script-friendly exit codes.
pub mod error;
pub mod git;
pub mod render;

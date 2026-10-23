//! Blocking GitLab REST v4 client: request building, auth headers, pagination.

/// The HTTP client and URL helpers.
pub mod client;

pub use client::Client;

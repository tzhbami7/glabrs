//! Configuration files: global `config.toml` and per-host `hosts.toml`.

/// Per-host connection and auth settings.
pub mod hosts;
/// Global settings with dotted-key access.
pub mod store;

use std::path::PathBuf;

pub use hosts::{HostConfig, Hosts};
pub use store::Config;

/// Base directory for glabrs config (respects XDG).
///
/// Falls back to `.glabrs/` in the current directory when no XDG config dir
/// exists.
#[must_use]
pub fn base_dir() -> PathBuf {
    if let Some(d) = dirs::config_dir() {
        d.join("glabrs")
    } else {
        PathBuf::from(".glabrs")
    }
}

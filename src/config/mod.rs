pub mod hosts;
pub mod store;

use std::path::PathBuf;

pub use hosts::{HostConfig, Hosts};
pub use store::Config;

/// Base directory for glabrs config (respects XDG).
pub fn base_dir() -> PathBuf {
    if let Some(d) = dirs::config_dir() {
        d.join("glabrs")
    } else {
        PathBuf::from(".glabrs")
    }
}

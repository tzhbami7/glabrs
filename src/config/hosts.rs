use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Error;

const HOSTS_FILE: &str = "hosts.toml";

/// Per-host connection and auth settings.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct HostConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct HostsFile {
    #[serde(flatten)]
    hosts: BTreeMap<String, HostConfig>,
}

/// All configured hosts.
pub struct Hosts {
    path: PathBuf,
    file: HostsFile,
}

impl Hosts {
    pub fn load() -> Result<Self, Error> {
        let path = Self::default_path();
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, Error> {
        let file = if path.exists() {
            let text = fs::read_to_string(path)?;
            toml::from_str(&text).map_err(|e| Error::runtime(format!("hosts.toml: {e}")))?
        } else {
            HostsFile::default()
        };
        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    pub fn get(&self, hostname: &str) -> Option<&HostConfig> {
        self.file.hosts.get(hostname)
    }

    pub fn get_mut(&mut self, hostname: &str) -> Option<&mut HostConfig> {
        self.file.hosts.get_mut(hostname)
    }

    pub fn entry(&mut self, hostname: &str) -> &mut HostConfig {
        self.file.hosts.entry(hostname.to_string()).or_default()
    }

    pub fn remove(&mut self, hostname: &str) -> bool {
        self.file.hosts.remove(hostname).is_some()
    }

    pub fn names(&self) -> Vec<String> {
        self.file.hosts.keys().cloned().collect()
    }

    /// Persist to disk with 0600 permissions.
    pub fn save(&self) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
            restrict_dir_perms(parent);
        }
        let text = toml::to_string_pretty(&self.file)
            .map_err(|e| Error::runtime(format!("hosts.toml: {e}")))?;
        write_private(&self.path, text.as_bytes())
    }

    fn default_path() -> PathBuf {
        super::base_dir().join(HOSTS_FILE)
    }
}

/// Write a file readable only by the owner.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    fs::write(path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn restrict_dir_perms(dir: &Path) {
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
}

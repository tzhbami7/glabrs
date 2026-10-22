use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::io::Write;
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

/// Write a file readable only by the owner, atomically: the content lands in
/// a temp file created 0600 and is renamed over the target, so a crash can
/// never leave the secret world-readable (fs::write + chmod has a window
/// where the file is 0644).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = tempfile_in(dir)?;
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .map_err(|e| Error::runtime(format!("{}: {e}", tmp.display())))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Create an unused temp path in `dir` (random hex name, retry on collision).
fn tempfile_in(dir: &Path) -> Result<PathBuf, Error> {
    for _ in 0..16 {
        let mut name = String::from(".glabrs-tmp-");
        let mut buf = [0u8; 8];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut buf))
            .map_err(|e| Error::runtime(format!("read /dev/urandom: {e}")))?;
        name.push_str(&buf.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let candidate = dir.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(Error::runtime("could not create a temp file name"))
}

fn restrict_dir_perms(dir: &Path) {
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_private_sets_0600_and_writes_content() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("hosts.toml");
        write_private(&target, b"token = \"x\"").expect("write");
        let mode = fs::metadata(&target).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "token = \"x\"");
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".glabrs-tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }
}

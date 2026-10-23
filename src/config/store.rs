use std::fs;
use std::path::PathBuf;

use crate::error::Error;

/// Global configuration file (config.toml), lazy-loaded TOML with dotted keys.
pub struct Config {
    path: PathBuf,
    value: toml::Value,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path: Self::default_path(),
            value: toml::Value::Table(toml::map::Map::new()),
        }
    }
}

impl Config {
    /// Load `config.toml` from the default config directory.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the file exists but cannot be read or parsed.
    pub fn load() -> Result<Self, Error> {
        let path = Self::default_path();
        Self::load_from(&path)
    }

    /// Load `config.toml` from an explicit path (missing file is an empty table).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the file exists but cannot be read or parsed.
    pub fn load_from(path: &PathBuf) -> Result<Self, Error> {
        let value = if path.exists() {
            let text = fs::read_to_string(path)?;
            toml::from_str(&text).map_err(|e| Error::runtime(format!("config.toml: {e}")))?
        } else {
            toml::Value::Table(toml::map::Map::new())
        };
        Ok(Self {
            path: path.clone(),
            value,
        })
    }

    /// Look up a value by dotted key (`headers.custom`), or `None`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&toml::Value> {
        let mut cur = &self.value;
        for part in key.split('.') {
            cur = cur.get(part)?;
        }
        Some(cur)
    }

    /// Look up a string value by dotted key, or `None`.
    #[must_use]
    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key).and_then(|v| v.as_str().map(String::from))
    }

    /// Set a value at a dotted key, creating intermediate tables.
    pub fn set(&mut self, key: &str, value: toml::Value) {
        let table = ensure_table(&mut self.value, key);
        let last = key.rsplit('.').next().unwrap_or(key);
        if let Some(t) = table.as_table_mut() {
            t.insert(last.to_string(), value);
        }
    }

    /// Remove a value at a dotted key (and prune empty parent tables);
    /// reports whether it existed.
    pub fn unset(&mut self, key: &str) -> bool {
        let parent_key = match key.rsplit_once('.') {
            Some((p, _)) => p,
            None => "",
        };
        match self.get_table_mut(parent_key) {
            Some(t) => {
                let last = key.rsplit('.').next().unwrap_or(key);
                let removed = t.as_table_mut().is_some_and(|tt| tt.remove(last).is_some());
                // Drop tables left empty by the removal (headers.OnlyKey etc.)
                // so the file does not accumulate empty sections.
                if removed && !parent_key.is_empty() {
                    self.prune_empty_tables(parent_key);
                }
                removed
            }
            None => false,
        }
    }

    /// Remove now-empty parent tables along a dotted key path.
    fn prune_empty_tables(&mut self, key: &str) {
        let mut path: Vec<String> = key.split('.').map(String::from).collect();
        while !path.is_empty() {
            let dotted = path.join(".");
            let empty = self
                .get_table_mut(&dotted)
                .and_then(|t| t.as_table().map(toml::map::Map::is_empty))
                .unwrap_or(false);
            if !empty {
                break;
            }
            let parent = match dotted.rsplit_once('.') {
                Some((p, _)) => p.to_string(),
                None => String::new(),
            };
            let last = dotted.rsplit('.').next().unwrap_or(&dotted).to_string();
            if let Some(t) = self.get_table_mut(&parent) {
                if let Some(tt) = t.as_table_mut() {
                    tt.remove(&last);
                }
            }
            path.pop();
        }
    }

    fn get_table_mut(&mut self, key: &str) -> Option<&mut toml::Value> {
        let mut cur = &mut self.value;
        if key.is_empty() {
            return Some(cur);
        }
        for part in key.split('.') {
            cur = cur.get_mut(part)?;
        }
        Some(cur)
    }

    /// The raw config root (merged view dumping reads this).
    #[must_use]
    pub fn root(&self) -> &toml::Value {
        &self.value
    }

    /// Persist the config file (not secret; default permissions).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when directory creation, serialization, or the write
    /// fails.
    pub fn save(&self) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(&self.value)
            .map_err(|e| Error::runtime(format!("config.toml: {e}")))?;
        fs::write(&self.path, text)?;
        Ok(())
    }

    fn default_path() -> PathBuf {
        super::base_dir().join("config.toml")
    }
}

fn ensure_table<'a>(root: &'a mut toml::Value, key: &str) -> &'a mut toml::Value {
    let mut cur = root;
    let parts: Vec<&str> = key.split('.').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        if !cur.get(part).is_some_and(toml::Value::is_table) {
            cur[part] = toml::Value::Table(toml::map::Map::new());
        }
        cur = &mut cur[part];
    }
    cur
}

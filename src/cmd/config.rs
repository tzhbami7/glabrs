use crate::cli::config_cmd::ConfigArgs;
use crate::cli::Root;
use crate::config::{store::Config, Hosts};
use crate::error::Error;

pub fn run(root: &Root, args: &ConfigArgs) -> Result<(), Error> {
    match args {
        ConfigArgs::Get { key, hostname } => get(root, key.as_deref(), hostname.as_deref()),
        ConfigArgs::Set {
            key,
            value,
            hostname,
        } => set(root, key, value, hostname.as_deref()),
        ConfigArgs::Unset { key, hostname } => unset(root, key, hostname.as_deref()),
    }
}

fn get(_root: &Root, key: Option<&str>, hostname: Option<&str>) -> Result<(), Error> {
    let cfg = Config::load()?;
    let hosts = Hosts::load()?;
    let host = hostname.map(|h| {
        hosts
            .get(&crate::cmd::normalize_host(h))
            .cloned()
            .unwrap_or_default()
    });

    match key {
        Some(k) => {
            // Per-host keys (token, ca_file, api_url, ...) resolve from hosts.toml.
            if let Some(hc) = &host {
                if let Some(v) = host_field_str(hc, k) {
                    println!("{v}");
                    return Ok(());
                }
            }
            match cfg.get_str(k) {
                Some(v) => println!("{v}"),
                None => return Err(Error::not_found(format!("unknown key {k}"))),
            }
        }
        None => {
            // Dump merged view (secrets masked).
            let mut lines: Vec<String> = Vec::new();
            for k in ["api_url", "pager", "editor"] {
                if let Some(v) = cfg.get_str(k) {
                    lines.push(format!("{k} = {v}"));
                }
            }
            if let Some(hc) = &host {
                for k in ["api_url", "ca_file", "client_cert", "client_key"] {
                    if let Some(v) = host_field_str(hc, k) {
                        lines.push(format!("{k} = {v}"));
                    }
                }
                if hc.token.is_some() {
                    lines.push("token = ***".to_string());
                }
            }
            if lines.is_empty() {
                return Err(Error::not_found("no configuration set"));
            }
            println!("{}", lines.join("\n"));
        }
    }
    Ok(())
}

fn set(_root: &Root, key: &str, value: &str, hostname: Option<&str>) -> Result<(), Error> {
    match hostname {
        Some(h) => {
            let mut hosts = Hosts::load()?;
            let entry = hosts.entry(&crate::cmd::normalize_host(h));
            set_host_field(entry, key, Some(value.to_string()))?;
            hosts.save()?;
        }
        None => {
            let mut cfg = Config::load()?;
            cfg.set(key, toml::Value::String(value.to_string()));
            cfg.save()?;
        }
    }
    Ok(())
}

fn unset(_root: &Root, key: &str, hostname: Option<&str>) -> Result<(), Error> {
    match hostname {
        Some(h) => {
            let mut hosts = Hosts::load()?;
            if let Some(entry) = hosts.get_mut(&crate::cmd::normalize_host(h)) {
                set_host_field(entry, key, None)?;
            }
            hosts.save()?;
        }
        None => {
            let mut cfg = Config::load()?;
            cfg.unset(key);
            cfg.save()?;
        }
    }
    Ok(())
}

fn set_host_field(
    hc: &mut crate::config::HostConfig,
    key: &str,
    value: Option<String>,
) -> Result<(), Error> {
    match key {
        "token" => hc.token = value,
        "api_url" => hc.api_url = value,
        "ca_file" => hc.ca_file = value,
        "client_cert" => hc.client_cert = value,
        "client_key" => hc.client_key = value,
        _ => return Err(Error::not_found(format!("unknown host key {key}"))),
    }
    Ok(())
}

fn host_field_str(hc: &crate::config::HostConfig, key: &str) -> Option<String> {
    match key {
        "token" => hc.token.as_ref().map(|_| "***".to_string()),
        "api_url" => hc.api_url.clone(),
        "ca_file" => hc.ca_file.clone(),
        "client_cert" => hc.client_cert.clone(),
        "client_key" => hc.client_key.clone(),
        _ => None,
    }
}

use crate::cli::auth_cmd::AuthArgs;
use crate::cli::Root;
use crate::config::Hosts;
use crate::error::Error;
use crate::render::pager::read_input;

pub fn run(root: &Root, args: &AuthArgs) -> Result<(), Error> {
    match args {
        AuthArgs::Login {
            hostname,
            token,
            ca_file,
            api_url,
        } => login(root, hostname, token, ca_file, api_url),
        AuthArgs::Status { hostname } => status(root, hostname),
        AuthArgs::Logout { hostname } => logout(root, hostname),
    }
}

fn login(
    root: &Root,
    hostname_flag: &Option<String>,
    token_arg: &Option<String>,
    ca_file: &Option<String>,
    api_url: &Option<String>,
) -> Result<(), Error> {
    let hostname = match hostname_flag
        .clone()
        .or_else(|| std::env::var("GLABRS_HOST").ok())
        .or_else(|| std::env::var("GITLAB_HOST").ok())
    {
        Some(h) => crate::cmd::normalize_host(&h),
        None => {
            return Err(Error::usage(
                "auth login requires --hostname (or set GLABRS_HOST/GITLAB_HOST)",
            ))
        }
    };

    // Token: --token > stdin (`-`) > hidden prompt.
    let token = match token_arg {
        Some(t) if t == "-" => read_input("-")?.trim().to_string(),
        Some(t) => t.clone(),
        None => rpassword::prompt_password(format!("Paste token for {hostname}: "))?,
    };
    if token.is_empty() {
        return Err(Error::auth("empty token"));
    }

    let host_cfg = crate::config::HostConfig {
        token: Some(token.clone()),
        api_url: api_url.clone(),
        ca_file: ca_file.clone(),
        ..Default::default()
    };

    // Validate the token against GET /user before storing.
    let agent = crate::cmd::build_agent(&host_cfg, root.insecure)?;
    if root.insecure {
        eprintln!(
            "WARNING: --insecure disables TLS certificate verification; the token was sent over an unverified connection."
        );
    }
    let settings = crate::api::client::HostSettings {
        api_url: api_url
            .clone()
            .unwrap_or_else(|| format!("https://{hostname}/api/v4")),
        headers: Default::default(),
    };
    let client = crate::api::Client::new(
        agent,
        settings,
        Some((token.clone(), crate::api::client::TokenKind::Personal)),
        root.verbose,
    );
    let user: serde_json::Value = client
        .get_json_value("/user", &[])
        .map_err(|e| Error::auth(format!("token validation failed: {e}")))?;
    let username = user.get("username").and_then(|v| v.as_str()).unwrap_or("?");

    let mut hosts = Hosts::load()?;
    let entry = hosts.entry(&hostname);
    entry.token = Some(token);
    if let Some(u) = api_url {
        entry.api_url = Some(u.clone());
    }
    if let Some(c) = ca_file {
        entry.ca_file = Some(c.clone());
    }
    hosts.save()?;

    if !root.json {
        println!("Authenticated as {username} on {hostname}");
    } else {
        crate::render::table::print_json(&user)?;
    }
    Ok(())
}

fn status(root: &Root, hostname_flag: &Option<String>) -> Result<(), Error> {
    let _ = root;
    let hosts = Hosts::load()?;
    let hostname = match hostname_flag
        .clone()
        .or_else(|| std::env::var("GLABRS_HOST").ok())
        .or_else(|| std::env::var("GITLAB_HOST").ok())
    {
        Some(h) => crate::cmd::normalize_host(&h),
        None => crate::git::detect_remote()?.hostname,
    };

    let entry = hosts.get(&hostname);
    let stored = entry.and_then(|h| h.token.clone());
    let api_url = entry
        .and_then(|h| h.api_url.clone())
        .unwrap_or_else(|| format!("https://{hostname}/api/v4"));

    if stored.is_none() {
        return Err(Error::auth(format!("no token stored for {hostname}")));
    }

    let host_cfg = entry.cloned().unwrap_or_default();
    let agent = crate::cmd::build_agent(&host_cfg, root.insecure)?;
    let settings = crate::api::client::HostSettings {
        api_url,
        headers: Default::default(),
    };
    let client = crate::api::Client::new(
        agent,
        settings,
        stored.map(|t| (t, crate::api::client::TokenKind::Personal)),
        root.verbose,
    );
    let user: serde_json::Value = client
        .get_json_value("/user", &[])
        .map_err(|e| Error::auth(format!("token check failed: {e}")))?;

    if root.json {
        crate::render::table::print_json(&user)?;
    } else {
        let username = user.get("username").and_then(|v| v.as_str()).unwrap_or("?");
        let name = user.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        println!("host:     {hostname}");
        println!("user:     {username} ({name})");
        println!("api_url:  {}", client.api_url);
    }
    Ok(())
}

fn logout(root: &Root, hostname_flag: &Option<String>) -> Result<(), Error> {
    let _ = root;
    let mut hosts = Hosts::load()?;
    let hostname = match hostname_flag
        .clone()
        .or_else(|| std::env::var("GLABRS_HOST").ok())
        .or_else(|| std::env::var("GITLAB_HOST").ok())
    {
        Some(h) => crate::cmd::normalize_host(&h),
        None => crate::git::detect_remote()?.hostname,
    };

    if hosts.remove(&hostname) {
        hosts.save()?;
        if !root.json {
            println!("Removed credentials for {hostname}");
        }
        Ok(())
    } else {
        Err(Error::not_found(format!(
            "no credentials stored for {hostname}"
        )))
    }
}

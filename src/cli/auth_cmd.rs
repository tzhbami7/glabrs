use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum AuthArgs {
    /// Authenticate against a GitLab instance
    Login {
        /// Instance hostname (e.g. gitlab.corp.example)
        #[arg(long)]
        hostname: Option<String>,

        /// Read token from argument, stdin (`-`), or prompt if omitted
        #[arg(long)]
        token: Option<String>,

        /// Path to a CA bundle for self-signed instances
        #[arg(long)]
        ca_file: Option<String>,

        /// API base URL override (for path-prefixed reverse proxies)
        #[arg(long)]
        api_url: Option<String>,
    },
    /// Show current authentication state
    Status {
        /// Instance hostname to inspect (default: detected)
        #[arg(long)]
        hostname: Option<String>,
    },
    /// Remove stored credentials for a host
    Logout {
        /// Instance hostname (default: detected)
        #[arg(long)]
        hostname: Option<String>,
    },
}

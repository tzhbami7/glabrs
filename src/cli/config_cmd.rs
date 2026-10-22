use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum ConfigArgs {
    /// Show merged configuration (flags > env > per-host > global)
    Get {
        /// Configuration key to read (default: dump all)
        key: Option<String>,
        /// Instance hostname context
        #[arg(long)]
        hostname: Option<String>,
    },
    /// Set a configuration key
    Set {
        /// Configuration key, e.g. pager, editor, per-host ca_file
        key: String,
        /// Value to store
        value: String,
        /// Apply to a specific host instead of global config
        #[arg(long)]
        hostname: Option<String>,
    },
    /// Remove a configuration key
    Unset {
        key: String,
        #[arg(long)]
        hostname: Option<String>,
    },
}

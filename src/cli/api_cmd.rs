use clap::Args;

/// Make an authenticated request to any REST v4 endpoint.
#[derive(Args, Debug)]
pub struct ApiArgs {
    /// Endpoint path, e.g. projects/42/issues
    pub path: String,

    /// HTTP method
    #[arg(long, default_value = "GET")]
    pub method: String,

    /// JSON body from a file or stdin (`-`)
    #[arg(long)]
    pub input: Option<String>,

    /// Field=value pairs (colon-prefixed path fields, -F style)
    #[arg(long = "field", short = 'f')]
    pub fields: Vec<String>,

    /// Follow X-Next-Page until exhausted (GET only)
    #[arg(long)]
    pub paginate: bool,

    /// Start paginating at this page (requires --paginate)
    #[arg(long, requires = "paginate", value_parser = clap::value_parser!(u32).range(1..))]
    pub page: Option<u32>,
}

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum IssueArgs {
    /// List issues
    List {
        #[arg(long)]
        assignee: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        labels: Option<String>,
        #[arg(long)]
        state: Option<String>,
        #[arg(long, default_value = "20")]
        per_page: u32,
        #[arg(long)]
        page: Option<u32>,
        #[arg(long)]
        paginate: bool,
    },
    /// Create an issue
    Create {
        #[arg(long, short = 't')]
        title: String,
        #[arg(long, short = 'd')]
        description: Option<String>,
        /// Read description from file or stdin (`-`)
        #[arg(long)]
        description_file: Option<String>,
        #[arg(long)]
        labels: Option<String>,
        #[arg(long)]
        milestone: Option<String>,
        #[arg(long)]
        assignee: Option<String>,
    },
    /// View an issue
    View {
        iid: u64,
        /// Open in browser
        #[arg(long)]
        web: bool,
    },
    /// Close an issue
    Close { iid: u64 },
    /// Reopen an issue
    Reopen { iid: u64 },
    /// Comment on an issue
    Comment {
        iid: u64,
        /// Comment body, or use --body-file
        #[arg(long)]
        body: Option<String>,
        /// Read body from file or stdin (`-`)
        #[arg(long)]
        body_file: Option<String>,
    },
    /// Delete an issue
    Delete {
        iid: u64,
        /// Skip confirmation
        #[arg(long)]
        yes: bool,
    },
}

//! clap definitions for the `issue` command group.

use clap::Subcommand;

use super::ListOpts;

/// Work with issues.
#[derive(Subcommand, Debug)]
pub enum IssueArgs {
    /// List issues
    List {
        /// Filter by assignee (`@me`, `@username`)
        #[arg(long)]
        assignee: Option<String>,
        /// Filter by author (`@me`, `@username`)
        #[arg(long)]
        author: Option<String>,
        /// Filter by comma-separated label names
        #[arg(long)]
        labels: Option<String>,
        /// Filter by state (opened, closed)
        #[arg(long)]
        state: Option<String>,
        #[command(flatten)]
        /// Paging options.
        list: ListOpts,
    },
    /// Create an issue
    Create {
        /// Issue title
        #[arg(long, short = 't')]
        title: String,
        /// Issue description
        #[arg(long, short = 'd')]
        description: Option<String>,
        /// Read description from file or stdin (`-`)
        #[arg(long)]
        description_file: Option<String>,
        /// Comma-separated label names
        #[arg(long)]
        labels: Option<String>,
        /// Milestone title (exact match)
        #[arg(long)]
        milestone: Option<String>,
        /// Assignee (`@me`, `@username`)
        #[arg(long)]
        assignee: Option<String>,
    },
    /// View an issue
    View {
        /// Issue IID
        iid: u64,
        /// Open in browser
        #[arg(long)]
        web: bool,
    },
    /// Close an issue
    Close {
        /// Issue IID
        iid: u64,
    },
    /// Reopen an issue
    Reopen {
        /// Issue IID
        iid: u64,
    },
    /// Comment on an issue
    Comment {
        /// Issue IID
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
        /// Issue IID
        iid: u64,
        /// Skip confirmation
        #[arg(long)]
        yes: bool,
    },
}

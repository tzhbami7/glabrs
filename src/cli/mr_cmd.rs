//! clap definitions for the `mr` command group.

use clap::Subcommand;

use super::ListOpts;

/// Work with merge requests.
#[derive(Subcommand, Debug)]
pub enum MrArgs {
    /// List merge requests
    List {
        /// Filter by assignee (`@me`, `@username`)
        #[arg(long)]
        assignee: Option<String>,
        /// Filter by reviewer (`@me`, `@username`)
        #[arg(long)]
        reviewer: Option<String>,
        /// Filter by author (`@me`, `@username`)
        #[arg(long)]
        author: Option<String>,
        /// Filter by state (opened, closed, merged, all; default: opened)
        #[arg(long)]
        state: Option<String>,
        #[command(flatten)]
        /// Paging options.
        list: ListOpts,
    },
    /// Create a merge request
    Create {
        /// MR title (default: derived from commits or branches)
        #[arg(long, short = 't')]
        title: Option<String>,
        /// MR description
        #[arg(long, short = 'd')]
        description: Option<String>,
        /// Read description from file or stdin (`-`)
        #[arg(long)]
        description_file: Option<String>,
        /// Source branch (default: current)
        #[arg(long, short = 's')]
        source_branch: Option<String>,
        /// Target branch (default: default branch)
        #[arg(long, short = 'b')]
        target_branch: Option<String>,
        /// Mark as draft (prefixes the title)
        #[arg(long)]
        draft: bool,
        /// Assignee (`@me`, `@username`)
        #[arg(long)]
        assignee: Option<String>,
        /// Reviewer (`@me`, `@username`)
        #[arg(long)]
        reviewer: Option<String>,
        /// Milestone title (exact match)
        #[arg(long)]
        milestone: Option<String>,
        /// Comma-separated label names
        #[arg(long)]
        labels: Option<String>,
        /// Autodetect title/description from commits
        #[arg(long)]
        fill: bool,
        /// Commit and push the current branch first
        #[arg(long)]
        push: bool,
        /// Squash commits on merge
        #[arg(long)]
        squash: bool,
        /// Remove source branch on merge
        #[arg(long)]
        remove_source_branch: bool,
        /// Open the new MR in browser
        #[arg(long)]
        web: bool,
    },
    /// View a merge request
    View {
        /// MR IID
        iid: u64,
        /// Open in browser
        #[arg(long)]
        web: bool,
    },
    /// Show MR diff
    Diff {
        /// MR IID
        iid: u64,
    },
    /// Approve a merge request
    Approve {
        /// MR IID
        iid: u64,
    },
    /// Merge a merge request
    Merge {
        /// MR IID
        iid: u64,
        /// Squash commits on merge
        #[arg(long)]
        squash: bool,
        /// Remove source branch on merge
        #[arg(long)]
        remove_source_branch: bool,
        /// Merge when pipeline succeeds
        #[arg(long)]
        when_pipeline_succeeds: bool,
    },
    /// Close a merge request
    Close {
        /// MR IID
        iid: u64,
    },
    /// Reopen a merge request
    Reopen {
        /// MR IID
        iid: u64,
    },
    /// Comment on a merge request
    Comment {
        /// MR IID
        iid: u64,
        /// Comment body, or use --body-file
        #[arg(long)]
        body: Option<String>,
        /// Read body from file or stdin (`-`)
        #[arg(long)]
        body_file: Option<String>,
    },
    /// Mark an MR ready for review (removes WIP/Draft prefix)
    Ready {
        /// MR IID
        iid: u64,
    },
    /// Update a merge request
    Update {
        /// MR IID
        iid: u64,
        /// New title
        #[arg(long, short = 't')]
        title: Option<String>,
        /// New description
        #[arg(long, short = 'd')]
        description: Option<String>,
        /// Assignee (`@me`, `@username`)
        #[arg(long)]
        assignee: Option<String>,
        /// Reviewer (`@me`, `@username`)
        #[arg(long)]
        reviewer: Option<String>,
        /// Milestone title (exact match)
        #[arg(long)]
        milestone: Option<String>,
        /// Comma-separated label names
        #[arg(long)]
        labels: Option<String>,
    },
}

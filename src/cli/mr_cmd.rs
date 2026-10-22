use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum MrArgs {
    /// List merge requests
    List {
        #[arg(long)]
        assignee: Option<String>,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        state: Option<String>,
        #[arg(long, default_value = "20")]
        per_page: u32,
        #[arg(long)]
        page: Option<u32>,
        #[arg(long)]
        paginate: bool,
    },
    /// Create a merge request
    Create {
        #[arg(long, short = 't')]
        title: Option<String>,
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
        #[arg(long)]
        draft: bool,
        #[arg(long)]
        assignee: Option<String>,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        milestone: Option<String>,
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
        iid: u64,
        /// Open in browser
        #[arg(long)]
        web: bool,
    },
    /// Show MR diff
    Diff { iid: u64 },
    /// Approve a merge request
    Approve { iid: u64 },
    /// Merge a merge request
    Merge {
        iid: u64,
        #[arg(long)]
        squash: bool,
        #[arg(long)]
        remove_source_branch: bool,
        /// Merge when pipeline succeeds
        #[arg(long)]
        when_pipeline_succeeds: bool,
    },
    /// Close a merge request
    Close { iid: u64 },
    /// Reopen a merge request
    Reopen { iid: u64 },
    /// Comment on a merge request
    Comment {
        iid: u64,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<String>,
    },
    /// Mark an MR ready for review (removes WIP/Draft prefix)
    Ready { iid: u64 },
    /// Update a merge request
    Update {
        iid: u64,
        #[arg(long, short = 't')]
        title: Option<String>,
        #[arg(long, short = 'd')]
        description: Option<String>,
        #[arg(long)]
        assignee: Option<String>,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        milestone: Option<String>,
        #[arg(long)]
        labels: Option<String>,
    },
}

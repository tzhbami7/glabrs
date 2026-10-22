use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum RepoArgs {
    /// Show project details
    View {
        /// Project ID or owner/name path
        target: Option<String>,
        /// Open in browser
        #[arg(long)]
        web: bool,
    },
    /// List projects (in a group, or your projects)
    List {
        #[arg(long)]
        group: Option<String>,
        #[arg(long, default_value = "20")]
        per_page: u32,
        #[arg(long)]
        page: Option<u32>,
        #[arg(long)]
        paginate: bool,
    },
    /// Clone a project by owner/name or URL
    Clone {
        /// owner/name or full URL
        target: String,
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
}

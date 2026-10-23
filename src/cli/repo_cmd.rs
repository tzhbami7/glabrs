//! clap definitions for the `repo` command group.

use clap::Subcommand;

use super::ListOpts;

/// Work with repositories and projects.
#[derive(Subcommand, Debug)]
pub enum RepoArgs {
    /// Show project details
    View {
        /// Project ID or owner/name path (default: detected)
        target: Option<String>,
        /// Open in browser
        #[arg(long)]
        web: bool,
    },
    /// List projects (in a group, or your projects)
    List {
        /// Group path to list projects under (default: your projects)
        #[arg(long)]
        group: Option<String>,
        #[command(flatten)]
        /// Paging options.
        list: ListOpts,
    },
    /// Clone a project by owner/name or URL
    Clone {
        /// Project owner/name or full URL
        target: String,
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
}

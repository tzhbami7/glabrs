//! clap definitions for the `release` command group.

use clap::Subcommand;

use super::ListOpts;

/// Work with releases.
#[derive(Subcommand, Debug)]
pub enum ReleaseArgs {
    /// List releases
    List {
        #[command(flatten)]
        /// Paging options.
        list: ListOpts,
    },
    /// Create a release
    Create {
        /// Tag name
        tag: String,
        /// Read release notes from file or stdin (`-`)
        #[arg(long)]
        notes_file: Option<String>,
        /// Release name (default: the tag)
        #[arg(long)]
        name: Option<String>,
        /// Git ref to point the tag at (for new tags)
        #[arg(long)]
        ref_: Option<String>,
    },
    /// View a release
    View {
        /// Tag name
        tag: String,
    },
    /// Delete a release
    Delete {
        /// Tag name
        tag: String,
        /// Skip confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Upload a release asset (link or file)
    Upload {
        /// Tag name
        tag: String,
        /// Path to the file, or URL for a link
        target: String,
        /// Asset name (default: the file name)
        #[arg(long)]
        name: Option<String>,
        /// Treat as a link instead of uploading
        #[arg(long)]
        link: bool,
    },
}

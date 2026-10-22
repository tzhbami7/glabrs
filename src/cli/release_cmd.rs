use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum ReleaseArgs {
    /// List releases
    List {
        #[arg(long, default_value = "20")]
        per_page: u32,
        #[arg(long)]
        page: Option<u32>,
        #[arg(long)]
        paginate: bool,
    },
    /// Create a release
    Create {
        /// Tag name
        tag: String,
        /// Read release notes from file or stdin (`-`)
        #[arg(long)]
        notes_file: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        ref_: Option<String>,
    },
    /// View a release
    View { tag: String },
    /// Delete a release
    Delete {
        tag: String,
        #[arg(long)]
        yes: bool,
    },
    /// Upload a release asset (link or file)
    Upload {
        tag: String,
        /// Path to the file, or URL for a link
        target: String,
        #[arg(long)]
        name: Option<String>,
        /// Treat as a link instead of uploading
        #[arg(long)]
        link: bool,
    },
}

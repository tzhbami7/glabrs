use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum CiArgs {
    /// List pipelines
    List {
        #[arg(long)]
        ref_: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value = "20")]
        per_page: u32,
        #[arg(long)]
        page: Option<u32>,
        #[arg(long)]
        paginate: bool,
    },
    /// Show pipeline status (exit code mirrors pipeline result with --wait)
    Status {
        #[arg(long)]
        ref_: Option<String>,
        /// Block until the pipeline reaches a terminal state
        #[arg(long)]
        wait: bool,
    },
    /// View a pipeline
    View { pipeline_id: u64 },
    /// Stream a job's log (stops at terminal state, exits with job result)
    Trace {
        /// Job ID or name
        target: String,
        /// Keep following until terminal state
        #[arg(long)]
        follow: bool,
    },
    /// Run a pipeline for a ref
    Run {
        #[arg(long)]
        ref_: String,
        #[arg(long)]
        variables: Vec<String>,
    },
    /// Retry a pipeline
    Retry { pipeline_id: u64 },
    /// Cancel a pipeline
    Cancel { pipeline_id: u64 },
    /// Validate CI config
    Lint {
        /// Read .gitlab-ci.yml from file or stdin (`-`)
        #[arg(long)]
        file: Option<String>,
    },
}

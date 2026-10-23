//! clap definitions for the `ci` command group.

use clap::Subcommand;

use super::ListOpts;

/// Work with CI/CD pipelines and jobs.
#[derive(Subcommand, Debug)]
pub enum CiArgs {
    /// List pipelines
    List {
        /// Filter by Git ref
        #[arg(long)]
        ref_: Option<String>,
        /// Filter by pipeline status
        #[arg(long)]
        status: Option<String>,
        #[command(flatten)]
        /// Paging options.
        list: ListOpts,
    },
    /// Show pipeline status (exit code mirrors pipeline result with --wait)
    Status {
        /// Pipeline ref (default: latest)
        #[arg(long)]
        ref_: Option<String>,
        /// Block until the pipeline reaches a terminal state
        #[arg(long)]
        wait: bool,
        /// Give up waiting after this many seconds (with --wait)
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// View a pipeline
    View {
        /// Pipeline ID
        pipeline_id: u64,
    },
    /// Stream a job's log (stops at terminal state, exits with job result)
    Trace {
        /// Job ID or name
        target: String,
        /// Keep following until terminal state
        #[arg(long)]
        follow: bool,
        /// Give up following after this many seconds (with --follow)
        #[arg(long)]
        timeout: Option<u64>,
        /// Look the job up in this pipeline instead of the latest (IDs and
        /// names are both matched within the pipeline)
        #[arg(long)]
        pipeline: Option<u64>,
    },
    /// Run a pipeline for a ref
    Run {
        /// Git ref to run the pipeline for
        #[arg(long)]
        ref_: String,
        /// Variables in key=value form (repeatable)
        #[arg(long)]
        variables: Vec<String>,
    },
    /// Retry a pipeline
    Retry {
        /// Pipeline ID
        pipeline_id: u64,
    },
    /// Cancel a pipeline
    Cancel {
        /// Pipeline ID
        pipeline_id: u64,
    },
    /// Validate CI config
    Lint {
        /// Read .gitlab-ci.yml from file or stdin (`-`)
        #[arg(long)]
        file: Option<String>,
    },
}

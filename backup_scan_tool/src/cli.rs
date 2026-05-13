//! CLI argument definitions.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "backup_scan_tool",
    about = "Scan Moodle MBZ backup files for XML and DB-constraint correctness",
    version
)]
pub struct Cli {
    /// Directory to scan recursively, or a single .mbz file.
    pub path: PathBuf,

    /// Parallel workers. Defaults to the number of CPUs.
    #[arg(long)]
    pub jobs: Option<usize>,

    /// Only scan paths matching this glob (repeatable).
    #[arg(long)]
    pub include: Vec<String>,

    /// Skip paths matching this glob (repeatable).
    #[arg(long)]
    pub exclude: Vec<String>,

    /// Treat warnings (e.g. unknown elements) as errors.
    #[arg(long)]
    pub strict: bool,

    /// Print only a one-line summary per MBZ.
    #[arg(long)]
    pub quiet: bool,

    /// Verbose progress.
    #[arg(short, long)]
    pub verbose: bool,
}

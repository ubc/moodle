//! CLI argument definitions.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable text (the default).
    Text,
    /// Serialized Report — one JSON object with `results: [...]`.
    Json,
    /// SARIF 2.1.0 — one run per MBZ.
    Sarif,
}

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

    /// Output format: text (default), json, or sarif.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Print only a one-line summary per MBZ (text format only).
    #[arg(long)]
    pub quiet: bool,

    /// Maximum number of issues per (code, xml_file) bucket before
    /// collapsing further ones into a "... and N more" summary line. Set to
    /// 0 to disable the cap. Default 5.
    #[arg(long, default_value_t = 5)]
    pub max_issues_per_code: usize,

    /// Verbose progress.
    #[arg(short, long)]
    pub verbose: bool,
}

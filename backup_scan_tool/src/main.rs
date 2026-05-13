//! CLI entry. Discovers MBZ files, scans them in parallel, prints the report.

mod cli;

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use globset::{Glob, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use walkdir::WalkDir;

use backup_scan_tool::report::{MbzResult, Report};
use backup_scan_tool::{scanner, schema};

use crate::cli::{Cli, OutputFormat};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {:#}", e);
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    if let Some(j) = cli.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(j)
            .build_global()
            .context("configuring rayon thread pool")?;
    }

    let mbzs = discover_mbzs(&cli.path, &cli.include, &cli.exclude, cli.verbose)?;
    if mbzs.is_empty() {
        eprintln!("No .mbz files found under {}", cli.path.display());
        return Ok(ExitCode::from(0));
    }

    if cli.verbose {
        eprintln!("Found {} .mbz file(s) to scan", mbzs.len());
    }

    let schema = schema::schema();

    let results: Vec<MbzResult> = mbzs
        .par_iter()
        .map(|p| scanner::scan_mbz(p, &schema, cli.strict))
        .collect();

    let report = Report { results };

    let stdout = io::stdout();
    let mut out = stdout.lock();
    match cli.format {
        OutputFormat::Text => {
            let color = out.is_terminal();
            report.print_text(&mut out, color, cli.quiet, cli.max_issues_per_code)?;
        }
        OutputFormat::Json => {
            report.print_json(&mut out)?;
        }
        OutputFormat::Sarif => {
            report.print_sarif(&mut out)?;
        }
    }
    let _ = out.flush();

    let errors = report.total_errors() + report.fatal_count();
    if errors > 0 {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::from(0))
    }
}

fn discover_mbzs(
    root: &Path,
    include: &[String],
    exclude: &[String],
    verbose: bool,
) -> Result<Vec<PathBuf>> {
    let include_set = build_globset(include).context("building --include glob")?;
    let exclude_set = build_globset(exclude).context("building --exclude glob")?;

    let md = std::fs::metadata(root)
        .with_context(|| format!("stat {}", root.display()))?;

    let mut out = Vec::new();
    if md.is_file() {
        if root.extension().map(|e| e == "mbz").unwrap_or(false) {
            out.push(root.to_path_buf());
        } else {
            anyhow::bail!("{} is not an .mbz file", root.display());
        }
    } else {
        for entry in WalkDir::new(root).follow_links(false).into_iter() {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.into_path();
            if !p.extension().map(|e| e == "mbz").unwrap_or(false) {
                continue;
            }
            if let Some(set) = &include_set {
                if !set.is_match(&p) {
                    continue;
                }
            }
            if let Some(set) = &exclude_set {
                if set.is_match(&p) {
                    if verbose {
                        eprintln!("skipping (excluded): {}", p.display());
                    }
                    continue;
                }
            }
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(Glob::new(p)?);
    }
    Ok(Some(b.build()?))
}

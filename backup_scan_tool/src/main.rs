//! CLI entry. Discovers MBZ files, scans them in parallel, prints the report.

mod cli;

use std::fs::File;
use std::io::{self, BufWriter, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

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

    let total = mbzs.len();
    let show_progress = !cli.no_progress;
    let started = Instant::now();
    if show_progress {
        eprintln!("Scanning {} MBZ file(s)...", total);
    }
    let completed = AtomicUsize::new(0);

    let results: Vec<MbzResult> = mbzs
        .par_iter()
        .map(|p| {
            let r = scanner::scan_mbz(p, &schema, cli.strict);
            if show_progress {
                let n = completed.fetch_add(1, Ordering::Relaxed) + 1;
                let errors = r.errors() + r.fatal.is_some() as usize;
                let warnings = r.warnings();
                // One line per MBZ, locked write to keep parallel output
                // from interleaving mid-line.
                let stderr = io::stderr();
                let mut h = stderr.lock();
                let _ = writeln!(
                    h,
                    "[{}/{}] {} — {} error(s), {} warning(s)",
                    n,
                    total,
                    p.display(),
                    errors,
                    warnings
                );
            }
            r
        })
        .collect();

    let report = Report { results };

    if show_progress {
        let elapsed = started.elapsed();
        eprintln!(
            "Done in {:.1}s — {} MBZ file(s) scanned, {} error(s), {} warning(s), {} fatal",
            elapsed.as_secs_f64(),
            total,
            report.total_errors(),
            report.total_warnings(),
            report.fatal_count(),
        );
    }

    // Pick where the full report goes: --output file, else stdout. The
    // sink type differs (BufWriter<File> vs StdoutLock), so we branch on
    // it rather than carrying a Box<dyn Write> through the format match
    // (which would lose IsTerminal for color detection on the stdout path).
    if let Some(out_path) = &cli.output {
        let f = File::create(out_path)
            .with_context(|| format!("creating output file {}", out_path.display()))?;
        let mut out = BufWriter::new(f);
        match cli.format {
            OutputFormat::Text => {
                // No ANSI colour codes in a file.
                report.print_text(&mut out, false, cli.quiet, cli.max_issues_per_code)?;
            }
            OutputFormat::Json => report.print_json(&mut out)?,
            OutputFormat::Sarif => report.print_sarif(&mut out)?,
        }
        out.flush().ok();
        if show_progress {
            eprintln!("Report written to {}", out_path.display());
        }
    } else {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        match cli.format {
            OutputFormat::Text => {
                let color = out.is_terminal();
                report.print_text(&mut out, color, cli.quiet, cli.max_issues_per_code)?;
            }
            OutputFormat::Json => report.print_json(&mut out)?,
            OutputFormat::Sarif => report.print_sarif(&mut out)?,
        }
        let _ = out.flush();
    }

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

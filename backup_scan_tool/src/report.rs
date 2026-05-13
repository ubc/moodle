//! Aggregate scan results into a Report and pretty-print it.

use std::io::Write;
use std::path::PathBuf;

use owo_colors::OwoColorize;
use serde::Serialize;

use crate::validate::{Issue, Severity};

#[derive(Debug, Serialize)]
pub struct MbzResult {
    pub path: PathBuf,
    pub xml_count: usize,
    pub issues: Vec<Issue>,
    /// Set when the MBZ itself failed to open or another fatal error stopped scanning.
    pub fatal: Option<String>,
}

impl MbzResult {
    pub fn errors(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
            .count()
    }
}

#[derive(Debug, Serialize, Default)]
pub struct Report {
    pub results: Vec<MbzResult>,
}

impl Report {
    pub fn total_errors(&self) -> usize {
        self.results.iter().map(|r| r.errors()).sum()
    }

    pub fn total_warnings(&self) -> usize {
        self.results.iter().map(|r| r.warnings()).sum()
    }

    pub fn fatal_count(&self) -> usize {
        self.results.iter().filter(|r| r.fatal.is_some()).count()
    }

    pub fn print_text<W: Write>(&self, mut out: W, color: bool, quiet: bool) -> std::io::Result<()> {
        for r in &self.results {
            let header = format!("== {} ==", r.path.display());
            if color {
                writeln!(out, "{}", header.bold())?;
            } else {
                writeln!(out, "{}", header)?;
            }
            if let Some(fatal) = &r.fatal {
                let line = format!("  FATAL: {}", fatal);
                if color {
                    writeln!(out, "{}", line.red())?;
                } else {
                    writeln!(out, "{}", line)?;
                }
                continue;
            }
            let errors = r.errors();
            let warnings = r.warnings();
            let summary = format!(
                "  scanned {} XML file(s) — {} error(s), {} warning(s)",
                r.xml_count, errors, warnings
            );
            writeln!(out, "{}", summary)?;
            if quiet {
                continue;
            }
            for issue in &r.issues {
                let tag = match issue.severity {
                    Severity::Error => "ERROR",
                    Severity::Warning => "WARN ",
                };
                let line = format!(
                    "  [{}] {}:{} {}",
                    tag, issue.xml_file, issue.location, issue.message
                );
                if color {
                    match issue.severity {
                        Severity::Error => writeln!(out, "{}", line.red())?,
                        Severity::Warning => writeln!(out, "{}", line.yellow())?,
                    }
                } else {
                    writeln!(out, "{}", line)?;
                }
            }
        }

        let total_e = self.total_errors();
        let total_w = self.total_warnings();
        let total_f = self.fatal_count();
        let n = self.results.len();
        let summary = format!(
            "\nTotal: {} MBZ file(s) scanned, {} error(s), {} warning(s), {} fatal",
            n, total_e, total_w, total_f
        );
        if color && (total_e > 0 || total_f > 0) {
            writeln!(out, "{}", summary.red().bold())?;
        } else if color && total_w > 0 {
            writeln!(out, "{}", summary.yellow())?;
        } else if color {
            writeln!(out, "{}", summary.green())?;
        } else {
            writeln!(out, "{}", summary)?;
        }
        Ok(())
    }
}

//! Aggregate scan results into a Report and pretty-print it.
//!
//! Three output formats:
//! - text: human-readable, colorized when stdout is a TTY (default)
//! - json: serde-derived JSON of the whole Report
//! - sarif: SARIF 2.1.0 with one `run` per MBZ; issue `code` becomes `ruleId`

use std::io::Write;
use std::path::PathBuf;

use owo_colors::OwoColorize;
use serde::Serialize;
use serde_json::{json, Value};

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

    pub fn print_text<W: Write>(
        &self,
        mut out: W,
        color: bool,
        quiet: bool,
        max_issues_per_code: usize,
    ) -> std::io::Result<()> {
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

            // Walk issues in original order, but cap per (code, xml_file)
            // bucket. We don't reorder so that the user can correlate with
            // the JSON/SARIF outputs.
            let mut seen_count: std::collections::HashMap<(&str, &str), usize> =
                std::collections::HashMap::new();
            let mut suppressed: std::collections::HashMap<(&str, &str), (usize, Severity)> =
                std::collections::HashMap::new();

            for issue in &r.issues {
                let bkey: (&str, &str) = (issue.code, issue.xml_file.as_str());
                let count = seen_count.entry(bkey).or_insert(0);
                *count += 1;
                if max_issues_per_code > 0 && *count > max_issues_per_code {
                    let entry = suppressed.entry(bkey).or_insert((0, issue.severity));
                    entry.0 += 1;
                    continue;
                }
                let tag = match issue.severity {
                    Severity::Error => "ERROR",
                    Severity::Warning => "WARN ",
                };
                let line = format!(
                    "  [{}] [{}] {}:{} {}",
                    tag, issue.code, issue.xml_file, issue.location, issue.message
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

            // Emit one summary line per suppressed bucket.
            let mut sup: Vec<((&str, &str), (usize, Severity))> = suppressed.into_iter().collect();
            sup.sort_by_key(|((code, file), _)| (*code, *file));
            for ((code, file), (n, sev)) in sup {
                let line = format!(
                    "  [{:5}] [{}] {}: ... and {} more issue(s) with this code",
                    match sev {
                        Severity::Error => "ERROR",
                        Severity::Warning => "WARN ",
                    },
                    code,
                    file,
                    n
                );
                if color {
                    match sev {
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

        // Top-10 code summary across all MBZs. Helps users staring at huge
        // reports understand the shape of failure.
        let by_code = self.tally_by_code();
        if !by_code.is_empty() {
            writeln!(out, "\nTop issue codes:")?;
            for (code, (e, w)) in by_code.iter().take(10) {
                writeln!(out, "  {:<36} errors={:<6} warnings={}", code, e, w)?;
            }
        }
        Ok(())
    }

    /// Sorted by (errors+warnings) descending, then code asc.
    fn tally_by_code(&self) -> Vec<(&'static str, (usize, usize))> {
        let mut m: std::collections::HashMap<&'static str, (usize, usize)> =
            std::collections::HashMap::new();
        for r in &self.results {
            for i in &r.issues {
                let e = m.entry(i.code).or_default();
                match i.severity {
                    Severity::Error => e.0 += 1,
                    Severity::Warning => e.1 += 1,
                }
            }
        }
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by(|a, b| (b.1 .0 + b.1 .1).cmp(&(a.1 .0 + a.1 .1)).then(a.0.cmp(b.0)));
        v
    }

    pub fn print_json<W: Write>(&self, mut out: W) -> std::io::Result<()> {
        let s = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        writeln!(out, "{}", s)
    }

    pub fn print_sarif<W: Write>(&self, mut out: W) -> std::io::Result<()> {
        let v = self.to_sarif();
        let s = serde_json::to_string_pretty(&v)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        writeln!(out, "{}", s)
    }

    fn to_sarif(&self) -> Value {
        let runs: Vec<Value> = self.results.iter().map(mbz_to_sarif_run).collect();
        json!({
            "version": "2.1.0",
            "$schema": "https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/schemas/sarif-schema-2.1.0.json",
            "runs": runs,
        })
    }
}

fn mbz_to_sarif_run(r: &MbzResult) -> Value {
    let mut results = Vec::with_capacity(r.issues.len());
    if let Some(fatal) = &r.fatal {
        results.push(json!({
            "ruleId": "MBZ-ARCHIVE-FATAL",
            "level": "error",
            "message": { "text": fatal },
            "locations": [{
                "physicalLocation": {
                    "artifactLocation": { "uri": r.path.display().to_string() },
                },
            }],
            "partialFingerprints": {
                "primary": format!("MBZ-ARCHIVE-FATAL:{}", r.path.display()),
            },
        }));
    }
    for issue in &r.issues {
        results.push(issue_to_sarif_result(&r.path, issue));
    }
    let rules: Vec<Value> = collect_rule_ids(r)
        .into_iter()
        .map(|code| json!({ "id": code }))
        .collect();
    json!({
        "tool": {
            "driver": {
                "name": "backup_scan_tool",
                "version": env!("CARGO_PKG_VERSION"),
                "informationUri": "https://moodle.org",
                "rules": rules,
            },
        },
        "results": results,
        "properties": {
            "mbz": r.path.display().to_string(),
            "xmlCount": r.xml_count,
        },
    })
}

fn issue_to_sarif_result(mbz_path: &PathBuf, issue: &Issue) -> Value {
    let level = match issue.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let mut physical = json!({
        "artifactLocation": { "uri": issue.xml_file.clone() },
    });
    if !issue.location.is_empty() {
        physical["region"] = json!({ "message": { "text": issue.location.clone() } });
    }
    json!({
        "ruleId": issue.code,
        "level": level,
        "message": { "text": issue.message.clone() },
        "locations": [{
            "physicalLocation": physical,
            "logicalLocations": [{ "fullyQualifiedName": issue.location.clone() }],
        }],
        "partialFingerprints": {
            "primary": format!(
                "{}|{}|{}|{}",
                issue.code,
                mbz_path.display(),
                issue.xml_file,
                issue.location
            ),
        },
    })
}

fn collect_rule_ids(r: &MbzResult) -> Vec<&'static str> {
    let mut seen = std::collections::BTreeSet::new();
    for i in &r.issues {
        seen.insert(i.code);
    }
    if r.fatal.is_some() {
        seen.insert("MBZ-ARCHIVE-FATAL");
    }
    seen.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::codes;
    use serde_json::Value;
    use std::path::PathBuf;

    fn sample_report() -> Report {
        Report {
            results: vec![MbzResult {
                path: PathBuf::from("/tmp/x.mbz"),
                xml_count: 3,
                fatal: None,
                issues: vec![
                    Issue::error(
                        codes::STRUCT_CHAR_OVERFLOW,
                        "course/course.xml",
                        "/course/fullname",
                        "char field <fullname> length 300 exceeds LENGTH=\"254\"",
                    ),
                    Issue::warning(
                        codes::STRUCT_UNKNOWN_LEAF,
                        "activities/foo_1/foo.xml",
                        "/activity/foo/bar",
                        "unknown leaf <bar> under <foo>",
                    ),
                ],
            }],
        }
    }

    #[test]
    fn json_roundtrip_preserves_code() {
        let r = sample_report();
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: Value = serde_json::from_slice(&buf).unwrap();
        let first = &v["results"][0]["issues"][0];
        assert_eq!(first["code"].as_str(), Some(codes::STRUCT_CHAR_OVERFLOW));
        assert_eq!(first["severity"].as_str(), Some("Error"));
        let second = &v["results"][0]["issues"][1];
        assert_eq!(second["code"].as_str(), Some(codes::STRUCT_UNKNOWN_LEAF));
        assert_eq!(second["severity"].as_str(), Some("Warning"));
    }

    #[test]
    fn sarif_has_driver_name_and_rule_ids() {
        let r = sample_report();
        let mut buf = Vec::new();
        r.print_sarif(&mut buf).unwrap();
        let v: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["version"].as_str(), Some("2.1.0"));
        let driver = &v["runs"][0]["tool"]["driver"];
        assert_eq!(driver["name"].as_str(), Some("backup_scan_tool"));
        assert!(driver["version"].is_string());

        let results = v["runs"][0]["results"].as_array().expect("results array");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["ruleId"].as_str(), Some(codes::STRUCT_CHAR_OVERFLOW));
        assert_eq!(results[0]["level"].as_str(), Some("error"));
        assert_eq!(results[1]["ruleId"].as_str(), Some(codes::STRUCT_UNKNOWN_LEAF));
        assert_eq!(results[1]["level"].as_str(), Some("warning"));
        // Fingerprint present for stability.
        assert!(results[0]["partialFingerprints"]["primary"].is_string());
        // Rule catalog mirrors the codes we emitted.
        let rule_ids: Vec<&str> = driver["rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect();
        assert!(rule_ids.contains(&codes::STRUCT_CHAR_OVERFLOW));
        assert!(rule_ids.contains(&codes::STRUCT_UNKNOWN_LEAF));
    }

    #[test]
    fn fatal_becomes_archive_fatal_result_in_sarif() {
        let r = Report {
            results: vec![MbzResult {
                path: PathBuf::from("/tmp/broken.mbz"),
                xml_count: 0,
                fatal: Some("could not open archive".to_string()),
                issues: vec![],
            }],
        };
        let mut buf = Vec::new();
        r.print_sarif(&mut buf).unwrap();
        let v: Value = serde_json::from_slice(&buf).unwrap();
        let results = v["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["ruleId"].as_str(), Some("MBZ-ARCHIVE-FATAL"));
        assert_eq!(results[0]["level"].as_str(), Some("error"));
    }
}

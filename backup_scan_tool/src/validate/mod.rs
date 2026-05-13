//! Validation core: issue types, severity, and the per-XML-file validators.

use serde::Serialize;

pub mod structure;
pub mod well_formed;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub severity: Severity,
    /// Entry name inside the MBZ (e.g. "activities/assign_3/assign.xml").
    pub xml_file: String,
    /// XPath-ish location, e.g. "/activity/assign/name" or "" for file-level.
    pub location: String,
    pub message: String,
}

impl Issue {
    pub fn error(xml_file: impl Into<String>, location: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            xml_file: xml_file.into(),
            location: location.into(),
            message: msg.into(),
        }
    }

    pub fn warning(xml_file: impl Into<String>, location: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            xml_file: xml_file.into(),
            location: location.into(),
            message: msg.into(),
        }
    }
}

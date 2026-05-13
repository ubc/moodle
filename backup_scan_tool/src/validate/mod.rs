//! Validation core: issue types, severity, codes, and per-XML-file validators.

use serde::Serialize;

pub mod codes;
pub mod completeness;
pub mod cross_ref;
pub mod safety;
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
    /// Stable machine-readable identifier, e.g. "MBZ-STRUCT-CHAR-OVERFLOW".
    /// Consumers (CI, dashboards) should key off this rather than the message.
    pub code: &'static str,
    /// Entry name inside the MBZ (e.g. "activities/assign_3/assign.xml").
    pub xml_file: String,
    /// XPath-ish location, e.g. "/activity/assign/name" or "" for file-level.
    pub location: String,
    pub message: String,
}

impl Issue {
    pub fn error(
        code: &'static str,
        xml_file: impl Into<String>,
        location: impl Into<String>,
        msg: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Error,
            code,
            xml_file: xml_file.into(),
            location: location.into(),
            message: msg.into(),
        }
    }

    pub fn warning(
        code: &'static str,
        xml_file: impl Into<String>,
        location: impl Into<String>,
        msg: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            xml_file: xml_file.into(),
            location: location.into(),
            message: msg.into(),
        }
    }
}

//! Archive-level safety: reject entry names that could escape the extraction
//! root, or that contain shapes a Moodle restore wouldn't accept.
//!
//! Moodle's `file_packer::extract_to_pathname` is the actual extractor at
//! restore time; it does not perform a defence-in-depth path-traversal check
//! itself. We do one here so callers can refuse hostile archives *before*
//! invoking restore.

use super::codes;
use super::Issue;
use crate::mbz::MbzArchive;

/// Returns one issue per offending entry name (capped output is unnecessary
/// here — these are rare).
pub fn check_entry_names(archive: &MbzArchive) -> Vec<Issue> {
    let mut issues = Vec::new();
    for name in archive.names() {
        if let Some(reason) = unsafe_reason(name) {
            issues.push(Issue::error(
                codes::SAFETY_PATH_TRAVERSAL,
                name.clone(),
                String::new(),
                format!("unsafe archive entry name: {}", reason),
            ));
        }
    }
    issues
}

/// Returns a human-readable reason if `name` is unsafe, or None if safe.
/// Rules:
/// - any component equal to ".." → traversal
/// - leading "/" or "\" (absolute path)
/// - Windows drive letter like "C:" anywhere
/// - NUL byte
fn unsafe_reason(name: &str) -> Option<&'static str> {
    if name.contains('\0') {
        return Some("contains NUL byte");
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Some("absolute path");
    }
    // Drive letter at start (Windows): X: or X:/
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && (bytes[0] as char).is_ascii_alphabetic() {
        return Some("Windows drive letter");
    }
    // Any component equal to "..".
    for comp in name.split(|c| c == '/' || c == '\\') {
        if comp == ".." {
            return Some("contains \"..\" path component");
        }
    }
    None
}

/// Hex-digit format checks for moodle_backup.xml `<backup_id>` (32 hex chars).
pub fn check_backup_id(xml_bytes: &[u8]) -> Vec<Issue> {
    let mut issues = Vec::new();
    if let Some(id) = extract_single_text(xml_bytes, b"backup_id") {
        if !(id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())) {
            issues.push(Issue::error(
                codes::MANIFEST_BAD_BACKUPID,
                "moodle_backup.xml",
                "/moodle_backup/information/backup_id".to_string(),
                format!(
                    "<backup_id> should be 32 hex characters, got {:?} (length {})",
                    id,
                    id.len()
                ),
            ));
        }
    }
    issues
}

/// 40-hex `<contenthash>` format check across all entries in files.xml.
/// Capped at one issue summarising the first bad value to avoid floods.
pub fn check_contenthash_format(xml_bytes: &[u8]) -> Vec<Issue> {
    let hashes = extract_all_text(xml_bytes, b"contenthash");
    let bad: Vec<&str> = hashes
        .iter()
        .filter(|h| {
            !(h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()))
        })
        .map(String::as_str)
        .collect();
    if bad.is_empty() {
        return Vec::new();
    }
    let n = bad.len();
    let first = bad[0];
    let detail = if n == 1 {
        format!(
            "<contenthash> value is not 40-hex sha1: {:?} (length {})",
            first,
            first.len()
        )
    } else {
        format!(
            "{} <contenthash> values are not 40-hex sha1 (first: {:?}, length {})",
            n,
            first,
            first.len()
        )
    };
    vec![Issue::error(
        codes::MANIFEST_BAD_CONTENTHASH,
        "files.xml",
        "/files".to_string(),
        detail,
    )]
}

fn extract_single_text(bytes: &[u8], target: &[u8]) -> Option<String> {
    let values = extract_all_text(bytes, target);
    values.into_iter().next()
}

fn extract_all_text(bytes: &[u8], target: &[u8]) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut in_target = false;
    let mut current = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == target {
                    in_target = true;
                    current.clear();
                }
            }
            Ok(Event::Text(t)) => {
                if in_target {
                    if let Ok(s) = t.unescape() {
                        current.push_str(&s);
                    }
                }
            }
            Ok(Event::CData(c)) => {
                if in_target {
                    current.push_str(&String::from_utf8_lossy(c.as_ref()));
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == target {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        out.push(trimmed.to_string());
                    }
                    in_target = false;
                    current.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_double_dot_component() {
        assert!(unsafe_reason("../etc/passwd").is_some());
        assert!(unsafe_reason("foo/../bar").is_some());
        assert!(unsafe_reason("foo/..").is_some());
    }

    #[test]
    fn flags_absolute_paths() {
        assert!(unsafe_reason("/etc/passwd").is_some());
        assert!(unsafe_reason("\\windows\\system32").is_some());
    }

    #[test]
    fn flags_drive_letter() {
        assert!(unsafe_reason("C:/Windows").is_some());
        assert!(unsafe_reason("z:foo").is_some());
    }

    #[test]
    fn allows_normal_paths() {
        assert!(unsafe_reason("activities/assign_5/module.xml").is_none());
        assert!(unsafe_reason("files/aa/aaaa").is_none());
        assert!(unsafe_reason("moodle_backup.xml").is_none());
    }

    #[test]
    fn backup_id_short_is_flagged() {
        let xml = br#"<moodle_backup><information><backup_id>abc</backup_id></information></moodle_backup>"#;
        let issues = check_backup_id(xml);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, codes::MANIFEST_BAD_BACKUPID);
    }

    #[test]
    fn backup_id_valid_is_silent() {
        let xml = br#"<moodle_backup><information><backup_id>0123456789abcdef0123456789abcdef</backup_id></information></moodle_backup>"#;
        assert!(check_backup_id(xml).is_empty());
    }

    #[test]
    fn contenthash_short_is_flagged() {
        let xml = br#"<files><file id="1"><contenthash>tooshort</contenthash></file></files>"#;
        let issues = check_contenthash_format(xml);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, codes::MANIFEST_BAD_CONTENTHASH);
    }

    #[test]
    fn contenthash_valid_is_silent() {
        let xml = br#"<files>
            <file id="1"><contenthash>0123456789abcdef0123456789abcdef01234567</contenthash></file>
        </files>"#;
        assert!(check_contenthash_format(xml).is_empty());
    }
}

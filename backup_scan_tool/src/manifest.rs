//! Cross-XML integrity checks against the MBZ central directory:
//!
//! - moodle_backup.xml lists activities/sections with directory paths; we
//!   verify each referenced directory has a `.xml` entry inside the zip.
//! - files.xml lists files with contenthash; we verify each non-empty hash
//!   maps to a `files/XX/HASH` entry in the zip (presence only — no payload reads).

use std::collections::HashSet;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::mbz::MbzArchive;
use crate::validate::Issue;

pub fn check_moodle_backup_manifest(
    archive: &MbzArchive,
    xml_bytes: &[u8],
) -> Vec<Issue> {
    let mut issues = Vec::new();
    let directories = extract_referenced_directories(xml_bytes);

    let entry_names: HashSet<&str> = archive.names().iter().map(|s| s.as_str()).collect();

    for dir in &directories {
        // The directory may be referenced as e.g. "activities/assign_3".
        // Look for any zip entry under that directory.
        let prefix = if dir.ends_with('/') {
            dir.clone()
        } else {
            format!("{}/", dir)
        };
        let has_any_child = entry_names.iter().any(|n| n.starts_with(&prefix));
        if !has_any_child {
            issues.push(Issue::error(
                "moodle_backup.xml",
                format!("/moodle_backup/.../directory={}", dir),
                format!(
                    "manifest references directory {:?} but no zip entries live under it",
                    dir
                ),
            ));
        }
    }

    issues
}

fn extract_referenced_directories(xml_bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut reader = Reader::from_reader(xml_bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut current_elem: Option<Vec<u8>> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                current_elem = Some(e.name().as_ref().to_vec());
                // <activity directory="...">, <section directory="...">, <course directory="...">
                if matches!(e.name().as_ref(), b"activity" | b"section" | b"course") {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"directory" {
                            let v = String::from_utf8_lossy(&attr.value).into_owned();
                            if !v.is_empty() {
                                out.push(v);
                            }
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                if matches!(e.name().as_ref(), b"activity" | b"section" | b"course") {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"directory" {
                            let v = String::from_utf8_lossy(&attr.value).into_owned();
                            if !v.is_empty() {
                                out.push(v);
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                // Also capture <directory>...</directory> if it appears as element text.
                if let Some(elem) = &current_elem {
                    if elem == b"directory" {
                        let v = t.unescape().unwrap_or_default().into_owned();
                        if !v.is_empty() {
                            out.push(v);
                        }
                    }
                }
            }
            Ok(Event::End(_)) => {
                current_elem = None;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    out
}

pub fn check_files_xml(archive: &MbzArchive, xml_bytes: &[u8]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let entry_names: HashSet<&str> = archive.names().iter().map(|s| s.as_str()).collect();
    let hashes = extract_contenthashes(xml_bytes);

    let mut missing: usize = 0;
    let mut first_missing: Option<String> = None;
    for h in &hashes {
        if h.len() < 2 {
            continue;
        }
        let prefix = &h[..2];
        let expected = format!("files/{}/{}", prefix, h);
        if !entry_names.contains(expected.as_str()) {
            missing += 1;
            if first_missing.is_none() {
                first_missing = Some(h.clone());
            }
        }
    }

    if missing > 0 {
        let detail = match first_missing {
            Some(h) => format!(
                "{} contenthash(es) in files.xml have no matching files/XX/HASH entry (first: {})",
                missing, h
            ),
            None => format!(
                "{} contenthash(es) in files.xml have no matching files/XX/HASH entry",
                missing
            ),
        };
        issues.push(Issue::error("files.xml", "/files".to_string(), detail));
    }

    issues
}

fn extract_contenthashes(xml_bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut reader = Reader::from_reader(xml_bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_contenthash = false;
    let mut current = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"contenthash" {
                    in_contenthash = true;
                    current.clear();
                }
            }
            Ok(Event::Text(t)) => {
                if in_contenthash {
                    current.push_str(&t.unescape().unwrap_or_default());
                }
            }
            Ok(Event::CData(c)) => {
                if in_contenthash {
                    current.push_str(&String::from_utf8_lossy(c.as_ref()));
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"contenthash" {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        out.push(trimmed.to_string());
                    }
                    in_contenthash = false;
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

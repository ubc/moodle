//! Per-MBZ scan: open the archive, iterate XML entries, run validators.

use std::path::{Path, PathBuf};

use crate::manifest;
use crate::mbz::MbzArchive;
use crate::report::MbzResult;
use crate::schema::Schema;
use crate::validate::completeness::{self, Manifest};
use crate::validate::cross_ref::{self, ReferenceIndex};
use crate::validate::safety;
use crate::validate::{self, codes, structure::StructureOptions};

/// Top-level XML names that feed the cross-reference index. Listing them
/// explicitly avoids scanning every XML for ids — the index only knows about
/// the *registries* of users/files/roles/etc, not about every backup row.
///
/// Note: `activities/<modname>_<cmid>/grades.xml` also carries `<grade_item>`
/// rows (the activity-specific grade items, distinct from the calculated /
/// category items in root `gradebook.xml`). Both are ingested into the same
/// `grade_items` set so that `inforef.xml` grade_item refs resolve regardless
/// of which file owns the item. See `is_activity_grades_xml`.
const INDEX_SOURCES: &[&str] = &[
    "users.xml",
    "files.xml",
    "roles.xml",
    "groups.xml",
    "scales.xml",
    "outcomes.xml",
    "gradebook.xml",
    "questions.xml",
];

fn is_activity_grades_xml(name: &str) -> bool {
    name.starts_with("activities/") && name.ends_with("/grades.xml")
}

pub fn scan_mbz(path: &Path, schema: &Schema, strict: bool) -> MbzResult {
    let mut result = MbzResult {
        path: PathBuf::from(path),
        xml_count: 0,
        issues: Vec::new(),
        fatal: None,
    };

    let mut archive = match MbzArchive::open(path) {
        Ok(a) => a,
        Err(e) => {
            result.fatal = Some(format!("{:#}", e));
            return result;
        }
    };

    let xml_names: Vec<String> = archive.xml_names().map(|s| s.to_string()).collect();
    result.xml_count = xml_names.len();

    // Archive-level safety: check every entry name (including non-XML) for
    // path traversal / absolute paths / drive letters / NUL bytes.
    result.issues.append(&mut safety::check_entry_names(&archive));

    let opts = StructureOptions { strict };

    // First pass: per-XML validators. We also lazily build the reference
    // index from any top-level index-source files we encounter, and stash
    // the parsed manifest + files.xml bytes for second-pass shape checks.
    let mut index = ReferenceIndex::default();
    let mut manifest: Option<Manifest> = None;
    let mut files_xml_bytes: Option<Vec<u8>> = None;

    for name in &xml_names {
        let bytes = match archive.read_to_vec(name) {
            Ok(b) => b,
            Err(e) => {
                result.issues.push(crate::validate::Issue::error(
                    codes::ARCHIVE_ENTRY_READ,
                    name.clone(),
                    String::new(),
                    format!("could not read entry: {:#}", e),
                ));
                continue;
            }
        };

        if let Some(issue) = validate::well_formed::check(name, &bytes) {
            result.issues.push(issue);
            // Well-formedness errors mean we can't trust structure analysis.
            continue;
        }

        let mut struct_issues = validate::structure::check(name, &bytes, schema, &opts);
        result.issues.append(&mut struct_issues);

        if name == "moodle_backup.xml" {
            let mut manifest_issues = manifest::check_moodle_backup_manifest(&archive, &bytes);
            result.issues.append(&mut manifest_issues);
            result.issues.append(&mut safety::check_backup_id(&bytes));
            manifest = Some(completeness::parse_manifest(&bytes));
        } else if name == "files.xml" {
            let mut file_issues = manifest::check_files_xml(&archive, &bytes);
            result.issues.append(&mut file_issues);
            result
                .issues
                .append(&mut safety::check_contenthash_format(&bytes));
            files_xml_bytes = Some(bytes.clone());
        }

        if INDEX_SOURCES.contains(&name.as_str()) {
            index.ingest(name, &bytes);
        } else if is_activity_grades_xml(name) {
            // The activity-specific grade_items live under
            // <activity_gradebook><grade_items><grade_item id=N>. ReferenceIndex::ingest
            // doesn't know that filename, so we drive collect_grade_items
            // directly. Treat presence as a "grade source" for the index's
            // present-set so inforef checks aren't suppressed when only
            // gradebook.xml is empty.
            cross_ref::ingest_activity_grades(&mut index, &bytes);
        }
    }

    // Shape & completeness — driven off the parsed moodle_backup.xml.
    if let Some(m) = &manifest {
        let mut shape_issues = completeness::check(&archive, m);
        result.issues.append(&mut shape_issues);
    }

    // Orphan file payloads — zip has files/XX/HASH entries not referenced
    // by files.xml. Only meaningful when files.xml was present and parseable.
    if let Some(bytes) = &files_xml_bytes {
        let mut orphans = completeness::check_orphan_files(&archive, bytes);
        result.issues.append(&mut orphans);
    }

    // Second pass: cross-XML refs from every inforef.xml against the index.
    // Re-reading each inforef.xml is cheap — they're small. We can't fold this
    // into the first pass because the index isn't fully populated until we've
    // visited all index sources (which may sort after some inforef.xml's).
    for name in &xml_names {
        if !cross_ref::is_inforef(name) {
            continue;
        }
        let bytes = match archive.read_to_vec(name) {
            Ok(b) => b,
            Err(_) => continue, // already reported in pass 1
        };
        let mut xref_issues = cross_ref::check_inforef(name, &bytes, &index);
        result.issues.append(&mut xref_issues);
    }

    result
}

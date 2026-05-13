//! Per-MBZ scan: open the archive, iterate XML entries, run validators.

use std::path::{Path, PathBuf};

use crate::manifest;
use crate::mbz::MbzArchive;
use crate::report::MbzResult;
use crate::schema::Schema;
use crate::validate::{self, structure::StructureOptions};

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

    let opts = StructureOptions { strict };

    for name in &xml_names {
        let bytes = match archive.read_to_vec(name) {
            Ok(b) => b,
            Err(e) => {
                result.issues.push(crate::validate::Issue::error(
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
        } else if name == "files.xml" {
            let mut file_issues = manifest::check_files_xml(&archive, &bytes);
            result.issues.append(&mut file_issues);
        }
    }

    result
}

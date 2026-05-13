//! Build-time schema baker.
//!
//! Walks $MOODLE_SCHEMA_ROOT (default: ../) for every db/install.xml, parses
//! them into a Schema, and bincode-serializes the result to $OUT_DIR/schema.bin
//! so the runtime crate can include_bytes!() it.
//!
//! Also walks `**/backup/moodle2/backup_*_stepslib.php` and extracts the
//! `set_source_alias(dbcol, xmlelem)` calls so the runtime validator can
//! resolve XML element names that don't directly match install.xml columns
//! (e.g. quiz's `<attempts_number>` alias for the `attempts` column).

#[path = "src/xmldb_shared.rs"]
#[allow(dead_code)]
mod xmldb_shared;

use std::env;
use std::path::{Path, PathBuf};

use regex::Regex;
use xmldb_shared::{LoadOutcome, Schema};

fn main() {
    let moodle_root: PathBuf = env::var_os("MOODLE_SCHEMA_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".."));
    let moodle_root = moodle_root
        .canonicalize()
        .unwrap_or_else(|e| panic!("MOODLE_SCHEMA_ROOT {:?}: {}", moodle_root, e));

    println!("cargo:rerun-if-env-changed=MOODLE_SCHEMA_ROOT");
    println!("cargo:rerun-if-changed=src/xmldb_shared.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let install_xmls = find_install_xmls(&moodle_root);
    let mut schema = Schema::default();
    let mut total_tables = 0usize;
    let mut conflicts = 0usize;

    for path in &install_xmls {
        // Re-bake when any install.xml changes.
        println!("cargo:rerun-if-changed={}", path.display());

        match schema.merge_install_xml(path, &moodle_root) {
            Ok(outcomes) => {
                for o in outcomes {
                    match o {
                        LoadOutcome::Inserted => total_tables += 1,
                        LoadOutcome::Conflict { existing_plugin } => {
                            conflicts += 1;
                            println!(
                                "cargo:warning=duplicate table from {} (first defined in {})",
                                path.display(),
                                existing_plugin
                            );
                        }
                    }
                }
            }
            Err(e) => {
                println!("cargo:warning={}", e);
            }
        }
    }

    // Now scan backup stepslibs for aliases and attach to the appropriate tables.
    let stepslibs = find_stepslib_php(&moodle_root);
    let mut alias_total = 0usize;
    for path in &stepslibs {
        println!("cargo:rerun-if-changed={}", path.display());
        alias_total += merge_aliases_from_stepslib(&mut schema, path);
    }

    println!(
        "cargo:warning=Baked schema: {} install.xml files, {} unique tables, {} conflicts, {} stepslibs scanned, {} aliases",
        install_xmls.len(),
        total_tables,
        conflicts,
        stepslibs.len(),
        alias_total
    );

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = Path::new(&out_dir).join("schema.bin");
    let bytes = bincode::serialize(&schema).expect("bincode serialize Schema");
    std::fs::write(&out_path, &bytes).expect("write schema.bin");
}

fn find_install_xmls(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "node_modules" | "vendor" | "target" | ".git" | "moodledata" | "tests"
            )
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.file_name() == "install.xml"
                && e.path()
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n == "db")
                    .unwrap_or(false)
        })
        .map(|e| e.into_path())
        .collect()
}

fn find_stepslib_php(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "node_modules" | "vendor" | "target" | ".git" | "moodledata"
            )
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let name = e.file_name().to_string_lossy();
            // Match the canonical stepslib filenames:
            //   backup/moodle2/backup_stepslib.php (core)
            //   mod/*/backup/moodle2/backup_*_stepslib.php
            //   blocks/*/backup/moodle2/backup_*_stepslib.php
            //   etc.
            name == "backup_stepslib.php"
                || (name.starts_with("backup_") && name.ends_with("_stepslib.php"))
        })
        .filter(|e| {
            // Keep only paths under .../backup/moodle2/
            let p = e.path();
            let mut ancestors = p.ancestors().skip(1);
            ancestors.any(|a| {
                a.file_name() == Some(std::ffi::OsStr::new("moodle2"))
                    && a.parent()
                        .and_then(|x| x.file_name())
                        .map(|x| x == "backup")
                        .unwrap_or(false)
            })
        })
        .map(|e| e.into_path())
        .collect()
}

/// Scan a stepslib PHP file and merge any `set_source_alias` calls into the
/// matching tables of the schema. Returns the number of aliases merged.
///
/// Patterns recognised:
///   `$var = new backup_nested_element('table', ...)`
///   `$var->set_source_alias('dbcol', 'xmlelem')`
fn merge_aliases_from_stepslib(schema: &mut Schema, php: &Path) -> usize {
    let src = match std::fs::read_to_string(php) {
        Ok(s) => s,
        Err(_) => return 0,
    };

    // Var-name -> table-name (lowercase).
    let mut var_to_table: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let decl_re = Regex::new(
        r#"\$(\w+)\s*=\s*new\s+backup_nested_element\s*\(\s*['"]([^'"]+)['"]"#,
    )
    .expect("declaration regex");
    for cap in decl_re.captures_iter(&src) {
        let var = cap.get(1).unwrap().as_str().to_string();
        let table = cap.get(2).unwrap().as_str().to_ascii_lowercase();
        var_to_table.insert(var, table);
    }

    let alias_re = Regex::new(
        r#"\$(\w+)\s*->\s*set_source_alias\s*\(\s*['"]([^'"]+)['"]\s*,\s*['"]([^'"]+)['"]\s*\)"#,
    )
    .expect("alias regex");
    let mut count = 0usize;
    for cap in alias_re.captures_iter(&src) {
        let var = cap.get(1).unwrap().as_str();
        let dbcol = cap.get(2).unwrap().as_str().to_ascii_lowercase();
        let xmlelem = cap.get(3).unwrap().as_str().to_ascii_lowercase();
        if let Some(table_name) = var_to_table.get(var) {
            if let Some(table) = schema.tables.get_mut(table_name) {
                table.aliases.insert(xmlelem, dbcol);
                count += 1;
            }
        }
    }
    count
}

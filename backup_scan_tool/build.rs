//! Build-time schema baker.
//!
//! Walks $MOODLE_SCHEMA_ROOT (default: ../) for every db/install.xml, parses
//! them into a Schema, and bincode-serializes the result to $OUT_DIR/schema.bin
//! so the runtime crate can include_bytes!() it.

#[path = "src/xmldb_shared.rs"]
#[allow(dead_code)]
mod xmldb_shared;

use std::env;
use std::path::{Path, PathBuf};

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

    println!(
        "cargo:warning=Baked schema: {} install.xml files, {} unique tables, {} conflicts",
        install_xmls.len(),
        total_tables,
        conflicts
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
            // Skip noisy directories that can't contain Moodle plugin schemas.
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

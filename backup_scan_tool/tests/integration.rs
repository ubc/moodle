//! Integration test: build a tiny synthetic MBZ archive, run the scanner,
//! and assert that the expected issues show up.
//!
//! Uses real Moodle table names (course, files) that are guaranteed to be in
//! the bundled schema regardless of which plugins are present.

use std::fs::File;
use std::io::Write;

use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use backup_scan_tool::{scanner, schema};

fn make_mbz(dir: &TempDir, name: &str, files: &[(&str, &[u8])]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let f = File::create(&path).unwrap();
    let mut zip = ZipWriter::new(f);
    let opts: SimpleFileOptions =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in files {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
    path
}

#[test]
fn well_formed_minimal_mbz_passes() {
    let dir = TempDir::new().unwrap();
    let path = make_mbz(
        &dir,
        "good.mbz",
        &[(
            "moodle_backup.xml",
            br#"<?xml version="1.0"?><moodle_backup><information><name>x</name></information></moodle_backup>"# as &[u8],
        )],
    );
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    assert!(result.fatal.is_none());
    assert_eq!(result.errors(), 0, "{:?}", result.issues);
}

#[test]
fn malformed_xml_is_reported() {
    let dir = TempDir::new().unwrap();
    let path = make_mbz(
        &dir,
        "bad.mbz",
        &[(
            "moodle_backup.xml",
            br#"<?xml version="1.0"?><moodle_backup><name>x</moodle_backup>"# as &[u8],
        )],
    );
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    assert!(result.errors() >= 1);
    assert!(result
        .issues
        .iter()
        .any(|i| i.message.contains("XML parse error")));
}

#[test]
fn files_xml_missing_contenthash_reports() {
    let dir = TempDir::new().unwrap();
    // files.xml references a hash but the zip has no files/XX/HASH entry.
    let files_xml = r#"<?xml version="1.0"?><files><file id="1">
        <contenthash>abcdef0123456789abcdef0123456789abcdef01</contenthash>
        <contextid>1</contextid>
        <component>core</component>
        <filearea>area</filearea>
        <itemid>0</itemid>
        <filepath>/</filepath>
        <filename>x.txt</filename>
        <userid>1</userid>
        <filesize>1</filesize>
        <mimetype>text/plain</mimetype>
        <status>0</status>
        <source>$@NULL@$</source>
        <author>$@NULL@$</author>
        <license>$@NULL@$</license>
        <timecreated>0</timecreated>
        <timemodified>0</timemodified>
        <sortorder>0</sortorder>
        <referencefileid>$@NULL@$</referencefileid>
    </file></files>"#;
    let path = make_mbz(&dir, "files.mbz", &[("files.xml", files_xml.as_bytes())]);
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    assert!(result
        .issues
        .iter()
        .any(|i| i.message.contains("contenthash") && i.xml_file == "files.xml"));
}

#[test]
fn manifest_directory_missing_in_zip_reports() {
    let dir = TempDir::new().unwrap();
    let backup_xml = r#"<?xml version="1.0"?><moodle_backup><information>
        <contents><activities>
            <activity><moduleid>1</moduleid><modulename>assign</modulename>
                <title>x</title><directory>activities/assign_99</directory></activity>
        </activities></contents>
    </information></moodle_backup>"#;
    let path = make_mbz(&dir, "manifest.mbz", &[("moodle_backup.xml", backup_xml.as_bytes())]);
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    assert!(result
        .issues
        .iter()
        .any(|i| i.message.contains("activities/assign_99")));
}

#[test]
fn course_field_length_violation_is_detected() {
    // The schema we ship includes the "course" table with fullname char(254).
    // We can directly use it to assert that a too-long fullname is flagged.
    let s = schema::schema();
    let Some(course) = s.table("course") else {
        eprintln!("skipping — schema has no 'course' table (no Moodle tree available)");
        return;
    };
    let Some(fullname) = course.fields.get("fullname") else {
        eprintln!("skipping — no fullname field in course schema");
        return;
    };
    let max = fullname.length.expect("char field has length") as usize;
    let too_long = "X".repeat(max + 5);
    let xml = format!(
        r#"<?xml version="1.0"?><course id="1"><fullname>{}</fullname></course>"#,
        too_long
    );

    let dir = TempDir::new().unwrap();
    let path = make_mbz(&dir, "course.mbz", &[("course/course.xml", xml.as_bytes())]);
    let result = scanner::scan_mbz(&path, &s, false);
    assert!(
        result.issues.iter().any(|i| {
            i.message.contains("fullname") && i.message.contains("exceeds LENGTH")
        }),
        "expected length error, got: {:?}",
        result.issues
    );
}

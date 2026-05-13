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
fn well_formed_minimal_mbz_has_no_parse_errors() {
    // Synthetic minimal archive: just moodle_backup.xml. Shape/completeness
    // will flag missing root files (expected — that check is exercised
    // separately). Here we assert ONLY that parsing succeeded — no
    // MBZ-XML-PARSE issues.
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
    assert!(
        !result.issues.iter().any(|i| i.code == "MBZ-XML-PARSE"),
        "unexpected parse errors: {:?}",
        result.issues
    );
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
fn inforef_dangling_user_reference_is_detected() {
    // users.xml has user id=1; activities/assign_5/inforef.xml references user id=99.
    // Expect MBZ-XREF-MISSING-USER on the inforef path.
    let users_xml = r#"<?xml version="1.0"?><users>
        <user id="1"><username>a</username></user>
    </users>"#;
    let inforef_xml = r#"<?xml version="1.0"?><inforef>
        <userref>
            <user><id>1</id></user>
            <user><id>99</id></user>
        </userref>
    </inforef>"#;

    let dir = TempDir::new().unwrap();
    let path = make_mbz(
        &dir,
        "xref.mbz",
        &[
            ("users.xml", users_xml.as_bytes()),
            ("activities/assign_5/inforef.xml", inforef_xml.as_bytes()),
        ],
    );
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    let xref = result
        .issues
        .iter()
        .find(|i| i.code == "MBZ-XREF-MISSING-USER")
        .expect("expected XREF-MISSING-USER");
    assert_eq!(xref.xml_file, "activities/assign_5/inforef.xml");
    assert!(xref.message.contains("99"), "msg: {}", xref.message);
}

#[test]
fn inforef_silent_when_source_xml_absent() {
    // No users.xml in archive. Inforef references a user id but we should
    // NOT report it — Phase 3 will flag the missing top-level file.
    let inforef_xml = r#"<?xml version="1.0"?><inforef>
        <userref><user><id>1</id></user></userref>
    </inforef>"#;
    let dir = TempDir::new().unwrap();
    let path = make_mbz(
        &dir,
        "xref_silent.mbz",
        &[("activities/assign_5/inforef.xml", inforef_xml.as_bytes())],
    );
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    assert!(
        !result
            .issues
            .iter()
            .any(|i| i.code.starts_with("MBZ-XREF-MISSING")),
        "expected no xref issues, got: {:?}",
        result.issues
    );
}

#[test]
fn shape_missing_activity_file_is_reported() {
    // Manifest declares activities/assign_5 but the archive lacks module.xml.
    let backup_xml = r#"<?xml version="1.0"?><moodle_backup><information>
        <contents>
          <activities>
            <activity>
              <moduleid>1</moduleid>
              <modulename>assign</modulename>
              <title>x</title>
              <directory>activities/assign_5</directory>
            </activity>
          </activities>
        </contents>
    </information></moodle_backup>"#;

    let dir = TempDir::new().unwrap();
    let path = make_mbz(
        &dir,
        "shape.mbz",
        &[
            ("moodle_backup.xml", backup_xml.as_bytes()),
            // Provide inforef.xml only — module/roles/grades/grading/competencies missing.
            (
                "activities/assign_5/inforef.xml",
                br#"<?xml version="1.0"?><inforef></inforef>"# as &[u8],
            ),
        ],
    );
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    let missing: Vec<_> = result
        .issues
        .iter()
        .filter(|i| i.code == "MBZ-SHAPE-MISSING-ACTIVITY-FILE")
        .collect();
    assert!(
        missing
            .iter()
            .any(|i| i.message.contains("module.xml") && i.message.contains("assign_5")),
        "expected module.xml missing for assign_5, got: {:?}",
        missing
    );
}

#[test]
fn shape_missing_users_xml_when_setting_enabled() {
    // Root setting users=1 but no users.xml in the archive.
    let backup_xml = r#"<?xml version="1.0"?><moodle_backup><information>
        <settings>
          <setting>
            <level>root</level>
            <name>users</name>
            <value>1</value>
          </setting>
        </settings>
    </information></moodle_backup>"#;

    let dir = TempDir::new().unwrap();
    let path = make_mbz(
        &dir,
        "users.mbz",
        &[("moodle_backup.xml", backup_xml.as_bytes())],
    );
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    assert!(
        result
            .issues
            .iter()
            .any(|i| i.code == "MBZ-SHAPE-MISSING-ROOT-FILE"
                && i.message.contains("users.xml")
                && i.message.contains("users=1")),
        "expected users.xml gated-missing issue, got: {:?}",
        result.issues
    );
}

#[test]
fn orphan_file_payload_in_zip_is_warned() {
    // files.xml references no payloads, but the zip has a files/aa/aa…aa entry.
    let files_xml = r#"<?xml version="1.0"?><files></files>"#;
    let orphan_name = "files/aa/aabbccddeeff00112233445566778899aabbccdd";
    let dir = TempDir::new().unwrap();
    let path = make_mbz(
        &dir,
        "orphan.mbz",
        &[
            ("files.xml", files_xml.as_bytes()),
            (orphan_name, b"binary payload bytes"),
        ],
    );
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    assert!(
        result
            .issues
            .iter()
            .any(|i| i.code == "MBZ-ORPHAN-FILE-PAYLOAD"),
        "expected orphan payload warning, got: {:?}",
        result.issues
    );
}

#[test]
fn unsafe_archive_entry_name_is_flagged() {
    let dir = TempDir::new().unwrap();
    let path = make_mbz(
        &dir,
        "evil.mbz",
        &[
            (
                "moodle_backup.xml",
                br#"<?xml version="1.0"?><moodle_backup></moodle_backup>"# as &[u8],
            ),
            ("../../../etc/passwd", b"hostile"),
        ],
    );
    let s = schema::schema();
    let result = scanner::scan_mbz(&path, &s, false);
    assert!(
        result
            .issues
            .iter()
            .any(|i| i.code == "MBZ-SAFETY-PATH-TRAVERSAL"),
        "expected path traversal flag, got: {:?}",
        result.issues
    );
}

#[test]
fn many_char_overflows_cap_in_text_output() {
    use backup_scan_tool::report::{MbzResult, Report};
    use backup_scan_tool::validate::{codes, Issue};
    use std::path::PathBuf;

    // Synthesize a report with 200 same-code issues, render at default cap (5).
    let issues: Vec<Issue> = (0..200)
        .map(|i| {
            Issue::error(
                codes::STRUCT_CHAR_OVERFLOW,
                "course/course.xml",
                format!("/course/row[{}]/fullname", i),
                format!("char field <fullname> length 300 exceeds LENGTH=\"254\" (row {})", i),
            )
        })
        .collect();
    let r = Report {
        results: vec![MbzResult {
            path: PathBuf::from("/tmp/big.mbz"),
            xml_count: 1,
            fatal: None,
            issues,
        }],
    };
    let mut buf: Vec<u8> = Vec::new();
    r.print_text(&mut buf, false, false, 5).unwrap();
    let s = String::from_utf8(buf).unwrap();
    // 5 verbatim ERROR lines plus one summary line.
    let verbatim = s.matches("[ERROR] [MBZ-STRUCT-CHAR-OVERFLOW] course/course.xml:/course/row[").count();
    assert_eq!(verbatim, 5, "expected 5 verbatim lines, got {} in:\n{}", verbatim, s);
    assert!(
        s.contains("and 195 more issue(s) with this code"),
        "expected summary line, got:\n{}",
        s
    );
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

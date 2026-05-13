//! Walk a backup XML file, identify table-row elements, and validate the
//! values of their leaf-text child elements against the schema.
//!
//! Design notes:
//!
//! - Backup XML is intentionally a SUBSET of the underlying DB row. Many
//!   columns are recomputed at restore time and never appear in the XML.
//!   So we never check "is every NOT NULL field present?". Only fields that
//!   ARE present are length/type-validated.
//!
//! - The same element name may be a table-row, a wrapper, or a nested
//!   reference. To minimise false positives we require both signals before
//!   treating an element as a row:
//!     1. The element name matches a table in the bundled schema, AND
//!     2. The element carries an `id` attribute (Moodle's row convention via
//!        `backup_nested_element('tablename', array('id'), ...)`).
//!
//! - For child elements directly under a row, we defer judgement until the
//!   end tag: if the child contained any nested elements, it is a wrapper or
//!   nested reference and we leave it alone. If it contained only text, we
//!   look up its name in the row's field map and apply the field's
//!   constraint. If the name isn't a field, we emit a warning (unknown
//!   leaf), promoted to error under `--strict`.

use std::collections::HashSet;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{Issue, Severity};
use crate::schema::{Field, FieldType, Schema, Table};
use crate::transformer;

/// Element names that are deliberately emitted under a row even though they
/// don't correspond to a column in that table's install.xml. Verified by
/// reading the Moodle backup/restore code — these are either back-compat
/// emissions, computed values, or cross-table denormalisations.
///
/// Keyed by (table_name, lower-cased element_name). Compared case-insensitive.
const CROSS_TABLE_EMISSIONS: &[(&str, &str)] = &[
    // backup_stepslib.php:547 — added explicitly for back-compat; the value
    // lives in course_format_options, not course.
    ("course", "numsections"),
    // Same cross-table pattern, course_format_options.
    ("course", "hiddensections"),
    ("course", "coursedisplay"),
    // backup_stepslib.php:482 references this field but it doesn't exist in
    // lib/db/install.xml's course table (latent Moodle inconsistency).
    ("course", "completionstartonenrol"),
    // backup_stepslib.php:762 — JOIN'd from role_names.name AS nameincourse.
    ("role", "nameincourse"),
];

fn is_known_cross_table(table: &str, leaf: &str) -> bool {
    let table = table.to_ascii_lowercase();
    let leaf = leaf.to_ascii_lowercase();
    CROSS_TABLE_EMISSIONS
        .iter()
        .any(|(t, l)| *t == table && *l == leaf)
}

pub struct StructureOptions {
    /// Promote unknown-leaf warnings to errors.
    pub strict: bool,
}

impl Default for StructureOptions {
    fn default() -> Self {
        Self { strict: false }
    }
}

pub fn check(
    xml_name: &str,
    bytes: &[u8],
    schema: &Schema,
    options: &StructureOptions,
) -> Vec<Issue> {
    let mut walker = Walker {
        schema,
        xml_file: xml_name.to_string(),
        strict: options.strict,
        issues: Vec::new(),
        path: Vec::new(),
        ctx_stack: Vec::new(),
        deferred: Vec::new(),
    };
    walker.run(bytes);
    walker.issues
}

struct Walker<'s> {
    schema: &'s Schema,
    xml_file: String,
    strict: bool,
    issues: Vec<Issue>,
    /// Element-name stack — each Start pushes, each End pops.
    path: Vec<String>,
    /// Stack of currently open table-row contexts.
    ctx_stack: Vec<TableContext<'s>>,
    /// Stack of direct-child elements of a row whose final classification
    /// (field vs wrapper) is pending. Closed in LIFO order with their element.
    deferred: Vec<DeferredChild<'s>>,
}

struct TableContext<'s> {
    table: &'s Table,
    /// path.len() at the moment we pushed (the row element is at that depth).
    open_depth: usize,
    /// Field names we've already validated, to deduplicate per-row.
    seen_fields: HashSet<String>,
}

struct DeferredChild<'s> {
    name: String,
    open_depth: usize,
    location: String,
    field: Option<&'s Field>,
    has_child_element: bool,
    text_buf: String,
}

impl<'s> Walker<'s> {
    fn run(&mut self, bytes: &[u8]) {
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let has_id = has_id_attribute(&e);
                    self.on_start(&name, has_id);
                }
                Ok(Event::Empty(e)) => {
                    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let has_id = has_id_attribute(&e);
                    self.on_start(&name, has_id);
                    self.on_end();
                }
                Ok(Event::Text(t)) => {
                    let text = t.unescape().unwrap_or_default();
                    self.on_text(&text);
                }
                Ok(Event::CData(c)) => {
                    let text = String::from_utf8_lossy(c.as_ref()).into_owned();
                    self.on_text(&text);
                }
                Ok(Event::End(_)) => {
                    self.on_end();
                }
                Ok(Event::Eof) => break,
                Err(_) => break, // well_formed pass reports parse errors
                _ => {}
            }
            buf.clear();
        }
    }

    fn current_location(&self) -> String {
        let mut s = String::from("/");
        s.push_str(&self.path.join("/"));
        s
    }

    fn on_start(&mut self, name: &str, has_id: bool) {
        self.path.push(name.to_string());
        let depth = self.path.len();
        let lower = name.to_ascii_lowercase();

        // If our parent is a pending deferred-child, note that it has a child
        // element — which makes it a wrapper, not a leaf field.
        if let Some(d) = self.deferred.last_mut() {
            if d.open_depth == depth - 1 {
                d.has_child_element = true;
            }
        }

        let parent_is_row = self
            .ctx_stack
            .last()
            .map(|c| c.open_depth == depth - 1)
            .unwrap_or(false);

        // Direct child of a row: defer judgement.
        if parent_is_row {
            let ctx = self.ctx_stack.last().expect("checked above");
            // 1. Try direct match. 2. Fall back to a set_source_alias lookup
            //    that points us at a real install.xml field.
            let field = ctx.table.fields.get(&lower).or_else(|| {
                ctx.table
                    .aliases
                    .get(&lower)
                    .and_then(|dbcol| ctx.table.fields.get(dbcol))
            });
            let location = self.current_location();
            self.deferred.push(DeferredChild {
                name: name.to_string(),
                open_depth: depth,
                location,
                field,
                has_child_element: false,
                text_buf: String::new(),
            });
            return;
        }

        // Not under a row. Is this element itself a row? Only if its name
        // matches a table AND it carries an `id` attribute.
        if has_id {
            if let Some(table) = self.schema.table(&lower) {
                self.ctx_stack.push(TableContext {
                    table,
                    open_depth: depth,
                    seen_fields: HashSet::new(),
                });
            }
        }
    }

    fn on_text(&mut self, text: &str) {
        if let Some(d) = self.deferred.last_mut() {
            if d.open_depth == self.path.len() {
                d.text_buf.push_str(text);
            }
        }
    }

    fn on_end(&mut self) {
        let depth = self.path.len();

        // Close deferred child if its depth matches.
        if let Some(d) = self.deferred.last() {
            if d.open_depth == depth {
                let d = self.deferred.pop().expect("just checked");
                self.finalize_deferred(d);
            }
        }

        // Close table-row context if its depth matches.
        if let Some(ctx) = self.ctx_stack.last() {
            if ctx.open_depth == depth {
                self.ctx_stack.pop();
            }
        }

        self.path.pop();
    }

    fn finalize_deferred(&mut self, d: DeferredChild<'s>) {
        // Wrapper: had child elements. Don't validate, don't warn.
        if d.has_child_element {
            return;
        }
        match d.field {
            Some(field) => {
                // Track that this field was seen for the current row.
                if let Some(ctx) = self.ctx_stack.last_mut() {
                    ctx.seen_fields.insert(field.name.to_ascii_lowercase());
                }
                self.validate_field_value(field, &d.text_buf, &d.location);
            }
            None => {
                // Unknown leaf under a row.
                let parent_table = self
                    .ctx_stack
                    .last()
                    .map(|c| c.table.name.as_str())
                    .unwrap_or("?");
                // Skip warning if the leaf is empty (e.g. `<foo/>` or `<foo></foo>`).
                if d.text_buf.trim().is_empty() {
                    return;
                }
                // Skip warning for documented cross-table emissions (course
                // format options serialised under <course>, role_names.name
                // joined under <role>, etc).
                if is_known_cross_table(parent_table, &d.name) {
                    return;
                }
                let msg = format!(
                    "unknown leaf <{}> under <{}> (no matching field in db/install.xml)",
                    d.name, parent_table
                );
                self.emit_unknown(&d.location, msg);
            }
        }
    }

    fn validate_field_value(&mut self, field: &Field, raw: &str, location: &str) {
        match transformer::decode_null(raw.trim()) {
            None => {
                if field.notnull {
                    self.issues.push(Issue::error(
                        self.xml_file.clone(),
                        location.to_string(),
                        format!(
                            "NOT NULL field <{}> contains $@NULL@$ sentinel",
                            field.name
                        ),
                    ));
                }
            }
            Some(value) => match field.ty {
                FieldType::Char => self.check_char(field, raw, location),
                FieldType::Int => self.check_int(field, value, location),
                FieldType::Number => self.check_number(field, value, location),
                FieldType::Float => self.check_float(field, value, location),
                FieldType::Datetime | FieldType::Timestamp | FieldType::Text | FieldType::Binary => {}
            },
        }
    }

    fn check_char(&mut self, field: &Field, raw: &str, location: &str) {
        // For chars, do NOT trim — leading/trailing whitespace is stored as-is.
        if let Some(max) = field.length {
            let actual = transformer::char_len(raw);
            if actual as u32 > max {
                self.issues.push(Issue::error(
                    self.xml_file.clone(),
                    location.to_string(),
                    format!(
                        "char field <{}> length {} exceeds LENGTH=\"{}\"",
                        field.name, actual, max
                    ),
                ));
            }
        }
    }

    fn check_int(&mut self, field: &Field, value: &str, location: &str) {
        if value.is_empty() {
            return;
        }
        if value.parse::<i64>().is_err() {
            self.issues.push(Issue::error(
                self.xml_file.clone(),
                location.to_string(),
                format!(
                    "int field <{}> has non-integer value {:?}",
                    field.name, value
                ),
            ));
            return;
        }
        if let Some(max_digits) = field.length {
            let digits = value.trim_start_matches('-').chars().count() as u32;
            if digits > max_digits {
                self.issues.push(Issue::error(
                    self.xml_file.clone(),
                    location.to_string(),
                    format!(
                        "int field <{}> has {} digits but LENGTH=\"{}\"",
                        field.name, digits, max_digits
                    ),
                ));
            }
        }
    }

    fn check_number(&mut self, field: &Field, value: &str, location: &str) {
        if value.is_empty() {
            return;
        }
        if value.parse::<f64>().is_err() {
            self.issues.push(Issue::error(
                self.xml_file.clone(),
                location.to_string(),
                format!(
                    "number field <{}> has non-numeric value {:?}",
                    field.name, value
                ),
            ));
            return;
        }
        let body = value.trim_start_matches('-');
        let (int_part, frac_part) = match body.split_once('.') {
            Some((i, f)) => (i, f),
            None => (body, ""),
        };
        let int_digits = int_part.chars().count() as u32;
        let frac_digits = frac_part.chars().count() as u32;
        let total_digits = int_digits + frac_digits;
        if let Some(p) = field.length {
            if total_digits > p {
                self.issues.push(Issue::error(
                    self.xml_file.clone(),
                    location.to_string(),
                    format!(
                        "number field <{}> precision {} exceeds LENGTH=\"{}\"",
                        field.name, total_digits, p
                    ),
                ));
            }
        }
        if let Some(d) = field.decimals {
            if frac_digits > d {
                self.issues.push(Issue::error(
                    self.xml_file.clone(),
                    location.to_string(),
                    format!(
                        "number field <{}> fractional digits {} exceed DECIMALS=\"{}\"",
                        field.name, frac_digits, d
                    ),
                ));
            }
        }
    }

    fn check_float(&mut self, field: &Field, value: &str, location: &str) {
        if value.is_empty() {
            return;
        }
        if value.parse::<f64>().is_err() {
            self.issues.push(Issue::error(
                self.xml_file.clone(),
                location.to_string(),
                format!(
                    "float field <{}> has non-numeric value {:?}",
                    field.name, value
                ),
            ));
        }
    }

    fn emit_unknown(&mut self, location: &str, msg: String) {
        let severity = if self.strict {
            Severity::Error
        } else {
            Severity::Warning
        };
        self.issues.push(Issue {
            severity,
            xml_file: self.xml_file.clone(),
            location: location.to_string(),
            message: msg,
        });
    }
}

fn has_id_attribute(e: &quick_xml::events::BytesStart) -> bool {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref().eq_ignore_ascii_case(b"id") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_schema() -> Schema {
        let mut fields = HashMap::new();
        fields.insert(
            "id".to_string(),
            Field {
                name: "id".to_string(),
                ty: FieldType::Int,
                length: Some(10),
                decimals: None,
                notnull: true,
                sequence: true,
                default: None,
            },
        );
        fields.insert(
            "name".to_string(),
            Field {
                name: "name".to_string(),
                ty: FieldType::Char,
                length: Some(10),
                decimals: None,
                notnull: true,
                sequence: false,
                default: None,
            },
        );
        fields.insert(
            "grade".to_string(),
            Field {
                name: "grade".to_string(),
                ty: FieldType::Number,
                length: Some(5),
                decimals: Some(2),
                notnull: false,
                sequence: false,
                default: None,
            },
        );
        fields.insert(
            "count".to_string(),
            Field {
                name: "count".to_string(),
                ty: FieldType::Int,
                length: Some(10),
                decimals: None,
                notnull: true,
                sequence: false,
                default: None,
            },
        );
        fields.insert(
            "intro".to_string(),
            Field {
                name: "intro".to_string(),
                ty: FieldType::Text,
                length: None,
                decimals: None,
                notnull: false,
                sequence: false,
                default: None,
            },
        );

        let mut tables = HashMap::new();
        tables.insert(
            "widget".to_string(),
            Table {
                name: "widget".to_string(),
                fields,
                plugin_path: "mod/widget".to_string(),
                aliases: HashMap::new(),
            },
        );
        Schema { tables }
    }

    fn run(xml: &str) -> Vec<Issue> {
        let s = sample_schema();
        check("test.xml", xml.as_bytes(), &s, &StructureOptions::default())
    }

    #[test]
    fn happy_path_no_issues() {
        let xml = r#"<activity><widget id="1">
            <name>hi</name>
            <count>3</count>
            <grade>12.34</grade>
            <intro>anything goes</intro>
        </widget></activity>"#;
        assert!(run(xml).is_empty());
    }

    #[test]
    fn char_too_long_reports_error() {
        let xml = r#"<widget id="1">
            <name>this-is-definitely-too-long</name>
        </widget>"#;
        let issues = run(xml);
        assert!(issues.iter().any(|i| {
            i.severity == Severity::Error && i.message.contains("char field <name>")
        }));
    }

    #[test]
    fn int_non_numeric_errors() {
        let xml = r#"<widget id="1"><count>abc</count></widget>"#;
        let issues = run(xml);
        assert!(issues.iter().any(|i| i.message.contains("non-integer value")));
    }

    #[test]
    fn null_sentinel_on_notnull_errors() {
        let xml = r#"<widget id="1"><name>$@NULL@$</name></widget>"#;
        let issues = run(xml);
        assert!(issues
            .iter()
            .any(|i| i.message.contains("$@NULL@$ sentinel")));
    }

    #[test]
    fn unknown_leaf_warns() {
        let xml = r#"<widget id="1"><whatsthis>oops</whatsthis></widget>"#;
        let issues = run(xml);
        assert!(issues
            .iter()
            .any(|i| i.severity == Severity::Warning && i.message.contains("<whatsthis>")));
    }

    #[test]
    fn wrapper_element_with_same_name_as_field_is_silent() {
        // <intro> is a text field, but here it's used as a wrapper (has children).
        // Should NOT be treated as a field — silent.
        let xml = r#"<widget id="1">
            <intro><note>nested</note></intro>
        </widget>"#;
        let issues = run(xml);
        assert!(issues.is_empty(), "unexpected: {:?}", issues);
    }

    #[test]
    fn element_without_id_is_not_a_row() {
        // <widget> (no id attribute) is a manifest entry, not a row. We must
        // NOT try to validate its children — even though widget is a schema table.
        let xml = r#"<widget>
            <name>x</name>
            <foo>123</foo>
        </widget>"#;
        let issues = run(xml);
        assert!(
            issues.is_empty(),
            "no-id element shouldn't be validated: {:?}",
            issues
        );
    }

    #[test]
    fn nested_table_reference_is_silent() {
        // Inside <widget id="1">, an element matching a column name but
        // containing children is a nested reference — silent.
        let xml = r#"<widget id="1">
            <name>x</name>
            <count><deeplything>1</deeplything></count>
        </widget>"#;
        let issues = run(xml);
        assert!(issues.is_empty(), "unexpected: {:?}", issues);
    }

    #[test]
    fn number_precision_check() {
        let xml = r#"<widget id="1"><grade>123.456</grade></widget>"#;
        let issues = run(xml);
        assert!(issues.iter().any(|i| i.message.contains("precision 6")));
        assert!(issues
            .iter()
            .any(|i| i.message.contains("fractional digits 3")));
    }

    #[test]
    fn alias_resolves_to_field_and_validates() {
        // Set up: widget table where `count_alias` aliases to `count`.
        let mut s = sample_schema();
        s.tables
            .get_mut("widget")
            .unwrap()
            .aliases
            .insert("count_alias".to_string(), "count".to_string());

        let xml = r#"<widget id="1"><count_alias>abc</count_alias></widget>"#;
        let issues = check("t.xml", xml.as_bytes(), &s, &StructureOptions::default());
        // We expect a type error on the int field, NOT an "unknown leaf" warning.
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("non-integer value")),
            "expected int-validation error via alias, got: {:?}",
            issues
        );
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("unknown leaf")),
            "should not emit unknown-leaf when alias resolves: {:?}",
            issues
        );
    }

    #[test]
    fn cross_table_allowlist_silences_known_emissions() {
        // <course> with <numsections> — must not warn even though numsections
        // is not in the (real) course install.xml.
        // We can't easily exercise this with sample_schema since the
        // allowlist is keyed on "course"/"role"/etc. Use the real Schema:
        let s = crate::schema::schema();
        let Some(_) = s.table("course") else {
            return; // skip if no course table in this build's schema
        };
        let xml = r#"<course id="1"><numsections>5</numsections></course>"#;
        let issues = check("test.xml", xml.as_bytes(), &s, &StructureOptions::default());
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("numsections")),
            "numsections should be silenced by allowlist: {:?}",
            issues
        );
    }

    #[test]
    fn missing_notnull_field_is_silent() {
        // Backup XML is intentionally a subset — missing NOT NULL fields
        // is normal, not an error.
        let xml = r#"<widget id="1"><intro>x</intro></widget>"#;
        assert!(run(xml).is_empty());
    }
}

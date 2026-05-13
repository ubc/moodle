//! Self-contained XMLDB parser and Schema types.
//!
//! Used by build.rs (to bake the schema snapshot) and by src/schema/ (to
//! deserialize it at runtime). Must not depend on anything beyond serde,
//! quick-xml, and std so build.rs can `#[path]`-include it.

use std::collections::HashMap;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FieldType {
    Int,
    Char,
    Text,
    Number,
    Float,
    Datetime,
    Timestamp,
    Binary,
}

impl FieldType {
    pub fn from_xmldb(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "int" => Some(FieldType::Int),
            "char" => Some(FieldType::Char),
            "text" => Some(FieldType::Text),
            "number" => Some(FieldType::Number),
            "float" => Some(FieldType::Float),
            "datetime" => Some(FieldType::Datetime),
            "timestamp" => Some(FieldType::Timestamp),
            "binary" => Some(FieldType::Binary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub ty: FieldType,
    pub length: Option<u32>,
    pub decimals: Option<u32>,
    pub notnull: bool,
    pub sequence: bool,
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    pub fields: HashMap<String, Field>,
    /// Source plugin path relative to the moodle root (informational only).
    pub plugin_path: String,
    /// XML-element-name -> DB-column-name aliases declared by backup steplibs
    /// via `set_source_alias(dbcol, xmlelem)`. Both keys and values are
    /// stored lowercase. Populated at build time by parsing stepslib PHP.
    #[serde(default)]
    pub aliases: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schema {
    pub tables: HashMap<String, Table>,
}

impl Schema {
    /// Get a table by name (case-insensitive).
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.get(&name.to_ascii_lowercase())
    }
}

#[derive(Debug)]
pub enum LoadOutcome {
    Inserted,
    Conflict { existing_plugin: String },
}

impl Schema {
    /// Parse a single install.xml file and merge its tables into this Schema.
    pub fn merge_install_xml(
        &mut self,
        install_xml: &Path,
        moodle_root: &Path,
    ) -> Result<Vec<LoadOutcome>, String> {
        let bytes = std::fs::read(install_xml)
            .map_err(|e| format!("read {}: {}", install_xml.display(), e))?;

        let plugin_path = install_xml
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.strip_prefix(moodle_root).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| install_xml.display().to_string());

        let tables = parse_install_xml(&bytes)
            .map_err(|e| format!("parse {}: {}", install_xml.display(), e))?;

        let mut outcomes = Vec::with_capacity(tables.len());
        for mut t in tables {
            let key = t.name.to_ascii_lowercase();
            t.plugin_path = plugin_path.clone();
            match self.tables.get(&key) {
                Some(existing) => {
                    outcomes.push(LoadOutcome::Conflict {
                        existing_plugin: existing.plugin_path.clone(),
                    });
                }
                None => {
                    self.tables.insert(key, t);
                    outcomes.push(LoadOutcome::Inserted);
                }
            }
        }
        Ok(outcomes)
    }
}

/// Parse a Moodle install.xml byte buffer and return its tables.
///
/// We only extract <TABLE NAME=...><FIELDS><FIELD .../></FIELDS></TABLE>. Keys,
/// indexes, statistics, etc. are ignored — they don't affect backup XML
/// validation.
pub fn parse_install_xml(bytes: &[u8]) -> Result<Vec<Table>, String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);

    let mut tables: Vec<Table> = Vec::new();
    let mut current_table: Option<Table> = None;
    let mut in_fields = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"TABLE" => {
                    let name = read_attr(&e, b"NAME").unwrap_or_default();
                    current_table = Some(Table {
                        name,
                        fields: HashMap::new(),
                        plugin_path: String::new(),
                        aliases: HashMap::new(),
                    });
                }
                b"FIELDS" => {
                    in_fields = true;
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"TABLE" => {
                    if let Some(t) = current_table.take() {
                        if !t.name.is_empty() {
                            tables.push(t);
                        }
                    }
                }
                b"FIELDS" => {
                    in_fields = false;
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if in_fields && e.name().as_ref() == b"FIELD" {
                    if let Some(field) = parse_field(&e) {
                        if let Some(t) = current_table.as_mut() {
                            t.fields.insert(field.name.to_ascii_lowercase(), field);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(format!(
                    "xml error at position {}: {}",
                    reader.buffer_position(),
                    e
                ));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(tables)
}

fn parse_field(e: &quick_xml::events::BytesStart) -> Option<Field> {
    let name = read_attr(e, b"NAME")?;
    let ty_str = read_attr(e, b"TYPE")?;
    let ty = FieldType::from_xmldb(&ty_str)?;
    let length = read_attr(e, b"LENGTH").and_then(|s| s.parse::<u32>().ok());
    let decimals = read_attr(e, b"DECIMALS").and_then(|s| s.parse::<u32>().ok());
    let notnull = read_attr(e, b"NOTNULL")
        .map(|s| matches!(s.to_ascii_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false);
    let sequence = read_attr(e, b"SEQUENCE")
        .map(|s| matches!(s.to_ascii_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false);
    let default = read_attr(e, b"DEFAULT");

    // Match Moodle xmldb_field::set_attributes(): LOBs (TEXT/BINARY) never
    // carry a length or decimals or default.
    let (length, decimals, default) = match ty {
        FieldType::Text | FieldType::Binary => (None, None, None),
        _ => (length, decimals, default),
    };

    Some(Field {
        name,
        ty,
        length,
        decimals,
        notnull,
        sequence,
        default,
    })
}

fn read_attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref().eq_ignore_ascii_case(key) {
            return Some(String::from_utf8_lossy(&attr.value).into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<XMLDB PATH="mod/sample/db">
  <TABLES>
    <TABLE NAME="sample" COMMENT="x">
      <FIELDS>
        <FIELD NAME="id" TYPE="int" LENGTH="10" NOTNULL="true" SEQUENCE="true"/>
        <FIELD NAME="name" TYPE="char" LENGTH="255" NOTNULL="true" SEQUENCE="false"/>
        <FIELD NAME="grade" TYPE="number" LENGTH="10" DECIMALS="5" NOTNULL="false" SEQUENCE="false"/>
        <FIELD NAME="intro" TYPE="text" NOTNULL="false" SEQUENCE="false" DEFAULT="ignored-on-text"/>
        <FIELD NAME="status" TYPE="int" LENGTH="2" NOTNULL="true" DEFAULT="0" SEQUENCE="false"/>
        <FIELD NAME="weight" TYPE="float" LENGTH="20" NOTNULL="false" SEQUENCE="false"/>
        <FIELD NAME="data" TYPE="binary" NOTNULL="false" SEQUENCE="false"/>
      </FIELDS>
    </TABLE>
  </TABLES>
</XMLDB>"#;

    #[test]
    fn parses_all_field_types() {
        let tables = parse_install_xml(SAMPLE.as_bytes()).unwrap();
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.name, "sample");
        assert_eq!(t.fields.len(), 7);

        let id = &t.fields["id"];
        assert_eq!(id.ty, FieldType::Int);
        assert_eq!(id.length, Some(10));
        assert!(id.notnull);
        assert!(id.sequence);

        let name = &t.fields["name"];
        assert_eq!(name.ty, FieldType::Char);
        assert_eq!(name.length, Some(255));

        let grade = &t.fields["grade"];
        assert_eq!(grade.ty, FieldType::Number);
        assert_eq!(grade.length, Some(10));
        assert_eq!(grade.decimals, Some(5));
        assert!(!grade.notnull);

        // TEXT must never carry length/default per xmldb_field set_attributes.
        let intro = &t.fields["intro"];
        assert_eq!(intro.ty, FieldType::Text);
        assert!(intro.length.is_none());
        assert!(intro.default.is_none());

        let status = &t.fields["status"];
        assert_eq!(status.default.as_deref(), Some("0"));

        let weight = &t.fields["weight"];
        assert_eq!(weight.ty, FieldType::Float);
        assert_eq!(weight.length, Some(20));

        let data = &t.fields["data"];
        assert_eq!(data.ty, FieldType::Binary);
        assert!(data.length.is_none());
    }
}

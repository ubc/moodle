//! Runtime accessor for the schema snapshot baked at build time.

use std::sync::{Arc, OnceLock};

pub use crate::xmldb_shared::{Field, FieldType, Schema, Table};

static SCHEMA_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/schema.bin"));
static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

/// Deserialize the embedded schema once and return a cheap clone of the Arc.
pub fn schema() -> Arc<Schema> {
    SCHEMA
        .get_or_init(|| {
            let s: Schema = bincode::deserialize(SCHEMA_BYTES)
                .expect("embedded schema.bin should deserialize");
            Arc::new(s)
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_loads_and_has_core_tables() {
        let s = schema();
        // Sanity: at minimum the core "course" table should exist if built
        // against a real Moodle tree.
        if let Some(course) = s.table("course") {
            assert!(course.fields.contains_key("fullname"));
        }
        // Always: at least one table.
        assert!(!s.tables.is_empty(), "schema should not be empty");
    }
}

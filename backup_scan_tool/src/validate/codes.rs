//! Stable issue codes. Consumers should key off these strings; the
//! human-readable messages are free to change.
//!
//! Naming: `MBZ-<area>-<what>`. Areas in use today:
//!   - `XML`        well-formedness / parse
//!   - `STRUCT`     per-row schema (length, type, NOT NULL, unknown leaf)
//!   - `MANIFEST`   cross-XML directory & contenthash refs in moodle_backup/files
//!   - `ARCHIVE`    archive-level (open failures, entry read errors)
//!   - `XREF`       cross-XML referential integrity via inforef.xml (Phase 2)
//!   - `SHAPE`      manifest-declared structure vs archive contents (Phase 3)
//!   - `SAFETY`     zip-slip / path traversal (Phase 4)
//!   - `ORPHAN`     payloads in the zip not referenced anywhere (Phase 3)

// --- XML well-formedness ---------------------------------------------------
pub const XML_PARSE: &str = "MBZ-XML-PARSE";

// --- Structure / schema validation -----------------------------------------
pub const STRUCT_CHAR_OVERFLOW: &str = "MBZ-STRUCT-CHAR-OVERFLOW";
pub const STRUCT_INT_INVALID: &str = "MBZ-STRUCT-INT-INVALID";
pub const STRUCT_INT_OVERFLOW: &str = "MBZ-STRUCT-INT-OVERFLOW";
pub const STRUCT_NUMBER_INVALID: &str = "MBZ-STRUCT-NUMBER-INVALID";
pub const STRUCT_NUMBER_PRECISION: &str = "MBZ-STRUCT-NUMBER-PRECISION";
pub const STRUCT_NUMBER_DECIMALS: &str = "MBZ-STRUCT-NUMBER-DECIMALS";
pub const STRUCT_FLOAT_INVALID: &str = "MBZ-STRUCT-FLOAT-INVALID";
pub const STRUCT_NOTNULL_VIOLATION: &str = "MBZ-STRUCT-NOTNULL";
pub const STRUCT_UNKNOWN_LEAF: &str = "MBZ-STRUCT-UNKNOWN-LEAF";

// --- Cross-XML manifest checks ---------------------------------------------
pub const MANIFEST_MISSING_DIR: &str = "MBZ-MANIFEST-MISSING-DIR";
pub const MANIFEST_MISSING_FILE_PAYLOAD: &str = "MBZ-MANIFEST-MISSING-FILE";

// --- Archive-level ---------------------------------------------------------
pub const ARCHIVE_ENTRY_READ: &str = "MBZ-ARCHIVE-ENTRY-READ";

// --- Structure / schema validation (unique-key constraints) ---------------
pub const STRUCT_UNIQUE_VIOLATION: &str = "MBZ-STRUCT-UNIQUE-VIOLATION";

// --- Archive safety --------------------------------------------------------
pub const SAFETY_PATH_TRAVERSAL: &str = "MBZ-SAFETY-PATH-TRAVERSAL";

// --- Manifest format -------------------------------------------------------
pub const MANIFEST_BAD_BACKUPID: &str = "MBZ-MANIFEST-BAD-BACKUPID";
pub const MANIFEST_BAD_CONTENTHASH: &str = "MBZ-MANIFEST-BAD-CONTENTHASH";

// --- MBZ shape completeness ------------------------------------------------
pub const SHAPE_MISSING_ACTIVITY_FILE: &str = "MBZ-SHAPE-MISSING-ACTIVITY-FILE";
pub const SHAPE_MISSING_SECTION_FILE: &str = "MBZ-SHAPE-MISSING-SECTION-FILE";
pub const SHAPE_MISSING_ROOT_FILE: &str = "MBZ-SHAPE-MISSING-ROOT-FILE";
pub const ORPHAN_FILE_PAYLOAD: &str = "MBZ-ORPHAN-FILE-PAYLOAD";

// --- Cross-XML referential integrity (inforef.xml) -------------------------
pub const XREF_MISSING_USER: &str = "MBZ-XREF-MISSING-USER";
pub const XREF_MISSING_FILE: &str = "MBZ-XREF-MISSING-FILE";
pub const XREF_MISSING_ROLE: &str = "MBZ-XREF-MISSING-ROLE";
pub const XREF_MISSING_GROUP: &str = "MBZ-XREF-MISSING-GROUP";
pub const XREF_MISSING_GROUPING: &str = "MBZ-XREF-MISSING-GROUPING";
pub const XREF_MISSING_SCALE: &str = "MBZ-XREF-MISSING-SCALE";
pub const XREF_MISSING_OUTCOME: &str = "MBZ-XREF-MISSING-OUTCOME";
pub const XREF_MISSING_GRADE_ITEM: &str = "MBZ-XREF-MISSING-GRADE-ITEM";
pub const XREF_MISSING_QUESTION_CATEGORY: &str = "MBZ-XREF-MISSING-QUESTION-CATEGORY";

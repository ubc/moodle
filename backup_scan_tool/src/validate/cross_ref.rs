//! Cross-XML referential integrity.
//!
//! Every activity directory and every section directory in a Moodle MBZ
//! contains an `inforef.xml` that catalogues *which* foreign rows the activity
//! depends on. The catalogue is by id only, e.g.
//!
//! ```text
//! <inforef>
//!   <userref>
//!     <user><id>5</id></user>
//!     <user><id>7</id></user>
//!   </userref>
//!   <fileref>
//!     <file><id>33</id></file>
//!   </fileref>
//! </inforef>
//! ```
//!
//! A restore that finds a `<user><id>5</id></user>` reference but no
//! `<user id="5">` in `users.xml` falls back to current-user or skips — masks
//! a real data-loss bug at backup time. We catch it here.
//!
//! Strategy:
//!   1. `ReferenceIndex::build` scans the known top-level XMLs (users.xml,
//!      files.xml, roles.xml, groups.xml, scales.xml, outcomes.xml,
//!      gradebook.xml, questions.xml) once, picking up every `<row id="N">`
//!      where the row element matches the expected wrapper.
//!   2. `check_inforef` walks one inforef.xml, looks every `<thing><id>N</id>`
//!      up in the index, and reports misses — but only when the relevant
//!      top-level file was *present* (otherwise Phase 3 "shape" will catch
//!      the missing file; we don't want duplicate noise).
//!
//! Output is capped: at most N (default 5) verbatim misses per (xref-type,
//! inforef-path), then "...and X more".

use std::collections::HashSet;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::codes;
use super::Issue;

const SAMPLE_CAP: usize = 5;

/// Per-MBZ catalogue of ids that exist in each top-level XML. The validator
/// queries this when walking inforef.xml.
#[derive(Default, Debug)]
pub struct ReferenceIndex {
    pub files: HashSet<u64>,
    pub users: HashSet<u64>,
    pub roles: HashSet<u64>,
    pub groups: HashSet<u64>,
    pub groupings: HashSet<u64>,
    pub scales: HashSet<u64>,
    pub outcomes: HashSet<u64>,
    pub grade_items: HashSet<u64>,
    pub question_categories: HashSet<u64>,
    /// Top-level XML names that contributed to the index. Used to suppress
    /// dangling-ref reports when the *whole* index source is absent (Phase 3
    /// reports that separately).
    pub present: HashSet<&'static str>,
}

impl ReferenceIndex {
    /// Feed a single top-level XML's bytes into the index. `entry_name` is the
    /// archive entry name (e.g. "users.xml"). Unknown names are no-ops.
    pub fn ingest(&mut self, entry_name: &str, bytes: &[u8]) {
        let target: Option<(&str, &str, &mut HashSet<u64>)> = match entry_name {
            "users.xml" => Some(("users", "user", &mut self.users)),
            "files.xml" => Some(("files", "file", &mut self.files)),
            "roles.xml" => Some(("roles", "role", &mut self.roles)),
            "groups.xml" => {
                // groups.xml carries BOTH <group id=N> rows and <grouping id=N> rows.
                collect_row_ids(bytes, "groups", "group", &mut self.groups);
                collect_row_ids(bytes, "groupings", "grouping", &mut self.groupings);
                self.present.insert("groups.xml");
                return;
            }
            "scales.xml" => Some(("scales_definition", "scale", &mut self.scales)),
            "outcomes.xml" => Some(("outcomes_definition", "outcome", &mut self.outcomes)),
            "gradebook.xml" => Some(("grade_items", "grade_item", &mut self.grade_items)),
            "questions.xml" => Some((
                "question_categories",
                "question_category",
                &mut self.question_categories,
            )),
            _ => None,
        };
        if let Some((parent, row, set)) = target {
            collect_row_ids(bytes, parent, row, set);
            // Map entry_name to a 'static &str for the present set. Match-and-
            // intern via the same list used above so we never lose ownership.
            let interned: &'static str = match entry_name {
                "users.xml" => "users.xml",
                "files.xml" => "files.xml",
                "roles.xml" => "roles.xml",
                "scales.xml" => "scales.xml",
                "outcomes.xml" => "outcomes.xml",
                "gradebook.xml" => "gradebook.xml",
                "questions.xml" => "questions.xml",
                _ => return,
            };
            self.present.insert(interned);
        }
    }

}

/// Index the `<grade_item id=N>` rows inside an activity's grades.xml. The
/// file's structure is `<activity_gradebook><grade_items><grade_item id=…>`,
/// so `collect_row_ids` picks them up via the existing parent-anywhere logic.
/// Activity grade items belong to the same logical set as root gradebook
/// grade items — store them in the same `grade_items` HashSet, and mark
/// gradebook.xml present (since this archive *does* carry grade item data,
/// just not in the root file).
pub fn ingest_activity_grades(idx: &mut ReferenceIndex, bytes: &[u8]) {
    collect_row_ids(bytes, "grade_items", "grade_item", &mut idx.grade_items);
    idx.present.insert("gradebook.xml");
}

impl ReferenceIndex {
    fn lookup(&self, kind: XrefKind, id: u64) -> bool {
        match kind {
            XrefKind::User => self.users.contains(&id),
            XrefKind::File => self.files.contains(&id),
            XrefKind::Role => self.roles.contains(&id),
            XrefKind::Group => self.groups.contains(&id),
            XrefKind::Grouping => self.groupings.contains(&id),
            XrefKind::Scale => self.scales.contains(&id),
            XrefKind::Outcome => self.outcomes.contains(&id),
            XrefKind::GradeItem => self.grade_items.contains(&id),
            XrefKind::QuestionCategory => self.question_categories.contains(&id),
        }
    }

    fn source_present(&self, kind: XrefKind) -> bool {
        let needed = match kind {
            XrefKind::User => "users.xml",
            XrefKind::File => "files.xml",
            XrefKind::Role => "roles.xml",
            XrefKind::Group => "groups.xml",
            XrefKind::Grouping => "groups.xml",
            XrefKind::Scale => "scales.xml",
            XrefKind::Outcome => "outcomes.xml",
            XrefKind::GradeItem => "gradebook.xml",
            XrefKind::QuestionCategory => "questions.xml",
        };
        self.present.contains(needed)
    }
}

/// The expected name of the inforef.xml in a relative path lives inside
/// activities/<modname>_<cmid>/ or sections/section_<id>/. We accept any
/// path ending in `inforef.xml` as a candidate.
pub fn is_inforef(entry_name: &str) -> bool {
    entry_name.ends_with("/inforef.xml") || entry_name == "inforef.xml"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum XrefKind {
    User,
    File,
    Role,
    Group,
    Grouping,
    Scale,
    Outcome,
    GradeItem,
    QuestionCategory,
}

impl XrefKind {
    /// (wrapper-element, child-element) pairs as emitted by
    /// `backup_inforef_structure_step`. Wrapper is e.g. "userref", child is
    /// e.g. "user".
    fn from_wrapper_child(wrapper: &str, child: &str) -> Option<Self> {
        match (wrapper, child) {
            ("userref", "user") => Some(XrefKind::User),
            ("fileref", "file") => Some(XrefKind::File),
            ("roleref", "role") => Some(XrefKind::Role),
            ("groupref", "group") => Some(XrefKind::Group),
            ("groupingref", "grouping") => Some(XrefKind::Grouping),
            ("scaleref", "scale") => Some(XrefKind::Scale),
            ("outcomeref", "outcome") => Some(XrefKind::Outcome),
            ("grade_itemref", "grade_item") => Some(XrefKind::GradeItem),
            ("question_categoryref", "question_category") => Some(XrefKind::QuestionCategory),
            _ => None,
        }
    }

    fn code(self) -> &'static str {
        match self {
            XrefKind::User => codes::XREF_MISSING_USER,
            XrefKind::File => codes::XREF_MISSING_FILE,
            XrefKind::Role => codes::XREF_MISSING_ROLE,
            XrefKind::Group => codes::XREF_MISSING_GROUP,
            XrefKind::Grouping => codes::XREF_MISSING_GROUPING,
            XrefKind::Scale => codes::XREF_MISSING_SCALE,
            XrefKind::Outcome => codes::XREF_MISSING_OUTCOME,
            XrefKind::GradeItem => codes::XREF_MISSING_GRADE_ITEM,
            XrefKind::QuestionCategory => codes::XREF_MISSING_QUESTION_CATEGORY,
        }
    }

    fn label(self) -> &'static str {
        match self {
            XrefKind::User => "user",
            XrefKind::File => "file",
            XrefKind::Role => "role",
            XrefKind::Group => "group",
            XrefKind::Grouping => "grouping",
            XrefKind::Scale => "scale",
            XrefKind::Outcome => "outcome",
            XrefKind::GradeItem => "grade_item",
            XrefKind::QuestionCategory => "question_category",
        }
    }

    fn source_xml(self) -> &'static str {
        match self {
            XrefKind::User => "users.xml",
            XrefKind::File => "files.xml",
            XrefKind::Role => "roles.xml",
            XrefKind::Group | XrefKind::Grouping => "groups.xml",
            XrefKind::Scale => "scales.xml",
            XrefKind::Outcome => "outcomes.xml",
            XrefKind::GradeItem => "gradebook.xml",
            XrefKind::QuestionCategory => "questions.xml",
        }
    }
}

/// Walk an inforef.xml and emit one issue per (kind, source-file) bucket
/// where any referenced ids are missing in the index.
pub fn check_inforef(entry_name: &str, bytes: &[u8], idx: &ReferenceIndex) -> Vec<Issue> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    // Element-name stack so we know the wrapper (e.g. "userref") and the
    // current child (e.g. "user") when we hit the inner <id>.
    let mut path: Vec<String> = Vec::new();
    let mut id_buf = String::new();
    let mut in_id = false;

    // (kind) -> Vec<missing-id>
    let mut misses: std::collections::BTreeMap<XrefKind, Vec<u64>> = Default::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "id" && path.len() >= 2 {
                    in_id = true;
                    id_buf.clear();
                }
                path.push(name);
            }
            Ok(Event::Text(t)) => {
                if in_id {
                    if let Ok(s) = t.unescape() {
                        id_buf.push_str(&s);
                    }
                }
            }
            Ok(Event::CData(c)) => {
                if in_id {
                    id_buf.push_str(&String::from_utf8_lossy(c.as_ref()));
                }
            }
            Ok(Event::End(_)) => {
                if in_id && path.last().map(String::as_str) == Some("id") {
                    in_id = false;
                    if let Some(kind) = id_context(&path) {
                        if idx.source_present(kind) {
                            if let Ok(id) = id_buf.trim().parse::<u64>() {
                                if !idx.lookup(kind, id) {
                                    misses.entry(kind).or_default().push(id);
                                }
                            }
                        }
                    }
                }
                path.pop();
            }
            Ok(Event::Empty(_)) => {
                // <id/> with no text — skip; nothing to check.
            }
            Ok(Event::Eof) => break,
            Err(_) => break, // well_formed handles parse errors
            _ => {}
        }
        buf.clear();
    }

    let mut issues = Vec::new();
    for (kind, ids) in misses {
        let n = ids.len();
        let sample: Vec<String> = ids
            .iter()
            .take(SAMPLE_CAP)
            .map(|i| i.to_string())
            .collect();
        let detail = if n > SAMPLE_CAP {
            format!(
                "inforef references {} {} id(s) absent from {}: {} (and {} more)",
                n,
                kind.label(),
                kind.source_xml(),
                sample.join(", "),
                n - SAMPLE_CAP
            )
        } else {
            format!(
                "inforef references {} {} id(s) absent from {}: {}",
                n,
                kind.label(),
                kind.source_xml(),
                sample.join(", "),
            )
        };
        issues.push(Issue::error(
            kind.code(),
            entry_name.to_string(),
            format!("/inforef/{}", wrapper_for(kind)),
            detail,
        ));
    }
    issues
}

fn wrapper_for(kind: XrefKind) -> &'static str {
    match kind {
        XrefKind::User => "userref",
        XrefKind::File => "fileref",
        XrefKind::Role => "roleref",
        XrefKind::Group => "groupref",
        XrefKind::Grouping => "groupingref",
        XrefKind::Scale => "scaleref",
        XrefKind::Outcome => "outcomeref",
        XrefKind::GradeItem => "grade_itemref",
        XrefKind::QuestionCategory => "question_categoryref",
    }
}

/// Given the element-name stack just before closing `<id>`, decide which xref
/// kind we're inside. Expected stack tail: [..., <wrapper>, <child>, "id"].
fn id_context(path: &[String]) -> Option<XrefKind> {
    let n = path.len();
    if n < 3 {
        return None;
    }
    let wrapper = path[n - 3].as_str();
    let child = path[n - 2].as_str();
    XrefKind::from_wrapper_child(wrapper, child)
}

/// Scan an XML for `<row id="N">` rows directly under `<parent>` and record
/// the ids into `out`. `parent` is matched anywhere in the ancestor chain —
/// not strictly required to be the immediate parent — because Moodle's
/// top-level files sometimes wrap rows in extra grouping elements (e.g.
/// `<grade_items>` lives under `<gradebook>`).
fn collect_row_ids(bytes: &[u8], parent: &str, row: &str, out: &mut HashSet<u64>) {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut path: Vec<String> = Vec::new();

    let want_row = row;
    let want_parent = parent;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == want_row && path.iter().any(|p| p == want_parent) {
                    if let Some(id) = read_id_attr(&e) {
                        out.insert(id);
                    }
                }
                path.push(name);
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == want_row && path.iter().any(|p| p == want_parent) {
                    if let Some(id) = read_id_attr(&e) {
                        out.insert(id);
                    }
                }
                // Self-closing: don't push.
            }
            Ok(Event::End(_)) => {
                path.pop();
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

fn read_id_attr(e: &quick_xml::events::BytesStart) -> Option<u64> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref().eq_ignore_ascii_case(b"id") {
            let s = String::from_utf8_lossy(&attr.value);
            return s.trim().parse::<u64>().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_users_indexes_ids() {
        let mut idx = ReferenceIndex::default();
        idx.ingest(
            "users.xml",
            br#"<users>
                <user id="1"><username>a</username></user>
                <user id="2"><username>b</username></user>
            </users>"#,
        );
        assert!(idx.users.contains(&1));
        assert!(idx.users.contains(&2));
        assert!(idx.present.contains("users.xml"));
    }

    #[test]
    fn ingest_groups_picks_up_both_group_and_grouping() {
        let mut idx = ReferenceIndex::default();
        idx.ingest(
            "groups.xml",
            br#"<groups_and_groupings>
                <groups>
                    <group id="10"><name>g1</name></group>
                </groups>
                <groupings>
                    <grouping id="20"><name>gg1</name></grouping>
                </groupings>
            </groups_and_groupings>"#,
        );
        assert!(idx.groups.contains(&10));
        assert!(idx.groupings.contains(&20));
    }

    #[test]
    fn missing_user_in_inforef_is_reported() {
        let mut idx = ReferenceIndex::default();
        idx.ingest(
            "users.xml",
            br#"<users><user id="1"><username>a</username></user></users>"#,
        );
        let inforef = br#"<inforef>
            <userref>
                <user><id>1</id></user>
                <user><id>99</id></user>
            </userref>
        </inforef>"#;
        let issues = check_inforef("activities/assign_5/inforef.xml", inforef, &idx);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, codes::XREF_MISSING_USER);
        assert!(issues[0].message.contains("99"));
        assert!(!issues[0].message.contains("1,") && !issues[0].message.ends_with(": 1"));
    }

    #[test]
    fn missing_source_xml_suppresses_inforef_reports() {
        // users.xml never ingested — Phase 3 will report it; we stay silent.
        let idx = ReferenceIndex::default();
        let inforef = br#"<inforef><userref><user><id>1</id></user></userref></inforef>"#;
        let issues = check_inforef("activities/assign_5/inforef.xml", inforef, &idx);
        assert!(issues.is_empty(), "got: {:?}", issues);
    }

    #[test]
    fn file_ref_missing_is_reported() {
        let mut idx = ReferenceIndex::default();
        idx.ingest(
            "files.xml",
            br#"<files><file id="1"><filename>x</filename></file></files>"#,
        );
        let inforef = br#"<inforef><fileref><file><id>7</id></file></fileref></inforef>"#;
        let issues = check_inforef("activities/assign_5/inforef.xml", inforef, &idx);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, codes::XREF_MISSING_FILE);
    }

    #[test]
    fn happy_path_inforef_yields_no_issues() {
        let mut idx = ReferenceIndex::default();
        idx.ingest(
            "users.xml",
            br#"<users><user id="5"><username>a</username></user></users>"#,
        );
        idx.ingest(
            "files.xml",
            br#"<files><file id="33"><filename>x</filename></file></files>"#,
        );
        let inforef = br#"<inforef>
            <userref><user><id>5</id></user></userref>
            <fileref><file><id>33</id></file></fileref>
        </inforef>"#;
        let issues = check_inforef("activities/assign_5/inforef.xml", inforef, &idx);
        assert!(issues.is_empty(), "got: {:?}", issues);
    }

    #[test]
    fn many_misses_cap_with_summary() {
        let mut idx = ReferenceIndex::default();
        idx.ingest("users.xml", br#"<users></users>"#);
        let mut body = String::from("<inforef><userref>");
        for i in 1..=10u64 {
            body.push_str(&format!("<user><id>{}</id></user>", i));
        }
        body.push_str("</userref></inforef>");
        let issues = check_inforef("activities/assign_5/inforef.xml", body.as_bytes(), &idx);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("and 5 more"));
    }

    #[test]
    fn nested_id_outside_inforef_wrapper_is_ignored() {
        // An <id> that's not under (wrapperref, child, id) should not be
        // treated as a reference. Here <foo><id>...</id></foo> with no
        // wrapping userref/fileref/etc must produce no issues.
        let idx = ReferenceIndex::default();
        let xml = br#"<inforef><foo><id>123</id></foo></inforef>"#;
        let issues = check_inforef("activities/assign_5/inforef.xml", xml, &idx);
        assert!(issues.is_empty());
    }
}

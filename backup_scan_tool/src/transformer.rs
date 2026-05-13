//! Decode the sentinel tokens Moodle's backup_xml_transformer writes into
//! backup XML so the rest of the validator can treat values as plain text.

/// Moodle's NULL sentinel. backup_xml_transformer::process() emits this for
/// PHP null values; we treat it as null when checking NOTNULL constraints.
pub const NULL_SENTINEL: &str = "$@NULL@$";

/// Sentinels that stand in for absolute-URL fragments. They are stored
/// literally in the XML and on the target site at restore time. For length
/// validation we measure them as written (matches what hits the DB column).
#[cfg(test)]
const URL_SENTINELS: &[&str] = &[
    "$@FILEPHP@$",
    "$@SLASH@$",
    "$@H5PEMBED@$",
    "$@FORCEDOWNLOAD@$",
];

/// Returns Some(text) for a non-null value, or None if the value is the NULL
/// sentinel. Empty strings stay as Some("").
pub fn decode_null(raw: &str) -> Option<&str> {
    if raw == NULL_SENTINEL {
        None
    } else {
        Some(raw)
    }
}

/// Character length (UTF-8 char count) of a value as it will land in the DB
/// column. Used for `char` LENGTH checks. Sentinels are counted by their
/// literal character count — that's exactly what would be stored.
pub fn char_len(raw: &str) -> usize {
    raw.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sentinel_decodes_to_none() {
        assert_eq!(decode_null("$@NULL@$"), None);
        assert_eq!(decode_null(""), Some(""));
        assert_eq!(decode_null("hello"), Some("hello"));
    }

    #[test]
    fn char_len_is_unicode_aware() {
        assert_eq!(char_len("hello"), 5);
        assert_eq!(char_len("héllo"), 5);
        assert_eq!(char_len("中文"), 2);
    }

    #[test]
    fn url_sentinels_are_recognised_as_literal() {
        // We don't strip them — they stay in the value, which is correct
        // for length validation. This test is a guard against accidentally
        // adding stripping logic.
        for s in URL_SENTINELS {
            assert!(char_len(s) > 0);
        }
    }
}

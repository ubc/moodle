//! Streaming well-formedness check.
//!
//! quick-xml events are pulled until EOF; the first parse error becomes an
//! issue and we stop (further events would be untrustworthy).

use quick_xml::events::Event;
use quick_xml::Reader;

use super::Issue;

pub fn check(xml_name: &str, bytes: &[u8]) -> Option<Issue> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => return None,
            Ok(_) => {}
            Err(e) => {
                return Some(Issue::error(
                    xml_name,
                    format!("byte {}", reader.buffer_position()),
                    format!("XML parse error: {}", e),
                ));
            }
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_returns_none() {
        let xml = br#"<?xml version="1.0"?><a><b>x</b></a>"#;
        assert!(check("t.xml", xml).is_none());
    }

    #[test]
    fn unclosed_tag_returns_error() {
        let xml = br#"<?xml version="1.0"?><a><b>x</a>"#;
        let issue = check("t.xml", xml).expect("expected error");
        assert!(issue.message.contains("XML parse error"));
    }
}

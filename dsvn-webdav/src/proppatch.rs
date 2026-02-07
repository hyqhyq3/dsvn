//! PROPPATCH request parser
//!
//! Parses WebDAV PROPPATCH XML requests to modify properties

use anyhow::Result;

/// Represents a PROPPATCH request
#[derive(Debug, Clone)]
pub struct PropPatchRequest {
    /// Resource to modify properties on
    pub href: String,

    /// Property modifications to apply
    pub modifications: Vec<PropertyModification>,
}

/// Property modification operation
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyModification {
    /// Set a property to a value
    Set {
        name: String,
        value: String,
        xmlns: Option<String>,
    },

    /// Remove a property
    Remove {
        name: String,
        xmlns: Option<String>,
    },
}

/// Parsed XML response structure
#[derive(Debug, Clone)]
pub struct PropPatchResponse {
    pub href: String,
    pub propstats: Vec<PropStat>,
}

#[derive(Debug, Clone)]
pub struct PropStat {
    pub status: String,
    pub responsedescription: Option<String>,
    pub props: Vec<Property>,
}

#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    pub value: Option<String>,
}

impl PropPatchRequest {
    /// Parse from XML body
    pub fn from_xml(xml: &str) -> Result<Self> {
        let href = "/".to_string(); // Simplified for MVP
        let mut modifications = Vec::new();

        // Parse <D:set> elements
        for set_match in find_xml_blocks(xml, "D:set") {
            if let Some((name, value)) = parse_property_element(&set_match) {
                modifications.push(PropertyModification::Set {
                    name,
                    value,
                    xmlns: None,
                });
            }
        }

        // Parse <D:remove> elements
        for remove_match in find_xml_blocks(xml, "D:remove") {
            if let Some((name, _)) = parse_property_element(&remove_match) {
                modifications.push(PropertyModification::Remove {
                    name,
                    xmlns: None,
                });
            }
        }

        Ok(Self {
            href,
            modifications,
        })
    }

    /// Check if this is a valid PROPPATCH request
    pub fn is_valid(&self) -> bool {
        !self.modifications.is_empty()
    }
}

impl PropPatchResponse {
    /// Create success response
    pub fn success(href: String) -> Self {
        Self {
            href,
            propstats: vec![PropStat {
                status: "HTTP/1.1 200 OK".to_string(),
                responsedescription: None,
                props: vec![],
            }],
        }
    }

    /// Create error response
    pub fn error(href: String, message: String) -> Self {
        Self {
            href,
            propstats: vec![PropStat {
                status: "HTTP/1.1 403 Forbidden".to_string(),
                responsedescription: Some(message),
                props: vec![],
            }],
        }
    }

    /// Convert to XML
    pub fn to_xml(&self) -> String {
        let mut xml = String::from(r#"<?xml version="1.0" encoding="utf-8"?>"#);
        xml.push_str(r#"<D:multistatus xmlns:D="DAV:">"#);

        for propstat in &self.propstats {
            xml.push_str(&format!(
                r#"<D:response><D:href>{}</D:href><D:propstat><D:status>{}</D:status>"#,
                self.href, propstat.status
            ));

            if let Some(desc) = &propstat.responsedescription {
                xml.push_str(&format!(
                    r#"<D:responsedescription>{}</D:responsedescription>"#,
                    escape_xml(desc)
                ));
            }

            xml.push_str(r#"</D:propstat></D:response>"#);
        }

        xml.push_str(r#"</D:multistatus>"#);
        xml
    }
}

/// Find all XML blocks with a given tag name
fn find_xml_blocks(xml: &str, tag: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut pos = 0;

    let start_tag = format!("<{}>", tag);
    let end_tag = format!("</{}>", tag);

    while let Some(start) = xml[pos..].find(&start_tag) {
        let start_pos = pos + start;
        if let Some(end) = xml[start_pos..].find(&end_tag) {
            let end_pos = start_pos + end + end_tag.len();
            blocks.push(xml[start_pos..end_pos].to_string());
            pos = end_pos;
        } else {
            break;
        }
    }

    blocks
}

/// Parse a property element (D:prop)
fn parse_property_element(xml: &str) -> Option<(String, String)> {
    // Format: <D:prop><svn:executable>*</svn:executable></D:prop>
    //        or: <D:prop><my:custom>value</my:custom></D:prop>

    let prop_start = xml.find("<D:prop>")? + 9;
    let prop_end = xml.find("</D:prop>")?;
    let content = &xml[prop_start..prop_end];

    // Find first opening tag (the property name)
    let tag_open = content.find('<')? + 1;
    let tag_close_relative = content[tag_open..].find('>')?;
    let tag_close = tag_open + tag_close_relative;
    let full_tag = &content[tag_open..tag_close];

    // Extract property name
    // Remove namespace prefix if present (e.g., "svn:" -> "")
    let prop_name = full_tag
        .split(':')
        .last()
        .unwrap_or(full_tag)
        .to_string();

    // Find closing tag for the value
    let closing_tag = format!("</{}>", full_tag);
    let value_start = tag_close + 1;

    // Value is between the > of opening tag and closing tag
    let value = if let Some(value_end_relative) = content[value_start..].find(&closing_tag) {
        let value_end = value_start + value_end_relative;
        content[value_start..value_end].trim().to_string()
    } else {
        String::new()
    };

    Some((prop_name, value))
}

/// Escape XML special characters
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_proppatch_set_request() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <svn:executable>*</svn:executable>
    </D:prop>
  </D:set>
</D:propertyupdate>"#;

        let request = PropPatchRequest::from_xml(xml).unwrap();

        assert_eq!(request.modifications.len(), 1);
        match &request.modifications[0] {
            PropertyModification::Set { name, value, .. } => {
                assert_eq!(name, "executable");
                assert_eq!(value, "*");
            }
            _ => panic!("Expected Set modification"),
        }
    }

    #[test]
    fn test_parse_proppatch_remove_request() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:remove>
    <D:prop>
      <svn:mime-type>application/octet-stream</svn:mime-type>
    </D:prop>
  </D:remove>
</D:propertyupdate>"#;

        let request = PropPatchRequest::from_xml(xml).unwrap();

        assert_eq!(request.modifications.len(), 1);
        match &request.modifications[0] {
            PropertyModification::Remove { name, .. } => {
                assert_eq!(name, "mime-type");
            }
            _ => panic!("Expected Remove modification"),
        }
    }

    #[test]
    fn test_parse_proppatch_multiple_properties() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <svn:executable>*</svn:executable>
    </D:prop>
  </D:set>
  <D:set>
    <D:prop>
      <svn:mime-type>text/plain</svn:mime-type>
    </D:prop>
  </D:set>
</D:propertyupdate>"#;

        let request = PropPatchRequest::from_xml(xml).unwrap();

        assert_eq!(request.modifications.len(), 2);
    }

    #[test]
    fn test_proppatch_response_success_xml() {
        let response = PropPatchResponse::success("/test.txt".to_string());
        let xml = response.to_xml();

        assert!(xml.contains("<D:multistatus"));
        assert!(xml.contains("<D:href>/test.txt</D:href>"));
        assert!(xml.contains("200 OK"));
        assert!(xml.contains("</D:multistatus>"));
    }

    #[test]
    fn test_proppatch_response_error_xml() {
        let response = PropPatchResponse::error(
            "/test.txt".to_string(),
            "Permission denied".to_string(),
        );
        let xml = response.to_xml();

        assert!(xml.contains("403 Forbidden"));
        assert!(xml.contains("Permission denied"));
        assert!(xml.contains("<D:responsedescription>"));
    }

    #[test]
    fn test_find_xml_blocks() {
        let xml = r#"<D:set>content1</D:set><D:set>content2</D:set>"#;
        let blocks = find_xml_blocks(xml, "D:set");
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("content1"));
        assert!(blocks[1].contains("content2"));
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("a<b"), "a&lt;b");
        assert_eq!(escape_xml("a&b"), "a&amp;b");
        assert_eq!(escape_xml("a\"b"), "a&quot;b");
    }

    #[test]
    fn test_parse_custom_property() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <my:custom>myvalue</my:custom>
    </D:prop>
  </D:set>
</D:propertyupdate>"#;

        let request = PropPatchRequest::from_xml(xml).unwrap();

        assert_eq!(request.modifications.len(), 1);
        match &request.modifications[0] {
            PropertyModification::Set { name, value, .. } => {
                assert_eq!(name, "custom");
                assert_eq!(value, "myvalue");
            }
            _ => panic!("Expected Set modification"),
        }
    }

    #[test]
    fn test_is_valid_proppatch() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <svn:executable>*</svn:executable>
    </D:prop>
  </D:set>
</D:propertyupdate>"#;

        let request = PropPatchRequest::from_xml(xml).unwrap();
        assert!(request.is_valid());
    }

    #[test]
    fn test_empty_proppatch() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
</D:propertyupdate>"#;

        let request = PropPatchRequest::from_xml(xml).unwrap();
        assert!(!request.is_valid());
        assert_eq!(request.modifications.len(), 0);
    }
}

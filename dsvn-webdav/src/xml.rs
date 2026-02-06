//! WebDAV XML parsing and generation utilities
//!
//! Handles XML serialization/deserialization for WebDAV operations

use quick_xml::events::{Event, *};
use quick_xml::writer::Writer;
use std::io::Cursor;

/// WebDAV XML namespace
pub const DAV_NS: &str = "DAV:";

/// Subversion XML namespace
pub const SVN_NS: &str = "http://subversion.tigris.org/xmlns/dav/";

/// Parse a WebDAV multistatus response
pub fn parse_multistatus(xml: &str) -> Result<Multistatus, XmlError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut multistatus = Multistatus::new();
    let mut current_response: Option<Response> = None;
    let mut current_propstat: Option<PropStat> = None;
    let mut in_prop = false;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"D:response" | b"response" => {
                    current_response = Some(Response::new());
                }
                b"D:propstat" | b"propstat" => {
                    current_propstat = Some(PropStat::new());
                }
                b"D:prop" | b"prop" => {
                    in_prop = true;
                }
                b"D:href" | b"href" => {
                    if let Some(ref mut resp) = current_response {
                        if let Ok(Event::Text(text)) = reader.read_event_into(&mut buf) {
                            resp.href = text.unescape().unwrap().into_owned();
                        }
                    }
                }
                b"D:status" | b"status" => {
                    if let Some(ref mut propstat) = current_propstat {
                        if let Ok(Event::Text(text)) = reader.read_event_into(&mut buf) {
                            propstat.status = text.unescape().unwrap().into_owned();
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"D:response" | b"response" => {
                    if let Some(resp) = current_response.take() {
                        multistatus.responses.push(resp);
                    }
                }
                b"D:propstat" | b"propstat" => {
                    if let Some(ref mut resp) = current_response {
                        if let Some(propstat) = current_propstat.take() {
                            resp.propstats.push(propstat);
                        }
                    }
                }
                b"D:prop" | b"prop" => {
                    in_prop = false;
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(XmlError::Parse(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(multistatus)
}

/// WebDAV multistatus response
#[derive(Debug, Clone)]
pub struct Multistatus {
    pub responses: Vec<Response>,
}

impl Multistatus {
    pub fn new() -> Self {
        Self {
            responses: Vec::new(),
        }
    }

    /// Serialize to XML
    pub fn to_xml(&self) -> Result<String, XmlError> {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        writer
            .create_element("D:multistatus")
            .with_attributes([("xmlns:D", DAV_NS), ("xmlns:svn", SVN_NS)])
            .write_inner_content(|w| {
                for response in &self.responses {
                    if let Err(e) = response.write_xml(w) {
                        return Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()));
                    }
                }
                Ok(())
            })
            .map_err(|e| XmlError::Serialization(e.to_string()))?;

        Ok(String::from_utf8_lossy(writer.into_inner().get_ref()).to_string())
    }
}

/// WebDAV response element
#[derive(Debug, Clone)]
pub struct Response {
    pub href: String,
    pub propstats: Vec<PropStat>,
}

impl Response {
    pub fn new() -> Self {
        Self {
            href: String::new(),
            propstats: Vec::new(),
        }
    }

    pub fn write_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<(), XmlError> {
        writer
            .create_element("D:response")
            .write_inner_content(|w| {
                // Write href
                w.create_element("D:href")
                    .write_text_content(BytesText::new(&self.href))?;

                // Write propstats
                for propstat in &self.propstats {
                    if let Err(e) = propstat.write_xml(w) {
                        return Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()));
                    }
                }

                Ok(())
            })
            .map_err(|e| XmlError::Serialization(e.to_string()))?;

        Ok(())
    }
}

/// WebDAV propstat element
#[derive(Debug, Clone)]
pub struct PropStat {
    pub props: Vec<Property>,
    pub status: String,
}

impl PropStat {
    pub fn new() -> Self {
        Self {
            props: Vec::new(),
            status: String::new(),
        }
    }

    pub fn write_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<(), XmlError> {
        writer
            .create_element("D:propstat")
            .write_inner_content(|w| {
                w.create_element("D:prop").write_inner_content(|_w| Ok(()))?;

                w.create_element("D:status")
                    .write_text_content(BytesText::new(&self.status))?;

                Ok(())
            })
            .map_err(|e| XmlError::Serialization(e.to_string()))?;

        Ok(())
    }
}

/// Property element
#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    pub value: Option<String>,
    pub namespace: Option<String>,
}

/// XML parsing errors
#[derive(Debug, thiserror::Error)]
pub enum XmlError {
    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Invalid XML structure: {0}")]
    InvalidStructure(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_multistatus() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/svn/</D:href>
    <D:propstat>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

        let result = parse_multistatus(xml);
        assert!(result.is_ok());
        let multistatus = result.unwrap();
        assert_eq!(multistatus.responses.len(), 1);
        assert_eq!(multistatus.responses[0].href, "/svn/");
    }

    #[test]
    fn test_serialize_multistatus() {
        let multistatus = Multistatus::new();
        let xml = multistatus.to_xml();
        assert!(xml.is_ok());
        assert!(xml.unwrap().contains("multistatus"));
    }
}

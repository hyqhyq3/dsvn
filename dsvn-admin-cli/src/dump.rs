//! SVN dump file parser
//!
//! Parses SVN dump format version 2/3 into structured `DumpFormat`.

use crate::dump_format::{DumpEntry, DumpFormat, NodeAction, NodeKind};
use anyhow::{anyhow, Result};
use std::io::{BufRead, Read};

/// Parse SVN dump file
pub fn parse_dump_file<R: BufRead>(mut reader: R) -> Result<DumpFormat> {
    let mut dump = DumpFormat::new();
    let mut current_rev_entry: Option<DumpEntry> = None;
    let mut current_node_entry: Option<DumpEntry> = None;
    let mut current_rev_num: u64 = 0;

    // Read header-style key-value pairs and content blocks
    let mut line_buf = String::new();

    loop {
        line_buf.clear();
        let bytes_read = reader.read_line(&mut line_buf)?;
        if bytes_read == 0 {
            break; // EOF
        }

        let trimmed = line_buf.trim();

        // --- Top-level headers ---
        if trimmed.starts_with("SVN-fs-dump-format-version:") {
            dump.format_version = parse_header_value(trimmed);
            continue;
        }

        if trimmed.starts_with("UUID:") {
            dump.uuid = parse_header_value(trimmed);
            continue;
        }

        // --- Revision header ---
        if trimmed.starts_with("Revision-number:") {
            // Flush any pending node
            if let Some(node) = current_node_entry.take() {
                dump.entries.push(node);
            }
            // Flush previous revision entry
            if let Some(rev) = current_rev_entry.take() {
                dump.entries.push(rev);
            }

            current_rev_num = parse_header_value(trimmed).parse().unwrap_or(0);
            let mut entry = DumpEntry::new(current_rev_num);

            // Read revision properties block
            let (prop_len, content_len) = read_content_headers(&mut reader)?;
            if content_len > 0 || prop_len > 0 {
                let total = if content_len > 0 { content_len } else { prop_len };
                let mut buf = vec![0u8; total];
                reader.read_exact(&mut buf)?;

                // Parse props from buffer
                if prop_len > 0 {
                    entry.props = parse_props(&buf[..prop_len]);
                }
            }

            current_rev_entry = Some(entry);
            continue;
        }

        // --- Node header ---
        if trimmed.starts_with("Node-path:") {
            // Flush any pending node
            if let Some(node) = current_node_entry.take() {
                dump.entries.push(node);
            }

            let mut entry = DumpEntry::new(current_rev_num);
            entry.node_path = Some(parse_header_value(trimmed));

            // Read remaining node headers until empty line or content
            loop {
                line_buf.clear();
                let n = reader.read_line(&mut line_buf)?;
                if n == 0 {
                    break;
                }
                let t = line_buf.trim();
                if t.is_empty() {
                    break;
                }

                if t.starts_with("Node-kind:") {
                    let kind_str = parse_header_value(t);
                    entry.node_kind = match kind_str.as_str() {
                        "file" => Some(NodeKind::File),
                        "dir" => Some(NodeKind::Dir),
                        _ => None,
                    };
                } else if t.starts_with("Node-action:") {
                    let action_str = parse_header_value(t);
                    entry.node_action = match action_str.as_str() {
                        "add" => Some(NodeAction::Add),
                        "delete" => Some(NodeAction::Delete),
                        "change" => Some(NodeAction::Change),
                        "replace" => Some(NodeAction::Replace),
                        _ => None,
                    };
                } else if t.starts_with("Node-copyfrom-path:") {
                    entry.copy_from_path = Some(parse_header_value(t));
                } else if t.starts_with("Node-copyfrom-rev:") {
                    entry.copy_from_rev = Some(parse_header_value(t).parse().unwrap_or(0));
                } else if t.starts_with("Text-content-md5:") {
                    entry.md5_checksum = Some(parse_header_value(t));
                } else if t.starts_with("Prop-content-length:") {
                    // Will be consumed below
                    let prop_len: usize = parse_header_value(t).parse().unwrap_or(0);
                    // Look for Content-length or Text-content-length
                    let mut text_len: usize = 0;
                    let mut total_content_len: usize = 0;

                    loop {
                        line_buf.clear();
                        let n2 = reader.read_line(&mut line_buf)?;
                        if n2 == 0 { break; }
                        let t2 = line_buf.trim();
                        if t2.is_empty() { break; }

                        if t2.starts_with("Text-content-length:") {
                            text_len = parse_header_value(t2).parse().unwrap_or(0);
                        } else if t2.starts_with("Content-length:") {
                            total_content_len = parse_header_value(t2).parse().unwrap_or(0);
                        } else if t2.starts_with("Text-content-md5:") {
                            entry.md5_checksum = Some(parse_header_value(t2));
                        } else if t2.starts_with("Text-content-sha1:") {
                            // ignore
                        }
                    }

                    // Read the combined content block
                    let read_len = if total_content_len > 0 {
                        total_content_len
                    } else {
                        prop_len + text_len
                    };

                    if read_len > 0 {
                        let mut buf = vec![0u8; read_len];
                        reader.read_exact(&mut buf)?;

                        // Parse props
                        if prop_len > 0 && prop_len <= buf.len() {
                            entry.props = parse_props(&buf[..prop_len]);
                        }

                        // Extract text content
                        if text_len > 0 && prop_len + text_len <= buf.len() {
                            entry.content = buf[prop_len..prop_len + text_len].to_vec();
                        }
                    }

                    // Consume trailing newline
                    line_buf.clear();
                    let _ = reader.read_line(&mut line_buf);

                    break;
                } else if t.starts_with("Text-content-length:") {
                    // Node with text but no props
                    let text_len: usize = parse_header_value(t).parse().unwrap_or(0);
                    let mut total_content_len: usize = 0;

                    loop {
                        line_buf.clear();
                        let n2 = reader.read_line(&mut line_buf)?;
                        if n2 == 0 { break; }
                        let t2 = line_buf.trim();
                        if t2.is_empty() { break; }
                        if t2.starts_with("Content-length:") {
                            total_content_len = parse_header_value(t2).parse().unwrap_or(0);
                        }
                    }

                    let read_len = if total_content_len > 0 { total_content_len } else { text_len };
                    if read_len > 0 {
                        let mut buf = vec![0u8; read_len];
                        reader.read_exact(&mut buf)?;
                        entry.content = buf[..text_len.min(read_len)].to_vec();
                    }

                    // Consume trailing newline
                    line_buf.clear();
                    let _ = reader.read_line(&mut line_buf);

                    break;
                } else if t.starts_with("Content-length:") {
                    // Content-length without Prop or Text headers (e.g., delete nodes)
                    let clen: usize = parse_header_value(t).parse().unwrap_or(0);

                    // Read to empty line (end of headers)
                    loop {
                        line_buf.clear();
                        let n2 = reader.read_line(&mut line_buf)?;
                        if n2 == 0 { break; }
                        if line_buf.trim().is_empty() { break; }
                    }

                    if clen > 0 {
                        let mut buf = vec![0u8; clen];
                        reader.read_exact(&mut buf)?;
                    }

                    line_buf.clear();
                    let _ = reader.read_line(&mut line_buf);

                    break;
                }
            }

            current_node_entry = Some(entry);
            continue;
        }
    }

    // Flush remaining
    if let Some(node) = current_node_entry.take() {
        dump.entries.push(node);
    }
    if let Some(rev) = current_rev_entry.take() {
        dump.entries.push(rev);
    }

    Ok(dump)
}

/// Parse "Key: Value" header, returning the value portion trimmed.
fn parse_header_value(line: &str) -> String {
    line.splitn(2, ": ")
        .nth(1)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Read Prop-content-length / Content-length headers that follow a Revision-number line.
/// Consumes lines until an empty line (header terminator).
/// Returns (prop_content_length, content_length).
fn read_content_headers<R: BufRead>(reader: &mut R) -> Result<(usize, usize)> {
    let mut prop_len: usize = 0;
    let mut content_len: usize = 0;
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let t = line.trim();
        if t.is_empty() {
            break;
        }
        if t.starts_with("Prop-content-length:") {
            prop_len = parse_header_value(t).parse().unwrap_or(0);
        } else if t.starts_with("Content-length:") {
            content_len = parse_header_value(t).parse().unwrap_or(0);
        }
    }

    Ok((prop_len, content_len))
}

/// Parse SVN properties block (K/V pairs terminated by PROPS-END)
fn parse_props(data: &[u8]) -> std::collections::HashMap<String, String> {
    let mut props = std::collections::HashMap::new();
    let text = String::from_utf8_lossy(data);
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line == "PROPS-END" {
            break;
        }

        if line.starts_with("K ") {
            let key_len: usize = line[2..].parse().unwrap_or(0);
            i += 1;
            if i >= lines.len() { break; }
            let key = &lines[i][..key_len.min(lines[i].len())];

            i += 1;
            if i >= lines.len() { break; }
            if lines[i].starts_with("V ") {
                let val_len: usize = lines[i][2..].parse().unwrap_or(0);
                i += 1;
                if i >= lines.len() { break; }
                let val = &lines[i][..val_len.min(lines[i].len())];
                props.insert(key.to_string(), val.to_string());
            }
        }
        i += 1;
    }

    props
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn test_parse_simple_dump() {
        let dump_data = b"SVN-fs-dump-format-version: 2\n\
UUID: test-uuid-1234\n\
\n\
Revision-number: 0\n\
Prop-content-length: 10\n\
Content-length: 10\n\
\n\
PROPS-END\n\
\n\
Revision-number: 1\n\
Prop-content-length: 56\n\
Content-length: 56\n\
\n\
K 7\n\
svn:log\n\
V 4\n\
test\n\
K 10\n\
svn:author\n\
V 4\n\
user\n\
PROPS-END\n\
\n\
Node-path: test.txt\n\
Node-kind: file\n\
Node-action: add\n\
Prop-content-length: 10\n\
Text-content-length: 5\n\
Content-length: 15\n\
\n\
PROPS-END\n\
hello\n";

        let reader = BufReader::new(&dump_data[..]);
        let dump = parse_dump_file(reader).unwrap();

        assert_eq!(dump.format_version, "2");
        assert_eq!(dump.uuid, "test-uuid-1234");
        // Should have: rev0 entry, rev1 entry, node entry
        assert!(dump.entries.len() >= 2);

        // Find the node entry
        let node = dump.entries.iter().find(|e| e.is_node());
        assert!(node.is_some(), "Should have a node entry");
        let node = node.unwrap();
        assert_eq!(node.node_path.as_deref(), Some("test.txt"));
        assert_eq!(node.node_kind, Some(NodeKind::File));
        assert_eq!(node.node_action, Some(NodeAction::Add));
        assert_eq!(node.content, b"hello");
    }
}

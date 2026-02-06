//! SVN dump format structures

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// SVN dump file format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpFormat {
    /// Format version (usually 2 or 3)
    pub format_version: String,

    /// Repository UUID
    pub uuid: String,

    /// Dump entries (revisions and nodes)
    pub entries: Vec<DumpEntry>,
}

impl DumpFormat {
    pub fn new() -> Self {
        Self {
            format_version: "3".to_string(),
            uuid: uuid::Uuid::new_v4().to_string(),
            entries: Vec::new(),
        }
    }
}

/// Dump entry (revision or node)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpEntry {
    /// Revision number
    pub revision_number: u64,

    /// Node path (if this is a node entry)
    pub node_path: Option<String>,

    /// Node kind
    pub node_kind: Option<NodeKind>,

    /// Node action
    pub node_action: Option<NodeAction>,

    /// Copy from path
    pub copy_from_path: Option<String>,

    /// Copy from revision
    pub copy_from_rev: Option<u64>,

    /// MD5 checksum
    pub md5_checksum: Option<String>,

    /// Properties
    pub props: HashMap<String, String>,

    /// Content data
    pub content: Vec<u8>,
}

impl DumpEntry {
    pub fn new(revision_number: u64) -> Self {
        Self {
            revision_number,
            node_path: None,
            node_kind: None,
            node_action: None,
            copy_from_path: None,
            copy_from_rev: None,
            md5_checksum: None,
            props: HashMap::new(),
            content: Vec::new(),
        }
    }

    /// Check if this is a revision entry (has no node path)
    pub fn is_revision(&self) -> bool {
        self.node_path.is_none()
    }

    /// Check if this is a node entry
    pub fn is_node(&self) -> bool {
        self.node_path.is_some()
    }
}

/// Node kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    File,
    Dir,
}

/// Node action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeAction {
    Add,
    Delete,
    Replace,
    Change,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dump_format_new() {
        let dump = DumpFormat::new();
        assert_eq!(dump.format_version, "3");
        assert!(!dump.uuid.is_empty());
        assert_eq!(dump.entries.len(), 0);
    }

    #[test]
    fn test_dump_entry_new() {
        let entry = DumpEntry::new(1);
        assert_eq!(entry.revision_number, 1);
        assert!(entry.is_revision());
        assert!(!entry.is_node());
    }

    #[test]
    fn test_node_entry() {
        let mut entry = DumpEntry::new(1);
        entry.node_path = Some("/trunk".to_string());
        entry.node_kind = Some(NodeKind::Dir);
        entry.node_action = Some(NodeAction::Add);

        assert!(!entry.is_revision());
        assert!(entry.is_node());
    }
}

//! Core object model for DSvn
//!
//! Implements content-addressable storage with Blob, Tree, and Commit objects
//! Similar to Git's object model but optimized for SVN workflows

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Unique identifier for any stored object
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectId([u8; 32]);

impl ObjectId {
    /// Create a new ObjectId from raw bytes
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Compute ObjectId from data
    pub fn from_data(data: &[u8]) -> Self {
        let hash = Sha256::digest(data);
        Self(hash.into())
    }

    /// Convert to hexadecimal string
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from hexadecimal string
    pub fn from_hex(hex_str: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(hex_str)?;
        if bytes.len() != 32 {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    /// Get raw bytes
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// File content object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blob {
    /// Raw content data
    pub data: Vec<u8>,
    /// Content length (cached)
    #[serde(skip)]
    pub size: u64,
    /// Executable flag (for Unix permissions)
    pub executable: bool,
}

impl Blob {
    /// Create a new blob from data
    pub fn new(data: Vec<u8>, executable: bool) -> Self {
        let size = data.len() as u64;
        Self {
            data,
            size,
            executable,
        }
    }

    /// Create a new blob from data (non-executable)
    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self::new(data, false)
    }

    /// Deserialize from binary format
    pub fn deserialize(data: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(data)
    }

    /// Compute the object ID
    pub fn id(&self) -> ObjectId {
        ObjectId::from_data(&self.data)
    }

    /// Serialize to binary format
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }
}

/// Directory tree entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeEntry {
    /// Name of the entry
    pub name: String,
    /// Object ID (points to Blob or Tree)
    pub id: ObjectId,
    /// Entry type
    pub kind: ObjectKind,
    /// File permissions (Unix mode)
    pub mode: u32,
}

impl TreeEntry {
    /// Create a new tree entry
    pub fn new(name: String, id: ObjectId, kind: ObjectKind, mode: u32) -> Self {
        Self {
            name,
            id,
            kind,
            mode,
        }
    }
}

/// Directory object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    /// Sorted entries for deterministic hashing
    pub entries: BTreeMap<String, TreeEntry>,
}

impl Tree {
    /// Create an empty tree
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Add or update an entry
    pub fn insert(&mut self, entry: TreeEntry) {
        self.entries.insert(entry.name.clone(), entry);
    }

    /// Remove an entry
    pub fn remove(&mut self, name: &str) -> Option<TreeEntry> {
        self.entries.remove(name)
    }

    /// Get an entry
    pub fn get(&self, name: &str) -> Option<&TreeEntry> {
        self.entries.get(name)
    }

    /// Compute the object ID
    pub fn id(&self) -> ObjectId {
        ObjectId::from_data(&bincode::serialize(self).unwrap_or_default())
    }

    /// Serialize to binary format
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// Deserialize from binary format
    pub fn from_bytes(data: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(data)
    }

    /// Iterate over entries
    pub fn iter(&self) -> impl Iterator<Item = &TreeEntry> {
        self.entries.values()
    }
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

/// Object type discriminator
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectKind {
    Blob,
    Tree,
    Commit,
}

/// Commit/Revision object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    /// Tree object ID for this revision
    pub tree_id: ObjectId,
    /// Parent commit IDs (empty for initial commit)
    pub parents: Vec<ObjectId>,
    /// Author name
    pub author: String,
    /// Commit message
    pub message: String,
    /// Commit timestamp (Unix seconds)
    pub timestamp: i64,
    /// Timezone offset in minutes
    pub tz_offset: i32,
}

impl Commit {
    /// Create a new commit
    pub fn new(
        tree_id: ObjectId,
        parents: Vec<ObjectId>,
        author: String,
        message: String,
        timestamp: i64,
        tz_offset: i32,
    ) -> Self {
        Self {
            tree_id,
            parents,
            author,
            message,
            timestamp,
            tz_offset,
        }
    }

    /// Compute the object ID
    pub fn id(&self) -> ObjectId {
        ObjectId::from_data(&bincode::serialize(self).unwrap_or_default())
    }

    /// Serialize to binary format
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// Deserialize from binary format
    pub fn from_bytes(data: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(data)
    }

    /// Check if this is an initial commit (no parents)
    pub fn is_initial(&self) -> bool {
        self.parents.is_empty()
    }
}

/// Generic object that can be any type
#[derive(Debug, Clone)]
pub enum Object {
    Blob(Blob),
    Tree(Tree),
    Commit(Commit),
}

impl Object {
    /// Get the object ID
    pub fn id(&self) -> ObjectId {
        match self {
            Object::Blob(blob) => blob.id(),
            Object::Tree(tree) => tree.id(),
            Object::Commit(commit) => commit.id(),
        }
    }

    /// Get the object kind
    pub fn kind(&self) -> ObjectKind {
        match self {
            Object::Blob(_) => ObjectKind::Blob,
            Object::Tree(_) => ObjectKind::Tree,
            Object::Commit(_) => ObjectKind::Commit,
        }
    }

    /// Serialize to binary format
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        match self {
            Object::Blob(blob) => blob.to_bytes(),
            Object::Tree(tree) => tree.to_bytes(),
            Object::Commit(commit) => commit.to_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_id_roundtrip() {
        let bytes = [42u8; 32];
        let id = ObjectId::new(bytes);
        let hex = id.to_hex();
        let id2 = ObjectId::from_hex(&hex).unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn test_blob_id() {
        let blob = Blob::from_bytes(b"hello world".to_vec());
        let id = blob.id();
        assert_eq!(id.to_hex().len(), 64);
    }

    #[test]
    fn test_tree_insert_remove() {
        let mut tree = Tree::new();
        let entry = TreeEntry::new(
            "test.txt".to_string(),
            ObjectId::new([0u8; 32]),
            ObjectKind::Blob,
            0o644,
        );
        tree.insert(entry.clone());
        assert!(tree.get("test.txt").is_some());
        tree.remove("test.txt");
        assert!(tree.get("test.txt").is_none());
    }

    #[test]
    fn test_commit_serialization() {
        let commit = Commit::new(
            ObjectId::new([1u8; 32]),
            vec![ObjectId::new([2u8; 32])],
            "Test Author".to_string(),
            "Test message".to_string(),
            1234567890,
            0,
        );
        let bytes = commit.to_bytes().unwrap();
        let commit2 = Commit::from_bytes(&bytes).unwrap();
        assert_eq!(commit.id(), commit2.id());
    }
}

//! Replication protocol definitions for dsvn.
//!
//! Defines the wire protocol for dsvn's own efficient synchronization,
//! including handshake, delta transfer, and verification messages.

use crate::object::{DeltaTree, ObjectId};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Protocol version constant.
pub const PROTOCOL_VERSION: u32 = 1;

/// Magic bytes for the dsvn sync protocol.
pub const PROTOCOL_MAGIC: &[u8; 4] = b"DSVN";

/// Maximum single message size (256 MB).
pub const MAX_MESSAGE_SIZE: usize = 256 * 1024 * 1024;

/// Compression method for transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compression {
    None,
    Zstd,
}

/// Handshake request from the sync client to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeRequest {
    /// Protocol magic.
    pub magic: [u8; 4],
    /// Protocol version.
    pub version: u32,
    /// Client's preferred compression method.
    pub compression: Compression,
    /// Client capabilities.
    pub capabilities: Vec<String>,
}

impl HandshakeRequest {
    pub fn new() -> Self {
        Self {
            magic: *PROTOCOL_MAGIC,
            version: PROTOCOL_VERSION,
            compression: Compression::Zstd,
            capabilities: vec![
                "incremental-sync".to_string(),
                "delta-transfer".to_string(),
                "checkpoint-resume".to_string(),
                "hash-verify".to_string(),
            ],
        }
    }
}

/// Handshake response from the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    /// Protocol version (negotiated).
    pub version: u32,
    /// Repository UUID.
    pub uuid: String,
    /// Current head revision.
    pub head_rev: u64,
    /// Agreed compression method.
    pub compression: Compression,
    /// Server capabilities.
    pub capabilities: Vec<String>,
    /// Whether the handshake was accepted.
    pub accepted: bool,
    /// Error message if rejected.
    pub error: Option<String>,
}

/// Request for a range of revisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    /// Start revision (inclusive).
    pub from_rev: u64,
    /// End revision (inclusive; 0 means "up to HEAD").
    pub to_rev: u64,
    /// Whether to include full object data (vs. just deltas).
    pub include_objects: bool,
    /// Whether to include file properties.
    pub include_properties: bool,
}

/// A single revision's data for transfer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionData {
    /// Revision number.
    pub revision: u64,
    /// Author.
    pub author: String,
    /// Commit message.
    pub message: String,
    /// Commit timestamp (Unix seconds).
    pub timestamp: i64,
    /// Delta tree (changes relative to previous revision).
    pub delta_tree: DeltaTree,
    /// Object data: list of (ObjectId, compressed bytes).
    pub objects: Vec<(ObjectId, Vec<u8>)>,
    /// Revision properties (name → value).
    pub properties: Vec<(String, String)>,
    /// SHA-256 hash of all object data for verification.
    pub content_hash: ObjectId,
}

impl RevisionData {
    /// Compute a content hash over all objects in this revision.
    pub fn compute_content_hash(objects: &[(ObjectId, Vec<u8>)]) -> ObjectId {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for (id, data) in objects {
            hasher.update(id.as_bytes());
            hasher.update(&(data.len() as u64).to_le_bytes());
            hasher.update(data);
        }
        let hash = hasher.finalize();
        ObjectId::new(hash.into())
    }

    /// Verify the content hash.
    pub fn verify_content_hash(&self) -> bool {
        let computed = Self::compute_content_hash(&self.objects);
        computed == self.content_hash
    }
}

/// Acknowledgment after receiving a revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncAck {
    /// The revision that was applied.
    pub revision: u64,
    /// Whether the revision was successfully applied.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
    /// Content hash as computed by the receiver (for verification).
    pub received_hash: ObjectId,
}

/// Overall sync completion message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncComplete {
    /// First revision synced.
    pub from_rev: u64,
    /// Last revision synced.
    pub to_rev: u64,
    /// Total revisions synced.
    pub revisions_synced: u64,
    /// Total objects transferred.
    pub objects_transferred: u64,
    /// Total bytes transferred (compressed).
    pub bytes_transferred: u64,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Whether all revisions succeeded.
    pub success: bool,
}

/// Envelope for all protocol messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMessage {
    HandshakeRequest(HandshakeRequest),
    HandshakeResponse(HandshakeResponse),
    SyncRequest(SyncRequest),
    RevisionData(RevisionData),
    SyncAck(SyncAck),
    SyncComplete(SyncComplete),
    /// Heartbeat/keep-alive.
    Ping,
    Pong,
    /// Error message.
    Error(String),
}

impl SyncMessage {
    /// Serialize a message to bytes with a length prefix.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let payload = bincode::serialize(self)
            .map_err(|e| anyhow!("Failed to serialize sync message: {}", e))?;
        if payload.len() > MAX_MESSAGE_SIZE {
            return Err(anyhow!(
                "Message too large: {} bytes (max {})",
                payload.len(),
                MAX_MESSAGE_SIZE
            ));
        }
        let mut buf = Vec::with_capacity(4 + payload.len());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&payload);
        Ok(buf)
    }

    /// Deserialize a message from bytes (after reading the length prefix).
    pub fn decode(data: &[u8]) -> Result<Self> {
        bincode::deserialize(data)
            .map_err(|e| anyhow!("Failed to deserialize sync message: {}", e))
    }

    /// Encode with zstd compression.
    pub fn encode_compressed(&self) -> Result<Vec<u8>> {
        let payload = bincode::serialize(self)
            .map_err(|e| anyhow!("Failed to serialize sync message: {}", e))?;
        let compressed = zstd::encode_all(&payload[..], 3)
            .map_err(|e| anyhow!("Failed to compress sync message: {}", e))?;
        // Header: [magic(4)] [flags(1)] [uncompressed_len(4)] [compressed_len(4)] [data]
        let mut buf = Vec::with_capacity(13 + compressed.len());
        buf.extend_from_slice(PROTOCOL_MAGIC);
        buf.push(0x01); // flags: compressed
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        buf.extend_from_slice(&compressed);
        Ok(buf)
    }

    /// Decode a compressed message.
    pub fn decode_compressed(data: &[u8]) -> Result<Self> {
        if data.len() < 13 {
            return Err(anyhow!("Message too short for header"));
        }
        if &data[0..4] != PROTOCOL_MAGIC {
            return Err(anyhow!("Invalid protocol magic"));
        }
        let flags = data[4];
        let _uncompressed_len = u32::from_le_bytes(data[5..9].try_into().unwrap()) as usize;
        let compressed_len = u32::from_le_bytes(data[9..13].try_into().unwrap()) as usize;

        if data.len() < 13 + compressed_len {
            return Err(anyhow!("Message truncated"));
        }

        let payload = if flags & 0x01 != 0 {
            zstd::decode_all(&data[13..13 + compressed_len])
                .map_err(|e| anyhow!("Failed to decompress: {}", e))?
        } else {
            data[13..13 + compressed_len].to_vec()
        };

        Self::decode(&payload)
    }
}

/// Summary info returned by the source repository for sync negotiation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub uuid: String,
    pub head_rev: u64,
    pub youngest_rev: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{ObjectKind, TreeChange, TreeEntry};

    #[test]
    fn test_handshake_request() {
        let req = HandshakeRequest::new();
        assert_eq!(req.magic, *PROTOCOL_MAGIC);
        assert_eq!(req.version, PROTOCOL_VERSION);
        assert!(!req.capabilities.is_empty());
    }

    #[test]
    fn test_sync_message_encode_decode() {
        let msg = SyncMessage::Ping;
        let encoded = msg.encode().unwrap();
        assert!(encoded.len() > 4);

        // Decode: skip 4-byte length prefix
        let len = u32::from_le_bytes(encoded[0..4].try_into().unwrap()) as usize;
        let decoded = SyncMessage::decode(&encoded[4..4 + len]).unwrap();
        match decoded {
            SyncMessage::Ping => {}
            _ => panic!("Expected Ping"),
        }
    }

    #[test]
    fn test_sync_message_compressed_roundtrip() {
        let msg = SyncMessage::HandshakeRequest(HandshakeRequest::new());
        let encoded = msg.encode_compressed().unwrap();
        let decoded = SyncMessage::decode_compressed(&encoded).unwrap();
        match decoded {
            SyncMessage::HandshakeRequest(req) => {
                assert_eq!(req.version, PROTOCOL_VERSION);
            }
            _ => panic!("Expected HandshakeRequest"),
        }
    }

    #[test]
    fn test_revision_data_content_hash() {
        let objects = vec![
            (ObjectId::from_data(b"hello"), b"hello".to_vec()),
            (ObjectId::from_data(b"world"), b"world".to_vec()),
        ];
        let hash = RevisionData::compute_content_hash(&objects);

        let rev_data = RevisionData {
            revision: 1,
            author: "test".to_string(),
            message: "test commit".to_string(),
            timestamp: 1000,
            delta_tree: DeltaTree::new(0, vec![], 0),
            objects: objects.clone(),
            properties: vec![],
            content_hash: hash,
        };

        assert!(rev_data.verify_content_hash());

        // Tamper with an object
        let mut tampered = rev_data.clone();
        tampered.objects[0].1 = b"tampered".to_vec();
        assert!(!tampered.verify_content_hash());
    }

    #[test]
    fn test_sync_message_error() {
        let msg = SyncMessage::Error("test error".to_string());
        let encoded = msg.encode().unwrap();
        let len = u32::from_le_bytes(encoded[0..4].try_into().unwrap()) as usize;
        let decoded = SyncMessage::decode(&encoded[4..4 + len]).unwrap();
        match decoded {
            SyncMessage::Error(e) => assert_eq!(e, "test error"),
            _ => panic!("Expected Error"),
        }
    }

    #[test]
    fn test_revision_data_serialization() {
        let rd = RevisionData {
            revision: 42,
            author: "alice".to_string(),
            message: "add feature".to_string(),
            timestamp: 1700000000,
            delta_tree: DeltaTree::new(41, vec![
                TreeChange::Upsert {
                    path: "src/main.rs".to_string(),
                    entry: TreeEntry::new(
                        "src/main.rs".to_string(),
                        ObjectId::from_data(b"main.rs content"),
                        ObjectKind::Blob,
                        0o644,
                    ),
                },
            ], 10),
            objects: vec![(ObjectId::from_data(b"main.rs content"), b"fn main() {}".to_vec())],
            properties: vec![("svn:log".to_string(), "add feature".to_string())],
            content_hash: RevisionData::compute_content_hash(&[
                (ObjectId::from_data(b"main.rs content"), b"fn main() {}".to_vec()),
            ]),
        };

        let msg = SyncMessage::RevisionData(rd.clone());
        let encoded = msg.encode_compressed().unwrap();
        let decoded = SyncMessage::decode_compressed(&encoded).unwrap();
        match decoded {
            SyncMessage::RevisionData(d) => {
                assert_eq!(d.revision, 42);
                assert_eq!(d.author, "alice");
                assert!(d.verify_content_hash());
            }
            _ => panic!("Expected RevisionData"),
        }
    }
}

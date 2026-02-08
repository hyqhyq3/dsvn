//! DSvn Core Library
//!
//! Core functionality for DSvn including:
//! - Object model (Blob, Tree, Commit)
//! - Storage abstraction (hot/warm/cold tiers)
//! - Repository operations
//! - In-memory repository for MVP
//! - Persistent repository using Fjall LSM-tree
//! - SQLite repository using rusqlite (WAL mode)
//! - Sync state management and replication protocol
//! - Replication log and delta transfer

pub mod object;
pub mod storage;
pub mod repository;
pub mod persistent;
pub mod sqlite_repository;
pub mod hot_store;
pub mod hooks;
pub mod packfile;
pub mod properties;
pub mod sync;
pub mod replication;

#[cfg(test)]
mod persistent_tests;

pub use object::{Blob, Commit, DeltaTree, Object, ObjectId, ObjectKind, Tree, TreeChange, TreeEntry};
pub use repository::Repository;
pub use persistent::{PersistentRepository, RepositoryMetadata};
pub use sqlite_repository::{SqliteRepository, SqlitePropertyStore};
pub use hooks::HookManager;
pub use storage::{HotStore, ObjectStore, Result, StorageError, TieredStore, WarmStore};
pub use sync::{SyncState, ReplicationLog, ReplicationLogEntry, SyncEndpointInfo, SyncConfig, RevisionSummary};
pub use replication::{
    SyncMessage, HandshakeRequest, HandshakeResponse, SyncRequest,
    RevisionData, SyncAck, SyncComplete, Compression, RepositoryInfo,
    PROTOCOL_VERSION, PROTOCOL_MAGIC,
};

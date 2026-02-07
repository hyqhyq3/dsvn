//! DSvn Core Library
//!
//! Core functionality for DSvn including:
//! - Object model (Blob, Tree, Commit)
//! - Storage abstraction (hot/warm/cold tiers)
//! - Repository operations
//! - In-memory repository for MVP
//! - Persistent repository using Fjall LSM-tree
//! - Disk repository using sled
//! - SQLite repository using rusqlite (WAL mode)

pub mod object;
pub mod storage;
pub mod repository;
pub mod persistent;
pub mod disk_repository;
pub mod sqlite_repository;
pub mod hot_store;
pub mod packfile;
pub mod properties;

#[cfg(test)]
mod persistent_tests;

pub use object::{Blob, Commit, Object, ObjectId, ObjectKind, Tree, TreeEntry};
pub use repository::Repository;
pub use persistent::{PersistentRepository, RepositoryMetadata};
pub use disk_repository::{DiskRepository, DiskPropertyStore};
pub use sqlite_repository::{SqliteRepository, SqlitePropertyStore};
pub use storage::{HotStore, ObjectStore, Result, StorageError, TieredStore, WarmStore};

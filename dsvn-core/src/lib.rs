//! DSvn Core Library
//!
//! Core functionality for DSvn including:
//! - Object model (Blob, Tree, Commit)
//! - Storage abstraction (hot/warm/cold tiers)
//! - Repository operations
//! - In-memory repository for MVP
//! - Persistent repository using Fjall LSM-tree

pub mod object;
pub mod storage;
pub mod repository;
pub mod persistent;

#[cfg(test)]
mod persistent_tests;

pub use object::{Blob, Commit, Object, ObjectId, ObjectKind, Tree, TreeEntry};
pub use repository::Repository;
pub use persistent::{PersistentRepository, RepositoryMetadata};
pub use storage::{HotStore, ObjectStore, Result, StorageError, TieredStore, WarmStore};

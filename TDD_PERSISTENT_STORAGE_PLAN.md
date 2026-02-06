# TDD Session: Persistent Storage Implementation

## Goal

Implement persistent repository storage using Fjall LSM-tree to replace the in-memory MVP implementation.

## TDD Cycle Overview

```
RED → GREEN → REFACTOR → REPEAT
```

## Step 1: Define Interface (SCAFFOLD)

### Core Traits Already Exist

From `dsvn-core/src/storage.rs`:

```rust
#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn get(&self, id: ObjectId) -> Result<Bytes>;
    async fn put(&self, data: Bytes) -> Result<ObjectId>;
    async fn exists(&self, id: ObjectId) -> Result<bool>;
}
```

### PersistentRepository Interface

```rust
// dsvn-core/src/persistent.rs

use crate::object::{ObjectId, Commit};
use crate::storage::ObjectStore;
use anyhow::Result;
use bytes::Bytes;

/// Persistent repository using Fjall LSM-tree
pub struct PersistentRepository {
    /// Hot storage (Fjall LSM-tree)
    hot_store: FjallStore,

    /// Repository metadata
    metadata: RepositoryMetadata,
}

/// Repository metadata
pub struct RepositoryMetadata {
    pub uuid: String,
    pub current_rev: u64,
    pub created_at: i64,
}

impl PersistentRepository {
    /// Open or create repository at path
    pub async fn open(path: &Path) -> Result<Self> {
        todo!("Implement in GREEN phase")
    }

    /// Get current revision number
    pub async fn current_rev(&self) -> u64 {
        todo!("Implement in GREEN phase")
    }

    /// Get file content by path and revision
    pub async fn get_file(&self, path: &str, rev: u64) -> Result<Bytes> {
        todo!("Implement in GREEN phase")
    }

    /// Add file to repository
    pub async fn add_file(&self, path: &str, content: Vec<u8>, executable: bool) -> Result<ObjectId> {
        todo!("Implement in GREEN phase")
    }

    /// Create commit
    pub async fn commit(&self, author: String, message: String, timestamp: i64) -> Result<u64> {
        todo!("Implement in GREEN phase")
    }

    /// Get commit log
    pub async fn log(&self, start_rev: u64, limit: usize) -> Result<Vec<Commit>> {
        todo!("Implement in GREEN phase")
    }

    /// Initialize repository
    pub async fn initialize(&self) -> Result<()> {
        todo!("Implement in GREEN phase")
    }

    /// Get repository UUID
    pub fn uuid(&self) -> &str {
        todo!("Implement in GREEN phase")
    }
}
```

## Step 2: Write Failing Tests (RED)

```rust
// dsvn-core/src/persistent_tests.rs

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::test;

    #[tokio::test]
    async fn test_create_persistent_repository() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // Test: Should create new repository
        let repo = PersistentRepository::open(repo_path).await.unwrap();

        assert!(!repo.uuid().is_empty());
        assert_eq!(repo.current_rev().await, 0);
    }

    #[tokio::test]
    async fn test_persist_and_retrieve_file() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // Create repository and add file
        let repo = PersistentRepository::open(repo_path).await.unwrap();
        repo.initialize().await.unwrap();

        let content = b"Hello, persistent storage!".to_vec();
        let id = repo.add_file("/test.txt", content.clone(), false).await.unwrap();

        // Close and reopen repository
        drop(repo);
        let repo = PersistentRepository::open(repo_path).await.unwrap();

        // Should retrieve same content
        let retrieved = repo.get_file("/test.txt", 1).await.unwrap();
        assert_eq!(retrieved.to_vec(), content);
    }

    #[tokio::test]
    async fn test_commit_persists_across_restarts() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // Create repository and make commits
        let repo = PersistentRepository::open(repo_path).await.unwrap();
        repo.initialize().await.unwrap();

        let rev1 = repo.commit("user1".into(), "Commit 1".into(), 1000).await.unwrap();
        let rev2 = repo.commit("user2".into(), "Commit 2".into(), 2000).await.unwrap();

        assert_eq!(rev1, 1);
        assert_eq!(rev2, 2);

        // Close and reopen
        drop(repo);
        let repo = PersistentRepository::open(repo_path).await.unwrap();

        // Should have same commits
        assert_eq!(repo.current_rev().await, 2);

        let log = repo.log(2, 10).await.unwrap();
        assert_eq!(log.len(), 3); // Initial + 2 commits
        assert_eq!(log[0].author, "user2");
        assert_eq!(log[1].author, "user1");
    }

    #[tokio::test]
    async fn test_repository_metadata_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // Create repository with custom UUID
        let repo = PersistentRepository::open(repo_path).await.unwrap();
        let original_uuid = repo.uuid().to_string();

        // Reopen
        drop(repo);
        let repo = PersistentRepository::open(repo_path).await.unwrap();

        // Should have same UUID
        assert_eq!(repo.uuid(), original_uuid);
    }

    #[tokio::test]
    async fn test_open_existing_repository() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // Create repository
        let repo1 = PersistentRepository::open(repo_path).await.unwrap();
        repo1.initialize().await.unwrap();
        let uuid1 = repo1.uuid().to_string();
        drop(repo1);

        // Open existing repository
        let repo2 = PersistentRepository::open(repo_path).await.unwrap();
        assert_eq!(repo2.uuid(), uuid1);
        assert_eq!(repo2.current_rev().await, 0);
    }

    #[tokio::test]
    async fn test_large_file_storage() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        let repo = PersistentRepository::open(repo_path).await.unwrap();
        repo.initialize().await.unwrap();

        // Create 10MB file
        let large_content = vec![0x42u8; 10 * 1024 * 1024];
        repo.add_file("/large.bin", large_content.clone(), false).await.unwrap();

        // Should retrieve successfully
        let retrieved = repo.get_file("/large.bin", 1).await.unwrap();
        assert_eq!(retrieved.len(), large_content.len());
        assert_eq!(retrieved.to_vec(), large_content);
    }
}
```

## Step 3: Run Tests - Verify FAIL

```bash
cargo test -p dsvn-core persistent

Expected output:
FAIL persistent_tests
  ✕ test_create_persistent_repository (2 ms)
    Error: not implemented

  ✕ test_persist_and_retrieve_file (1 ms)
    Error: not implemented

  ...

All tests failing as expected ✅
```

## Step 4: Implement Minimal Code (GREEN)

### Phase 1: Basic Structure

```rust
// dsvn-core/src/persistent.rs

use crate::object::{Blob, Commit, ObjectId, ObjectKind, Tree, TreeEntry};
use crate::storage::ObjectStore;
use anyhow::{anyhow, Result};
use bytes::Bytes;
use fjall::{Config, KvSeparationOptions, PersistMode, Transaction};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct PersistentRepository {
    /// Fjall LSM-tree instance
    db: Arc<fjall::Keyspace>,

    /// Objects tree (key: ObjectId, value: serialized object)
    objects: Arc<fjall::Tree>,

    /// Commits tree (key: revision number, value: commit metadata)
    commits: Arc<fjall::Tree>,

    /// Path index (key: path, value: latest ObjectId)
    path_index: Arc<fjall::Tree>,

    /// Metadata
    metadata: Arc<RwLock<RepositoryMetadata>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryMetadata {
    pub uuid: String,
    pub current_rev: u64,
    pub created_at: i64,
}

impl PersistentRepository {
    pub async fn open(path: &Path) -> Result<Self> {
        // Create/open Fjall keyspace
        let config = Config::new(path);
        let keyspace = fjall::Keyspace::open(config)?;

        // Open trees
        let objects = keyspace.open_tree("objects")?;
        let commits = keyspace.open_tree("commits")?;
        let path_index = keyspace.open_tree("path_index")?;

        // Load or create metadata
        let metadata_key = b"metadata";
        let metadata = if let Some(bytes) = commits.get(metadata_key)? {
            let meta: RepositoryMetadata = bincode::deserialize(&bytes)?;
            Arc::new(RwLock::new(meta))
        } else {
            let meta = RepositoryMetadata {
                uuid: uuid::Uuid::new_v4().to_string(),
                current_rev: 0,
                created_at: chrono::Utc::now().timestamp(),
            };
            Arc::new(RwLock::new(meta))
        };

        Ok(Self {
            db: Arc::new(keyspace),
            objects: Arc::new(objects),
            commits: Arc::new(commits),
            path_index: Arc::new(path_index),
            metadata,
        })
    }

    pub async fn current_rev(&self) -> u64 {
        self.metadata.read().await.current_rev
    }

    pub fn uuid(&self) -> &str {
        // This is a problem - can't return &str from async context holding lock
        // Need to clone
        todo!("Fix in refactor phase")
    }

    pub async fn initialize(&self) -> Result<()> {
        // Create initial commit
        let tree = Tree::new();
        let tree_id = tree.id();
        let tree_data = tree.to_bytes()?;

        // Store tree
        self.objects.insert(tree_id.to_hex(), tree_data)?;

        // Create commit
        let commit = Commit::new(
            tree_id,
            vec![],
            "system".to_string(),
            "Initial commit".to_string(),
            chrono::Utc::now().timestamp(),
            0,
        );

        let commit_data = commit.to_bytes()?;
        self.commits.insert(b"0", commit_data)?;

        // Update metadata
        let mut meta = self.metadata.write().await;
        meta.current_rev = 0;
        self.save_metadata(&meta).await?;

        Ok(())
    }

    async fn save_metadata(&self, meta: &RepositoryMetadata) -> Result<()> {
        let bytes = bincode::serialize(meta)?;
        self.commits.insert(b"metadata", bytes)?;
        Ok(())
    }
}
```

### Phase 2: Add File Operations

```rust
impl PersistentRepository {
    pub async fn add_file(&self, path: &str, content: Vec<u8>, executable: bool) -> Result<ObjectId> {
        // Create blob
        let blob = Blob::new(content, executable);
        let blob_id = blob.id();
        let blob_data = blob.to_bytes()?;

        // Store in object tree
        self.objects.insert(blob_id.to_hex(), blob_data)?;

        // Update path index
        self.path_index.insert(path.as_bytes(), blob_id.to_hex())?;

        Ok(blob_id)
    }

    pub async fn get_file(&self, path: &str, rev: u64) -> Result<Bytes> {
        // Get commit for revision
        let commit_key = rev.to_string();
        let commit_data = self.commits.get(commit_key.as_bytes())?
            .ok_or_else(|| anyhow!("Revision {} not found", rev))?;

        let commit: Commit = bincode::deserialize(&commit_data)?;

        // Get tree
        let tree_data = self.objects.get(commit.tree_id.to_hex().as_bytes())?
            .ok_or_else(|| anyhow!("Tree {} not found", commit.tree_id))?;

        let tree: Tree = bincode::deserialize(&tree_data)?;

        // Navigate path
        let path_parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let mut current_tree = tree;

        for (i, part) in path_parts.iter().enumerate() {
            if part.is_empty() { continue; }

            if let Some(entry) = current_tree.get(*part) {
                if i == path_parts.len() - 1 {
                    // This is the file - get blob
                    let blob_data = self.objects.get(entry.id.to_hex().as_bytes())?
                        .ok_or_else(|| anyhow!("Blob {} not found", entry.id))?;

                    let blob: Blob = bincode::deserialize(&blob_data)?;
                    return Ok(Bytes::from(blob.data));
                } else {
                    // This is a directory - get tree
                    let tree_data = self.objects.get(entry.id.to_hex().as_bytes())?
                        .ok_or_else(|| anyhow!("Tree {} not found", entry.id))?;

                    current_tree = bincode::deserialize(&tree_data)?;
                }
            }
        }

        Err(anyhow!("Path not found: {}", path))
    }
}
```

## Step 5: Run Tests - Verify FAIL

```bash
cargo test -p dsvn-core persistent

Expected errors:
  ✕ test_create_persistent_repository - compile error: uuid() lifetime issue
  ✕ test_persist_and_retrieve_file - not yet implemented

Ready to fix...
```

## Step 6: Fix Issues and Complete Implementation

Continue implementing each method to make tests pass. This is the GREEN phase.

## Step 7: Refactor (IMPROVE)

Once tests pass:
1. Extract common patterns
2. Add helper methods
3. Improve error messages
4. Add documentation
5. Optimize hot paths

## Step 8: Check Coverage

```bash
cargo test -p dsvn-core --coverage

Target: 80%+ coverage
```

---

**Next Action**: Run the tests to verify they fail, then implement!

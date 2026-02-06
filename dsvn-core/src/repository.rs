//! In-memory repository implementation for MVP
//!
//! Provides a simple in-memory repository to support basic checkout/commit operations

use crate::object::{Blob, Commit, ObjectId, ObjectKind, Tree, TreeEntry};
use anyhow::Result;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// In-memory repository
pub struct Repository {
    /// Object storage
    objects: Arc<RwLock<HashMap<ObjectId, Bytes>>>,

    /// Current root tree
    root_tree: Arc<RwLock<Tree>>,

    /// Current revision number
    current_rev: Arc<RwLock<u64>>,

    /// Commit history (rev -> commit)
    commits: Arc<RwLock<HashMap<u64, Commit>>>,

    /// Path → Object ID mapping (for quick lookup)
    path_index: Arc<RwLock<HashMap<String, ObjectId>>>,

    /// Repository UUID
    uuid: String,
}

impl Repository {
    /// Create a new in-memory repository
    pub fn new() -> Self {
        let tree = Tree::new();
        // Initialize with empty root
        let uuid = uuid::Uuid::new_v4().to_string();

        Self {
            objects: Arc::new(RwLock::new(HashMap::new())),
            root_tree: Arc::new(RwLock::new(tree)),
            current_rev: Arc::new(RwLock::new(0)),
            commits: Arc::new(RwLock::new(HashMap::new())),
            path_index: Arc::new(RwLock::new(HashMap::new())),
            uuid,
        }
    }

    /// Get repository UUID
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// Get current revision
    pub async fn current_rev(&self) -> u64 {
        *self.current_rev.read().await
    }

    /// Get file content by path
    pub async fn get_file(&self, path: &str, rev: u64) -> Result<Bytes> {
        // Get commit for revision
        let commits = self.commits.read().await;
        let commit = commits
            .get(&rev)
            .ok_or_else(|| anyhow::anyhow!("Revision {} not found", rev))?;

        // Get tree
        let objects = self.objects.read().await;
        let tree_data = objects.get(&commit.tree_id).ok_or_else(|| {
            anyhow::anyhow!("Tree {} not found", commit.tree_id)
        })?;
        let tree: Tree = bincode::deserialize(tree_data)?;

        // Navigate to file
        let path_parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let mut current_tree = tree;

        for (i, part) in path_parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }

            if let Some(entry) = current_tree.get(*part) {
                if i == path_parts.len() - 1 {
                    // This is the file - deserialize blob and return its data
                    let blob_data = objects.get(&entry.id).ok_or_else(|| {
                        anyhow::anyhow!("Blob {} not found", entry.id)
                    })?;
                    let blob: Blob = Blob::deserialize(blob_data)?;
                    return Ok(Bytes::from(blob.data));
                } else {
                    // This is a directory, traverse deeper
                    let tree_data = objects.get(&entry.id).ok_or_else(|| {
                        anyhow::anyhow!("Tree {} not found", entry.id)
                    })?;
                    current_tree = bincode::deserialize(tree_data)?;
                }
            } else {
                return Err(anyhow::anyhow!("Path not found: {}", path));
            }
        }

        Err(anyhow::anyhow!("Path not found: {}", path))
    }

    /// List directory contents
    pub async fn list_dir(&self, path: &str, rev: u64) -> Result<Vec<String>> {
        // Get commit for revision
        let commits = self.commits.read().await;
        let commit = commits
            .get(&rev)
            .ok_or_else(|| anyhow::anyhow!("Revision {} not found", rev))?;

        // Get tree
        let objects = self.objects.read().await;
        let tree_data = objects.get(&commit.tree_id).ok_or_else(|| {
            anyhow::anyhow!("Tree {} not found", commit.tree_id)
        })?;
        let mut current_tree: Tree = bincode::deserialize(tree_data)?;

        // Navigate to directory
        let path_parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

        for part in &path_parts {
            if part.is_empty() {
                continue;
            }

            if let Some(entry) = current_tree.get(*part) {
                let tree_data = objects.get(&entry.id).ok_or_else(|| {
                    anyhow::anyhow!("Tree {} not found", entry.id)
                })?;
                current_tree = bincode::deserialize(tree_data)?;
            } else {
                return Err(anyhow::anyhow!("Directory not found: {}", path));
            }
        }

        // List entries
        let entries: Vec<String> = current_tree.iter().map(|e| e.name.clone()).collect();
        Ok(entries)
    }

    /// Add or update a file
    pub async fn add_file(&self, path: &str, content: Vec<u8>, executable: bool) -> Result<ObjectId> {
        // Create blob
        let blob = Blob::new(content, executable);
        let blob_id = blob.id();
        let blob_data = blob.to_bytes()?;

        // Store blob
        let mut objects = self.objects.write().await;
        objects.insert(blob_id, Bytes::from(blob_data));
        drop(objects);

        // Update root tree with the new file entry
        let mut root_tree = self.root_tree.write().await;
        let filename = path.trim_start_matches('/');
        let entry = TreeEntry::new(
            filename.to_string(),
            blob_id,
            ObjectKind::Blob,
            if executable { 0o755 } else { 0o644 },
        );
        root_tree.insert(entry);
        drop(root_tree);

        // Update path index
        let mut path_index = self.path_index.write().await;
        path_index.insert(path.to_string(), blob_id);

        Ok(blob_id)
    }

    /// Create a directory
    pub async fn mkdir(&self, _path: &str) -> Result<ObjectId> {
        let tree = Tree::new();
        let tree_id = tree.id();
        let tree_data = tree.to_bytes()?;

        let mut objects = self.objects.write().await;
        objects.insert(tree_id, Bytes::from(tree_data));

        Ok(tree_id)
    }

    /// Create a new commit
    pub async fn commit(
        &self,
        author: String,
        message: String,
        timestamp: i64,
    ) -> Result<u64> {
        // Get current root tree
        let root_tree = self.root_tree.read().await;
        let tree_id = root_tree.id();
        let tree_data = root_tree.to_bytes()?;

        // Store tree if not exists
        let mut objects = self.objects.write().await;
        if !objects.contains_key(&tree_id) {
            objects.insert(tree_id, Bytes::from(tree_data));
        }
        drop(objects);

        // Get parent commit
        let current_rev = *self.current_rev.read().await;
        let parents = if current_rev > 0 {
            let commits = self.commits.read().await;
            if let Some(parent) = commits.get(&current_rev) {
                vec![parent.id()]
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Create commit
        let commit = Commit::new(
            tree_id,
            parents,
            author.clone(),
            message.clone(),
            timestamp,
            0,
        );
        let commit_id = commit.id();
        let commit_data = commit.to_bytes()?;

        // Store commit
        let new_rev = current_rev + 1;

        let mut objects = self.objects.write().await;
        objects.insert(commit_id, Bytes::from(commit_data));

        let mut commits = self.commits.write().await;
        commits.insert(new_rev, commit.clone());

        let mut current_rev = self.current_rev.write().await;
        *current_rev = new_rev;

        Ok(new_rev)
    }

    /// Get commit log
    pub async fn log(&self, start_rev: u64, limit: usize) -> Result<Vec<Commit>> {
        let commits = self.commits.read().await;
        let mut result = Vec::new();

        let current = *self.current_rev.read().await;
        let end = std::cmp::min(start_rev, current);

        for rev in (0..=end).rev() {
            if let Some(commit) = commits.get(&rev) {
                result.push(commit.clone());
                if result.len() >= limit {
                    break;
                }
            }
        }

        Ok(result)
    }

    /// Check if path exists
    pub async fn exists(&self, path: &str, rev: u64) -> Result<bool> {
        match self.get_file(path, rev).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Initialize repository with initial commit
    pub async fn initialize(&self) -> Result<()> {
        // Create initial empty tree
        let tree = Tree::new();
        let tree_id = tree.id();
        let tree_data = tree.to_bytes()?;

        let mut objects = self.objects.write().await;
        objects.insert(tree_id, Bytes::from(tree_data));
        drop(objects);

        // Create initial commit (revision 0)
        let commit = Commit::new(
            tree_id,
            vec![],
            "system".to_string(),
            "Initial commit".to_string(),
            chrono::Utc::now().timestamp(),
            0,
        );
        let commit_id = commit.id();
        let commit_data = commit.to_bytes()?;

        let mut objects = self.objects.write().await;
        objects.insert(commit_id, Bytes::from(commit_data));

        let mut commits = self.commits.write().await;
        commits.insert(0, commit);

        Ok(())
    }
}

impl Default for Repository {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_repository_create() {
        let repo = Repository::new();
        assert!(repo.initialize().await.is_ok());
        assert_eq!(repo.current_rev().await, 0);
    }

    #[tokio::test]
    async fn test_add_file() {
        let repo = Repository::new();
        repo.initialize().await.unwrap();

        let content = b"Hello, World!".to_vec();
        let id = repo.add_file("/test.txt", content.clone(), false).await.unwrap();
        assert_ne!(id.to_hex().len(), 0);

        // Add file and commit
        repo.commit("test".to_string(), "Add test file".to_string(), 0)
            .await
            .unwrap();
        assert_eq!(repo.current_rev().await, 1);
    }

    #[tokio::test]
    async fn test_get_file() {
        let repo = Repository::new();
        repo.initialize().await.unwrap();

        let content = b"Hello, World!".to_vec();
        repo.add_file("/test.txt", content.clone(), false).await.unwrap();
        repo.commit("test".to_string(), "Add test file".to_string(), 0)
            .await
            .unwrap();

        let retrieved = repo.get_file("/test.txt", 1).await.unwrap();
        assert_eq!(retrieved.to_vec(), content);
    }

    #[tokio::test]
    async fn test_log() {
        let repo = Repository::new();
        repo.initialize().await.unwrap();

        repo.commit("user1".to_string(), "Commit 1".to_string(), 0)
            .await
            .unwrap();
        repo.commit("user2".to_string(), "Commit 2".to_string(), 0)
            .await
            .unwrap();

        let log = repo.log(10, 100).await.unwrap();
        assert_eq!(log.len(), 3); // Initial + 2 commits
    }
}

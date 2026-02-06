//! Persistent repository using Fjall LSM-tree
//!
//! This provides real persistent storage that survives restarts

use crate::object::{Blob, Commit, ObjectId, Tree};
use anyhow::{anyhow, Result};
use bytes::Bytes;
use fjall::{Config, Keyspace, PersistMode, Transaction};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Persistent repository using Fjall LSM-tree
pub struct PersistentRepository {
    /// Fjall keyspace
    keyspace: Arc<Keyspace>,
    
    /// Objects tree
    objects: Arc<fjall::Tree>,
    
    /// Commits tree
    commits: Arc<fjall::Tree>,
    
    /// Path index tree
    path_index: Arc<fjall::Tree>,
    
    /// Metadata (cached in memory for fast access)
    metadata: Arc<RwLock<RepositoryMetadata>>,
}

/// Repository metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryMetadata {
    pub uuid: String,
    pub current_rev: u64,
    pub created_at: i64,
}

impl PersistentRepository {
    /// Open or create repository at path
    pub async fn open(path: &Path) -> Result<Self> {
        // Configure Fjall
        let config = Config::new(path).persist_mode(PersistMode::Immediate);
        let keyspace = Keyspace::open(config)?;
        
        // Open trees
        let objects = keyspace.open_tree("objects")?;
        let commits = keyspace.open_tree("commits")?;
        let path_index = keyspace.open_tree("path_index")?;
        
        // Load or create metadata
        let metadata_key = b"__metadata__";
        let metadata = if let Some(bytes) = commits.get(metadata_key)? {
            let meta: RepositoryMetadata = bincode::deserialize(&bytes)?;
            Arc::new(RwLock::new(meta))
        } else {
            let meta = RepositoryMetadata {
                uuid: uuid::Uuid::new_v4().to_string(),
                current_rev: 0,
                created_at: chrono::Utc::now().timestamp(),
            };
            
            // Save initial metadata
            let bytes = bincode::serialize(&meta)?;
            commits.insert(metadata_key, bytes)?;
            
            Arc::new(RwLock::new(meta))
        };
        
        Ok(Self {
            keyspace: Arc::new(keyspace),
            objects: Arc::new(objects),
            commits: Arc::new(commits),
            path_index: Arc::new(path_index),
            metadata,
        })
    }
    
    /// Get current revision
    pub async fn current_rev(&self) -> u64 {
        self.metadata.read().await.current_rev
    }
    
    /// Get UUID
    pub async fn uuid(&self) -> String {
        self.metadata.read().await.uuid.clone()
    }
    
    /// Initialize repository
    pub async fn initialize(&self) -> Result<()> {
        // Check if already initialized
        if self.commits.get(b"0")?.is_some() {
            return Ok(());
        }
        
        let tree = Tree::new();
        let tree_id = tree.id();
        let tree_data = tree.to_bytes()?;
        
        // Store tree
        self.objects.insert(tree_id.to_hex().as_bytes(), tree_data)?;
        
        // Create initial commit
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
        let bytes = bincode::serialize(&*meta)?;
        self.commits.insert(b"__metadata__", bytes)?;
        
        Ok(())
    }
    
    /// Add file to repository
    pub async fn add_file(&self, path: &str, content: Vec<u8>, _executable: bool) -> Result<ObjectId> {
        let blob = Blob::new(content, false);
        let blob_id = blob.id();
        let blob_data = blob.to_bytes()?;
        
        // Store blob in objects tree
        self.objects.insert(blob_id.to_hex().as_bytes(), blob_data)?;
        
        // Update path index
        self.path_index.insert(path.as_bytes(), blob_id.to_hex().as_bytes())?;
        
        Ok(blob_id)
    }
    
    /// Get file content
    pub async fn get_file(&self, path: &str, rev: u64) -> Result<Bytes> {
        // Get commit for revision
        let rev_key = rev.to_string();
        let commit_data = self.commits.get(rev_key.as_bytes())?
            .ok_or_else(|| anyhow!("Revision {} not found", rev))?;
        
        let commit: Commit = bincode::deserialize(&commit_data)?;
        
        // Get root tree
        let tree_data = self.objects.get(commit.tree_id.to_hex().as_bytes())?
            .ok_or_else(|| anyhow!("Tree {} not found", commit.tree_id))?;
        
        let mut current_tree: Tree = bincode::deserialize(&tree_data)?;
        
        // Navigate path
        let path_parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        
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
        
        Err(anyhow!("Path not found"))
    }
    
    /// Create commit
    pub async fn commit(&self, author: String, message: String, timestamp: i64) -> Result<u64> {
        let tree = Tree::new();
        let tree_id = tree.id();
        let tree_data = tree.to_bytes()?;
        
        // Store tree
        self.objects.insert(tree_id.to_hex().as_bytes(), tree_data)?;
        
        // Get parent commit
        let current_rev = self.current_rev().await;
        let parents = if current_rev > 0 {
            let rev_key = current_rev.to_string();
            if let Some(parent_data) = self.commits.get(rev_key.as_bytes())? {
                let parent: Commit = bincode::deserialize(&parent_data)?;
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
            author,
            message,
            timestamp,
            0,
        );
        
        let new_rev = current_rev + 1;
        let commit_data = commit.to_bytes()?;
        self.commits.insert(new_rev.to_string().as_bytes(), commit_data)?;
        
        // Update metadata
        let mut meta = self.metadata.write().await;
        meta.current_rev = new_rev;
        let bytes = bincode::serialize(&*meta)?;
        self.commits.insert(b"__metadata__", bytes)?;
        
        Ok(new_rev)
    }
    
    /// Get commit log
    pub async fn log(&self, start_rev: u64, limit: usize) -> Result<Vec<Commit>> {
        let mut result = Vec::new();
        
        // Scan commits from start_rev downwards
        for rev in (0..=start_rev).rev() {
            if result.len() >= limit {
                break;
            }
            
            let rev_key = rev.to_string();
            if let Some(commit_data) = self.commits.get(rev_key.as_bytes())? {
                let commit: Commit = bincode::deserialize(&commit_data)?;
                result.push(commit);
            }
        }
        
        Ok(result)
    }
}

//! Persistent repository using file-based storage
//!
//! MVP: Uses simple file-based persistence before Fjall integration

use crate::object::{Blob, Commit, ObjectId, Tree};
use anyhow::{anyhow, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Persistent repository
pub struct PersistentRepository {
    /// Repository path
    path: PathBuf,
    
    /// In-memory cache
    objects: Arc<RwLock<HashMap<ObjectId, Vec<u8>>>>,
    commits: Arc<RwLock<HashMap<u64, Commit>>>,
    path_index: Arc<RwLock<HashMap<String, ObjectId>>>,
    
    /// Metadata
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
    /// Open or create repository
    pub async fn open(path: &Path) -> Result<Self> {
        let path = path.to_path_buf();
        
        // Create directory if not exists
        fs::create_dir_all(&path)?;
        
        // Load or create metadata
        let metadata_path = path.join("metadata.json");
        let metadata = if metadata_path.exists() {
            let file = File::open(&metadata_path)?;
            let reader = BufReader::new(file);
            let meta: RepositoryMetadata = serde_json::from_reader(reader)?;
            Arc::new(RwLock::new(meta))
        } else {
            let meta = RepositoryMetadata {
                uuid: uuid::Uuid::new_v4().to_string(),
                current_rev: 0,
                created_at: chrono::Utc::now().timestamp(),
            };
            
            // Save metadata
            let file = File::create(&metadata_path)?;
            let mut writer = BufWriter::new(file);
            serde_json::to_writer_pretty(&mut writer, &meta)?;
            writer.flush()?;
            
            Arc::new(RwLock::new(meta))
        };
        
        Ok(Self {
            path,
            objects: Arc::new(RwLock::new(HashMap::new())),
            commits: Arc::new(RwLock::new(HashMap::new())),
            path_index: Arc::new(RwLock::new(HashMap::new())),
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
        let commits_file = self.path.join("commits.json");
        if commits_file.exists() {
            // Load existing data
            self.load_from_disk().await?;
            return Ok(());
        }
        
        let tree = Tree::new();
        let tree_id = tree.id();
        let tree_data = tree.to_bytes()?;
        
        // Store in memory
        self.objects.write().await.insert(tree_id, tree_data);
        
        // Create initial commit
        let commit = Commit::new(
            tree_id,
            vec![],
            "system".to_string(),
            "Initial commit".to_string(),
            chrono::Utc::now().timestamp(),
            0,
        );
        
        self.commits.write().await.insert(0, commit);
        
        // Update metadata
        let mut meta = self.metadata.write().await;
        meta.current_rev = 0;
        
        // Save to disk
        self.save_to_disk().await?;
        
        Ok(())
    }
    
    /// Add file
    pub async fn add_file(&self, path: &str, content: Vec<u8>, _executable: bool) -> Result<ObjectId> {
        let blob = Blob::new(content, false);
        let blob_id = blob.id();
        let blob_data = blob.to_bytes()?;
        
        self.objects.write().await.insert(blob_id, blob_data.clone());
        self.path_index.write().await.insert(path.to_string(), blob_id);
        
        // Persist immediately
        self.save_to_disk().await?;
        
        Ok(blob_id)
    }
    
    /// Get file
    pub async fn get_file(&self, path: &str, rev: u64) -> Result<Bytes> {
        // Ensure data is loaded
        if self.commits.read().await.is_empty() {
            self.load_from_disk().await?;
        }
        
        let commits = self.commits.read().await;
        let commit = commits.get(&rev)
            .ok_or_else(|| anyhow!("Revision {} not found", rev))?;
        
        let objects = self.objects.read().await;
        let tree_data = objects.get(&commit.tree_id)
            .ok_or_else(|| anyhow!("Tree not found"))?;
        
        let mut current_tree: Tree = bincode::deserialize(tree_data)?;
        let path_parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        
        for (i, part) in path_parts.iter().enumerate() {
            if part.is_empty() { continue; }
            
            if let Some(entry) = current_tree.get(*part) {
                if i == path_parts.len() - 1 {
                    let blob_data = objects.get(&entry.id)
                        .ok_or_else(|| anyhow!("Blob not found"))?;
                    let blob: Blob = bincode::deserialize(blob_data)?;
                    return Ok(Bytes::from(blob.data));
                } else {
                    let tree_data = objects.get(&entry.id)
                        .ok_or_else(|| anyhow!("Tree not found"))?;
                    current_tree = bincode::deserialize(tree_data)?;
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
        
        self.objects.write().await.insert(tree_id, tree_data);
        
        let current_rev = self.current_rev().await;
        let parents = if current_rev > 0 {
            let commits = self.commits.read().await;
            commits.get(&current_rev).map(|c| vec![c.id()]).unwrap_or_default()
        } else {
            vec![]
        };
        
        let commit = Commit::new(
            tree_id,
            parents,
            author,
            message,
            timestamp,
            0,
        );
        
        let new_rev = current_rev + 1;
        self.commits.write().await.insert(new_rev, commit);
        
        // Update metadata
        let mut meta = self.metadata.write().await;
        meta.current_rev = new_rev;
        
        // Save to disk
        self.save_to_disk().await?;
        
        Ok(new_rev)
    }
    
    /// Get log
    pub async fn log(&self, start_rev: u64, limit: usize) -> Result<Vec<Commit>> {
        if self.commits.read().await.is_empty() {
            self.load_from_disk().await?;
        }
        
        let commits = self.commits.read().await;
        let mut result = Vec::new();
        
        for rev in (0..=start_rev).rev() {
            if result.len() >= limit {
                break;
            }
            if let Some(commit) = commits.get(&rev) {
                result.push(commit.clone());
            }
        }
        
        Ok(result)
    }
    
    /// Save to disk
    async fn save_to_disk(&self) -> Result<()> {
        // Save metadata
        let metadata_path = self.path.join("metadata.json");
        let meta = self.metadata.read().await;
        let file = File::create(&metadata_path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &*meta)?;
        writer.flush()?;
        
        // Save commits
        let commits_path = self.path.join("commits.json");
        let commits = self.commits.read().await;
        let commits_map: HashMap<u64, &Commit> = commits.iter().map(|(k, v)| (*k, v)).collect();
        let file = File::create(&commits_path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &commits_map)?;
        writer.flush()?;
        
        // Save objects
        let objects_path = self.path.join("objects.json");
        let objects = self.objects.read().await;
        let objects_map: HashMap<String, &[u8]> = objects.iter()
            .map(|(k, v)| (k.to_hex(), v.as_slice()))
            .collect();
        let file = File::create(&objects_path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &objects_map)?;
        writer.flush()?;
        
        Ok(())
    }
    
    /// Load from disk
    async fn load_from_disk(&self) -> Result<()> {
        // Load commits
        let commits_path = self.path.join("commits.json");
        if commits_path.exists() {
            let file = File::open(&commits_path)?;
            let reader = BufReader::new(file);
            let commits_map: HashMap<u64, Commit> = serde_json::from_reader(reader)?;
            
            let mut commits = self.commits.write().await;
            commits.clear();
            for (rev, commit) in commits_map {
                commits.insert(rev, commit);
            }
        }
        
        // Load objects
        let objects_path = self.path.join("objects.json");
        if objects_path.exists() {
            let file = File::open(&objects_path)?;
            let reader = BufReader::new(file);
            let objects_map: HashMap<String, Vec<u8>> = serde_json::from_reader(reader)?;
            
            let mut objects = self.objects.write().await;
            objects.clear();
            for (id_hex, data) in objects_map {
                let id = ObjectId::from_hex(&id_hex)?;
                objects.insert(id, data);
            }
        }
        
        Ok(())
    }
}

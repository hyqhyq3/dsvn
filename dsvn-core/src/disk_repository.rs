//! Disk-persistent repository implementation
//!
//! Stores objects on disk using content-addressed filesystem (like git objects).
//! The working tree (file path → entry mapping) is stored in a sled embedded
//! database for O(log n) inserts/lookups instead of O(n) HashMap serialization.
//! Designed to handle 10GB data / 100,000 commits without OOM.

use crate::object::{Blob, Commit, ObjectId, ObjectKind, Tree, TreeEntry};
use crate::properties::PropertySet;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Compact record stored as sled value for each tree entry.
/// This is the on-disk format; we convert to/from TreeEntry for the public API.
#[derive(Serialize, Deserialize)]
struct TreeEntryRecord {
    object_id: ObjectId,
    kind: ObjectKind,
    mode: u32,
}

impl TreeEntryRecord {
    fn from_tree_entry(entry: &TreeEntry) -> Self {
        Self {
            object_id: entry.id,
            kind: entry.kind,
            mode: entry.mode,
        }
    }

    fn to_tree_entry(&self, name: String) -> TreeEntry {
        TreeEntry::new(name, self.object_id, self.kind, self.mode)
    }

    fn serialize(&self) -> Result<Vec<u8>> {
        bincode::serialize(self).map_err(|e| anyhow!("Failed to serialize TreeEntryRecord: {}", e))
    }

    fn deserialize(data: &[u8]) -> Result<Self> {
        bincode::deserialize(data)
            .map_err(|e| anyhow!("Failed to deserialize TreeEntryRecord: {}", e))
    }
}

/// Disk-persistent repository
///
/// Layout on disk:
/// ```text
/// {root}/
///   uuid                    — repository UUID
///   refs/head               — current revision number (text)
///   objects/{hash[0..2]}/{hash[2..]}  — content-addressed object store
///   commits/{rev}.bin       — commit objects (bincode)
///   trees/{rev}.bin         — tree snapshots per revision (bincode)
///   props/{sha256(path)}.json — property sets keyed by path hash
///   tree.db/                — sled database for current working tree
/// ```
pub struct DiskRepository {
    root: PathBuf,
    uuid: String,
    current_rev: Arc<RwLock<u64>>,
    /// The in-progress (staged) root tree — stored in sled for O(log n) operations
    tree_db: sled::Db,
    /// Property store (in-memory, persisted on commit)
    property_store: Arc<DiskPropertyStore>,
    /// Batch mode flag: when true, skip per-operation flush
    batch_mode: std::sync::atomic::AtomicBool,
}

/// Disk-backed property store
pub struct DiskPropertyStore {
    root: PathBuf,
    cache: RwLock<HashMap<String, PropertySet>>,
}

impl DiskPropertyStore {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            cache: RwLock::new(HashMap::new()),
        }
    }

    fn props_dir(&self) -> PathBuf {
        self.root.join("props")
    }

    fn path_hash(path: &str) -> String {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(path.as_bytes());
        hex::encode(hash)
    }

    fn prop_file(&self, path: &str) -> PathBuf {
        let hash = Self::path_hash(path);
        self.props_dir().join(format!("{}.json", hash))
    }

    pub async fn get(&self, path: &str) -> PropertySet {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(ps) = cache.get(path) {
                return ps.clone();
            }
        }
        // Load from disk
        let file_path = self.prop_file(path);
        if file_path.exists() {
            if let Ok(data) = fs::read_to_string(&file_path) {
                if let Ok(ps) = serde_json::from_str::<PropertySet>(&data) {
                    let mut cache = self.cache.write().await;
                    cache.insert(path.to_string(), ps.clone());
                    return ps;
                }
            }
        }
        PropertySet::new()
    }

    pub async fn set(&self, path: String, name: String, value: String) -> Result<()> {
        let mut cache = self.cache.write().await;
        let ps = cache.entry(path.clone()).or_insert_with(PropertySet::new);
        ps.set(name, value);
        // Persist
        let file_path = self.prop_file(&path);
        fs::create_dir_all(file_path.parent().unwrap())?;
        let data = serde_json::to_string(ps)?;
        fs::write(&file_path, data)?;
        Ok(())
    }

    pub async fn remove(&self, path: &str, name: &str) -> Result<Option<String>> {
        let mut cache = self.cache.write().await;
        if let Some(ps) = cache.get_mut(path) {
            let val = ps.remove(name);
            // Persist
            let file_path = self.prop_file(path);
            fs::create_dir_all(file_path.parent().unwrap())?;
            let data = serde_json::to_string(ps)?;
            fs::write(&file_path, data)?;
            Ok(val)
        } else {
            Ok(None)
        }
    }

    pub async fn list(&self, path: &str) -> Vec<String> {
        self.get(path).await.list()
    }

    pub async fn contains(&self, path: &str, name: &str) -> bool {
        self.get(path).await.contains(name)
    }
}

impl DiskRepository {
    /// Open or create a repository at the given path
    pub fn open(path: &Path) -> Result<Self> {
        let root = path.to_path_buf();

        // Ensure directories exist
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("commits"))?;
        fs::create_dir_all(root.join("trees"))?;
        fs::create_dir_all(root.join("props"))?;
        fs::create_dir_all(root.join("refs"))?;

        // UUID
        let uuid_path = root.join("uuid");
        let uuid = if uuid_path.exists() {
            fs::read_to_string(&uuid_path)?.trim().to_string()
        } else {
            let u = uuid::Uuid::new_v4().to_string();
            fs::write(&uuid_path, &u)?;
            u
        };

        // Current revision
        let head_path = root.join("refs").join("head");
        let current_rev = if head_path.exists() {
            fs::read_to_string(&head_path)?
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
        } else {
            // Not yet initialized — will be set by initialize()
            0
        };

        // Open sled database for the working tree
        let tree_db = sled::open(root.join("tree.db"))
            .with_context(|| format!("Failed to open sled database at {:?}", root.join("tree.db")))?;

        // Migration: if old root_tree.bin exists and sled is empty, migrate data
        let root_tree_path = root.join("root_tree.bin");
        if root_tree_path.exists() && tree_db.is_empty() {
            if let Ok(data) = fs::read(&root_tree_path) {
                if let Ok(old_tree) = bincode::deserialize::<Tree>(&data) {
                    for entry in old_tree.iter() {
                        let record = TreeEntryRecord::from_tree_entry(entry);
                        if let Ok(serialized) = record.serialize() {
                            let _ = tree_db.insert(entry.name.as_bytes(), serialized);
                        }
                    }
                    let _ = tree_db.flush();
                    // Remove old file after successful migration
                    let _ = fs::remove_file(&root_tree_path);
                    tracing::info!("Migrated root_tree.bin to sled database ({} entries)", old_tree.entries.len());
                }
            }
        }

        let property_store = Arc::new(DiskPropertyStore::new(root.clone()));

        Ok(Self {
            root,
            uuid,
            current_rev: Arc::new(RwLock::new(current_rev)),
            tree_db,
            property_store,
            batch_mode: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Get repository UUID
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// Get current revision
    pub async fn current_rev(&self) -> u64 {
        *self.current_rev.read().await
    }

    /// Get the property store
    pub fn property_store(&self) -> &Arc<DiskPropertyStore> {
        &self.property_store
    }

    // ==================== Sled Tree Helpers ====================

    /// Insert an entry into the sled tree database
    fn tree_insert(&self, path: &str, entry: &TreeEntryRecord) -> Result<()> {
        let serialized = entry.serialize()?;
        self.tree_db
            .insert(path.as_bytes(), serialized)
            .with_context(|| format!("Failed to insert tree entry: {}", path))?;
        Ok(())
    }

    /// Get an entry from the sled tree database
    fn tree_get(&self, path: &str) -> Result<Option<TreeEntry>> {
        match self.tree_db.get(path.as_bytes())? {
            Some(value) => {
                let record = TreeEntryRecord::deserialize(&value)?;
                Ok(Some(record.to_tree_entry(path.to_string())))
            }
            None => Ok(None),
        }
    }

    /// Remove an entry from the sled tree database
    fn tree_remove(&self, path: &str) -> Result<()> {
        self.tree_db.remove(path.as_bytes())?;
        Ok(())
    }

    /// Build a full Tree object from sled (needed for commit snapshots and tree hashing)
    fn build_tree_from_sled(&self) -> Result<Tree> {
        let mut tree = Tree::new();
        for item in self.tree_db.iter() {
            let (key, value) = item.map_err(|e| anyhow!("sled iteration error: {}", e))?;
            let path = String::from_utf8(key.to_vec())
                .map_err(|e| anyhow!("Invalid UTF-8 in tree key: {}", e))?;
            let record = TreeEntryRecord::deserialize(&value)?;
            tree.insert(record.to_tree_entry(path));
        }
        Ok(tree)
    }

    /// Populate sled from a Tree object (used when loading tree snapshots)
    fn populate_sled_from_tree(&self, tree: &Tree) -> Result<()> {
        // Clear existing entries
        self.tree_db.clear()?;
        // Insert all entries from the tree
        for entry in tree.iter() {
            let record = TreeEntryRecord::from_tree_entry(entry);
            self.tree_insert(&entry.name, &record)?;
        }
        Ok(())
    }

    // ==================== Object Store ====================

    fn object_path(&self, id: &ObjectId) -> PathBuf {
        let hex = id.to_hex();
        self.root
            .join("objects")
            .join(&hex[..2])
            .join(&hex[2..])
    }

    fn store_object(&self, id: &ObjectId, data: &[u8]) -> Result<()> {
        let path = self.object_path(id);
        if path.exists() {
            return Ok(()); // Already stored (content-addressed = idempotent)
        }
        fs::create_dir_all(path.parent().unwrap())?;
        // Write atomically via temp file
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, data)?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    fn load_object(&self, id: &ObjectId) -> Result<Vec<u8>> {
        let path = self.object_path(id);
        fs::read(&path).with_context(|| format!("Object {} not found at {:?}", id, path))
    }

    // ==================== Commit Store ====================

    fn commit_path(&self, rev: u64) -> PathBuf {
        self.root.join("commits").join(format!("{}.bin", rev))
    }

    fn store_commit(&self, rev: u64, commit: &Commit) -> Result<()> {
        let path = self.commit_path(rev);
        let data = bincode::serialize(commit)?;
        fs::write(&path, &data)?;
        Ok(())
    }

    fn load_commit(&self, rev: u64) -> Result<Commit> {
        let path = self.commit_path(rev);
        let data = fs::read(&path).with_context(|| format!("Commit r{} not found", rev))?;
        Ok(bincode::deserialize(&data)?)
    }

    // ==================== Tree Store ====================

    fn tree_snapshot_path(&self, rev: u64) -> PathBuf {
        self.root.join("trees").join(format!("{}.bin", rev))
    }

    fn store_tree_snapshot(&self, rev: u64, tree: &Tree) -> Result<()> {
        let path = self.tree_snapshot_path(rev);
        let data = bincode::serialize(tree)?;
        fs::write(&path, &data)?;
        Ok(())
    }

    fn load_tree_snapshot(&self, rev: u64) -> Result<Tree> {
        let path = self.tree_snapshot_path(rev);
        let data =
            fs::read(&path).with_context(|| format!("Tree snapshot for r{} not found", rev))?;
        Ok(bincode::deserialize(&data)?)
    }

    // ==================== Head Ref ====================

    fn save_head(&self, rev: u64) -> Result<()> {
        let path = self.root.join("refs").join("head");
        fs::write(&path, rev.to_string())?;
        Ok(())
    }

    // ==================== Public API (matching in-memory Repository) ====================

    /// Initialize the repository with an empty root commit (revision 0)
    pub async fn initialize(&self) -> Result<()> {
        // Check if already initialized
        let head_path = self.root.join("refs").join("head");
        if head_path.exists() {
            // Already initialized — reload current_rev
            let rev = fs::read_to_string(&head_path)?
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
            let mut cr = self.current_rev.write().await;
            *cr = rev;

            // Load root tree from latest revision into sled (if sled is empty)
            if self.tree_db.is_empty() {
                if let Ok(tree) = self.load_tree_snapshot(rev) {
                    self.populate_sled_from_tree(&tree)?;
                }
            }
            return Ok(());
        }

        // Create empty tree
        let tree = Tree::new();
        let tree_id = tree.id();
        let tree_data = tree.to_bytes()?;
        self.store_object(&tree_id, &tree_data)?;

        // Create initial commit
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
        self.store_object(&commit_id, &commit_data)?;
        self.store_commit(0, &commit)?;
        self.store_tree_snapshot(0, &tree)?;
        self.save_head(0)?;

        // sled is already empty, which represents an empty tree
        self.tree_db.clear()?;

        Ok(())
    }

    /// Get file content by path at a given revision
    pub async fn get_file(&self, path: &str, rev: u64) -> Result<Bytes> {
        let commit = self.load_commit(rev)?;
        let tree_data = self.load_object(&commit.tree_id)?;
        let tree: Tree = bincode::deserialize(&tree_data)?;

        let full_path = path.trim_start_matches('/');

        // First try: flat path lookup (MVP storage model)
        if let Some(entry) = tree.get(full_path) {
            if entry.kind == ObjectKind::Blob {
                let blob_data = self.load_object(&entry.id)?;
                let blob: Blob = Blob::deserialize(&blob_data)?;
                return Ok(Bytes::from(blob.data));
            }
        }

        // Second try: hierarchical navigation
        let path_parts: Vec<&str> = full_path.split('/').filter(|p| !p.is_empty()).collect();
        let mut current_tree = tree;

        for (i, part) in path_parts.iter().enumerate() {
            if let Some(entry) = current_tree.get(*part) {
                if i == path_parts.len() - 1 {
                    let blob_data = self.load_object(&entry.id)?;
                    let blob: Blob = Blob::deserialize(&blob_data)?;
                    return Ok(Bytes::from(blob.data));
                } else {
                    let tree_data = self.load_object(&entry.id)?;
                    current_tree = bincode::deserialize(&tree_data)?;
                }
            } else {
                return Err(anyhow!("Path not found: {}", path));
            }
        }

        Err(anyhow!("Path not found: {}", path))
    }

    /// List directory contents at a given revision
    pub async fn list_dir(&self, path: &str, rev: u64) -> Result<Vec<String>> {
        let commit = self.load_commit(rev)?;
        let tree_data = self.load_object(&commit.tree_id)?;
        let mut current_tree: Tree = bincode::deserialize(&tree_data)?;

        let path_parts: Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|p| !p.is_empty())
            .collect();

        for part in &path_parts {
            if let Some(entry) = current_tree.get(*part) {
                let td = self.load_object(&entry.id)?;
                current_tree = bincode::deserialize(&td)?;
            } else {
                return Err(anyhow!("Directory not found: {}", path));
            }
        }

        Ok(current_tree.iter().map(|e| e.name.clone()).collect())
    }

    /// Add or update a file in the working tree (staged, not yet committed)
    pub async fn add_file(
        &self,
        path: &str,
        content: Vec<u8>,
        executable: bool,
    ) -> Result<ObjectId> {
        // Create and store blob on disk
        let blob = Blob::new(content, executable);
        let blob_id = blob.id();
        let blob_data = blob.to_bytes()?;
        self.store_object(&blob_id, &blob_data)?;

        // Update sled working tree — O(log n)
        let full_path = path.trim_start_matches('/');
        let record = TreeEntryRecord {
            object_id: blob_id,
            kind: ObjectKind::Blob,
            mode: if executable { 0o755 } else { 0o644 },
        };
        self.tree_insert(full_path, &record)?;

        Ok(blob_id)
    }

    /// Create a directory in the working tree
    pub async fn mkdir(&self, path: &str) -> Result<ObjectId> {
        let new_tree = Tree::new();
        let tree_id = new_tree.id();
        let tree_data = new_tree.to_bytes()?;
        self.store_object(&tree_id, &tree_data)?;

        let full_path = path.trim_start_matches('/');
        let record = TreeEntryRecord {
            object_id: tree_id,
            kind: ObjectKind::Tree,
            mode: 0o755,
        };
        self.tree_insert(full_path, &record)?;

        Ok(tree_id)
    }

    /// Delete a file from the working tree
    pub async fn delete_file(&self, path: &str) -> Result<()> {
        let filename = path.trim_start_matches('/');
        self.tree_remove(filename)?;
        Ok(())
    }

    /// Create a new commit from the current working tree
    pub async fn commit(
        &self,
        author: String,
        message: String,
        timestamp: i64,
    ) -> Result<u64> {
        // Build Tree from sled and serialize
        let root_tree = self.build_tree_from_sled()?;
        let tree_id = root_tree.id();
        let tree_data = root_tree.to_bytes()?;
        self.store_object(&tree_id, &tree_data)?;

        let current_rev = *self.current_rev.read().await;

        // Get parent commit id
        let parents = if current_rev > 0 || self.commit_path(current_rev).exists() {
            if let Ok(parent) = self.load_commit(current_rev) {
                vec![parent.id()]
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Create commit
        let commit = Commit::new(tree_id, parents, author, message, timestamp, 0);
        let commit_id = commit.id();
        let commit_data = commit.to_bytes()?;

        let new_rev = current_rev + 1;

        // Store on disk
        self.store_object(&commit_id, &commit_data)?;
        self.store_commit(new_rev, &commit)?;
        self.store_tree_snapshot(new_rev, &root_tree)?;
        self.save_head(new_rev)?;

        // Update in-memory rev
        let mut cr = self.current_rev.write().await;
        *cr = new_rev;

        Ok(new_rev)
    }

    // ==================== Batch Import API ====================
    //
    // Synchronous methods optimized for high-throughput bulk import.
    // These use sled's O(log n) insert directly. In batch mode, sled flush
    // is deferred to end_batch() for maximum throughput.

    /// Enter batch mode: suppress per-operation sled flush.
    /// Must call end_batch() when done to ensure persistence.
    pub fn begin_batch(&self) {
        self.batch_mode.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Exit batch mode: flush sled to ensure all data is persisted.
    pub fn end_batch(&self) {
        self.batch_mode.store(false, std::sync::atomic::Ordering::Relaxed);
        // Flush sled to disk
        let _ = self.tree_db.flush();
    }

    /// Synchronous add_file for batch import.
    /// O(log n) insert into sled — no full-tree serialization needed.
    pub fn add_file_sync(
        &self,
        path: &str,
        content: Vec<u8>,
        executable: bool,
    ) -> Result<ObjectId> {
        let blob = Blob::new(content, executable);
        let blob_id = blob.id();
        let blob_data = blob.to_bytes()?;
        self.store_object(&blob_id, &blob_data)?;

        // Update sled working tree — O(log n)
        let full_path = path.trim_start_matches('/');
        let record = TreeEntryRecord {
            object_id: blob_id,
            kind: ObjectKind::Blob,
            mode: if executable { 0o755 } else { 0o644 },
        };
        self.tree_insert(full_path, &record)?;

        Ok(blob_id)
    }

    /// Synchronous mkdir for batch import.
    pub fn mkdir_sync(&self, path: &str) -> Result<ObjectId> {
        let new_tree = Tree::new();
        let tree_id = new_tree.id();
        let tree_data = new_tree.to_bytes()?;
        self.store_object(&tree_id, &tree_data)?;

        let full_path = path.trim_start_matches('/');
        let record = TreeEntryRecord {
            object_id: tree_id,
            kind: ObjectKind::Tree,
            mode: 0o755,
        };
        self.tree_insert(full_path, &record)?;

        Ok(tree_id)
    }

    /// Synchronous delete_file for batch import.
    pub fn delete_file_sync(&self, path: &str) -> Result<()> {
        let filename = path.trim_start_matches('/');
        self.tree_remove(filename)?;
        Ok(())
    }

    /// Synchronous commit for batch import.
    /// This is the main commit path optimized for import.
    /// Builds Tree from sled only when needed for the commit snapshot.
    pub fn commit_sync(
        &self,
        author: String,
        message: String,
        timestamp: i64,
    ) -> Result<u64> {
        // Build Tree from sled for the commit snapshot
        let root_tree = self.build_tree_from_sled()?;
        let tree_id = root_tree.id();
        let tree_data = root_tree.to_bytes()?;
        self.store_object(&tree_id, &tree_data)?;

        let current_rev = *self.current_rev.blocking_read();

        let parents = if current_rev > 0 || self.commit_path(current_rev).exists() {
            if let Ok(parent) = self.load_commit(current_rev) {
                vec![parent.id()]
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        let commit = Commit::new(tree_id, parents, author, message, timestamp, 0);
        let commit_id = commit.id();
        let commit_data = commit.to_bytes()?;

        let new_rev = current_rev + 1;

        self.store_object(&commit_id, &commit_data)?;
        self.store_commit(new_rev, &commit)?;
        self.store_tree_snapshot(new_rev, &root_tree)?;
        self.save_head(new_rev)?;

        // Flush sled if not in batch mode
        if !self.batch_mode.load(std::sync::atomic::Ordering::Relaxed) {
            self.tree_db.flush()
                .map_err(|e| anyhow!("sled flush failed: {}", e))?;
        }

        let mut cr = self.current_rev.blocking_write();
        *cr = new_rev;

        Ok(new_rev)
    }

    /// Get commit log (newest first)
    pub async fn log(&self, start_rev: u64, limit: usize) -> Result<Vec<Commit>> {
        let current = *self.current_rev.read().await;
        let end = std::cmp::min(start_rev, current);
        let mut result = Vec::new();

        for rev in (0..=end).rev() {
            if result.len() >= limit {
                break;
            }
            if let Ok(commit) = self.load_commit(rev) {
                result.push(commit);
            }
        }

        Ok(result)
    }

    /// Check if a path exists at a given revision
    pub async fn exists(&self, path: &str, rev: u64) -> Result<bool> {
        match self.get_file(path, rev).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_disk_repo_create_and_init() {
        let tmp = TempDir::new().unwrap();
        let repo = DiskRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();
        assert_eq!(repo.current_rev().await, 0);
        assert_eq!(repo.uuid().len(), 36);
    }

    #[tokio::test]
    async fn test_disk_repo_add_file_and_commit() {
        let tmp = TempDir::new().unwrap();
        let repo = DiskRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        repo.add_file("/test.txt", b"Hello, World!".to_vec(), false)
            .await
            .unwrap();
        let rev = repo
            .commit("tester".into(), "add test.txt".into(), 1000)
            .await
            .unwrap();
        assert_eq!(rev, 1);

        let content = repo.get_file("/test.txt", 1).await.unwrap();
        assert_eq!(content.as_ref(), b"Hello, World!");
    }

    #[tokio::test]
    async fn test_disk_repo_persistence_across_reopen() {
        let tmp = TempDir::new().unwrap();

        // Create repo and commit
        {
            let repo = DiskRepository::open(tmp.path()).unwrap();
            repo.initialize().await.unwrap();
            repo.add_file("/hello.txt", b"hello".to_vec(), false)
                .await
                .unwrap();
            repo.commit("user".into(), "first".into(), 100)
                .await
                .unwrap();
        }

        // Reopen
        {
            let repo = DiskRepository::open(tmp.path()).unwrap();
            repo.initialize().await.unwrap();
            assert_eq!(repo.current_rev().await, 1);

            let content = repo.get_file("/hello.txt", 1).await.unwrap();
            assert_eq!(content.as_ref(), b"hello");

            let log = repo.log(10, 100).await.unwrap();
            assert_eq!(log.len(), 2); // initial + 1
            assert_eq!(log[0].author, "user");
        }
    }

    #[tokio::test]
    async fn test_disk_repo_uuid_stable() {
        let tmp = TempDir::new().unwrap();
        let uuid1;
        {
            let repo = DiskRepository::open(tmp.path()).unwrap();
            uuid1 = repo.uuid().to_string();
        }
        {
            let repo = DiskRepository::open(tmp.path()).unwrap();
            assert_eq!(repo.uuid(), uuid1);
        }
    }

    #[tokio::test]
    async fn test_disk_repo_many_commits() {
        let tmp = TempDir::new().unwrap();
        let repo = DiskRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        for i in 0..50 {
            repo.add_file(
                &format!("/file_{}.txt", i),
                format!("content {}", i).into_bytes(),
                false,
            )
            .await
            .unwrap();
            repo.commit("bot".into(), format!("commit {}", i), i as i64)
                .await
                .unwrap();
        }

        assert_eq!(repo.current_rev().await, 50);
        let log = repo.log(50, 10).await.unwrap();
        assert_eq!(log.len(), 10);
    }

    #[tokio::test]
    async fn test_disk_repo_property_store() {
        let tmp = TempDir::new().unwrap();
        let repo = DiskRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        let ps = repo.property_store();
        ps.set("/test.txt".into(), "svn:mime-type".into(), "text/plain".into())
            .await
            .unwrap();

        let props = ps.get("/test.txt").await;
        assert_eq!(props.get("svn:mime-type"), Some(&"text/plain".to_string()));

        // Reopen and verify persistence
        drop(repo);
        let repo2 = DiskRepository::open(tmp.path()).unwrap();
        let ps2 = repo2.property_store();
        let props2 = ps2.get("/test.txt").await;
        assert_eq!(props2.get("svn:mime-type"), Some(&"text/plain".to_string()));
    }

    #[tokio::test]
    async fn test_disk_repo_batch_sync() {
        let tmp = TempDir::new().unwrap();
        let repo = DiskRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        repo.begin_batch();
        for i in 0..100 {
            repo.add_file_sync(
                &format!("file_{}.txt", i),
                format!("content {}", i).into_bytes(),
                false,
            )
            .unwrap();
        }
        let rev = repo.commit_sync("bot".into(), "batch commit".into(), 1000).unwrap();
        repo.end_batch();

        assert_eq!(rev, 1);

        // Verify file content via committed tree
        let content = repo.get_file("/file_0.txt", 1).await.unwrap();
        assert_eq!(content.as_ref(), b"content 0");
    }

    #[tokio::test]
    async fn test_disk_repo_delete_file() {
        let tmp = TempDir::new().unwrap();
        let repo = DiskRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        repo.add_file("/to_delete.txt", b"data".to_vec(), false)
            .await
            .unwrap();
        repo.commit("user".into(), "add".into(), 100).await.unwrap();

        repo.delete_file("/to_delete.txt").await.unwrap();
        repo.commit("user".into(), "delete".into(), 200).await.unwrap();

        // File should exist at rev 1, not at rev 2
        assert!(repo.get_file("/to_delete.txt", 1).await.is_ok());
        assert!(repo.get_file("/to_delete.txt", 2).await.is_err());
    }

    #[tokio::test]
    async fn test_disk_repo_migration_from_root_tree_bin() {
        let tmp = TempDir::new().unwrap();

        // Create repo with old format (simulate by writing root_tree.bin)
        {
            let repo = DiskRepository::open(tmp.path()).unwrap();
            repo.initialize().await.unwrap();
            repo.add_file("/migrated.txt", b"old data".to_vec(), false)
                .await
                .unwrap();
            repo.commit("user".into(), "pre-migration".into(), 100)
                .await
                .unwrap();

            // Write a root_tree.bin manually to simulate old format
            let tree = repo.build_tree_from_sled().unwrap();
            let data = bincode::serialize(&tree).unwrap();
            fs::write(tmp.path().join("root_tree.bin"), &data).unwrap();

            // Remove tree.db to simulate clean state
            drop(repo);
            let _ = fs::remove_dir_all(tmp.path().join("tree.db"));
        }

        // Reopen — should migrate from root_tree.bin
        {
            let repo = DiskRepository::open(tmp.path()).unwrap();
            // Check that sled has the migrated entry
            let entry = repo.tree_get("migrated.txt").unwrap();
            assert!(entry.is_some());
            assert_eq!(entry.unwrap().kind, ObjectKind::Blob);
            // root_tree.bin should be removed after migration
            assert!(!tmp.path().join("root_tree.bin").exists());
        }
    }
}
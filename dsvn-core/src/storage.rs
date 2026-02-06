//! Storage abstraction layer for DSvn
//!
//! Implements tiered storage with hot/warm/cold data separation

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;

use crate::object::ObjectId;

/// Result type for storage operations
pub type Result<T> = std::result::Result<T, StorageError>;

/// Errors that can occur during storage operations
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("Object not found: {0}")]
    NotFound(ObjectId),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Storage backend error: {0}")]
    Backend(String),
}

/// Generic object store interface
///
/// All storage tiers must implement this trait
#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// Get object data by ID
    async fn get(&self, id: ObjectId) -> Result<Bytes>;

    /// Check if object exists
    async fn exists(&self, id: ObjectId) -> Result<bool>;

    /// Put object data (returns the object ID)
    async fn put(&self, data: Bytes) -> Result<ObjectId>;

    /// Delete an object
    async fn delete(&self, id: ObjectId) -> Result<()>;

    /// Get multiple objects in parallel
    async fn get_batch(&self, ids: Vec<ObjectId>) -> Result<Vec<Option<Bytes>>>;

    /// Put multiple objects in a batch
    async fn put_batch(&self, data: Vec<Bytes>) -> Result<Vec<ObjectId>>;

    /// List all objects (with optional pagination)
    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<ObjectId>>;
}

/// Hot data store using LSM-tree (Fjall)
///
/// For recent commits and frequently accessed objects
pub struct HotStore {
    /// Fjall database instance
    db: Arc<fjall::Database>,

    /// Partition for storing objects
    objects: Arc<fjall::Keyspace>,

    /// Path to the store
    _path: String,
}

impl HotStore {
    /// Open or create a new hot store
    pub fn open(path: &str) -> Result<Self> {
        let db = fjall::Database::builder(path)
            .open()
            .map_err(|e| StorageError::Database(format!("Failed to open hot store: {}", e)))?;

        let objects = db
            .keyspace("objects", || fjall::KeyspaceCreateOptions::default())
            .map_err(|e| StorageError::Database(format!("Failed to open objects partition: {}", e)))?;

        Ok(Self {
            db: Arc::new(db),
            objects: Arc::new(objects),
            _path: path.to_string(),
        })
    }

    /// Persist data to disk
    pub fn persist(&self) -> Result<()> {
        self.db
            .persist(fjall::PersistMode::SyncAll)
            .map_err(|e| StorageError::Database(format!("Failed to persist: {}", e)))?;
        Ok(())
    }
}

#[async_trait]
impl ObjectStore for HotStore {
    async fn get(&self, id: ObjectId) -> Result<Bytes> {
        let key = id.to_hex();

        self.objects
            .get(&key)
            .map_err(|e| StorageError::Database(format!("Get failed: {}", e)))?
            .map(|v| Bytes::from(v.to_vec()))
            .ok_or(StorageError::NotFound(id))
    }

    async fn exists(&self, id: ObjectId) -> Result<bool> {
        let key = id.to_hex();

        Ok(self
            .objects
            .get(&key)
            .map_err(|e| StorageError::Database(format!("Exists check failed: {}", e)))?
            .is_some())
    }

    async fn put(&self, data: Bytes) -> Result<ObjectId> {
        let id = ObjectId::from_data(&data);
        let key = id.to_hex();

        self.objects
            .insert(&key, data.as_ref())
            .map_err(|e| StorageError::Database(format!("Put failed: {}", e)))?;

        Ok(id)
    }

    async fn delete(&self, id: ObjectId) -> Result<()> {
        let key = id.to_hex();

        self.objects
            .remove(&key)
            .map_err(|e| StorageError::Database(format!("Delete failed: {}", e)))?;

        Ok(())
    }

    async fn get_batch(&self, ids: Vec<ObjectId>) -> Result<Vec<Option<Bytes>>> {
        let mut results = Vec::with_capacity(ids.len());

        for id in ids {
            let key = id.to_hex();
            let value = self
                .objects
                .get(&key)
                .map_err(|e| StorageError::Database(format!("Batch get failed: {}", e)))?
                .map(|v| Bytes::from(v.to_vec()));
            results.push(value);
        }

        Ok(results)
    }

    async fn put_batch(&self, data: Vec<Bytes>) -> Result<Vec<ObjectId>> {
        let mut ids = Vec::with_capacity(data.len());

        for bytes in data {
            let id = ObjectId::from_data(&bytes);
            let key = id.to_hex();

            self.objects
                .insert(&key, bytes.as_ref())
                .map_err(|e| StorageError::Database(format!("Batch put failed: {}", e)))?;

            ids.push(id);
        }

        Ok(ids)
    }

    async fn list(&self, _offset: usize, _limit: usize) -> Result<Vec<ObjectId>> {
        // Fjall supports iteration but for now we'll return an empty list
        // TODO: Implement proper iteration using self.objects.iter()
        Ok(Vec::new())
    }
}

/// Pack file entry
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackEntry {
    pub object_id: String,
    pub offset: u64,
    pub size: u64,
    pub compressed_size: Option<u64>,
}

/// Warm data store using pack files
///
/// For older, less frequently accessed data
pub struct WarmStore {
    base_path: std::path::PathBuf,
    packs: Arc<tokio::sync::RwLock<Vec<PackEntry>>>,
}

impl WarmStore {
    /// Open or create a warm store
    pub fn open(base_path: &str) -> Result<Self> {
        let path = std::path::PathBuf::from(base_path);
        std::fs::create_dir_all(&path)?;

        // Load pack index (simplified - would scan directory in real implementation)
        let packs = Arc::new(tokio::sync::RwLock::new(Vec::new()));

        Ok(Self { base_path: path, packs })
    }

    /// Write a new pack file
    pub async fn write_pack(&self, objects: Vec<(ObjectId, Bytes)>) -> Result<()> {
        let pack_id = uuid::Uuid::new_v4();
        let pack_path = self.base_path.join(format!("pack-{}.pack", pack_id));
        let index_path = self.base_path.join(format!("pack-{}.idx", pack_id));

        // Create compressed pack file
        let mut pack_data = Vec::new();
        let mut index_data = Vec::new();

        for (id, data) in &objects {
            let offset = pack_data.len() as u64;

            // Compress object
            let compressed = zstd::encode_all(data.as_ref(), 3)
                .map_err(|e| StorageError::Backend(format!("Compression failed: {}", e)))?;

            pack_data.extend_from_slice(&compressed);

            // Add to index
            index_data.push(PackEntry {
                object_id: id.to_hex(),
                offset,
                size: data.len() as u64,
                compressed_size: Some(compressed.len() as u64),
            });
        }

        // Write pack file
        tokio::fs::write(&pack_path, pack_data).await?;

        // Write index file (simplified)
        let index_json = serde_json::to_string_pretty(&index_data)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        tokio::fs::write(&index_path, index_json).await?;

        // Update pack index
        let mut packs = self.packs.write().await;
        packs.extend(index_data);

        Ok(())
    }
}

#[async_trait]
impl ObjectStore for WarmStore {
    async fn get(&self, id: ObjectId) -> Result<Bytes> {
        let packs = self.packs.read().await;

        // Find object in pack index
        let _entry = packs
            .iter()
            .find(|e| e.object_id == id.to_hex())
            .ok_or(StorageError::NotFound(id))?;

        // Read from pack file (simplified - would need to track which pack file)
        // In real implementation, would track pack file per entry
        Err(StorageError::Backend("WarmStore get not fully implemented".to_string()))
    }

    async fn exists(&self, id: ObjectId) -> Result<bool> {
        let packs = self.packs.read().await;
        Ok(packs.iter().any(|e| e.object_id == id.to_hex()))
    }

    async fn put(&self, data: Bytes) -> Result<ObjectId> {
        // Warm store is write-through to pack files
        // Would buffer writes and flush periodically
        let id = ObjectId::from_data(&data);
        Ok(id)
    }

    async fn delete(&self, _id: ObjectId) -> Result<()> {
        // Pack files are immutable
        Err(StorageError::Backend("Cannot delete from warm store".to_string()))
    }

    async fn get_batch(&self, ids: Vec<ObjectId>) -> Result<Vec<Option<Bytes>>> {
        // Batch implementation would scan packs more efficiently
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            match self.get(id).await {
                Ok(data) => results.push(Some(data)),
                Err(_) => results.push(None),
            }
        }
        Ok(results)
    }

    async fn put_batch(&self, data: Vec<Bytes>) -> Result<Vec<ObjectId>> {
        let ids: Vec<_> = data.iter().map(|d| ObjectId::from_data(d.as_ref())).collect();
        self.write_pack(ids.iter().copied().zip(data.into_iter()).collect())
            .await?;
        Ok(ids)
    }

    async fn list(&self, _offset: usize, limit: usize) -> Result<Vec<ObjectId>> {
        let packs = self.packs.read().await;
        Ok(packs
            .iter()
            .take(limit)
            .filter_map(|e| ObjectId::from_hex(&e.object_id).ok())
            .collect())
    }
}

/// Tiered storage manager
///
/// Routes operations to appropriate storage tier
pub struct TieredStore {
    hot: Arc<HotStore>,
    warm: Arc<WarmStore>,
}

impl TieredStore {
    /// Create a new tiered store
    pub fn new(hot_path: &str, warm_path: &str) -> Result<Self> {
        let hot = Arc::new(HotStore::open(hot_path)?);
        let warm = Arc::new(WarmStore::open(warm_path)?);

        Ok(Self { hot, warm })
    }

    /// Get hot store reference
    pub fn hot(&self) -> &HotStore {
        &self.hot
    }

    /// Get warm store reference
    pub fn warm(&self) -> &WarmStore {
        &self.warm
    }

    /// Promote cold objects to hot store
    pub async fn promote(&self, ids: Vec<ObjectId>) -> Result<()> {
        for id in ids {
            if let Ok(data) = self.warm.get(id).await {
                self.hot.put(data).await?;
            }
        }
        Ok(())
    }

    /// Demote old objects to warm store
    pub async fn demote(&self, ids: Vec<ObjectId>) -> Result<()> {
        for id in ids {
            if let Ok(data) = self.hot.get(id).await {
                self.warm.put(data).await?;
                self.hot.delete(id).await?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ObjectStore for TieredStore {
    async fn get(&self, id: ObjectId) -> Result<Bytes> {
        // Try hot store first
        if let Ok(data) = self.hot.get(id).await {
            return Ok(data);
        }

        // Fall back to warm store
        if let Ok(data) = self.warm.get(id).await {
            return Ok(data);
        }

        Err(StorageError::NotFound(id))
    }

    async fn exists(&self, id: ObjectId) -> Result<bool> {
        Ok(self.hot.exists(id).await? || self.warm.exists(id).await?)
    }

    async fn put(&self, data: Bytes) -> Result<ObjectId> {
        // Always write to hot store first
        self.hot.put(data).await
    }

    async fn delete(&self, id: ObjectId) -> Result<()> {
        // Try to delete from both stores
        let _ = self.hot.delete(id).await;
        let _ = self.warm.delete(id).await;
        Ok(())
    }

    async fn get_batch(&self, ids: Vec<ObjectId>) -> Result<Vec<Option<Bytes>>> {
        let mut results = Vec::with_capacity(ids.len());

        // Batch get from hot store
        let hot_results = self.hot.get_batch(ids.clone()).await?;

        // Fill in misses from warm store
        for (i, id) in ids.iter().enumerate() {
            if let Some(data) = hot_results.get(i).and_then(|v| v.as_ref()) {
                results.push(Some(data.clone()));
            } else if let Ok(data) = self.warm.get(*id).await {
                results.push(Some(data));
            } else {
                results.push(None);
            }
        }

        Ok(results)
    }

    async fn put_batch(&self, data: Vec<Bytes>) -> Result<Vec<ObjectId>> {
        self.hot.put_batch(data).await
    }

    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<ObjectId>> {
        // List from hot store first, then warm
        let mut hot_ids = self.hot.list(offset, limit).await.unwrap_or_default();
        if hot_ids.len() < limit {
            let remaining = limit - hot_ids.len();
            let warm_ids = self.warm.list(0, remaining).await.unwrap_or_default();
            hot_ids.extend(warm_ids);
        }
        Ok(hot_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hot_store_put_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = HotStore::open(dir.path().to_str().unwrap()).unwrap();
        let data = Bytes::from(b"hello world".as_ref());
        let id = store.put(data.clone()).await.unwrap();
        let retrieved = store.get(id).await.unwrap();
        assert_eq!(data, retrieved);
    }

    #[tokio::test]
    async fn test_hot_store_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();

        // Create store and write data
        let store = HotStore::open(path).unwrap();
        let data = Bytes::from(b"persistent data".as_ref());
        let id = store.put(data.clone()).await.unwrap();
        store.persist().unwrap();

        // Drop store to release lock
        drop(store);

        // Reopen store and verify data
        let store2 = HotStore::open(path).unwrap();
        let retrieved = store2.get(id).await.unwrap();
        assert_eq!(data, retrieved);
    }

    #[tokio::test]
    async fn test_hot_store_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = HotStore::open(dir.path().to_str().unwrap()).unwrap();
        let data = Bytes::from(b"to be deleted".as_ref());
        let id = store.put(data).await.unwrap();

        store.delete(id).await.unwrap();

        let result = store.get(id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tiered_store() {
        let hot_dir = tempfile::tempdir().unwrap();
        let warm_dir = tempfile::tempdir().unwrap();
        let store = TieredStore::new(
            hot_dir.path().to_str().unwrap(),
            warm_dir.path().to_str().unwrap(),
        )
        .unwrap();
        let data = Bytes::from(b"test data".as_ref());
        let id = store.put(data.clone()).await.unwrap();
        let retrieved = store.get(id).await.unwrap();
        assert_eq!(data, retrieved);
    }
}

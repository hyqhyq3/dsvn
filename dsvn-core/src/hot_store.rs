//! Hot store using Fjall LSM-tree
//!
//! Provides high-performance storage for recent/active objects

use crate::object::ObjectId;
use anyhow::{Context, Result};
use bytes::Bytes;
use fjall::{Database, KeyspaceCreateOptions};
use std::path::Path;
use std::sync::Arc;

/// Hot store configuration
#[derive(Debug, Clone)]
pub struct HotStoreConfig {
    /// Path to the store
    pub path: String,
}

impl Default for HotStoreConfig {
    fn default() -> Self {
        Self {
            path: "data/hot".to_string(),
        }
    }
}

/// Hot store for recent objects
pub struct HotStore {
    /// Fjall database
    db: Database,

    /// Object keyspace
    objects: fjall::Keyspace,
}

impl HotStore {
    /// Open or create hot store
    pub async fn open(config: HotStoreConfig) -> Result<Self> {
        // Create directory if not exists
        std::fs::create_dir_all(&config.path)
            .context("Failed to create hot store directory")?;

        let path = Path::new(&config.path);

        // Open database
        let db = Database::builder(path).open()
            .context("Failed to open hot store database")?;

        // Create or open objects keyspace
        let objects = db.keyspace("objects", || KeyspaceCreateOptions::default())
            .context("Failed to open objects keyspace")?;

        Ok(Self {
            db,
            objects,
        })
    }

    /// Put object into hot store
    pub async fn put(&self, id: ObjectId, data: &[u8]) -> Result<()> {
        let key = id.to_hex();

        self.objects.insert(key.as_bytes(), data)
            .context("Failed to insert object into hot store")?;

        Ok(())
    }

    /// Get object from hot store
    pub async fn get(&self, id: ObjectId) -> Result<Option<Bytes>> {
        let key = id.to_hex();

        match self.objects.get(key.as_bytes()) {
            Ok(Some(data)) => {
                let bytes: &[u8] = data.as_ref();
                Ok(Some(Bytes::copy_from_slice(bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e).context("Failed to get object from hot store"),
        }
    }

    /// Check if object exists
    pub async fn contains(&self, id: ObjectId) -> Result<bool> {
        Ok(self.get(id).await?.is_some())
    }

    /// Delete object from hot store
    pub async fn delete(&self, id: ObjectId) -> Result<bool> {
        let key = id.to_hex();

        // Check if exists first
        let exists = self.objects.get(key.as_bytes())?.is_some();

        if exists {
            self.objects.remove(key.as_bytes())
                .context("Failed to delete object from hot store")?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Persist data to disk
    pub async fn persist(&self) -> Result<()> {
        self.db.persist(fjall::PersistMode::SyncAll)
            .context("Failed to persist hot store")?;
        Ok(())
    }

    /// Get approximate size (number of objects)
    pub async fn size(&self) -> Result<usize> {
        // Fjall doesn't provide direct size, return 0 for now
        // TODO: Implement object counter
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_store() -> HotStore {
        let temp_dir = TempDir::new().unwrap();
        let config = HotStoreConfig {
            path: temp_dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        };
        HotStore::open(config).await.unwrap()
    }

    #[tokio::test]
    async fn test_hot_store_put_and_get() {
        let store = create_test_store().await;

        let id = ObjectId::from_data(b"hello world");
        let data = b"hello world".to_vec();

        // Put
        store.put(id, &data).await.unwrap();

        // Get
        let retrieved = store.get(id).await.unwrap().unwrap();
        assert_eq!(retrieved.to_vec(), data);
    }

    #[tokio::test]
    async fn test_hot_store_get_nonexistent() {
        let store = create_test_store().await;

        let id = ObjectId::from_data(b"nonexistent");
        let result = store.get(id).await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_hot_store_contains() {
        let store = create_test_store().await;

        let id = ObjectId::from_data(b"test data");
        let data = b"test data".to_vec();

        assert!(!store.contains(id).await.unwrap());

        store.put(id, &data).await.unwrap();
        assert!(store.contains(id).await.unwrap());
    }

    #[tokio::test]
    async fn test_hot_store_delete() {
        let store = create_test_store().await;

        let id = ObjectId::from_data(b"delete me");
        let data = b"delete me".to_vec();

        store.put(id, &data).await.unwrap();
        assert!(store.contains(id).await.unwrap());

        let deleted = store.delete(id).await.unwrap();
        assert!(deleted);
        assert!(!store.contains(id).await.unwrap());
    }

    #[tokio::test]
    async fn test_hot_store_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let config = HotStoreConfig {
            path: temp_dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        };

        // Create store and add data
        let store = HotStore::open(config.clone()).await.unwrap();
        let id = ObjectId::from_data(b"persistent data");
        let data = b"persistent data".to_vec();

        store.put(id, &data).await.unwrap();
        store.persist().await.unwrap();

        // Drop store
        drop(store);

        // Reopen store
        let store2 = HotStore::open(config).await.unwrap();
        let retrieved = store2.get(id).await.unwrap().unwrap();
        assert_eq!(retrieved.to_vec(), data);
    }

    #[tokio::test]
    async fn test_hot_store_large_object() {
        let store = create_test_store().await;

        // Create 1MB data
        let large_data = vec![0u8; 1024 * 1024];
        let id = ObjectId::from_data(&large_data);

        store.put(id, &large_data).await.unwrap();

        let retrieved = store.get(id).await.unwrap().unwrap();
        assert_eq!(retrieved.len(), large_data.len());
        assert_eq!(retrieved.to_vec(), large_data);
    }
}

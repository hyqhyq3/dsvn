//! Persistent repository integration tests
//!
//! These tests must be run with --test-threads=1 to avoid Fjall file lock issues

use dsvn_core::{PersistentRepository, RepositoryMetadata};
use tempfile::TempDir;

#[tokio::test]
async fn test_create_persistent_repository() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    let repo = PersistentRepository::open(repo_path).await.unwrap();

    let uuid = repo.uuid().await;
    assert!(!uuid.is_empty());
    assert_eq!(uuid.len(), 36);
    
    let rev = repo.current_rev().await;
    assert_eq!(rev, 0);
}

#[tokio::test]
async fn test_repository_metadata_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    let repo = PersistentRepository::open(repo_path).await.unwrap();
    let original_uuid = repo.uuid().await;
    drop(repo);

    let repo = PersistentRepository::open(repo_path).await.unwrap();
    let current_uuid = repo.uuid().await;
    assert_eq!(current_uuid, original_uuid);
}

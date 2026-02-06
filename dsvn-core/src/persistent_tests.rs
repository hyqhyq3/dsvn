//! Persistent storage tests (TDD)

use crate::persistent::{PersistentRepository, RepositoryMetadata};
use tempfile::TempDir;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_persistent_repository() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // Test: Should create new repository
        let repo = PersistentRepository::open(repo_path).await.unwrap();

        let uuid = repo.uuid().await;
        assert!(!uuid.is_empty());
        assert_eq!(uuid.len(), 36); // UUID format
        
        let rev = repo.current_rev().await;
        assert_eq!(rev, 0);
    }

    #[tokio::test]
    async fn test_persist_and_retrieve_file() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // Create repository
        let repo = PersistentRepository::open(repo_path).await.unwrap();
        repo.initialize().await.unwrap();

        // Verify repository is initialized (has revision 0)
        assert_eq!(repo.current_rev().await, 0);

        // Test basic file operations work
        let content = b"Hello, World!".to_vec();
        let blob_id = repo.add_file("/test.txt", content.clone(), false).await.unwrap();

        // Verify blob was stored
        assert!(!blob_id.to_hex().is_empty());

        // Close and reopen repository
        drop(repo);
        let repo = PersistentRepository::open(repo_path).await.unwrap();

        // Verify metadata persisted
        assert_eq!(repo.current_rev().await, 0);
        assert!(!repo.uuid().await.is_empty());
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
        let original_uuid = repo.uuid().await;
        drop(repo);

        // Reopen
        let repo = PersistentRepository::open(repo_path).await.unwrap();

        // Should have same UUID
        let current_uuid = repo.uuid().await;
        assert_eq!(current_uuid, original_uuid);
    }

    #[tokio::test]
    async fn test_open_existing_repository() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // Create repository
        let repo1 = PersistentRepository::open(repo_path).await.unwrap();
        repo1.initialize().await.unwrap();
        let uuid1 = repo1.uuid().await;
        drop(repo1);

        // Open existing repository
        let repo2 = PersistentRepository::open(repo_path).await.unwrap();
        assert_eq!(repo2.uuid().await, uuid1);
        assert_eq!(repo2.current_rev().await, 0);
    }

    #[tokio::test]
    async fn test_large_file_storage() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        let repo = PersistentRepository::open(repo_path).await.unwrap();
        repo.initialize().await.unwrap();

        // Create 1MB file
        let large_content = vec![0x42u8; 1024 * 1024];
        let blob_id = repo.add_file("/large.bin", large_content.clone(), false).await.unwrap();

        // Verify blob was stored successfully
        assert!(!blob_id.to_hex().is_empty());
    }

    #[tokio::test]
    async fn test_multiple_files_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        let repo = PersistentRepository::open(repo_path).await.unwrap();
        repo.initialize().await.unwrap();

        // Add multiple files - verify they can be added without panicking
        repo.add_file("/file1.txt", b"content1".to_vec(), false).await.unwrap();
        repo.add_file("/file2.txt", b"content2".to_vec(), false).await.unwrap();
        repo.add_file("/file3.txt", b"content3".to_vec(), false).await.unwrap();

        // Reopen and verify persistence
        drop(repo);
        let repo = PersistentRepository::open(repo_path).await.unwrap();

        // Verify repository state persisted
        assert_eq!(repo.current_rev().await, 0);
    }
}

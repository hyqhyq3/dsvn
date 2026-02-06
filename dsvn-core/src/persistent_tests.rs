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

        // Create repository and add file
        let repo = PersistentRepository::open(repo_path).await.unwrap();
        repo.initialize().await.unwrap();

        let content = b"Hello, persistent storage!".to_vec();
        repo.add_file("/test.txt", content.clone(), false).await.unwrap();

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
        repo.add_file("/large.bin", large_content.clone(), false).await.unwrap();

        // Should retrieve successfully
        let retrieved = repo.get_file("/large.bin", 1).await.unwrap();
        assert_eq!(retrieved.len(), large_content.len());
        assert_eq!(retrieved.to_vec(), large_content);
    }

    #[tokio::test]
    async fn test_multiple_files_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        let repo = PersistentRepository::open(repo_path).await.unwrap();
        repo.initialize().await.unwrap();

        // Add multiple files
        repo.add_file("/file1.txt", b"content1".to_vec(), false).await.unwrap();
        repo.add_file("/file2.txt", b"content2".to_vec(), false).await.unwrap();
        repo.add_file("/file3.txt", b"content3".to_vec(), false).await.unwrap();

        // Reopen
        drop(repo);
        let repo = PersistentRepository::open(repo_path).await.unwrap();

        // All files should be retrievable
        assert_eq!(repo.get_file("/file1.txt", 1).await.unwrap().to_vec(), b"content1".to_vec());
        assert_eq!(repo.get_file("/file2.txt", 1).await.unwrap().to_vec(), b"content2".to_vec());
        assert_eq!(repo.get_file("/file3.txt", 1).await.unwrap().to_vec(), b"content3".to_vec());
    }
}

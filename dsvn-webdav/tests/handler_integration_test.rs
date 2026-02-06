//! Integration tests for DSvn repository operations
//! These tests validate core repository functionality

use dsvn_core::Repository;

/// Test helper that creates a repository and initializes it
async fn setup_test_repository() -> Repository {
    let repo = Repository::new();
    repo.initialize().await.unwrap();
    repo
}

#[tokio::test]
async fn test_repository_initialization() {
    // This test validates that we can set up a repository
    let repo = setup_test_repository().await;
    assert_eq!(repo.current_rev().await, 0);
}

#[tokio::test]
async fn test_basic_repository_operations() {
    // Test basic repository operations
    let repo = setup_test_repository().await;

    // Add a file
    let content = b"Hello, World!".to_vec();
    let _id = repo.add_file("/test.txt", content, false).await.unwrap();

    // Commit
    let rev = repo.commit("test".to_string(), "Test commit".to_string(), 0).await.unwrap();
    assert_eq!(rev, 1);
}

#[tokio::test]
async fn test_repository_file_retrieval() {
    // Test file retrieval
    let repo = setup_test_repository().await;

    let content = b"Test content".to_vec();
    repo.add_file("/file.txt", content.clone(), false).await.unwrap();
    repo.commit("user".to_string(), "Add file".to_string(), 0).await.unwrap();

    let retrieved = repo.get_file("/file.txt", 1).await.unwrap();
    assert_eq!(retrieved.to_vec(), content);
}

#[tokio::test]
async fn test_repository_directory_listing() {
    // Test directory listing
    let repo = setup_test_repository().await;

    // Add multiple files
    repo.add_file("/file1.txt", b"content1".to_vec(), false).await.unwrap();
    repo.add_file("/file2.txt", b"content2".to_vec(), false).await.unwrap();
    repo.commit("user".to_string(), "Add files".to_string(), 0).await.unwrap();

    // List directory
    let entries = repo.list_dir("/", 1).await.unwrap();
    assert!(entries.len() >= 2);
    assert!(entries.iter().any(|e| e.contains("file1.txt")));
    assert!(entries.iter().any(|e| e.contains("file2.txt")));
}

#[tokio::test]
async fn test_repository_log() {
    // Test commit log
    let repo = setup_test_repository().await;

    repo.commit("user1".to_string(), "Commit 1".to_string(), 0).await.unwrap();
    repo.commit("user2".to_string(), "Commit 2".to_string(), 0).await.unwrap();

    let log = repo.log(10, 100).await.unwrap();
    assert!(log.len() >= 3); // Initial + 2 commits
}

#[tokio::test]
async fn test_repository_mkdir() {
    // Test directory creation
    let repo = setup_test_repository().await;

    let dir_id = repo.mkdir("/src").await.unwrap();
    assert_ne!(dir_id.to_hex().len(), 0);
}

#[tokio::test]
async fn test_repository_delete() {
    // Test file deletion
    let repo = setup_test_repository().await;

    repo.add_file("/to_delete.txt", b"delete me".to_vec(), false).await.unwrap();
    repo.commit("user".to_string(), "Add file".to_string(), 0).await.unwrap();

    let result = repo.delete_file("/to_delete.txt").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_repository_exists() {
    // Test file existence check
    let repo = setup_test_repository().await;

    repo.add_file("/exists.txt", b"content".to_vec(), false).await.unwrap();
    repo.commit("user".to_string(), "Add file".to_string(), 0).await.unwrap();

    assert!(repo.exists("/exists.txt", 1).await.unwrap());
    assert!(!repo.exists("/nonexistent.txt", 1).await.unwrap());
}

#[tokio::test]
async fn test_repository_add_multiple_files() {
    // Test adding multiple files before committing
    let repo = setup_test_repository().await;

    repo.add_file("/main.rs", b"fn main() {}".to_vec(), true).await.unwrap();
    repo.add_file("/README.md", b"# Test".to_vec(), false).await.unwrap();
    repo.add_file("/Cargo.toml", b"[package]".to_vec(), false).await.unwrap();

    let rev = repo.commit("user".to_string(), "Add project files".to_string(), 0).await.unwrap();
    assert_eq!(rev, 1);

    // Verify all files exist
    assert!(repo.exists("/main.rs", 1).await.unwrap());
    assert!(repo.exists("/README.md", 1).await.unwrap());
    assert!(repo.exists("/Cargo.toml", 1).await.unwrap());
}

#[tokio::test]
async fn test_repository_overwrite_file() {
    // Test overwriting an existing file
    let repo = setup_test_repository().await;

    // Add initial file
    repo.add_file("/config.txt", b"version=1".to_vec(), false).await.unwrap();
    repo.commit("user".to_string(), "Initial commit".to_string(), 0).await.unwrap();

    // Modify file
    repo.add_file("/config.txt", b"version=2".to_vec(), false).await.unwrap();
    repo.commit("user".to_string(), "Update config".to_string(), 0).await.unwrap();

    // Verify new content
    let content = repo.get_file("/config.txt", 2).await.unwrap();
    assert_eq!(content.to_vec(), b"version=2");
}

#[tokio::test]
async fn test_repository_empty_directory_listing() {
    // Test listing empty repository
    let repo = setup_test_repository().await;

    let entries = repo.list_dir("/", 0).await.unwrap();
    assert_eq!(entries.len(), 0);
}

#[tokio::test]
async fn test_repository_log_limit() {
    // Test log limit parameter
    let repo = setup_test_repository().await;

    for i in 1..=5 {
        repo.commit("user".to_string(), format!("Commit {}", i), 0).await.unwrap();
    }

    // Request only 3 commits
    let log = repo.log(10, 3).await.unwrap();
    assert_eq!(log.len(), 3);
}

#[tokio::test]
async fn test_repository_nested_directories() {
    // Test creating nested directory structure
    let repo = setup_test_repository().await;

    repo.mkdir("/src").await.unwrap();
    repo.mkdir("/src/bin").await.unwrap();
    repo.add_file("/src/main.rs", b"fn main() {}".to_vec(), true).await.unwrap();
    repo.add_file("/src/bin/main.rs", b"fn main() {}".to_vec(), true).await.unwrap();
    repo.commit("user".to_string(), "Add nested structure".to_string(), 0).await.unwrap();

    assert!(repo.exists("/src/main.rs", 1).await.unwrap());
    assert!(repo.exists("/src/bin/main.rs", 1).await.unwrap());
}

// NOTE: Full HTTP handler tests require a more sophisticated test setup
// The quick-test.sh script provides end-to-end testing with real SVN client,
// which is more valuable than trying to mock HTTP requests at the unit level.
//
// To run comprehensive integration tests:
// make quick-test

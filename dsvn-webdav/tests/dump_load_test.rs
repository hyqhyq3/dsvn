//! Integration tests for the svnrdump protocol (dump/load cycle)

use dsvn_core::SqliteRepository;
use dsvn_webdav::dump_handlers::{self, DumpParams, LoadStats};
use std::sync::Arc;
use tempfile::TempDir;

/// Create a test repository with some commits
async fn create_test_repo(dir: &std::path::Path) -> Arc<SqliteRepository> {
    let repo = SqliteRepository::open(dir).unwrap();
    repo.initialize().await.unwrap();
    let repo = Arc::new(repo);

    // Commit 1: add files
    repo.add_file("/hello.txt", b"Hello, World!".to_vec(), false)
        .await
        .unwrap();
    repo.add_file("/readme.md", b"# Test Repo\nThis is a test.".to_vec(), false)
        .await
        .unwrap();
    repo.commit("alice".into(), "Initial commit".into(), 1700000000)
        .await
        .unwrap();

    // Commit 2: add a directory and another file
    repo.mkdir("/docs").await.unwrap();
    repo.add_file("/docs/guide.txt", b"Guide content here".to_vec(), false)
        .await
        .unwrap();
    repo.commit("bob".into(), "Add documentation".into(), 1700001000)
        .await
        .unwrap();

    // Commit 3: modify a file
    repo.add_file("/hello.txt", b"Hello, Updated World!".to_vec(), false)
        .await
        .unwrap();
    repo.commit("alice".into(), "Update greeting".into(), 1700002000)
        .await
        .unwrap();

    // Commit 4: delete a file
    repo.delete_file("/readme.md").await.unwrap();
    repo.commit("charlie".into(), "Remove readme".into(), 1700003000)
        .await
        .unwrap();

    assert_eq!(repo.current_rev().await, 4);
    repo
}

#[tokio::test]
async fn test_full_dump() {
    let tmp = TempDir::new().unwrap();
    let repo = create_test_repo(tmp.path()).await;

    let params = DumpParams::from_query("", 4);
    let data = dump_handlers::generate_dump(&repo, &params).await.unwrap();
    let dump_str = String::from_utf8_lossy(&data);

    // Verify dump format header
    assert!(dump_str.starts_with("SVN-fs-dump-format-version: 3"));
    assert!(dump_str.contains(&format!("UUID: {}", repo.uuid())));

    // Verify revision headers
    assert!(dump_str.contains("Revision-number: 0"));
    assert!(dump_str.contains("Revision-number: 1"));
    assert!(dump_str.contains("Revision-number: 2"));
    assert!(dump_str.contains("Revision-number: 3"));
    assert!(dump_str.contains("Revision-number: 4"));

    // Verify authors and log messages
    assert!(dump_str.contains("alice"));
    assert!(dump_str.contains("bob"));
    assert!(dump_str.contains("charlie"));
    assert!(dump_str.contains("Initial commit"));
    assert!(dump_str.contains("Add documentation"));
    assert!(dump_str.contains("Update greeting"));
    assert!(dump_str.contains("Remove readme"));

    // Verify node entries
    assert!(dump_str.contains("Node-path: hello.txt"));
    assert!(dump_str.contains("Node-path: readme.md"));
    assert!(dump_str.contains("Node-path: docs/guide.txt"));
    assert!(dump_str.contains("Node-action: add"));
    assert!(dump_str.contains("Node-action: change"));
    assert!(dump_str.contains("Node-action: delete"));

    // Verify content
    assert!(dump_str.contains("Hello, World!"));
    assert!(dump_str.contains("Hello, Updated World!"));
    assert!(dump_str.contains("Guide content here"));

    // Verify MD5 checksums are present
    assert!(dump_str.contains("Text-content-md5:"));
}

#[tokio::test]
async fn test_ranged_dump() {
    let tmp = TempDir::new().unwrap();
    let repo = create_test_repo(tmp.path()).await;

    // Dump only revisions 2-3
    let params = DumpParams::from_query("r=2:3", 4);
    let data = dump_handlers::generate_dump(&repo, &params).await.unwrap();
    let dump_str = String::from_utf8_lossy(&data);

    assert!(dump_str.contains("Revision-number: 2"));
    assert!(dump_str.contains("Revision-number: 3"));
    assert!(!dump_str.contains("Revision-number: 0\n"));
    assert!(!dump_str.contains("Revision-number: 1\n"));
    assert!(!dump_str.contains("Revision-number: 4"));

    assert!(dump_str.contains("Add documentation"));
    assert!(dump_str.contains("Update greeting"));
}

#[tokio::test]
async fn test_incremental_dump() {
    let tmp = TempDir::new().unwrap();
    let repo = create_test_repo(tmp.path()).await;

    let params = DumpParams::from_query("r=3:4&incremental=true", 4);
    let data = dump_handlers::generate_dump(&repo, &params).await.unwrap();
    let dump_str = String::from_utf8_lossy(&data);

    // Incremental dumps should NOT include UUID
    assert!(!dump_str.contains("UUID:"));
    assert!(dump_str.starts_with("SVN-fs-dump-format-version: 3"));

    assert!(dump_str.contains("Revision-number: 3"));
    assert!(dump_str.contains("Revision-number: 4"));
}

#[tokio::test]
async fn test_dump_load_roundtrip() {
    // Create source repo
    let src_tmp = TempDir::new().unwrap();
    let src_repo = create_test_repo(src_tmp.path()).await;

    // Full dump of source
    let params = DumpParams {
        start_rev: 0,
        end_rev: 4,
        incremental: false,
        format_version: 3,
    };
    let dump_data = dump_handlers::generate_dump(&src_repo, &params)
        .await
        .unwrap();

    // Create destination repo
    let dst_tmp = TempDir::new().unwrap();
    let dst_repo = Arc::new(SqliteRepository::open(dst_tmp.path()).unwrap());
    dst_repo.initialize().await.unwrap();

    // Load dump into destination
    let response = dump_handlers::handle_load(dst_repo.clone(), dump_data).await;
    assert_eq!(response.status(), 200);

    // Verify destination has the same data
    let dst_head = dst_repo.current_rev().await;
    assert!(dst_head >= 3, "Destination should have at least 3 revisions, got {}", dst_head);

    // Check that files exist at the final revision
    let hello_content = dst_repo
        .get_file("/hello.txt", dst_head)
        .await
        .unwrap();
    assert_eq!(hello_content.as_ref(), b"Hello, Updated World!");

    let guide_content = dst_repo
        .get_file("/docs/guide.txt", dst_head)
        .await
        .unwrap();
    assert_eq!(guide_content.as_ref(), b"Guide content here");

    // readme.md should be deleted
    assert!(dst_repo.get_file("/readme.md", dst_head).await.is_err());
}

#[tokio::test]
async fn test_dump_format_version_2() {
    let tmp = TempDir::new().unwrap();
    let repo = create_test_repo(tmp.path()).await;

    let params = DumpParams::from_query("format=2", 4);
    let data = dump_handlers::generate_dump(&repo, &params).await.unwrap();
    let dump_str = String::from_utf8_lossy(&data);

    assert!(dump_str.starts_with("SVN-fs-dump-format-version: 2"));
}

#[tokio::test]
async fn test_empty_repo_dump() {
    let tmp = TempDir::new().unwrap();
    let repo = SqliteRepository::open(tmp.path()).unwrap();
    repo.initialize().await.unwrap();

    let params = DumpParams::from_query("", 0);
    let data = dump_handlers::generate_dump(&repo, &params).await.unwrap();
    let dump_str = String::from_utf8_lossy(&data);

    assert!(dump_str.contains("SVN-fs-dump-format-version: 3"));
    assert!(dump_str.contains("UUID:"));
    assert!(dump_str.contains("Revision-number: 0"));
}

#[tokio::test]
async fn test_dump_with_binary_content() {
    let tmp = TempDir::new().unwrap();
    let repo = SqliteRepository::open(tmp.path()).unwrap();
    repo.initialize().await.unwrap();

    // Add a binary file
    let binary_content: Vec<u8> = (0..=255).collect();
    repo.add_file("/binary.dat", binary_content.clone(), false)
        .await
        .unwrap();
    repo.commit("system".into(), "Add binary file".into(), 1700000000)
        .await
        .unwrap();

    let params = DumpParams::from_query("", 1);
    let data = dump_handlers::generate_dump(&repo, &params).await.unwrap();

    // The dump should contain the binary content
    // Find the binary content in the dump
    let dump_str = String::from_utf8_lossy(&data);
    assert!(dump_str.contains("Node-path: binary.dat"));
    assert!(dump_str.contains("Text-content-length: 256"));
    assert!(dump_str.contains("Text-content-md5:"));
}

#[tokio::test]
async fn test_load_stats() {
    let tmp = TempDir::new().unwrap();
    let repo = Arc::new(SqliteRepository::open(tmp.path()).unwrap());
    repo.initialize().await.unwrap();

    let dump = b"SVN-fs-dump-format-version: 3\n\
\n\
UUID: test-uuid\n\
\n\
Revision-number: 0\n\
Prop-content-length: 10\n\
Content-length: 10\n\
\n\
PROPS-END\n\
\n\
Revision-number: 1\n\
Prop-content-length: 56\n\
Content-length: 56\n\
\n\
K 7\n\
svn:log\n\
V 4\n\
test\n\
K 10\n\
svn:author\n\
V 4\n\
user\n\
PROPS-END\n\
\n\
Node-path: file1.txt\n\
Node-kind: file\n\
Node-action: add\n\
Prop-content-length: 10\n\
Text-content-length: 7\n\
Content-length: 17\n\
\n\
PROPS-END\n\
content\n\
\n\
Node-path: file2.txt\n\
Node-kind: file\n\
Node-action: add\n\
Prop-content-length: 10\n\
Text-content-length: 6\n\
Content-length: 16\n\
\n\
PROPS-END\n\
hello!\n";

    let response = dump_handlers::handle_load(repo.clone(), dump.to_vec()).await;
    assert_eq!(response.status(), 200);

    // Parse response body as JSON
    let body = response.into_body();
    use http_body_util::BodyExt;
    let bytes = body.collect().await.unwrap().to_bytes();
    let stats: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(stats["revisions_loaded"], 1);
    assert_eq!(stats["nodes_processed"], 2);
    assert_eq!(stats["files_added"], 2);
    assert!(stats["final_revision"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn test_is_dump_load_detection() {
    assert!(dump_handlers::is_dump_request("application/vnd.svn-dumpfile"));
    assert!(dump_handlers::is_dump_request("text/html, application/vnd.svn-dumpfile;q=0.9"));
    assert!(!dump_handlers::is_dump_request("text/html"));

    assert!(dump_handlers::is_load_request("application/vnd.svn-dumpfile"));
    assert!(!dump_handlers::is_load_request("application/json"));
}

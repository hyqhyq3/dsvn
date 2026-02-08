//! Integration tests for the /sync HTTP endpoints.

use dsvn_core::SqliteRepository;
use dsvn_core::sync::{SyncConfig, SyncEndpointInfo, RevisionSummary};
use dsvn_core::replication::RevisionData;
use http_body_util::BodyExt;
use std::sync::Arc;
use tempfile::TempDir;

/// Helper to set up a test repository with some commits.
async fn setup_test_repo() -> (TempDir, Arc<SqliteRepository>) {
    let tmp = TempDir::new().unwrap();
    let repo = SqliteRepository::open(tmp.path()).unwrap();
    repo.initialize().await.unwrap();

    // Create a few test commits
    repo.add_file("/file1.txt", b"Hello World".to_vec(), false)
        .await.unwrap();
    repo.commit("alice".into(), "First commit".into(), 1000).await.unwrap();

    repo.add_file("/file2.txt", b"Second file".to_vec(), false)
        .await.unwrap();
    repo.commit("bob".into(), "Second commit".into(), 2000).await.unwrap();

    repo.add_file("/dir/nested.txt", b"Nested".to_vec(), false)
        .await.unwrap();
    repo.commit("alice".into(), "Third commit".into(), 3000).await.unwrap();

    drop(repo);

    let repo2 = SqliteRepository::open(tmp.path()).unwrap();
    repo2.initialize().await.unwrap();
    let arc = Arc::new(repo2);

    (tmp, arc)
}

/// Extract response body bytes from a Full<Bytes> response.
async fn body_bytes(resp: hyper::Response<http_body_util::Full<bytes::Bytes>>) -> Vec<u8> {
    resp.into_body().collect().await.unwrap().to_bytes().to_vec()
}

#[tokio::test]
async fn test_sync_info() {
    let (_tmp, repo) = setup_test_repo().await;

    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/info", "GET", &[], "", &repo,
    ).await;

    assert_eq!(resp.status(), 200);

    let body = body_bytes(resp).await;
    let info: SyncEndpointInfo = serde_json::from_slice(&body).unwrap();

    assert_eq!(info.head_rev, 3);
    assert_eq!(info.protocol_version, 1);
    assert!(!info.uuid.is_empty());
    assert!(!info.capabilities.is_empty());
}

#[tokio::test]
async fn test_sync_revs() {
    let (_tmp, repo) = setup_test_repo().await;

    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/revs", "GET", &[], "from=1&to=3", &repo,
    ).await;

    assert_eq!(resp.status(), 200);

    let body = body_bytes(resp).await;
    let revs: Vec<RevisionSummary> = serde_json::from_slice(&body).unwrap();

    assert_eq!(revs.len(), 3);
    assert_eq!(revs[0].rev, 1);
    assert_eq!(revs[0].author, "alice");
    assert_eq!(revs[1].rev, 2);
    assert_eq!(revs[1].author, "bob");
    assert_eq!(revs[2].rev, 3);
}

#[tokio::test]
async fn test_sync_delta() {
    let (_tmp, repo) = setup_test_repo().await;

    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/delta", "GET", &[], "from=1&to=2", &repo,
    ).await;

    assert_eq!(resp.status(), 200);

    let body = body_bytes(resp).await;
    let revisions: Vec<RevisionData> = serde_json::from_slice(&body).unwrap();

    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].revision, 1);
    assert!(revisions[0].verify_content_hash());
    assert_eq!(revisions[1].revision, 2);
    assert!(revisions[1].verify_content_hash());

    // First revision should have file1.txt as an object
    assert!(!revisions[0].objects.is_empty());
}

#[tokio::test]
async fn test_sync_config_get_default() {
    let (_tmp, repo) = setup_test_repo().await;

    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/config", "GET", &[], "", &repo,
    ).await;

    assert_eq!(resp.status(), 200);

    let body = body_bytes(resp).await;
    let config: SyncConfig = serde_json::from_slice(&body).unwrap();

    assert!(config.enabled);
    assert_eq!(config.max_cache_age_hours, 720);
}

#[tokio::test]
async fn test_sync_config_set() {
    let (_tmp, repo) = setup_test_repo().await;

    let new_config = SyncConfig {
        enabled: false,
        cache_dir: Some("/tmp/test-cache".into()),
        max_cache_age_hours: 48,
        require_auth: true,
        allowed_sources: vec!["192.168.1.0/24".into()],
    };
    let body = serde_json::to_vec(&new_config).unwrap();

    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/config", "POST", &body, "", &repo,
    ).await;

    assert_eq!(resp.status(), 200);

    // Verify it was saved
    let loaded = SyncConfig::load(repo.root()).unwrap();
    assert!(!loaded.enabled);
    assert_eq!(loaded.max_cache_age_hours, 48);
    assert!(loaded.require_auth);
}

#[tokio::test]
async fn test_sync_disabled() {
    let (_tmp, repo) = setup_test_repo().await;

    // Disable sync
    let config = SyncConfig {
        enabled: false,
        ..SyncConfig::default()
    };
    config.save(repo.root()).unwrap();

    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/info", "GET", &[], "", &repo,
    ).await;

    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_sync_unknown_endpoint() {
    let (_tmp, repo) = setup_test_repo().await;

    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/nonexistent", "GET", &[], "", &repo,
    ).await;

    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_sync_revs_invalid_range() {
    let (_tmp, repo) = setup_test_repo().await;

    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/revs", "GET", &[], "from=10&to=5", &repo,
    ).await;

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_sync_objects() {
    let (_tmp, repo) = setup_test_repo().await;

    // First get delta to find object IDs
    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/delta", "GET", &[], "from=1&to=1", &repo,
    ).await;
    let body = body_bytes(resp).await;
    let revisions: Vec<RevisionData> = serde_json::from_slice(&body).unwrap();
    assert!(!revisions[0].objects.is_empty());

    let object_id = revisions[0].objects[0].0;
    let hex_id = object_id.to_hex();

    // Fetch the object via /sync/objects
    let query = format!("id={}", hex_id);
    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/objects", "GET", &[], &query, &repo,
    ).await;

    assert_eq!(resp.status(), 200);

    let body = body_bytes(resp).await;
    // Binary format: [32 bytes id][4 bytes len][N bytes data]
    assert!(body.len() > 36);

    // Verify the ObjectId in the response matches
    let mut id_bytes = [0u8; 32];
    id_bytes.copy_from_slice(&body[0..32]);
    assert_eq!(id_bytes, *object_id.as_bytes());

    // Verify length field
    let len = u32::from_be_bytes(body[32..36].try_into().unwrap());
    assert!(len > 0);
    assert_eq!(body.len(), 36 + len as usize);
}

#[tokio::test]
async fn test_sync_objects_nonexistent() {
    let (_tmp, repo) = setup_test_repo().await;

    // Request a non-existent object
    let fake_hex = "0".repeat(64);
    let query = format!("id={}", fake_hex);

    let resp = dsvn_webdav::sync_handlers::handle_sync_request(
        "/objects", "GET", &[], &query, &repo,
    ).await;

    assert_eq!(resp.status(), 200);

    let body = body_bytes(resp).await;
    // 32 bytes id + 4 bytes sentinel (0xFFFFFFFF)
    assert_eq!(body.len(), 36);
    let len = u32::from_be_bytes(body[32..36].try_into().unwrap());
    assert_eq!(len, 0xFFFF_FFFF); // sentinel for not found
}

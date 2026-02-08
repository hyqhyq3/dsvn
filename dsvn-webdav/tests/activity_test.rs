//! Tests for /!svn/act/ activity (transaction) management endpoints
//!
//! Tests cover:
//! - MKACTIVITY: create activities with client/server-generated IDs
//! - GET /!svn/act/: list all activities
//! - DELETE /!svn/act/{txn-id}: abort/remove activities
//! - Duplicate activity prevention
//! - Delete non-existent activity

use dsvn_webdav::{WebDavHandler, Config};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use std::net::SocketAddr;

/// Spawn a local test HTTP server, returns the listen address.
async fn spawn_test_server() -> SocketAddr {
    let tmp = tempfile::tempdir().unwrap();
    let repo_path = tmp.path().to_path_buf();
    Box::leak(Box::new(tmp));

    dsvn_webdav::init_repository(&repo_path).unwrap_or_else(|_| {});
    dsvn_webdav::init_repository_async().await.unwrap_or_else(|_| {});

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => continue,
            };
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = hyper::service::service_fn(|req| {
                    let handler = WebDavHandler::with_config(Config::default());
                    async move {
                        handler.handle(req).await.map_err(|e| {
                            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
                        })
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            });
        }
    });

    // Give the server a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    addr
}

/// Send an HTTP request via raw TCP and parse the response.
/// Returns (status_code, headers_text, body_text).
async fn send_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
) -> (u16, String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();

    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n",
        method = method,
        path = path,
        port = addr.port()
    );
    stream.write_all(request.as_bytes()).await.unwrap();

    // Read the full response
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response).to_string();

    // Parse status code
    let status_line = response_str.lines().next().unwrap_or("");
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // Split headers and body at \r\n\r\n
    let (headers, body) = if let Some(pos) = response_str.find("\r\n\r\n") {
        (
            response_str[..pos].to_string(),
            response_str[pos + 4..].to_string(),
        )
    } else {
        (response_str.clone(), String::new())
    };

    (status_code, headers, body)
}

/// Extract a header value (case-insensitive) from the raw headers block.
fn get_header(headers: &str, name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    for line in headers.lines() {
        if let Some(colon) = line.find(':') {
            if line[..colon].trim().to_lowercase() == lower {
                return Some(line[colon + 1..].trim().to_string());
            }
        }
    }
    None
}

// ============================================================
// MKACTIVITY tests
// ============================================================

#[tokio::test]
async fn test_mkactivity_creates_activity() {
    let addr = spawn_test_server().await;

    let (status, headers, body) =
        send_request(addr, "MKACTIVITY", "/svn/!svn/act/test-txn-100").await;
    assert_eq!(status, 201, "Expected 201, got {}: {}", status, body);

    // Location header
    let location = get_header(&headers, "location");
    assert!(location.is_some(), "Should have Location header");
    assert!(
        location.unwrap().contains("/!svn/act/test-txn-100"),
        "Location should point to the activity"
    );

    // SVN-Txn-Name header
    let txn = get_header(&headers, "svn-txn-name");
    assert_eq!(txn.as_deref(), Some("test-txn-100"));

    // Body
    assert!(body.contains("/!svn/act/test-txn-100"), "Body: {}", body);
    assert!(body.contains("mkactivity-response"), "Body: {}", body);
}

#[tokio::test]
async fn test_mkactivity_duplicate_rejected() {
    let addr = spawn_test_server().await;

    let (s, _, _) = send_request(addr, "MKACTIVITY", "/svn/!svn/act/dup-txn").await;
    assert_eq!(s, 201);

    let (s, _, body) = send_request(addr, "MKACTIVITY", "/svn/!svn/act/dup-txn").await;
    assert_eq!(s, 405, "Duplicate should be 405, got {}: {}", s, body);
    assert!(body.contains("already exists"), "Body: {}", body);
}

#[tokio::test]
async fn test_mkactivity_server_generated_id() {
    let addr = spawn_test_server().await;

    let (status, headers, _body) = send_request(addr, "MKACTIVITY", "/svn/!svn/act/").await;
    assert_eq!(status, 201);

    let txn = get_header(&headers, "svn-txn-name");
    assert!(txn.is_some(), "Should have SVN-Txn-Name");
    assert!(!txn.unwrap().is_empty(), "ID should be non-empty");
}

#[tokio::test]
async fn test_mkactivity_non_activity_path_rejected() {
    let addr = spawn_test_server().await;

    let (status, _, _) = send_request(addr, "MKACTIVITY", "/svn/some/other/path").await;
    assert_eq!(status, 405, "MKACTIVITY on wrong path should be 405");
}

// ============================================================
// GET /!svn/act/ — list activities
// ============================================================

#[tokio::test]
async fn test_list_activities() {
    let addr = spawn_test_server().await;

    // Create two activities
    let (s, _, _) = send_request(addr, "MKACTIVITY", "/svn/!svn/act/list-a").await;
    assert_eq!(s, 201);
    let (s, _, _) = send_request(addr, "MKACTIVITY", "/svn/!svn/act/list-b").await;
    assert_eq!(s, 201);

    // List
    let (status, headers, body) = send_request(addr, "GET", "/svn/!svn/act/").await;
    assert_eq!(status, 200, "Expected 200, got {}: {}", status, body);

    // Content-Type
    let ct = get_header(&headers, "content-type").unwrap_or_default();
    assert!(ct.contains("text/xml"), "CT: {}", ct);

    // UUID header
    assert!(get_header(&headers, "svn-repository-uuid").is_some());

    // Both activities listed
    assert!(body.contains("activity-collection-set"), "Body: {}", body);
    assert!(body.contains("list-a"), "Body: {}", body);
    assert!(body.contains("list-b"), "Body: {}", body);
}

// ============================================================
// DELETE /!svn/act/{txn-id}
// ============================================================

#[tokio::test]
async fn test_delete_activity() {
    let addr = spawn_test_server().await;

    let (s, _, _) = send_request(addr, "MKACTIVITY", "/svn/!svn/act/del-me").await;
    assert_eq!(s, 201);

    let (status, _, _) = send_request(addr, "DELETE", "/svn/!svn/act/del-me").await;
    assert_eq!(status, 204, "DELETE should return 204");

    // Gone from listing
    let (_, _, body) = send_request(addr, "GET", "/svn/!svn/act/").await;
    assert!(!body.contains("del-me"), "Should be gone, body: {}", body);
}

#[tokio::test]
async fn test_delete_nonexistent_activity() {
    let addr = spawn_test_server().await;

    let (status, _, body) = send_request(addr, "DELETE", "/svn/!svn/act/ghost").await;
    assert_eq!(status, 404, "Expected 404, got {}", status);
    assert!(body.contains("not found"), "Body: {}", body);
}

#[tokio::test]
async fn test_delete_activity_collection_rejected() {
    let addr = spawn_test_server().await;

    let (status, _, _) = send_request(addr, "DELETE", "/svn/!svn/act/").await;
    assert_eq!(status, 405, "DELETE on collection itself should be 405");
}

// ============================================================
// Full lifecycle
// ============================================================

#[tokio::test]
async fn test_full_activity_workflow() {
    let addr = spawn_test_server().await;

    // 1. List (initial)
    let (s, _, _) = send_request(addr, "GET", "/svn/!svn/act/").await;
    assert_eq!(s, 200);

    // 2. Create
    let (s, _, _) = send_request(addr, "MKACTIVITY", "/svn/!svn/act/wf-txn").await;
    assert_eq!(s, 201);

    // 3. Visible in listing
    let (_, _, body) = send_request(addr, "GET", "/svn/!svn/act/").await;
    assert!(body.contains("wf-txn"));

    // 4. Delete
    let (s, _, _) = send_request(addr, "DELETE", "/svn/!svn/act/wf-txn").await;
    assert_eq!(s, 204);

    // 5. Gone
    let (_, _, body) = send_request(addr, "GET", "/svn/!svn/act/").await;
    assert!(!body.contains("wf-txn"));
}

//! HTTP sync endpoint handlers for dsvn server-to-server replication.
//!
//! Endpoints:
//!   GET  /sync/info         → repository identity & head revision
//!   GET  /sync/revs         → revision metadata list (from/to query params)
//!   GET  /sync/objects      → batch object download (id query params)
//!   GET  /sync/delta        → full RevisionData for a revision range
//!   GET  /sync/config       → current sync configuration
//!   POST /sync/config       → update sync configuration

use bytes::Bytes;
use dsvn_core::replication::RevisionData;
use dsvn_core::sync::{RevisionSummary, SyncConfig, SyncEndpointInfo};
use dsvn_core::{Blob, ObjectId, ObjectKind, SqliteRepository, TreeChange};
use http_body_util::Full;
use hyper::{Request, Response};
use std::collections::HashMap;
use std::sync::Arc;

/// Current sync API protocol version.
const SYNC_PROTOCOL_VERSION: u32 = 1;

/// Maximum number of objects that can be fetched in a single /sync/objects request.
const MAX_OBJECTS_PER_REQUEST: usize = 1000;

/// Maximum revision range for a single /sync/delta request.
const MAX_DELTA_RANGE: u64 = 500;

// ─────────────────────────────────────────────────────
// Public dispatch
// ─────────────────────────────────────────────────────

/// Route a sync request to the appropriate handler.
/// `path` is the portion after the "/sync" prefix (e.g. "/info", "/revs").
pub async fn handle_sync_request(
    path: &str,
    method: &str,
    body: &[u8],
    query: &str,
    repo: &Arc<SqliteRepository>,
) -> Response<Full<Bytes>> {
    // Check if sync is enabled
    match SyncConfig::load(repo.root()) {
        Ok(config) if !config.enabled => {
            return json_error(403, "Sync endpoints are disabled");
        }
        Err(e) => {
            tracing::warn!("Failed to load sync config: {}", e);
            // continue with defaults (enabled)
        }
        _ => {}
    }

    match (method, path) {
        ("GET", "/info") => handle_info(repo).await,
        ("GET", "/revs") => handle_revs(repo, query).await,
        ("GET", "/objects") => handle_objects(repo, query).await,
        ("GET", "/delta") => handle_delta(repo, query).await,
        ("GET", "/config") => handle_get_config(repo).await,
        ("POST", "/config") => handle_set_config(repo, body).await,
        _ => json_error(404, &format!("Unknown sync endpoint: {} /sync{}", method, path)),
    }
}

// ─────────────────────────────────────────────────────
// GET /sync/info
// ─────────────────────────────────────────────────────

async fn handle_info(repo: &Arc<SqliteRepository>) -> Response<Full<Bytes>> {
    let head_rev = repo.current_rev().await;
    let info = SyncEndpointInfo {
        uuid: repo.uuid().to_string(),
        head_rev,
        protocol_version: SYNC_PROTOCOL_VERSION,
        capabilities: vec![
            "incremental-sync".into(),
            "delta-transfer".into(),
            "batch-objects".into(),
            "on-demand-fetch".into(),
        ],
    };
    json_ok(&info)
}

// ─────────────────────────────────────────────────────
// GET /sync/revs?from=X&to=Y
// ─────────────────────────────────────────────────────

async fn handle_revs(repo: &Arc<SqliteRepository>, query: &str) -> Response<Full<Bytes>> {
    let params = parse_query(query);
    let from: u64 = params.get("from").and_then(|v| v.parse().ok()).unwrap_or(1);
    let to: u64 = params
        .get("to")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            // default to HEAD — use blocking read since we're in a sync context
            let head_path = repo.root().join("refs").join("head");
            std::fs::read_to_string(&head_path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0)
        });

    if from > to {
        return json_error(400, "from must be <= to");
    }

    let mut summaries = Vec::new();
    for rev in from..=to {
        match repo.get_commit_sync(rev) {
            Ok(commit) => {
                let change_count = repo
                    .get_delta_tree(rev)
                    .map(|dt| dt.changes.len())
                    .unwrap_or(0);
                summaries.push(RevisionSummary {
                    rev,
                    author: commit.author,
                    message: commit.message,
                    timestamp: commit.timestamp,
                    change_count,
                });
            }
            Err(_) => {
                // revision doesn't exist, skip
            }
        }
    }

    json_ok(&summaries)
}

// ─────────────────────────────────────────────────────
// GET /sync/objects?id=HEXID&id=HEXID...
// ─────────────────────────────────────────────────────

async fn handle_objects(repo: &Arc<SqliteRepository>, query: &str) -> Response<Full<Bytes>> {
    let ids = parse_query_multi(query, "id");

    if ids.is_empty() {
        return json_error(400, "No object ids specified (use ?id=HEX&id=HEX)");
    }
    if ids.len() > MAX_OBJECTS_PER_REQUEST {
        return json_error(
            400,
            &format!(
                "Too many objects requested ({}, max {})",
                ids.len(),
                MAX_OBJECTS_PER_REQUEST
            ),
        );
    }

    // Parse hex IDs
    let mut parsed_ids = Vec::new();
    for hex_id in &ids {
        match ObjectId::from_hex(hex_id) {
            Ok(oid) => parsed_ids.push(oid),
            Err(_) => return json_error(400, &format!("Invalid object id: {}", hex_id)),
        }
    }

    // Build a multipart-style binary response:
    // For each object: [32 bytes ObjectId] [4 bytes big-endian length] [N bytes data]
    // If object not found: length = 0xFFFFFFFF
    let mut buf = Vec::new();

    for oid in &parsed_ids {
        buf.extend_from_slice(oid.as_bytes());
        match repo.load_object_raw(oid) {
            Ok(data) => {
                let len = data.len() as u32;
                buf.extend_from_slice(&len.to_be_bytes());
                buf.extend_from_slice(&data);
            }
            Err(_) => {
                // sentinel: object not found
                buf.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
            }
        }
    }

    Response::builder()
        .status(200)
        .header("Content-Type", "application/octet-stream")
        .header("X-Object-Count", parsed_ids.len().to_string())
        .body(Full::new(Bytes::from(buf)))
        .unwrap()
}

// ─────────────────────────────────────────────────────
// GET /sync/delta?from=X&to=Y
// ─────────────────────────────────────────────────────

async fn handle_delta(repo: &Arc<SqliteRepository>, query: &str) -> Response<Full<Bytes>> {
    let params = parse_query(query);
    let from: u64 = params.get("from").and_then(|v| v.parse().ok()).unwrap_or(1);
    let to: u64 = params
        .get("to")
        .and_then(|v| v.parse().ok())
        .unwrap_or(from);

    if from > to {
        return json_error(400, "from must be <= to");
    }
    if to - from + 1 > MAX_DELTA_RANGE {
        return json_error(
            400,
            &format!(
                "Range too large ({} revisions, max {})",
                to - from + 1,
                MAX_DELTA_RANGE
            ),
        );
    }

    let mut revisions = Vec::new();
    for rev in from..=to {
        match build_revision_data(repo, rev) {
            Ok(rd) => revisions.push(rd),
            Err(e) => {
                return json_error(
                    404,
                    &format!("Failed to build revision data for r{}: {}", rev, e),
                );
            }
        }
    }

    json_ok(&revisions)
}

// ─────────────────────────────────────────────────────
// GET /sync/config
// ─────────────────────────────────────────────────────

async fn handle_get_config(repo: &Arc<SqliteRepository>) -> Response<Full<Bytes>> {
    match SyncConfig::load(repo.root()) {
        Ok(config) => json_ok(&config),
        Err(e) => json_error(500, &format!("Failed to load sync config: {}", e)),
    }
}

// ─────────────────────────────────────────────────────
// POST /sync/config
// ─────────────────────────────────────────────────────

async fn handle_set_config(repo: &Arc<SqliteRepository>, body: &[u8]) -> Response<Full<Bytes>> {
    let config: SyncConfig = match serde_json::from_slice(body) {
        Ok(c) => c,
        Err(e) => return json_error(400, &format!("Invalid JSON body: {}", e)),
    };

    match config.save(repo.root()) {
        Ok(()) => {
            #[derive(serde::Serialize)]
            struct ConfigSaved {
                ok: bool,
                message: String,
            }
            json_ok(&ConfigSaved {
                ok: true,
                message: "Sync configuration updated".into(),
            })
        }
        Err(e) => json_error(500, &format!("Failed to save sync config: {}", e)),
    }
}

// ─────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────

/// Build a `RevisionData` from the repository for a given revision.
fn build_revision_data(repo: &SqliteRepository, rev: u64) -> anyhow::Result<RevisionData> {
    let commit = repo.get_commit_sync(rev)?;

    let delta_tree = repo.get_delta_tree(rev).unwrap_or_else(|_| {
        dsvn_core::DeltaTree::new(if rev > 0 { rev - 1 } else { 0 }, vec![], 0)
    });

    // Collect blob objects referenced in this revision's changes
    let mut objects = Vec::new();
    for change in &delta_tree.changes {
        if let TreeChange::Upsert { entry, .. } = change {
            if entry.kind == ObjectKind::Blob {
                if let Ok(raw) = repo.load_object_raw(&entry.id) {
                    if let Ok(blob) = Blob::deserialize(&raw) {
                        objects.push((entry.id, blob.data));
                    }
                }
            }
        }
    }

    // Revision properties
    let revprops_path = repo.root().join("revprops").join(format!("{}.json", rev));
    let properties: Vec<(String, String)> = if revprops_path.exists() {
        std::fs::read_to_string(&revprops_path)
            .ok()
            .and_then(|data| serde_json::from_str::<HashMap<String, String>>(&data).ok())
            .map(|m| m.into_iter().collect())
            .unwrap_or_default()
    } else {
        vec![]
    };

    let content_hash = RevisionData::compute_content_hash(&objects);

    // Check if this is an empty commit
    let empty_commit = delta_tree.changes.is_empty();

    Ok(RevisionData {
        revision: rev,
        author: commit.author,
        message: commit.message,
        timestamp: commit.timestamp,
        delta_tree,
        objects,
        properties,
        content_hash,
        empty_commit,
    })
}

/// Parse a query string into a HashMap (last value wins for duplicate keys).
fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(
                urldecode(k),
                urldecode(v),
            );
        }
    }
    map
}

/// Parse all values for a given query parameter key.
fn parse_query_multi(query: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        if let Some((k, v)) = pair.split_once('=') {
            if urldecode(k) == key {
                values.push(urldecode(v));
            }
        }
    }
    values
}

/// Minimal URL percent-decoding.
fn urldecode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().unwrap_or(b'0');
            let lo = chars.next().unwrap_or(b'0');
            let val = hex_val(hi) << 4 | hex_val(lo);
            result.push(val as char);
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

fn json_ok<T: serde::Serialize>(data: &T) -> Response<Full<Bytes>> {
    let body = serde_json::to_vec(data).unwrap_or_default();
    Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

fn json_error(status: u16, message: &str) -> Response<Full<Bytes>> {
    #[derive(serde::Serialize)]
    struct ErrorBody {
        error: String,
    }
    let body = serde_json::to_vec(&ErrorBody {
        error: message.to_string(),
    })
    .unwrap_or_default();
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_query() {
        let q = "from=1&to=10";
        let params = parse_query(q);
        assert_eq!(params.get("from").unwrap(), "1");
        assert_eq!(params.get("to").unwrap(), "10");
    }

    #[test]
    fn test_parse_query_multi() {
        let q = "id=abc123&id=def456&other=1";
        let ids = parse_query_multi(q, "id");
        assert_eq!(ids, vec!["abc123", "def456"]);
    }

    #[test]
    fn test_parse_empty_query() {
        let params = parse_query("");
        assert!(params.is_empty());
        let ids = parse_query_multi("", "id");
        assert!(ids.is_empty());
    }

    #[test]
    fn test_urldecode() {
        assert_eq!(urldecode("hello%20world"), "hello world");
        assert_eq!(urldecode("a+b"), "a b");
        assert_eq!(urldecode("abc"), "abc");
    }

    #[test]
    fn test_sync_config_default() {
        let config = SyncConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_cache_age_hours, 720);
        assert!(!config.require_auth);
        assert_eq!(config.allowed_sources, vec!["*".to_string()]);
    }

    #[test]
    fn test_sync_config_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = SyncConfig {
            enabled: false,
            max_cache_age_hours: 48,
            require_auth: true,
            allowed_sources: vec!["10.0.0.0/8".into()],
            allow_empty: false,
        };
        config.save(tmp.path()).unwrap();
        let loaded = SyncConfig::load(tmp.path()).unwrap();
        assert!(!loaded.enabled);
        assert_eq!(loaded.max_cache_age_hours, 48);
        assert!(loaded.require_auth);
        assert_eq!(loaded.allowed_sources, vec!["10.0.0.0/8".to_string()]);
        assert!(!loaded.allow_empty);
    }
}

//! HTTP remote synchronization client for dsvn.
//!
//! Connects to a dsvn server's /sync endpoints to perform:
//! - Metadata-first sync (fetch revision list, then on-demand objects)
//! - Object deduplication (skip objects already in local repository)
//! - Incremental pull from remote to local repository

use anyhow::{anyhow, Context, Result};
use dsvn_core::replication::RevisionData;
use dsvn_core::sync::{RevisionSummary, SyncConfig, SyncEndpointInfo};
use dsvn_core::{
    Blob, ObjectId, ObjectKind, SqliteRepository, SyncState, TreeChange,
    ReplicationLog, ReplicationLogEntry,
};
use std::path::{Path, PathBuf};

/// HTTP sync client for pulling from a remote dsvn server.
pub struct RemoteSyncClient {
    base_url: String,
    http: reqwest::Client,
}

impl RemoteSyncClient {
    /// Create a new client targeting `base_url` (e.g. `http://server:8080`).
    pub fn new(base_url: &str) -> Self {
        let url = base_url.trim_end_matches('/').to_string();
        Self {
            base_url: url,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// GET /sync/info
    pub async fn get_info(&self) -> Result<SyncEndpointInfo> {
        let url = format!("{}/sync/info", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to connect to {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("GET /sync/info failed ({}): {}", status, body));
        }
        resp.json()
            .await
            .context("Failed to parse /sync/info response")
    }

    /// GET /sync/revs?from=X&to=Y
    pub async fn get_revisions(&self, from: u64, to: u64) -> Result<Vec<RevisionSummary>> {
        let url = format!("{}/sync/revs?from={}&to={}", self.base_url, from, to);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch revisions from {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("GET /sync/revs failed ({}): {}", status, body));
        }
        resp.json()
            .await
            .context("Failed to parse /sync/revs response")
    }

    /// GET /sync/delta?from=X&to=Y — fetch full revision data with objects.
    pub async fn get_delta(&self, from: u64, to: u64) -> Result<Vec<RevisionData>> {
        let url = format!("{}/sync/delta?from={}&to={}", self.base_url, from, to);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch delta from {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("GET /sync/delta failed ({}): {}", status, body));
        }
        resp.json()
            .await
            .context("Failed to parse /sync/delta response")
    }

    /// GET /sync/objects?id=...&id=... — fetch raw object data in batch.
    /// Returns `(ObjectId, Option<Vec<u8>>)` for each requested ID.
    pub async fn get_objects(
        &self,
        ids: &[ObjectId],
    ) -> Result<Vec<(ObjectId, Option<Vec<u8>>)>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let mut url = format!("{}/sync/objects", self.base_url);
        for (i, id) in ids.iter().enumerate() {
            if i == 0 {
                url.push('?');
            } else {
                url.push('&');
            }
            url.push_str(&format!("id={}", id.to_hex()));
        }

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| "Failed to fetch objects")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("GET /sync/objects failed ({}): {}", status, body));
        }

        let data = resp.bytes().await?.to_vec();

        // Parse binary response: [32B id][4B len][N bytes data]...
        let mut results = Vec::new();
        let mut pos = 0;
        while pos + 36 <= data.len() {
            let mut id_bytes = [0u8; 32];
            id_bytes.copy_from_slice(&data[pos..pos + 32]);
            let oid = ObjectId::new(id_bytes);
            pos += 32;

            let len =
                u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
            pos += 4;

            if len == 0xFFFF_FFFF {
                results.push((oid, None));
            } else {
                let end = pos + len as usize;
                if end > data.len() {
                    return Err(anyhow!("Truncated object data"));
                }
                results.push((oid, Some(data[pos..end].to_vec())));
                pos = end;
            }
        }

        Ok(results)
    }

    /// GET /sync/config
    pub async fn get_config(&self) -> Result<SyncConfig> {
        let url = format!("{}/sync/config", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| "Failed to fetch sync config")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("GET /sync/config failed ({}): {}", status, body));
        }
        resp.json().await.context("Failed to parse sync config")
    }
}

/// Pull from a remote server to a local repository.
pub struct RemotePull {
    client: RemoteSyncClient,
    dest_path: PathBuf,
}

impl RemotePull {
    pub fn new(source_url: &str, dest_path: &Path) -> Self {
        Self {
            client: RemoteSyncClient::new(source_url),
            dest_path: dest_path.to_path_buf(),
        }
    }

    /// Initialize a sync relationship with remote server.
    pub async fn init(&self) -> Result<SyncState> {
        // Check if already initialized
        if let Some(existing) = SyncState::load(&self.dest_path)? {
            return Err(anyhow!(
                "Destination already has sync state (source: {}). Use cleanup first.",
                existing.source_url
            ));
        }

        let info = self.client.get_info().await?;

        let mut state = SyncState::new(info.uuid, self.client.base_url.clone());
        state.source_head_rev = info.head_rev;
        state.protocol_version = info.protocol_version;
        state.save(&self.dest_path)?;

        Ok(state)
    }

    /// Perform an incremental pull from remote server.
    /// Uses metadata-first approach: fetch rev list, then only needed objects.
    /// Objects are deduplicated against the local repository's objects directory.
    pub async fn pull(&self) -> Result<PullResult> {
        let start_time = std::time::Instant::now();

        let dest_repo = SqliteRepository::open(&self.dest_path)?;
        dest_repo.initialize().await?;

        let mut state = SyncState::load(&self.dest_path)?
            .ok_or_else(|| anyhow!("Sync not initialized. Run init first."))?;

        // Get remote info
        let info = self.client.get_info().await?;
        state.verify_source(&info.uuid)?;

        let from_rev = state.effective_start_rev() + 1;
        let source_head = info.head_rev;

        if from_rev > source_head {
            return Ok(PullResult {
                from_rev: 0,
                to_rev: 0,
                revisions_synced: 0,
                objects_transferred: 0,
                objects_cached: 0,
                bytes_transferred: 0,
                duration_ms: start_time.elapsed().as_millis() as u64,
                already_up_to_date: true,
            });
        }

        state.source_head_rev = source_head;
        state.begin_sync(&self.dest_path)?;

        let repl_log = ReplicationLog::new(&self.dest_path);
        let mut total_objects = 0u64;
        let mut cached_objects = 0u64;
        let mut total_bytes = 0u64;
        let mut revisions_synced = 0u64;

        // Process in batches of up to 100 revisions
        let batch_size = 100u64;
        let mut current = from_rev;

        dest_repo.begin_batch();

        while current <= source_head {
            let batch_end = (current + batch_size - 1).min(source_head);

            // Step 1: Fetch metadata to know what objects we need
            let revisions = self.client.get_revisions(current, batch_end).await?;
            if revisions.is_empty() {
                break;
            }

            // Step 2: Fetch full delta (includes objects)
            let rev_data_list = self.client.get_delta(current, batch_end).await?;

            for rev_data in &rev_data_list {
                // Verify content hash
                if !rev_data.verify_content_hash() {
                    dest_repo.end_batch();
                    return Err(anyhow!(
                        "Content hash mismatch for revision {}",
                        rev_data.revision
                    ));
                }

                // Store objects (with dedup against local repository)
                for (id, data) in &rev_data.objects {
                    let hex = id.to_hex();
                    let obj_path = dest_repo
                        .root()
                        .join("objects")
                        .join(&hex[..2])
                        .join(&hex[2..]);

                    if obj_path.exists() {
                        cached_objects += 1;
                        continue;
                    }

                    // Store new object
                    if let Some(parent) = obj_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let blob = Blob::new(data.clone(), false);
                    std::fs::write(&obj_path, blob.to_bytes()?)?;

                    total_bytes += data.len() as u64;
                    total_objects += 1;
                }

                // Apply tree changes
                for change in &rev_data.delta_tree.changes {
                    match change {
                        TreeChange::Upsert { path, entry } => {
                            if entry.kind == ObjectKind::Blob {
                                if let Some((_, data)) =
                                    rev_data.objects.iter().find(|(oid, _)| *oid == entry.id)
                                {
                                    dest_repo.add_file_sync(
                                        path,
                                        data.clone(),
                                        entry.mode == 0o755,
                                    )?;
                                }
                            } else {
                                dest_repo.mkdir_sync(path)?;
                            }
                        }
                        TreeChange::Delete { path } => {
                            dest_repo.delete_file_sync(path)?;
                        }
                    }
                }

                // Commit
                dest_repo.commit_sync(
                    rev_data.author.clone(),
                    rev_data.message.clone(),
                    rev_data.timestamp,
                )?;

                // Save revprops
                if !rev_data.properties.is_empty() {
                    let props_dir = dest_repo.root().join("revprops");
                    std::fs::create_dir_all(&props_dir)?;
                    let props_map: std::collections::HashMap<String, String> =
                        rev_data.properties.iter().cloned().collect();
                    let props_path = props_dir.join(format!("{}.json", rev_data.revision));
                    std::fs::write(&props_path, serde_json::to_string_pretty(&props_map)?)?;
                }

                revisions_synced += 1;
            }

            // Update checkpoint
            state.set_checkpoint(&self.dest_path, batch_end)?;
            current = batch_end + 1;
        }

        dest_repo.end_batch();

        let duration_ms = start_time.elapsed().as_millis() as u64;

        // Log sync
        repl_log.append(&ReplicationLogEntry {
            from_rev,
            to_rev: source_head,
            timestamp: chrono::Utc::now().timestamp(),
            objects_transferred: total_objects,
            bytes_transferred: total_bytes,
            duration_ms,
            success: true,
            error: None,
        })?;

        state.complete_sync(&self.dest_path, source_head)?;

        Ok(PullResult {
            from_rev,
            to_rev: source_head,
            revisions_synced,
            objects_transferred: total_objects,
            objects_cached: cached_objects,
            bytes_transferred: total_bytes,
            duration_ms,
            already_up_to_date: false,
        })
    }
}

/// Result of a remote pull operation.
#[derive(Debug)]
pub struct PullResult {
    pub from_rev: u64,
    pub to_rev: u64,
    pub revisions_synced: u64,
    pub objects_transferred: u64,
    pub objects_cached: u64,
    pub bytes_transferred: u64,
    pub duration_ms: u64,
    pub already_up_to_date: bool,
}

/// Fetch missing objects from remote and store them in local repository.
/// Returns the number of objects fetched.
pub async fn fetch_objects_and_repair(
    source_url: &str,
    dest_repo: &SqliteRepository,
    object_ids: &[ObjectId],
) -> Result<RepairResult> {
    if object_ids.is_empty() {
        return Ok(RepairResult {
            objects_fetched: 0,
            bytes_fetched: 0,
            objects_already_present: 0,
        });
    }

    let client = RemoteSyncClient::new(source_url);
    let start_time = std::time::Instant::now();

    // Filter out objects that already exist locally
    let mut ids_to_fetch = Vec::new();
    let mut already_present = 0u64;

    for id in object_ids {
        let hex = id.to_hex();
        let obj_path = dest_repo
            .root()
            .join("objects")
            .join(&hex[..2])
            .join(&hex[2..]);

        if obj_path.exists() {
            already_present += 1;
        } else {
            ids_to_fetch.push(*id);
        }
    }

    if ids_to_fetch.is_empty() {
        return Ok(RepairResult {
            objects_fetched: 0,
            bytes_fetched: 0,
            objects_already_present: already_present,
        });
    }

    // Fetch from remote
    let fetched_objects = client.get_objects(&ids_to_fetch).await?;

    let mut objects_fetched = 0u64;
    let mut bytes_fetched = 0u64;

    for (id, data_opt) in fetched_objects {
        if let Some(data) = data_opt {
            let hex = id.to_hex();
            let obj_path = dest_repo
                .root()
                .join("objects")
                .join(&hex[..2])
                .join(&hex[2..]);

            // Ensure parent directory exists
            if let Some(parent) = obj_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Store blob
            let blob = Blob::new(data.clone(), false);
            std::fs::write(&obj_path, blob.to_bytes()?)?;

            objects_fetched += 1;
            bytes_fetched += data.len() as u64;
        }
    }

    Ok(RepairResult {
        objects_fetched,
        bytes_fetched,
        objects_already_present: already_present,
    })
}

/// Result of a repair operation.
#[derive(Debug)]
pub struct RepairResult {
    pub objects_fetched: u64,
    pub bytes_fetched: u64,
    pub objects_already_present: u64,
}

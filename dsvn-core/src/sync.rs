//! Sync state management for dsvn repository replication.
//!
//! Tracks the synchronization state between a source (master) and destination
//! (slave) repository. Stores metadata like last synced revision, source UUID,
//! and sync timestamps.
//!
//! Also provides `SyncInfo` and `SyncConfig` types used by the HTTP /sync endpoints
//! for server-to-server replication with on-demand object fetching.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────
// HTTP sync endpoint types (used by /sync/*)
// ─────────────────────────────────────────────────────

/// Information about a repository for the /sync/info endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEndpointInfo {
    /// Repository UUID.
    pub uuid: String,
    /// Current HEAD revision.
    pub head_rev: u64,
    /// Protocol version for the sync API.
    pub protocol_version: u32,
    /// Capabilities advertised by the server.
    pub capabilities: Vec<String>,
}

/// Sync configuration stored at `repo/sync-config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Whether sync endpoints are enabled.
    pub enabled: bool,
    /// Optional cache directory for fetched objects.
    pub cache_dir: Option<PathBuf>,
    /// Maximum age (in hours) for cached objects before eviction.
    pub max_cache_age_hours: u32,
    /// Whether authentication is required for sync endpoints.
    pub require_auth: bool,
    /// Allowed source patterns (`["*"]` means allow all).
    #[serde(default = "default_allowed_sources")]
    pub allowed_sources: Vec<String>,
}

fn default_allowed_sources() -> Vec<String> {
    vec!["*".to_string()]
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cache_dir: None,
            max_cache_age_hours: 720, // 30 days
            require_auth: false,
            allowed_sources: default_allowed_sources(),
        }
    }
}

impl SyncConfig {
    /// Load sync config from a repository path.
    pub fn load(repo_path: &Path) -> Result<Self> {
        let config_path = repo_path.join("sync-config.json");
        if !config_path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read sync config from {:?}", config_path))?;
        let config: SyncConfig = serde_json::from_str(&data)
            .with_context(|| "Failed to parse sync config JSON")?;
        Ok(config)
    }

    /// Save sync config to a repository path.
    pub fn save(&self, repo_path: &Path) -> Result<()> {
        let config_path = repo_path.join("sync-config.json");
        let tmp_path = config_path.with_extension("tmp");
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&tmp_path, &data)?;
        fs::rename(&tmp_path, &config_path)?;
        Ok(())
    }
}

/// Summary of a single revision for the /sync/revs endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionSummary {
    pub rev: u64,
    pub author: String,
    pub message: String,
    pub timestamp: i64,
    /// Number of changes (adds + deletes) in this revision.
    pub change_count: usize,
}

/// Synchronization state persisted in the destination repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    /// UUID of the source repository.
    pub source_uuid: String,
    /// URL/path of the source repository.
    pub source_url: String,
    /// Last revision successfully synced from the source.
    pub last_synced_rev: u64,
    /// Last revision available at the source (as of last check).
    pub source_head_rev: u64,
    /// Timestamp of the last successful sync (Unix seconds).
    pub last_sync_timestamp: i64,
    /// Number of revisions synced in total.
    pub total_synced_revisions: u64,
    /// Whether the sync is currently in progress (for crash recovery).
    pub sync_in_progress: bool,
    /// Protocol version used for sync.
    pub protocol_version: u32,
    /// Optional: checkpoint revision for resumable sync.
    pub checkpoint_rev: Option<u64>,
}

impl SyncState {
    /// Create a new SyncState for initial sync setup.
    pub fn new(source_uuid: String, source_url: String) -> Self {
        Self {
            source_uuid,
            source_url,
            last_synced_rev: 0,
            source_head_rev: 0,
            last_sync_timestamp: 0,
            total_synced_revisions: 0,
            sync_in_progress: false,
            protocol_version: 1,
            checkpoint_rev: None,
        }
    }

    /// Load sync state from a repository path.
    pub fn load(repo_path: &Path) -> Result<Option<Self>> {
        let state_path = Self::state_file_path(repo_path);
        if !state_path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&state_path)
            .with_context(|| format!("Failed to read sync state from {:?}", state_path))?;
        let state: SyncState = serde_json::from_str(&data)
            .with_context(|| "Failed to parse sync state JSON")?;
        Ok(Some(state))
    }

    /// Save sync state to a repository path.
    pub fn save(&self, repo_path: &Path) -> Result<()> {
        let state_path = Self::state_file_path(repo_path);
        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = state_path.with_extension("tmp");
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&tmp_path, &data)?;
        fs::rename(&tmp_path, &state_path)?;
        Ok(())
    }

    /// Remove sync state from a repository path.
    pub fn remove(repo_path: &Path) -> Result<()> {
        let state_path = Self::state_file_path(repo_path);
        if state_path.exists() {
            fs::remove_file(&state_path)?;
        }
        // Also remove replication log
        let log_dir = repo_path.join("repl-log");
        if log_dir.exists() {
            fs::remove_dir_all(&log_dir)?;
        }
        Ok(())
    }

    /// Mark sync as in-progress (for crash recovery).
    pub fn begin_sync(&mut self, repo_path: &Path) -> Result<()> {
        self.sync_in_progress = true;
        self.save(repo_path)
    }

    /// Mark sync as completed with the new revision.
    pub fn complete_sync(&mut self, repo_path: &Path, synced_rev: u64) -> Result<()> {
        self.last_synced_rev = synced_rev;
        self.total_synced_revisions += 1;
        self.last_sync_timestamp = chrono::Utc::now().timestamp();
        self.sync_in_progress = false;
        self.checkpoint_rev = None;
        self.save(repo_path)
    }

    /// Set a checkpoint for resumable sync.
    pub fn set_checkpoint(&mut self, repo_path: &Path, rev: u64) -> Result<()> {
        self.checkpoint_rev = Some(rev);
        self.save(repo_path)
    }

    /// Get the effective start revision for syncing (considers checkpoints).
    pub fn effective_start_rev(&self) -> u64 {
        self.checkpoint_rev.unwrap_or(self.last_synced_rev)
    }

    /// Check if the source UUID matches.
    pub fn verify_source(&self, uuid: &str) -> Result<()> {
        if self.source_uuid != uuid {
            return Err(anyhow!(
                "Source UUID mismatch: expected {}, got {}. \
                 The source repository may have been recreated.",
                self.source_uuid,
                uuid
            ));
        }
        Ok(())
    }

    fn state_file_path(repo_path: &Path) -> PathBuf {
        repo_path.join("sync-state.json")
    }
}

/// SVN-compatible sync properties stored on revision 0.
pub mod svn_sync_props {
    /// The URL of the source repository.
    pub const SYNC_FROM_URL: &str = "svn:sync-from-url";
    /// The UUID of the source repository.
    pub const SYNC_FROM_UUID: &str = "svn:sync-from-uuid";
    /// The last merged revision.
    pub const SYNC_LAST_MERGED_REV: &str = "svn:sync-last-merged-rev";
    /// Lock token (prevents concurrent syncs).
    pub const SYNC_LOCK: &str = "svn:sync-lock";
    /// Currently syncing revision.
    pub const SYNC_CURRENTLY_COPYING: &str = "svn:sync-currently-copying";
}

/// Replication log entry — records a completed sync operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationLogEntry {
    /// Revision range start (inclusive).
    pub from_rev: u64,
    /// Revision range end (inclusive).
    pub to_rev: u64,
    /// Timestamp when the sync occurred.
    pub timestamp: i64,
    /// Number of objects transferred.
    pub objects_transferred: u64,
    /// Total bytes transferred (compressed).
    pub bytes_transferred: u64,
    /// Duration of the sync in milliseconds.
    pub duration_ms: u64,
    /// Whether the sync completed successfully.
    pub success: bool,
    /// Error message if the sync failed.
    pub error: Option<String>,
}

/// Manages the replication log for a repository.
pub struct ReplicationLog {
    log_dir: PathBuf,
}

impl ReplicationLog {
    /// Create a new ReplicationLog for the given repository.
    pub fn new(repo_path: &Path) -> Self {
        Self {
            log_dir: repo_path.join("repl-log"),
        }
    }

    /// Ensure the log directory exists.
    pub fn ensure_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.log_dir)?;
        Ok(())
    }

    /// Append a log entry.
    pub fn append(&self, entry: &ReplicationLogEntry) -> Result<()> {
        self.ensure_dir()?;
        let filename = format!("{}_{}.json", entry.from_rev, entry.to_rev);
        let path = self.log_dir.join(&filename);
        let data = serde_json::to_string_pretty(entry)?;
        fs::write(&path, data)?;
        Ok(())
    }

    /// Query log entries in a revision range.
    pub fn query(&self, from_rev: u64, to_rev: u64) -> Result<Vec<ReplicationLogEntry>> {
        if !self.log_dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.log_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(data) = fs::read_to_string(&path) {
                    if let Ok(log_entry) = serde_json::from_str::<ReplicationLogEntry>(&data) {
                        // Include if the entry overlaps with the requested range
                        if log_entry.to_rev >= from_rev && log_entry.from_rev <= to_rev {
                            entries.push(log_entry);
                        }
                    }
                }
            }
        }
        entries.sort_by_key(|e| e.from_rev);
        Ok(entries)
    }

    /// Get all log entries.
    pub fn all(&self) -> Result<Vec<ReplicationLogEntry>> {
        self.query(0, u64::MAX)
    }

    /// Get the latest log entry.
    pub fn latest(&self) -> Result<Option<ReplicationLogEntry>> {
        let entries = self.all()?;
        Ok(entries.into_iter().last())
    }

    /// Clean up log entries older than a given revision.
    pub fn cleanup_before(&self, rev: u64) -> Result<u64> {
        if !self.log_dir.exists() {
            return Ok(0);
        }
        let mut removed = 0u64;
        for entry in fs::read_dir(&self.log_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(data) = fs::read_to_string(&path) {
                    if let Ok(log_entry) = serde_json::from_str::<ReplicationLogEntry>(&data) {
                        if log_entry.to_rev < rev {
                            fs::remove_file(&path)?;
                            removed += 1;
                        }
                    }
                }
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_sync_state_new() {
        let state = SyncState::new(
            "test-uuid".to_string(),
            "file:///repo".to_string(),
        );
        assert_eq!(state.source_uuid, "test-uuid");
        assert_eq!(state.source_url, "file:///repo");
        assert_eq!(state.last_synced_rev, 0);
        assert!(!state.sync_in_progress);
    }

    #[test]
    fn test_sync_state_save_load() {
        let tmp = TempDir::new().unwrap();
        let state = SyncState::new(
            "uuid-123".to_string(),
            "file:///source".to_string(),
        );
        state.save(tmp.path()).unwrap();

        let loaded = SyncState::load(tmp.path()).unwrap().unwrap();
        assert_eq!(loaded.source_uuid, "uuid-123");
        assert_eq!(loaded.source_url, "file:///source");
    }

    #[test]
    fn test_sync_state_load_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let result = SyncState::load(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_sync_state_remove() {
        let tmp = TempDir::new().unwrap();
        let state = SyncState::new("uuid".to_string(), "url".to_string());
        state.save(tmp.path()).unwrap();
        assert!(tmp.path().join("sync-state.json").exists());

        SyncState::remove(tmp.path()).unwrap();
        assert!(!tmp.path().join("sync-state.json").exists());
    }

    #[test]
    fn test_sync_state_begin_complete() {
        let tmp = TempDir::new().unwrap();
        let mut state = SyncState::new("uuid".to_string(), "url".to_string());

        state.begin_sync(tmp.path()).unwrap();
        let loaded = SyncState::load(tmp.path()).unwrap().unwrap();
        assert!(loaded.sync_in_progress);

        state.complete_sync(tmp.path(), 5).unwrap();
        let loaded = SyncState::load(tmp.path()).unwrap().unwrap();
        assert!(!loaded.sync_in_progress);
        assert_eq!(loaded.last_synced_rev, 5);
        assert_eq!(loaded.total_synced_revisions, 1);
    }

    #[test]
    fn test_sync_state_checkpoint() {
        let tmp = TempDir::new().unwrap();
        let mut state = SyncState::new("uuid".to_string(), "url".to_string());
        state.last_synced_rev = 10;

        assert_eq!(state.effective_start_rev(), 10);

        state.set_checkpoint(tmp.path(), 15).unwrap();
        assert_eq!(state.effective_start_rev(), 15);
    }

    #[test]
    fn test_sync_state_verify_source() {
        let state = SyncState::new("uuid-abc".to_string(), "url".to_string());
        assert!(state.verify_source("uuid-abc").is_ok());
        assert!(state.verify_source("uuid-xyz").is_err());
    }

    #[test]
    fn test_replication_log_append_query() {
        let tmp = TempDir::new().unwrap();
        let log = ReplicationLog::new(tmp.path());

        let entry1 = ReplicationLogEntry {
            from_rev: 1,
            to_rev: 5,
            timestamp: 1000,
            objects_transferred: 10,
            bytes_transferred: 4096,
            duration_ms: 500,
            success: true,
            error: None,
        };
        log.append(&entry1).unwrap();

        let entry2 = ReplicationLogEntry {
            from_rev: 6,
            to_rev: 10,
            timestamp: 2000,
            objects_transferred: 8,
            bytes_transferred: 3000,
            duration_ms: 300,
            success: true,
            error: None,
        };
        log.append(&entry2).unwrap();

        let all = log.all().unwrap();
        assert_eq!(all.len(), 2);

        let queried = log.query(4, 7).unwrap();
        assert_eq!(queried.len(), 2); // Both entries overlap with [4, 7]

        let queried = log.query(7, 8).unwrap();
        assert_eq!(queried.len(), 1);
        assert_eq!(queried[0].from_rev, 6);
    }

    #[test]
    fn test_replication_log_latest() {
        let tmp = TempDir::new().unwrap();
        let log = ReplicationLog::new(tmp.path());

        assert!(log.latest().unwrap().is_none());

        log.append(&ReplicationLogEntry {
            from_rev: 1, to_rev: 5, timestamp: 1000,
            objects_transferred: 0, bytes_transferred: 0, duration_ms: 0,
            success: true, error: None,
        }).unwrap();

        log.append(&ReplicationLogEntry {
            from_rev: 6, to_rev: 10, timestamp: 2000,
            objects_transferred: 0, bytes_transferred: 0, duration_ms: 0,
            success: true, error: None,
        }).unwrap();

        let latest = log.latest().unwrap().unwrap();
        assert_eq!(latest.from_rev, 6);
        assert_eq!(latest.to_rev, 10);
    }

    #[test]
    fn test_replication_log_cleanup() {
        let tmp = TempDir::new().unwrap();
        let log = ReplicationLog::new(tmp.path());

        for i in 0..5 {
            log.append(&ReplicationLogEntry {
                from_rev: i * 10 + 1, to_rev: (i + 1) * 10,
                timestamp: 1000 + i as i64,
                objects_transferred: 0, bytes_transferred: 0, duration_ms: 0,
                success: true, error: None,
            }).unwrap();
        }

        assert_eq!(log.all().unwrap().len(), 5);
        let removed = log.cleanup_before(25).unwrap();
        assert_eq!(removed, 2); // entries [1,10] and [11,20] are before rev 25
        assert_eq!(log.all().unwrap().len(), 3);
    }
}

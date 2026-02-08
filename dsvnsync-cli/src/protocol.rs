//! dsvnsync — Synchronization protocol definitions for dsvn.
//!
//! Re-exports core protocol types and adds higher-level sync operations
//! for the CLI tool.

pub use dsvn_core::replication::*;
pub use dsvn_core::sync::*;

use anyhow::{anyhow, Result};
use dsvn_core::{
    Blob, DeltaTree, ObjectKind, SqliteRepository, TreeChange,
};
use std::collections::HashMap;

/// Extract RevisionData for a specific revision from a repository.
/// This is the core function that prepares data for transfer.
pub fn extract_revision_data(
    repo: &SqliteRepository,
    rev: u64,
) -> Result<RevisionData> {
    let rt = tokio::runtime::Handle::current();
    let commit = rt
        .block_on(repo.get_commit(rev))
        .ok_or_else(|| anyhow!("Commit for revision {} not found", rev))?;

    // Load delta tree
    let delta_path = repo.root().join("tree_deltas").join(format!("{}.bin", rev));
    let delta_tree = if delta_path.exists() {
        let data = std::fs::read(&delta_path)?;
        bincode::deserialize::<DeltaTree>(&data)?
    } else {
        // Fallback: compute delta from tree diff
        DeltaTree::new(if rev > 0 { rev - 1 } else { 0 }, vec![], 0)
    };

    // Collect all blob objects referenced in this revision's changes
    let mut objects = Vec::new();
    for change in &delta_tree.changes {
        match change {
            TreeChange::Upsert { path, entry } => {
                if entry.kind == ObjectKind::Blob {
                    if let Ok(content) = rt.block_on(repo.get_file(&format!("/{}", path), rev)) {
                        objects.push((entry.id, content.to_vec()));
                    }
                }
            }
            TreeChange::Delete { .. } => {
                // No objects needed for deletes
            }
        }
    }

    // Load revision properties
    let revprops_path = repo.root().join("revprops").join(format!("{}.json", rev));
    let properties: Vec<(String, String)> = if revprops_path.exists() {
        let data = std::fs::read_to_string(&revprops_path)?;
        let map: HashMap<String, String> = serde_json::from_str(&data)?;
        map.into_iter().collect()
    } else {
        vec![]
    };

    let content_hash = RevisionData::compute_content_hash(&objects);

    Ok(RevisionData {
        revision: rev,
        author: commit.author.clone(),
        message: commit.message.clone(),
        timestamp: commit.timestamp,
        delta_tree,
        objects,
        properties,
        content_hash,
    })
}

/// Apply a RevisionData to a destination repository (replay a revision).
pub fn apply_revision_data(
    repo: &SqliteRepository,
    rev_data: &RevisionData,
) -> Result<u64> {
    // Verify content hash
    if !rev_data.verify_content_hash() {
        return Err(anyhow!(
            "Content hash verification failed for revision {}",
            rev_data.revision
        ));
    }

    // Store all objects first
    for (id, data) in &rev_data.objects {
        let obj_path = {
            let hex = id.to_hex();
            repo.root()
                .join("objects")
                .join(&hex[..2])
                .join(&hex[2..])
        };
        if !obj_path.exists() {
            if let Some(parent) = obj_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Store as blob
            let blob = Blob::new(data.clone(), false);
            std::fs::write(&obj_path, blob.to_bytes()?)?;
        }
    }

    // Apply tree changes
    for change in &rev_data.delta_tree.changes {
        match change {
            TreeChange::Upsert { path, entry } => {
                if entry.kind == ObjectKind::Blob {
                    // Find the object data
                    if let Some((_, data)) = rev_data.objects.iter().find(|(id, _)| *id == entry.id)
                    {
                        repo.add_file_sync(path, data.clone(), entry.mode == 0o755)?;
                    } else {
                        // Object might already exist in dest repo
                        repo.add_file_sync(path, vec![], entry.mode == 0o755)?;
                    }
                } else {
                    repo.mkdir_sync(path)?;
                }
            }
            TreeChange::Delete { path } => {
                repo.delete_file_sync(path)?;
            }
        }
    }

    // Commit with original metadata
    let new_rev = repo.commit_sync(
        rev_data.author.clone(),
        rev_data.message.clone(),
        rev_data.timestamp,
    )?;

    // Save revision properties
    if !rev_data.properties.is_empty() {
        let props_dir = repo.root().join("revprops");
        std::fs::create_dir_all(&props_dir)?;
        let props_map: HashMap<String, String> = rev_data.properties.iter().cloned().collect();
        let props_path = props_dir.join(format!("{}.json", new_rev));
        std::fs::write(&props_path, serde_json::to_string_pretty(&props_map)?)?;
    }

    Ok(new_rev)
}

/// Perform a full sync between source and destination repositories (local-to-local).
pub struct LocalSync<'a> {
    pub source: &'a SqliteRepository,
    pub dest: &'a SqliteRepository,
}

impl<'a> LocalSync<'a> {
    pub fn new(source: &'a SqliteRepository, dest: &'a SqliteRepository) -> Self {
        Self { source, dest }
    }

    /// Initialize sync: set up sync state on the destination.
    pub fn init(&self) -> Result<SyncState> {
        let rt = tokio::runtime::Handle::current();

        // Check if already initialized
        if let Some(existing) = SyncState::load(self.dest.root())? {
            return Err(anyhow!(
                "Destination already has sync state (source: {}). Use cleanup first.",
                existing.source_url
            ));
        }

        let source_uuid = self.source.uuid().to_string();
        let source_url = format!("file://{}", self.source.root().display());
        let source_head = rt.block_on(self.source.current_rev());

        let mut state = SyncState::new(source_uuid, source_url);
        state.source_head_rev = source_head;
        state.save(self.dest.root())?;

        // Set SVN-compatible sync properties
        let dest_props = self.dest.property_store();
        rt.block_on(dest_props.set(
            "/:rev0".to_string(),
            dsvn_core::sync::svn_sync_props::SYNC_FROM_URL.to_string(),
            format!("file://{}", self.source.root().display()),
        ))?;
        rt.block_on(dest_props.set(
            "/:rev0".to_string(),
            dsvn_core::sync::svn_sync_props::SYNC_FROM_UUID.to_string(),
            self.source.uuid().to_string(),
        ))?;

        Ok(state)
    }

    /// Perform incremental sync.
    pub fn sync(&self) -> Result<SyncResult> {
        let rt = tokio::runtime::Handle::current();
        let start_time = std::time::Instant::now();

        let mut state = SyncState::load(self.dest.root())?
            .ok_or_else(|| anyhow!("Sync not initialized. Run 'init' first."))?;

        // Verify source UUID
        state.verify_source(self.source.uuid())?;

        let source_head = rt.block_on(self.source.current_rev());
        let _dest_rev = rt.block_on(self.dest.current_rev());
        let from_rev = state.effective_start_rev() + 1;

        if from_rev > source_head {
            return Ok(SyncResult {
                from_rev: 0,
                to_rev: 0,
                revisions_synced: 0,
                objects_transferred: 0,
                bytes_transferred: 0,
                duration_ms: start_time.elapsed().as_millis() as u64,
                already_up_to_date: true,
            });
        }

        state.source_head_rev = source_head;
        state.begin_sync(self.dest.root())?;

        let repl_log = ReplicationLog::new(self.dest.root());
        let mut total_objects = 0u64;
        let mut total_bytes = 0u64;
        let mut revisions_synced = 0u64;

        // Begin batch mode for efficient writes
        self.dest.begin_batch();

        for rev in from_rev..=source_head {
            // Extract revision data from source
            let rev_data = extract_revision_data(self.source, rev)?;

            // Track transfer stats
            for (_, data) in &rev_data.objects {
                total_bytes += data.len() as u64;
                total_objects += 1;
            }

            // Apply to destination
            apply_revision_data(self.dest, &rev_data)?;
            revisions_synced += 1;

            // Update checkpoint every 100 revisions
            if revisions_synced % 100 == 0 {
                state.set_checkpoint(self.dest.root(), rev)?;
            }
        }

        self.dest.end_batch();

        let duration_ms = start_time.elapsed().as_millis() as u64;

        // Log the sync operation
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

        // Update sync state
        state.complete_sync(self.dest.root(), source_head)?;

        Ok(SyncResult {
            from_rev,
            to_rev: source_head,
            revisions_synced,
            objects_transferred: total_objects,
            bytes_transferred: total_bytes,
            duration_ms,
            already_up_to_date: false,
        })
    }

    /// Get sync information.
    pub fn info(&self) -> Result<SyncInfo> {
        let rt = tokio::runtime::Handle::current();
        let state = SyncState::load(self.dest.root())?;
        let repl_log = ReplicationLog::new(self.dest.root());
        let latest_entry = repl_log.latest()?;
        let dest_rev = rt.block_on(self.dest.current_rev());

        Ok(SyncInfo {
            state,
            dest_current_rev: dest_rev,
            latest_repl_entry: latest_entry,
        })
    }
}

/// Result of a sync operation.
#[derive(Debug)]
pub struct SyncResult {
    pub from_rev: u64,
    pub to_rev: u64,
    pub revisions_synced: u64,
    pub objects_transferred: u64,
    pub bytes_transferred: u64,
    pub duration_ms: u64,
    pub already_up_to_date: bool,
}

/// Sync information summary.
#[derive(Debug)]
pub struct SyncInfo {
    pub state: Option<SyncState>,
    pub dest_current_rev: u64,
    pub latest_repl_entry: Option<ReplicationLogEntry>,
}

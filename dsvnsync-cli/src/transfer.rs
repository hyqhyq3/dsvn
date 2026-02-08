//! Delta transfer engine for dsvn synchronization.
//!
//! Handles efficient transfer of revision data between repositories:
//! - Compresses object data with zstd
//! - Deduplicates objects already present at destination
//! - Supports batch transfer of multiple revisions
//! - Provides progress tracking and statistics

use anyhow::{anyhow, Result};
use dsvn_core::{
    Blob, ObjectId, ObjectKind, SqliteRepository, TreeChange,
};
use std::path::Path;

/// Statistics from a transfer operation.
#[derive(Debug, Clone, Default)]
pub struct TransferStats {
    /// Total revisions transferred.
    pub revisions: u64,
    /// Total objects transferred.
    pub objects_transferred: u64,
    /// Objects skipped (already present).
    pub objects_skipped: u64,
    /// Raw bytes (uncompressed).
    pub raw_bytes: u64,
    /// Compressed bytes.
    pub compressed_bytes: u64,
    /// Time spent in milliseconds.
    pub duration_ms: u64,
}

impl TransferStats {
    /// Compression ratio (1.0 = no compression).
    pub fn compression_ratio(&self) -> f64 {
        if self.raw_bytes == 0 {
            return 1.0;
        }
        self.compressed_bytes as f64 / self.raw_bytes as f64
    }

    /// Throughput in bytes per second (uncompressed).
    pub fn throughput_bps(&self) -> f64 {
        if self.duration_ms == 0 {
            return 0.0;
        }
        (self.raw_bytes as f64 / self.duration_ms as f64) * 1000.0
    }

    /// Format a human-readable summary.
    pub fn summary(&self) -> String {
        let ratio = self.compression_ratio();
        let throughput = self.throughput_bps();
        format!(
            "Transferred {} revisions, {} objects ({} skipped)\n\
             Raw: {} bytes, Compressed: {} bytes (ratio: {:.2})\n\
             Time: {}ms, Throughput: {}/s",
            self.revisions,
            self.objects_transferred,
            self.objects_skipped,
            format_size(self.raw_bytes),
            format_size(self.compressed_bytes),
            ratio,
            self.duration_ms,
            format_size(throughput as u64),
        )
    }
}

/// Compress a blob with zstd.
pub fn compress_object(data: &[u8]) -> Result<Vec<u8>> {
    zstd::encode_all(data, 3).map_err(|e| anyhow!("Compression failed: {}", e))
}

/// Decompress a blob with zstd.
pub fn decompress_object(data: &[u8]) -> Result<Vec<u8>> {
    zstd::decode_all(data).map_err(|e| anyhow!("Decompression failed: {}", e))
}

/// Check which objects from a list are missing at the destination.
pub fn find_missing_objects(
    dest_repo: &SqliteRepository,
    object_ids: &[ObjectId],
) -> Vec<ObjectId> {
    let mut missing = Vec::new();
    for id in object_ids {
        let hex = id.to_hex();
        let obj_path = dest_repo
            .root()
            .join("objects")
            .join(&hex[..2])
            .join(&hex[2..]);
        if !obj_path.exists() {
            missing.push(*id);
        }
    }
    missing
}

/// Transfer a batch of revisions between repositories with deduplication.
pub fn transfer_revisions(
    source: &SqliteRepository,
    dest: &SqliteRepository,
    from_rev: u64,
    to_rev: u64,
    progress_callback: Option<&dyn Fn(u64, u64)>,
) -> Result<TransferStats> {
    let _rt = tokio::runtime::Handle::current();
    let start = std::time::Instant::now();
    let mut stats = TransferStats::default();

    // Collect all known object IDs at destination for dedup
    // (For large repos, we use path-based existence checks instead)

    dest.begin_batch();

    for rev in from_rev..=to_rev {
        let rev_data = crate::protocol::extract_revision_data(source, rev)?;

        // Count raw bytes
        for (_, data) in &rev_data.objects {
            stats.raw_bytes += data.len() as u64;
        }

        // Apply with dedup: only store objects not already present
        for (id, data) in &rev_data.objects {
            let hex = id.to_hex();
            let obj_path = dest
                .root()
                .join("objects")
                .join(&hex[..2])
                .join(&hex[2..]);

            if obj_path.exists() {
                stats.objects_skipped += 1;
            } else {
                // Compress for stats tracking
                let compressed = compress_object(data)?;
                stats.compressed_bytes += compressed.len() as u64;
                stats.objects_transferred += 1;

                // Store the original blob (not compressed — storage handles its own format)
                if let Some(parent) = obj_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let blob = Blob::new(data.clone(), false);
                std::fs::write(&obj_path, blob.to_bytes()?)?;
            }
        }

        // Apply tree changes
        for change in &rev_data.delta_tree.changes {
            match change {
                TreeChange::Upsert { path, entry } => {
                    if entry.kind == ObjectKind::Blob {
                        if let Some((_, data)) =
                            rev_data.objects.iter().find(|(id, _)| *id == entry.id)
                        {
                            dest.add_file_sync(path, data.clone(), entry.mode == 0o755)?;
                        }
                    } else {
                        dest.mkdir_sync(path)?;
                    }
                }
                TreeChange::Delete { path } => {
                    dest.delete_file_sync(path)?;
                }
            }
        }

        // Commit
        dest.commit_sync(
            rev_data.author.clone(),
            rev_data.message.clone(),
            rev_data.timestamp,
        )?;

        stats.revisions += 1;

        if let Some(cb) = &progress_callback {
            cb(rev, to_rev);
        }
    }

    dest.end_batch();

    stats.duration_ms = start.elapsed().as_millis() as u64;

    // If no objects were compressed (all skipped), set compressed = raw
    if stats.compressed_bytes == 0 && stats.raw_bytes > 0 {
        stats.compressed_bytes = stats.raw_bytes;
    }

    Ok(stats)
}

/// Verify that two repositories have the same content at a given revision.
pub fn verify_sync(
    source: &SqliteRepository,
    dest: &SqliteRepository,
    rev: u64,
) -> Result<VerifyResult> {
    let source_tree = source.get_tree_at_rev(rev)?;
    let dest_tree = dest.get_tree_at_rev(rev)?;

    let mut mismatches = Vec::new();
    let mut missing_in_dest = Vec::new();
    let mut extra_in_dest = Vec::new();

    // Check all source entries exist in dest with same content
    for (path, entry) in &source_tree {
        match dest_tree.get(path) {
            Some(dest_entry) => {
                if entry.id != dest_entry.id {
                    mismatches.push(path.clone());
                }
            }
            None => {
                missing_in_dest.push(path.clone());
            }
        }
    }

    // Check for extra entries in dest
    for path in dest_tree.keys() {
        if !source_tree.contains_key(path) {
            extra_in_dest.push(path.clone());
        }
    }

    let ok = mismatches.is_empty() && missing_in_dest.is_empty() && extra_in_dest.is_empty();

    Ok(VerifyResult {
        revision: rev,
        source_entries: source_tree.len(),
        dest_entries: dest_tree.len(),
        mismatches,
        missing_in_dest,
        extra_in_dest,
        ok,
    })
}

/// Result of sync verification.
#[derive(Debug)]
pub struct VerifyResult {
    pub revision: u64,
    pub source_entries: usize,
    pub dest_entries: usize,
    pub mismatches: Vec<String>,
    pub missing_in_dest: Vec<String>,
    pub extra_in_dest: Vec<String>,
    pub ok: bool,
}

impl std::fmt::Display for VerifyResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Verification for r{}:", self.revision)?;
        writeln!(f, "  Source entries: {}", self.source_entries)?;
        writeln!(f, "  Dest entries:   {}", self.dest_entries)?;
        if self.ok {
            writeln!(f, "  Status: OK ✓")?;
        } else {
            if !self.mismatches.is_empty() {
                writeln!(f, "  Mismatches ({}):", self.mismatches.len())?;
                for p in &self.mismatches {
                    writeln!(f, "    {}", p)?;
                }
            }
            if !self.missing_in_dest.is_empty() {
                writeln!(f, "  Missing in dest ({}):", self.missing_in_dest.len())?;
                for p in &self.missing_in_dest {
                    writeln!(f, "    {}", p)?;
                }
            }
            if !self.extra_in_dest.is_empty() {
                writeln!(f, "  Extra in dest ({}):", self.extra_in_dest.len())?;
                for p in &self.extra_in_dest {
                    writeln!(f, "    {}", p)?;
                }
            }
            writeln!(f, "  Status: FAILED ✗")?;
        }
        Ok(())
    }
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_repo(path: &std::path::Path) -> SqliteRepository {
        let repo = SqliteRepository::open(path).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(repo.initialize()).unwrap();
        repo
    }

    #[test]
    fn test_compress_decompress() {
        let data = b"Hello, World! This is some test data for compression.";
        let compressed = compress_object(data).unwrap();
        let decompressed = decompress_object(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_compress_ratio() {
        // Highly compressible data
        let data = vec![b'A'; 10000];
        let compressed = compress_object(&data).unwrap();
        assert!(compressed.len() < data.len() / 2);
    }

    #[test]
    fn test_transfer_stats_summary() {
        let stats = TransferStats {
            revisions: 10,
            objects_transferred: 50,
            objects_skipped: 5,
            raw_bytes: 1024 * 1024,
            compressed_bytes: 512 * 1024,
            duration_ms: 1000,
        };
        let summary = stats.summary();
        assert!(summary.contains("10 revisions"));
        assert!(summary.contains("50 objects"));
        assert!(summary.contains("5 skipped"));
    }

    #[test]
    fn test_compression_ratio() {
        let stats = TransferStats {
            raw_bytes: 1000,
            compressed_bytes: 500,
            ..Default::default()
        };
        assert!((stats.compression_ratio() - 0.5).abs() < 0.001);
    }
}

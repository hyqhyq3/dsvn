//! Replication log management for dsvnsync CLI.
//!
//! Provides CLI-friendly wrappers around the core ReplicationLog,
//! including human-readable output formatting.

pub use dsvn_core::sync::{ReplicationLog, ReplicationLogEntry};

use anyhow::Result;
use std::io::Write;
use std::path::Path;

/// Format a replication log entry for human-readable display.
pub fn format_entry(entry: &ReplicationLogEntry) -> String {
    let date = chrono::DateTime::from_timestamp(entry.timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| entry.timestamp.to_string());

    let status = if entry.success { "OK" } else { "FAILED" };

    format!(
        "[{}] r{}-r{} | {} objects, {} bytes | {}ms | {}{}",
        date,
        entry.from_rev,
        entry.to_rev,
        entry.objects_transferred,
        format_size(entry.bytes_transferred),
        entry.duration_ms,
        status,
        entry
            .error
            .as_ref()
            .map(|e| format!(" ({})", e))
            .unwrap_or_default(),
    )
}

/// Print all replication log entries for a repository.
pub fn print_repl_log(
    repo_path: &Path,
    from_rev: Option<u64>,
    to_rev: Option<u64>,
    writer: &mut dyn Write,
) -> Result<()> {
    let log = ReplicationLog::new(repo_path);
    let entries = match (from_rev, to_rev) {
        (Some(f), Some(t)) => log.query(f, t)?,
        _ => log.all()?,
    };

    if entries.is_empty() {
        writeln!(writer, "No replication log entries found.")?;
        return Ok(());
    }

    writeln!(writer, "Replication Log ({} entries):", entries.len())?;
    writeln!(writer, "{}", "-".repeat(80))?;
    for entry in &entries {
        writeln!(writer, "  {}", format_entry(entry))?;
    }
    writeln!(writer, "{}", "-".repeat(80))?;

    // Summary
    let total_objects: u64 = entries.iter().map(|e| e.objects_transferred).sum();
    let total_bytes: u64 = entries.iter().map(|e| e.bytes_transferred).sum();
    let total_ms: u64 = entries.iter().map(|e| e.duration_ms).sum();
    let successes = entries.iter().filter(|e| e.success).count();

    writeln!(
        writer,
        "Summary: {} syncs ({} successful), {} objects, {}, {}ms total",
        entries.len(),
        successes,
        total_objects,
        format_size(total_bytes),
        total_ms,
    )?;

    Ok(())
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

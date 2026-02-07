//! Load SVN dump file into DSvn repository
//!
//! Optimized for high-throughput import of large SVN dump files.
//! Key optimizations:
//! - Streaming parser: processes entries as they are parsed, no full in-memory load
//! - Batched disk writes: defers root_tree persistence until commit time
//! - Progress reporting: only prints every N revisions to reduce IO
//! - Efficient object storage: minimizes redundant serialization

use crate::dump_format::{NodeAction, NodeKind};
use anyhow::Result;
use dsvn_core::DiskRepository;
use std::io::BufRead;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::task;

/// Load SVN dump file into repository using streaming parser.
///
/// Runs the entire import in a blocking thread pool to avoid
/// blocking the async runtime with synchronous disk operations.
pub async fn load_dump_file<R: BufRead + Send + 'static>(
    repo: Arc<DiskRepository>,
    reader: R,
) -> Result<()> {
    task::spawn_blocking(move || {
        load_dump_file_blocking(&repo, reader)
    })
    .await
    .map_err(|e| anyhow::anyhow!("Import task failed: {:?}", e))?
}

/// Blocking version of load_dump_file (runs in spawn_blocking context).
fn load_dump_file_blocking<R: BufRead>(
    repo: &DiskRepository,
    reader: R,
) -> Result<()> {
    let start_time = Instant::now();
    let total_revisions = AtomicU64::new(0);
    let total_nodes = AtomicU64::new(0);
    let final_rev = AtomicU64::new(0);
    let mut last_report = Instant::now();

    // Use the streaming parser — callback is invoked per revision
    crate::dump::parse_dump_streaming(reader, |revision| {
        let rev_num = revision.revision_number;

        if rev_num == 0 {
            return Ok(());
        }

        let node_count = revision.nodes.len();

        if node_count == 0 {
            return Ok(());
        }

        // Begin batch mode: suppress per-file root_tree persistence
        repo.begin_batch();

        let mut has_changes = false;

        for node in &revision.nodes {
            let path = &node.path;
            let kind = node.kind;
            let action = node.action;

            match action {
                Some(NodeAction::Add) | Some(NodeAction::Replace) => {
                    if node.copy_from_path.is_some() && node.copy_from_rev.is_some() {
                        // TODO: Implement copy operation
                    } else {
                        match kind {
                            Some(NodeKind::File) => {
                                repo.add_file_sync(path, node.content.clone(), false)?;
                                has_changes = true;
                            }
                            Some(NodeKind::Dir) => {
                                repo.mkdir_sync(path)?;
                                has_changes = true;
                            }
                            None => {
                                if !node.content.is_empty() {
                                    repo.add_file_sync(path, node.content.clone(), false)?;
                                    has_changes = true;
                                }
                            }
                        }
                    }
                }
                Some(NodeAction::Delete) => {
                    repo.delete_file_sync(path)?;
                    has_changes = true;
                }
                Some(NodeAction::Change) => {
                    if !node.content.is_empty() {
                        repo.add_file_sync(path, node.content.clone(), false)?;
                        has_changes = true;
                    }
                }
                None => {}
            }
        }

        if has_changes {
            let rev = repo.commit_sync(
                revision.author.clone(),
                revision.message.clone(),
                revision.timestamp,
            )?;
            final_rev.store(rev, Ordering::SeqCst);
        }

        // End batch mode: persist root_tree once
        repo.end_batch();

        let tr = total_revisions.fetch_add(1, Ordering::SeqCst) + 1;
        total_nodes.fetch_add(node_count as u64, Ordering::SeqCst);

        // Progress reporting: every 500 revisions or every 5 seconds
        let now = Instant::now();
        if tr % 500 == 0 || now.duration_since(last_report).as_secs() >= 5 {
            let elapsed = start_time.elapsed().as_secs_f64();
            let rate = tr as f64 / (elapsed / 60.0);
            println!(
                "  Progress: rev {} | {} revisions ({} nodes) in {:.1}s | {:.0} rev/min",
                rev_num, tr, total_nodes.load(Ordering::SeqCst), elapsed, rate
            );
            last_report = now;
        }

        Ok(())
    })?;

    let elapsed = start_time.elapsed();
    let tr = total_revisions.load(Ordering::SeqCst);
    let rate = if elapsed.as_secs() > 0 {
        tr as f64 / (elapsed.as_secs_f64() / 60.0)
    } else {
        tr as f64
    };

    println!("Load complete!");
    println!("  Total revisions: {}", tr);
    println!("  Total nodes: {}", total_nodes.load(Ordering::SeqCst));
    println!("  Final revision: {}", final_rev.load(Ordering::SeqCst));
    println!("  Elapsed: {:.1}s", elapsed.as_secs_f64());
    println!("  Rate: {:.0} revisions/min", rate);

    Ok(())
}

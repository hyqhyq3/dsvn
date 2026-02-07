//! Load SVN dump file into DSvn repository

use crate::dump_format::{NodeAction, NodeKind};
use anyhow::Result;
use dsvn_core::DiskRepository;
use std::io::BufRead;

/// Load SVN dump file into repository.
///
/// The dump entries are ordered: revision entry, then its node entries, then
/// the next revision entry, etc. We collect nodes under each revision and
/// commit them together.
pub async fn load_dump_file<R: BufRead>(
    repo: &DiskRepository,
    reader: R,
) -> Result<()> {
    let dump = crate::dump::parse_dump_file(reader)?;

    println!("Loading dump file...");
    println!("Format version: {}", dump.format_version);
    println!("UUID: {}", dump.uuid);
    println!("Entries: {}", dump.entries.len());

    if dump.entries.is_empty() {
        return Ok(());
    }

    // Group entries into (revision_entry, [node_entries])
    struct RevisionGroup {
        revision_number: u64,
        author: String,
        message: String,
        timestamp: i64,
        has_nodes: bool,
    }

    let mut current_group: Option<RevisionGroup> = None;
    let mut final_rev: u64 = 0;

    for entry in &dump.entries {
        if entry.is_revision() {
            // Commit the previous group if it had nodes and was not rev 0
            if let Some(group) = current_group.take() {
                if group.has_nodes && group.revision_number > 0 {
                    let rev = repo.commit(
                        group.author,
                        group.message,
                        group.timestamp,
                    ).await?;
                    final_rev = rev;
                    println!("  Committed revision {}", rev);
                }
            }

            println!("Processing revision {}", entry.revision_number);

            let author = entry.props.get("svn:author")
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            let message = entry.props.get("svn:log")
                .cloned()
                .unwrap_or_default();
            let timestamp = entry.props.get("svn:date")
                .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                .map(|dt| dt.timestamp())
                .unwrap_or_else(|| chrono::Utc::now().timestamp());

            current_group = Some(RevisionGroup {
                revision_number: entry.revision_number,
                author,
                message,
                timestamp,
                has_nodes: false,
            });

        } else if entry.is_node() {
            let path = entry.node_path.as_ref().unwrap();
            let kind = entry.node_kind;
            let action = entry.node_action;

            if let Some(ref mut group) = current_group {
                group.has_nodes = true;
            }

            match action {
                Some(NodeAction::Add) | Some(NodeAction::Replace) => {
                    if entry.copy_from_path.is_some() && entry.copy_from_rev.is_some() {
                        println!("  Copy {} from {}@{}", path,
                            entry.copy_from_path.as_ref().unwrap(),
                            entry.copy_from_rev.unwrap());
                        // TODO: Implement copy operation
                    } else {
                        match kind {
                            Some(NodeKind::File) => {
                                repo.add_file(path, entry.content.clone(), false).await?;
                                println!("  Added file: {} ({} bytes)", path, entry.content.len());
                            }
                            Some(NodeKind::Dir) => {
                                repo.mkdir(path).await?;
                                println!("  Created directory: {}", path);
                            }
                            None => {
                                // Unknown kind — if has content, treat as file
                                if !entry.content.is_empty() {
                                    repo.add_file(path, entry.content.clone(), false).await?;
                                    println!("  Added file (unknown kind): {}", path);
                                }
                            }
                        }
                    }
                }
                Some(NodeAction::Delete) => {
                    repo.delete_file(path).await?;
                    println!("  Deleted: {}", path);
                }
                Some(NodeAction::Change) => {
                    if !entry.content.is_empty() {
                        repo.add_file(path, entry.content.clone(), false).await?;
                        println!("  Modified: {}", path);
                    }
                }
                None => {
                    // No action, just metadata
                }
            }
        }
    }

    // Flush the last revision group
    if let Some(group) = current_group.take() {
        if group.has_nodes && group.revision_number > 0 {
            let rev = repo.commit(
                group.author,
                group.message,
                group.timestamp,
            ).await?;
            final_rev = rev;
            println!("  Committed revision {}", rev);
        }
    }

    println!("Load complete! Final revision: {}", final_rev);
    Ok(())
}

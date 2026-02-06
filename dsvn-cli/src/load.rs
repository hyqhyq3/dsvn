//! Load SVN dump file into DSvn repository

use crate::dump_format::{DumpFormat, NodeAction, NodeKind};
use anyhow::Result;
use dsvn_core::Repository;
use std::io::BufRead;

/// Load SVN dump file into repository
pub async fn load_dump_file<R: BufRead>(
    repo: &Repository,
    reader: R,
) -> Result<()> {
    // Parse dump file
    let dump = crate::dump::parse_dump_file(reader)?;

    println!("Loading dump file...");
    println!("Format version: {}", dump.format_version);
    println!("UUID: {}", dump.uuid);
    println!("Entries: {}", dump.entries.len());

    // Initialize repository if needed
    if dump.entries.is_empty() {
        return Ok(());
    }

    // Process each entry
    let mut current_rev = 0u64;
    let mut _file_buffer: Vec<u8> = Vec::new();

    for entry in &dump.entries {
        if entry.is_revision() {
            // This is a revision entry
            println!("Processing revision {}", entry.revision_number);

            // Extract author, message, date from props
            let author = entry.props.get("svn:author")
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            let message = entry.props.get("svn:log")
                .cloned()
                .unwrap_or_else(|| String::new());

            let timestamp = entry.props.get("svn:date")
                .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                .map(|dt| dt.timestamp())
                .unwrap_or_else(|| chrono::Utc::now().timestamp());

            // Create commit
            let rev = repo.commit(
                author.clone(),
                message.clone(),
                timestamp,
            ).await?;

            current_rev = rev;
            println!("  Committed revision {}", rev);

        } else if entry.is_node() {
            // This is a node entry
            let path = entry.node_path.as_ref().unwrap();
            let kind = entry.node_kind;
            let action = entry.node_action;

            match action {
                Some(NodeAction::Add) | Some(NodeAction::Replace) => {
                    if entry.copy_from_path.is_some() && entry.copy_from_rev.is_some() {
                        // Copy operation
                        println!("  Copy {} from {}@{}", path, entry.copy_from_path.as_ref().unwrap(), entry.copy_from_rev.unwrap());
                        // TODO: Implement copy operation
                    } else {
                        // Add with content
                        if !entry.content.is_empty() {
                            let is_file = kind == Some(NodeKind::File);
                            if is_file {
                                repo.add_file(path, entry.content.clone(), false).await?;
                                println!("  Added file: {}", path);
                            } else {
                                repo.mkdir(path).await?;
                                println!("  Created directory: {}", path);
                            }
                        }
                    }
                }
                Some(NodeAction::Delete) => {
                    println!("  Deleted: {}", path);
                    // TODO: Implement delete
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

    println!("Load complete! Final revision: {}", current_rev);
    Ok(())
}

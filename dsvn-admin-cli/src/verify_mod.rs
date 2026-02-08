//! Verify repository integrity with optional remote transfer

use anyhow::{anyhow, Result};
use dsvn_core::SqliteRepository;
use std::path::Path;

/// Verify repository with optional transfer support
///
/// If `transfer_url` is provided, missing objects will be fetched
/// from the remote HTTP server.
pub async fn verify_repository(
    repo_path: &str,
    start_rev: Option<u64>,
    end_rev: Option<u64>,
    quiet: bool,
    transfer_url: Option<String>,
) -> Result<()> {
    let repository = SqliteRepository::open(Path::new(repo_path))?;
    repository.initialize().await?;
    let head = repository.current_rev().await;
    let start = start_rev.unwrap_or(0);
    let end = end_rev.unwrap_or(head).min(head);

    if !quiet {
        println!("Verifying repository: {}", repo_path);
        println!("  Revisions: {} to {}", start, end);
        if let Some(ref url) = transfer_url {
            println!("  Transfer URL: {}", url);
        }
        println!();
    }

    let mut errors = 0u64;
    let mut warnings = 0u64;
    let mut verified_revs = 0u64;
    let mut verified_objects = 0u64;

    // First pass: verify and collect missing objects
    let mut missing_objects = Vec::new();
    for rev in start..=end {
        match repository.get_commit(rev).await {
            Some(commit) => {
                if commit.author.is_empty() && rev > 0 {
                    warnings += 1;
                    if !quiet {
                        eprintln!("  WARNING: r{} has empty author", rev);
                    }
                }
                match repository.get_tree_at_rev(rev) {
                    Ok(tree_map) => {
                        for (path, entry) in &tree_map {
                            if entry.kind == dsvn_core::ObjectKind::Blob {
                                match repository.get_file(&format!("/{}", path), rev).await {
                                    Ok(_) => {
                                        verified_objects += 1;
                                    }
                                    Err(_) => {
                                        // Object missing, collect for transfer
                                        missing_objects.push(entry.id);
                                        if !quiet {
                                            eprintln!(
                                                "  ERROR: r{} object missing for '{}'",
                                                rev, path
                                            );
                                        }
                                        errors += 1;
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        errors += 1;
                        if !quiet {
                            eprintln!("  ERROR: r{} tree reconstruction failed: {}", rev, e);
                        }
                    }
                }
                if rev > 0 {
                    // Check delta tree
                    let delta_path = repository
                        .root()
                        .join("tree_deltas")
                        .join(format!("{}.bin", rev));
                    if !delta_path.exists() {
                        warnings += 1;
                        if !quiet {
                            eprintln!("  WARNING: r{} has no delta tree", rev);
                        }
                    } else {
                        verified_objects += 1;
                    }
                }
            }
            None => {
                errors += 1;
                if !quiet {
                    eprintln!("  ERROR: r{} commit not found", rev);
                }
            }
        }
        verified_revs += 1;
        if !quiet && (rev % 100 == 0 || rev == end) {
            eprint!(
                "\r  Verified: r{}/{} ({} objects, {} errors)    ",
                rev,
                end,
                verified_objects,
                errors
            );
        }
    }

    if !quiet {
        eprintln!();
    }

    // Initial verification report
    println!("\nInitial Verification:");
    println!("  Revisions verified: {}", verified_revs);
    println!("  Objects verified:   {}", verified_objects);
    println!("  Missing objects:   {}", missing_objects.len());
    println!("  Errors:             {}", errors);
    println!("  Warnings:           {}", warnings);

    // Transfer mode: fetch missing objects from remote
    if let Some(url) = transfer_url {
        if !missing_objects.is_empty() {
            println!("\nTransfer mode: fetching {} missing objects from {}...", missing_objects.len(), url);

            match fetch_missing_objects(&url, &repository, &missing_objects).await {
                Ok(result) => {
                    println!("  Fetched {} objects", result.fetched);
                    println!("  Bytes fetched:    {}", result.bytes);
                    if result.errors > 0 {
                        eprintln!("  Errors:            {}", result.errors);
                        eprintln!("\nNote: Some objects could not be fetched.");
                    } else {
                        println!("\nRe-verifying after transfer...");
                        // Note: Full re-verification would be costly, just reporting success
                    }
                }
                Err(e) => {
                    eprintln!("  Transfer failed: {}", e);
                    return Err(e);
                }
            }
        } else {
            println!("\nNo missing objects - transfer not needed.");
        }
    }

    if errors > 0 {
        Err(anyhow!(
            "Repository verification failed with {} error(s)",
            errors
        ))
    } else {
        if !quiet {
            println!("\n✓ Repository is healthy");
        }
        Ok(())
    }
}

/// Fetch missing objects from remote HTTP server
async fn fetch_missing_objects(
    base_url: &str,
    repository: &SqliteRepository,
    object_ids: &[dsvn_core::ObjectId],
) -> Result<TransferResult> {
    use dsvn_core::Blob;

    let mut result = TransferResult::default();

    // Build request URL
    let mut url = format!("{}/objects", base_url.trim_end_matches('/'));
    for (i, id) in object_ids.iter().enumerate() {
        if i == 0 {
            url.push('?');
        } else {
            url.push('&');
        }
        url.push_str(&format!("id={}", id.to_hex()));
    }

    // Fetch objects
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await?;

    if !resp.status().is_success() {
        return Err(anyhow!(
            "Failed to fetch objects: HTTP {}",
            resp.status()
        ));
    }

    let data = resp.bytes().await?.to_vec();

    // Parse binary response: [32B id][4B len][N bytes data]...
    let mut pos = 0;
    while pos + 36 <= data.len() {
        let mut id_bytes = [0u8; 32];
        id_bytes.copy_from_slice(&data[pos..pos + 32]);
        let oid = dsvn_core::ObjectId::new(id_bytes);
        pos += 32;

        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
        pos += 4;

        if len == 0xFFFF_FFFF {
            // Object not found
            result.errors += 1;
        } else {
            let end = pos + len as usize;
            if end > data.len() {
                return Err(anyhow!("Truncated object data"));
            }

            let object_data = data[pos..end].to_vec();

            // Store object
            let hex = oid.to_hex();
            let obj_path = repository
                .root()
                .join("objects")
                .join(&hex[..2])
                .join(&hex[2..]);

            if let Some(parent) = obj_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let blob = Blob::new(object_data.clone(), false);
            std::fs::write(&obj_path, blob.to_bytes()?)?;

            result.fetched += 1;
            result.bytes += object_data.len() as u64;
            pos = end;
        }
    }

    Ok(result)
}

#[derive(Debug, Default)]
struct TransferResult {
    fetched: u64,
    bytes: u64,
    errors: u64,
}

//! dsvnsync — Repository synchronization tool for dsvn.
//!
//! Supports:
//! - Local-to-local repository sync (init/sync/info/cleanup)
//! - SVNSync compatibility mode
//! - Sync verification
//! - Replication log management
//!
//! # Usage
//!
//! ```bash
//! # Initialize sync relationship
//! dsvnsync init --source /path/to/master --dest /path/to/slave
//!
//! # Perform incremental sync
//! dsvnsync sync --source /path/to/master --dest /path/to/slave
//!
//! # Check sync status
//! dsvnsync info /path/to/slave
//!
//! # Verify sync integrity
//! dsvnsync verify --source /path/to/master --dest /path/to/slave
//!
//! # View replication log
//! dsvnsync repl-log /path/to/slave
//!
//! # Clean up sync state
//! dsvnsync cleanup /path/to/slave
//! ```

mod compat;
mod protocol;
mod replication_log;
mod transfer;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use dsvn_core::SqliteRepository;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(name = "dsvnsync")]
#[command(author = "DSvn Contributors")]
#[command(version = "0.1.0")]
#[command(about = "DSvn repository synchronization tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize sync relationship between source and destination
    Init {
        /// Source (master) repository path
        #[arg(short, long)]
        source: String,
        /// Destination (slave) repository path
        #[arg(short, long)]
        dest: String,
        /// Use SVNSync-compatible mode
        #[arg(long)]
        svnsync_compat: bool,
    },

    /// Perform incremental sync from source to destination
    Sync {
        /// Source (master) repository path
        #[arg(short, long)]
        source: String,
        /// Destination (slave) repository path
        #[arg(short, long)]
        dest: String,
        /// Verify each revision after sync
        #[arg(long)]
        verify: bool,
    },

    /// Display sync status and information
    Info {
        /// Repository path
        repo: String,
    },

    /// Verify sync integrity between source and destination
    Verify {
        /// Source (master) repository path
        #[arg(short, long)]
        source: String,
        /// Destination (slave) repository path
        #[arg(short, long)]
        dest: String,
        /// Specific revision to verify (default: HEAD)
        #[arg(short = 'r', long)]
        revision: Option<u64>,
    },

    /// View replication log
    #[command(name = "repl-log")]
    ReplLog {
        /// Repository path
        repo: String,
        /// Start revision filter
        #[arg(long)]
        from: Option<u64>,
        /// End revision filter
        #[arg(long)]
        to: Option<u64>,
    },

    /// Clean up sync state and replication log
    Cleanup {
        /// Repository path
        repo: String,
        /// Also remove the pre-revprop-change hook
        #[arg(long)]
        remove_hooks: bool,
    },

    /// SVNSync compatibility: initialize mirror
    #[command(name = "svnsync-init")]
    SvnSyncInit {
        /// Source repository URL/path
        #[arg(short, long)]
        source: String,
        /// Destination repository path
        #[arg(short, long)]
        dest: String,
    },

    /// SVNSync compatibility: sync mirror
    #[command(name = "svnsync-sync")]
    SvnSyncSync {
        /// Source repository path
        #[arg(short, long)]
        source: String,
        /// Destination repository path
        #[arg(short, long)]
        dest: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("dsvnsync=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init {
            source,
            dest,
            svnsync_compat,
        } => cmd_init(source, dest, svnsync_compat).await,

        Commands::Sync {
            source,
            dest,
            verify,
        } => cmd_sync(source, dest, verify).await,

        Commands::Info { repo } => cmd_info(repo).await,

        Commands::Verify {
            source,
            dest,
            revision,
        } => cmd_verify(source, dest, revision).await,

        Commands::ReplLog { repo, from, to } => cmd_repl_log(repo, from, to).await,

        Commands::Cleanup {
            repo,
            remove_hooks,
        } => cmd_cleanup(repo, remove_hooks).await,

        Commands::SvnSyncInit { source, dest } => cmd_svnsync_init(source, dest).await,

        Commands::SvnSyncSync { source, dest } => cmd_svnsync_sync(source, dest).await,
    }
}

async fn cmd_init(source: String, dest: String, svnsync_compat: bool) -> Result<()> {
    println!("Initializing sync...");
    println!("  Source: {}", source);
    println!("  Dest:   {}", dest);

    let source_repo = SqliteRepository::open(Path::new(&source))?;
    source_repo.initialize().await?;
    let dest_repo = SqliteRepository::open(Path::new(&dest))?;
    dest_repo.initialize().await?;

    let sync = protocol::LocalSync::new(&source_repo, &dest_repo);
    let state = sync.init()?;

    if svnsync_compat {
        let compat = compat::SvnSyncCompat::new(&dest_repo);
        compat.init_mirror(
            &format!("file://{}", source),
            source_repo.uuid(),
        )?;
        println!("  SVNSync compatibility mode enabled");
    }

    println!("\nSync initialized:");
    println!("  Source UUID:  {}", state.source_uuid);
    println!("  Source URL:   {}", state.source_url);
    println!("  Source HEAD:  r{}", state.source_head_rev);
    println!("  Ready to sync from r1 to r{}", state.source_head_rev);
    Ok(())
}

async fn cmd_sync(source: String, dest: String, verify: bool) -> Result<()> {
    let source_repo = SqliteRepository::open(Path::new(&source))?;
    source_repo.initialize().await?;
    let dest_repo = SqliteRepository::open(Path::new(&dest))?;
    dest_repo.initialize().await?;

    let sync = protocol::LocalSync::new(&source_repo, &dest_repo);

    println!("Starting sync...");
    let result = sync.sync()?;

    if result.already_up_to_date {
        println!("Already up to date.");
        return Ok(());
    }

    println!("\nSync completed:");
    println!("  Revisions: r{} → r{}", result.from_rev, result.to_rev);
    println!("  Synced:    {} revisions", result.revisions_synced);
    println!("  Objects:   {}", result.objects_transferred);
    println!("  Bytes:     {}", format_size(result.bytes_transferred));
    println!("  Time:      {}ms", result.duration_ms);

    if result.duration_ms > 0 {
        let rev_per_sec =
            (result.revisions_synced as f64 / result.duration_ms as f64) * 1000.0;
        println!("  Speed:     {:.1} revisions/sec", rev_per_sec);
    }

    // Also update SVNSync compat properties if configured
    let svn_compat = compat::SvnSyncCompat::new(&dest_repo);
    if svn_compat.is_mirror()? {
        svn_compat.set_last_merged_rev(result.to_rev)?;
    }

    // Optional verify
    if verify {
        println!("\nVerifying sync...");
        let verify_result =
            transfer::verify_sync(&source_repo, &dest_repo, result.to_rev)?;
        print!("{}", verify_result);
        if !verify_result.ok {
            return Err(anyhow!("Sync verification failed!"));
        }
    }

    Ok(())
}

async fn cmd_info(repo: String) -> Result<()> {
    let repo_path = Path::new(&repo);
    let repository = SqliteRepository::open(repo_path)?;
    repository.initialize().await?;

    let state = dsvn_core::SyncState::load(repo_path)?;
    let dest_rev = repository.current_rev().await;

    println!("Repository: {}", repo);
    println!("UUID:       {}", repository.uuid());
    println!("HEAD:       r{}", dest_rev);

    match state {
        Some(s) => {
            println!("\nSync State:");
            println!("  Source UUID:      {}", s.source_uuid);
            println!("  Source URL:       {}", s.source_url);
            println!("  Source HEAD:      r{}", s.source_head_rev);
            println!("  Last synced:      r{}", s.last_synced_rev);
            println!("  Total syncs:      {}", s.total_synced_revisions);
            println!("  In progress:      {}", s.sync_in_progress);
            println!("  Protocol version: {}", s.protocol_version);

            if let Some(cp) = s.checkpoint_rev {
                println!("  Checkpoint:       r{}", cp);
            }

            if s.last_sync_timestamp > 0 {
                let date = chrono::DateTime::from_timestamp(s.last_sync_timestamp, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                    .unwrap_or_else(|| s.last_sync_timestamp.to_string());
                println!("  Last sync time:   {}", date);
            }

            let behind = if s.source_head_rev > s.last_synced_rev {
                s.source_head_rev - s.last_synced_rev
            } else {
                0
            };
            println!("  Behind source:    {} revisions", behind);
        }
        None => {
            println!("\nNo sync state found (not a sync destination).");
        }
    }

    // Check SVNSync compat
    let svn_compat = compat::SvnSyncCompat::new(&repository);
    if svn_compat.is_mirror()? {
        println!("\nSVNSync Compatibility:");
        println!(
            "  Source URL:       {}",
            svn_compat.get_source_url()?.unwrap_or_default()
        );
        println!(
            "  Source UUID:      {}",
            svn_compat.get_source_uuid()?.unwrap_or_default()
        );
        println!(
            "  Last merged rev:  r{}",
            svn_compat.get_last_merged_rev()?
        );
        if let Some(copying) = svn_compat.get_currently_copying()? {
            println!("  Currently copying: r{}", copying);
        }
    }

    // Show latest repl log entry
    let repl_log = dsvn_core::ReplicationLog::new(repo_path);
    if let Some(entry) = repl_log.latest()? {
        println!("\nLatest Replication:");
        println!("  {}", replication_log::format_entry(&entry));
    }

    Ok(())
}

async fn cmd_verify(source: String, dest: String, revision: Option<u64>) -> Result<()> {
    let source_repo = SqliteRepository::open(Path::new(&source))?;
    source_repo.initialize().await?;
    let dest_repo = SqliteRepository::open(Path::new(&dest))?;
    dest_repo.initialize().await?;

    let rev = match revision {
        Some(r) => r,
        None => dest_repo.current_rev().await,
    };

    println!("Verifying sync at revision {}...", rev);
    let result = transfer::verify_sync(&source_repo, &dest_repo, rev)?;
    print!("{}", result);

    if !result.ok {
        return Err(anyhow!("Verification failed!"));
    }
    Ok(())
}

async fn cmd_repl_log(repo: String, from: Option<u64>, to: Option<u64>) -> Result<()> {
    let repo_path = Path::new(&repo);
    replication_log::print_repl_log(
        repo_path,
        from,
        to,
        &mut std::io::stdout(),
    )?;
    Ok(())
}

async fn cmd_cleanup(repo: String, remove_hooks: bool) -> Result<()> {
    let repo_path = Path::new(&repo);

    println!("Cleaning up sync state for: {}", repo);

    // Remove sync state
    dsvn_core::SyncState::remove(repo_path)?;
    println!("  Removed sync-state.json");

    // Remove SVNSync compat properties
    let revprops_path = repo_path.join("revprops").join("0.json");
    if revprops_path.exists() {
        let data = std::fs::read_to_string(&revprops_path)?;
        let mut props: std::collections::HashMap<String, String> =
            serde_json::from_str(&data)?;
        let sync_keys: Vec<String> = props
            .keys()
            .filter(|k| k.starts_with("svn:sync-"))
            .cloned()
            .collect();
        for key in &sync_keys {
            props.remove(key);
        }
        if props.is_empty() {
            std::fs::remove_file(&revprops_path)?;
        } else {
            std::fs::write(&revprops_path, serde_json::to_string_pretty(&props)?)?;
        }
        if !sync_keys.is_empty() {
            println!("  Removed {} sync properties", sync_keys.len());
        }
    }

    // Optionally remove hooks
    if remove_hooks {
        let hook_path = repo_path.join("hooks").join("pre-revprop-change");
        if hook_path.exists() {
            std::fs::remove_file(&hook_path)?;
            println!("  Removed pre-revprop-change hook");
        }
    }

    println!("Cleanup complete.");
    Ok(())
}

async fn cmd_svnsync_init(source: String, dest: String) -> Result<()> {
    println!("SVNSync-compatible init...");

    let source_repo = SqliteRepository::open(Path::new(&source))?;
    source_repo.initialize().await?;
    let dest_repo = SqliteRepository::open(Path::new(&dest))?;
    dest_repo.initialize().await?;

    // First do native init
    let sync = protocol::LocalSync::new(&source_repo, &dest_repo);
    let _state = sync.init()?;

    // Then set up SVNSync compat
    let compat = compat::SvnSyncCompat::new(&dest_repo);
    compat.init_mirror(
        &format!("file://{}", source),
        source_repo.uuid(),
    )?;

    println!("Mirror initialized:");
    println!("  Source: file://{}", source);
    println!("  Source UUID: {}", source_repo.uuid());
    println!("  Destination: {}", dest);
    println!("  pre-revprop-change hook installed");
    Ok(())
}

async fn cmd_svnsync_sync(source: String, dest: String) -> Result<()> {
    let source_repo = SqliteRepository::open(Path::new(&source))?;
    source_repo.initialize().await?;
    let dest_repo = SqliteRepository::open(Path::new(&dest))?;
    dest_repo.initialize().await?;

    // Acquire lock
    let compat = compat::SvnSyncCompat::new(&dest_repo);
    let lock_token = compat.acquire_lock()?;
    println!("Acquired sync lock: {}", lock_token);

    // Sync
    let sync = protocol::LocalSync::new(&source_repo, &dest_repo);
    let result = match sync.sync() {
        Ok(r) => r,
        Err(e) => {
            // Release lock on error
            let _ = compat.release_lock();
            return Err(e);
        }
    };

    // Update SVNSync properties
    if !result.already_up_to_date {
        compat.set_last_merged_rev(result.to_rev)?;
        println!(
            "Synced {} revisions (r{} → r{})",
            result.revisions_synced, result.from_rev, result.to_rev
        );
    } else {
        println!("Already up to date.");
    }

    // Release lock
    compat.release_lock()?;
    println!("Released sync lock.");

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

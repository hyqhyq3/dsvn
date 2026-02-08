//! dsvnsync — Repository synchronization tool for dsvn.
//!
//! Supports:
//! - Local-to-local repository sync (init/sync/info/cleanup)
//! - SVNSync compatibility mode
//! - Sync verification
//! - Replication log management

mod compat;
mod protocol;
mod remote;
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
        #[arg(short, long)]
        source: String,
        #[arg(short, long)]
        dest: String,
        #[arg(long)]
        svnsync_compat: bool,
    },

    /// Perform incremental sync from source to destination
    Sync {
        #[arg(short, long)]
        source: String,
        #[arg(short, long)]
        dest: String,
        #[arg(long)]
        verify: bool,
    },

    /// Display sync status and information
    Info {
        repo: String,
    },

    /// Verify sync integrity between source and destination
    Verify {
        #[arg(short, long)]
        source: String,
        #[arg(short, long)]
        dest: String,
        #[arg(short = 'r', long)]
        revision: Option<u64>,
    },

    /// View replication log
    #[command(name = "repl-log")]
    ReplLog {
        repo: String,
        #[arg(long)]
        from: Option<u64>,
        #[arg(long)]
        to: Option<u64>,
    },

    /// Clean up sync state and replication log
    Cleanup {
        repo: String,
        #[arg(long)]
        remove_hooks: bool,
    },

    /// SVNSync compatibility: initialize mirror
    #[command(name = "svnsync-init")]
    SvnSyncInit {
        #[arg(short, long)]
        source: String,
        #[arg(short, long)]
        dest: String,
    },

    /// SVNSync compatibility: sync mirror
    #[command(name = "svnsync-sync")]
    SvnSyncSync {
        #[arg(short, long)]
        source: String,
        #[arg(short, long)]
        dest: String,
    },

    /// Pull from a remote HTTP dsvn server
    Pull {
        /// Remote server URL (e.g. http://server:8080)
        #[arg(short, long)]
        source: String,
        /// Local repository path
        #[arg(short, long)]
        dest: String,
        /// Optional cache directory for objects
        #[arg(long)]
        cache: Option<String>,
        /// Initialize sync relationship first (if not already done)
        #[arg(long)]
        init: bool,
    },

    /// Show remote server sync info
    #[command(name = "remote-info")]
    RemoteInfo {
        /// Remote server URL (e.g. http://server:8080)
        url: String,
    },

    /// Clean expired objects from sync cache
    #[command(name = "cache-clean")]
    CacheClean {
        /// Cache directory
        #[arg(short, long)]
        cache_dir: String,
        /// Maximum age in hours (default: 720 = 30 days)
        #[arg(long, default_value = "720")]
        max_age_hours: u32,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("dsvnsync=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init { source, dest, svnsync_compat } =>
            cmd_init(source, dest, svnsync_compat).await,
        Commands::Sync { source, dest, verify } =>
            cmd_sync(source, dest, verify).await,
        Commands::Info { repo } =>
            cmd_info(repo).await,
        Commands::Verify { source, dest, revision } =>
            cmd_verify(source, dest, revision).await,
        Commands::ReplLog { repo, from, to } =>
            cmd_repl_log(repo, from, to).await,
        Commands::Cleanup { repo, remove_hooks } =>
            cmd_cleanup(repo, remove_hooks).await,
        Commands::SvnSyncInit { source, dest } =>
            cmd_svnsync_init(source, dest).await,
        Commands::SvnSyncSync { source, dest } =>
            cmd_svnsync_sync(source, dest).await,
        Commands::Pull { source, dest, cache, init } =>
            cmd_pull(source, dest, cache, init).await,
        Commands::RemoteInfo { url } =>
            cmd_remote_info(url).await,
        Commands::CacheClean { cache_dir, max_age_hours } =>
            cmd_cache_clean(cache_dir, max_age_hours).await,
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
        compat.init_mirror(&format!("file://{}", source), source_repo.uuid())?;
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
    // Run sync in a blocking context since commit_sync uses blocking_lock
    let result = tokio::task::block_in_place(|| sync.sync())?;

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
        let rev_per_sec = (result.revisions_synced as f64 / result.duration_ms as f64) * 1000.0;
        println!("  Speed:     {:.1} revisions/sec", rev_per_sec);
    }

    // Update SVNSync compat properties if configured
    let svn_compat = compat::SvnSyncCompat::new(&dest_repo);
    if svn_compat.is_mirror()? {
        svn_compat.set_last_merged_rev(result.to_rev)?;
    }

    if verify {
        println!("\nVerifying sync...");
        let verify_result = transfer::verify_sync(&source_repo, &dest_repo, result.to_rev)?;
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
            let behind = s.source_head_rev.saturating_sub(s.last_synced_rev);
            println!("  Behind source:    {} revisions", behind);
        }
        None => {
            println!("\nNo sync state found (not a sync destination).");
        }
    }

    let svn_compat = compat::SvnSyncCompat::new(&repository);
    if svn_compat.is_mirror()? {
        println!("\nSVNSync Compatibility:");
        println!("  Source URL:       {}", svn_compat.get_source_url()?.unwrap_or_default());
        println!("  Source UUID:      {}", svn_compat.get_source_uuid()?.unwrap_or_default());
        println!("  Last merged rev:  r{}", svn_compat.get_last_merged_rev()?);
        if let Some(copying) = svn_compat.get_currently_copying()? {
            println!("  Currently copying: r{}", copying);
        }
    }

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
    replication_log::print_repl_log(Path::new(&repo), from, to, &mut std::io::stdout())?;
    Ok(())
}

async fn cmd_cleanup(repo: String, remove_hooks: bool) -> Result<()> {
    let repo_path = Path::new(&repo);
    println!("Cleaning up sync state for: {}", repo);

    dsvn_core::SyncState::remove(repo_path)?;
    println!("  Removed sync-state.json");

    let revprops_path = repo_path.join("revprops").join("0.json");
    if revprops_path.exists() {
        let data = std::fs::read_to_string(&revprops_path)?;
        let mut props: std::collections::HashMap<String, String> = serde_json::from_str(&data)?;
        let sync_keys: Vec<String> = props.keys().filter(|k| k.starts_with("svn:sync-")).cloned().collect();
        for key in &sync_keys { props.remove(key); }
        if props.is_empty() { std::fs::remove_file(&revprops_path)?; }
        else { std::fs::write(&revprops_path, serde_json::to_string_pretty(&props)?)?; }
        if !sync_keys.is_empty() { println!("  Removed {} sync properties", sync_keys.len()); }
    }

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

    let sync = protocol::LocalSync::new(&source_repo, &dest_repo);
    let _state = sync.init()?;

    let compat = compat::SvnSyncCompat::new(&dest_repo);
    compat.init_mirror(&format!("file://{}", source), source_repo.uuid())?;

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

    let compat = compat::SvnSyncCompat::new(&dest_repo);
    let lock_token = compat.acquire_lock()?;
    println!("Acquired sync lock: {}", lock_token);

    let sync = protocol::LocalSync::new(&source_repo, &dest_repo);
    let result = match tokio::task::block_in_place(|| sync.sync()) {
        Ok(r) => r,
        Err(e) => {
            let _ = compat.release_lock();
            return Err(e);
        }
    };

    if !result.already_up_to_date {
        compat.set_last_merged_rev(result.to_rev)?;
        println!("Synced {} revisions (r{} → r{})", result.revisions_synced, result.from_rev, result.to_rev);
    } else {
        println!("Already up to date.");
    }

    compat.release_lock()?;
    println!("Released sync lock.");
    Ok(())
}

async fn cmd_pull(source: String, dest: String, cache: Option<String>, init: bool) -> Result<()> {
    let dest_path = Path::new(&dest);

    // Create destination repo if it doesn't exist
    let dest_repo = SqliteRepository::open(dest_path)?;
    dest_repo.initialize().await?;
    drop(dest_repo); // release before RemotePull takes over

    let cache_dir = cache.map(|c| std::path::PathBuf::from(c));
    let puller = remote::RemotePull::new(&source, dest_path, cache_dir);

    // Auto-init if requested
    if init || dsvn_core::SyncState::load(dest_path)?.is_none() {
        println!("Initializing sync relationship with {}...", source);
        let state = puller.init().await?;
        println!("  Source UUID:  {}", state.source_uuid);
        println!("  Source HEAD:  r{}", state.source_head_rev);
        println!();
    }

    println!("Pulling from {}...", source);
    let result = puller.pull().await?;

    if result.already_up_to_date {
        println!("Already up to date.");
        return Ok(());
    }

    println!("\nPull completed:");
    println!("  Revisions: r{} → r{}", result.from_rev, result.to_rev);
    println!("  Synced:    {} revisions", result.revisions_synced);
    println!("  Objects:   {} transferred, {} from cache/existing",
             result.objects_transferred, result.objects_cached);
    println!("  Bytes:     {}", format_size(result.bytes_transferred));
    println!("  Time:      {}ms", result.duration_ms);
    if result.duration_ms > 0 {
        let rev_per_sec = (result.revisions_synced as f64 / result.duration_ms as f64) * 1000.0;
        println!("  Speed:     {:.1} revisions/sec", rev_per_sec);
    }

    Ok(())
}

async fn cmd_remote_info(url: String) -> Result<()> {
    let client = remote::RemoteSyncClient::new(&url);

    println!("Querying remote server: {}", url);
    let info = client.get_info().await?;

    println!("\nRemote Repository:");
    println!("  UUID:             {}", info.uuid);
    println!("  HEAD revision:    r{}", info.head_rev);
    println!("  Protocol version: {}", info.protocol_version);
    println!("  Capabilities:     {}", info.capabilities.join(", "));

    // Also fetch config if available
    match client.get_config().await {
        Ok(config) => {
            println!("\nSync Configuration:");
            println!("  Enabled:          {}", config.enabled);
            println!("  Require auth:     {}", config.require_auth);
            println!("  Max cache age:    {} hours", config.max_cache_age_hours);
            println!("  Allowed sources:  {}", config.allowed_sources.join(", "));
            if let Some(ref cache_dir) = config.cache_dir {
                println!("  Cache dir:        {}", cache_dir.display());
            }
        }
        Err(e) => {
            println!("\n  (Could not fetch sync config: {})", e);
        }
    }

    Ok(())
}

async fn cmd_cache_clean(cache_dir: String, max_age_hours: u32) -> Result<()> {
    let path = Path::new(&cache_dir);
    println!("Cleaning cache: {}", cache_dir);
    println!("  Max age: {} hours", max_age_hours);

    let result = remote::clean_cache(path, max_age_hours)?;
    println!("  Files removed: {}", result.files_removed);
    println!("  Bytes freed:   {}", format_size(result.bytes_freed));
    Ok(())
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB { format!("{:.2} GB", bytes as f64 / GB as f64) }
    else if bytes >= MB { format!("{:.2} MB", bytes as f64 / MB as f64) }
    else if bytes >= KB { format!("{:.1} KB", bytes as f64 / KB as f64) }
    else { format!("{} B", bytes) }
}

//! DSvn Administration CLI

mod dump;
mod dump_format;
mod load;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use dsvn_core::SqliteRepository;
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::Path;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "dsvn-admin")]
#[command(author = "DSvn Contributors")]
#[command(version = "0.1.0")]
#[command(about = "DSvn repository administration tool (compatible with svnadmin)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize a new repository (equivalent to svnadmin create)
    #[command(alias = "create")]
    Init { path: String },

    /// Load SVN dump file into repository
    Load {
        #[arg(short, long)]
        file: String,
        #[arg(short, long)]
        repo: String,
    },

    /// Dump repository to SVN dump format
    Dump {
        #[arg(short, long)]
        repo: String,
        #[arg(short, long, default_value = "-")]
        output: String,
        #[arg(short, long)]
        start: Option<u64>,
        #[arg(short, long)]
        end: Option<u64>,
    },

    /// Verify repository integrity
    Verify {
        repo: String,
        #[arg(short, long)]
        start: Option<u64>,
        #[arg(short, long)]
        end: Option<u64>,
        #[arg(short, long)]
        quiet: bool,
    },

    /// Display repository information
    Info { repo: String },

    /// Set repository UUID
    #[command(name = "setuuid")]
    SetUuid {
        repo: String,
        uuid: Option<String>,
    },

    /// Hot-copy repository to a new location
    #[command(name = "hotcopy")]
    HotCopy {
        src: String,
        dst: String,
        #[arg(long)]
        clean: bool,
    },

    /// Pack/optimize repository storage
    Pack { repo: String },

    /// List locks in repository
    #[command(name = "lslocks")]
    LsLocks { repo: String },

    /// Remove locks from repository
    #[command(name = "rmlocks")]
    RmLocks {
        repo: String,
        path: Option<String>,
    },

    /// Set a revision property
    #[command(name = "setrevprop")]
    SetRevProp {
        repo: String,
        #[arg(short = 'r', long)]
        revision: u64,
        #[arg(short, long)]
        name: String,
        #[arg(short, long)]
        value: Option<String>,
    },

    /// Delete a revision property
    #[command(name = "delrevprop")]
    DelRevProp {
        repo: String,
        #[arg(short = 'r', long)]
        revision: u64,
        #[arg(short, long)]
        name: String,
    },

    /// Export replication log for a revision range
    #[command(name = "repl-log")]
    ReplLog {
        #[arg(short, long)]
        repo: String,
        #[arg(long)]
        from: Option<u64>,
        #[arg(long)]
        to: Option<u64>,
    },
}

// ─── Helpers ────────────────────────────────────────────────

fn dir_size(path: &Path) -> u64 {
    if !path.exists() { return 0; }
    if path.is_file() { return path.metadata().map(|m| m.len()).unwrap_or(0); }
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for e in entries.flatten() {
            let p = e.path();
            total += if p.is_dir() { dir_size(&p) } else { p.metadata().map(|m| m.len()).unwrap_or(0) };
        }
    }
    total
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

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let sp = entry.path();
        let dp = dst.join(entry.file_name());
        if sp.is_dir() {
            if entry.file_name() == "tree.db" { continue; }
            copy_dir_recursive(&sp, &dp)?;
        } else {
            let ns = entry.file_name().to_string_lossy().to_string();
            if ns.ends_with("-wal") || ns.ends_with("-shm") || ns.ends_with(".tmp") { continue; }
            fs::copy(&sp, &dp)?;
        }
    }
    Ok(())
}

fn remove_tmp_files(path: &Path, count: &mut u64) -> Result<()> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() { remove_tmp_files(&p, count)?; }
            else if p.extension().map(|e| e == "tmp").unwrap_or(false) {
                fs::remove_file(&p)?; *count += 1;
            }
        }
    }
    Ok(())
}

fn load_delta_tree(repo: &SqliteRepository, rev: u64) -> Result<dsvn_core::DeltaTree> {
    let path = repo.root().join("tree_deltas").join(format!("{}.bin", rev));
    let data = fs::read(&path)?;
    Ok(bincode::deserialize(&data)?)
}

fn load_revprops(repo_path: &Path, rev: u64) -> std::collections::HashMap<String, String> {
    let path = repo_path.join("revprops").join(format!("{}.json", rev));
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(props) = serde_json::from_str(&data) { return props; }
    }
    std::collections::HashMap::new()
}

fn save_revprops(repo_path: &Path, rev: u64, props: &std::collections::HashMap<String, String>) -> Result<()> {
    let dir = repo_path.join("revprops");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", rev));
    if props.is_empty() { if path.exists() { fs::remove_file(&path)?; } }
    else { fs::write(&path, serde_json::to_string_pretty(props)?)?; }
    Ok(())
}

fn write_revision(writer: &mut dyn Write, repo: &SqliteRepository, rev: u64) -> Result<()> {
    writeln!(writer, "Revision-number: {}", rev)?;
    let rt = tokio::runtime::Handle::current();
    let commit_opt = rt.block_on(repo.get_commit(rev));
    if let Some(commit) = commit_opt {
        let date = chrono::DateTime::from_timestamp(commit.timestamp, 0)
            .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string())
            .unwrap_or_default();
        let mut props = String::new();
        if !commit.author.is_empty() {
            props.push_str(&format!("K 10\nsvn:author\nV {}\n{}\n", commit.author.len(), commit.author));
        }
        if !commit.message.is_empty() {
            props.push_str(&format!("K 7\nsvn:log\nV {}\n{}\n", commit.message.len(), commit.message));
        }
        if !date.is_empty() {
            props.push_str(&format!("K 8\nsvn:date\nV {}\n{}\n", date.len(), date));
        }
        props.push_str("PROPS-END\n");
        let pb = props.as_bytes();
        writeln!(writer, "Prop-content-length: {}", pb.len())?;
        writeln!(writer, "Content-length: {}", pb.len())?;
        writeln!(writer)?;
        writer.write_all(pb)?;
        writeln!(writer)?;
    } else {
        let props = "PROPS-END\n";
        writeln!(writer, "Prop-content-length: {}", props.len())?;
        writeln!(writer, "Content-length: {}", props.len())?;
        writeln!(writer)?;
        write!(writer, "{}", props)?;
        writeln!(writer)?;
    }
    if rev > 0 {
        if let Ok(delta) = load_delta_tree(repo, rev) {
            for change in &delta.changes {
                match change {
                    dsvn_core::TreeChange::Upsert { path, entry } => {
                        writeln!(writer, "Node-path: {}", path)?;
                        writeln!(writer, "Node-kind: {}", if entry.kind == dsvn_core::ObjectKind::Blob { "file" } else { "dir" })?;
                        writeln!(writer, "Node-action: add")?;
                        if entry.kind == dsvn_core::ObjectKind::Blob {
                            if let Ok(content) = rt.block_on(repo.get_file(&format!("/{}", path), rev)) {
                                let pe = "PROPS-END\n";
                                writeln!(writer, "Prop-content-length: {}", pe.len())?;
                                writeln!(writer, "Text-content-length: {}", content.len())?;
                                writeln!(writer, "Content-length: {}", pe.len() + content.len())?;
                                writeln!(writer)?;
                                write!(writer, "{}", pe)?;
                                writer.write_all(&content)?;
                                writeln!(writer)?;
                            }
                        } else {
                            let pe = "PROPS-END\n";
                            writeln!(writer, "Prop-content-length: {}", pe.len())?;
                            writeln!(writer, "Content-length: {}", pe.len())?;
                            writeln!(writer)?;
                            write!(writer, "{}", pe)?;
                            writeln!(writer)?;
                        }
                    }
                    dsvn_core::TreeChange::Delete { path } => {
                        writeln!(writer, "Node-path: {}", path)?;
                        writeln!(writer, "Node-action: delete")?;
                        writeln!(writer)?;
                    }
                }
            }
        }
    }
    Ok(())
}

// ─── Commands Implementation ────────────────────────────────

async fn cmd_init(path: String) -> Result<()> {
    println!("Initializing repository at {}", path);
    let repo = SqliteRepository::open(Path::new(&path))?;
    repo.initialize().await?;
    println!("Repository initialized successfully (UUID: {})", repo.uuid());
    Ok(())
}

async fn cmd_load(file: String, repo: String) -> Result<()> {
    println!("Loading SVN dump file: {}", file);
    println!("  Backend: SQLite (WAL mode)");
    let repository = SqliteRepository::open(Path::new(&repo))?;
    repository.initialize().await?;
    let repository = Arc::new(repository);
    if file == "-" {
        load::load_dump_file(repository, BufReader::new(std::io::stdin())).await?;
    } else {
        load::load_dump_file(repository, BufReader::new(File::open(&file)?)).await?;
    }
    Ok(())
}

async fn cmd_setrevprop(repo: String, revision: u64, name: String, value: Option<String>) -> Result<()> {
    let repo_path = Path::new(&repo);
    let repository = SqliteRepository::open(repo_path)?;
    repository.initialize().await?;
    let head = repository.current_rev().await;
    if revision > head {
        return Err(anyhow!("Revision {} does not exist (HEAD is r{})", revision, head));
    }

    let value = match value {
        Some(v) => v,
        None => {
            let mut buf = String::new();
            std::io::stdin().read_line(&mut buf)?;
            buf.trim_end().to_string()
        }
    };

    let hook_mgr = dsvn_core::HookManager::new(repo_path.to_path_buf());

    match name.as_str() {
        "svn:log" | "svn:author" | "svn:date" => {
            let commit_file = repo_path.join("commits").join(format!("{}.bin", revision));
            if !commit_file.exists() {
                return Err(anyhow!("Commit for revision {} not found", revision));
            }
            let data = fs::read(&commit_file)?;
            let mut commit: dsvn_core::Commit = bincode::deserialize(&data)?;
            hook_mgr.run_pre_revprop_change(revision, &commit.author, &name, "M", &value)?;
            match name.as_str() {
                "svn:log" => commit.message = value.clone(),
                "svn:author" => commit.author = value.clone(),
                "svn:date" => {
                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&value) {
                        commit.timestamp = dt.timestamp();
                    } else {
                        return Err(anyhow!("Invalid date format (expected RFC 3339): {}", value));
                    }
                }
                _ => unreachable!(),
            }
            fs::write(&commit_file, bincode::serialize(&commit)?)?;
            hook_mgr.run_post_revprop_change(revision, &commit.author, &name, "M")?;
        }
        _ => {
            let author = repository.get_commit(revision).await
                .map(|c| c.author.clone()).unwrap_or_default();
            hook_mgr.run_pre_revprop_change(revision, &author, &name, "M", &value)?;
            let mut props = load_revprops(repo_path, revision);
            props.insert(name.clone(), value);
            save_revprops(repo_path, revision, &props)?;
            hook_mgr.run_post_revprop_change(revision, &author, &name, "M")?;
        }
    }
    println!("Property '{}' set on revision {}", name, revision);
    Ok(())
}

async fn cmd_delrevprop(repo: String, revision: u64, name: String) -> Result<()> {
    let repo_path = Path::new(&repo);
    let repository = SqliteRepository::open(repo_path)?;
    repository.initialize().await?;
    let head = repository.current_rev().await;
    if revision > head {
        return Err(anyhow!("Revision {} does not exist (HEAD is r{})", revision, head));
    }

    let hook_mgr = dsvn_core::HookManager::new(repo_path.to_path_buf());
    let author = repository.get_commit(revision).await
        .map(|c| c.author.clone()).unwrap_or_default();

    match name.as_str() {
        "svn:log" | "svn:author" => {
            hook_mgr.run_pre_revprop_change(revision, &author, &name, "D", "")?;
            let commit_file = repo_path.join("commits").join(format!("{}.bin", revision));
            if !commit_file.exists() {
                return Err(anyhow!("Commit for revision {} not found", revision));
            }
            let data = fs::read(&commit_file)?;
            let mut commit: dsvn_core::Commit = bincode::deserialize(&data)?;
            match name.as_str() {
                "svn:log" => commit.message = String::new(),
                "svn:author" => commit.author = String::new(),
                _ => unreachable!(),
            }
            fs::write(&commit_file, bincode::serialize(&commit)?)?;
            hook_mgr.run_post_revprop_change(revision, &author, &name, "D")?;
        }
        "svn:date" => {
            return Err(anyhow!("Cannot delete svn:date property"));
        }
        _ => {
            hook_mgr.run_pre_revprop_change(revision, &author, &name, "D", "")?;
            let mut props = load_revprops(repo_path, revision);
            if props.remove(&name).is_none() {
                return Err(anyhow!("Property '{}' not found on revision {}", name, revision));
            }
            save_revprops(repo_path, revision, &props)?;
            hook_mgr.run_post_revprop_change(revision, &author, &name, "D")?;
        }
    }
    println!("Property '{}' deleted from revision {}", name, revision);
    Ok(())
}

// ─── Main ───────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path } => cmd_init(path).await,
        Commands::Load { file, repo } => cmd_load(file, repo).await,

        Commands::Dump { repo, output, start, end } => {
            let repository = SqliteRepository::open(Path::new(&repo))?;
            repository.initialize().await?;
            let head = repository.current_rev().await;
            let start_rev = start.unwrap_or(0);
            if start_rev > head { return Err(anyhow!("Start revision {} exceeds HEAD ({})", start_rev, head)); }
            let end_rev = end.unwrap_or(head).min(head);
            eprintln!("Dumping revisions {} to {} ...", start_rev, end_rev);
            let write_dump = |w: &mut dyn Write| -> Result<()> {
                writeln!(w, "SVN-fs-dump-format-version: 2")?;
                writeln!(w)?;
                writeln!(w, "UUID: {}", repository.uuid())?;
                writeln!(w)?;
                for rev in start_rev..=end_rev { write_revision(w, &repository, rev)?; }
                Ok(())
            };
            if output == "-" {
                write_dump(&mut std::io::stdout().lock())?;
            } else {
                write_dump(&mut File::create(&output)?)?;
                eprintln!("Dump written to {}", output);
            }
            Ok(())
        }

        Commands::Verify { repo, start, end, quiet } => {
            let repository = SqliteRepository::open(Path::new(&repo))?;
            repository.initialize().await?;
            let head = repository.current_rev().await;
            let start_rev = start.unwrap_or(0);
            let end_rev = end.unwrap_or(head).min(head);
            if !quiet { println!("Verifying repository: {}\n  Revisions: {} to {}\n", repo, start_rev, end_rev); }
            let mut errors = 0u64;
            let mut warnings = 0u64;
            let mut verified_revs = 0u64;
            let mut verified_objects = 0u64;
            for rev in start_rev..=end_rev {
                match repository.get_commit(rev).await {
                    Some(commit) => {
                        verified_objects += 1;
                        if commit.author.is_empty() && rev > 0 { warnings += 1; if !quiet { eprintln!("  WARNING: r{} has empty author", rev); } }
                        match repository.get_tree_at_rev(rev) {
                            Ok(tree_map) => {
                                verified_objects += 1;
                                for (path, entry) in &tree_map {
                                    if entry.kind == dsvn_core::ObjectKind::Blob {
                                        match repository.get_file(&format!("/{}", path), rev).await {
                                            Ok(_) => { verified_objects += 1; }
                                            Err(e) => { errors += 1; eprintln!("  ERROR: r{} object missing for '{}': {}", rev, path, e); }
                                        }
                                    }
                                }
                            }
                            Err(e) => { errors += 1; eprintln!("  ERROR: r{} tree reconstruction failed: {}", rev, e); }
                        }
                        if rev > 0 {
                            match load_delta_tree(&repository, rev) {
                                Ok(_) => { verified_objects += 1; }
                                Err(_) => { warnings += 1; if !quiet { eprintln!("  WARNING: r{} has no delta tree (legacy format?)", rev); } }
                            }
                        }
                    }
                    None => { errors += 1; eprintln!("  ERROR: r{} commit not found", rev); }
                }
                verified_revs += 1;
                if !quiet && (rev % 100 == 0 || rev == end_rev) {
                    eprint!("\r  Verified: r{}/{} ({} objects, {} errors)    ", rev, end_rev, verified_objects, errors);
                }
            }
            if !quiet { eprintln!(); }
            println!("\nVerification complete:");
            println!("  Revisions verified: {}", verified_revs);
            println!("  Objects verified:   {}", verified_objects);
            println!("  Errors:             {}", errors);
            println!("  Warnings:           {}", warnings);
            if errors > 0 { Err(anyhow!("Repository verification failed with {} error(s)", errors)) }
            else { println!("  Status:             OK ✓"); Ok(()) }
        }

        Commands::Info { repo } => {
            let rp = Path::new(&repo);
            let repository = SqliteRepository::open(rp)?;
            repository.initialize().await?;
            let head = repository.current_rev().await;
            let (fc, dc) = if head > 0 {
                repository.get_tree_at_rev(head).map(|tm| {
                    (tm.values().filter(|e| e.kind == dsvn_core::ObjectKind::Blob).count(),
                     tm.values().filter(|e| e.kind == dsvn_core::ObjectKind::Tree).count())
                }).unwrap_or((0, 0))
            } else { (0, 0) };
            println!("Repository: {}", rp.canonicalize().unwrap_or_else(|_| rp.to_path_buf()).display());
            println!("UUID:       {}", repository.uuid());
            println!("Head:       r{}", head);
            println!("Files:      {}", fc);
            println!("Dirs:       {}", dc);
            println!("Disk size:  {}", format_size(dir_size(rp)));
            println!("Backend:    SQLite (WAL)");
            println!("\nStorage breakdown:");
            for (label, sub) in [("objects/","objects"),("commits/","commits"),("trees/","trees"),("tree_deltas/","tree_deltas"),("props/","props")] {
                println!("  {:<14} {}", label, format_size(dir_size(&rp.join(sub))));
            }
            println!("  {:<14} {}", "tree.sqlite", format_size(rp.join("tree.sqlite").metadata().map(|m| m.len()).unwrap_or(0)));
            Ok(())
        }

        Commands::SetUuid { repo, uuid } => {
            let rp = Path::new(&repo);
            if !rp.join("uuid").exists() { return Err(anyhow!("Not a valid dsvn repository: {}", repo)); }
            let old = fs::read_to_string(rp.join("uuid"))?.trim().to_string();
            let new = match uuid {
                Some(u) => { if uuid::Uuid::parse_str(&u).is_err() { return Err(anyhow!("Invalid UUID format: {}", u)); } u }
                None => uuid::Uuid::new_v4().to_string(),
            };
            fs::write(rp.join("uuid"), &new)?;
            println!("UUID changed:\n  Old: {}\n  New: {}", old, new);
            Ok(())
        }

        Commands::HotCopy { src, dst, clean } => {
            let sp = Path::new(&src);
            let dp = Path::new(&dst);
            if !sp.join("uuid").exists() { return Err(anyhow!("Not a valid dsvn repository: {}", src)); }
            if dp.exists() {
                if clean { fs::remove_dir_all(dp)?; }
                else { return Err(anyhow!("Destination already exists: {} (use --clean to overwrite)", dst)); }
            }
            println!("Hot-copying repository...\n  Source:      {}\n  Destination: {}", src, dst);
            copy_dir_recursive(sp, dp)?;
            let cr = SqliteRepository::open(dp)?; cr.initialize().await?; let crev = cr.current_rev().await;
            let sr = SqliteRepository::open(sp)?; sr.initialize().await?; let srev = sr.current_rev().await;
            println!("\nHot-copy complete:\n  Source HEAD:  r{}\n  Copy HEAD:   r{}\n  Copy size:   {}", srev, crev, format_size(dir_size(dp)));
            if srev != crev { eprintln!("  WARNING: HEAD revision mismatch!"); } else { println!("  Status:      OK ✓"); }
            Ok(())
        }

        Commands::Pack { repo } => {
            let rp = Path::new(&repo);
            if !rp.join("uuid").exists() { return Err(anyhow!("Not a valid dsvn repository: {}", repo)); }
            let repository = SqliteRepository::open(rp)?;
            repository.initialize().await?;
            println!("Packing repository: {}", repo);
            let sb = dir_size(rp);
            let mut tc = 0u64;
            remove_tmp_files(rp, &mut tc)?;
            let sled = rp.join("tree.db");
            if sled.exists() { let s = dir_size(&sled); fs::remove_dir_all(&sled)?; println!("  Removed legacy sled database ({})", format_size(s)); }
            let rtb = rp.join("root_tree.bin");
            if rtb.exists() { let s = rtb.metadata().map(|m| m.len()).unwrap_or(0); fs::remove_file(&rtb)?; println!("  Removed legacy root_tree.bin ({})", format_size(s)); }
            let dbp = rp.join("tree.sqlite");
            if dbp.exists() {
                let before = dbp.metadata().map(|m| m.len()).unwrap_or(0);
                drop(repository);
                let conn = rusqlite::Connection::open(&dbp)?;
                conn.execute_batch("VACUUM")?;
                drop(conn);
                let after = dbp.metadata().map(|m| m.len()).unwrap_or(0);
                if before > after { println!("  SQLite VACUUM saved {}", format_size(before - after)); }
                else { println!("  SQLite already compact"); }
            }
            for ext in ["tree.sqlite-wal", "tree.sqlite-shm"] {
                let p = rp.join(ext);
                if p.exists() { let s = p.metadata().map(|m| m.len()).unwrap_or(0); fs::remove_file(&p).ok(); if s > 0 { println!("  Removed {} ({})", ext, format_size(s)); } }
            }
            let sa = dir_size(rp);
            println!("\nPack complete:\n  Size before: {}\n  Size after:  {}", format_size(sb), format_size(sa));
            if sb > sa { println!("  Saved:       {}", format_size(sb - sa)); }
            if tc > 0 { println!("  Tmp files cleaned: {}", tc); }
            Ok(())
        }

        Commands::LsLocks { repo } => {
            let rp = Path::new(&repo);
            if !rp.join("uuid").exists() { return Err(anyhow!("Not a valid dsvn repository: {}", repo)); }
            let ld = rp.join("locks");
            if !ld.exists() { println!("No locks found."); return Ok(()); }
            let entries: Vec<_> = fs::read_dir(&ld)?.flatten().filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false)).collect();
            if entries.is_empty() { println!("No locks found."); return Ok(()); }
            println!("{:<40} {:<20} {:<30} {}", "Path", "Owner", "Created", "Token");
            println!("{}", "-".repeat(110));
            for e in entries {
                if let Ok(data) = fs::read_to_string(e.path()) {
                    if let Ok(lock) = serde_json::from_str::<serde_json::Value>(&data) {
                        println!("{:<40} {:<20} {:<30} {}",
                            lock.get("path").and_then(|v| v.as_str()).unwrap_or("?"),
                            lock.get("owner").and_then(|v| v.as_str()).unwrap_or("?"),
                            lock.get("created").and_then(|v| v.as_str()).unwrap_or("?"),
                            lock.get("token").and_then(|v| v.as_str()).unwrap_or("?"));
                    }
                }
            }
            Ok(())
        }

        Commands::RmLocks { repo, path } => {
            let rp = Path::new(&repo);
            if !rp.join("uuid").exists() { return Err(anyhow!("Not a valid dsvn repository: {}", repo)); }
            let ld = rp.join("locks");
            if !ld.exists() { println!("No locks directory found."); return Ok(()); }
            let mut removed = 0u64;
            for entry in fs::read_dir(&ld)?.flatten() {
                if !entry.path().extension().map(|e| e == "json").unwrap_or(false) { continue; }
                if let Some(ref tp) = path {
                    if let Ok(data) = fs::read_to_string(entry.path()) {
                        if let Ok(lock) = serde_json::from_str::<serde_json::Value>(&data) {
                            let lp = lock.get("path").and_then(|v| v.as_str()).unwrap_or("");
                            if lp == tp.as_str() {
                                fs::remove_file(entry.path())?;
                                removed += 1;
                                println!("  Removed lock: {}", lp);
                            }
                        }
                    }
                } else {
                    if let Ok(data) = fs::read_to_string(entry.path()) {
                        if let Ok(lock) = serde_json::from_str::<serde_json::Value>(&data) {
                            println!("  Removed lock: {}", lock.get("path").and_then(|v| v.as_str()).unwrap_or("?"));
                        }
                    }
                    fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
            println!("Removed {} lock(s).", removed);
            Ok(())
        }

        Commands::SetRevProp { repo, revision, name, value } => {
            cmd_setrevprop(repo, revision, name, value).await
        }

        Commands::DelRevProp { repo, revision, name } => {
            cmd_delrevprop(repo, revision, name).await
        }

        Commands::ReplLog { repo, from, to } => {
            let rp = Path::new(&repo);
            if !rp.join("uuid").exists() { return Err(anyhow!("Not a valid dsvn repository: {}", repo)); }
            let repository = SqliteRepository::open(rp)?;
            repository.initialize().await?;
            let head = repository.current_rev().await;

            let from_rev = from.unwrap_or(0);
            let to_rev = to.unwrap_or(head);

            println!("Replication log for: {}", repo);
            println!("  Revision range: r{} to r{}", from_rev, to_rev);
            println!();

            // Check for native dsvn replication log
            let repl_log = dsvn_core::ReplicationLog::new(rp);
            let entries = repl_log.query(from_rev, to_rev)?;

            if entries.is_empty() {
                // No native repl log — generate from commits/deltas
                println!("No native replication log entries found.");
                println!("Generating from commit history:\n");
                for rev in from_rev..=to_rev.min(head) {
                    if let Some(commit) = repository.get_commit(rev).await {
                        let date = chrono::DateTime::from_timestamp(commit.timestamp, 0)
                            .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string())
                            .unwrap_or_default();
                        println!("r{:<6} | {:<20} | {} | {}",
                            rev, commit.author,
                            &date[..19],
                            if commit.message.len() > 60 { format!("{}...", &commit.message[..57]) } else { commit.message.clone() }
                        );
                    }
                }
            } else {
                println!("Found {} replication log entries:\n", entries.len());
                for entry in &entries {
                    let date = chrono::DateTime::from_timestamp(entry.timestamp, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                        .unwrap_or_else(|| entry.timestamp.to_string());
                    let status = if entry.success { "OK" } else { "FAILED" };
                    println!("[{}] r{}-r{} | {} objects, {} | {}ms | {}",
                        date, entry.from_rev, entry.to_rev,
                        entry.objects_transferred,
                        format_size(entry.bytes_transferred),
                        entry.duration_ms,
                        status);
                }

                // Summary
                let total_objects: u64 = entries.iter().map(|e| e.objects_transferred).sum();
                let total_bytes: u64 = entries.iter().map(|e| e.bytes_transferred).sum();
                let successes = entries.iter().filter(|e| e.success).count();
                println!("\nSummary: {} syncs ({} successful), {} objects, {}",
                    entries.len(), successes, total_objects, format_size(total_bytes));
            }
            Ok(())
        }
    }
}
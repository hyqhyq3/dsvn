# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

DSvn is a high-performance Subversion-compatible server written in Rust that **speaks the SVN WebDAV/DeltaV protocol but uses a completely different storage engine**. The storage is **NOT compatible** with FSFS format - it uses content-addressable storage (Git-like) for massive scalability (billions of files, millions of commits).

## Critical Architecture Decisions

1. **Protocol Compatibility, Storage Incompatibility**
   - Clients use standard SVN protocol over HTTP/WebDAV
   - Storage is content-addressable (SHA-256), NOT FSFS
   - Migration requires dump/load (svnadmin dump → dsvn-admin load)

2. **Content-Addressable Storage Model**
   - `Blob`: File content → SHA-256 → ObjectId
   - `Tree`: Directory structure (BTreeMap<String, TreeEntry>)
   - `Commit`: Revision metadata with parent references
   - All objects immutable and deduplicated automatically

3. **Three-Tier Storage** (planned, MVP uses in-memory)
   - Hot: Fjall LSM-tree for recent/active data
   - Warm: Compressed packfiles for medium-age data
   - Cold: Archive storage for deep history

4. **Global Revision Numbers** (like SVN, not like Git)
   - Sequential revision numbers across all commits
   - Enables SVN protocol compatibility
   - Different from Git's DAG model

## Workspace Structure

This is a Cargo workspace with 4 crates:

- **dsvn-core**: Core object model (Blob, Tree, Commit), storage abstractions, repository implementations
  - `object.rs`: ObjectId, Blob, Tree, Commit, TreeEntry
  - `storage.rs`: HotStore, WarmStore, TieredStore traits
  - `repository.rs`: In-memory Repository (MVP)
  - `persistent.rs`: PersistentRepository using Fjall LSM-tree (in progress)

- **dsvn-webdav**: WebDAV/DeltaV protocol handlers
  - `handlers.rs`: All HTTP method implementations (PROPFIND, REPORT, MERGE, GET, PUT, etc.)
  - `lib.rs`: WebDavHandler main entry point
  - Uses lazy_static global REPOSITORY for MVP (will be refactored)

- **dsvn-server**: HTTP server binary
  - Uses Hyper + Tokio
  - CLI: `dsvn start --repo-root <path> --hot-path <path> --warm-path <path>`

- **dsvn-cli**: Administration tools
  - CLI: `dsvn-admin init <path>`, `dsvn-admin load --file <dump>`
  - `dump.rs`: SVN dump file parser
  - `load.rs`: Dump loader importing into Repository

## Common Development Commands

```bash
# Build all workspace members
cargo build --release

# Run tests (workspace-wide)
cargo test

# Run tests for specific crate
cargo test -p dsvn-core

# Run specific test
cargo test -p dsvn-core test_repository_create

# Build and run server
cargo run --release --bin dsvn start --repo-root ./data/repo --debug

# Initialize repository
cargo run --release --bin dsvn-admin init /tmp/test-repo

# Load SVN dump file
cargo run --release --bin dsvn-admin load --file repo.dump

# Check code (ensure it compiles)
cargo check

# Format code
cargo fmt

# Run linter
cargo clippy -- -D warnings
```

## Async Rust Best Practices (Lessons Learned)

### Lock Management in Async Code

**CRITICAL: Avoid holding locks across async/await points**

#### ❌ WRONG - Causes Deadlock
```rust
// DON'T: Hold write lock while calling async function that needs read lock
pub async fn commit(&self, author: String, message: String) -> Result<u64> {
    // ... create commit ...

    let mut meta = self.metadata.write().await;  // Take write lock
    meta.current_rev = new_rev;

    self.save_to_disk().await?;  // DEADLOCK! save_to_disk needs metadata.read()

    Ok(new_rev)
} // Lock released here (too late)
```

#### ✅ CORRECT - Release Lock Before Async Call
```rust
// DO: Release locks before calling functions that need them
pub async fn commit(&self, author: String, message: String) -> Result<u64> {
    // ... create commit ...

    // Scope the lock to release it before save_to_disk
    {
        let mut meta = self.metadata.write().await;
        meta.current_rev = new_rev;
    } // Lock released here

    self.save_to_disk().await?;  // Can now acquire metadata.read()

    Ok(new_rev)
}
```

### I/O Operations in Async Context

**Use `tokio::task::spawn_blocking` for synchronous I/O**

#### ❌ WRONG - Blocks Async Runtime
```rust
async fn save_to_disk(&self) -> Result<()> {
    let commits = self.commits.read().await;

    // Blocks the async thread!
    let file = File::create("commits.json")?;
    serde_json::to_writer(file, &commits)?;

    Ok(())
}
```

#### ✅ CORRECT - Offload to Blocking Thread
```rust
async fn save_to_disk(&self) -> Result<()> {
    // Clone data while holding lock
    let commits_map = {
        let commits = self.commits.read().await;
        commits.iter().map(|(k, v)| (*k, v.clone())).collect()
    };

    // Perform I/O outside of locks in blocking thread
    tokio::task::spawn_blocking(move || {
        let file = File::create("commits.json")?;
        serde_json::to_writer_pretty(&mut writer, &commits_map)?;
        Ok::<(), anyhow::Error>(())
    })
    .await?
}
```

### Debugging Test Hangs

When tests hang indefinitely:

1. **Run with timeout**: `timeout 10 cargo test`
2. **Run specific test**: `cargo test test_name`
3. **Add debug output**: Use `eprintln!` to trace execution
4. **Check for locks**: Look for lock acquisition patterns
5. **Use `--test-threads=1`**: Eliminate race conditions

```bash
# Identify stuck test
timeout 10 cargo test -p dsvn-core --lib

# Run with single thread
cargo test --lib -- --test-threads=1

# Add output to see where it hangs
cargo test test_name -- --nocapture
```

### Common Deadlock Patterns

1. **Write lock → Read lock deadlock**: Holding write lock while calling function that needs read lock
2. **Lock ordering**: Always acquire locks in consistent order
3. **Async-aware locks**: Use `tokio::sync::RwLock` not `std::sync::RwLock`
4. **Lock scope**: Minimize time holding locks, clone data if needed

### Testing Persistent Code

- **Use tempdir**: `tempfile::TempDir` for test isolation
- **Test restart scenarios**: Drop and reopen repository
- **Verify persistence**: Check data survives process restart
- **Test concurrent access**: Use `tokio::spawn` for parallel operations

## Object Model Details

### ObjectId
- 32-byte SHA-256 hash
- `from_data(data: &[u8]) -> ObjectId`
- `to_hex()` / `from_hex()` for string representation
- Used as primary key in all storage

### Blob
```rust
pub struct Blob {
    pub data: Vec<u8>,
    pub size: u64,
    pub executable: bool,
}
// ObjectId = SHA-256(serde serialization)
```

### Tree
```rust
pub struct Tree {
    pub entries: BTreeMap<String, TreeEntry>,  // Sorted for deterministic hashing
}

pub struct TreeEntry {
    pub name: String,
    pub id: ObjectId,     // Points to Blob or Tree
    pub kind: ObjectKind, // Blob or Tree
    pub mode: u32,        // Unix permissions
}
// ObjectId = SHA-256(serialized entries)
```

### Commit
```rust
pub struct Commit {
    pub tree_id: ObjectId,
    pub parents: Vec<ObjectId>,
    pub author: String,
    pub message: String,
    pub timestamp: i64,
    pub tz_offset: i32,
}
// ObjectId = SHA-256(serialization)
```

## Repository Operations

The `Repository` trait provides:

- `get_file(path, rev) -> Result<Bytes>`: Navigate tree to retrieve blob
- `add_file(path, content, executable) -> ObjectId`: Store blob and update root tree
- `commit(author, message, timestamp) -> u64`: Create new commit, increment global rev
- `log(start_rev, limit) -> Vec<Commit>`: Query commit history
- `list_dir(path, rev) -> Vec<String>`: List tree entries
- `initialize()`: Create initial commit (revision 0)

MVP implementation uses in-memory HashMap. Persistent implementation (`PersistentRepository`) uses Fjall LSM-tree.

## WebDAV Handler Key Points

- **Global repository**: `lazy_static::lazy_static! { static ref REPOSITORY: Arc<Repository> }`
- All handlers access this global (will be refactored to per-request context)
- `propfind_handler`: Returns directory listing as XML multistatus
- `report_handler`: Handles log-retrieve and update-report
- `merge_handler`: Creates commit via `REPOSITORY.commit()`
- `get_handler`: Retrieves file via `REPOSITORY.get_file()`

## SVN Dump Format

Located in `dsvn-cli/src/dump.rs` and `dump_format.rs`:

- Parses standard SVN dump format (version 2 or 3)
- Key headers: `Revision-number:`, `Node-path:`, `Node-kind:`, `Node-action:`
- Import via: `dsvn-admin load --file repo.dump`
- Export (planned): `dsvn-admin dump --repo <path> --output dumpfile`

## Testing Strategy

Use TDD methodology (`/tdd` command):

1. Write tests FIRST (they must fail)
2. Implement minimal code to pass
3. Refactor while keeping tests green
4. Ensure 80%+ coverage

Current tests in:
- `dsvn-core/src/repository.rs`: Unit tests for Repository operations
- `dsvn-cli/src/dump.rs`: Dump format parsing tests

## Development Patterns

### Error Handling
- Use `anyhow::Result<T>` for application errors
- Use `thiserror` for library error types
- Return descriptive errors with context

### Async/Await
- All repository operations are async
- Use `tokio::sync::RwLock` for concurrent access
- Use `Arc` for shared state across tasks

### Serialization
- `serde` for all object types
- `bincode` for compact binary serialization
- Manual `to_bytes()` / `deserialize()` methods on objects for control

## Current Implementation Status (2026-02-06)

### ✅ Completed Features

**WebDAV Protocol Layer (100% complete)**:
- ✅ PROPFIND - Directory listings with Depth header support
- ✅ REPORT - Log retrieval and update reports
- ✅ MERGE - Commit creation via `REPOSITORY.commit()`
- ✅ GET - File content retrieval
- ✅ PUT - File creation and updates with executable detection
- ✅ MKCOL - Directory/collection creation
- ✅ DELETE - File and directory deletion
- ✅ CHECKOUT - Working resource creation (WebDAV DeltaV)
- ✅ CHECKIN - Commit from working resource (WebDAV DeltaV)
- ✅ MKACTIVITY - SVN transaction management with UUID tracking
- ✅ PROPPATCH, LOCK, UNLOCK, COPY, MOVE - Stub implementations

**Core Object Model (100% complete)**:
- ✅ Blob, Tree, Commit, ObjectId implementations
- ✅ SHA-256 content addressing
- ✅ Deterministic tree serialization (BTreeMap)
- ✅ Unix permission support

**Repository Layer (85% complete)**:
- ✅ In-memory `Repository` with full CRUD operations
- ✅ Global revision numbers (SVN-compatible)
- ✅ Path-based queries with fast lookups
- ✅ Commit history tracking
- ✅ Transaction management infrastructure
- ✅ Thread-safe async operations (`Arc<RwLock<>>`)
- ✅ Persistent repository with JSON file storage (deadlock-free)
- ✅ Proper async lock management patterns
- 🔄 Fjall LSM-tree integration (planned)

**CLI Tools (100% complete)**:
- ✅ `dsvn` - Server management
- ✅ `dsvn-admin` - Repository admin (init, load)
- ✅ SVN dump format parser

### 🔄 In Progress

- **Persistent Storage**: `PersistentRepository` using Fjall LSM-tree
- **Integration Testing**: End-to-end tests with real SVN client

### ⏳ Known Limitations (MVP)

1. **Simple file-based persistence**: JSON format, not optimized for large repos
2. **Global repository singleton**: No multi-repo support yet
3. **Basic transaction handling**: No rollback or conflict resolution
4. **No authentication**: All operations open
5. **Tree integration incomplete**: `add_file()` stores blobs but doesn't update commit trees

## Performance Targets

Design goals:
- Checkout 1M files: < 30 seconds (vs 30 min for SVN)
- Checkout 10GB file: < 2 minutes (streaming)
- Global access: < 10ms latency (with edge proxies)
- Storage: 30-40 PB for 10B files (vs 60-70 PB without deduplication)

## Migration from SVN

```bash
# Export from SVN
svnadmin dump /path/to/svn/repo > repo.dump

# Import to DSvn
dsvn-admin load --file repo.dump

# Start DSvn server
dsvn start --repo-root /data/repos/my-project

# Update working copies to point to DSvn
svn switch --relocate http://svn-server/repo http://dsvn-server/svn
```

## Key Files for Understanding

- `dsvn-core/src/object.rs`: Object model and content addressing
- `dsvn-core/src/repository.rs`: In-memory repository implementation
- `dsvn-core/src/persistent.rs`: Fjall-based persistent repository
- `dsvn-webdav/src/handlers.rs`: Protocol implementation
- `ARCHITECTURE.md`: Detailed architecture documentation
- `ROADMAP.md`: Implementation roadmap and phases

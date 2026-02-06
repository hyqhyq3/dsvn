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

## Known Limitations (MVP)

1. **In-memory storage**: Data lost on restart (PersistentRepository in progress)
2. **Global repository singleton**: No multi-repo support yet
3. **Basic commit handling**: No full transaction support
4. **Limited WebDAV**: Basic operations only
5. **No authentication**: All operations open

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

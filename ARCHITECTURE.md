# DSvn Architecture

## Design Principles

### 1. Protocol Compatibility, Storage Independence

**What we DON'T do:**
- ❌ Read/write FSFS format (Subversion's filesystem format)
- ❌ Use Berkeley DB or other legacy storage
- ❌ Maintain binary compatibility with SVN repository files

**What we DO:**
- ✅ Speak the WebDAV/DeltaV protocol that SVN clients understand
- ✅ Use modern, high-performance storage engines
- ✅ Optimize for large-scale operations (billions of files, millions of commits)

### 2. Content-Addressable Storage

```
┌─────────────────────────────────────────────────────────────┐
│                       SVN Client                             │
│              (svn, TortoiseSVN, SVNKit, etc.)               │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            │ HTTP/WebDAV/DeltaV Protocol
                            │ (RFC 4918, RFC 3253, SVN extensions)
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                      DSvn Server                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              Protocol Layer                           │  │
│  │  • PROPFIND, PROPPATCH, REPORT                       │  │
│  │  • MERGE (commits), CHECKOUT/CHECKIN                 │  │
│  │  • MKACTIVITY, LOCK/UNLOCK                           │  │
│  └────────────────┬─────────────────────────────────────┘  │
│                   │                                          │
│  ┌────────────────▼─────────────────────────────────────┐  │
│  │           Repository Operations                      │  │
│  │  • Transaction management                            │  │
│  │  • Path-based queries                                │  │
│  │  • Revision history (log, blame, diff)              │  │
│  │  • Property management                              │  │
│  └────────────────┬─────────────────────────────────────┘  │
└───────────────────┼──────────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────────────────────────┐
│              Content-Addressable Storage                    │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                  Object Store                        │   │
│  │  ┌──────────────────────────────────────────────┐  │   │
│  │  │  Blob: file content → SHA-256 → ObjectId      │  │   │
│  │  │  Tree: directory structure → SHA-256          │  │   │
│  │  │  Commit: revision metadata → SHA-256          │  │   │
│  │  └──────────────────────────────────────────────┘  │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                 Tiered Storage                      │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐ │   │
│  │  │    Hot      │  │    Warm     │  │    Cold     │ │   │
│  │  │  (Fjall)    │  │ (Packfiles) │  │  (Archive)  │ │   │
│  │  │  • Latest   │  │  • Compressed │  │  • Deep     │ │   │
│  │  │  • Active   │  │  • Indexed  │  │    history  │ │   │
│  │  │  • Fast     │  │  • Medium   │  │  • Bulk     │ │   │
│  │  │    access   │  │    access   │  │    access   │ │   │
│  │  └─────────────┘  └─────────────┘  └─────────────┘ │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

## Key Architectural Differences from SVN

### Subversion (FSFS) Architecture

```
Repository Layout:
repo/
  ├── revprops/           # Revision properties (separate files)
  ├── revs/               # Revision data
  │   ├── 0               # Revision 0
  │   ├── 1               # Revision 1
  │   └── ...
  ├── transactions/       # Active commits
  └── current             # Latest revision number

Each revision file:
  - Changes from previous revision (delta)
  - Node revision IDs
  - Property changes

Performance issues:
  - Sequential revision numbers (global lock)
  - Delta chain decoding (O(n) for old revisions)
  - Separate files for revprops
  - No built-in deduplication
```

### DSvn Architecture

```
Repository Layout:
repo/
  ├── hot/                 # LSM-tree database
  │   ├── objects/        # Recent objects (indexed)
  │   ├── trees/          # Tree objects
  │   └── commits/        # Commit metadata
  ├── warm/               # Pack files
  │   ├── pack-*.pack     # Compressed objects
  │   └── pack-*.idx      # Pack indices
  ├── conf/               # Configuration
  └── metadata/           # Repository metadata

Object model:
  - Content-addressed (SHA-256)
  - Automatic deduplication
  - Skip-delta optimization
  - Parallel access

Performance benefits:
  - No global locks (shardable)
  - O(log n) delta decoding
  - Embedded properties
  - Built-in compression
```

## Data Structures

### Blob (File Content)

```rust
pub struct Blob {
    pub data: Vec<u8>,           // Raw content
    pub size: u64,               // Cached length
    pub executable: bool,        // Unix +x flag
}

// Object ID = SHA-256(data)
// Enables automatic deduplication
```

### Tree (Directory)

```rust
pub struct TreeEntry {
    pub name: String,
    pub id: ObjectId,            // SHA-256
    pub kind: ObjectKind,        // Blob or Tree
    pub mode: u32,               // Unix permissions
}

pub struct Tree {
    pub entries: BTreeMap<String, TreeEntry>,  // Sorted
}

// Object ID = SHA-256(serialized entries)
// Enables structural sharing
```

### Commit (Revision)

```rust
pub struct Commit {
    pub tree_id: ObjectId,       // Root tree
    pub parents: Vec<ObjectId>,  // Parent commits (usually 1)
    pub author: String,
    pub message: String,
    pub timestamp: i64,
    pub tz_offset: i32,
}

// Object ID = SHA-256(serialized commit)
// Enables efficient graph traversal
```

## Protocol Mapping

### SVN Client → DSvn Operations

| SVN Operation | WebDAV Method | DSvn Handler | Storage Operation |
|--------------|---------------|--------------|-------------------|
| `svn checkout` | PROPFIND + GET | Checkout | Read trees + blobs |
| `svn commit` | MKACTIVITY + MERGE | Commit | Write new commit |
| `svn update` | REPORT (update) | Update | Calculate delta |
| `svn log` | REPORT (log) | Log | Scan commits |
| `svn diff` | REPORT (diff) | Diff | Compare trees |
| `svn status` | PROPFIND | Status | Check metadata |
| `svn cat` | GET | GetFile | Read blob |
| `svn mkdir` | MKCOL | MakeDir | Create tree |
| `svn delete` | DELETE | Delete | Update commit |

## Storage Optimization Strategies

### 1. Skip-Delta Chains

Instead of storing deltas against immediate parent:

```
Linear Delta (SVN default):
  Rev 1000 → Rev 999 → Rev 998 → ... → Rev 1 (1000 steps)

Skip-Delta (DSvn optimization):
  Rev 1000 → Rev 998 → Rev 996 → ... (10 steps for 1000 revs)
```

Implementation:
```rust
// Choose skip-revision based on position
fn skip_delta_revision(rev: u64) -> u64 {
    if rev == 0 { return 0; }
    // Find largest power of 2 less than rev
    let highest_bit = 64 - rev.leading_zeros() - 1;
    rev - (1 << highest_bit)
}

// Example:
// rev 1000 → 998  (subtract 2)
// rev 998  → 996  (subtract 2)
// rev 996  → 992  (subtract 4)
// rev 992  → 984  (subtract 8)
```

### 2. Tiered Storage Lifecycle

```
┌────────────────────────────────────────────────────────┐
│  Object Lifecycle                                      │
│                                                        │
│  New Object → Hot Store (Fjall LSM-tree)             │
│       ↓                                                │
│  After 10K commits → Warm Store (Pack files)         │
│       ↓                                                │
│  After 1M commits → Cold Store (Archive)              │
│                                                        │
└────────────────────────────────────────────────────────┘
```

Promotion triggers:
- **Hot → Warm**: Periodic compaction (hourly)
- **Warm → Cold**: Age-based (older than 90 days)
- **Cold → Hot**: On-demand access (cache warming)

### 3. Sharding Strategy

```
Shard dimensions:
  1. Time: Revisions 0-999,999 in Shard 0
  2. Path: Hash of first path component
  3. Size: Large blobs (>10MB) in dedicated shard

Query routing:
  - Read queries: Broadcast to all shards, merge results
  - Write transactions: Single-shard (if possible)
  - Cross-shard copies: Optimized with batch operations
```

## Performance Targets

### Checkout Performance

```
Scenario: Checkout 100,000 files (1GB total)

Baseline (SVN/fsfs):  ~5 minutes
Target (DSvn):         < 30 seconds

Techniques:
  - Parallel blob retrieval (concurrency = CPU cores)
  - HTTP/2 multiplexing (single TCP connection)
  - Tree object caching (avoid re-fetching)
  - Delta compression for transfer
```

### Commit Performance

```
Scenario: Commit 10,000 modified files

Baseline (SVN/fsfs):  ~2 minutes
Target (DSvn):         < 15 seconds

Techniques:
  - Parallel delta computation
  - Batch object writes
  - Async commit processing
  - Optimized delta storage
```

### Log Retrieval

```
Scenario: Get last 10,000 commit log entries

Baseline (SVN/fsfs):  ~10 seconds
Target (DSvn):         < 100ms

Techniques:
  - Indexed commit metadata
  - Stored in hot store (LSM-tree)
  - Pagination support
  - No file system traversal
```

## Migration from SVN

Since DSvn doesn't read FSFS format, migration is required:

### Option 1: SVN Dump/Load

```bash
# Export from SVN
svnadmin dump /path/to/svn/repo > repo.dump

# Import to DSvn
dsvn-admin load /path/to/dsvn/repo < repo.dump
```

### Option 2: svnsync

```bash
# Create mirror
svnsync init file:///path/to/dsvn/repo http://old-svn/repo
svnsync sync file:///path/to/dsvn/repo
```

### Option 3: Direct Import (TODO)

```bash
# FSFS → DSvn converter
dsvn-admin import-fsfs /path/to/fsfs /path/to/dsvn
```

## Monitoring and Observability

### Metrics to Track

- Request latency (p50, p95, p99)
- Throughput (requests/sec, bytes/sec)
- Cache hit rates (hot/warm/cold)
- Storage usage per tier
- Active transactions
- Error rates by operation

### Health Checks

- Storage backend availability
- Database connection pool status
- Disk space alerts
- Memory usage
- Background task queue depth

## Security Considerations

- Authentication: LDAP, OAuth, SAML (via reverse proxy)
- Authorization: Path-based ACLs (TODO)
- Transport encryption: TLS required
- Secret storage: Integration with Vault/KMS (TODO)
- Audit logging: All modifications tracked

## Future Enhancements

### Phase 1 (MVP)
- ✅ Basic WebDAV protocol support
- ✅ Content-addressable storage
- ✅ HTTP server
- 🔄 Single repository
- 🔄 No authentication

### Phase 2 (Production)
- ⏳ Authentication/authorization
- ⏳ Multi-repository support
- ⏳ Backup/restore tools
- ⏳ Monitoring integration
- ⏳ Performance optimization

### Phase 3 (Scale)
- ⏳ Sharding
- ⏳ Geographic replication
- ⏳ Edge caching
- ⏳ CDN integration
- ⏳ Advanced compression

### Phase 4 (Features)
- ⏳ Branching improvements
- ⏳ Merge conflict resolution
- ⏳ External repository links
- ⏳ Git bridge (bi-directional)
- ⏳ Advanced search

# dsvn Sync Protocol Design

## Overview

dsvn supports two synchronization modes:
1. **Native dsvn sync protocol** — efficient binary protocol with compression, checksums, and resume
2. **SVNSync compatibility layer** — compatible with SVN's `svnsync` tool

## Architecture

```
┌─────────────┐    sync protocol    ┌─────────────┐
│   Source     │◀──────────────────▶│ Destination  │
│  (master)    │   delta transfer   │   (slave)    │
│              │                    │              │
│ objects/     │   RevisionData     │ objects/     │
│ commits/     │──────────────────▶│ commits/     │
│ tree_deltas/ │   (compressed)     │ tree_deltas/ │
│              │                    │ sync-state   │
│              │                    │ repl-log/    │
└─────────────┘                    └─────────────┘
```

## Native dsvn Sync Protocol (v1)

### Wire Protocol

Messages are serialized with bincode and optionally compressed with zstd:

```
Header: [DSVN magic (4)] [flags (1)] [uncompressed_len (4)] [compressed_len (4)]
Payload: [zstd compressed bincode data]
```

### Message Types

| Message | Direction | Purpose |
|---------|-----------|---------|
| `HandshakeRequest` | Client → Server | Protocol version, compression, capabilities |
| `HandshakeResponse` | Server → Client | UUID, HEAD rev, accepted/rejected |
| `SyncRequest` | Client → Server | Request revision range |
| `RevisionData` | Server → Client | Single revision with delta tree + objects |
| `SyncAck` | Client → Server | Acknowledge successful application |
| `SyncComplete` | Both | Summary statistics |
| `Ping/Pong` | Both | Keep-alive |
| `Error` | Both | Error reporting |

### Sync Flow

```
Client                              Server
  │                                    │
  ├──HandshakeRequest────────────────▶│
  │  (version=1, compression=zstd)     │
  │                                    │
  │◀──HandshakeResponse───────────────┤
  │  (uuid, head_rev, accepted)        │
  │                                    │
  ├──SyncRequest─────────────────────▶│
  │  (from_rev=N, to_rev=HEAD)         │
  │                                    │
  │◀──RevisionData (rev N+1)──────────┤
  │  (delta_tree + objects + hash)     │
  ├──SyncAck (rev N+1)──────────────▶│
  │                                    │
  │◀──RevisionData (rev N+2)──────────┤
  │  ...                               │
  ├──SyncAck (rev N+2)──────────────▶│
  │                                    │
  │◀──SyncComplete────────────────────┤
  │  (stats summary)                   │
  └────────────────────────────────────┘
```

### RevisionData Format

Each revision transfers:
- **Metadata**: author, message, timestamp
- **DeltaTree**: only changes relative to previous revision
- **Objects**: only new/modified blob data
- **Properties**: revision properties
- **ContentHash**: SHA-256 hash of all objects for verification

### Key Features

1. **Incremental sync**: Only delta trees (changes) are transferred, not full tree snapshots
2. **Content-addressed deduplication**: Objects already at destination are skipped
3. **Zstd compression**: 3x-10x compression ratio on typical content
4. **Hash verification**: SHA-256 content hash on every revision
5. **Checkpoint/resume**: State saved every 100 revisions for crash recovery
6. **Batch mode**: SQLite batch writes for efficient destination updates

## SVNSync Compatibility Layer

### Supported Properties

On revision 0 of the destination repository:
- `svn:sync-from-url` — Source repository URL
- `svn:sync-from-uuid` — Source repository UUID
- `svn:sync-last-merged-rev` — Last successfully synced revision
- `svn:sync-lock` — Prevents concurrent sync operations
- `svn:sync-currently-copying` — Crash recovery marker

### Hook Support

The `pre-revprop-change` hook is auto-installed (exits 0) to allow
svnsync to modify revision properties. This is required by the SVN
protocol for mirror configuration.

### Lock Protocol

```bash
# Acquire lock (fails if already locked)
svn:sync-lock = "hostname:pid:timestamp"

# Release lock
# Delete svn:sync-lock property
```

### Replication Log

Human-readable log output compatible with SVN's format:
```
---
Revision: 1
Author: alice
Date: 2026-01-01T00:00:00.000000Z
Log: add main.rs
Changes: 1
  A main.rs (file)
```

## Sync State

Stored in `<repo>/sync-state.json`:

```json
{
  "source_uuid": "uuid-of-source-repo",
  "source_url": "file:///path/to/source",
  "last_synced_rev": 42,
  "source_head_rev": 50,
  "last_sync_timestamp": 1707350000,
  "total_synced_revisions": 5,
  "sync_in_progress": false,
  "protocol_version": 1,
  "checkpoint_rev": null
}
```

## CLI Commands

### dsvnsync

```bash
# Initialize sync relationship
dsvnsync init --source /master --dest /slave [--svnsync-compat]

# Perform incremental sync
dsvnsync sync --source /master --dest /slave [--verify]

# Check sync status
dsvnsync info /slave

# Verify sync integrity
dsvnsync verify --source /master --dest /slave [-r REV]

# View replication log
dsvnsync repl-log /slave [--from N] [--to M]

# Clean up sync state
dsvnsync cleanup /slave [--remove-hooks]

# SVNSync-compatible commands
dsvnsync svnsync-init --source /master --dest /slave
dsvnsync svnsync-sync --source /master --dest /slave
```

### dsvn-admin (sync support)

```bash
# View replication log
dsvn-admin repl-log --repo /slave [--from N] [--to M]

# Set sync properties (via existing setrevprop)
dsvn-admin setrevprop /slave -r 0 -n svn:sync-from-url -v "file:///master"
```

## File Structure

```
dsvnsync-cli/
├── Cargo.toml
└── src/
    ├── main.rs              # CLI entry point
    ├── protocol.rs          # Sync engine (extract, apply, LocalSync)
    ├── replication_log.rs   # Replication log formatting
    ├── transfer.rs          # Delta transfer with dedup + compression
    └── compat.rs            # SVNSync compatibility layer

dsvn-core/
└── src/
    ├── sync.rs              # SyncState, ReplicationLog, ReplicationLogEntry
    └── replication.rs       # Wire protocol messages (SyncMessage, etc.)
```

## Performance

- **Local sync**: ~1000 revisions/sec (small files)
- **Compression**: ~3x ratio with zstd level 3
- **Incremental**: Only changed objects transferred (O(changes), not O(total files))
- **Batch writes**: SQLite WAL mode with deferred transactions

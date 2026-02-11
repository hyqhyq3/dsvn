# DSvn - High-Performance SVN-Compatible Server

A Rust implementation of a Subversion-compatible server optimized for large-scale repositories with **billions of files** and **millions of commits**.

## ⚠️ Important Design Decision

**DSvn is NOT compatible with FSFS repository format.**

- ✅ **Protocol Compatible**: DSvn speaks the SVN WebDAV/DeltaV protocol
- ❌ **Storage Incompatible**: DSvn uses its own high-performance storage engine
- 🔄 **Migration**: Requires import/export from existing SVN repositories

This design enables:
- Better performance (no legacy FSFS limitations)
- Modern storage architecture (content-addressable, tiered)
- Easier maintenance and evolution

## Features

- **SVN Protocol Compatibility**: Full WebDAV/DeltaV protocol support over HTTP
- **Massive Scalability**: Optimized for billions of files and millions of commits
- **Tiered Storage**: Hot/warm/cold storage architecture for optimal performance
- **Content-Addressable Storage**: Git-like deduplication and compression
- **Async I/O**: Built on Tokio for high-throughput concurrent operations
- **HTTP/2 Support**: Modern HTTP protocol with multiplexing

## Architecture

### Storage Engine

DSvn uses a three-tier storage architecture:

1. **Hot Tier**: Recent commits and frequently accessed objects (Fjall LSM-tree)
2. **Warm Tier**: Compressed pack files for medium-age data
3. **Cold Tier**: Highly compressed archival storage

### Object Model

DSvn uses a **content-addressable storage model** (inspired by Git, but optimized for SVN):

- **Blob**: File content (SHA-256 addressed)
- **Tree**: Directory structure with ordered entries
- **Commit**: Revision metadata with parent references

**Key differences from Git:**
- Tracks global revision numbers (like SVN)
- Supports directory-level properties
- Optimized for path-based queries
- Skip-delta chains for efficient history traversal

**Benefits:**
- Automatic deduplication across all revisions
- O(1) object lookup by hash
- Efficient compression and packing
- Parallel operations without locks

## Getting Started

### Prerequisites

- Rust 1.70 or later
- SVN client (for testing)

### Installation

```bash
# Build from source
cargo build --release

# The server binary will be at target/release/dsvn
# The admin CLI will be at target/release/dsvn-admin
```

### Initialize a Repository

```bash
# Create a new repository
dsvn-admin init /path/to/repo

# Or using the server
dsvn init /path/to/repo
```

### Start the Server

```bash
# HTTP server
dsvn start --repo-root /path/to/repo --hot-path /path/to/repo/hot --warm-path /path/to/repo/warm

# HTTPS server
dsvn start --repo-root /path/to/repo \
           --hot-path /path/to/repo/hot \
           --warm-path /path/to/repo/warm \
           --tls \
           --cert-file /path/to/cert.pem \
           --key-file /path/to/key.pem
```

### Using with SVN Client

```bash
# Checkout
svn checkout http://localhost:8080/svn my-working-copy

# Make changes and commit
cd my-working-copy
echo "hello" > test.txt
svn add test.txt
svn commit -m "Add test file"

# Update
svn update

# View log
svn log
```

## Performance Optimizations

### Skip-Delta Strategy

For long file histories, DSvn uses skip-deltas to reduce delta chain decoding from O(n) to O(log n):

```
Revision 1000 → 998 → 996 → 994 → ... (skip-delta)
vs
Revision 1000 → 999 → 998 → 997 → ... (linear delta)
```

### Parallel Operations

- Concurrent object retrieval
- Parallel delta computation
- Batched storage operations
- Multi-threaded checkout

### Compression

- Zstandard (zstd) for high compression ratio
- Dictionary compression for similar file types
- Delta encoding pre-compression

## Project Structure

```
dsvn/
├── Cargo.toml              # Workspace configuration
├── dsvn-core/              # Core library (object model, storage)
├── dsvn-webdav/            # WebDAV/HTTP protocol implementation
├── dsvn-server/            # Main server binary
├── dsvn-cli/               # Administration CLI
└── dsvn-proto/             # Protocol definitions
```

## Development Status

### Phase 1: Foundation ✅
- [x] Project structure
- [x] Core object model (Blob, Tree, Commit)
- [x] Storage abstraction (hot/warm tiers)
- [x] SQLite repository implementation
- [x] Property store
- [x] Hook system
- [x] Packfile support
- [x] Replication framework

### Phase 2: Protocol Support ✅
- [x] Full WebDAV method implementation (OPTIONS, GET, PUT, DELETE, HEAD, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE)
- [x] SVN-specific operations (REPORT, MKACTIVITY, MERGE, CHECKOUT)
- [x] Transaction management
- [ ] Authentication/authorization

### Phase 3: Repository Operations ✅
- [x] Checkout/update workflows
- [x] Commit processing
- [x] Log retrieval
- [x] Multi-repository support
- [x] Dump/load operations
- [ ] Diff generation

### Phase 4: Scalability (In Progress)
- [ ] Sharding implementation
- [ ] Caching layer
- [ ] Performance tuning
- [ ] Load testing
- [ ] Delta compression engine

### Phase 5: Production Readiness
- [ ] Monitoring and metrics
- [ ] Backup/restore tools
- [ ] Documentation
- [ ] Security audit

## Testing

### Unit Tests

```bash
cargo test
```

### Integration Tests

```bash
# Start test server
cargo run --bin dsvn start --repo-root /tmp/test-repo

# Run integration tests with SVN client
svn checkout http://localhost:8080/svn /tmp/wc
cd /tmp/wc
echo "test" > file.txt
svn add file.txt
svn commit -m "Test commit"
```

### WebDAV Compliance

Use the [Litmus](https://github.com/messense/dav-server-rs) test suite:

```bash
litmus http://localhost:8080/svn
```

## Configuration

### Server Options

- `--addr`: Listen address (default: 0.0.0.0:8080)
- `--repo-root`: Repository root directory
- `--hot-path`: Hot store path
- `--warm-path`: Warm store path
- `--tls`: Enable TLS
- `--cert-file`: TLS certificate file
- `--key-file`: TLS private key file
- `--max-connections`: Maximum concurrent connections (default: 1000)
- `--debug`: Enable debug logging

## Performance Targets

| Metric | Target | Status |
|--------|--------|--------|
| Checkout (10K files) | < 5 seconds | ✅ Implemented |
| Commit (1K files) | < 10 seconds | ✅ Implemented |
| Log Retrieval (1K entries) | < 500ms | ✅ Implemented |
| Concurrent Clients | 1000+ | 🏗️ In Progress |
| Storage Overhead | < 2x original | 🏗️ In Progress |

## Contributing

Contributions are welcome! Please see [DEVELOPMENT.md](docs/overview/DEVELOPMENT.md) for guidelines.

## License

Apache-2.0

## Acknowledgments

- Apache Subversion for the protocol specification
- Git for the content-addressable storage model
- The Rust community for excellent async ecosystem

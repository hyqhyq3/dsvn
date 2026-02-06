# DSvn Development Guide

## Development Workflow

### Prerequisites

- Rust 1.70+
- SVN client (for testing)
- Optional: Docker for containerized testing

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run with debug logging
RUST_LOG=debug cargo run --bin dsvn start --repo-root ./test-repo
```

### Running Tests

```bash
# Unit tests
cargo test

# Integration tests
cargo test --test integration

# Benchmark tests
cargo test --benches
```

## Code Organization

### dsvn-core

Core data structures and storage engine:

- `object.rs`: Blob, Tree, Commit objects
- `storage.rs`: Tiered storage (hot/warm/cold)
- `delta.rs`: Delta compression (TODO)

### dsvn-webdav

WebDAV/HTTP protocol implementation:

- `handlers.rs`: HTTP method handlers
- `xml.rs`: XML parsing utilities

### dsvn-server

Server binary:

- `main.rs`: Server entry point
- HTTP/HTTPS server setup
- Request routing

## Testing with SVN Client

### Start Test Server

```bash
# Terminal 1: Start server
cargo run --bin dsvn start --repo-root /tmp/test-repo --debug
```

### Test Checkout

```bash
# Terminal 2: Checkout repository
svn checkout http://localhost:8080/svn /tmp/wc

# Make changes
cd /tmp/wc
echo "hello world" > test.txt
svn add test.txt

# Commit
svn commit -m "Add test file"

# Update
svn update

# View log
svn log

# View status
svn status
```

## Protocol Implementation Notes

### WebDAV Methods

- **PROPFIND**: Retrieve properties/metadata
- **PROPPATCH**: Modify properties
- **REPORT**: SVN-specific queries (log, diff, etc.)
- **MERGE**: Commit changes
- **MKACTIVITY**: Create transaction
- **CHECKOUT/CHECKIN**: DeltaV versioning
- **LOCK/UNLOCK**: Resource locking

### SVN-Specific Reports

- `svn:log-retrieve`: Get commit history
- `update-report`: Get changes for update
- `get-file-revs`: Get file revision history
- `dated-rev-report`: Get revision by date

## Performance Profiling

### Flamegraphs

```bash
# Install flamegraph
cargo install flamegraph

# Generate flamegraph
cargo flamegraph --bin dsvn -- start --repo-root ./test-repo
```

### Benchmarking

```bash
# Run benchmarks
cargo bench

# With specific filter
cargo bench --checkout
```

## Debugging

### Logging

```bash
# Enable debug logging
RUST_LOG=debug cargo run --bin dsvn start --repo-root ./test-repo

# Enable trace for specific module
RUST_LOG=dsvn_webdav=trace cargo run --bin dsvn start --repo-root ./test-repo
```

### Debugging with GDB/LLDB

```bash
# Build with debug symbols
cargo build

# Run with debugger
rust-lldb target/debug/dsvn start --repo-root ./test-repo
```

## Architecture Decisions

### Why Content-Addressable Storage?

- Automatic deduplication across all revisions
- Efficient compression and packing
- Proven scalability (Git handles massive repos)
- Enables parallel operations

### Why Tiered Storage?

- Hot data in fast LSM-tree for low latency
- Warm data in compressed pack files
- Cold data in highly compressed archive
- Reduces storage costs while maintaining performance

### Why Async I/O?

- High concurrency with minimal threads
- Efficient resource utilization
- Non-blocking operations for slow clients
- HTTP/2 support with multiplexing

## Future Work

### Delta Compression

- Implement xdelta3 algorithm
- Skip-delta optimization
- Adaptive delta strategy selection

### Sharding

- Time-based partitioning
- Path-based hashing
- Cross-shard query routing

### Caching

- LRU cache for hot objects
- CDN/edge cache support
- Cache warming utilities

### Monitoring

- Prometheus metrics
- Structured logging
- Health check endpoints
- Performance dashboards

## Resources

### Protocol Specifications

- [RFC 4918 - HTTP Extensions for WebDAV](https://datatracker.ietf.org/doc/html/rfc4918)
- [RFC 3253 - Versioning Extensions to WebDAV](https://datatracker.ietf.org/doc/html/rfc3253)
- [Apache SVN WebDAV Protocol](https://svn.apache.org/repos/asf/subversion/trunk/notes/http-and-webdav/webdav-protocol)

### Rust Ecosystem

- [Tokio](https://tokio.rs/) - Async runtime
- [Hyper](https://hyper.rs/) - HTTP server
- [Fjall](https://fjall-rs.github.io/) - LSM-tree storage
- [Rustls](https://docs.rs/rustls) - TLS implementation

### Testing

- [Litmus](https://github.com/messense/dav-server-rs) - WebDAV compliance test
- [SVN Book](https://svnbook.red-bean.com/) - SVN documentation

# Integration Tests

This directory contains integration tests that may have special requirements.

## Persistent Repository Tests

The `persistent_repository_test.rs` file tests Fjall LSM database functionality.
**These tests must be run with single-threaded mode** to avoid file lock race conditions:

```bash
# Run persistent tests safely
cargo test --test persistent_repository_test -- --test-threads=1

# Run all integration tests safely
cargo test --tests -- --test-threads=1
```

## Why Single-Threaded?

Fjall uses file locks that can cause tests to hang when multiple tests access
different temporary databases concurrently. Running with `--test-threads=1` ensures
serial execution and prevents race conditions.

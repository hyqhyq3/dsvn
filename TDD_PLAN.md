# Test-Driven Development Plan for DSvn

## Current Status (Baseline)

### Test Coverage Analysis
- **dsvn-core**: ~60% coverage (object model, repository basics)
- **dsvn-webdav**: ~0% coverage (NO tests for handlers!)
- **dsvn-server**: 0% coverage
- **dsvn-admin-cli**: ~40% coverage (dump format parser)

### Critical Gaps
1. **WebDAV Handlers** (0% coverage)
   - PROPFIND, REPORT, MERGE, GET, PUT, MKCOL, DELETE, etc.
   - These are the core protocol handlers!

2. **XML Serialization** (minimal coverage)
   - WebDAV XML responses
   - SVN-specific XML formats

3. **Integration Tests** (0% coverage)
   - End-to-end workflows
   - quick-test.sh is the ONLY integration test!

## TDD Strategy

### Phase 1: Unit Tests for WebDAV Handlers (RED→GREEN→REFACTOR)

#### 1.1 PROPFIND Handler Tests
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{Request, Body};

    #[tokio::test]
    async fn test_propfind_root_directory() {
        // RED: Test doesn't exist yet
        // Should return 207 Multistatus with collection
    }

    #[tokio::test]
    async fn test_propfind_with_depth_zero() {
        // Should not list directory entries
    }

    #[tokio::test]
    async fn test_propfind_with_depth_one() {
        // Should list immediate children
    }

    #[tokio::test]
    async fn test_propfind_file() {
        // Should return file properties
    }
}
```

#### 1.2 REPORT Handler Tests
```rust
#[tokio::test]
async fn test_report_log_retrieve() {
    // Should return commit log as XML
}

#[tokio::test]
async fn test_report_update() {
    // Should return update report
}

#[tokio::test]
async fn test_report_unknown_type() {
    // Should handle gracefully
}
```

#### 1.3 MERGE Handler Tests
```rust
#[tokio::test]
async fn test_merge_creates_commit() {
    // Should create new commit
}

#[tokio::test]
async fn test_merge_with_transaction() {
    // Should use transaction from activity
}
```

#### 1.4 GET Handler Tests
```rust
#[tokio::test]
async fn test_get_file() {
    // Should return file content
}

#[tokio::test]
async fn test_get_directory() {
    // Should return error or redirect
}

#[tokio::test]
async fn test_get_nonexistent() {
    // Should return 404
}
```

#### 1.5 PUT Handler Tests
```rust
#[tokio::test]
async fn test_put_new_file() {
    // Should create file
}

#[tokio::test]
async fn test_put_executable_file() {
    // Should detect executable bit
}

#[tokio::test]
async fn test_put_overwrite() {
    // Should update existing file
}
```

#### 1.6 MKCOL Handler Tests
```rust
#[tokio::test]
async fn test_mkcol_directory() {
    // Should create directory
}

#[tokio::test]
async fn test_mkcol_exists() {
    // Should handle existing directory
}
```

#### 1.7 DELETE Handler Tests
```rust
#[tokio::test]
async fn test_delete_file() {
    // Should remove file
}

#[tokio::test]
async fn test_delete_directory() {
    // Should remove directory if empty
}

#[tokio::test]
async fn test_delete_nonexistent() {
    // Should return 404
}
```

### Phase 2: Repository Edge Cases

#### 2.1 Concurrent Access Tests
```rust
#[tokio::test]
async fn test_concurrent_commits() {
    // Multiple threads committing simultaneously
}

#[tokio::test]
async fn test_concurrent_reads() {
    // Multiple readers accessing same revision
}
```

#### 2.2 Error Handling Tests
```rust
#[tokio::test]
async fn test_get_file_invalid_path() {
    // Should return meaningful error
}

#[tokio::test]
async fn test_commit_empty_repository() {
    // Should handle gracefully
}
```

#### 2.3 Large File Tests
```rust
#[tokio::test]
async fn test_large_file_storage() {
    // Files > 1MB
}

#[tokio::test]
async fn test_binary_file() {
    // Binary content preservation
}
```

### Phase 3: Integration Tests

#### 3.1 Checkout Workflow
```rust
#[tokio::test]
async fn test_full_checkout_workflow() {
    // Initialize → Checkout → Verify files
}
```

#### 3.2 Commit Workflow
```rust
#[tokio::test]
async fn test_full_commit_workflow() {
    // Checkout → Add → Commit → Update → Verify
}
```

#### 3.3 Branch/Tag Operations
```rust
#[tokio::test]
async fn test_copy_directory() {
    // svn cp should work
}
```

## Implementation Order

### Week 1: WebDAV Handlers (Critical!)
1. Day 1-2: PROPFIND and REPORT handlers
2. Day 3-4: GET and PUT handlers
3. Day 5: MERGE and MKCOL handlers

### Week 2: Repository and Storage
1. Day 1-2: DELETE and remaining handlers
2. Day 3-4: Repository edge cases
3. Day 5: Concurrent access tests

### Week 3: Integration and Coverage
1. Day 1-3: Integration tests
2. Day 4-5: Coverage gap analysis and fill missing tests

## Coverage Targets

| Component | Current | Target | Priority |
|-----------|---------|--------|----------|
| dsvn-webdav | 0% | 80%+ | CRITICAL |
| dsvn-core | 60% | 85%+ | HIGH |
| dsvn-server | 0% | 70%+ | MEDIUM |
| dsvn-admin-cli | 40% | 75%+ | MEDIUM |

## Success Criteria

✅ All handlers have unit tests
✅ Coverage ≥ 80% for all components
✅ `cargo test --workspace` passes
✅ `make quick-test` passes
✅ No `#[allow(dead_code)]` without tests
✅ All error paths tested

## Next Steps

1. **RUN BASELINE**: `cargo llvm-cov --workspace --html`
2. **START TDD**: Write first failing test for PROPFIND handler
3. **ITERATE**: RED → GREEN → REFACTOR cycle
4. **VERIFY**: Check coverage after each handler

## Tools

```bash
# Run specific test
cargo test -p dsvn-webdav test_propfind_root_directory

# Run with coverage
cargo llvm-cov -p dsvn-webdav

# Generate HTML report
cargo llvm-cov --workspace --html --output-dir coverage

# View coverage
open coverage/index.html
```

## Notes

- Use `tokio::test` for async tests
- Mock repository state for handler tests
- Test error cases, not just happy paths
- Each test should be independent
- Use descriptive test names: `test_<function>_<scenario>`

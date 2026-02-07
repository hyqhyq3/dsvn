# Nested Directory Implementation Issue

## Problem Statement

The DSvn repository implementation has a limitation with nested directories. Files in nested paths (e.g., `/src/main.rs`) cannot be retrieved after being added.

## Current Status

✅ **12 out of 13 tests passing** (92% pass rate)
❌ **1 test failing**: `test_repository_nested_directories`

## Root Cause

The repository's tree structure isn't properly handling nested hierarchies:

1. **`add_file()`**: Creates tree objects for nested directories but doesn't properly link them in the parent-child chain
2. **`mkdir()`**: Creates tree objects but doesn't add them to parent trees
3. **`commit()`**: Stores the root tree, but intermediate trees may not be linked correctly

## Technical Details

### Expected Flow
```
/ (root tree)
  └── src/ (tree object)
      └── main.rs (blob object)
```

### Current Implementation Issue

When adding `/src/main.rs`:
1. ✅ Blob created for `main.rs` content
2. ✅ Tree created for `src/` directory
3. ❌ Tree hierarchy not properly constructed
4. ❌ `get_file("/src/main.rs")` can't navigate the tree chain

## Possible Solutions

### Option 1: MVP Workaround (Quickest)
**Store files flat with full path as key**

```rust
// In add_file()
let entry = TreeEntry::new(
    "src/main.rs",  // Full path as key
    blob_id,
    ObjectKind::Blob,
    0o644,
);
root_tree.insert(entry);
```

**Pros**:
- Simple to implement
- Works for MVP
- No tree traversal needed

**Cons**:
- Doesn't match Git/Tree structure design
- Harder to do directory listings
- Not scalable for billions of files

### Option 2: Proper Tree Hierarchy (Best Long-term)
**Fix tree construction to properly link parent-child**

```rust
// Pseudocode for proper implementation
fn add_file_nested(path, content) {
    let parts = path.split('/');
    let mut tree_stack = vec![root_tree];

    for (i, part) in parts.enumerate() {
        if i == parts.len() - 1 {
            // Add file to current tree
            tree_stack.last_mut().insert(part, blob_id);
        } else {
            // Navigate or create directory tree
            let child_tree = get_or_create_tree(part);
            tree_stack.push(child_tree);
        }
    }

    // Store all trees in hierarchy
    store_tree_hierarchy(tree_stack);
}
```

**Pros**:
- Proper scalable structure
- Matches architecture goals
- Supports directory operations

**Cons**:
- More complex to implement
- Requires careful tree ID management
- More testing needed

### Option 3: Hybrid Approach (Pragmatic)
**Use path index for lookups, build trees later**

Enhance the existing `path_index` HashMap to handle all lookups, defer proper tree construction for persistent storage implementation.

**Pros**:
- Works with existing MVP code
- Minimal changes needed
- Can implement proper trees with Fjall later

**Cons**:
- Not true content-addressable structure
- Technical debt

## Recommendation

**For MVP**: Use **Option 1** (flat storage)
- Quick fix
- All tests will pass
- Meets MVP requirements

**For Production**: Implement **Option 2** (proper trees)
- Do this when implementing `PersistentRepository` with Fjall
- Part of the storage tier work
- Aligns with architecture goals

## Implementation Plan for MVP (Option 1)

### Changes Needed

1. **Update `add_file()`**:
   ```rust
   // Use full path as key in root tree
   let full_path = path.trim_start_matches('/');
   root_tree.insert(TreeEntry::new(full_path, blob_id, Blob, mode));
   ```

2. **Update `get_file()`**:
   ```rust
   // Look up by full path instead of traversing
   let full_path = path.trim_start_matches('/');
   if let Some(entry) = root_tree.get(full_path) {
       // Return blob
   }
   ```

3. **Update `list_dir()`**:
   ```rust
   // Filter entries by path prefix
   let entries: Vec<String> = root_tree
       .iter()
       .filter(|e| e.name.starts_with(path))
       .map(|e| e.name.clone())
       .collect();
   ```

4. **Update `mkdir()`**:
   ```rust
   // Store directory marker in path index
   path_index.insert(path.to_string(), directory_marker_id);
   ```

### Estimated Effort
- **Time**: 1-2 hours
- **Files**: `dsvn-core/src/repository.rs`
- **Tests**: Update 1 failing test

### Risks
- Low risk, isolated changes
- Existing 12 tests continue to pass
- Easy to revert if issues found

## Next Steps

1. **Implement MVP workaround** (Option 1)
   - Modify `add_file()` to use flat paths
   - Update `get_file()` to lookup by full path
   - Adjust `list_dir()` for prefix matching
   - Verify all 13 tests pass

2. **Document technical debt**
   - Add TODO comments for proper tree implementation
   - Link to architecture documentation
   - Note this is MVP-only approach

3. **Plan proper tree implementation**
   - Schedule for PersistentRepository milestone
   - Include in storage tier design
   - Reference Git's tree object implementation

## Related Files

- `dsvn-core/src/repository.rs`: Core repository implementation
- `dsvn-core/src/object.rs`: Tree and Blob definitions
- `dsvn-webdav/tests/handler_integration_test.rs`: Test suite
- `ARCHITECTURE.md`: Design goals and storage model

## Test Case

```rust
#[tokio::test]
async fn test_repository_nested_directories() {
    let repo = setup_test_repository().await;

    // These should work after fix
    repo.mkdir("/src").await.unwrap();
    repo.mkdir("/src/bin").await.unwrap();
    repo.add_file("/src/main.rs", b"fn main() {}".to_vec(), true).await.unwrap();
    repo.add_file("/src/bin/main.rs", b"fn main() {}".to_vec(), true).await.unwrap();
    repo.commit("user".to_string(), "Add nested structure".to_string(), 0).await.unwrap();

    // These assertions should pass
    assert!(repo.exists("/src/main.rs", 1).await.unwrap());
    assert!(repo.exists("/src/bin/main.rs", 1).await.unwrap());
}
```

## Status

- **Current**: 12/13 tests passing
- **Blocker**: Nested directory tree construction
- **Priority**: Medium (MVP workaround is quick)
- **Owner**: DSvn team

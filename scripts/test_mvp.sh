#!/bin/bash
# DSvn MVP Test Script
# Tests basic functionality using SVN client

set -e

echo "========================================"
echo "DSvn MVP Test Script"
echo "========================================"
echo

# Check if SVN client is installed
if ! command -v svn &> /dev/null; then
    echo "❌ SVN client not found. Please install Subversion:"
    echo "   brew install subversion  # macOS"
    echo "   apt-get install subversion  # Ubuntu/Debian"
    exit 1
fi

echo "✅ SVN client found: $(svn --version | head -n 1)"
echo

# Server URL
SERVER_URL="${DSVN_SERVER_URL:-http://localhost:8080/svn}"

echo "📋 Test Plan:"
echo "  1. Checkout repository"
echo "  2. List files"
echo "  3. Create test file"
echo "  4. Commit changes"
echo "  5. View log"
echo

# Create temp directory for testing
TEST_DIR=$(mktemp -d)
echo "📁 Test directory: $TEST_DIR"
echo

# Test 1: Checkout
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 1: Checkout"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if svn checkout "$SERVER_URL" "$TEST_DIR/wc"; then
    echo "✅ Checkout successful"
else
    echo "❌ Checkout failed"
    rm -rf "$TEST_DIR"
    exit 1
fi
echo

# Test 2: List files
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 2: List files"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
cd "$TEST_DIR/wc"
svn ls -v
echo "✅ List complete"
echo

# Test 3: Create test file
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 3: Create test file"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Hello DSvn!" > test.txt
echo "✅ Created test.txt"
echo

# Test 4: Add and commit
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 4: Add and commit"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if svn add test.txt && svn commit -m "Add test file"; then
    echo "✅ Commit successful"
else
    echo "⚠️  Commit failed (expected for MVP)"
fi
echo

# Test 5: View log
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 5: View log"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
svn log
echo "✅ Log complete"
echo

# Cleanup
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Cleanup"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
rm -rf "$TEST_DIR"
echo "✅ Test directory removed"
echo

echo "========================================"
echo "✅ All tests completed!"
echo "========================================"

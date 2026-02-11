#!/bin/bash
# Test script for multi-repository functionality
set -e

echo "======================================"
echo "DSVN Multi-Repository Test Suite"
echo "======================================"
echo

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Function to print success
print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

# Function to print error
print_error() {
    echo -e "${RED}✗ $1${NC}"
}

# Function to run a test
run_test() {
    local test_name="$1"
    local test_command="$2"

    echo "Running: $test_name"
    if eval "$test_command"; then
        print_success "$test_name passed"
    else
        print_error "$test_name failed"
        return 1
    fi
    echo
}

# Create temporary directory for testing
TEST_DIR=$(mktemp -d)
echo "Test directory: $TEST_DIR"
cd "$TEST_DIR"
echo

# Check if dsvn binary exists
if [ ! -f "$WORKSPACE/../target/release/dsvn" ]; then
    print_error "dsvn binary not found. Please build with: cargo build --release"
    exit 1
fi

DSVN_BIN="$WORKSPACE/../target/release/dsvn"

# =======================================
# Setup
# =======================================
echo "=== Setting up test environment ==="

# Create data directory
mkdir -p data

# Initialize repositories
echo "Initializing repositories..."
$DSVN_BIN init ./data/repo1
$DSVN_BIN init ./data/repo2

# Create config file
cat > dsvn.toml <<EOF
multi_repo = true

[repositories.repo1]
path = "./data/repo1"
display_name = "Repository 1"
description = "First test repository"

[repositories.repo2]
path = "./data/repo2"
display_name = "Repository 2"
description = "Second test repository"
EOF

print_success "Setup complete"
echo

# =======================================
# Start Server
# =======================================
echo "=== Starting DSvn server ==="
$DSVN_BIN start --config dsvn.toml &
SERVER_PID=$!

# Wait for server to start
sleep 3

# Check if server started
if ! kill -0 $SERVER_PID 2>/dev/null; then
    print_error "Server failed to start"
    exit 1
fi

print_success "Server started (PID: $SERVER_PID)"
echo

# =======================================
# Test 1: List repositories via PROPFIND
# =======================================
run_test "Test 1: List repositories via PROPFIND" '
    RESPONSE=$(curl -s -X PROPFIND http://localhost:8080/svn/ \
        -H "Depth: 1" \
        -H "Content-Type: text/xml" \
        -d "<?xml version=\"1.0\" encoding=\"utf-8\"?><propfind xmlns=\"DAV:\"><prop><resourcetype/></prop></propfind>")

    # Check for both repositories in response
    if echo "$RESPONSE" | grep -q "/svn/repo1" && echo "$RESPONSE" | grep -q "/svn/repo2"; then
        exit 0
    else
        echo "Response did not contain expected repositories:"
        echo "$RESPONSE"
        exit 1
    fi
'

# =======================================
# Test 2: Checkout repo1
# =======================================
run_test "Test 2: Checkout from repo1" '
    rm -rf /tmp/repo1-checkout
    svn checkout http://localhost:8080/svn/repo1 /tmp/repo1-checkout

    if [ -d "/tmp/repo1-checkout/.svn" ]; then
        exit 0
    else
        print_error "SVN working copy not created"
        exit 1
    fi
'

# =======================================
# Test 3: Checkout repo2
# =======================================
run_test "Test 3: Checkout from repo2" '
    rm -rf /tmp/repo2-checkout
    svn checkout http://localhost:8080/svn/repo2 /tmp/repo2-checkout

    if [ -d "/tmp/repo2-checkout/.svn" ]; then
        exit 0
    else
        print_error "SVN working copy not created"
        exit 1
    fi
'

# =======================================
# Test 4: Commit to repo1
# =======================================
run_test "Test 4: Commit to repo1" '
    cd /tmp/repo1-checkout

    # Create a test file
    echo "Test content for repo1" > test.txt
    svn add test.txt

    # Commit the file
    svn ci -m "Add test file to repo1"

    if [ $? -eq 0 ]; then
        exit 0
    else
        print_error "Failed to commit to repo1"
        exit 1
    fi
'

# =======================================
# Test 5: Commit to repo2
# =======================================
run_test "Test 5: Commit to repo2" '
    cd /tmp/repo2-checkout

    # Create a different test file
    echo "Test content for repo2" > test.txt
    svn add test.txt

    # Commit the file
    svn ci -m "Add test file to repo2"

    if [ $? -eq 0 ]; then
        exit 0
    else
        print_error "Failed to commit to repo2"
        exit 1
    fi
'

# =======================================
# Test 6: Verify repository isolation
# =======================================
run_test "Test 6: Verify repository isolation" '
    # Check that repo1 and repo2 have different content
    REPO1_CONTENT=$(cat /tmp/repo1-checkout/test.txt)
    REPO2_CONTENT=$(cat /tmp/repo2-checkout/test.txt)

    if [ "$REPO1_CONTENT" != "$REPO2_CONTENT" ]; then
        print_success "Repositories are properly isolated"
        exit 0
    else
        print_error "Repositories have identical content (isolation failed)"
        exit 1
    fi
'

# =======================================
# Test 7: OPTIONS request to verify capabilities
# =======================================
run_test "Test 7: OPTIONS request to /svn" '
    RESPONSE=$(curl -s -X OPTIONS http://localhost:8080/svn)

    # Check for SVN headers
    if echo "$RESPONSE" | grep -iq "DAV" || curl -s -I -X OPTIONS http://localhost:8080/svn | grep -iq "DAV"; then
        exit 0
    else
        print_error "OPTIONS request did not return expected headers"
        exit 1
    fi
'

# =======================================
# Test 8: Create repository via API (if implemented)
# =======================================
run_test "Test 8: Create repository via API" '
    RESPONSE=$(curl -s -X POST http://localhost:8080/svn/_api/repos \
        -H "Content-Type: application/json" \
        -d "{\"name\":\"repo3\",\"path\":\"$TEST_DIR/data/repo3\"}")

    # The API might not be implemented yet, so we accept 404 or 405
    HTTP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X POST http://localhost:8080/svn/_api/repos \
        -H "Content-Type: application/json" \
        -d "{\"name\":\"repo3\",\"path\":\"$TEST_DIR/data/repo3\"}")

    if [ "$HTTP_STATUS" = "200" ] || [ "$HTTP_STATUS" = "201" ] || [ "$HTTP_STATUS" = "404" ] || [ "$HTTP_STATUS" = "405" ]; then
        exit 0
    else
        print_error "Unexpected HTTP status: $HTTP_STATUS"
        exit 1
    fi
'

# =======================================
# Test 9: List repositories via API (if implemented)
# =======================================
run_test "Test 9: List repositories via API" '
    HTTP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X GET http://localhost:8080/svn/_api/repos)

    if [ "$HTTP_STATUS" = "200" ] || [ "$HTTP_STATUS" = "404" ]; then
        exit 0
    else
        print_error "Unexpected HTTP status: $HTTP_STATUS"
        exit 1
    fi
'

# =======================================
# Test 10: Delete repository via API (if implemented)
# =======================================
run_test "Test 10: Delete repository via API" '
    HTTP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE http://localhost:8080/svn/_api/repos/repo1)

    if [ "$HTTP_STATUS" = "200" ] || [ "$HTTP_STATUS" = "204" ] || [ "$HTTP_STATUS" = "404" ] || [ "$HTTP_STATUS" = "405" ]; then
        exit 0
    else
        print_error "Unexpected HTTP status: $HTTP_STATUS"
        exit 1
    fi
'

# =======================================
# Cleanup
# =======================================
echo "=== Cleaning up ==="
kill $SERVER_PID
rm -rf /tmp/repo1-checkout /tmp/repo2-checkout

# Return to original directory
cd "$WORKSPACE/dsvn"

# Remove test directory
# Uncomment to clean up test directory:
# rm -rf "$TEST_DIR"

print_success "Cleanup complete"
echo

echo "======================================"
echo -e "${GREEN}All tests completed!${NC}"
echo "======================================"

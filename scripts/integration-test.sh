#!/bin/bash
# DSvn Integration Test - Real SVN Client Testing
#
# This script tests DSvn server with a real SVN client to verify
# checkout, add, commit, and update operations work correctly.

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
SVN_SERVER_PORT=8080
SVN_SERVER_URL="http://localhost:${SVN_SERVER_PORT}/svn"
TEST_REPO_DIR="./test-data/repo"
TEST_WORKING_COPY="./test-data/wc"
SERVER_PID_FILE="./test-data/server.pid"
SERVER_LOG_FILE="./test-data/server.log"

# Helper functions
log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

cleanup() {
    log_info "Cleaning up..."

    # Stop server if running
    if [ -f "$SERVER_PID_FILE" ]; then
        SERVER_PID=$(cat "$SERVER_PID_FILE")
        if kill -0 "$SERVER_PID" 2>/dev/null; then
            log_info "Stopping server (PID: $SERVER_PID)"
            kill "$SERVER_PID"
            wait "$SERVER_PID" 2>/dev/null || true
        fi
        rm -f "$SERVER_PID_FILE"
    fi

    # Don't remove test-data for debugging purposes
    # Uncomment to clean up after successful tests:
    # if [ -d "./test-data" ]; then
    #     log_info "Removing test data directory"
    #     rm -rf "./test-data"
    # fi
}

# Trap to ensure cleanup on exit
trap cleanup EXIT INT TERM

# Check prerequisites
check_prerequisites() {
    log_info "Checking prerequisites..."

    # Check if svn command is available
    if ! command -v svn &> /dev/null; then
        log_error "SVN client not found. Please install Subversion client:"
        log_error "  macOS: brew install subversion"
        log_error "  Ubuntu: sudo apt-get install subversion"
        exit 1
    fi

    # Check if cargo is available
    if ! command -v cargo &> /dev/null; then
        log_error "Cargo not found. Please install Rust toolchain."
        exit 1
    fi

    log_info "✓ SVN client found: $(svn --version --quiet | head -n 1)"
    log_info "✓ All prerequisites satisfied"
}

# Build DSvn components
build_dsvn() {
    log_info "Building DSvn server and admin tools..."

    # Create log directory
    mkdir -p "$(dirname "$SERVER_LOG_FILE")"

    # Build in release mode for performance
    if ! cargo build --release --bin dsvn --bin dsvn-admin 2>&1 | tee -a "$SERVER_LOG_FILE"; then
        log_error "Failed to build DSvn"
        exit 1
    fi

    log_info "✓ Build completed successfully"
}

# Initialize test repository
init_repository() {
    log_info "Initializing test repository..."

    # Create test directory
    mkdir -p "$(dirname "$TEST_REPO_DIR")"

    # Initialize repository using dsvn-admin
    if ./target/release/dsvn-admin init "$TEST_REPO_DIR" 2>&1 | tee -a "$SERVER_LOG_FILE"; then
        log_info "✓ Repository initialized at: $TEST_REPO_DIR"
    else
        log_error "Failed to initialize repository"
        cat "$SERVER_LOG_FILE"
        exit 1
    fi
}

# Start DSvn server
start_server() {
    log_info "Starting DSvn server on port $SVN_SERVER_PORT..."

    # Start server in background
    ./target/release/dsvn start \
        --addr "0.0.0.0:$SVN_SERVER_PORT" \
        --repo-root "$TEST_REPO_DIR" \
        2>&1 | tee -a "$SERVER_LOG_FILE" &

    SERVER_PID=$!
    echo $SERVER_PID > "$SERVER_PID_FILE"

    # Wait for server to be ready
    log_info "Waiting for server to start (PID: $SERVER_PID)..."
    for i in {1..30}; do
        if curl -s "http://localhost:$SVN_SERVER_PORT/" > /dev/null 2>&1; then
            log_info "✓ Server is ready"
            return 0
        fi
        sleep 1
    done

    log_error "Server failed to start within 30 seconds"
    log_error "Server log:"
    cat "$SERVER_LOG_FILE"
    exit 1
}

# Test 1: SVN Checkout
test_checkout() {
    log_info "Test 1: SVN Checkout"

    # Remove old working copy if exists
    rm -rf "$TEST_WORKING_COPY"

    # Perform checkout
    if svn checkout "$SVN_SERVER_URL" "$TEST_WORKING_COPY" 2>&1 | tee -a "$SERVER_LOG_FILE"; then
        log_info "✓ Checkout successful"
    else
        log_error "Checkout failed"
        cat "$SERVER_LOG_FILE"
        exit 1
    fi

    # Verify working copy was created
    if [ -d "$TEST_WORKING_COPY/.svn" ]; then
        log_info "✓ Working copy verified at: $TEST_WORKING_COPY"
    else
        log_error "Working copy .svn directory not found"
        exit 1
    fi
}

# Test 2: Add files
test_add_files() {
    log_info "Test 2: Add files to working copy"

    cd "$TEST_WORKING_COPY"

    # Create test files
    echo "Hello, DSvn!" > README.md
    echo "print('Hello from Python')" > test.py
    mkdir -p src
    echo "fn main() { println!(\"Hello from Rust\"); }" > src/main.rs

    log_info "Created test files: README.md, test.py, src/main.rs"

    # Add files to SVN
    if svn add README.md test.py src 2>&1 | tee -a "$SERVER_LOG_FILE"; then
        log_info "✓ Files added to SVN"
    else
        log_error "Failed to add files"
        cd - > /dev/null
        exit 1
    fi

    # Check status
    log_info "SVN Status:"
    svn status 2>&1 | tee -a "$SERVER_LOG_FILE"

    cd - > /dev/null
}

# Test 3: Commit changes
test_commit() {
    log_info "Test 3: Commit changes"

    cd "$TEST_WORKING_COPY"

    # Configure SVN username
    export SVN_USERNAME=testuser

    # Commit changes
    if svn commit -m "Initial commit: Add README, Python test, and Rust source" 2>&1 | tee -a "$SERVER_LOG_FILE"; then
        log_info "✓ Commit successful"
    else
        log_error "Commit failed"
        cat "$SERVER_LOG_FILE"
        cd - > /dev/null
        exit 1
    fi

    # Verify commit
    REVISION=$(svn info --show-item last-changed-revision 2>&1)
    log_info "✓ Working copy at revision: $REVISION"

    cd - > /dev/null
}

# Test 4: Verify files in repository
test_verify() {
    log_info "Test 4: Verify committed files"

    cd "$TEST_WORKING_COPY"

    # List files in repository
    log_info "Repository contents:"
    svn list -R "$SVN_SERVER_URL" 2>&1 | tee -a "$SERVER_LOG_FILE"

    # Verify file contents
    log_info "Verifying file contents..."
    if [ "$(cat README.md)" = "Hello, DSvn!" ]; then
        log_info "✓ README.md content verified"
    else
        log_error "README.md content mismatch"
        cd - > /dev/null
        exit 1
    fi

    if [ "$(cat test.py)" = "print('Hello from Python')" ]; then
        log_info "✓ test.py content verified"
    else
        log_error "test.py content mismatch"
        cd - > /dev/null
        exit 1
    fi

    cd - > /dev/null
}

# Test 5: Update from fresh checkout
test_update() {
    log_info "Test 5: Fresh checkout and update test"

    # Create second working copy
    WC2="./test-data/wc2"
    rm -rf "$WC2"

    if svn checkout "$SVN_SERVER_URL" "$WC2" 2>&1 | tee -a "$SERVER_LOG_FILE"; then
        log_info "✓ Second checkout successful"
    else
        log_error "Second checkout failed"
        exit 1
    fi

    # Verify files are present
    cd "$WC2"
    if [ -f README.md ] && [ -f test.py ] && [ -f src/main.rs ]; then
        log_info "✓ All files present in second working copy"
    else
        log_error "Some files missing in second working copy"
        ls -la
        cd - > /dev/null
        exit 1
    fi
    cd - > /dev/null
}

# Test 6: Modify and commit again
test_modify_commit() {
    log_info "Test 6: Modify existing file and commit"

    cd "$TEST_WORKING_COPY"

    # Modify README
    echo "" >> README.md
    echo "This is a modified line." >> README.md

    # Commit modification
    export SVN_USERNAME=testuser
    if svn commit -m "Update README with additional content" 2>&1 | tee -a "$SERVER_LOG_FILE"; then
        log_info "✓ Modification commit successful"
    else
        log_error "Modification commit failed"
        cd - > /dev/null
        exit 1
    fi

    cd - > /dev/null
}

# Test 7: Log retrieval
test_log() {
    log_info "Test 7: Retrieve commit log"

    if svn log "$SVN_SERVER_URL" --limit 10 2>&1 | tee -a "$SERVER_LOG_FILE"; then
        log_info "✓ Log retrieval successful"
    else
        log_error "Log retrieval failed"
        exit 1
    fi
}

# Main test execution
main() {
    echo "=========================================="
    echo "  DSvn Integration Test"
    echo "  Testing with real SVN client"
    echo "=========================================="
    echo ""

    check_prerequisites
    build_dsvn
    init_repository
    start_server
    test_checkout
    test_add_files
    test_commit
    test_verify
    test_update
    test_modify_commit
    test_log

    echo ""
    echo "=========================================="
    log_info "✓ All integration tests passed!"
    echo "=========================================="
}

# Run main function
main "$@"

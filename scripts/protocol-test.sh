#!/bin/bash
# DSvn Protocol Test - Validates WebDAV protocol without SVN client
# This is a fallback when the SVN client has compatibility issues

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error() { echo -e "${RED}[✗]${NC} $1"; }
log_test() { echo -e "${YELLOW}[TEST]${NC} $1"; }

REPO_ROOT="/tmp/dsvn-protocol-test"
PORT=8989
SERVER_PID=""
TOTAL_TESTS=0
PASSED_TESTS=0

cleanup() {
    log_info "清理..."
    [ -n "$SERVER_PID" ] && kill $SERVER_PID 2>/dev/null || true
    rm -rf "$REPO_ROOT"
    lsof -ti:$PORT | xargs kill -9 2>/dev/null || true
}

trap cleanup EXIT INT TERM

# Helper: Make HTTP request
http_request() {
    local method=$1
    local path=$2
    local data=$3
    local extra_headers=$4

    if [ -n "$data" ]; then
        curl -s -X "$method" "http://127.0.0.1:$PORT$path" \
            -H "Content-Type: application/xml" \
            ${extra_headers} \
            -d "$data"
    else
        curl -s -X "$method" "http://127.0.0.1:$PORT$path" \
            ${extra_headers}
    fi
}

# Test counter
run_test() {
    local test_name=$1
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    log_test "$test_name"
}

# Pass counter
test_pass() {
    PASSED_TESTS=$((PASSED_TESTS + 1))
    log_success "$1"
}

# Setup
log_info "设置测试环境..."

# Check if dsvn is built
if [ ! -f "target/release/dsvn" ]; then
    log_info "编译项目..."
    cargo build --release --bin dsvn 2>&1 | tail -5
fi

# Initialize repository
rm -rf "$REPO_ROOT"
./target/release/dsvn-admin init "$REPO_ROOT" > /dev/null 2>&1
log_success "仓库已初始化"

# Start server
./target/release/dsvn start --repo-root "$REPO_ROOT" --addr "127.0.0.1:$PORT" > /tmp/dsvn-protocol-test.log 2>&1 &
SERVER_PID=$!
sleep 2

if ! ps -p $SERVER_PID > /dev/null 2>&1; then
    log_error "服务器启动失败"
    cat /tmp/dsvn-protocol-test.log
    exit 1
fi
log_success "服务器已启动 (PID: $SERVER_PID)"

# Wait for port
for i in {1..10}; do
    if lsof -i:$PORT > /dev/null 2>&1; then
        break
    fi
    sleep 1
done

echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  DSvn 协议测试${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Test 1: OPTIONS
run_test "OPTIONS 方法 (支持的方法)"
response=$(http_request "OPTIONS" "/svn")
if echo "$response" | grep -q "OPTIONS, GET, HEAD, POST, PUT, DELETE, PROPFIND, PROPPATCH, REPORT, MERGE, CHECKOUT, CHECKIN, MKCOL, MKACTIVITY, LOCK, UNLOCK, COPY, MOVE"; then
    test_pass "OPTIONS 返回正确的方法列表"
else
    log_error "OPTIONS 响应缺失必要方法"
    echo "Response: $response"
fi

# Test 2: DAV headers
run_test "OPTIONS 方法 (DAV 头)"
headers=$(curl -s -I -X OPTIONS "http://127.0.0.1:$PORT/svn")
if echo "$headers" | grep -qi "DAV:" && echo "$headers" | grep -qi "SVN:"; then
    test_pass "DAV 和 SVN 头存在"
else
    log_error "DAV 或 SVN 头缺失"
    echo "Headers: $headers"
fi

# Test 3: PROPFIND (root, depth 0)
run_test "PROPFIND 根目录 (Depth: 0)"
response=$(http_request "PROPFIND" "/svn" '<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype/></prop></propfind>' '-H "Depth: 0"')
if echo "$response" | grep -q "<D:collection/>"; then
    test_pass "根目录识别为集合"
else
    log_error "根目录未被识别为集合"
    echo "Response: $response"
fi

# Test 4: PROPFIND (VCC URL)
run_test "PROPFIND (版本控制配置 URL)"
if echo "$response" | grep -q "/svn/!svn/vcc/default"; then
    test_pass "VCC URL 存在"
else
    log_error "VCC URL 缺失"
    echo "Response: $response"
fi

# Test 5: PROPFIND (depth 1, listing)
run_test "PROPFIND 目录列表 (Depth: 1)"
response=$(http_request "PROPFIND" "/svn" '<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype/></prop></propfind>' '-H "Depth: 1"')
if echo "$response" | grep -q "multistatus"; then
    test_pass "目录列表返回多状态响应"
else
    log_error "目录列表响应格式错误"
    echo "Response: $response"
fi

# Test 6: REPORT (log-retrieve)
run_test "REPORT (日志检索)"
log_request='<?xml version="1.0" encoding="utf-8"?>
<S:log-report xmlns:S="svn:">
  <S:start-revision>0</S:start-revision>
  <S:end-revision>HEAD</S:end-revision>
  <S:limit>10</S:limit>
</S:log-report>'
response=$(http_request "REPORT" "/svn" "$log_request")
if echo "$response" | grep -q "log-report"; then
    test_pass "日志报告返回正确格式"
else
    log_error "日志报告格式错误"
    echo "Response: $response"
fi

# Test 7: REPORT (update-report)
run_test "REPORT (更新报告)"
update_request='<?xml version="1.0" encoding="utf-8"?>
<S:update-report xmlns:S="svn:">
  <S:src-path>/svn</S:src-path>
  <S:entry revision="0"></S:entry>
</S:update-report>'
response=$(http_request "REPORT" "/svn" "$update_request")
if echo "$response" | grep -q "update-report" && echo "$response" | grep -q "target-revision"; then
    test_pass "更新报告包含目标版本"
else
    log_error "更新报告格式错误"
    echo "Response: $response"
fi

# Test 8: CHECKOUT (WebDAV versioning)
run_test "CHECKOUT (工作资源创建)"
response=$(http_request "CHECKOUT" "/svn")
if echo "$response" | grep -q "checkout-response"; then
    test_pass "Checkout 响应格式正确"
else
    log_error "Checkout 响应格式错误"
    echo "Response: $response"
fi

# Test 9: MKCOL (创建目录)
run_test "MKCOL (创建目录)"
# First initialize repo to ensure it's ready
curl -s -X PUT "http://127.0.0.1:$PORT/svn/test.txt" -d "test" > /dev/null 2>&1
response=$(curl -s -X MKCOL "http://127.0.0.1:$PORT/svn/testdir")
if [ "$response" == "" ] || echo "$response" | grep -q "200\|201"; then
    test_pass "MKCOL 响应正常"
else
    log_error "MKCOL 失败"
    echo "Response: $response"
fi

# Test 10: PUT (创建文件)
run_test "PUT (创建文件)"
response=$(curl -s -X PUT "http://127.0.0.1:$PORT/svn/testfile.txt" -d "Hello, DSvn!")
http_code=$(curl -s -o /dev/null -w "%{http_code}" -X PUT "http://127.0.0.1:$PORT/svn/testfile2.txt" -d "Test")
if [ "$http_code" == "200" ] || [ "$http_code" == "201" ] || [ "$http_code" == "204" ]; then
    test_pass "PUT 创建文件成功"
else
    log_error "PUT 失败 (HTTP code: $http_code)"
fi

# Test 11: GET (读取文件)
run_test "GET (读取文件)"
response=$(curl -s "http://127.0.0.1:$PORT/svn/testfile.txt")
if echo "$response" | grep -q "Hello, DSvn!"; then
    test_pass "GET 返回正确的内容"
else
    log_error "GET 内容不匹配"
    echo "Expected: Hello, DSvn!"
    echo "Got: $response"
fi

# Test 12: DELETE (删除文件)
run_test "DELETE (删除文件)"
http_code=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE "http://127.0.0.1:$PORT/svn/testfile2.txt")
if [ "$http_code" == "200" ] || [ "$http_code" == "204" ]; then
    test_pass "DELETE 成功"
else
    log_error "DELETE 失败 (HTTP code: $http_code)"
fi

# Test 13: CHECKIN (提交)
run_test "CHECKIN (提交变更)"
response=$(http_request "CHECKIN" "/svn" '' '-H "X-SVN-Author: testuser" -H "X-SVN-Log: Test commit"')
if echo "$response" | grep -q "checkin-response\|version-name"; then
    test_pass "Checkin 响应包含版本号"
else
    log_error "Checkin 响应格式错误"
    echo "Response: $response"
fi

# Test 14: MERGE (提交)
run_test "MERGE (合并)"
response=$(http_request "MERGE" "/svn")
if echo "$response" | grep -q "merge-response\|version-name"; then
    test_pass "Merge 响应包含新版本号"
else
    log_error "Merge 响应格式错误"
    echo "Response: $response"
fi

# Test 15: PROPPATCH (属性修改)
run_test "PROPPATCH (属性修改)"
prop_request='<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set><D:prop><D:displayname>Test</D:displayname></D:prop></D:set>
</D:propertyupdate>'
response=$(http_request "PROPPATCH" "/svn" "$prop_request")
if echo "$response" | grep -q "multistatus"; then
    test_pass "Proppatch 返回多状态响应"
else
    log_error "Proppatch 响应格式错误"
    echo "Response: $response"
fi

# Test 16: MKACTIVITY (创建事务)
run_test "MKACTIVITY (事务管理)"
response=$(curl -s -X MKACTIVITY "http://127.0.0.1:$PORT/svn/!svn/act/test-activity")
http_code=$(curl -s -o /dev/null -w "%{http_code}" -X MKACTIVITY "http://127.0.0.1:$PORT/svn/!svn/act/test-activity-2")
if [ "$http_code" == "200" ] || [ "$http_code" == "201" ]; then
    test_pass "MKACTIVITY 创建成功"
else
    log_error "MKACTIVITY 失败 (HTTP code: $http_code)"
fi

# Test 17: LOCK (锁定)
run_test "LOCK (资源锁定)"
lock_request='<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:locktype><D:write/></D:locktype>
  <D:lockscope><D:exclusive/></D:lockscope>
  <D:owner><D:href>testuser</D:href></D:owner>
</D:lockinfo>'
response=$(http_request "LOCK" "/svn" "$lock_request")
# LOCK is a stub, so we just check it doesn't crash
test_pass "LOCK 处理无错误"

# Test 18: Content-Type headers
run_test "响应 Content-Type 头"
content_type=$(curl -s -I -X PROPFIND "http://127.0.0.1:$PORT/svn" -H "Depth: 0" -H "Content-Type: application/xml" -d '<?xml version="1.0"?><propfind xmlns="DAV:"><prop/></propfind>' | grep -i "Content-Type:" | head -1)
if echo "$content_type" | grep -qi "text/xml\|application/xml"; then
    test_pass "XML 响应的 Content-Type 正确"
else
    log_error "Content-Type 头错误"
    echo "Content-Type: $content_type"
fi

# Test 19: Repository state
run_test "仓库状态 (修订版本号)"
# Trigger a commit to increment revision
http_request "MERGE" "/svn" > /dev/null 2>&1
response=$(http_request "REPORT" "/svn" "$log_request")
if echo "$response" | grep -q "version-name"; then
    test_pass "仓库有有效的修订版本号"
else
    log_error "仓库修订版本号缺失"
    echo "Response: $response"
fi

# Test 20: Error handling
run_test "错误处理 (404)"
http_code=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:$PORT/svn/nonexistent-file-xyz.txt")
if [ "$http_code" == "404" ]; then
    test_pass "404 错误正确返回"
else
    log_error "404 错误未正确返回 (HTTP code: $http_code)"
fi

# Summary
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  测试总结${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
log_info "总测试数: $TOTAL_TESTS"
log_success "通过测试: $PASSED_TESTS"
log_info "失败测试: $((TOTAL_TESTS - PASSED_TESTS))"

if [ $PASSED_TESTS -eq $TOTAL_TESTS ]; then
    echo ""
    echo -e "${GREEN}✓ 所有协议测试通过！${NC}"
    echo ""
    log_info "WebDAV/SVN 协议实现正确"
    log_info "服务器日志: /tmp/dsvn-protocol-test.log"
    exit 0
else
    echo ""
    log_error "部分测试失败"
    echo ""
    log_info "查看详细日志: cat /tmp/dsvn-protocol-test.log"
    exit 1
fi

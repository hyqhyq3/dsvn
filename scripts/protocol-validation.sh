#!/bin/bash
# DSvn Protocol Validation Test
# WebDAV 协议验证测试 - 使用 curl 而非 SVN 客户端

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
WC_DIR="/tmp/dsvn-protocol-wc"
SERVER_PID=""

cleanup() {
    log_info "清理..."
    [ -n "$SERVER_PID" ] && kill $SERVER_PID 2>/dev/null || true
    rm -rf "$REPO_ROOT" "$WC_DIR"
}
trap cleanup EXIT INT TERM

# 1. 编译
log_info "编译项目..."
cargo build --release --bin dsvn --bin dsvn-admin > /dev/null 2>&1
log_success "编译完成"

# 2. 初始化仓库
log_info "初始化仓库..."
rm -rf "$REPO_ROOT"
./target/release/dsvn-admin init "$REPO_ROOT" > /dev/null 2>&1
log_success "仓库已初始化"

# 3. 启动服务器
log_info "启动服务器 (端口 $PORT)..."
./target/release/dsvn start --repo-root "$REPO_ROOT" --addr "127.0.0.1:$PORT" > /tmp/dsvn-protocol.log 2>&1 &
SERVER_PID=$!
sleep 2

if ! ps -p $SERVER_PID > /dev/null 2>&1; then
    log_error "服务器启动失败"
    cat /tmp/dsvn-protocol.log
    exit 1
fi
log_success "服务器已启动"

# 4. 等待端口就绪
log_info "等待端口就绪..."
for i in {1..10}; do
    if curl -s http://127.0.0.1:$PORT/svn > /dev/null 2>&1; then
        break
    fi
    sleep 1
done
log_success "端口就绪"

# 测试函数
check_http_status() {
    local url="$1"
    local expected_status="$2"
    local method="${3:-GET}"
    local data="$4"

    local status_code
    if [ -n "$data" ]; then
        status_code=$(curl -s -o /dev/null -w "%{http_code}" -X "$method" -H "Content-Type: text/xml" -d "$data" "$url")
    else
        status_code=$(curl -s -o /dev/null -w "%{http_code}" -X "$method" "$url")
    fi

    if [ "$status_code" = "$expected_status" ]; then
        log_success "$method $url → $status_code"
        return 0
    else
        log_error "$method $url → $status_code (expected $expected_status)"
        return 1
    fi
}

check_response_contains() {
    local url="$1"
    local expected="$2"

    local response=$(curl -s "$url")
    if echo "$response" | grep -q "$expected"; then
        log_success "响应包含: $expected"
        return 0
    else
        log_error "响应不包含: $expected"
        log_error "实际响应: $response"
        return 1
    fi
}

# WebDAV 协议测试
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  WebDAV 协议验证测试${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Test 1: OPTIONS - 检查支持的 DAV 方法
log_test "1. OPTIONS - 检查支持的 DAV 方法"
if check_http_status "http://127.0.0.1:$PORT/svn" "200" "OPTIONS"; then
    if curl -s -X OPTIONS "http://127.0.0.1:$PORT/svn" -I | grep -q "DAV:"; then
        log_success "DAV header 存在"
    else
        log_error "DAV header 缺失"
    fi
fi

# Test 2: PROPFIND - 获取根目录属性
log_test "2. PROPFIND - 获取根目录属性"
check_http_status "http://127.0.0.1:$PORT/svn" "207" "PROPFIND"
check_response_contains "http://127.0.0.1:$PORT/svn" "multistatus"

# Test 3: PROPFIND with Depth: 1 - 列出根目录内容
log_test "3. PROPFIND (Depth: 1) - 列出根目录内容"
PROPFIND_DATA='<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype xmlns="DAV:"/></prop></propfind>'
check_http_status "http://127.0.0.1:$PORT/svn" "207" "PROPFIND" "$PROPFIND_DATA"

# Test 4: MKACTIVITY - 创建活动
log_test "4. MKACTIVITY - 创建 SVN 活动"
ACTIVITY_URL="http://127.0.0.1:$PORT/svn/!svn/act/test-activity"
check_http_status "$ACTIVITY_URL" "201" "MKACTIVITY"

# Test 5: CHECKOUT - 创建工作资源
log_test "5. CHECKOUT - 创建工作资源"
CHECKOUT_DATA='<?xml version="1.0" encoding="utf-8"?><checkout xmlns="DAV:"></checkout>'
VCC_URL="http://127.0.0.1:$PORT/svn/!svn/vcc/default"
check_http_status "$VCC_URL" "201" "CHECKOUT" "$CHECKOUT_DATA"

# Test 6: REPORT - 获取提交日志
log_test "6. REPORT - 获取提交日志 (log)"
REPORT_LOG='<?xml version="1.0" encoding="utf-8"?><S:log-report xmlns:S="svn:"><S:start-revision>0</S:start-revision><S:end-revision>HEAD</S:end-revision><S:limit>10</S:limit></S:log-report>'
check_http_status "http://127.0.0.1:$PORT/svn" "207" "REPORT" "$REPORT_LOG"

# Test 7: PUT - 上传文件
log_test "7. PUT - 上传文件"
FILE_URL="http://127.0.0.1:$PORT/svn/README.md"
if check_http_status "$FILE_URL" "201" "PUT"; then
    echo "Hello World" > /tmp/test-upload.txt
    curl -s -X PUT -T /tmp/test-upload.txt "$FILE_URL" > /dev/null
    log_success "文件上传成功"
fi

# Test 8: GET - 下载文件
log_test "8. GET - 下载文件"
if check_http_status "$FILE_URL" "200" "GET"; then
    if curl -s "$FILE_URL" | grep -q "Hello World"; then
        log_success "文件内容正确"
    else
        log_error "文件内容不匹配"
    fi
fi

# Test 9: MKCOL - 创建目录
log_test "9. MKCOL - 创建目录"
MKCOL_URL="http://127.0.0.1:$PORT/svn/src"
check_http_status "$MKCOL_URL" "201" "MKCOL"

# Test 10: PROPFIND - 验证目录创建
log_test "10. PROPFIND - 验证目录创建"
PROPFIND_DATA='<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype xmlns="DAV:"/></prop></propfind>'
if check_http_status "$MKCOL_URL" "207" "PROPFIND" "$PROPFIND_DATA"; then
    if curl -s -X PROPFIND "$MKCOL_URL" -H "Depth: 0" -H "Content-Type: text/xml" -d "$PROPFIND_DATA" | grep -q "collection"; then
        log_success "目录验证成功"
    else
        log_error "目录验证失败"
    fi
fi

# Test 11: MERGE - 创建提交
log_test "11. MERGE - 创建提交"
MERGE_DATA='<?xml version="1.0" encoding="utf-8"?><merge xmlns="DAV:"><source><href>/!svn/vcc/default</href></source></merge>'
check_http_status "http://127.0.0.1:$PORT/svn" "200" "MERGE" "$MERGE_DATA"

# Test 12: DELETE - 删除文件
log_test "12. DELETE - 删除文件"
check_http_status "$FILE_URL" "204" "DELETE"

# Test 13: REPORT - 获取更新报告
log_test "13. REPORT - 获取更新报告 (update)"
REPORT_UPDATE='<?xml version="1.0" encoding="utf-8"?><S:update-report xmlns:S="svn:" send-all="true"><S:src-path>/svn</S:src-path><S:entry revision="0" depth="infinity"/></S:update-report>'
check_http_status "http://127.0.0.1:$PORT/svn" "207" "REPORT" "$REPORT_UPDATE"

# Test 14: CHECKIN - 提交工作资源
log_test "14. CHECKIN - 提交工作资源"
check_http_status "http://127.0.0.1:$PORT/svn/!svn/vcc/default" "200" "CHECKIN"

# Test 15: PROPFIND - 验证提交后的修订版本
log_test "15. PROPFIND - 验证提交历史"
if curl -s -X PROPFIND "http://127.0.0.1:$PORT/svn" -H "Depth: 0" | grep -q "version-controlled-configuration"; then
    log_success "版本控制配置正确"
else
    log_error "版本控制配置缺失"
fi

# 测试总结
echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  协议验证测试完成${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
log_info "测试日志: /tmp/dsvn-protocol.log"
log_info "服务器仍在运行 (PID: $SERVER_PID)"
log_info "停止服务器: kill $SERVER_PID"

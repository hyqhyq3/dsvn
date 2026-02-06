#!/bin/bash
# DSvn Quick Test Script
# 快速测试脚本 - 用于日常开发验证

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error() { echo -e "${RED}[✗]${NC} $1"; }

REPO_ROOT="/tmp/dsvn-quick-test"
PORT=8989
WC_DIR="/tmp/dsvn-quick-wc"
SERVER_PID=""

cleanup() {
    log_info "清理..."
    [ -n "$SERVER_PID" ] && kill $SERVER_PID 2>/dev/null || true
    rm -rf "$REPO_ROOT" "$WC_DIR"
    lsof -ti:$PORT | xargs kill -9 2>/dev/null || true
}

trap cleanup EXIT INT TERM

# 1. 检查编译
log_info "检查编译..."
if [ ! -f "target/release/dsvn" ] || [ ! -f "target/release/dsvn-admin" ]; then
    log_info "编译项目..."
    cargo build --release --bin dsvn --bin dsvn-admin
fi
log_success "可执行文件就绪"

# 2. 初始化仓库
log_info "初始化仓库..."
rm -rf "$REPO_ROOT"
./target/release/dsvn-admin init "$REPO_ROOT"
log_success "仓库已初始化"

# 3. 启动服务器
log_info "启动服务器 (端口 $PORT)..."
./target/release/dsvn start --repo-root "$REPO_ROOT" --addr "127.0.0.1:$PORT" > /tmp/dsvn-quick.log 2>&1 &
SERVER_PID=$!
sleep 2

if ! ps -p $SERVER_PID > /dev/null 2>&1; then
    log_error "服务器启动失败"
    cat /tmp/dsvn-quick.log
    exit 1
fi
log_success "服务器已启动 (PID: $SERVER_PID)"

# 4. 等待端口就绪
for i in {1..10}; do
    if lsof -i:$PORT > /dev/null 2>&1; then
        break
    fi
    sleep 1
done

# 5. Checkout
log_info "Checkout..."
svn checkout "http://127.0.0.1:$PORT/svn" "$WC_DIR" --quiet
log_success "Checkout 完成"

# 6. 创建测试文件
log_info "创建测试文件..."
echo "# DSvn Quick Test" > "$WC_DIR/README.md"
echo 'fn main() { println!("Hello!"); }' > "$WC_DIR/main.rs"
chmod +x "$WC_DIR/test.sh" && echo "#!/bin/bash" > "$WC_DIR/test.sh"

# 7. 添加文件
log_info "添加文件..."
svn add "$WC_DIR"/* --quiet
log_success "文件已添加"

# 8. 提交
log_info "提交..."
svn commit "$WC_DIR" -m "Quick test commit" --username test --password test --quiet
log_success "提交成功"

# 9. 更新
log_info "更新..."
svn update "$WC_DIR" --quiet
log_success "更新成功"

# 10. 查看状态
echo ""
log_info "工作副本状态:"
svn status "$WC_DIR"

# 11. 查看日志
echo ""
log_info "提交日志:"
svn log "$WC_DIR" --limit 3

# 12. 验证文件
echo ""
log_info "验证文件内容:"
if [ -f "$WC_DIR/README.md" ] && grep -q "DSvn Quick Test" "$WC_DIR/README.md"; then
    log_success "README.md 内容正确"
else
    log_error "README.md 内容错误"
fi

# 13. 测试目录创建
log_info "测试创建目录..."
svn mkdir "$WC_DIR/src" -m "Create src dir" --quiet
log_success "目录创建成功"

# 14. 测试文件移动
log_info "测试移动文件..."
svn mv "$WC_DIR/main.rs" "$WC_DIR/src/main.rs"
svn commit "$WC_DIR" -m "Move main.rs" --quiet
log_success "文件移动成功"

# 15. 测试文件删除
log_info "测试删除文件..."
svn rm "$WC_DIR/test.sh"
svn commit "$WC_DIR" -m "Remove test.sh" --quiet
log_success "文件删除成功"

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  快速测试全部通过！${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
log_info "统计信息:"
local REV=$(svn info "http://127.0.0.1:$PORT/svn" 2>/dev/null | grep "Revision:" | awk '{print $2}')
echo "  - 当前修订版本: $REV"
echo "  - 仓库路径: $REPO_ROOT"
echo "  - 工作副本: $WC_DIR"
echo "  - 服务器日志: /tmp/dsvn-quick.log"
echo ""

# 保持服务器运行以便手动测试
log_info "服务器仍在运行，可以继续手动测试"
log_info "停止服务器: kill $SERVER_PID"
log_info "或运行: make stop-quick-test"

.PHONY: all build test clean quick-test acceptance-test help

# 默认目标
all: build

# 编译项目
build:
	@echo "🔨 编译DSvn项目..."
	cargo build --release --workspace

# 快速测试（用于日常开发）
quick-test: build
	@echo "🚀 运行快速测试..."
	@chmod +x scripts/quick-test.sh
	@./scripts/quick-test.sh

# 完整验收测试
acceptance-test: build
	@echo "🧪 运行完整验收测试..."
	@chmod +x scripts/acceptance-test.sh
	@./scripts/acceptance-test.sh

# 仅编译不测试
check:
	@echo "🔍 检查代码..."
	cargo check --workspace

# 运行单元测试
unit-test:
	@echo "🧪 运行单元测试..."
	cargo test --workspace

# 代码格式化
fmt:
	@echo "🎨 格式化代码..."
	cargo fmt --all

# 代码检查
clippy:
	@echo "🔍 代码检查..."
	cargo clippy --all-targets --all-features -- -D warnings

# 清理构建产物
clean:
	@echo "🧹 清理..."
	cargo clean
	rm -rf /tmp/dsvn-* /tmp/dsvn-*.log

# 停止所有测试服务器
stop-test:
	@echo "🛑 停止测试服务器..."
	@lsof -ti:8080 | xargs kill -9 2>/dev/null || true
	@lsof -ti:8989 | xargs kill -9 2>/dev/null || true
	@rm -rf /tmp/dsvn-*

# 初始化测试仓库
init-repo: build
	@echo "📦 初始化测试仓库..."
	@mkdir -p /tmp/dsvn-test-repo
	@./target/release/dsvn-admin init /tmp/dsvn-test-repo
	@echo "✓ 仓库已初始化: /tmp/dsvn-test-repo"

# 启动测试服务器
start-server: build init-repo
	@echo "🚀 启动DSvn服务器..."
	@./target/release/dsvn start --repo-root /tmp/dsvn-test-repo --addr "127.0.0.1:8080"

# 查看日志
logs:
	@echo "📋 服务器日志:"
	@tail -f /tmp/dsvn-server.log 2>/dev/null || echo "日志文件不存在"

# 显示帮助
help:
	@echo "DSvn 开发命令:"
	@echo ""
	@echo "编译:"
	@echo "  make build          - 编译项目"
	@echo "  make check          - 检查代码"
	@echo "  make fmt            - 格式化代码"
	@echo "  make clippy         - 代码检查"
	@echo ""
	@echo "测试:"
	@echo "  make quick-test     - 快速测试（日常开发）"
	@echo "  make acceptance-test - 完整验收测试"
	@echo "  make unit-test      - 单元测试"
	@echo ""
	@echo "服务器:"
	@echo "  make init-repo      - 初始化测试仓库"
	@echo "  make start-server   - 启动测试服务器"
	@echo "  make stop-test      - 停止所有测试服务器"
	@echo "  make logs           - 查看服务器日志"
	@echo ""
	@echo "清理:"
	@echo "  make clean          - 清理构建产物和测试数据"
	@echo ""
	@echo "示例:"
	@echo "  make quick-test     - 快速验证所有功能"
	@echo "  make acceptance-test - 运行完整的测试套件"

# 开发工作流
dev: fmt clippy build unit-test quick-test
	@echo "✨ 开发流程完成！"

# 生产构建检查
production-ready: fmt clippy build test
	@echo "🎉 生产就绪检查通过！"

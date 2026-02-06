#!/bin/bash
# 完整的 SVN → DSvn 迁移测试脚本

set -e

echo "========================================"
echo "SVN → DSvn 迁移测试"
echo "========================================"
echo

# 检查 SVN 客户端
if ! command -v svnadmin &> /dev/null; then
    echo "❌ svnadmin not found. Please install Subversion:"
    echo "   brew install subversion  # macOS"
    echo "   apt-get install subversion  # Ubuntu/Debian"
    exit 1
fi

if ! command -v svn &> /dev/null; then
    echo "❌ svn not found. Please install Subversion"
    exit 1
fi

echo "✅ Subversion found: $(svn --version | head -n 1)"
echo

# 临时目录
TMP_DIR=$(mktemp -d)
SVN_REPO="$TMP_DIR/svn-repo"
SVN_WC="$TMP_DIR/svn-wc"
DUMP_FILE="$TMP_DIR/repo.dump"

echo "📁 Temporary directory: $TMP_DIR"
echo

# ============================================
# Step 1: 创建 SVN 仓库
# ============================================
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 1: Creating SVN repository"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

svnadmin create "$SVN_REPO"
echo "✅ Repository created at $SVN_REPO"
echo

# ============================================
# Step 2: 创建标准目录结构
# ============================================
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 2: Creating directory structure"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

svn checkout "file://$SVN_REPO" "$SVN_WC" > /dev/null
cd "$SVN_WC"

mkdir -p trunk branches tags
svn add trunk branches tags > /dev/null
svn commit -m "Initialize repository structure" > /dev/null

echo "✅ Created trunk/branches/tags structure"
echo

# ============================================
# Step 3: 添加测试文件
# ============================================
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 3: Adding test files"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# 创建 README
cat > trunk/README.md << 'EOF'
# DSvn Test Repository

This is a test repository for DSvn migration.

## Features
- SVN protocol compatible
- High performance storage
- Easy migration from SVN
EOF

# 创建源代码文件
cat > trunk/main.py << 'EOF'
#!/usr/bin/env python3
"""Main application entry point."""

def greet(name):
    """Greet the user."""
    return f"Hello, {name}!"

if __name__ == "__main__":
    print(greet("DSvn"))
EOF

# 创建配置文件
cat > trunk/config.json << 'EOF'
{
  "name": "dsvn",
  "version": "0.1.0",
  "description": "High-performance SVN-compatible server"
}
EOF

svn add trunk/* > /dev/null
svn commit -m "Add initial project files" > /dev/null

echo "✅ Added test files:"
echo "   - README.md"
echo "   - main.py"
echo "   - config.json"
echo

# ============================================
# Step 4: 创建分支
# ============================================
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 4: Creating branch"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

svn copy trunk branches/feature-1 -m "Create feature branch" > /dev/null

echo "✅ Created branch: branches/feature-1"
echo

# ============================================
# Step 5: 在分支上修改
# ============================================
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 5: Modifying branch"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

cd branches/feature-1
cat >> main.py << 'EOF'

def farewell(name):
    """Say goodbye."""
    return f"Goodbye, {name}!"
EOF

svn commit -m "Add farewell function" > /dev/null

echo "✅ Modified branch"
echo

# ============================================
# Step 6: 创建标签
# ============================================
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 6: Creating tag"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

cd "$SVN_WC"
svn copy trunk tags/v0.1.0 -m "Tag version 0.1.0" > /dev/null

echo "✅ Created tag: tags/v0.1.0"
echo

# ============================================
# Step 7: 导出为 dump 文件
# ============================================
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 7: Dumping SVN repository"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

cd "$TMP_DIR"
svnadmin dump "$SVN_REPO" > "$DUMP_FILE"

DUMP_SIZE=$(du -h "$DUMP_FILE" | cut -f1)
echo "✅ Dump file created: $DUMP_FILE"
echo "   Size: $DUMP_SIZE"
echo

# 显示 dump 文件信息
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Dump file contents (first 50 lines):"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
head -n 50 "$DUMP_FILE"
echo

# ============================================
# Step 8: 导入到 DSvn
# ============================================
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 8: Loading into DSvn"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ -f "./target/release/dsvn-admin" ]; then
    ./target/release/dsvn-admin load --file "$DUMP_FILE"
else
    echo "⚠️  dsvn-admin not found. Building first..."
    cargo build --release --bin dsvn-admin
    ./target/release/dsvn-admin load --file "$DUMP_FILE"
fi

echo
echo "========================================"
echo "✅ Migration test completed!"
echo "========================================"
echo
echo "Summary:"
echo "  - Created SVN repository with:"
echo "    • trunk/branches/tags structure"
echo "    • 3 test files"
echo "    • 1 branch"
echo "    • 1 tag"
echo "    • 5 revisions"
echo "  - Dumped to: $DUMP_FILE"
echo "  - Imported to DSvn"
echo
echo "Files preserved:"
echo "  - SVN repo: $SVN_REPO"
echo "  - Dump file: $DUMP_FILE"
echo "  - Working copy: $SVN_WC"
echo
echo "To cleanup:"
echo "  rm -rf $TMP_DIR"

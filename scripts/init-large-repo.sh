#!/bin/bash
# init-large-repo.sh — 初始化一个 10GB / 100,000 commit 的 DSvn 测试仓库
#
# 用法: ./scripts/init-large-repo.sh [REPO_DIR] [SVN_URL]
# 默认: REPO_DIR=/tmp/dsvn-large-repo, SVN_URL=http://127.0.0.1:9000/svn
#
# 策略:
#   - 100,000 commits，每个 commit 添加/修改若干文件
#   - 目标总大小 ~10GB => 每 commit 平均 ~100KB 数据
#   - 使用多种文件类型模拟真实仓库（代码、文档、二进制）
#   - 批量操作，每 1000 commits 打印进度

set -euo pipefail

REPO_DIR="${1:-/tmp/dsvn-large-repo}"
SVN_URL="${2:-http://127.0.0.1:9000/svn}"
WC_DIR="/tmp/dsvn-large-wc"
TOTAL_COMMITS=100000
BATCH_SIZE=1000

# 每 commit 的平均目标大小（字节）~100KB => 100000 * 100KB = 10GB
AVG_COMMIT_SIZE=102400

echo "========================================="
echo "  DSvn 大型仓库初始化脚本"
echo "========================================="
echo "仓库目录: $REPO_DIR"
echo "SVN URL:  $SVN_URL"
echo "目标:     $TOTAL_COMMITS commits, ~10GB 数据"
echo "========================================="
echo ""

# 确认 dsvn 服务器已编译
DSVN_BIN="$(dirname "$0")/../target/release/dsvn"
if [ ! -f "$DSVN_BIN" ]; then
    echo "[INFO] 编译 dsvn..."
    cd "$(dirname "$0")/.."
    cargo build --release --bin dsvn 2>&1 | tail -3
fi

# 停止旧服务器，启动新的
echo "[INFO] 启动 DSvn 服务器..."
pkill -f "dsvn start" 2>/dev/null || true
sleep 1
rm -rf "$REPO_DIR"
"$DSVN_BIN" start --repo-root "$REPO_DIR" --addr 127.0.0.1:9000 > /tmp/dsvn-large.log 2>&1 &
DSVN_PID=$!
sleep 2

if ! kill -0 $DSVN_PID 2>/dev/null; then
    echo "[ERROR] DSvn 启动失败"
    cat /tmp/dsvn-large.log
    exit 1
fi
echo "[OK] DSvn 已启动 (PID: $DSVN_PID)"

# Checkout
echo "[INFO] Checkout..."
rm -rf "$WC_DIR"
svn checkout --non-interactive "$SVN_URL" "$WC_DIR" 2>&1 | tail -1
cd "$WC_DIR"

# 创建目录结构
echo "[INFO] 创建目录结构..."
for d in src lib docs assets tests config scripts data logs tmp; do
    mkdir -p "$d"
    for sub in core utils common models services handlers middleware; do
        mkdir -p "$d/$sub"
    done
done
svn add --force . 2>&1 | tail -1
svn commit -m "initial directory structure" --non-interactive --username bot 2>&1 | tail -1

# 生成随机数据的函数
gen_code_file() {
    # 生成模拟代码文件 (1KB - 50KB)
    local size=$((RANDOM % 50000 + 1000))
    head -c "$size" /dev/urandom | base64 | head -c "$size"
}

gen_text_file() {
    # 生成模拟文档文件 (500B - 10KB)
    local size=$((RANDOM % 10000 + 500))
    head -c "$size" /dev/urandom | base64 | fold -w 80 | head -c "$size"
}

gen_binary_file() {
    # 生成模拟二进制文件 (10KB - 500KB)
    local size=$((RANDOM % 500000 + 10000))
    head -c "$size" /dev/urandom
}

# 目录列表
DIRS=(src lib docs assets tests config scripts data logs tmp)
SUBDIRS=(core utils common models services handlers middleware)
EXTENSIONS_CODE=(.rs .py .js .ts .go .java .c .h .cpp .rb)
EXTENSIONS_TEXT=(.md .txt .toml .yaml .json .xml .csv)
EXTENSIONS_BIN=(.bin .dat .png .jpg .wasm .o .so)

echo "[INFO] 开始生成 $TOTAL_COMMITS commits..."
echo ""

START_TIME=$(date +%s)
TOTAL_BYTES=0

for ((i=1; i<=TOTAL_COMMITS; i++)); do
    # 每个 commit 操作 1-5 个文件
    NUM_FILES=$((RANDOM % 5 + 1))
    COMMIT_BYTES=0

    for ((f=0; f<NUM_FILES; f++)); do
        DIR="${DIRS[$((RANDOM % ${#DIRS[@]}))]}"
        SUBDIR="${SUBDIRS[$((RANDOM % ${#SUBDIRS[@]}))]}"
        
        # 选择文件类型 (70% 代码, 20% 文档, 10% 二进制)
        FILE_TYPE=$((RANDOM % 10))
        
        if [ $FILE_TYPE -lt 7 ]; then
            # 代码文件
            EXT="${EXTENSIONS_CODE[$((RANDOM % ${#EXTENSIONS_CODE[@]}))]}"
            FILENAME="$DIR/$SUBDIR/file_${i}_${f}${EXT}"
            gen_code_file > "$FILENAME"
        elif [ $FILE_TYPE -lt 9 ]; then
            # 文档文件
            EXT="${EXTENSIONS_TEXT[$((RANDOM % ${#EXTENSIONS_TEXT[@]}))]}"
            FILENAME="$DIR/$SUBDIR/doc_${i}_${f}${EXT}"
            gen_text_file > "$FILENAME"
        else
            # 二进制文件
            EXT="${EXTENSIONS_BIN[$((RANDOM % ${#EXTENSIONS_BIN[@]}))]}"
            FILENAME="$DIR/$SUBDIR/blob_${i}_${f}${EXT}"
            gen_binary_file > "$FILENAME"
        fi
        
        FILE_SIZE=$(wc -c < "$FILENAME")
        COMMIT_BYTES=$((COMMIT_BYTES + FILE_SIZE))
        
        # 随机决定是新建还是修改 (前 10% 全新建，之后 50% 概率修改已有文件)
        if [ $i -gt $((TOTAL_COMMITS / 10)) ] && [ $((RANDOM % 2)) -eq 0 ]; then
            # 修改模式：覆盖已有文件
            EXISTING_NUM=$((RANDOM % i + 1))
            EXISTING_FILE="$DIR/$SUBDIR/file_${EXISTING_NUM}_0${EXT}"
            if [ -f "$EXISTING_FILE" ]; then
                gen_code_file > "$EXISTING_FILE"
                FILENAME="$EXISTING_FILE"
            fi
        fi
        
        # svn add（忽略已添加的文件）
        svn add --force "$FILENAME" 2>/dev/null || true
    done
    
    TOTAL_BYTES=$((TOTAL_BYTES + COMMIT_BYTES))
    
    # 提交
    svn commit -m "commit #${i}: ${NUM_FILES} files, ${COMMIT_BYTES} bytes" --non-interactive --username "user$((RANDOM % 20))" 2>&1 > /dev/null
    
    # 进度报告
    if [ $((i % BATCH_SIZE)) -eq 0 ]; then
        NOW=$(date +%s)
        ELAPSED=$((NOW - START_TIME))
        RATE=$((i * 3600 / (ELAPSED + 1)))
        TOTAL_MB=$((TOTAL_BYTES / 1048576))
        ETA=$(( (TOTAL_COMMITS - i) * ELAPSED / (i + 1) ))
        printf "[%s] %d/%d commits (%d%%) | %d MB | %d commits/hr | ETA: %dm%ds\n" \
            "$(date +%H:%M:%S)" "$i" "$TOTAL_COMMITS" "$((i * 100 / TOTAL_COMMITS))" \
            "$TOTAL_MB" "$RATE" "$((ETA / 60))" "$((ETA % 60))"
    fi
done

END_TIME=$(date +%s)
TOTAL_ELAPSED=$((END_TIME - START_TIME))
TOTAL_MB=$((TOTAL_BYTES / 1048576))

echo ""
echo "========================================="
echo "  完成!"
echo "========================================="
echo "Commits:  $TOTAL_COMMITS"
echo "数据量:   ${TOTAL_MB} MB"
echo "耗时:     ${TOTAL_ELAPSED}s ($((TOTAL_ELAPSED / 60))m)"
echo "速率:     $((TOTAL_COMMITS * 3600 / (TOTAL_ELAPSED + 1))) commits/hr"
echo "仓库目录: $REPO_DIR"
echo "仓库大小: $(du -sh "$REPO_DIR" | cut -f1)"
echo "========================================="

# 清理工作副本
echo "[INFO] 清理工作副本..."
rm -rf "$WC_DIR"
echo "[OK] Done."

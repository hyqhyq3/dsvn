#!/bin/bash
# init-large-repo-fast.sh — 快速初始化 10GB / 100,000 commit 的 DSvn 仓库
# 直接通过 HTTP API 提交，不走 svn 客户端
#
# 用法: ./scripts/init-large-repo-fast.sh [REPO_DIR] [ADDR]

set -euo pipefail

REPO_DIR="${1:-/tmp/dsvn-large-repo}"
ADDR="${2:-127.0.0.1:9000}"
SVN_URL="http://$ADDR/svn"
TOTAL_COMMITS=100000
BATCH=1000

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DSVN_BIN="$PROJECT_DIR/target/release/dsvn"

echo "========================================="
echo "  DSvn 大型仓库初始化"
echo "========================================="
echo "仓库: $REPO_DIR | 服务器: $ADDR"
echo "目标: $TOTAL_COMMITS commits, ~10GB"
echo "========================================="

# 编译
if [ ! -f "$DSVN_BIN" ]; then
    echo "[INFO] 编译..."
    cd "$PROJECT_DIR" && cargo build --release --bin dsvn 2>&1 | tail -2
fi

# 启动服务器
pkill -f "dsvn start" 2>/dev/null || true; sleep 1
rm -rf "$REPO_DIR"
"$DSVN_BIN" start --repo-root "$REPO_DIR" --addr "$ADDR" > /tmp/dsvn-large.log 2>&1 &
DSVN_PID=$!; sleep 2

if ! kill -0 $DSVN_PID 2>/dev/null; then
    echo "[ERROR] 启动失败"; cat /tmp/dsvn-large.log; exit 1
fi
echo "[OK] PID: $DSVN_PID"

# 预生成随机数据缓冲区 (避免每次调用 /dev/urandom)
RAND_FILE="/tmp/dsvn-rand-buf"
echo "[INFO] 预生成 2MB 随机数据缓冲区..."
head -c 2097152 /dev/urandom | base64 > "$RAND_FILE"
RAND_SIZE=$(wc -c < "$RAND_FILE")

# 目录和扩展名
DIRS=(src lib docs assets tests config scripts data logs modules packages build vendor cache api)
SUBDIRS=(core utils common models services handlers middleware auth crypto net io fmt db)
EXTS=(.rs .py .js .ts .go .java .c .h .cpp .md .txt .toml .yaml .json)

START_TIME=$(date +%s)
TOTAL_BYTES=0

echo "[INFO] 开始..."

for ((i=1; i<=TOTAL_COMMITS; i++)); do
    # POST: 创建事务
    TXN=$(curl -s -X POST "$SVN_URL/!svn/me" \
        -H "Content-Type: application/vnd.svn-skel" \
        -d "(create-txn)" -D - 2>/dev/null | grep -i "svn-txn-name" | awk '{print $2}' | tr -d '\r')
    
    # 每 commit 1-5 个文件
    NUM_FILES=$((RANDOM % 5 + 1))
    
    for ((f=0; f<NUM_FILES; f++)); do
        DIR="${DIRS[$((RANDOM % ${#DIRS[@]}))]}"
        SUB="${SUBDIRS[$((RANDOM % ${#SUBDIRS[@]}))]}"
        EXT="${EXTS[$((RANDOM % ${#EXTS[@]}))]}"
        FPATH="$DIR/$SUB/file_${i}_${f}${EXT}"
        
        # 文件大小: 目标平均 ~100KB/commit => ~33KB/file
        # 70% 小(1-50KB), 20% 中(50-200KB), 10% 大(200KB-2MB)
        R=$((RANDOM % 10))
        if [ $R -lt 7 ]; then
            SIZE=$((RANDOM % 50000 + 1000))
        elif [ $R -lt 9 ]; then
            SIZE=$((RANDOM % 150000 + 50000))
        else
            SIZE=$((RANDOM % 1800000 + 200000))
        fi
        
        # 从缓冲区截取随机偏移
        OFFSET=$((RANDOM % (RAND_SIZE - SIZE - 1) ))
        
        # PUT 文件
        dd if="$RAND_FILE" bs=1 skip="$OFFSET" count="$SIZE" 2>/dev/null | \
            curl -s -X PUT "$SVN_URL/!svn/txr/$TXN/$FPATH" \
            -H "Content-Type: application/octet-stream" --data-binary @- > /dev/null
        
        TOTAL_BYTES=$((TOTAL_BYTES + SIZE))
    done
    
    # MERGE: 提交
    curl -s -X MERGE "$SVN_URL" \
        -H "X-SVN-Log-Message: commit #${i}: ${NUM_FILES} files" \
        -H "X-SVN-User: user$((RANDOM % 20))" \
        -d "<?xml version=\"1.0\"?><D:merge xmlns:D=\"DAV:\"><D:source><D:href>/svn/!svn/txn/$TXN</D:href></D:source></D:merge>" \
        > /dev/null
    
    # 进度
    if [ $((i % BATCH)) -eq 0 ]; then
        NOW=$(date +%s); ELAPSED=$((NOW - START_TIME))
        RATE=$((i * 3600 / (ELAPSED + 1)))
        MB=$((TOTAL_BYTES / 1048576))
        ETA=$(( (TOTAL_COMMITS - i) * ELAPSED / (i + 1) ))
        printf "[%s] %d/%d (%d%%) | %d MB | %d c/hr | ETA %dm%ds\n" \
            "$(date +%H:%M:%S)" "$i" "$TOTAL_COMMITS" "$((i * 100 / TOTAL_COMMITS))" \
            "$MB" "$RATE" "$((ETA / 60))" "$((ETA % 60))"
    fi
done

END_TIME=$(date +%s); ELAPSED=$((END_TIME - START_TIME))
MB=$((TOTAL_BYTES / 1048576))

echo ""
echo "========================================="
echo "  完成!"
echo "  Commits: $TOTAL_COMMITS"
echo "  数据: ${MB} MB (~$((MB / 1024)) GB)"
echo "  耗时: ${ELAPSED}s ($((ELAPSED / 60))m)"
echo "  速率: $((TOTAL_COMMITS * 3600 / (ELAPSED + 1))) c/hr"
echo "  仓库: $(du -sh "$REPO_DIR" 2>/dev/null | cut -f1)"
echo "========================================="

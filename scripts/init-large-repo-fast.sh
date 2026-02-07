#!/bin/bash
# init-large-repo-fast.sh — 快速初始化 10GB / 100,000 commit 的 DSvn 仓库
set -euo pipefail

REPO_DIR="${1:-/tmp/dsvn-large-repo}"
ADDR="${2:-127.0.0.1:9000}"
SVN_URL="http://$ADDR/svn"
TOTAL_COMMITS=100000
BATCH=500

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DSVN_BIN="$PROJECT_DIR/target/release/dsvn"

echo "========================================="
echo "  DSvn 大型仓库初始化"
echo "  $TOTAL_COMMITS commits, ~10GB"
echo "========================================="

if [ ! -f "$DSVN_BIN" ]; then
    cd "$PROJECT_DIR" && cargo build --release --bin dsvn 2>&1 | tail -2
fi

pkill -f "dsvn start" 2>/dev/null || true; sleep 1
rm -rf "$REPO_DIR"
"$DSVN_BIN" start --repo-root "$REPO_DIR" --addr "$ADDR" > /tmp/dsvn-large.log 2>&1 &
DSVN_PID=$!; sleep 2
kill -0 $DSVN_PID 2>/dev/null || { echo "[ERROR] 启动失败"; exit 1; }
echo "[OK] PID: $DSVN_PID"

# 预生成数据文件 (不同大小)
# 目标: 平均 ~100KB/commit, ~3 files/commit => ~33KB/file avg
echo "[INFO] 预生成数据..."
TD="/tmp/dsvn-gendata"; rm -rf "$TD" && mkdir -p "$TD"
head -c 5120    /dev/urandom | base64 > "$TD/tiny.dat"    # 5KB
head -c 20480   /dev/urandom | base64 > "$TD/small.dat"   # 20KB
head -c 51200   /dev/urandom | base64 > "$TD/medium.dat"  # 50KB
head -c 204800  /dev/urandom | base64 > "$TD/large.dat"   # 200KB

DIRS=(src lib docs assets tests config scripts data logs modules packages build vendor cache api)
SUBDIRS=(core utils common models services handlers middleware auth crypto net io fmt db)
EXTS=(.rs .py .js .ts .go .java .c .h .cpp .md .txt .toml .yaml .json)

START_TIME=$(date +%s)
TOTAL_BYTES=0

echo "[INFO] 开始..."

for ((i=1; i<=TOTAL_COMMITS; i++)); do
    TXN=$(curl -s -X POST "$SVN_URL/!svn/me" \
        -H "Content-Type: application/vnd.svn-skel" \
        -d "(create-txn)" -D - 2>/dev/null | grep -i "svn-txn-name" | awk '{print $2}' | tr -d '\r')
    
    NUM_FILES=$((RANDOM % 4 + 1))  # 1-4 files
    
    for ((f=0; f<NUM_FILES; f++)); do
        DIR="${DIRS[$((RANDOM % ${#DIRS[@]}))]}"
        SUB="${SUBDIRS[$((RANDOM % ${#SUBDIRS[@]}))]}"
        EXT="${EXTS[$((RANDOM % ${#EXTS[@]}))]}"
        FPATH="$DIR/$SUB/file_${i}_${f}${EXT}"
        
        # 70% tiny(5KB), 20% small(20KB), 8% medium(50KB), 2% large(200KB)
        # avg per file: 0.7*5 + 0.2*20 + 0.08*50 + 0.02*200 = 3.5+4+4+4 = 15.5KB
        # avg per commit (2.5 files): ~39KB => 100K commits => ~3.9GB
        # 不够10G，调大一点
        # 40% tiny(5KB), 30% small(20KB), 20% medium(50KB), 10% large(200KB)  
        # avg/file: 0.4*5 + 0.3*20 + 0.2*50 + 0.1*200 = 2+6+10+20 = 38KB
        # avg/commit (2.5 files): ~95KB => 100K => ~9.5GB ✓
        R=$((RANDOM % 10))
        if [ $R -lt 4 ]; then
            DATA="$TD/tiny.dat"; SIZE=5120
        elif [ $R -lt 7 ]; then
            DATA="$TD/small.dat"; SIZE=20480
        elif [ $R -lt 9 ]; then
            DATA="$TD/medium.dat"; SIZE=51200
        else
            DATA="$TD/large.dat"; SIZE=204800
        fi
        
        curl -s -X PUT "$SVN_URL/!svn/txr/$TXN/$FPATH" \
            -H "Content-Type: application/octet-stream" \
            --data-binary "@$DATA" > /dev/null
        
        TOTAL_BYTES=$((TOTAL_BYTES + SIZE))
    done
    
    curl -s -X MERGE "$SVN_URL" \
        -H "X-SVN-Log-Message: commit #${i}: ${NUM_FILES} files" \
        -H "X-SVN-User: user$((RANDOM % 20))" \
        -d "<?xml version=\"1.0\"?><D:merge xmlns:D=\"DAV:\"><D:source><D:href>/svn/!svn/txn/$TXN</D:href></D:source></D:merge>" \
        > /dev/null
    
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
echo "  完成! $TOTAL_COMMITS commits | ${MB}MB (~$((MB/1024))GB) | ${ELAPSED}s ($((ELAPSED/60))m)"
echo "  仓库: $(du -sh "$REPO_DIR" 2>/dev/null | cut -f1)"
echo "========================================="

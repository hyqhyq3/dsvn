#!/usr/bin/env python3
"""
gen-large-dump.py — 生成 10GB / 100,000 commits 的 SVN dump 文件

用法: python3 gen-large-dump.py [输出文件] [commits数] [目标大小GB]
默认: /tmp/large-repo.dump 100000 10
"""

import sys
import random
import string
import time
from datetime import datetime, timezone
from pathlib import Path

# 可配置参数
OUTPUT_FILE = sys.argv[1] if len(sys.argv) > 1 else "/tmp/large-repo.dump"
TOTAL_COMMITS = int(sys.argv[2]) if len(sys.argv) > 2 else 100000
TARGET_GB = float(sys.argv[3]) if len(sys.argv) > 3 else 10
TARGET_BYTES = TARGET_GB * 1024 * 1024 * 1024

# 目录结构
DIRS = ["src", "lib", "docs", "assets", "tests", "config", "scripts", "data", "modules", "packages"]
SUBDIRS = ["core", "utils", "common", "models", "services", "handlers", "middleware", "auth", "db"]
EXTS = [".rs", ".py", ".js", ".ts", ".go", ".java", ".c", ".h", ".cpp", ".md", ".txt", ".toml", ".yaml", ".json"]

AUTHORS = ["alice", "bob", "charlie", "diana", "eve", "frank", "grace", "henry"]
MESSAGES = [
    "fix: resolve null pointer exception",
    "feat: add user authentication",
    "refactor: simplify error handling",
    "docs: update API documentation",
    "test: add unit tests for utils",
    "chore: update dependencies",
    "perf: optimize database queries",
    "style: fix code formatting",
    "feat: implement caching layer",
    "fix: handle edge case in parser",
    "refactor: extract common logic",
    "docs: add usage examples",
    "test: improve coverage",
    "chore: clean up debug logs",
    "perf: reduce memory allocations",
]

def random_string(length):
    return ''.join(random.choices(string.ascii_letters + string.digits, k=length))

def random_content(size):
    """生成指定大小的随机内容（base64-like，可打印字符）"""
    # 使用随机但确定性的内容
    lines = []
    remaining = size
    while remaining > 0:
        line_len = min(80, remaining)
        lines.append(random_string(line_len))
        remaining -= line_len + 1  # +1 for newline
    return '\n'.join(lines[:size//80 + 1])[:size]

def format_props(props):
    """格式化 SVN 属性块"""
    lines = []
    for key, value in props.items():
        lines.append(f"K {len(key)}")
        lines.append(key)
        lines.append(f"V {len(value)}")
        lines.append(value)
    lines.append("PROPS-END")
    content = '\n'.join(lines)
    return f"Prop-content-length: {len(content)}\nContent-length: {len(content)}\n\n{content}"

def generate_revision(f, rev, file_counter, total_bytes_ref):
    """生成一个 revision 块"""
    author = random.choice(AUTHORS)
    message = random.choice(MESSAGES)
    date = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")
    
    # Revision 属性
    props = {
        "svn:log": f"{message} (rev {rev})",
        "svn:author": author,
        "svn:date": date,
    }
    
    # 计算这个 revision 的节点
    num_files = random.randint(1, 5)
    nodes = []
    
    for _ in range(num_files):
        dir_name = random.choice(DIRS)
        sub_name = random.choice(SUBDIRS)
        ext = random.choice(EXTS)
        
        # 文件大小分布：70%小(1-10KB), 20%中(10-50KB), 8%大(50-200KB), 2%超大(200KB-1MB)
        r = random.random()
        if r < 0.70:
            size = random.randint(1000, 10000)
        elif r < 0.90:
            size = random.randint(10000, 50000)
        elif r < 0.98:
            size = random.randint(50000, 200000)
        else:
            size = random.randint(200000, 1000000)
        
        # 决定是添加新文件还是修改已有文件
        if file_counter < 100 or random.random() < 0.3:
            # 添加新文件
            path = f"{dir_name}/{sub_name}/file_{file_counter}{ext}"
            action = "add"
            file_counter += 1
        else:
            # 修改已有文件
            existing = random.randint(0, file_counter - 1)
            path = f"{dir_name}/{sub_name}/file_{existing}{ext}"
            action = "change"
        
        content = random_content(size)
        nodes.append((path, action, content))
        total_bytes_ref[0] += size
    
    # 先写 revision header
    f.write(f"\nRevision-number: {rev}\n")
    f.write(format_props(props))
    f.write("\n")
    
    # 写 nodes
    for path, action, content in nodes:
        f.write(f"\nNode-path: {path}\n")
        f.write("Node-kind: file\n")
        f.write(f"Node-action: {action}\n")
        
        node_props = "PROPS-END"
        prop_len = len(node_props)
        content_len = len(content)
        total_len = prop_len + 1 + content_len  # +1 for empty line between props and content
        
        f.write(f"Prop-content-length: {prop_len}\n")
        f.write(f"Text-content-length: {content_len}\n")
        f.write(f"Content-length: {total_len}\n\n")
        f.write(node_props)
        f.write("\n")
        f.write(content)
        f.write("\n")
    
    return file_counter

def main():
    print("=" * 60)
    print("  SVN Dump 文件生成器")
    print("=" * 60)
    print(f"输出: {OUTPUT_FILE}")
    print(f"目标: {TOTAL_COMMITS} commits, ~{TARGET_GB}GB")
    print("=" * 60)
    print()
    
    output_path = Path(OUTPUT_FILE)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    
    start_time = time.time()
    total_bytes = [0]  # 用 list 做引用传递
    file_counter = 0
    
    with open(OUTPUT_FILE, 'w', buffering=1024*1024) as f:  # 1MB buffer
        # 写入文件头
        f.write("SVN-fs-dump-format-version: 2\n\n")
        f.write(f"UUID: {random_string(8)}-{random_string(4)}-{random_string(4)}-{random_string(4)}-{random_string(12)}\n")
        
        # Revision 0 (空 revision)
        f.write("\nRevision-number: 0\n")
        f.write("Prop-content-length: 10\nContent-length: 10\n\nPROPS-END\n")
        
        # 生成 revisions
        batch_size = 1000
        for rev in range(1, TOTAL_COMMITS + 1):
            file_counter = generate_revision(f, rev, file_counter, total_bytes)
            
            # 进度报告
            if rev % batch_size == 0:
                elapsed = time.time() - start_time
                mb = total_bytes[0] / (1024 * 1024)
                rate = rev / elapsed if elapsed > 0 else 0
                eta = (TOTAL_COMMITS - rev) / rate if rate > 0 else 0
                print(f"[{rev}/{TOTAL_COMMITS}] {mb:.1f}MB | {rate:.0f} rev/s | ETA {eta/60:.1f}m")
                
                # 如果已经达到目标大小，提前结束
                if total_bytes[0] >= TARGET_BYTES:
                    print(f"\n达到目标大小 {TARGET_GB}GB，提前结束")
                    break
    
    elapsed = time.time() - start_time
    final_size = output_path.stat().st_size / (1024 * 1024 * 1024)
    
    print()
    print("=" * 60)
    print("  完成!")
    print("=" * 60)
    print(f"文件: {OUTPUT_FILE}")
    print(f"大小: {final_size:.2f} GB")
    print(f"Revisions: {rev}")
    print(f"耗时: {elapsed:.1f}s ({elapsed/60:.1f}m)")
    print(f"速度: {rev/elapsed:.0f} rev/s")
    print("=" * 60)

if __name__ == "__main__":
    main()

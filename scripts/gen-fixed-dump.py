#!/usr/bin/env python3
"""生成官方 SVN 兼容的 dump 文件"""
import sys, random, string, time
from datetime import datetime, timezone

# 生成标准 UUID 格式
import uuid

OUTPUT = sys.argv[1] if len(sys.argv) > 1 else "/tmp/fixed.dump"
COMMITS = int(sys.argv[2]) if len(sys.argv) > 2 else 1000

def fmt_props(props):
    lines = []
    for k, v in props.items():
        lines.append(f"K {len(k)}\n{k}\nV {len(v)}\n{v}")
    lines.append("PROPS-END")
    content = '\n'.join(lines)
    return f"Prop-content-length: {len(content)}\nContent-length: {len(content)}\n\n{content}\n"

# 生成 dump
with open(OUTPUT, 'w') as f:
    f.write("SVN-fs-dump-format-version: 2\n\n")
    f.write(f"UUID: {uuid.uuid4()}\n")
    
    # Rev 0
    f.write("\nRevision-number: 0\n")
    f.write("Prop-content-length: 10\nContent-length: 10\n\nPROPS-END\n")
    
    file_counter = 0
    for rev in range(1, COMMITS + 1):
        props = {
            "svn:log": f"rev {rev}",
            "svn:author": "user",
            "svn:date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ"),
        }
        
        f.write(f"\nRevision-number: {rev}\n")
        f.write(fmt_props(props))
        
        # 添加 1 个文件
        path = f"file_{file_counter}.txt"
        content = "hello world\n"
        file_counter += 1
        
        f.write(f"\nNode-path: {path}\nNode-kind: file\nNode-action: add\n")
        f.write(f"Prop-content-length: 10\nText-content-length: {len(content)}\n")
        f.write(f"Content-length: {len(content) + 10}\n\nPROPS-END\n{content}")

print(f"生成完成: {OUTPUT}")
print(f"Commits: {COMMITS}")

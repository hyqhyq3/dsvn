# DSvn Dump/Load 功能文档

## 概述

`dsvn-admin` 工具实现了 SVN dump 文件的导入功能，允许从现有 SVN 仓库迁移数据到 DSvn。

## 命令

### load - 导入 SVN dump 文件

从 SVN dump 文件导入到 DSvn 仓库：

```bash
dsvn-admin load --file repo.dump
```

从标准输入读取：

```bash
cat repo.dump | dsvn-admin load --file -
# or
svnadmin dump /path/to/svn/repo | dsvn-admin load --file -
```

### dump - 导出为 SVN dump 格式

导出 DSvn 仓库为 SVN dump 格式（计划中）：

```bash
dsvn-admin dump --repo /path/to/repo --output repo.dump
```

## SVN Dump 文件格式

SVN dump 文件格式定义：https://svn.apache.org/repos/asf/subversion/trunk/notes/dump-load-format.txt

### 基本结构

```
SVN-fs-dump-format-version: 3
UUID: <repository-uuid>

Revision-number: 0
Prop-content-length: <length>
Content-length: <length>

PROPS-END

Revision-number: 1
Prop-content-length: <length>
Content-length: <length>

K 7
svn:log
V <length>
<log message>
K 10
svn:author
V <length>
<author>
K 8
svn:date
V <length>
<date>
PROPS-END

Node-path: <path>
Node-kind: (file|dir)
Node-action: (add|delete|replace|change)
[Node-copyfrom-path: <path>]
[Node-copyfrom-rev: <rev>]
Prop-content-length: <length>
Content-length: <length>

PROPS-END
[Text-content-length: <length>]
Content-length: <length>

<content bytes>
```

### 示例

```
SVN-fs-dump-format-version: 3
UUID: 12345678-1234-1234-1234-123456789abc

Revision-number: 1
Prop-content-length: 116
Content-length: 116

K 7
svn:log
V 11
Initial commit
K 10
svn:author
V 5
admin
K 8
svn:date
V 27
2024-01-06T00:00:00.000000Z
PROPS-END

Node-path: trunk
Node-kind: dir
Node-action: add
Prop-content-length: 10
Content-length: 10

PROPS-END

Node-path: trunk/README.md
Node-kind: file
Node-action: add
Text-content-length: 13
Content-length: 13

Hello DSvn!
```

## 数据结构

### DumpFormat

```rust
pub struct DumpFormat {
    pub format_version: String,
    pub uuid: String,
    pub entries: Vec<DumpEntry>,
}
```

### DumpEntry

```rust
pub struct DumpEntry {
    pub revision_number: u64,
    pub node_path: Option<String>,
    pub node_kind: Option<NodeKind>,
    pub node_action: Option<NodeAction>,
    pub copy_from_path: Option<String>,
    pub copy_from_rev: Option<u64>,
    pub props: HashMap<String, String>,
    pub content: Vec<u8>,
}
```

## 迁移工作流

### 从 SVN 迁移到 DSvn

#### 方法 1: 使用 svnadmin dump

```bash
# 1. 导出 SVN 仓库
svnadmin dump /path/to/svn/repo > repo.dump

# 2. 导入到 DSvn
dsvn-admin load --file repo.dump

# 3. 启动 DSvn 服务器
dsvn start --repo-root /data/dsvn-repo

# 4. 客户端切换 URL
svn switch --relocate \
  http://old-svn-server/repo \
  http://dsvn-server/svn \
  /path/to/working-copy
```

#### 方法 2: 使用管道

```bash
# 直接导出并导入
svnadmin dump /path/to/svn/repo | \
  dsvn-admin load --file -
```

#### 方法 3: 增量导入

```bash
# 导出特定版本范围
svnadmin dump /path/to/svn/repo -r 0:1000 > repo-1.dump
svnadmin dump /path/to/svn/repo -r 1001:2000 --incremental > repo-2.dump

# 导入
dsvn-admin load --file repo-1.dump
dsvn-admin load --file repo-2.dump
```

### 从 DSvn 导出（计划中）

```bash
# 导出为 SVN dump 格式
dsvn-admin dump --repo /data/dsvn-repo --output repo.dump

# 导入到 SVN
svnadmin load /path/to/svn/repo < repo.dump
```

## 测试

### 自动化测试

```bash
./test_dump.sh
```

这将：
1. 创建测试 dump 文件
2. 导入到 DSvn
3. 验证结果

### 手动测试

```bash
# 1. 创建测试 dump 文件
cat > /tmp/test.dump << 'EOF'
SVN-fs-dump-format-version: 3
UUID: test-uuid-123

Revision-number: 1
Prop-content-length: 10
Content-length: 10

PROPS-END
EOF

# 2. 导入
./target/release/dsvn-admin load --file /tmp/test.dump

# 3. 验证
# (需要实现 verify 命令)
```

## 当前限制

### MVP 阶段

1. **只读导入**
   - ✅ 解析 dump 文件格式
   - ✅ 导入到内存仓库
   - ❌ 持久化存储
   - ❌ 完整的属性支持
   - ❌ Copyfrom 操作
   - ❌ Delta 内容

2. **数据持久化**
   - 当前：内存存储
   - 计划：集成 Fjall LSM-tree

### 未来改进

1. **完整支持**
   - 所有属性类型
   - Copyfrom/Copyrev
   - Delta 内容
   - 增量 dump/load

2. **性能优化**
   - 流式处理大文件
   - 并行导入
   - 进度显示

3. **验证工具**
   - dump 文件验证
   - 数据完整性检查
   - 错误恢复

## 常见问题

### Q: 导入大文件会内存溢出吗？

A: MVP 版本会加载到内存。未来版本将实现流式处理。

### Q: 支持增量导入吗？

A: 计划支持。当前只支持完整导入。

### Q: 如何验证导入的正确性？

A: 使用 `svnadmin verify` 对比原始仓库。

### Q: 导出功能什么时候完成？

A: 需要实现持久化存储后才能导出。

## 相关文档

- [SVN Dump Format](https://svn.apache.org/repos/asf/subversion/trunk/notes/dump-load-format.txt)
- [svnadmin dump](https://svnbook.red-bean.com/en/1.8/svn.ref.svnadmin.c.dump.html)
- [svnadmin load](https://svnbook.red-bean.com/en/1.8/svn.ref.svnadmin.c.load.html)

---

**需要帮助？** 请查看 [QUICKSTART.md](QUICKSTART.md) 或提交 issue。

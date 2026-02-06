# SVN → DSvn 迁移指南

## 快速开始

### 一键测试

我们提供了一个完整的自动化测试脚本：

```bash
./test_migration.sh
```

这个脚本将：
1. ✅ 创建 SVN 仓库
2. ✅ 添加测试数据（文件、分支、标签）
3. ✅ 导出为 dump 文件
4. ✅ 导入到 DSvn

## 手动迁移步骤

### 第 1 步：创建 SVN 仓库

```bash
# 创建仓库
svnadmin create /path/to/svn-repo

# 检出工作目录
svn checkout file:///path/to/svn-repo /path/to/wc
cd /path/to/wc
```

### 第 2 步：添加数据

```bash
# 创建标准目录结构
mkdir trunk branches tags
svn add trunk branches tags
svn commit -m "Initialize repository structure"

# 添加文件
echo "Hello DSvn" > trunk/README.md
svn add trunk/README.md
svn commit -m "Add README"
```

### 第 3 步：导出为 dump 文件

```bash
# 导出整个仓库
svnadmin dump /path/to/svn-repo > repo.dump

# 或导出特定版本范围
svnadmin dump /path/to/svn-repo -r 0:1000 > repo.dump

# 或增量导出
svnadmin dump /path/to/svn-repo -r 0:1000 > repo-part1.dump
svnadmin dump /path/to/svn-repo -r 1001:HEAD --incremental > repo-part2.dump
```

### 第 4 步：导入到 DSvn

```bash
# 导入 dump 文件
dsvn-admin load --file repo.dump

# 或从标准输入读取
cat repo.dump | dsvn-admin load --file -

# 或直接从 SVN 导出并导入
svnadmin dump /path/to/svn-repo | dsvn-admin load --file -
```

## 测试脚本详解

### test_migration.sh

脚本创建以下测试数据：

```
svn-repo/
├── trunk/
│   ├── README.md      # 项目说明
│   ├── main.py        # Python 程序
│   └── config.json    # 配置文件
├── branches/
│   └── feature-1/     # 功能分支
└── tags/
    └── v0.1.0/        # 版本标签
```

**版本历史**:
- Rev 1: 初始化目录结构
- Rev 2: 添加初始文件
- Rev 3: 创建分支
- Rev 4: 修改分支
- Rev 5: 创建标签

## 实际迁移场景

### 场景 1: 小型项目（< 1000 文件）

```bash
# 1. 导出
svnadmin dump /svn/project > project.dump

# 2. 导入
dsvn-admin load --file project.dump

# 3. 验证
svn ls http://localhost:8080/svn
```

### 场景 2: 大型项目（分步迁移）

```bash
# 1. 增量导出（每 1000 版本一个文件）
svnadmin dump /svn/large-project -r 0:1000 > part-1.dump
svnadmin dump /svn/large-project -r 1001:2000 --incremental > part-2.dump
svnadmin dump /svn/large-project -r 2001:3000 --incremental > part-3.dump

# 2. 按顺序导入
dsvn-admin load --file part-1.dump
dsvn-admin load --file part-2.dump
dsvn-admin load --file part-3.dump
```

### 场景 3: 迁移并保留历史

```bash
# 1. 完整导出
svnadmin dump /svn/project --deltas > full-history.dump

# 2. 导入（会保留所有历史）
dsvn-admin load --file full-history.dump

# 3. 验证历史
svn log http://localhost:8080/svn
```

## 验证迁移

### 检查版本数量

```bash
# 原始 SVN 仓库
svnlook youngest /svn/repo

# DSvn 仓库
svn log http://localhost:8080/svn | grep "^r[0-9]" | wc -l
```

### 检查文件内容

```bash
# 从原始检出
svn checkout file:///svn/repo /tmp/svn-wc
cat /tmp/svn-wc/trunk/README.md

# 从 DSvn 检出
svn checkout http://localhost:8080/svn /tmp/dsvn-wc
cat /tmp/dsvn-wc/trunk/README.md

# 对比
diff /tmp/svn-wc/trunk/README.md /tmp/dsvn-wc/trunk/README.md
```

### 检查日志

```bash
# 原始 SVN
svn log file:///svn/repo | head -20

# DSvn
svn log http://localhost:8080/svn | head -20
```

## 常见问题

### Q: dump 文件太大怎么办？

A: 使用增量导出和压缩：

```bash
# 分段导出
svnadmin dump /svn/repo -r 0:5000 | gzip > part-1.dump.gz
svnadmin dump /svn/repo -r 5001:10000 --incremental | gzip > part-2.dump.gz

# 导入时解压
gunzip -c part-1.dump.gz | dsvn-admin load --file -
gunzip -c part-2.dump.gz | dsvn-admin load --file -
```

### Q: 如何验证导入成功？

A: 使用 `svnadmin verify` 对比：

```bash
# 验证原始仓库
svnadmin verify /svn/repo

# 验证 DSvn 仓库（需要实现 verify 命令）
dsvn-admin verify --repo /data/dsvn-repo
```

### Q: 支持哪些 dump 格式？

A: 支持标准 SVN dump 格式：
- Format version 2
- Format version 3（推荐）
- 带增量的 dump
- 不带增量的 dump

### Q: 导入失败怎么办？

A: 检查以下几点：

1. dump 文件格式是否正确
```bash
head -n 5 repo.dump
# 应该看到: SVN-fs-dump-format-version: 2 或 3
```

2. 查看详细错误
```bash
RUST_LOG=debug dsvn-admin load --file repo.dump
```

3. 验证 dump 文件完整性
```bash
svnadmin load /tmp/test-repo < repo.dump
```

## 性能考虑

### 大文件处理

对于包含大文件的仓库，使用 `--deltas` 选项：

```bash
svnadmin dump /svn/repo --deltas > repo-deltas.dump
```

### 并行处理

对于超大型仓库，可以分段并行导出：

```bash
# 导出不同的版本范围（并行）
svnadmin dump /svn/repo -r 0:10000 > part1.dump &
svnadmin dump /svn/repo -r 10001:20000 > part2.dump &
svnadmin dump /svn/repo -r 20001:30000 > part3.dump &

wait

# 按顺序导入
for part in part1.dump part2.dump part3.dump; do
    dsvn-admin load --file "$part"
done
```

## 客户端迁移

迁移完成后，切换客户端 URL：

```bash
# 在现有工作目录中
cd /path/to/working-copy

# 切换到新服务器
svn switch --relocate \
  http://old-svn-server/repo \
  http://dsvn-server/svn

# 更新
svn update
```

## 回滚方案

如果迁移失败，可以回滚：

```bash
# 1. 停止 DSvn 服务器
# 2. 恢复原始 SVN 服务器
# 3. 客户端重新切换回去
svn switch --relocate \
  http://dsvn-server/svn \
  http://old-svn-server/repo
```

## 最佳实践

1. **备份优先**
   ```bash
   # 在迁移前备份
   svnadmin hotcopy /svn/repo /backup/svn-repo-$(date +%Y%m%d)
   ```

2. **测试迁移**
   ```bash
   # 先在测试环境验证
   svnadmin dump /svn/repo | dsvn-admin load --file -
   ```

3. **验证完整性**
   ```bash
   # 对比文件数量
   svnlook tree /svn/repo | wc -l
   svn ls -R http://localhost:8080/svn | wc -l
   ```

4. **逐步切换**
   ```bash
   # 1. 迁移到 DSvn
   # 2. 小规模试用（10-20 用户）
   # 3. 收集反馈
   # 4. 全面切换
   ```

---

**下一步**：运行 `./test_migration.sh` 进行完整测试！

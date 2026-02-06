# DSvn 测试脚本使用指南

本目录包含用于自动化测试和验收的脚本。

## 📜 脚本列表

### 1. `acceptance-test.sh` - 完整验收测试
**用途**: 端到端自动化测试，覆盖所有DSvn功能

**测试内容**:
- ✅ 依赖检查
- ✅ 项目编译
- ✅ 仓库初始化
- ✅ 服务器启动
- ✅ 基础SVN操作（checkout, add, commit, update等）
- ✅ 高级SVN操作（mkdir, mv, cp, rm, diff等）
- ✅ 并发测试
- ✅ 性能测试
- ✅ 结果验证
- ✅ 测试报告生成

**运行时间**: 约2-3分钟

**输出**: 生成测试报告到 `/tmp/dsvn-test-report.md`

### 2. `quick-test.sh` - 快速测试
**用途**: 日常开发中的快速验证

**测试内容**:
- ✅ 编译检查
- ✅ 仓库初始化
- ✅ 服务器启动
- ✅ 基础SVN操作
- ✅ 简单的目录/文件操作

**运行时间**: 约30秒

**特点**:
- 服务器保持运行，便于手动测试
- 失败时自动清理

## 🚀 使用方法

### 方法1: 使用Makefile（推荐）

```bash
# 快速测试（日常开发）
make quick-test

# 完整验收测试
make acceptance-test

# 查看所有可用命令
make help
```

### 方法2: 直接运行脚本

```bash
# 快速测试
./scripts/quick-test.sh

# 完整验收测试
./scripts/acceptance-test.sh
```

## 📋 测试场景说明

### 场景1: 快速开发验证
```bash
# 修改代码后快速验证
make quick-test
```

**输出示例**:
```
[INFO] 检查编译...
[✓] 可执行文件就绪
[INFO] 初始化仓库...
[✓] 仓库已初始化
[INFO] 启动服务器 (端口 8989)...
[✓] 服务器已启动 (PID: 12345)
...
========================================
  快速测试全部通过！
========================================
```

### 场景2: 完整功能验证
```bash
# 发布前或重大变更后
make acceptance-test
```

**测试流程**:
1. **准备阶段** (30秒)
   - 编译项目
   - 初始化仓库
   - 启动服务器

2. **基础操作测试** (60秒)
   - Checkout工作副本
   - 添加多种类型文件
   - 提交变更
   - 更新和日志查看

3. **高级操作测试** (60秒)
   - 创建目录（MKCOL）
   - 移动/复制/删除文件
   - 查看差异
   - 设置属性

4. **并发测试** (30秒)
   - 3个并发工作副本
   - 同时提交变更

5. **性能测试** (30秒)
   - 批量操作100个文件
   - 测量提交时间

6. **验证和报告** (10秒)
   - 仓库完整性验证
   - 生成Markdown报告

## 🧪 测试的SVN命令

### 基础命令
- `svn checkout` - 检出仓库
- `svn add` - 添加文件
- `svn commit` - 提交变更
- `svn update` - 更新工作副本
- `svn status` - 查看状态
- `svn log` - 查看日志
- `svn info` - 查看信息

### 高级命令
- `svn mkdir` - 创建目录
- `svn mv` - 移动文件/目录
- `svn cp` - 复制文件/目录
- `svn rm` - 删除文件/目录
- `svn diff` - 查看差异
- `svn ls` - 列出文件
- `svn export` - 导出不包含.svn的副本
- `svn propset` - 设置属性

## 📊 测试数据

脚本会创建以下测试文件：

### 文本文件
- `README.md` - Markdown文档
- `config.toml` - TOML配置文件

### 代码文件
- `main.rs` - Rust代码

### 可执行文件
- `script.sh` - Bash脚本（+x权限）

### 二进制文件
- `binary.bin` - 二进制数据
- `large.bin` - 10MB大文件

## 🔍 故障排除

### 问题1: 端口被占用
```bash
# 停止所有测试服务器
make stop-test

# 或手动停止
lsof -ti:8080 | xargs kill -9
lsof -ti:8989 | xargs kill -9
```

### 问题2: 测试失败
```bash
# 查看服务器日志
cat /tmp/dsvn-server.log

# 或实时查看
tail -f /tmp/dsvn-server.log
```

### 问题3: 清理并重新测试
```bash
# 清理所有测试数据
make clean

# 重新运行测试
make acceptance-test
```

## 📈 性能基准

预期测试时间（参考）:

| 测试类型 | 文件数 | 预期时间 |
|---------|--------|----------|
| 快速测试 | ~5 | 30秒 |
| 基础操作 | ~10 | 60秒 |
| 高级操作 | ~15 | 60秒 |
| 并发测试 | 3工作副本 | 30秒 |
| 性能测试 | 100文件 | 30秒 |

## 🎯 持续集成

这些脚本可以集成到CI/CD流程中：

```yaml
# .github/workflows/test.yml 示例
name: DSvn Tests
on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - name: Install SVN
        run: sudo apt-get install subversion
      - name: Run tests
        run: make acceptance-test
```

## 📝 自定义测试

### 修改服务器端口
编辑脚本中的`PORT`或`SERVER_PORT`变量：

```bash
# quick-test.sh
PORT=9999

# acceptance-test.sh
SERVER_PORT=9999
```

### 修改测试数据
编辑脚本中的`prepare_test_data()`函数。

### 添加新测试
在相应脚本中添加新的测试函数，并在`main()`中调用。

## 🔄 工作流程建议

### 日常开发
```bash
# 1. 修改代码
vim dsvn-webdav/src/handlers.rs

# 2. 快速测试
make quick-test

# 3. 如果通过，继续开发
# 4. 如果失败，查看日志
make logs
```

### 提交前
```bash
# 1. 完整测试
make acceptance-test

# 2. 代码检查
make clippy

# 3. 格式化
make fmt

# 4. 提交
git add .
git commit -m "Feature: ..."
```

### 发布前
```bash
# 1. 完整的开发流程
make dev

# 2. 生产就绪检查
make production-ready

# 3. 打包
cargo build --release
```

## 📞 获取帮助

```bash
# 查看所有Makefile命令
make help

# 查看脚本使用说明
cat scripts/README.md
```

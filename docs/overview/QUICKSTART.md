# DSvn MVP - 快速入门指南

## 🎯 什么是 DSvn MVP

DSvn MVP 是一个最小可行产品，实现了基本的 SVN 协议兼容功能：

- ✅ 使用标准 SVN 客户端检出
- ✅ 提交文件
- ✅ 查看日志
- ✅ 列出文件

**注意**: MVP 使用内存存储，重启后数据会丢失。

## 📦 前置要求

### 1. 安装 Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### 2. 安装 SVN 客户端

**macOS**:
```bash
brew install subversion
```

**Ubuntu/Debian**:
```bash
sudo apt-get install subversion
```

## 🚀 5 分钟快速开始

### 步骤 1: 构建 DSvn

```bash
cd /Users/yangqihuang/.openclaw/workspace/dsvn
cargo build --release
```

**预期输出**:
```
Compiling dsvn-core v0.1.0
Compiling dsvn-webdav v0.1.0
Compiling dsvn-server v0.1.0
Finished `release` profile [optimized] target(s) in X.XXs
```

### 步骤 2: 启动服务器

**终端 1**:
```bash
./target/release/dsvn start --repo-root ./data/repo --debug
```

**预期输出**:
```
Starting DSvn server on 0.0.0.0:8080
Repository root: ./data/repo
Initializing in-memory repository (MVP mode)
Server listening on 0.0.0.0:8080
Ready to accept SVN client connections
```

保持这个终端运行。

### 步骤 3: 检出仓库

**终端 2**:
```bash
svn checkout http://localhost:8080/svn /tmp/dsvn-wc
```

**预期输出**:
```
Checked out revision 0.
```

### 步骤 4: 创建和提交文件

```bash
cd /tmp/dsvn-wc
echo "Hello DSvn!" > README.md
svn add README.md
svn commit -m "Initial commit"
```

**预期输出**:
```
Adding         README.md
Transmitting file data .done
Committing transaction...
Committed revision 1.
```

### 步骤 5: 查看日志

```bash
svn log
```

**预期输出**:
```
------------------------------------------------------------------------
r1 | test_user | 2024-01-06 00:00:00 +0000 (Sun, 06 Jan 2024) | 1 line
Test commit via MERGE
------------------------------------------------------------------------
```

## 🧪 自动化测试

我们提供了一个自动化测试脚本：

```bash
./test_mvp.sh
```

这将运行完整的测试流程：
- ✅ 检出仓库
- ✅ 列出文件
- ✅ 创建测试文件
- ✅ 提交变更
- ✅ 查看日志

## 📂 项目结构

```
dsvn/
├── dsvn-core/          # 核心库 (对象模型、存储)
├── dsvn-webdav/        # WebDAV 协议实现
├── dsvn-server/        # 服务器主程序
├── dsvn-cli/           # 管理工具
├── test_mvp.sh         # 自动化测试脚本
├── MVP_SUMMARY.md      # MVP 实现总结
└── QUICKSTART.md       # 本文件
```

## 🔧 常用命令

### 服务器管理

```bash
# 启动服务器 (调试模式)
./target/release/dsvn start --repo-root ./data/repo --debug

# 启动服务器 (生产模式)
./target/release/dsvn start --repo-root ./data/repo

# 初始化仓库
./target/release/dsvn-admin init /path/to/repo
```

### SVN 客户端操作

```bash
# 检出
svn checkout http://localhost:8080/svn /tmp/wc

# 更新
cd /tmp/wc
svn update

# 添加文件
svn newfile.txt
svn add newfile.txt

# 提交
svn commit -m "Add new file"

# 查看日志
svn log

# 查看状态
svn status

# 查看文件内容
svn cat README.md
```

## 🐛 故障排除

### 问题: 端口已被占用

**错误信息**:
```
Error: Os { code: 48, kind: AddrInUse, message: "Address already in use" }
```

**解决方案**:
```bash
# 查找占用端口的进程
lsof -i :8080

# 杀死进程
kill -9 <PID>

# 或者使用其他端口
./target/release/dsvn start --repo-root ./data/repo --addr 0.0.0.0:8081
```

### 问题: SVN 客户端连接失败

**检查**:
1. 服务器是否运行: `curl http://localhost:8080/`
2. 防火墙是否阻止
3. 端口是否正确

### 问题: 编译错误

**确保**:
1. Rust 版本 >= 1.70: `rustc --version`
2. 依赖已更新: `cargo update`
3. 清理重建: `cargo clean && cargo build`

## 📚 下一步

### 学习更多

- **[MVP_SUMMARY.md](MVP_SUMMARY.md)**: MVP 实现总结
- **[ARCHITECTURE.md](ARCHITECTURE.md)**: 架构设计
- **[PERFORCE_ANALYSIS.md](PERFORCE_ANALYSIS.md)**: Perforce 借鉴分析
- **[ROADMAP.md](ROADMAP.md)**: 开发路线图

### 参与贡献

我们欢迎贡献！重点领域：
1. 持久化存储 (Fjall 集成)
2. 完善事务管理
3. 改进错误处理
4. 添加更多测试

## ⚠️ 已知限制

1. **内存存储**: 数据在重启后丢失
2. **简单认证**: 无权限控制
3. **基本错误处理**: 错误消息不够详细
4. **单线程提交**: 串行化提交（后续优化）

## 💡 提示

### 调试技巧

1. **启用调试日志**:
   ```bash
   RUST_LOG=debug ./target/release/dsvn start --repo-root ./data/repo --debug
   ```

2. **查看请求日志**:
   服务器会输出每个请求的详细信息

3. **使用 curl 测试**:
   ```bash
   curl -v http://localhost:8080/svn
   ```

### 性能测试

```bash
# 创建大量文件
for i in {1..100}; do
  echo "File $i" > file$i.txt
done
svn add file*.txt
svn commit -m "Add 100 files"
```

---

**需要帮助?** 请查看 [DEVELOPMENT.md](DEVELOPMENT.md) 或提交 issue。

**准备好了吗?** 让我们开始吧！🚀

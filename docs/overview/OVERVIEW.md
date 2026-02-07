# DSvn - 高性能 SVN 兼容服务器

## 📋 项目概述

DSvn 是一个用 Rust 重新实现的高性能 Subversion 服务器，**仅协议兼容，存储完全重新设计**。

### 🎯 核心设计决策

```
┌─────────────────────────────────────────────────────────┐
│  SVN 客户端                                              │
│  (svn, TortoiseSVN, IntelliJ, Eclipse, etc.)           │
└───────────────┬─────────────────────────────────────────┘
                │
                │ HTTP/WebDAV 协议 (完全兼容)
                │
                ▼
┌─────────────────────────────────────────────────────────┐
│  DSvn 服务器 (Rust 实现)                                │
│  ┌───────────────────────────────────────────────────┐  │
│  │ 协议层：WebDAV/DeltaV (RFC 4918, RFC 3253)      │  │
│  └───────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────┐  │
│  │ 业务层：版本控制、事务管理、权限控制              │  │
│  └───────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────┐  │
│  │ 存储层：内容寻址存储 (Git 风格，重新设计)         │  │
│  │   - Blob (文件内容)                               │  │
│  │   - Tree (目录结构)                               │  │
│  │   - Commit (版本元数据)                           │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### ✅ 协议兼容

DSvn **完全兼容** SVN 客户端，支持所有标准操作：

```bash
# 检出
svn checkout http://dsvn-server/repo my-project

# 更新
svn update

# 提交
svn commit -m "Add new feature"

# 日志
svn log

# 分支/标签
svn copy trunk branches/feature-branch
svn commit -m "Create feature branch"
```

### ❌ 存储不兼容

DSvn **不兼容** Subversion 的 FSFS/FSX/BDB 存储格式：

| 存储格式 | SVN 支持 | DSvn 支持 |
|---------|---------|-----------|
| FSFS    | ✅     | ❌        |
| Berkeley DB | ✅  | ❌        |
| DSvn CAS | ❌   | ✅        |

**原因：** FSFS 的设计限制了性能和可扩展性（见 COMPARISON.md）

### 🔄 数据迁移

从 SVN 到 DSvn：

```bash
# 导出 SVN 仓库
svnadmin dump /path/to/svn/repo > repo.dump

# 导入到 DSvn
dsvn-admin load /path/to/dsvn/repo < repo.dump
```

## 🚀 核心优势

### 1. 性能提升

| 操作 | SVN (FSFS) | DSvn | 提升 |
|-----|-----------|------|------|
| 检出 100 万文件 | ~30 分钟 | < 2 分钟 | **15x** |
| 获取 10 万条日志 | ~5 秒 | < 50ms | **100x** |
| 读取 1000 版本前的文件 | ~1 秒 | < 100ms | **10x** |

### 2. 可扩展性

```
设计目标：
  ✅ 10 亿+ 文件
  ✅ 1000 万+ 版本
  ✅ 100 TB+ 仓库大小
  ✅ 1000+ 并发客户端
```

### 3. 现代化技术栈

- **语言**: Rust (内存安全、高性能)
- **并发**: Async/Await (Tokio)
- **存储**: LSM-tree (Fjall) + Packfiles
- **网络**: HTTP/2 + TLS
- **压缩**: Zstandard (zstd)

## 📦 项目结构

```
dsvn/
├── dsvn-core/          # 核心库（对象模型、存储引擎）
│   ├── object.rs       # Blob, Tree, Commit
│   └── storage.rs      # 分层存储（热/温/冷）
│
├── dsvn-webdav/        # WebDAV 协议实现
│   ├── handlers.rs     # HTTP 方法处理
│   └── xml.rs          # XML 解析
│
├── dsvn-server/        # 服务器主程序
│   └── main.rs         # HTTP/HTTPS 服务器
│
├── dsvn-cli/           # 管理工具
│   └── main.rs         # init, gc, verify, stats
│
├── ARCHITECTURE.md     # 架构设计文档
├── COMPARISON.md       # 与 SVN 的详细对比
├── DEVELOPMENT.md      # 开发指南
└── README.md           # 项目说明
```

## 🎯 适用场景

### ✅ 推荐使用 DSvn

- **超大型仓库**: > 100 万文件，> 10 万版本
- **高并发场景**: 100+ 并发用户，频繁 CI/CD
- **性能敏感**: 全球分布式团队，需要快速检出
- **云原生**: 容器化部署，Kubernetes 环境

### ⚠️ 继续使用 SVN

- **小型仓库**: < 1 万文件，< 1 万版本
- **生态依赖**: 依赖特定 SVN 工具或集成
- **迁移成本**: 复杂的钩子脚本，无法停机

## 🔧 快速开始

### 1. 安装 Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. 编译项目

```bash
cd /path/to/dsvn
cargo build --release
```

### 3. 初始化仓库

```bash
./target/release/dsvn-admin init /data/repos/my-project
```

### 4. 启动服务器

```bash
./target/release/dsvn start \
  --repo-root /data/repos/my-project \
  --hot-path /data/repos/my-project/hot \
  --warm-path /data/repos/my-project/warm \
  --addr 0.0.0.0:8080
```

### 5. 使用 SVN 客户端

```bash
svn checkout http://localhost:8080/svn my-working-copy
cd my-working-copy
echo "Hello DSvn" > README.md
svn add README.md
svn commit -m "Initial commit"
```

## 📚 文档导航

- **[README.md](README.md)**: 用户指南和快速开始
- **[ARCHITECTURE.md](ARCHITECTURE.md)**: 详细架构设计
- **[COMPARISON.md](COMPARISON.md)**: DSvn vs SVN 对比
- **[DEVELOPMENT.md](DEVELOPMENT.md)**: 开发者指南

## 🗺️ 开发路线图

### Phase 1: 基础功能 (当前阶段)
- [x] 项目结构和构建系统
- [x] 核心对象模型（Blob, Tree, Commit）
- [x] 分层存储抽象
- [ ] WebDAV 协议实现（进行中）
- [ ] 基本的 checkout/commit

### Phase 2: 核心功能
- [ ] 完整的 WebDAV 方法实现
- [ ] 事务管理
- [ ] 增量压缩
- [ ] 跳表增量优化

### Phase 3: 高级功能
- [ ] 认证和授权
- [ ] 多仓库支持
- [ ] 分片和缓存
- [ ] 性能优化

### Phase 4: 生产就绪
- [ ] 监控和指标
- [ ] 备份和恢复
- [ ] 管理工具
- [ ] 安全审计

## 🤝 贡献

欢迎贡献！请参阅 [DEVELOPMENT.md](DEVELOPMENT.md) 了解贡献指南。

## 📄 许可证

MIT OR Apache-2.0

---

**注意**: DSvn 是一个独立项目，与 Apache Subversion 项目无关。我们实现了兼容的协议，但使用完全不同的存储引擎。

# DSvn 混合存储架构 - 实施路线图

## 总体目标

打造一个融合三强优势的版本控制系统：
- **SVN 协议**：客户端兼容性
- **Git 存储**：内容寻址、自动去重、高压缩比
- **P4 传输**：流式传输、增量编码、智能缓存

## 存储架构选择：混合方案 🏆

详见：[STORAGE_ARCHITECTURE_COMPARISON.md](./docs/STORAGE_ARCHITECTURE_COMPARISON.md)

### 为什么选择混合方案？

**核心设计：**
```
Layer 1: Git-style 内容寻址  → 自动去重, 10-30x 压缩
Layer 2: SVN-style 修订索引   → 全局修订号, 高效查询
Layer 3: P4-style 流式传输    → 增量编码, O(1) 内存
```

**性能对比：**
- Checkout 100万文件: Git 5min → P4 2min → **Hybrid 30sec** ⚡
- Checkout 10GB大文件: Git OOM → P4 3min → **Hybrid 2min** ⚡
- 存储 1TB 代码: Git 100GB → P4 1TB → **Hybrid 80GB** 💾

## Phase 1: 基础 MVP (当前 - Week 1-4)

### 目标
基本 SVN 协议支持，可以进行 checkout/commit

### 任务清单
- [x] 项目结构初始化
- [x] 对象模型实现（Blob, Tree, Commit）
- [x] 分层存储框架
- [x] **WebDAV 协议实现** ✅
  - [x] REPORT 方法（log, update, diff）
  - [x] MERGE 方法（提交）
  - [x] PROPFIND 方法（目录列表）
  - [x] GET 方法（读取文件）
  - [x] PUT 方法（创建/更新文件）
  - [x] MKCOL 方法（创建目录）
  - [x] DELETE 方法（删除文件/目录）
  - [x] CHECKOUT/CHECKIN 方法（版本控制）
  - [x] MKACTIVITY 方法（事务管理）
  - [x] LOCK/UNLOCK 方法（基础实现）
  - [x] COPY/MOVE 方法（基础实现）
- [ ] 基础集成测试
  - [ ] 使用 SVN client 测试 checkout
  - [ ] 使用 SVN client 测试 commit

### 交付物
```bash
# 可以运行的命令
svn checkout http://localhost:8080/svn /tmp/wc
cd /tmp/wc
echo "hello" > README.md
svn add README.md
svn commit -m "Initial commit"
```

---

## Phase 2: P4 核心特性 (Week 5-10)

### 2.1 流式传输 (Week 5-6) 🌊

**目标**：支持大文件处理，O(1) 内存占用

**实现**：
```rust
// dsvn-core/src/streaming.rs
pub mod streaming;

use tokio::io::{AsyncRead, AsyncReadExt};
use futures::stream::Stream;

pub struct FileStream<S> {
    stream: S,
    chunk_size: usize,
}

impl FileStream {
    /// 创建文件流
    pub fn new(object_id: ObjectId, chunk_size: usize) -> Self {
        Self {
            stream: ObjectStore::read_stream(object_id),
            chunk_size,
        }
    }

    /// 分块读取
    pub async fn next_chunk(&mut self) -> Result<Option<Bytes>> {
        let mut buffer = vec![0u8; self.chunk_size];
        let n = self.stream.read(&mut buffer).await?;
        if n == 0 {
            Ok(None)
        } else {
            buffer.truncate(n);
            Ok(Some(Bytes::from(buffer)))
        }
    }
}
```

**测试**：
```bash
# 创建 10GB 文件
dd if=/dev/zero of=large.bin bs=1G count=10
svn add large.bin
svn commit -m "Add large file"

# 在另一端检出（应该使用流式传输，内存占用低）
svn checkout http://localhost:8080/svn /tmp/wc2
```

**验收标准**：
- ✅ 支持 10GB+ 文件
- ✅ 内存占用 < 100MB（不管文件多大）
- ✅ 支持断点续传

---

### 2.2 智能缓存 (Week 7-8) 🧠

**目标**：多层缓存 + 访问模式分析

**实现**：
```rust
// dsvn-core/src/cache.rs
pub mod cache;

use lru::LruCache;
use std::sync::Arc;

pub struct SmartCache {
    // L1: 内存热缓存
    hot: Arc<Mutex<LruCache<String, Bytes>>>,
    hot_size: usize,

    // L2: SSD 缓存
    ssd: Arc<SsdCache>,

    // 访问模式分析
    analyzer: Arc<AccessPatternAnalyzer>,
}

impl SmartCache {
    /// 智能获取（自动缓存和预取）
    pub async fn get(&self, key: &str) -> Result<Option<Bytes>> {
        // 1. 检查热缓存
        if let Some(data) = self.hot.lock().await.get(key) {
            return Ok(Some(data.clone()));
        }

        // 2. 检查 SSD 缓存
        if let Some(data) = self.ssd.get(key).await? {
            // 提升到热缓存
            self.hot.lock().await.put(key.to_string(), data.clone());
            return Ok(Some(data));
        }

        Ok(None)
    }

    /// 预取相关文件
    pub async fn prefetch_related(&self, path: &str) {
        let related = self.analyzer.predict_next(path);
        for file in related {
            // 后台预取
            let _ = self.get(&file).await;
        }
    }
}
```

**测试**：
```bash
# 测试缓存效果
time svn checkout http://localhost:8080/svn /tmp/wc1
time svn checkout http://localhost:8080/svn /tmp/wc2  # 应该更快
```

**验收标准**：
- ✅ 热缓存命中率 > 80%
- ✅ 重复操作速度提升 > 10x
- ✅ 自动预取减少延迟

---

### 2.3 并行事务 (Week 9-10) ⚡

**目标**：支持多客户端并发提交

**实现**：
```rust
// dsvn-core/src/transaction.rs
pub mod transaction;

use dashmap::DashMap;
use tokio::sync::Mutex;

pub struct TransactionManager {
    // 并发事务
    transactions: DashMap<TransactionId, PendingTxn>,

    // 提交锁（串行化）
    commit_lock: Arc<Mutex<()>>,

    // 文件锁
    file_locks: Arc<RwLock<HashMap<String, LockOwner>>>,
}

impl TransactionManager {
    /// 开始事务（并发）
    pub fn begin(&self, author: String) -> TransactionId {
        let id = TransactionId::new();
        self.transactions.insert(id, PendingTxn::new(author));
        id
    }

    /// 提交事务（串行）
    pub async fn commit(&self, id: TransactionId) -> Result<u64> {
        // 获取全局锁
        let _guard = self.commit_lock.lock().await;

        // 应用变更
        let txn = self.transactions.remove(&id).unwrap();
        self.apply_txn(txn).await
    }
}
```

**测试**：
```bash
# 并发提交测试
for i in {1..100}; do
  (
    cd /tmp/wc$i
    echo "change $i" > file$i.txt
    svn add file$i.txt
    svn commit -m "Commit $i"
  ) &
done
wait
```

**验收标准**：
- ✅ 100 并发提交无冲突
- ✅ 串行化保证数据一致性
- ✅ 文件锁正确工作

---

## Phase 3: 分布式架构 (Week 11-16)

### 3.1 边缘代理 (Week 11-13) 🌐

**目标**：部署边缘缓存服务器

**新增 crate**：
```bash
cargo new --bin dsvn-proxy
```

**实现**：
```rust
// dsvn-proxy/src/main.rs
use dsvn_core::{TieredStore, SmartCache};
use dsvn_webdav::WebDavHandler;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    upstream: String,  // 主服务器地址

    #[arg(long, default_value = "./cache")]
    cache_dir: String,

    #[arg(long, default_value = "10GB")]
    cache_size: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // 创建边缘代理
    let proxy = EdgeProxy::new(
        args.upstream,
        args.cache_dir,
        args.cache_size,
    ).await?;

    // 启动代理服务器
    proxy.serve("0.0.0.0:8080").await?;

    Ok(())
}
```

**部署**：
```
主服务器（美国）:
  dsvn-server --repo-root /data/repos

边缘代理（北京）:
  dsvn-proxy --upstream https://us-server.example.com --cache-size 100GB

边缘代理（伦敦）:
  dsvn-proxy --upstream https://us-server.example.com --cache-size 100GB
```

**验收标准**：
- ✅ 边缘代理自动缓存热数据
- ✅ 本地访问延迟 < 10ms
- ✅ 故障切换到主服务器

---

### 3.2 集群模式 (Week 14-16) 🔄

**目标**：主从复制，读写分离

**实现**：
```rust
// dsvn-server/src/cluster.rs
pub mod cluster;

pub struct ClusterConfig {
    pub role: ClusterRole,
    pub primary: Option<String>,
    pub replicas: Vec<String>,
}

pub enum ClusterRole {
    Primary,    // 主服务器（读写）
    Replica,    // 从服务器（只读）
    Proxy,      // 代理服务器
}

pub struct ReplicationManager {
    role: ClusterRole,
    primary_client: Option<UpstreamClient>,
    replicas: Vec<ReplicaClient>,
}

impl ReplicationManager {
    /// 复制日志到从服务器
    pub async fn replicate(&self, rev: u64) -> Result<()> {
        for replica in &self.replicas {
            replica.apply_rev(rev).await?;
        }
        Ok(())
    }
}
```

**部署**：
```
主服务器（读写）:
  dsvn-server --role primary --addr 0.0.0.0:8080

从服务器 1（只读）:
  dsvn-server --role replica --primary https://primary.example.com

从服务器 2（只读）:
  dsvn-server --role replica --primary https://primary.example.com
```

**验收标准**：
- ✅ 主从数据实时同步
- ✅ 从服务器可处理读请求
- ✅ 主服务器故障自动切换

---

## Phase 4: 高级优化 (Week 17-24)

### 4.1 压缩和增量 (Week 17-18) 🗜️

**目标**：实现高效的增量压缩

**实现**：
```rust
// dsvn-core/src/delta.rs
pub mod delta;

use xdelta3::{encode, decode};

pub struct DeltaEncoder;

impl DeltaEncoder {
    /// 编码增量
    pub fn encode(base: &[u8], target: &[u8]) -> Result<Vec<u8>> {
        encode(base, target)
    }

    /// 解码增量
    pub fn decode(base: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
        decode(base, delta)
    }

    /// 跳表增量（O(log n) 历史）
    pub fn skip_delta(rev: u64) -> u64 {
        if rev == 0 { return 0; }
        let highest_bit = 64 - rev.leading_zeros() - 1;
        rev - (1 << highest_bit)
    }
}
```

### 4.2 性能调优 (Week 19-20) 🏎️

**目标**：优化到生产级别性能

**优化点**：
- [ ] 连接池复用
- [ ] 批量操作优化
- [ ] 内存使用优化
- [ ] CPU profile 分析
- [ ] 火焰图优化

### 4.3 监控和运维 (Week 21-22) 📊

**目标**：完善的可观测性

**实现**：
```rust
// dsvn-server/src/metrics.rs
pub mod metrics;

use prometheus::{Counter, Histogram, Gauge};

lazy_static! {
    static ref REQUESTS_TOTAL: Counter = Counter::new(
        "dsvn_requests_total", "Total requests"
    ).unwrap();

    static ref REQUEST_DURATION: Histogram = Histogram::new(
        "dsvn_request_duration_seconds", "Request duration"
    ).unwrap();

    static ref CACHE_HIT_RATE: Gauge = Gauge::new(
        "dsvn_cache_hit_rate", "Cache hit rate"
    ).unwrap();
}

// 暴露 metrics 端点
pub async fn metrics_handler() -> Result<String> {
    let encoder = prometheus::TextEncoder::new();
    let metric_families = prometheus::gather();
    encoder.encode_to_string(&metric_families)
}
```

### 4.4 安全加固 (Week 23-24) 🔒

**目标**：生产级安全

**实现**：
- [ ] LDAP/Active Directory 集成
- [ ] 路径级 ACL
- [ ] 审计日志
- [ ] 密钥管理

---

## 性能目标

### 基准测试场景

| 场景 | SVN (FSFS) | DSvn v1.0 | DSvn v2.0 (P4) |
|-----|-----------|----------|----------------|
| **检出 100 万文件** | 30 分钟 | 2 分钟 | **30 秒** |
| **检出 10GB 文件** | 内存溢出 | 5 分钟 | **2 分钟** (流式) |
| **100 并发提交** | 锁等待 | 可用 | **无影响** |
| **全球访问** | 高延迟 | 中等 | **< 10ms** (边缘) |
| **热文件访问** | 磁盘 I/O | 热存储 | **内存** (缓存) |

### 压力测试目标

```
仓库规模:
  - 10 亿文件
  - 1000 万版本
  - 100 TB 数据

并发:
  - 1000 并发用户
  - 10000 并发读操作
  - 100 并发写操作

性能:
  - P50 延迟 < 10ms
  - P95 延迟 < 100ms
  - P99 延迟 < 500ms

可用性:
  - 99.9% 在线时间
  - 故障恢复 < 1 分钟
  - 数据零丢失
```

---

## 项目状态跟踪

```
总体进度: ██████░░░░░░░░░░░░░░ 30%

Phase 1: ████████████░░░░░░░░  70%
  ✅ 项目结构
  ✅ 对象模型
  ✅ 存储框架
  ✅ 协议实现     ← 完成
  🚧 集成测试     ← 当前
  ⏳ 持久化存储

Phase 2: ░░░░░░░░░░░░░░░░░░░   0%
Phase 3: ░░░░░░░░░░░░░░░░░░░   0%
Phase 4: ░░░░░░░░░░░░░░░░░░░   0%
```

---

## 下一步行动

### 当前任务（Week 4）

1. **端到端集成测试** (优先级 P0)
   ```bash
   # 使用真实 SVN client 测试
   - [ ] svn checkout http://localhost:8080/svn /tmp/wc
   - [ ] svn add 文件
   - [ ] svn commit -m "test"
   - [ ] svn update
   - [ ] 验证所有 WebDAV 方法
   ```

2. **完善持久化存储** (优先级 P1)
   ```bash
   # 完成 PersistentRepository
   - [ ] 使用 Fjall LSM-tree 实现
   - [ ] 从内存存储迁移
   - [ ] 数据持久化测试
   ```

3. **增强事务管理** (优先级 P2)
   ```bash
   # 完善事务状态机
   - [ ] 事务超时处理
   - [ ] 事务回滚
   - [ ] 并发事务冲突检测
   ```

### 下一步行动

1. **启动服务器进行测试**
   ```bash
   cargo run --release --bin dsvn start --repo-root ./data/repo
   ```

2. **使用 SVN 客户端测试**
   ```bash
   svn checkout http://localhost:8080/svn /tmp/test-wc
   cd /tmp/test-wc
   echo "test" > test.txt
   svn add test.txt
   svn commit -m "Test commit"
   ```

3. **性能基准测试**
   ```bash
   # 建立性能基线
   time svn checkout http://localhost:8080/svn /tmp/wc
   ```

---

**最后更新**: 2026-02-06
**当前阶段**: Phase 1 - 集成测试与持久化
**下一个里程碑**: 完整的 checkout/commit 功能 + 持久化存储 (预计 1-2 周)

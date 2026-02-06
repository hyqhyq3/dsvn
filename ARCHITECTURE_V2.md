# DSvn 架构设计 v2.0

## 设计理念：三强融合

```
┌─────────────────────────────────────────────────────────────┐
│                  DSvn = SVN + P4 + Git                      │
│                                                              │
│  SVN Protocol  →  客户端兼容，无缝迁移                      │
│  P4 Architecture →  分布式、流式、智能缓存                   │
│  Git Storage    →  内容寻址、自动去重                        │
└─────────────────────────────────────────────────────────────┘
```

## 核心组件

### 1. 内容寻址存储（Git 风格）

```rust
/// 对象模型
pub enum Object {
    Blob(Vec<u8>),              // 文件内容
    Tree(Vec<TreeEntry>),       // 目录结构
    Commit(CommitData),         // 版本元数据
}

/// 内容寻址：ID = SHA-256(内容)
impl Object {
    pub fn id(&self) -> ObjectId {
        let bytes = bincode::serialize(self).unwrap();
        ObjectId::sha256(&bytes)
    }
}
```

**优势：**
- ✅ 自动去重（相同内容只存一次）
- ✅ O(1) 对象查找
- ✅ 天然支持并行
- ✅ 简化压缩和归档

### 2. 边缘代理架构（P4 风格）

```
全球部署示例:

北京办公室                ┐
                        │
  [Edge Proxy] ─────────┼───> [Commit Server] (主服务器)
    ↓ 本地缓存          │        ↓
    低延迟              │      权威数据源
                        │
纽约办公室                │
                        │
  [Edge Proxy] ─────────┘
    ↓ 本地缓存
    低延迟
```

**边缘代理实现：**
```rust
pub struct EdgeProxy {
    // L1: 内存缓存（最热数据）
    hot_cache: Arc<RwLock<LruCache<Path, Bytes>>>,
    hot_size: usize,  // 例如：1GB

    // L2: SSD 缓存（温数据）
    ssd_cache: Arc<SsdStore>,
    ssd_size: usize,  // 例如：100GB

    // L3: 上游服务器连接
    upstream: Arc<UpstreamClient>,

    // 智能预取
    prefetcher: Arc<Prefetcher>,
}

impl EdgeProxy {
    /// 读取文件（优先缓存）
    pub async fn get_file(&self, path: &str, rev: u64) -> Result<Bytes> {
        // 1. 检查热缓存
        if let Some(data) = self.hot_cache.read().await.get(path) {
            return Ok(data.clone());
        }

        // 2. 检查 SSD 缓存
        if let Some(data) = self.ssd_cache.get(path, rev).await? {
            // 提升到热缓存
            self.hot_cache.write().await.put(path, data.clone());
            return Ok(data);
        }

        // 3. 从上游获取
        let data = self.upstream.get_file(path, rev).await?;

        // 4. 缓存到 SSD
        self.ssd_cache.put(path, rev, &data).await?;

        // 5. 预取相关文件
        self.prefetcher.prefetch_related(path).await;

        Ok(data)
    }
}
```

### 3. 流式传输（P4 风格）

```rust
use tokio::io::AsyncReadExt;
use futures::stream::Stream;

/// 流式读取大文件
pub fn stream_file(
    &self,
    path: &str,
    rev: u64,
) -> impl Stream<Item = Result<Bytes>> {
    async_stream::try_stream! {
        // 解析对象 ID
        let object_id = self.resolve_path(path, rev).await?;

        // 分块读取（每次 1MB）
        let chunk_size = 1024 * 1024;
        let mut offset = 0;

        loop {
            let chunk = self.store.read_chunk(object_id, offset, chunk_size).await?;

            if chunk.is_empty() {
                break;
            }

            yield Bytes::from(chunk);
            offset += chunk_size;
        }
    }
}

/// HTTP 响应（分块传输）
pub async fn get_handler(
    req: Request<Incoming>,
) -> Result<Response<Body>> {
    let path = parse_path(req.uri())?;
    let rev = parse_revision(req.uri())?;

    // 创建流
    let stream = storage.stream_file(&path, rev).await?;

    // 返回流式响应
    Ok(Response::builder()
        .header("Transfer-Encoding", "chunked")
        .header("Content-Type", "application/octet-stream")
        .body(Body::wrap_stream(stream))
        .unwrap())
}
```

**优势：**
- ✅ 内存占用恒定（O(1)）
- ✅ 支持 TB 级文件
- ✅ 支持断点续传
- ✅ 实时开始传输（无需等待完整文件）

### 4. 智能缓存策略（P4 + 改进）

```rust
pub struct SmartCache {
    // 访问模式分析
    analyzer: AccessPatternAnalyzer,

    // 预测模型
    predictor: FilePredictor,
}

impl SmartCache {
    /// 记录访问
    pub fn record_access(&self, path: &str) {
        self.analyzer.record(path);
    }

    /// 预测下一个需要的文件
    pub fn predict_next(&self, current: &str) -> Vec<String> {
        self.predictor.predict_next(current)
    }

    /// 预取相关文件
    pub async fn prefetch(&self, path: &str, rev: u64) {
        let related = self.predict_next(path);

        for file in related {
            // 后台预取（不阻塞）
            let _ = self.storage.get(&file, rev).await;
        }
    }
}

/// 访问模式分析器
pub struct AccessPatternAnalyzer {
    // 文件 → 访问时间序列
    access_history: RwLock<HashMap<String, VecDeque<DateTime>>>,

    // 文件关联度（文件 A → 文件 B）
    correlations: RwLock<HashMap<String, Vec<(String, f32)>>>,
}

impl AccessPatternAnalyzer {
    /// 分析模式
    pub fn analyze(&self) -> CorrelationMatrix {
        // 实现 Apriori 算法或协同过滤
        // 找出经常一起访问的文件
        todo!()
    }
}
```

**缓存策略：**
```
L1: 内存热缓存
  - 最近 1 小时访问的文件
  - 容量：1-10GB
  - 淘汰：LRU

L2: SSD 温缓存
  - 最近 7 天访问的文件
  - 容量：100GB-1TB
  - 淘汰：LFU

L3: 对象存储
  - 所有数据
  - 分层：热 / 温 / 冷
  - 自动迁移
```

### 5. 并行事务管理（P4 风格）

```rust
pub struct TransactionManager {
    // 活跃事务（并发访问）
    transactions: DashMap<TransactionId, PendingTransaction>,

    // 提交锁（串行化提交）
    commit_lock: Mutex<()>,

    // 文件锁
    file_locks: RwLock<HashMap<String, LockOwner>>,
}

impl TransactionManager {
    /// 开始事务（并发）
    pub fn begin(&self) -> TransactionId {
        let id = TransactionId::new();
        self.transactions.insert(id, PendingTransaction::new());
        id
    }

    /// 添加变更（并发）
    pub async fn add_change(
        &self,
        txn_id: TransactionId,
        path: String,
        change: Change,
    ) -> Result<()> {
        let mut txn = self.transactions.get_mut(&txn_id)
            .ok_or(Error::NotFound)?;

        txn.changes.push((path, change));
        Ok(())
    }

    /// 提交事务（串行）
    pub async fn commit(&self, txn_id: TransactionId) -> Result<u64> {
        // 获取全局提交锁
        let _guard = self.commit_lock.lock().await;

        // 移除事务
        let txn = self.transactions.remove(&txn_id)
            .ok_or(Error::NotFound)?;

        // 验证文件锁
        self.validate_locks(&txn).await?;

        // 应用变更
        let new_rev = self.apply_changes(txn).await?;

        // 释放锁
        self.release_locks().await;

        Ok(new_rev)
    }
}

/// 挂起的事务
pub struct PendingTransaction {
    id: TransactionId,
    author: String,
    message: String,
    changes: Vec<(String, Change)>,
    timestamp: i64,
}

/// 变更类型
pub enum Change {
    Add { content: Vec<u8> },
    Modify { delta: Vec<u8> },
    Delete,
    Copy { from: String },
    Move { from: String },
}
```

## 存储架构

### 对象存储层

```rust
pub trait ObjectStore: Send + Sync {
    /// 读取对象
    async fn get(&self, id: ObjectId) -> Result<Bytes>;

    /// 写入对象
    async fn put(&self, data: Bytes) -> Result<ObjectId>;

    /// 分块读取（流式）
    async fn read_chunk(
        &self,
        id: ObjectId,
        offset: u64,
        size: u64,
    ) -> Result<Bytes>;

    /// 检查存在
    async fn exists(&self, id: ObjectId) -> Result<bool>;

    /// 批量读取
    async fn get_batch(&self, ids: Vec<ObjectId>) -> Result<Vec<Option<Bytes>>>;
}
```

### 分层存储

```rust
pub struct TieredStore {
    // 热存储（Fjall LSM-tree）
    hot: Arc<HotStore>,

    // 温存储（Packfiles）
    warm: Arc<WarmStore>,

    // 冷存储（归档）
    cold: Arc<ColdStore>,

    // 迁移策略
    policy: Arc<MigractionPolicy>,
}

impl TieredStore {
    /// 读取对象（自动路由到正确层级）
    pub async fn get(&self, id: ObjectId) -> Result<Bytes> {
        // 1. 尝试热存储
        if let Ok(data) = self.hot.get(id).await {
            return Ok(data);
        }

        // 2. 尝试温存储
        if let Ok(data) = self.warm.get(id).await {
            // 提升到热存储
            self.hot.put(data.clone()).await?;
            return Ok(data);
        }

        // 3. 尝试冷存储
        if let Ok(data) = self.cold.get(id).await {
            // 提升到温存储
            self.warm.put(data.clone()).await?;
            return Ok(data);
        }

        Err(Error::NotFound(id))
    }

    /// 写入对象（总是写入热存储）
    pub async fn put(&self, data: Bytes) -> Result<ObjectId> {
        self.hot.put(data).await
    }

    /// 后台迁移任务
    pub async fn run_migration(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;

            // 热 → 温
            self.demote_hot_to_warm().await;

            // 温 → 冷
            self.demote_warm_to_cold().await;
        }
    }
}
```

## 元数据索引

```rust
pub struct MetadataIndex {
    // Fjall LSM-tree 实例

    // 路径索引：path → [rev1, rev2, ...]
    path_index: fjall::Tree,

    // 版本索引：rev → (tree_id, [changed_paths])
    rev_index: fjall::Tree,

    // 作者索引：author → [rev1, rev2, ...]
    author_index: fjall::Tree,

    // 时间索引：timestamp → rev
    time_index: fjall::Tree,
}

impl MetadataIndex {
    /// 查询文件历史
    pub async fn get_file_history(&self, path: &str) -> Result<Vec<u64>> {
        let key = format!("path:{}", path);
        let data = self.path_index.get(&key)?;
        Ok(bincode::deserialize(&data)?)
    }

    /// 查询时间范围内的提交
    pub async fn get_commits_in_range(
        &self,
        start: i64,
        end: i64,
    ) -> Result<Vec<Commit>> {
        let mut results = Vec::new();

        // 范围扫描
        for entry in self.time_index.range(start..=end) {
            let rev: u64 = bincode::deserialize(&entry.value)?;
            let commit = self.load_commit(rev).await?;
            results.push(commit);
        }

        Ok(results)
    }

    /// 查询作者的所有提交
    pub async fn get_commits_by_author(&self, author: &str) -> Result<Vec<Commit>> {
        let key = format!("author:{}", author);
        let data = self.author_index.get(&key)?;
        let revs: Vec<u64> = bincode::deserialize(&data)?;

        let mut commits = Vec::new();
        for rev in revs {
            commits.push(self.load_commit(rev).await?);
        }

        Ok(commits)
    }
}
```

## 协议层

### WebDAV 方法实现

```rust
pub struct WebDavHandler {
    storage: Arc<TieredStore>,
    transactions: Arc<TransactionManager>,
    metadata: Arc<MetadataIndex>,
    cache: Arc<SmartCache>,
}

impl WebDavHandler {
    /// PROPFIND: 获取属性
    pub async fn propfind(&self, req: Request) -> Result<Response> {
        let depth = parse_depth(req.headers())?;
        let path = parse_path(req.uri())?;

        // 构建响应
        let multistatus = self.build_propfind_response(path, depth).await?;

        Ok(Response::builder()
            .status(207) // Multi-Status
            .header("Content-Type", "text/xml; charset=utf-8")
            .body(multistatus.to_xml())
            .unwrap())
    }

    /// REPORT: 查询操作
    pub async fn report(&self, req: Request) -> Result<Response> {
        let body = read_body(req).await?;
        let report_type = parse_report_type(&body)?;

        match report_type {
            ReportType::Log => self.handle_log_report(body).await,
            ReportType::Update => self.handle_update_report(body).await,
            ReportType::Diff => self.handle_diff_report(body).await,
            ReportType::FileRevs => self.handle_file_revs(body).await,
        }
    }

    /// MERGE: 提交变更
    pub async fn merge(&self, req: Request) -> Result<Response> {
        // 1. 解析事务
        let txn_id = parse_transaction_id(req.uri())?;

        // 2. 提交事务（串行化）
        let new_rev = self.transactions.commit(txn_id).await?;

        // 3. 构建响应
        let response = build_merge_response(new_rev)?;

        Ok(Response::builder()
            .status(200)
            .header("Content-Type", "text/xml; charset=utf-8")
            .body(response)
            .unwrap())
    }
}
```

## 性能优化

### 1. 连接复用

```rust
pub struct ConnectionPool {
    connections: Arc<Mutex<Vec<UpstreamConnection>>>,
    max_size: usize,
}

impl ConnectionPool {
    pub async fn get(&self) -> Result<PooledConnection> {
        // 实现连接池
        // 复用 HTTP/2 连接
    }
}
```

### 2. 批量操作

```rust
impl TieredStore {
    /// 批量读取
    pub async fn get_batch(&self, ids: Vec<ObjectId>) -> Result<Vec<Option<Bytes>>> {
        // 并发读取
        let futures: Vec<_> = ids.into_iter()
            .map(|id| self.get(id))
            .collect();

        let results = futures::future::join_all(futures).await;

        Ok(results.into_iter().map(|r| r.ok()).collect())
    }

    /// 批量写入
    pub async fn put_batch(&self, data: Vec<Bytes>) -> Result<Vec<ObjectId>> {
        // 批量写入热存储
        let ids = self.hot.put_batch(data).await?;
        Ok(ids)
    }
}
```

### 3. 预取和预测

```rust
pub struct Prefetcher {
    pattern: Arc<AccessPatternAnalyzer>,
    storage: Arc<TieredStore>,
}

impl Prefetcher {
    /// 预取相关文件
    pub async fn prefetch_related(&self, path: &str, rev: u64) {
        let related = self.pattern.predict_next(path);

        // 并发预取
        let futures: Vec<_> = related.into_iter()
            .map(|file| self.storage.get(&file, rev))
            .collect();

        futures::future::join_all(futures).await;
    }
}
```

## 监控和可观测性

```rust
use prometheus::{Counter, Histogram, Gauge};

pub struct Metrics {
    // 请求计数
    pub requests_total: Counter,

    // 请求延迟
    pub request_duration: Histogram,

    // 缓存命中率
    pub cache_hits: Counter,
    pub cache_misses: Counter,

    // 存储统计
    pub hot_objects: Gauge,
    pub warm_objects: Gauge,
    pub cold_objects: Gauge,

    // 事务统计
    pub active_transactions: Gauge,
    pub commits_total: Counter,
}

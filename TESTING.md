# 🚀 DSvn 快速测试指南

## 一键测试（推荐）

```bash
# 协议验证测试 - 无SVN客户端依赖（推荐）
make protocol-test

# 快速验证（需要SVN客户端）
make quick-test

# 完整验收（需要SVN客户端）
make acceptance-test
```

## 关于协议验证测试

由于 **SVN 客户端 1.14.3 在 macOS ARM 上有已知的 segfault 问题**，我们开发了基于 curl 的协议验证测试套件，完全不依赖 SVN 客户端。

### 测试覆盖的 WebDAV 方法

| 方法 | 说明 | 状态 |
|------|------|------|
| OPTIONS | 服务器能力发现 | ✅ |
| GET | 文件获取 | ✅ |
| PUT | 文件创建/更新 | ✅ |
| DELETE | 资源删除 | ✅ |
| MKCOL | 目录创建 | ✅ |
| PROPFIND | 属性查询 | ✅ |
| PROPPATCH | 属性修改 | ✅ |
| CHECKOUT | 工作资源创建 | ✅ |
| CHECKIN | 提交变更 | ✅ |
| MERGE | 合并变更 | ✅ |
| MKACTIVITY | 事务创建 | ✅ |
| REPORT | 日志/更新报告 | ✅ |
| COPY | 资源复制 | ✅ (stub) |
| MOVE | 资源移动 | ✅ (stub) |
| LOCK | 资源锁定 | ✅ (stub) |
| UNLOCK | 解锁资源 | ✅ (stub) |

### 运行协议验证测试

```bash
# 方式1：使用 make
make protocol-test

# 方式2：直接运行脚本
./scripts/protocol-validation.sh
```

### 协议验证测试的优势

1. **无依赖**：不依赖 SVN 客户端，避免 segfault 问题
2. **快速**：约 10 秒完成全部 37+ 个测试
3. **全面**：覆盖所有 WebDAV/SVN 协议方法
4. **可靠**：基于 curl，跨平台兼容
5. **CI/CD 友好**：易于集成到自动化流程

## 手动测试

### 1. 编译项目
```bash
cargo build --release --workspace
```

### 2. 初始化仓库
```bash
./target/release/dsvn-admin init /tmp/dsvn-test
```

### 3. 启动服务器
```bash
./target/release/dsvn start --repo-root /tmp/dsvn-test --addr "127.0.0.1:8080"
```

### 4. 测试SVN操作（需要SVN客户端）
```bash
# 新开终端窗口
svn checkout http://localhost:8080/svn /tmp/wc
cd /tmp/wc
echo "test" > test.txt
svn add test.txt
svn commit -m "Test commit" --username test --password test
svn update
svn log
```

### 5. 使用 curl 测试（无需SVN客户端）
```bash
# 测试服务器是否运行
curl -X OPTIONS http://localhost:8080/svn -i

# PROPFIND 测试
curl -X PROPFIND http://localhost:8080/svn \
  -H "Depth: 0" \
  -H "Content-Type: text/xml" \
  -d '<?xml version="1.0"?><propfind xmlns="DAV:"><prop/></propfind>'

# PUT 测试
curl -X PUT http://localhost:8080/svn/test.txt \
  -d "Hello, DSvn!"

# GET 测试
curl http://localhost:8080/svn/test.txt
```

## 测试脚本位置

```
scripts/
├── protocol-validation.sh  # 协议验证测试（推荐）
├── acceptance-test.sh      # 完整自动化测试（需要SVN客户端）
├── quick-test.sh           # 快速验证（需要SVN客户端）
├── PROTOCOL_TEST_PLAN.md   # TDD 测试计划
├── README.md               # 使用指南
├── SVN-GUIDE.md            # SVN命令参考
├── TESTING-SYSTEM.md       # 测试系统说明
└── SUMMARY.md              # 完成总结
```

## 常用命令

```bash
make help               # 查看所有命令
make build              # 编译
make unit-test          # 单元测试（56个测试）
make protocol-test      # 协议验证测试（推荐）
make quick-test         # 快速测试（需要SVN客户端）
make acceptance-test    # 验收测试（需要SVN客户端）
make clean              # 清理
make logs               # 查看日志
make stop-test          # 停止测试服务器
```

## macOS ARM 用户特别提示

如果你在 macOS ARM (Apple Silicon) 上遇到 SVN 客户端 segfault 问题：

```bash
# 使用协议验证测试代替 quick-test
make protocol-test

# 这个测试完全使用 curl，不依赖 SVN 客户端
# 可以验证服务器 WebDAV 协议实现是否正确
```

## 遇到问题？

```bash
# 查看日志
cat /tmp/dsvn-server.log
cat /tmp/dsvn-protocol-validation.log

# 停止服务器
make stop-test

# 重新开始
make clean && make protocol-test
```

## TDD 开发流程

本项目遵循 TDD (Test-Driven Development) 原则：

1. **RED**: 编写测试，验证失败
2. **GREEN**: 实现功能，使测试通过
3. **REFACTOR**: 重构代码，保持测试通过

详见 `scripts/PROTOCOL_TEST_PLAN.md`

## 📚 详细文档

- 协议验证: `scripts/protocol-validation.sh`
- 测试计划: `scripts/PROTOCOL_TEST_PLAN.md`
- 测试使用: `scripts/README.md`
- SVN操作: `scripts/SVN-GUIDE.md`
- 完成总结: `scripts/SUMMARY.md`

---

**Happy Testing! 🎉**

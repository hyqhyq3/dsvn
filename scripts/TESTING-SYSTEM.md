# DSvn 验收测试系统

完整的自动化测试框架，用于验证DSvn服务器的功能和性能。

## 🚀 快速开始

```bash
# 最快的方式 - 快速测试（30秒）
make quick-test

# 或者直接运行脚本
./scripts/quick-test.sh
```

## 📁 文件结构

```
scripts/
├── acceptance-test.sh    # 完整验收测试（170+行）
├── quick-test.sh         # 快速测试（70行）
├── README.md             # 测试脚本使用指南
├── SVN-GUIDE.md          # SVN客户端操作指南
└── TESTING-SYSTEM.md     # 本文件
```

## 🎯 测试类型

### 1. 快速测试 (Quick Test)
**文件**: `quick-test.sh`
**时间**: ~30秒
**用途**: 日常开发验证

**测试内容**:
- ✅ 编译检查
- ✅ 仓库初始化
- ✅ 服务器启动
- ✅ 基础SVN操作
- ✅ 简单的文件/目录操作

**使用场景**:
- 代码修改后的快速验证
- 功能开发的中间检查
- 持续开发的频繁测试

### 2. 完整验收测试 (Acceptance Test)
**文件**: `acceptance-test.sh`
**时间**: ~2-3分钟
**用途**: 发布前的全面验证

**测试内容**:
- ✅ 依赖环境检查
- ✅ 完整的SVN客户端兼容性测试
- ✅ 所有WebDAV方法测试
- ✅ 并发操作测试
- ✅ 性能基准测试
- ✅ 自动生成测试报告

**使用场景**:
- 版本发布前的验收
- 重大功能变更后
- CI/CD流水线集成
- 性能回归测试

## 🛠️ 使用方法

### 方法1: Makefile（推荐）

```bash
# 查看所有命令
make help

# 快速测试
make quick-test

# 完整验收测试
make acceptance-test

# 开发流程
make dev

# 生产就绪检查
make production-ready
```

### 方法2: 直接运行

```bash
# 赋予执行权限（首次）
chmod +x scripts/*.sh

# 运行快速测试
./scripts/quick-test.sh

# 运行完整验收测试
./scripts/acceptance-test.sh
```

## 📊 测试覆盖

### WebDAV方法覆盖

| 方法 | 功能 | 状态 |
|-----|------|------|
| PROPFIND | 目录列表 | ✅ |
| REPORT | 日志/更新报告 | ✅ |
| MERGE | 提交变更 | ✅ |
| GET | 读取文件 | ✅ |
| PUT | 创建/更新文件 | ✅ |
| MKCOL | 创建目录 | ✅ |
| DELETE | 删除文件/目录 | ✅ |
| CHECKOUT | 检出资源 | ✅ |
| CHECKIN | 检入变更 | ✅ |
| MKACTIVITY | 创建事务 | ✅ |
| LOCK/UNLOCK | 锁定操作 | ✅ |
| COPY/MOVE | 复制/移动 | ✅ |

### SVN命令覆盖

**基础命令**:
- `svn checkout` - 检出仓库
- `svn add` - 添加文件
- `svn commit` - 提交变更
- `svn update` - 更新工作副本
- `svn status` - 查看状态
- `svn log` - 查看日志
- `svn info` - 查看信息

**高级命令**:
- `svn mkdir` - 创建目录
- `svn mv` - 移动文件/目录
- `svn cp` - 复制文件/目录
- `svn rm` - 删除文件/目录
- `svn diff` - 查看差异
- `svn ls` - 列出文件
- `svn export` - 导出副本
- `svn propset` - 设置属性

## 🧪 测试场景

### 场景1: 日常开发流程
```bash
# 1. 修改代码
vim dsvn-webdav/src/handlers.rs

# 2. 快速验证
make quick-test

# 3. 如果通过，继续开发
# 4. 如果失败，查看日志
make logs
```

### 场景2: 提交前验证
```bash
# 1. 完整测试
make acceptance-test

# 2. 代码质量检查
make clippy

# 3. 格式化
make fmt

# 4. 提交
git add .
git commit -m "Feature: Implement new WebDAV methods"
```

### 场景3: 发布前检查
```bash
# 1. 生产就绪检查
make production-ready

# 2. 查看测试报告
cat /tmp/dsvn-test-report.md

# 3. 手动验证关键功能
# （服务器仍在运行）
svn checkout http://localhost:8080/svn /tmp/verify-wc
# ... 手动测试 ...
```

## 📈 性能基准

测试脚本会测量以下性能指标：

| 操作 | 目标 | 测量方法 |
|------|------|---------|
| Checkout (10文件) | < 5秒 | `time svn checkout` |
| Commit (100文件) | < 30秒 | 批量提交计时 |
| Update | < 2秒 | `time svn update` |
| 服务器启动 | < 3秒 | 启动计时 |
| 内存占用 | < 50MB | `ps -o rss` |

## 🔄 自动化程度

### 完全自动化的环节
- ✅ 编译检查
- ✅ 依赖验证
- ✅ 仓库初始化
- ✅ 服务器启动
- ✅ 测试数据准备
- ✅ SVN命令执行
- ✅ 结果验证
- ✅ 报告生成
- ✅ 环境清理

### 手动环节
- ⏳ 服务器停止（测试后保持运行）
- ⏳ 报告审查（人工阅读）

## 📝 输出文件

### 快速测试输出
```
/tmp/dsvn-quick.log      - 服务器日志
/tmp/dsvn-quick-test/    - 仓库路径
/tmp/dsvn-quick-wc/      - 工作副本
```

### 完整测试输出
```
/tmp/dsvn-test-repo/     - 仓库路径
/tmp/dsvn-wc/            - 主工作副本
/tmp/dsvn-test-data/     - 测试数据
/tmp/dsvn-server.log     - 服务器日志
/tmp/dsvn-test-report.md - 测试报告
/tmp/dsvn-export/        - 导出的干净副本
```

## 🐛 故障排除

### 常见问题

**Q: 端口被占用**
```bash
make stop-test
# 或
lsof -ti:8080 | xargs kill -9
```

**Q: 测试失败**
```bash
# 查看详细日志
cat /tmp/dsvn-server.log

# 重新运行
make clean
make acceptance-test
```

**Q: SVN命令找不到**
```bash
# macOS
brew install subversion

# Ubuntu/Debian
sudo apt-get install subversion

# Fedora/RHEL
sudo dnf install subversion
```

## 📊 测试报告示例

完整验收测试会生成Markdown格式的报告：

```markdown
# DSvn Acceptance Test Report

**测试时间**: 2026-02-06 20:40:00
**测试主机**: mbp.local
**SVN版本**: svn, version 1.14.0

## 测试结果

### ✅ 通过的测试

1. **依赖检查**: 所有必需工具已安装
2. **项目编译**: Release版本编译成功
...
```

## 🎓 学习资源

- **测试脚本**: `scripts/acceptance-test.sh` - 学习完整测试流程
- **快速测试**: `scripts/quick-test.sh` - 学习基础测试
- **使用指南**: `scripts/README.md` - 详细的脚本使用说明
- **SVN指南**: `scripts/SVN-GUIDE.md` - SVN客户端操作参考

## 🔧 自定义测试

### 修改端口
编辑脚本中的端口变量：
```bash
# quick-test.sh
PORT=9999

# acceptance-test.sh
SERVER_PORT=9999
```

### 添加新测试
在相应脚本的`main()`函数中添加：
```bash
# 添加新的测试函数
test_my_feature() {
    log_section "测试我的新功能"
    # ... 测试代码 ...
}

# 在main()中调用
main() {
    # ... 现有测试 ...
    test_my_feature
}
```

### 修改测试数据
编辑`prepare_test_data()`函数。

## 📞 获取帮助

```bash
# 查看Makefile命令
make help

# 查看脚本使用说明
cat scripts/README.md

# 查看SVN操作指南
cat scripts/SVN-GUIDE.md
```

## 🎯 下一步

测试通过后，你可以：

1. **继续开发**: 修改代码，再次运行`make quick-test`
2. **查看报告**: 阅读生成的测试报告
3. **手动测试**: 服务器仍在运行，可以手动操作
4. **停止测试**: 运行`make stop-test`
5. **提交代码**: 确保所有测试通过后提交

## 📋 检查清单

在提交代码前，确保：

- [ ] `make quick-test` 通过
- [ ] `make acceptance-test` 通过
- [ ] `make clippy` 无警告
- [ ] `make fmt` 代码已格式化
- [ ] 查看测试报告无严重问题

## 🌟 最佳实践

1. **频繁测试**: 每次修改后运行`make quick-test`
2. **完整测试**: 提交前运行`make acceptance-test`
3. **查看日志**: 失败时检查`/tmp/dsvn-server.log`
4. **清理环境**: 使用`make clean`清理旧的测试数据
5. **阅读报告**: 仔细阅读生成的测试报告

---

**Happy Testing! 🎉**

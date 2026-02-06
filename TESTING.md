# 🚀 DSvn 快速测试指南

## 一键测试（推荐）

```bash
# 快速验证（30秒）
make quick-test

# 完整验收（2-3分钟）
make acceptance-test
```

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

### 4. 测试SVN操作
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

## 测试脚本位置

```
scripts/
├── acceptance-test.sh    # 完整自动化测试
├── quick-test.sh         # 快速验证
├── README.md             # 使用指南
├── SVN-GUIDE.md          # SVN命令参考
├── TESTING-SYSTEM.md     # 测试系统说明
└── SUMMARY.md            # 完成总结
```

## 常用命令

```bash
make help               # 查看所有命令
make build              # 编译
make quick-test         # 快速测试
make acceptance-test    # 验收测试
make clean              # 清理
make logs               # 查看日志
make stop-test          # 停止测试服务器
```

## 遇到问题？

```bash
# 查看日志
cat /tmp/dsvn-server.log

# 停止服务器
make stop-test

# 重新开始
make clean && make quick-test
```

## 📚 详细文档

- 测试使用: `scripts/README.md`
- SVN操作: `scripts/SVN-GUIDE.md`
- 完成总结: `scripts/SUMMARY.md`

---

**Happy Testing! 🎉**

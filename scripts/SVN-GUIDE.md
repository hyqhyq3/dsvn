# SVN 客户端配置示例
# 用于测试 DSvn 服务器

## 配置说明

DSvn服务器默认不需要认证，但SVN客户端可能要求提供凭据。
测试脚本使用 `--username test --password test` 作为占位符。

## 服务器配置

### 1. HTTP WebDAV 访问

```bash
# 服务器地址
export SVN_REPO_URL="http://localhost:8080/svn"

# 或者使用其他端口
export SVN_REPO_URL="http://localhost:8989/svn"
```

### 2. 禁用证书验证（HTTPS测试时）

```bash
# 在 ~/.subversion/servers 中添加
[global]
http-auth-types = basic;digest
ssl-trust-default-ca = false
store-plaintext-passwords = yes

[/localhost]
ssl-authority-files = /path/to/cert
```

## 测试命令示例

### 基础操作

```bash
# 1. 检出仓库
svn checkout http://localhost:8080/svn /tmp/my-wc

# 2. 查看状态
svn status /tmp/my-wc

# 3. 添加文件
echo "test" > /tmp/my-wc/test.txt
svn add /tmp/my-wc/test.txt

# 4. 提交
svn commit /tmp/my-wc/test.txt -m "Add test file" \
  --username test --password test

# 5. 更新
svn update /tmp/my-wc

# 6. 查看日志
svn log /tmp/my-wc

# 7. 查看信息
svn info /tmp/my-wc
```

### 高级操作

```bash
# 1. 创建目录
svn mkdir /tmp/my-wc/src -m "Create src directory"

# 2. 移动文件
svn mv /tmp/my-wc/test.txt /tmp/my-wc/src/test.txt
svn commit /tmp/my-wc -m "Move test to src/"

# 3. 复制文件
svn cp /tmp/my-wc/src/test.txt /tmp/my-wc/src/test2.txt
svn commit /tmp/my-wc -m "Copy test to test2"

# 4. 删除文件
svn rm /tmp/my-wc/src/test2.txt
svn commit /tmp/my-wc -m "Remove test2"

# 5. 查看差异
svn diff /tmp/my-wc

# 6. 查看文件列表
svn ls -R /tmp/my-wc

# 7. 导出（不含.svn）
svn export http://localhost:8080/svn /tmp/export-clean

# 8. 设置属性
svn propset svn:executable "*" /tmp/my-wc/script.sh
svn commit /tmp/my-wc -m "Make script executable"
```

### 批量操作

```bash
# 批量添加文件
find /tmp/my-wc -type f | xargs svn add

# 批量提交
svn commit /tmp/my-wc -m "Batch commit"

# 批量更新
svn update /tmp/my-wc --accept theirs-full
```

## 性能测试

### 测试大文件
```bash
# 创建10MB文件
dd if=/dev/zero of=/tmp/my-wc/large.bin bs=1M count=10
svn add /tmp/my-wc/large.bin
time svn commit /tmp/my-wc/large.bin -m "Add large file"
```

### 测试大量小文件
```bash
# 创建100个小文件
for i in {1..100}; do
  echo "File $i" > /tmp/my-wc/file$i.txt
done

svn add /tmp/my-wc/file*.txt
time svn commit /tmp/my-wc -m "Add 100 files"
```

### 并发测试
```bash
# 3个并发工作副本
for i in 1 2 3; do
  (
    svn checkout http://localhost:8080/svn /tmp/concurrent-wc-$i
    echo "Change $i" > /tmp/concurrent-wc-$i/change$i.txt
    svn add /tmp/concurrent-wc-$i/change$i.txt
    svn commit /tmp/concurrent-wc-$i -m "Concurrent commit $i"
  ) &
done
wait

# 验证所有提交
for i in 1 2 3; do
  svn update /tmp/concurrent-wc-$i
done
```

## 故障排除

### 问题: 检出失败
```bash
# 查看详细错误
svn checkout http://localhost:8080/svn /tmp/wc -v

# 检查服务器是否运行
curl -I http://localhost:8080/svn

# 检查端口占用
lsof -i:8080
```

### 问题: 提交失败
```bash
# 检查工作副本状态
svn status /tmp/wc

# 查看详细错误
svn commit /tmp/wc -m "Test" -v

# 清理状态
svn cleanup /tmp/wc
```

### 问题: 连接超时
```bash
# 增加超时时间
svn checkout http://localhost:8080/svn /tmp/wc --config-option config:general:http-timeout=600

# 检查防火墙
telnet localhost 8080
```

### 问题: 权限错误
```bash
# 使用测试用户（即使不需要认证）
svn commit -m "Test" --username testuser --password testpass

# 检查文件权限
ls -la /tmp/wc
```

## IDE 集成

### IntelliJ IDEA
```
Settings → Version Control → Subversion
General → Network: HTTP Timeout: 60000
```

### VSCode
安装 SVN 扩展，配置：
```json
{
  "svn.username": "test",
  "svn.password": "test"
}
```

### Eclipse
```
Team → SVN → Properties
General → Connection Timeout: 60000
```

## 调试技巧

### 启用详细日志
```bash
# SVN 客户端日志
export SVN_DEBUG_HTTP="true"
svn checkout http://localhost:8080/svn /tmp/wc

# 服务器日志
tail -f /tmp/dsvn-server.log
```

### 捕获HTTP请求
```bash
# 使用tcpdump
sudo tcpdump -i lo -w capture.pcap port 8080

# 使用Wireshark分析
wireshark capture.pcap
```

### 测试特定方法
```bash
# 只测试PROPFIND
curl -X PROPFIND http://localhost:8080/svn

# 只测试REPORT
svn log http://localhost:8080/svn -v

# 只测试MERGE
echo "test" > test.txt
svn import test.txt http://localhost:8080/svn/test.txt -m "Test"
```

## 性能监控

### 测量Checkout时间
```bash
time svn checkout http://localhost:8080/svn /tmp/perf-wc
```

### 测量Commit时间
```bash
# 创建测试文件
echo "test" > /tmp/wc/test.txt
svn add /tmp/wc/test.txt

# 测量提交时间
time svn commit /tmp/wc/test.txt -m "Test"
```

### 测量Update时间
```bash
# 先创建新修订
svn mkdir /tmp/wc/newdir -m "Test"

# 测量更新时间
time svn update /tmp/wc
```

## 最佳实践

1. **使用绝对路径**进行操作以避免混淆
2. **定期cleanup**工作副本以维护一致性
3. **使用--quiet**减少输出在脚本中
4. **检查返回值**确保命令成功
5. **测试前清理**旧的测试数据

```bash
# 好的做法
rm -rf /tmp/test-wc
svn checkout http://localhost:8080/svn /tmp/test-wc
cd /tmp/test-wc

# 完成后清理
svn cleanup /tmp/test-wc
```

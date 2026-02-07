# DSvn WebDAV 协议兼容性测试

这个测试文件确保 DSvn 的 WebDAV 实现与 SVN 客户端保持兼容性。

## 测试文件

- `protocol_compatibility_test.rs` - 协议兼容性单元测试

## 测试覆盖的协议要求

### 1. OPTIONS 响应测试

验证 OPTIONS 请求返回正确的头部：

#### DAV Header
- 必须包含 WebDAV 基础类: `1`, `2`
- 必须包含 DeltaV 能力: `version-controlled-configuration`
- 必须包含 SVN 特定能力:
  - `http://subversion.tigris.org/xmlns/dav/svn/depth`
  - `http://subversion.tigris.org/xmlns/dav/svn/mergeinfo`
  - `http://subversion.tigris.org/xmlns/dav/svn/log-revprops`

#### SVN Header
- 格式: `SVN: 1, 2`
- 表示服务器支持的 SVN 版本

#### SVN-Youngest-Revision Header
- 格式: 非负整数
- 表示仓库最新的版本号

#### Allow Header
- 必须包含所有支持的 WebDAV 方法:
  - `OPTIONS`, `PROPFIND`, `PROPPATCH`, `REPORT`, `MERGE`
  - `CHECKOUT`, `CHECKIN`, `MKACTIVITY`, `MKCOL`
  - `LOCK`, `UNLOCK`, `COPY`, `MOVE`
  - 标准 HTTP 方法: `GET`, `PUT`, `DELETE`, `HEAD`, `POST`

### 2. PROPFIND 响应测试

验证 PROPFIND 请求返回正确的 XML 格式：

#### Status 元素格式（回归测试）
- **正确格式**: `<status>200 OK</status>`
- **错误格式**（已修复）: `<status>HTTP/1.1 200 OK</status>`
- SVN 客户端期望不包含 `HTTP/1.1` 前缀的状态码

#### 必需的 SVN 属性
- `resourcetype` - 资源类型（目录为 `<collection/>`）
- `version-controlled-configuration` - VCC 链接
- `svn:baseline-relative-path` - 相对于基线的路径
- `svn:repository-uuid` - 仓库 UUID

#### baseline-relative-path 格式（回归测试）
- **正确格式**: 相对路径，不包含 `/svn/` 前缀
  - 根路径: 空字符串
  - 子目录: `trunk/` 或 `branches/feature/`
- **错误格式**（已修复）: 包含 `/svn/` 前缀如 `/svn/trunk/`

#### XML 结构
- 必须包含 XML 声明: `<?xml version="1.0" encoding="utf-8"?>`
- 根元素: `<multistatus>`
- 命名空间声明:
  - DAV: `xmlns="DAV:"`
  - SVN: `xmlns:svn="http://subversion.tigris.org/xmlns/dav/"`
- 状态码: 207 Multi-Status
- Content-Type: `text/xml; charset=utf-8`

## 运行测试

```bash
# 运行所有协议兼容性测试
cargo test -p dsvn-webdav --test protocol_compatibility_test

# 运行特定模块的测试
cargo test -p dsvn-webdav --test protocol_compatibility_test options_tests
cargo test -p dsvn-webdav --test protocol_compatibility_test propfind_tests
cargo test -p dsvn-webdav --test protocol_compatibility_test regression_tests
```

## 回归测试

这些测试确保已修复的问题不会在未来被重新引入：

### test_no_http_prefix_in_status_element
验证 PROPFIND 响应中的 `<status>` 元素不包含 `HTTP/1.1` 前缀。

### test_baseline_relative_path_no_svn_prefix  
验证 `baseline-relative-path` 属性不包含 `/svn/` 前缀。

## 添加新测试

当修复协议兼容性问题时，请添加相应的回归测试：

```rust
#[test]
fn test_new_protocol_requirement() {
    // 描述测试目的
    let sample_response = "...";
    
    // 验证正确的行为
    assert!(sample_response.contains("expected-value"));
    
    // 验证回归不会再次发生
    assert!(!sample_response.contains("incorrect-value"));
}
```

## 参考

- [RFC 2518 - HTTP Extensions for Distributed Authoring (WebDAV)](https://tools.ietf.org/html/rfc2518)
- [RFC 3253 - Versioning Extensions to WebDAV (DeltaV)](https://tools.ietf.org/html/rfc3253)
- [SVN HTTP Protocol Documentation](https://svn.apache.org/repos/asf/subversion/trunk/notes/http-and-webdav/)

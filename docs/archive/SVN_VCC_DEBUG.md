# SVN VCC 问题对比分析

## SVN 客户端请求

```xml
<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:">
  <prop>
    <version-controlled-configuration xmlns="DAV:"/>
    <resourcetype xmlns="DAV:"/>
    <baseline-relative-path xmlns="http://subversion.tigris.org/xmlns/dav/"/>
    <repository-uuid xmlns="http://subversion.tigris.org/xmlns/dav/"/>
  </prop>
</propfind>
```

**关键发现**：
1. 所有属性都使用 `xmlns="DAV:"` 或其他命名空间
2. 每个属性都是**自闭合的空元素**（`/>`）
3. 这表示客户端期望：服务器返回这些属性的**值**
4. `version-controlled-configuration` 是一个属性，期望服务器返回它的值（href 字符串）

## DSvn 响应（修复后）

```xml
<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:" xmlns:svn="http://subversion.tigris.org/xmlns/dav/">
  <response>
    <href>/svn</href>
    <propstat>
      <prop>
        <resourcetype><collection/></resourcetype>
        <version-controlled-configuration xmlns="DAV:">/svn/!svn/vcc/default</version-controlled-configuration>
        <checked-in>
          <href>/svn/!svn/bln/0</href>
        </checked-in>
        <baseline-relative-path xmlns="http://subversion.tigris.org/xmlns/dav/"></baseline-relative-path>
        <repository-uuid xmlns="http://subversion.tigris.org/xmlns/dav/">ca906884-549f-4329-afd3-7fc6910dc292</repository-uuid>
        
      </prop>
      <status>200 OK</status>
    </propstat>
  </response>
</multistatus>
```

## 可能的问题

### 问题 1: 属性值格式不一致

**客户端请求的属性**：`version-controlled-configuration xmlns="DAV:"/>`
- 这是**属性请求**，使用 `xmlns="DAV:"`
- 是一个空元素（`/>`），期望服务器返回属性值

**DSvn 响应**：`<version-controlled-configuration xmlns="DAV:">/svn/!svn/vcc/default</version-controlled-configuration>`
- 使用了命名空间前缀 `xmlns="DAV:"`（带引号）
- 但值是一个**字符串内容**，不是属性

**可能问题**：
1. 客户端可能在解析时期望 `version-controlled-configuration` 是一个属性（`version-controlled-configuration="value"`）
2. 而不是带命名空间的元素（`<version-controlled-configuration xmlns="DAV:">value</version-controlled-configuration>`）

### 问题 2: 其他属性也使用了不同格式

查看响应中的其他属性：
- `<checked-in>` - 没有命名空间
- `<svn:baseline-relative-path>` - 使用了 `svn:` 命名空间
- `<svn:repository-uuid>` - 使用了 `svn:` 命名空间

这些格式不一致可能导致解析问题。

## 解决方案

需要将 `version-controlled-configuration` 改为属性格式，而不是元素格式。

### 正确的属性格式

**方案 A（推荐）：使用 DAV 命名空间属性**
```xml
<version-controlled-configuration xmlns="DAV:" xmlns:D="DAV:">/svn/!svn/vcc/default</version-controlled-configuration>
```

或者：
```xml
<version-controlled-configuration xmlns="DAV:" D:version-controlled-configuration="/svn/!svn/vcc/default"/>
```

### 方案 B：不带命名空间
```xml
<version-controlled-configuration>/svn/!svn/vcc/default</version-controlled-configuration>
```

## 参考

WebDAV DeltaV 规范 (RFC 3253)：
- 属性可以使用命名空间限定
- 也可以不带命名空间
- 关键是要与 PROPFIND 请求的格式一致

## 下一步

根据 SVN 协议规范修改响应格式，确保属性值正确返回。

# SVN VCC (Version Controlled Configuration) 问题分析

## 当前错误

```
svn: E175003: PROPFIND 响应中没有包含请求的 version-controlled-configuration 值
```

## 当前 DSvn 响应

```xml
<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:">
  <response xmlns="DAV:">
    <href>/svn</href>
    <propstat>
      <prop>
        <resourcetype xmlns="DAV:"><collection/></resourcetype>
        <version-controlled-configuration xmlns="DAV:">
          <D:href xmlns="DAV:">/svn/!svn/vcc/default</D:href>
        </version-controlled-configuration>
        <baseline-relative-path xmlns="http://subversion.tigris.org/xmlns/dav/"></baseline-relative-path>
        <repository-uuid xmlns="http://subversion.tigris.org/xmlns/dav/">a4bec8cb-3c25-473d-902f-4eb1dba3b5db</repository-uuid>
      </prop>
      <status>200 OK</status>
    </propstat>
  </response>
</multistatus>
```

## 可能的问题

### 1. 命名空间问题

**当前：**
```xml
<version-controlled-configuration xmlns="DAV:">
  <D:href xmlns="DAV:">/svn/!svn/vcc/default</D:href>
</version-controlled-configuration>
```

**可能正确：**
```xml
<version-controlled-configuration>
  <D:href xmlns="DAV:">/svn/!svn/vcc/default</D:href>
</version-controlled-configuration>
```

注意：
- `version-controlled-configuration` 元素应该在其父级定义的命名空间内
- 不应该在元素上重复定义命名空间

### 2. 版本控制配置值的格式

根据 WebDAV DeltaV 规范，`version-controlled-configuration` 属性的值应该是一个指向 VCC 资源的 href，而不是嵌套的 href 元素。

**当前（错误）：**
```xml
<version-controlled-configuration xmlns="DAV:">
  <D:href xmlns="DAV:">/svn/!svn/vcc/default</D:href>
</version-controlled-configuration>
```

**可能正确：**
```xml
<version-controlled-configuration>
  <D:href>/svn/!svn/vcc/default</D:href>
</version-controlled-configuration>
```

或者更简单：
```xml
<version-controlled-configuration>
  <D:href>/svn/!svn/vcc/default</D:href>
</version-controlled-configuration>
```

## SVN 客户端期望的格式

根据 SVN 协议文档，SVN 客户端期望的 PROPFIND 响应应该包含一个 `version-controlled-configuration` 属性，其值是一个指向 VCC 资源的 href。

SVN 客户端可能在解析时期望：
1. `version-controlled-configuration` 是一个简单的 href 字符串，而不是嵌套元素
2. 或者 `version-controlled-configuration` 有特定的 XML 结构

## 需要的修复

1. 修复 `version-controlled-configuration` 的 XML 结构
2. 确保命名空间正确
3. 测试验证

## 参考

- WebDAV DeltaV 规范: RFC 3253
- SVN 协议文档: http://svn.apache.org/repos/asf/subversion/trunk/notes/

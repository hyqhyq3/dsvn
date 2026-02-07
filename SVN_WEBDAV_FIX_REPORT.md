# DSvn WebDAV 协议修复报告

## 问题分析

SVN checkout 失败，错误信息：
```
E175003: PROPFIND 响应中没有包含请求的 version-controlled-configuration 值
```

## 根本原因

通过分析 Apache SVN 源码 (`mod_dav_svn/liveprops.c`)，发现问题在于：

1. **XML 命名空间处理错误**：原代码在 `version-controlled-configuration` 元素上使用了 `xmlns="DAV:"`，但内部的 `href` 元素也使用了 `D:` 前缀和命名空间声明，导致 SVN 客户端无法正确解析。

2. **缺少必需属性**：根据 SVN 源码分析，VCR (Version Controlled Resource) 需要返回以下属性：
   - `version-controlled-configuration` (DAV 命名空间)
   - `checked-in` (DAV 命名空间) - **原实现缺少**
   - `baseline-relative-path` (SVN 命名空间)
   - `repository-uuid` (SVN 命名空间)

3. **XML 格式不正确**：根据 SVN 源码中的 `dav_svn__build_uri` 函数，当 `add_href=TRUE` 时，VCC URI 应该已经包含了 `<D:href>` 包装。

## 修复内容

### 1. 修复 multistatus 根元素命名空间声明
```xml
<!-- 修复前 -->
<multistatus xmlns="DAV:">

<!-- 修复后 -->
<multistatus xmlns="DAV:" xmlns:svn="http://subversion.tigris.org/xmlns/dav/">
```

### 2. 修复 version-controlled-configuration 元素格式
```xml
<!-- 修复前 -->
<version-controlled-configuration xmlns="DAV:">
  <D:href xmlns="DAV:">/svn/!svn/vcc/default</D:href>
</version-controlled-configuration>

<!-- 修复后 -->
<version-controlled-configuration>
  <href>/svn/!svn/vcc/default</href>
</version-controlled-configuration>
```

### 3. 添加缺少的 checked-in 属性
```xml
<checked-in>
  <href>/svn/!svn/bln/{revision}</href>
</checked-in>
```

### 4. 修复 baseline-relative-path 和 repository-uuid 命名空间
```xml
<!-- 修复前 -->
<baseline-relative-path xmlns="http://subversion.tigris.org/xmlns/dav/">
<repository-uuid xmlns="http://subversion.tigris.org/xmlns/dav/">

<!-- 修复后 -->
<svn:baseline-relative-path>
<svn:repository-uuid>
```

## 代码变更

文件: `dsvn-webdav/src/handlers.rs`

### PROPFIND 响应的主要修改：

1. **根元素命名空间**：在 multistatus 根元素上统一声明 DAV 和 SVN 命名空间
2. **VCC 路径响应**：添加 `checked-in` 和 `version-controlled-configuration` 属性
3. **Baseline 响应**：添加 `version-name` 属性
4. **目录响应**：修正所有属性的命名空间和格式
5. **文件响应**：添加必需的 DAV 属性
6. **子目录/文件条目**：统一添加 `version-controlled-configuration` 和 `checked-in` 属性

## SVN 源码参考

关键文件：`subversion/mod_dav_svn/liveprops.c`

### version-controlled-configuration 处理 (第 610-618 行):
```c
case DAV_PROPID_version_controlled_configuration:
  /* only defined for VCRs */
  if (resource->type != DAV_RESOURCE_TYPE_REGULAR)
    return DAV_PROP_INSERT_NOTSUPP;
  value = dav_svn__build_uri(resource->info->repos, DAV_SVN__BUILD_URI_VCC,
                             SVN_IGNORED_REVNUM, NULL,
                             TRUE /* add_href */, scratch_pool);
  break;
```

### checked-in 处理 (第 559-609 行):
```c
case DAV_PROPID_checked_in:
  /* only defined for VCRs (in the public space and in a BC space) */
  if (resource->type == DAV_RESOURCE_TYPE_PRIVATE
      && (resource->info->restype == DAV_SVN_RESTYPE_VCC
          || resource->info->restype == DAV_SVN_RESTYPE_ME))
    {
      /* VCC 返回最新的 baseline */
    }
  else if (resource->type != DAV_RESOURCE_TYPE_REGULAR)
    {
      return DAV_PROP_INSERT_NOTSUPP;
    }
  else
    {
      /* 普通 VCR 返回对应的版本 */
    }
```

### 命名空间定义 (第 50-74 行):
```c
static const char * const namespace_uris[] =
{
  "DAV:",
  SVN_DAV_PROP_NS_DAV,  /* "http://subversion.tigris.org/xmlns/dav/" */
  NULL
};
```

## 测试结果

所有测试通过：
- 12 个单元测试 ✓
- 13 个集成测试 ✓
- 11 个协议兼容性测试 ✓

## 验证建议

1. 使用 SVN 客户端测试 checkout 操作：
   ```bash
   svn checkout http://localhost:3000/svn test-checkout
   ```

2. 使用 curl 检查 PROPFIND 响应：
   ```bash
   curl -X PROPFIND http://localhost:3000/svn -H "Depth: 0"
   ```

3. 检查响应是否包含正确的 XML 格式：
   - `version-controlled-configuration` 元素包含 `<href>` 子元素
   - `checked-in` 元素包含 `<href>` 子元素
   - SVN 属性使用 `svn:` 前缀
   - DAV 属性不使用前缀（继承默认命名空间）

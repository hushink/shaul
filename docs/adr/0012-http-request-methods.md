# 非 GET HTTP 下载

## Context

部分下载入口只接受 POST/PUT/PATCH，需要在请求体里携带 JSON、表单或 token。
内置 HTTP 后端此前只发送 GET，非 GET 只能通过自定义 backend 绕开内置的
续传、镜像和校验。与此同时，分段、Range 和续传都建立在幂等 GET 之上，不能
直接套用到带请求体的方法。

## Decision

- `RequestOptions` 新增 `method: Option<String>` 和 `body: Option<String>`；
  默认 GET、无 body。字段带 `#[serde(default)]`，旧 sidecar 仍可读取。
- 只有直接 HTTP 后端使用这两个字段。Router 在选择非 `http` backend 时返回
  `Error::Config`；Hub client 也会清空它们作为防御。
- method 为 GET 且 body 为空时，行为与 1.2 完全一致：探测、分段、续传、
  镜像回退。
- 其他情况（非 GET，或 GET 携带 body）按"单请求"处理：
  - 探测请求本身就是下载请求，响应体直接流式写入 `.part`，不重复发送。
  - 禁用分段、续传和镜像回退；中断后从头开始。
  - POST/PATCH 不自动重试（非幂等）；GET/HEAD/PUT/DELETE/OPTIONS/TRACE
    仍按 `network.retries` 重试。
- 重定向规则：
  - 301/302 对 POST 改为 GET 并丢弃 body；
  - 303 对所有非 HEAD 方法改为 GET 并丢弃 body；
  - 307/308 保留方法和 body。
  - 跨域且需要保留 body 时，除非 `allow_cross_origin_auth`，否则拒绝转发。
- body 支持 `${env:NAME}` 展开，展开发生在请求构造时；sidecar 保存原始
  文本（Unix 上仍是 0600）。`--json` 输出把 body 显示为 `<redacted>`。
- CLI 增加 `--method`、`--data`、`--data-file`；`--data`/`--data-file` 在
  没有 `--method` 时隐含 POST。库侧同样：body 存在而 method 为空时按 POST
  发送。HEAD 被拒绝，因为它没有可下载的响应体。

## Consequences

- 非 GET 下载不能断点续传；中断后需要从头开始。这是避免重复服务端副作用
  的取舍。
- 非 GET 任务不参与镜像回退；镜像语义与请求体不兼容。
- 内容缓存、校验、解压、磁盘预检、限速、代理和连接预算仍然生效。
- 库用户构造 `RequestOptions` 时应使用 `..Default::default()`；这是
  `api-audit.md` 已记录的 semver 例外。

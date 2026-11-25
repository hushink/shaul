# shaul-http

[![crates.io](https://img.shields.io/crates/v/shaul-http.svg)](https://crates.io/crates/shaul-http)
[![docs.rs](https://docs.rs/shaul-http/badge.svg)](https://docs.rs/shaul-http)

[English](README.md) | **简体中文**

[shaul](https://github.com/hushink/shaul) 的 HTTP 直链后端。shaul 是一个
多协议下载引擎。

## 特性

- 基于 HTTP range 请求的分片下载
- 工作窃取区间队列与自适应分片大小
- 通过 `ETag`、`Last-Modified` 与大小 validator 续传
- 定位写入单个预分配的 `.part` 文件
- `If-Range`、`416` 与 `200 + Content-Range` 处理
- 服务器忽略 `Range` 时回退单流
- 手动重定向处理，跨源时剥离凭据
- 非 GET 请求（`POST`/`PUT`/`PATCH` 等）与可选请求体；单次发送，不分片、
  不续传、不使用镜像
- 自定义请求头，以及 bearer、basic 或 header 认证
- HTTP/HTTPS/SOCKS5 代理、全局/单主机/单任务限速与活动请求预算
- SHA-256/SHA-512/SHA-1/MD5 校验，以及 `Digest` 与 `Content-MD5`
- 已知校验值和大小的情况下，可选命中内容寻址缓存
- 支持 `Content-Disposition` 文件名
- 取消时分片传输会暂停并保留检查点，不丢失已下载数据
- 响应头超时，以及检查点写入前的 fsync 持久化
- 代理模式区分“环境代理”与“显式直连”

## 用法

最简单的入口是 [`shaul`](https://crates.io/crates/shaul) 门面。若只嵌入
HTTP 栈：

```rust,no_run
use shaul_core::Config;

fn main() -> shaul_core::Result<()> {
    let config = Config::default();
    let router = shaul_http::default_router(&config)?;
    let _ = router;
    Ok(())
}
```

`HttpBackend` 与 `DirectLinkParser` 也可以注册到自定义 `ParserRegistry` 或
`Router` 中。

## 文档

- [项目 README](https://github.com/hushink/shaul/blob/main/README.zh-CN.md)
- [库使用指南](https://github.com/hushink/shaul/blob/main/docs/library.md)
- [API 参考](https://docs.rs/shaul-http)

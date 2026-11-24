# shaul-bt

[![crates.io](https://img.shields.io/crates/v/shaul-bt.svg)](https://crates.io/crates/shaul-bt)
[![docs.rs](https://docs.rs/shaul-bt/badge.svg)](https://docs.rs/shaul-bt)

[English](README.md) | **简体中文**

[shaul](https://github.com/hushink/shaul) 的 BitTorrent 支持，基于
[`librqbit`](https://crates.io/crates/librqbit)。

## 支持的输入

- Magnet 链接（`magnet:?xt=urn:btih:...`）
- 裸 40 字符 BitTorrent v1 info hash
- 本地 `.torrent` 文件

## 特性

- 内嵌 librqbit session，支持 DHT、tracker、监听与持久化
- 使用 include/exclude glob 选择文件
- 进度通过 shaul 的类型化事件流上报
- 仅做下载端（leech-only）：下载期间响应对端请求，完成后暂停 torrent；
  shaul 不会持续做种
- 除非显式设置 overwrite/`--force`，否则绝不覆盖目标目录中的已有文件
- 惰性创建 session：构造 backend 不会产生文件系统副作用
- 拒绝 `--output`；BitTorrent 总是写入目标目录

## 用法

通常通过带 `bt` feature 的 `shaul` 门面使用：

```toml
[dependencies]
shaul = { version = "1.2", features = ["bt"] }
```

也可以直接嵌入 `shaul-bt`：`TorrentParser` 解析 torrent 输入，`BtBackend`
持有 librqbit session。该 backend 需要 Tokio runtime。

## 文档

- [项目 README](https://github.com/hushink/shaul/blob/main/README.zh-CN.md)
- [Leech-only ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0005-bt-leech-only.md)
- [API 参考](https://docs.rs/shaul-bt)

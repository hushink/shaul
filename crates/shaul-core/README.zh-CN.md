# shaul-core

[![crates.io](https://img.shields.io/crates/v/shaul-core.svg)](https://crates.io/crates/shaul-core)
[![docs.rs](https://docs.rs/shaul-core/badge.svg)](https://docs.rs/shaul-core)

[English](README.md) | **简体中文**

[shaul](https://github.com/hushink/shaul) 的核心领域类型与下载引擎。shaul 是
面向 HTTP、BitTorrent、Hugging Face、ModelScope 与 FTP 的多协议下载引擎。

`shaul-core` 与具体协议无关。它定义通用流水线：

```text
Link -> LinkParser -> Resource -> Router -> DownloadBackend
```

`DownloadEngine` 负责驱动任务、持久化可续传状态、发出类型化事件，并运行
校验与解压 finalizer。

## 包含内容

- `Link`、`Resource`、`Router` 与 `ParserRegistry`
- `LinkParser` 与 `DownloadBackend` 扩展 trait
- 任务 sidecar（`SidecarStore`）、原子写入与跨进程运行锁
- 续传 manifest、validator 与陈旧任务恢复
- 分片区间计算与 SHA-256/SHA-512/SHA-1/MD5 校验
- 安全的相对路径处理，以及压缩包识别/解压
- 校验失败隔离（`<name>.shaul.failed`）
- 严格配置解析：拒绝未知键和空路径
- 已验证内容寻址缓存（`ContentCache`）与目录布局保护

## 何时直接使用

大多数应用应使用 [`shaul`](https://crates.io/crates/shaul) 门面，它已经组装
好 HTTP、Hub 以及可选的 BitTorrent/FTP/Xet 后端。

当你需要编写自定义 parser、backend 或 router，或者只想嵌入引擎与任务存储
时，可直接使用 `shaul-core`。

```toml
[dependencies]
shaul-core = "1.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## 示例

```rust,no_run
use shaul_core::{Config, SidecarStore, TaskStore};

#[tokio::main]
async fn main() -> shaul_core::Result<()> {
    let config = Config::default();
    let store = SidecarStore::new(config.state_dir()?)?;
    let tasks = store.list().await?;
    println!("{} task(s)", tasks.len());
    Ok(())
}
```

用你的 parser 和 backend 构建 `Router`，再连同 `TaskStore` 与 `Config`
传给 `DownloadEngine::new`。

## 文档

- [库使用指南](https://github.com/hushink/shaul/blob/main/docs/library.md)
- [扩展指南](https://github.com/hushink/shaul/blob/main/docs/extending.md)
- [续传与加固](https://github.com/hushink/shaul/blob/main/docs/resume.md)
- [API 参考](https://docs.rs/shaul-core)

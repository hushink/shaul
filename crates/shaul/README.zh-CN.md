# shaul

[![crates.io](https://img.shields.io/crates/v/shaul.svg)](https://crates.io/crates/shaul)
[![docs.rs](https://docs.rs/shaul/badge.svg)](https://docs.rs/shaul)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue)](https://github.com/hushink/shaul/blob/main/Cargo.toml)

[English](README.md) | **简体中文**

统一、可续传的下载引擎，支持 HTTP(S) 直链、HLS、DASH、Hugging Face、
ModelScope、BitTorrent、FTP，以及可选的 Hugging Face Xet。

`shaul` 是门面 crate。它把 `parser → router → backend` 流水线组装起来：
默认包含 [`shaul-core`](https://crates.io/crates/shaul-core)、
[`shaul-http`](https://crates.io/crates/shaul-http)、
[`shaul-hub`](https://crates.io/crates/shaul-hub)，并按 feature 组合
[`shaul-bt`](https://crates.io/crates/shaul-bt)、
[`shaul-ftp`](https://crates.io/crates/shaul-ftp)、
[`shaul-hls`](https://crates.io/crates/shaul-hls) 和
[`shaul-dash`](https://crates.io/crates/shaul-dash)。

## 安装

```toml
[dependencies]
shaul = "1.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }

# 可选协议支持：
# shaul = { version = "1.2", features = ["bt", "dash", "ftp", "hls", "xet"] }
```

| Feature | 增加的协议 |
| --- | --- |
| `bt` | BitTorrent、magnet 链接和 `.torrent` 文件 |
| `dash` | 静态点播 DASH（`.mpd`）Representation，拼接成一个输出文件 |
| `ftp` | 单流、可续传的 `ftp://` 下载 |
| `hls` | 点播 HLS（`.m3u8`），拼接成一个输出文件 |
| `xet` | Hugging Face Xet 下载，失败时回退 HTTP |

HLS 支持 TS、单 init map 的 fMP4、AES-128 加密、`#EXT-X-BYTERANGE`、
点播分片级续传、分片并发下载，以及通过 `TaskOptions` 的
`live_duration_secs`/`live_segments` 进行有界直播录制（不可续传）；暂不支持
SAMPLE-AES。DASH 支持静态与动态单 Period 清单、SegmentTemplate/SegmentList/
SegmentBase 寻址、点播分片级续传和兼容的静态多 Period 拼接；动态清单支持
SegmentTemplate + SegmentTimeline 或 constant-duration 模板的有界直播录制。
暂不支持 DRM、动态多 Period、init 不兼容的多 Period 和低延迟
`availabilityTimeOffset`。

## 快速开始

```rust,no_run
use shaul::{Config, Link, TaskOptions};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let (engine, _store) = shaul::default_engine(Config::default())?;

    let options = TaskOptions {
        connections: Some(8),
        extract: Some(true),
        ..TaskOptions::default()
    };

    let task = engine
        .add(
            Link::new("https://example.com/data.tar.gz")?,
            PathBuf::from("./downloads"),
            options,
        )
        .await?;

    let task = engine.run_task(task.id).await?;
    println!("{:?}: {}", task.state, task.downloaded_bytes());
    Ok(())
}
```

`Config::default()` 是“惰性”的：不读配置文件，也不读环境变量。若需要 CLI
同款的配置分层（配置文件、`SHAUL_*`、provider 环境变量），使用
`Config::load`。

## 配置

```rust,no_run
use shaul::Config;

fn build() -> shaul::Result<()> {
    // 默认值 -> 环境变量 -> 配置文件（或 $SHAUL_CONFIG / 显式路径）
    let (config, path) = Config::load(None)?;
    println!("config: {}", path.display());

    // 在这里叠加应用自身的命令行参数，保持优先级：
    // 命令行 > 配置文件 > 环境变量 > 默认值。
    let (engine, _store) = shaul::default_engine(config)?;
    let _ = engine;
    Ok(())
}
```

常用开关：

```rust,no_run
use shaul::{Config, TaskOptions};

fn configure() -> shaul::Result<(Config, TaskOptions)> {
    let mut config = Config::default();
    config.network.proxy = Some("socks5://127.0.0.1:1080".to_string());
    config.network.no_proxy = Some("localhost,127.0.0.1".to_string());
    // 将 proxy 设为空字符串可禁用环境变量 HTTP(S)_PROXY：
    // config.network.proxy = Some(String::new());

    config.download.rate_limit = Some(8 * 1024 * 1024); // 全局字节/秒
    config.download.per_host_rate_limit = Some(2 * 1024 * 1024);
    config.download.file_jobs = 4; // 单个多文件任务内的并发文件数
    config.download.global = 32; // 所有任务的活动 HTTP 传输上限
    config.download.per_host_active = 8; // 每个主机的活动传输上限
    config.download.content_cache = true;

    let options = TaskOptions {
        rate_limit: Some(1024 * 1024), // 额外的单任务限速
        file_jobs: Some(4),
        ..TaskOptions::default()
    };
    Ok((config, options))
}
```

## 订阅下载进度与生命周期事件

`engine.subscribe()` 返回 `tokio::sync::broadcast::Receiver<DownloadEvent>`。
事件是尽力投递：慢订阅者可能漏掉事件，但 store 中的任务状态始终是权威来源。

```rust,no_run
use std::path::PathBuf;
use std::time::Duration;

use shaul::{Config, EventKind, Link, TaskOptions, default_engine};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;
    let mut events = engine.subscribe();

    let listener = tokio::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    eprintln!("事件流落后，错过 {missed} 个事件");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };

            match event.kind {
                EventKind::TaskStarted { task_id, .. } => {
                    println!("[{task_id}] 开始");
                }
                EventKind::FileStarted {
                    file,
                    total_bytes,
                    ..
                } => {
                    println!("{file}: 开始（{:?} 字节）", total_bytes);
                }
                EventKind::Progress {
                    file,
                    downloaded_bytes,
                    total_bytes,
                    ..
                } => {
                    println!("{file}: {downloaded_bytes}/{:?}", total_bytes);
                }
                EventKind::Retry {
                    file,
                    attempt,
                    error,
                    ..
                } => {
                    eprintln!("{file}: 第 {attempt} 次重试：{error}");
                }
                EventKind::VerificationCompleted { ok, actual, .. } => {
                    println!("校验结果={ok} 摘要={actual}");
                }
                EventKind::ExtractionCompleted {
                    target,
                    files_extracted,
                    ..
                } => {
                    println!(
                        "已解压 {files_extracted} 个文件到 {}",
                        target.display()
                    );
                }
                EventKind::TaskCompleted { bytes, files, .. } => {
                    println!("完成：{files} 个文件，{bytes} 字节");
                    break;
                }
                EventKind::TaskFailed { error, .. } => {
                    eprintln!("失败：{error}");
                    break;
                }
                EventKind::TaskPaused { .. } | EventKind::TaskCancelled { .. } => break,
                _ => {}
            }
        }
    });

    let task = engine
        .add(
            Link::new("https://example.com/file.bin")?,
            PathBuf::from("./downloads"),
            TaskOptions {
                file_jobs: Some(4),
                ..TaskOptions::default()
            },
        )
        .await?;

    // 演示：10 秒后取消。
    let cancel = shaul::CancellationToken::new();
    let stopper = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            cancel.cancel();
        })
    };
    let _task = engine.run_task_with_cancel(task.id, Some(cancel)).await?;
    stopper.abort();
    let _ = listener.await;
    Ok(())
}
```

CLI 的进度条与 `--json` NDJSON 输出来自同一个事件流。如果库调用方只需要
计数指标，请参阅 [Telemetry](#telemetry)。

## 阻塞式门面

如果程序不自己持有 Tokio runtime，可使用 `default_blocking_engine`。它内部
创建 runtime，并暴露同步方法。

```rust,no_run
use std::path::PathBuf;

use shaul::{Config, DownloadEvent, EventKind, Link, TaskOptions};

fn main() -> shaul::Result<()> {
    let engine = shaul::default_blocking_engine(Config::default())?;

    // std::sync::mpsc::Receiver<DownloadEvent>
    let events = engine.subscribe();
    std::thread::spawn(move || {
        for DownloadEvent { kind, .. } in events {
            if let EventKind::Progress {
                file,
                downloaded_bytes,
                total_bytes,
                ..
            } = kind
            {
                println!("{file}: {downloaded_bytes}/{:?}", total_bytes);
            }
        }
    });

    let task = engine.download(
        Link::new("https://example.com/file.bin")?,
        PathBuf::from("./downloads"),
        TaskOptions::default(),
    )?;
    println!("{:?}", task.state);
    Ok(())
}
```

## 持久队列、暂停/恢复与取消

任务以 JSON sidecar 持久化。store 会按规范化后的链接 + 目标目录对活动任务
去重；已经持有跨进程锁的任务不能被第二个执行者启动。`requeue_stale` 会恢复
那些因进程崩溃而遗留为 `Running`、但 OS 锁已释放的任务。

```rust,no_run
use std::path::PathBuf;
use std::time::Duration;

use shaul::{CancellationToken, Config, Link, TaskOptions, default_engine};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;

    // 崩溃恢复：把仍处于 Running 且运行锁已释放的任务重新入队。
    engine.requeue_stale().await?;

    let destination = PathBuf::from("./downloads");
    let task = engine
        .add(
            Link::new("https://example.com/a.bin")?,
            destination.clone(),
            TaskOptions {
                priority: Some(10),
                ..TaskOptions::default()
            },
        )
        .await?;

    let paused = engine.pause(task.id).await?;
    println!("已暂停：{:?}", paused.state);
    let resumed = engine.resume(task.id).await?;
    println!("已恢复：{:?}", resumed.state);

    // 通过外部取消令牌运行。
    let cancel = CancellationToken::new();
    let stop = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            cancel.cancel();
        })
    };
    let task = engine.run_task_with_cancel(task.id, Some(cancel)).await?;
    stop.abort();
    println!("最终状态：{:?}", task.state);

    // 并发处理 Pending 任务，最多 4 个。
    let completed = engine.run_pending(4, CancellationToken::new()).await?;
    println!("完成 {} 个任务", completed.len());

    for task in engine.list().await? {
        println!("{} {:?} {}", task.id, task.state, task.link);
    }
    Ok(())
}
```

取消分片传输会把任务转为 `Paused`，保留已写入检查点的 `.part` 文件；之后
再次运行即可续传。若任务正被另一个存活的 worker 持有，会返回
`invalid task state: already running`。

## 校验、解压与内容缓存

```rust,no_run
use std::path::PathBuf;

use shaul::{Checksum, Config, Link, TaskOptions, VerifyMode, default_engine};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;

    let task = engine
        .add(
            Link::new("https://example.com/model.tar.gz")?,
            PathBuf::from("./downloads"),
            TaskOptions {
                checksum: Some(Checksum::parse("sha256:<hex>")?),
                verify: Some(VerifyMode::Require),
                extract: Some(true),
                extract_dir: Some(PathBuf::from("./extracted")),
                delete_archive: Some(true),
                content_cache: Some(true),
                ..TaskOptions::default()
            },
        )
        .await?;

    engine.run_task(task.id).await?;
    Ok(())
}
```

- `VerifyMode::Auto` 在已知校验值时校验；`Require` 在缺少校验值时失败；
  `Disabled` 跳过可选校验。
- 校验先于解压。最终校验失败时，输出会被隔离为 `<name>.shaul.failed`。
- `--rename` / `TaskOptions::rename_on_conflict` 会为已存在的解压目标追加
  后缀（`name-1` 等），而不是直接失败。
- 内容缓存只保存同时已知校验值和文件大小的文件；命中后会重新计算哈希再使用。

```rust,no_run
use shaul::{Config, ContentCache};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let config = Config::default();
    let cache = ContentCache::new(config.content_cache_dir()?)?;
    let stats = cache.stats().await?;
    println!("{} 个条目，{} 字节", stats.entries, stats.bytes);

    // 破坏性维护会拒绝看起来不像内容缓存的目录。
    let report = cache.prune(None, Some(50 * 1024 * 1024 * 1024)).await?;
    println!("删除 {} 个条目", report.entries_removed);
    Ok(())
}
```

## 接入自定义 parser 与 backend

Parser 把 `Link` 转为与协议无关的 `Resource`；backend 消费该 resource。将
两者注册到 `Router`，再交给 `DownloadEngine`。实现异步 trait 需要
`async-trait` 依赖。

```rust,no_run
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use shaul::{
    BackendCapabilities, Config, DownloadBackend, DownloadEngine, DownloadOutcome,
    DownloadRequest, Error, EventKind, FileOutcome, Link, LinkParser, ParseOptions,
    ParserRegistry, Resource, ResourceFile, ResourceKind, Result, Router, SidecarStore,
    TaskStore, backend_arc, parser_arc,
};
use url::Url;

struct MemoryParser;

#[async_trait]
impl LinkParser for MemoryParser {
    fn name(&self) -> &'static str {
        "memory-demo"
    }

    fn supports(&self, link: &Link) -> bool {
        link.as_str().starts_with("mem://")
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let name = link.as_str().trim_start_matches("mem://").trim_matches('/');
        if name.is_empty() || name.contains("..") {
            return Err(Error::InvalidLink(link.to_string()));
        }
        Ok(Resource {
            kind: ResourceKind::Custom {
                protocol: "mem".to_string(),
            },
            name: name.to_string(),
            files: vec![ResourceFile {
                path: name.to_string(),
                url: Url::parse(&format!("mem://{name}"))
                    .map_err(|_| Error::InvalidLink(link.to_string()))?,
                size: Some(5),
                checksum: None,
                headers: Vec::new(),
                metadata: BTreeMap::new(),
                use_response_filename: false,
            }],
            request: options.request.clone(),
            checksum: None,
            revision: None,
            metadata: BTreeMap::new(),
        })
    }
}

struct MemoryBackend;

#[async_trait]
impl DownloadBackend for MemoryBackend {
    fn name(&self) -> &'static str {
        "memory-demo"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(false, false, true)
    }

    fn supports(&self, resource: &Resource) -> bool {
        matches!(&resource.kind, ResourceKind::Custom { protocol } if protocol == "mem")
    }

    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        let payload = b"hello";
        let mut outcomes = Vec::new();
        for file in &request.resource.files {
            if request.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let path = request.destination.join(&file.path);
            tokio::fs::write(&path, payload).await?;
            request.events.emit(EventKind::Progress {
                task_id: request.task_id,
                file: file.path.clone(),
                downloaded_bytes: payload.len() as u64,
                total_bytes: Some(payload.len() as u64),
            });
            outcomes.push(FileOutcome::new(
                path,
                file.path.clone(),
                payload.len() as u64,
                None,
                false,
            ));
        }
        Ok(DownloadOutcome::Completed(outcomes))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = Config::default();
    config.state_dir = Some("./state".into());

    let parsers = ParserRegistry::new().with(parser_arc(MemoryParser));
    let router = Router::new(parsers, vec![backend_arc(MemoryBackend)]);
    let store: Arc<dyn TaskStore> = Arc::new(SidecarStore::new(config.state_dir()?)?);
    let engine = DownloadEngine::new(Arc::new(router), store, config);

    let task = engine
        .add(Link::new("mem://hello.txt")?, "./downloads".into(), Default::default())
        .await?;
    engine.run_task(task.id).await?;
    Ok(())
}
```

Backend 必须响应 `DownloadRequest::cancel`、通过 `DownloadRequest::events`
上报进度，并通过 `DownloadRequest::checkpoint` 持久化可续传进度。完整可运行
版本见 `crates/shaul/examples/custom_protocol.rs` 与 `custom_parser.rs`。

## 自定义 finalizer 与解压器

`FinalizerPipeline` 在传输完成后运行。默认流水线是 `VerifyFinalizer` 后接
`ExtractFinalizer`。

```rust,no_run
use std::sync::Arc;

use async_trait::async_trait;
use shaul::{
    ArchiveExtractor, ExtractReport, ExtractorRegistry, ExtractorRequest, FinalizeContext,
    FinalizeResult, Finalizer, FinalizerPipeline, Result, VerifyFinalizer,
};

struct AuditFinalizer;

#[async_trait]
impl Finalizer for AuditFinalizer {
    fn name(&self) -> &'static str {
        "audit"
    }

    async fn finalize(&self, context: &FinalizeContext<'_>) -> Result<FinalizeResult> {
        println!(
            "校验={} 文件={}",
            context.file.checksum_verified, context.file.relative_path
        );
        Ok(FinalizeResult::default())
    }
}

struct DemoExtractor;

impl ArchiveExtractor for DemoExtractor {
    fn name(&self) -> &'static str {
        "demo"
    }

    fn supports(&self, path: &std::path::Path) -> Result<bool> {
        Ok(path.extension().and_then(|ext| ext.to_str()) == Some("demo"))
    }

    fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
        let target = request.extract_root.join("demo");
        std::fs::create_dir_all(&target)?;
        std::fs::copy(request.path, target.join("payload.txt"))?;
        Ok(ExtractReport {
            target,
            files_extracted: 1,
            warnings: Vec::new(),
        })
    }
}

fn install(engine: &mut shaul::DownloadEngine) {
    let registry = Arc::new(ExtractorRegistry::default().with(Arc::new(DemoExtractor)));
    engine.set_finalizers(
        FinalizerPipeline::new(vec![
            Arc::new(VerifyFinalizer),
            Arc::new(AuditFinalizer),
            Arc::new(shaul::ExtractorFinalizer::new(registry)),
        ]),
    );
}
```

自定义解压器必须把所有写入限制在 `ExtractorRequest::extract_root` 内，拒绝
不安全的条目路径，并遵循 `ExtractorRequest::rename_on_conflict`。registry
会规范化返回的目标路径并拒绝越界。

## Telemetry

`Telemetry` 接收低基数的 counter、gauge 和 histogram。每个方法都有默认空实现；
实现不得阻塞 executor。

```rust,no_run
use std::sync::Arc;

use shaul::{Telemetry, metrics};

struct Metrics;

impl Telemetry for Metrics {
    fn counter(&self, name: &str, value: u64, labels: &[(&str, &str)]) {
        eprintln!("counter {name}={value} {labels:?}");
    }

    fn gauge(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        eprintln!("gauge {name}={value} {labels:?}");
    }

    fn histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        eprintln!("histogram {name}={value} {labels:?}");
    }
}

fn attach(mut engine: shaul::DownloadEngine) -> shaul::DownloadEngine {
    engine.set_telemetry(Arc::new(Metrics));
    // 稳定指标名包括 metrics::TASKS_COMPLETED、metrics::BYTES_DOWNLOADED、
    // metrics::ACTIVE_CONNECTIONS、metrics::CACHE_HITS 与 metrics::QUEUE_DEPTH。
    engine
}
```

## 能力

- 分片 HTTP 下载，带工作窃取区间队列与自适应分片大小
- 通过 `ETag`、`Last-Modified` 与大小校验实现安全续传
- 自定义请求头，以及 bearer、basic 或 header 认证
- HTTP/HTTPS/SOCKS5 代理与 `NO_PROXY`；proxy 设为空字符串可禁用环境代理
- 全局、单主机与单任务带宽限制；活动请求预算
- 兼容镜像回退
- SHA-256/SHA-512/SHA-1/MD5 校验，可按下载开关
- 校验后解压，解压器可插拔
- Hugging Face 与 ModelScope 仓库，可选 provider 缓存
- 对带 `xetHash` 的 Hugging Face 文件可选启用 Xet
- 通过 `hls` feature 支持 HLS 点播与有界直播录制，通过 `dash` feature 支持
  DASH 点播与有界动态录制
- 通过 `bt` feature 支持 BitTorrent、magnet 与 `.torrent`
- 通过 `ftp` feature 支持单流 `ftp://` 下载，使用 `SIZE`/`MDTM`/`REST` 续传
- 多文件并发下载
- 可选的、面向已校验文件的内容寻址缓存
- 持久队列，支持优先级、暂停/恢复/取消与重试事件
- 类型化事件、NDJSON 输出与低基数 telemetry 指标

## 示例

| 示例 | 内容 |
| --- | --- |
| [`library_basic`](examples/library_basic.rs) | 构建默认引擎，添加并运行单个任务 |
| [`library_events`](examples/library_events.rs) | 消费类型化 `EventKind` 事件流 |
| [`library_blocking`](examples/library_blocking.rs) | 使用同步门面 |
| [`library_queue`](examples/library_queue.rs) | 持久任务、暂停/恢复并运行队列 |
| [`custom_parser`](examples/custom_parser.rs) | 离线自定义 parser |
| [`custom_protocol`](examples/custom_protocol.rs) | 自定义 parser + backend + telemetry |
| [`custom_extractor`](examples/custom_extractor.rs) | 自定义压缩格式 |

```console
$ cargo run -p shaul --example library_events -- https://example.com/file.bin .
$ cargo run -p shaul --example custom_protocol
```

## 扩展方式

实现 `LinkParser` 和/或 `DownloadBackend`，然后用 `ParserRegistry` 组装
`Router`。压缩格式可通过 `ArchiveExtractor` 与 `ExtractorRegistry` 扩展；
后处理步骤实现 `Finalizer`。扩展在编译期完成，不依赖不稳定的插件 ABI。

## CLI

配套的 [`shaul-cli`](https://crates.io/crates/shaul-cli) crate 提供 `shaul`
命令：

```console
$ cargo install shaul-cli
```

## 文档

- [项目 README](https://github.com/hushink/shaul/blob/main/README.zh-CN.md)
- [库使用指南](https://github.com/hushink/shaul/blob/main/docs/library.md)
- [扩展指南](https://github.com/hushink/shaul/blob/main/docs/extending.md)
- [续传与加固](https://github.com/hushink/shaul/blob/main/docs/resume.md)
- [常见问题](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API 参考](https://docs.rs/shaul)

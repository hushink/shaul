# shaul

[![CI](https://github.com/hushink/shaul/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/hushink/shaul/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/shaul.svg)](https://crates.io/crates/shaul)
[![docs.rs](https://docs.rs/shaul/badge.svg)](https://docs.rs/shaul)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue)](https://github.com/hushink/shaul/blob/main/Cargo.toml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](https://github.com/hushink/shaul/blob/main/LICENSE)

[English](README.md) | **简体中文**

可续传、可插拔的 Rust 下载引擎 —— 用一套 API 处理 HTTP(S)、Hugging Face、
ModelScope、BitTorrent、FTP 与可选 Xet。

shaul 把分片传输、安全续传、校验、解压、持久队列和可选内容寻址缓存组合在
一组可替换组件之后。你可以把它作为库嵌入应用，也可以直接使用 `shaul` 命令。

> [!NOTE]
> CLI 默认启用 BitTorrent、FTP 与 Xet。库用户从默认 feature 开始，按需启用
> `features = ["bt"]`、`["ftp"]` 和/或 `["xet"]`。

## 为什么选择 shaul

- **一套模型处理多种协议。** `LinkParser` 把输入解析为与协议无关的
  `Resource`；`Router` 选择第一个能处理它的后端；`DownloadEngine` 负责
  任务、状态与持久化。
- **续传会校验前提。** sidecar 记录字节区间、校验值和 validator。`ETag`、
  `Last-Modified` 或文件大小发生变化时会安全地重新开始；服务器忽略 `Range`
  时自动回退单流。
- **面向大型模型仓库。** Hugging Face / ModelScope 快照支持 revision、
  include/exclude glob、provider 凭据和可选 provider 缓存；多文件任务可以
  并发下载。
- **生产控制。** 全局、单主机和单任务限速，活动连接预算，代理，兼容镜像
  回退，以及按优先级调度的持久队列。
- **可扩展。** Parser、Backend、TaskStore、Finalizer 与解压器都是 trait，
  不依赖不稳定的动态插件 ABI。

## 工作原理

```text
Link -> LinkParser -> Resource -> Router -> DownloadBackend
                                   |
                            DownloadEngine
                                   |
                     Task -> FileItem -> Segment / Piece
                                   |
                    Finalizer: merge -> verify -> extract -> rename
```

Parser 负责理解输入格式；Resource 以协议无关的方式描述文件；Router 选择后端；
DownloadEngine 负责任务状态、重试、checkpoint 与 finalizer。

## 安装

### CLI

```console
$ cargo install shaul-cli
```

预编译包与安装脚本附在
[`shaul-cli` 的 GitHub Releases](https://github.com/hushink/shaul/releases) 中。

### 库

```toml
[dependencies]
shaul = "1.2"
```

按需启用协议支持：

```toml
[dependencies]
shaul = { version = "1.2", features = ["bt", "ftp", "xet"] }
```

`shaul` 默认不启用任何可选协议；`bt`、`ftp` 与 `xet` 都需显式开启。

## 快速开始

### 使用 CLI

```console
# 直链：续传与校验
$ shaul download https://example.com/model.bin \
      --checksum sha256:<hex> --connections 8

# Hugging Face 仓库快照
$ export HF_TOKEN=hf_...
$ shaul download https://huggingface.co/org/repo \
      --revision main --include '*.safetensors' --cache

# ModelScope 仓库快照
$ export MODELSCOPE_API_TOKEN=...
$ shaul download https://modelscope.cn/models/org/repo

# BitTorrent
$ shaul download 'magnet:?xt=urn:btih:<info-hash>&dn=example'
$ shaul download ./example.torrent --include '*.mkv' --dir ./media

# FTP
$ shaul download ftp://user:password@example.com/pub/file.bin --dir ./downloads

# 代理、全局/单主机限速
$ shaul download https://example.com/file.bin \
      --proxy socks5://127.0.0.1:1080 \
      --limit 10MiB --per-host-limit 5MiB

# 持久队列
$ shaul add --input-file links.txt
$ shaul run --limit 50MiB
$ shaul list
$ shaul status <id 或后缀>
$ shaul pause <id>
$ shaul resume <id>

# 诊断、统计与清理
$ shaul doctor
$ shaul stats
$ shaul clean --finished --cache
```

### 嵌入库

```rust
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

    engine.run_task(task.id).await?;
    Ok(())
}
```

`default_engine` 注册 Hub 与直链 HTTP 的 parser/backend。启用
`bt`、`ftp` 和/或 `xet` 可加入对应后端。通过 `engine.subscribe()` 订阅类型化
进度事件；不想自己持有 Tokio runtime 时使用 `default_blocking_engine`。

## 支持的输入

| 输入 | 示例 |
| --- | --- |
| HTTP(S) 直链 | `https://example.com/file.bin` |
| Hugging Face | `hf://org/repo`、`hf://datasets/org/repo`、仓库 URL |
| ModelScope | `ms://org/repo`、`ms://datasets/org/repo`、仓库 URL |
| BitTorrent | 磁力链、40 位 v1 info hash、本地 `.torrent` |
| FTP | `ftp://user:password@example.com/path/file.bin` |
| 自定义协议 | `ResourceKind::Custom { protocol }` + 自定义 parser/backend |

Hugging Face / ModelScope 链接支持 `--revision`、`--include` 与
`--exclude`；`--revision` 会覆盖 URL 中携带的 revision。仓库 HTTPS URL 也接受
`/tree/<rev>`、`/blob/<rev>/<path>` 和 `/resolve/<rev>/<path>` 形式。

## 核心能力

### 传输

- HTTP 分片下载：工作窃取式区间队列与自适应分片大小。
- 安全续传：`ETag`、`Last-Modified` 与大小 validator；显式处理 `416`；
  服务器忽略 `Range` 时回退单流。
- 兼容镜像回退：镜像必须通过大小与 validator（或 checksum）检查才会使用。
- HTTP/HTTPS/SOCKS5 代理与 `NO_PROXY`；全局、单主机和单任务限速；
  跨任务/单主机的活动请求预算。
- 多文件任务通过 `file_jobs` 并发下载。

### 校验与后处理

- SHA-256、SHA-512、SHA-1、MD5 校验；自动识别 provider checksum 与常见
  HTTP digest 头。
- 先校验再解压；损坏的压缩包不会被展开。
- 支持 zip、tar、tar.gz、tar.zst、gzip、zstd；路径穿越防护、原子替换与
  解压膨胀限制。
- 可通过 `ArchiveExtractor` 接入自定义压缩格式。

### Hub 与可选 Xet

- Hugging Face / ModelScope 快照下载，token 来自环境变量或官方凭据文件。
- 可选 HF 兼容 / ModelScope 缓存，优先硬链接，失败时复制。
- 可选 Xet：带 `xetHash` 的 Hugging Face 文件走 Xet CAS；token 或协议失败
  自动回退普通 HTTP。

### 队列与自动化

- 持久 sidecar 任务存储，支持跨进程 pause/resume/cancel。
- 优先级调度与带 jitter 的重试，重试可通过类型化事件观测。
- NDJSON、类型化事件、`tracing` span 与可插拔 telemetry。
- `doctor`、`stats`、`clean` 运维命令。

### 内容缓存

启用 `download.content_cache`（或 `--content-cache`）后，已知 checksum +
size 的文件可以从内容寻址缓存复用。命中会再次校验；只有校验成功后才会写入
缓存。可用 `shaul clean --cache` 按时间或总大小淘汰缓存；破坏性清理会拒绝
顶层布局不像内容缓存的目录。

## 配置

优先级：

```text
CLI 参数 > 配置文件 > 环境变量 > 默认值
```

`shaul config path` 打印实际路径；`--config PATH` 或 `SHAUL_CONFIG` 可覆盖；
`shaul config init|show|get|set|edit` 管理配置。未知键会被拒绝；配置文件以
原子方式写入，在 Unix 上权限为 0600。`config set` 会原地修改文件，保留注释和
键顺序。

```toml
[download]
dir = "."
connections = 8
per_host = 16
per_host_active = 16
global = 64
segment_size = 8388608
rate_limit = 0
per_host_rate_limit = 0
file_jobs = 1
content_cache = false
# content_cache_dir = "/var/cache/shaul/content"
disk_space_check = true
resume = true
verify = "auto"        # auto | require | disabled
extract = false
# extract_dir = "/path/to/extract"
delete_archive = false

[network]
timeout_secs = 60
connect_timeout_secs = 15
retries = 5            # HTTP 传输
user_agent = "shaul/1.2.1"
allow_cross_origin_auth = false
# proxy = "http://127.0.0.1:8080"
# no_proxy = "localhost,127.0.0.1"

[hub]
use_cache = false
# hf_cache = "/path/to/huggingface/hub"
# modelscope_cache = "/path/to/modelscope"
# hf_endpoint = "https://hf-mirror.com"
# modelscope_endpoint = "https://modelscope.cn"

[bt]
dht = true
trackers = true
listen = true
persistence = true
# peer_limit = 50
# initial_peers = ["1.2.3.4:6881"]

# state_dir = "/path/to/tasks"

[headers]
# X-Test = "value"
# Authorization = "Bearer ${env:MY_TOKEN}"
```

将 `network.proxy` 设为空字符串（或传 `--proxy ""`）可禁用环境代理、直接连接。

常用环境变量包括 `SHAUL_CONFIG`、`SHAUL_DIR`、`SHAUL_STATE_DIR`、
`SHAUL_CONNECTIONS`、`SHAUL_RATE_LIMIT`、`SHAUL_PER_HOST_RATE_LIMIT`、
`SHAUL_FILE_JOBS`、`SHAUL_CONTENT_CACHE`、`SHAUL_CONTENT_CACHE_DIR`、
`SHAUL_DISK_SPACE_CHECK`、`SHAUL_PROXY`、`SHAUL_NO_PROXY`、`HF_TOKEN` 与
`MODELSCOPE_API_TOKEN`。完整配置项可通过 `shaul config` 命令查看。

provider token 在运行时解析，不会写入任务 sidecar。Unix 上 sidecar 以 0600
权限创建。`--json` 输出会脱敏敏感 header，并去掉 HTTP 任务视图中的查询串。

## 关键语义

- HTTP 分片写入同一个预分配 `.part` 文件的不同偏移，不做“每片一文件”的合并。
- 续传默认开启。后端通过 `BackendCapabilities` 声明能力；不支持续传时，
  引擎丢弃旧 manifest 并从零开始。FTP 服务器拒绝 `REST` 时会自动从零重试。
- `--verify` 要求存在校验值；`--no-verify` 关闭可选校验。BitTorrent piece
  校验由协议内部完成，并视为已校验。
- 解压默认关闭。先校验压缩包，再解压到临时目录，最后原子 rename。
  `--rename` 会为已存在的解压目标加后缀；`--no-delete-archive` 可覆盖配置中的
  删除行为。
- `--output` 用于单文件 HTTP/FTP 直链；Hub 仓库和 BitTorrent 请用 `--dir`，
  这两个后端会拒绝 `--output`。
- BitTorrent 只做 leech：下载期间响应上传，完成后暂停，不常驻做种。目标位置
  已有文件时默认拒绝覆盖，需显式 `--force`；正常情况下续传由 librqbit 的
  持久化状态负责。

## 库文档

[库使用指南](docs/library.md) 覆盖配置、事件、持久队列、取消、阻塞 API、
telemetry 与 tracing，并附可直接运行的示例：

- [`library_basic`](crates/shaul/examples/library_basic.rs) —— 添加并运行单个任务
- [`library_events`](crates/shaul/examples/library_events.rs) —— 类型化事件流
- [`library_blocking`](crates/shaul/examples/library_blocking.rs) —— 同步 API
- [`library_queue`](crates/shaul/examples/library_queue.rs) —— 暂停/恢复与批量运行

新增协议时，实现 `LinkParser` 和/或 `DownloadBackend`，再用
`ParserRegistry` 组装 `Router`。参考
[扩展开发指南](docs/extending.md)、
[`custom_parser`](crates/shaul/examples/custom_parser.rs)、
[`custom_protocol`](crates/shaul/examples/custom_protocol.rs) 与
[`custom_extractor`](crates/shaul/examples/custom_extractor.rs) 示例。

## 文档

- [库使用指南](docs/library.md)
- [扩展开发指南](docs/extending.md)
- [续传与加固](docs/resume.md)
- [FAQ](docs/faq.md)
- [API 参考](https://docs.rs/shaul)

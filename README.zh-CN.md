# shaul

[![CI](https://github.com/hushink/shaul/actions/workflows/ci.yml/badge.svg?branch=main&style=flat-square)](https://github.com/hushink/shaul/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/shaul.svg?style=flat-square)](https://crates.io/crates/shaul)
[![docs.rs](https://docs.rs/shaul/badge.svg?style=flat-square)](https://docs.rs/shaul)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue?style=flat-square)](https://github.com/hushink/shaul/blob/main/Cargo.toml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](LICENSE)

可续传、可插拔的 Rust 下载引擎 —— 同时提供库与 CLI。

[English](README.md)

> **状态：1.0.3** —— 直链 HTTP、Hugging Face、ModelScope 与 BitTorrent
> 已实现并发布到 [crates.io](https://crates.io/crates/shaul)。1.x 稳定性
> 规则见 [API 审计](docs/api-audit.md)。

## 为什么做 shaul

现有 Rust 下载 crate 多数只是单流 HTTP 封装。shaul 用一套小而明确的领域模型，
把协议细节藏在可替换的后端之后：

- **HTTP 分片下载**：工作窃取式分片队列 + 每文件连接数上限。
- **正确的断点续传**：带版本号的 sidecar、`ETag`/`Last-Modified`/大小
  validator、`If-Range`，以及服务器忽略 `Range`（200 而非 206，包括
  ModelScope 的 `200 + Content-Range`）时的安全回退。
- **校验**：SHA-256/SHA-512/SHA-1/MD5；自动识别 HF LFS `X-Linked-Etag`、
  HTTP `Digest`/`Content-MD5`、ModelScope `Sha256` 与 HF 树 API 的
  `lfs.oid`。
- **Finalizer 流水线**：合并（仅独立分片后端）→ 校验 → 解压 → 原子落盘。
- **压缩包解压**：zip、tar、tar.gz、tar.zst、gz、zst，带路径穿越防护与
  原子替换。解压器可通过 `ArchiveExtractor` 替换，7z/rar/xz 可由第三方提供。
- **每任务请求头与鉴权**：可重复 header、Bearer/Basic、跨域重定向剥离
  敏感头，并支持 `${env:NAME}` 占位符。
- **Hub 一等公民**：Hugging Face / ModelScope 仓库快照、revision、
  include/exclude glob、环境变量或官方凭据文件中的 token，以及可选的
  HF 兼容缓存。
- **BitTorrent**：磁力链、裸 v1 info hash、本地 `.torrent`；通过 librqbit
  使用 DHT、tracker、peer wire、文件选择与 pause/resume。只做 leech：
  完成后立即暂停，不常驻做种。
- **持久队列**：`shaul add`、`shaul run` worker，以及跨进程
  pause/resume/cancel。
- **网络控制**：显式 HTTP/HTTPS/SOCKS5 代理与 `NO_PROXY`，全局及单任务限速。
- **可观测性**：同一条类型化事件流同时驱动人类可读进度条与 NDJSON 输出；
  `tracing` span 与可插拔 `Telemetry` 指标钩子支持日志、监控和外部接入。

## 架构

```text
Link -> LinkParser -> Resource -> Router -> DownloadBackend
                                   |
                            DownloadEngine
                                   |
                     Task -> FileItem -> Segment / Piece
                                   |
                    Finalizer: merge -> verify -> extract -> rename
```

Workspace crate：

- `shaul-core` —— 领域类型、引擎、路由、任务存储、续传与 finalizer。
- `shaul-http` —— 直链 HTTP 解析器与分片传输后端。
- `shaul-hub` —— Hugging Face / ModelScope 解析器、REST 客户端与后端。
- `shaul-bt` —— BitTorrent 解析器与 leech-only 后端（可选）。
- `shaul-cli` —— `shaul` 二进制。
- `shaul` —— 默认组装；库用户通过 `bt` feature 选择是否引入 BT，CLI 默认启用。

`LinkParser` 允许发起网络请求补全元数据（Hub 链接）；本地 `.torrent` 路径
直接解析。`Resource` 与协议无关；`Router` 选择第一个支持该资源的后端。
第一个认领链接的 parser 具有权威性，因此 Hub 元数据错误不会回退成
"把 HTML 页面当文件下载"。术语见 [GLOSSARY.md](GLOSSARY.md)，设计决策见
[docs/adr](docs/adr)。

## 快速开始

### CLI

```console
$ cargo install shaul-cli
# 预编译包与安装脚本：shaul-cli 的 GitHub Releases
# （从下一个 CLI 版本起提供）

# 直链
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

# 代理与限速
$ shaul download https://example.com/file.bin \
      --proxy socks5://127.0.0.1:1080 --limit 10MiB

# 持久队列与批量输入
$ shaul add --input-file links.txt
$ shaul run --limit 50MiB
$ shaul list
$ shaul status <id 或后缀>
$ shaul pause <id>      # 运行中的 worker 约 500ms 内感知
$ shaul resume <id>
```

也接受 `hf://org/repo`、`hf://datasets/org/repo`、`ms://org/repo`、
`ms://datasets/org/repo`，以及 `/tree/<rev>`、`/blob/<rev>/<path>`、
`/resolve/<rev>/<path>` 形式的 URL。40 位十六进制字符串会按裸 BitTorrent
v1 info hash 处理。

常用参数：`--dir`、`--output`（单文件直链）、`--checksum`、
`--verify/--no-verify`、`--extract/--no-extract`、`--extract-dir`、
`--delete-archive`、`--header`、`--header-file`、`--bearer-token`、`--basic`、
`--proxy`、`--no-proxy`、`--limit`、`--input-file`、`--revision`、
`--include`、`--exclude`、`--cache/--no-cache`、`--json`、`--force`、
`--overwrite`、`--rename`。`download`/`add` 的 `--limit` 限制单个任务；
`shaul run --limit` 限制 worker 的全局速率。

### 库

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
            PathBuf::from("."),
            options,
        )
        .await?;
    engine.run_task(task.id).await?;
    Ok(())
}
```

`default_engine` 会注册 Hub 与直链 HTTP 的 parser/backend；在 `shaul` 依赖上
启用 `bt` feature 即可加入 BitTorrent。通过 `engine.subscribe()` 订阅类型化
事件；不想自己持有 Tokio runtime 时可用 `default_blocking_engine`。

[库使用指南](docs/library.md) 覆盖配置、事件、持久队列、取消、阻塞 API、
telemetry 与 tracing，并附可直接运行的示例：

- [`library_basic`](crates/shaul/examples/library_basic.rs) —— 添加并运行单个任务
- [`library_events`](crates/shaul/examples/library_events.rs) —— 类型化事件流
- [`library_blocking`](crates/shaul/examples/library_blocking.rs) —— 同步 API
- [`library_queue`](crates/shaul/examples/library_queue.rs) —— 暂停/恢复与批量运行

新增协议时，实现 `LinkParser` 和/或 `DownloadBackend`，再用 `ParserRegistry`
与后端优先级组装 `Router`。参考 [扩展开发指南](docs/extending.md)、
[`custom_parser`](crates/shaul/examples/custom_parser.rs)、
[`custom_protocol`](crates/shaul/examples/custom_protocol.rs) 与
[`custom_extractor`](crates/shaul/examples/custom_extractor.rs) 示例。扩展是编译期
完成的，不涉及不稳定的动态插件 ABI。

## 配置

优先级：**CLI 参数 > 配置文件 > 环境变量 > 内置默认值**。

`shaul config path` 打印实际路径；`--config PATH` 或 `SHAUL_CONFIG` 可覆盖；
`shaul config init|show|get|set|edit` 管理配置。

```toml
[download]
dir = "."
connections = 8
per_host = 16
global = 64             # 预留；尚未强制执行
segment_size = 8388608
rate_limit = 0         # 全局共享字节/秒；0 = 不限速
resume = true
verify = "auto"        # auto | require | disabled
extract = false
extract_dir = ""
delete_archive = false

[network]
timeout_secs = 60
connect_timeout_secs = 15
retries = 5
user_agent = "shaul/1.0.3"
allow_cross_origin_auth = false
# proxy = "http://127.0.0.1:8080"   # http://、https:// 或 socks5://
# no_proxy = "localhost,127.0.0.1"

[hub]
use_cache = false      # 开启 HF 兼容缓存 / ModelScope 缓存
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

网络相关环境变量包括 `SHAUL_RATE_LIMIT`、`SHAUL_PROXY`、
`SHAUL_NO_PROXY`，以及常规的 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` /
`NO_PROXY`。显式配置优先于环境变量。

Hub 相关环境变量包括 `HF_TOKEN`、`HUGGING_FACE_HUB_TOKEN`、`HF_HOME`、
`HF_HUB_CACHE`、`HF_ENDPOINT`、`MODELSCOPE_API_TOKEN`、`MODELSCOPE_CACHE`、
`MODELSCOPE_ENDPOINT`、`SHAUL_HF_CACHE`、`SHAUL_MODELSCOPE_CACHE`、
`SHAUL_HF_ENDPOINT`、`SHAUL_MODELSCOPE_ENDPOINT` 与 `SHAUL_HUB_USE_CACHE`。

BitTorrent 环境变量包括 `SHAUL_BT_DHT`、`SHAUL_BT_TRACKERS`、
`SHAUL_BT_LISTEN`、`SHAUL_BT_PERSISTENCE`、`SHAUL_BT_PEER_LIMIT` 与
`SHAUL_BT_INITIAL_PEERS`（逗号分隔的 `ip:port`）。

Unix 上 task sidecar 以 0600 权限写入。来自环境变量或官方凭据文件的 provider
token 在运行时解析，不会落盘；用户显式传入的 `--bearer-token`/`--basic`/header
会保存在 sidecar 中，建议使用 `${env:NAME}` 占位符。`--json` 输出会脱敏敏感
header，并在任务视图中去掉 HTTP 链接的查询串。`config show` 与
`config get network.proxy` 会脱敏代理凭据。

## 关键语义

- HTTP 分片写入同一个预分配 `.part` 文件的不同偏移，不做"每片一文件"的合并；
  只有产生独立分片的后端才需要 merge 步骤。
- 续传默认开启。validator 变化会安全地重新开始；服务器忽略 `Range` 时回退单流；
  `416` 有明确处理；`200 + Content-Range` 视为部分响应。
- `--verify` 要求存在校验值；`--no-verify` 关闭可选校验。BitTorrent piece
  校验是内部完成的，并被视为已经校验。
- 解压默认关闭。先校验压缩包，再解压到临时目录，最后原子 rename。路径穿越与
  符号链接条目会被拒绝或跳过。
- Hub 快照落在 `<目标目录>/<repo 名>/<仓库内相对路径>`。`--output` 用于单文件
  直链；Hub 与 BitTorrent 请用 `--dir`。`--checksum` 只接受单文件 Hub 或
  直链；BitTorrent 会拒绝。
- Hub 缓存默认关闭，用 `hub.use_cache` / `--cache` 开启。HF 缓存布局为
  `models--org--repo/{blobs,refs,snapshots}`（数据集为 `datasets--...`）；
  快照与目标文件通过硬链接或复制引用 blob。ModelScope 使用 shaul 自己的
  `models--org--repo` 布局。
- BitTorrent 只做 leech：下载期间按协议响应上传，完成后立即暂停，不常驻做种。
  piece 状态与 fastresume 由 librqbit 拥有，位于 `<state_dir>/bt`。task sidecar
  在完成/暂停时更新；下载中的实时进度通过事件流输出。
- `shaul pause/resume/cancel` 修改 sidecar；运行中的 `shaul run` worker 约 500ms
  内感知。worker 通过锁文件保证每个状态目录只有一个实例。

## 路线图

- **0.3** —— 已完成：通过 `librqbit` 实现 BitTorrent 后端（只做 leech）。
- **1.0** —— 已完成：API 冻结、各 crate README、跨平台 CI 与 release-plz 发布。
- **1.1** —— 下一步：代理、限速、可替换解压器、完整 `BlockingEngine`
  生命周期与 `--input-file`。
- **1.2** —— 吞吐与多源：多文件并发、真正执行连接预算、镜像回退与任务优先级。
- **1.3** —— Hub 差异化：spike 通过后的可选 Xet 后端、CAS 缓存与缓存维护。
- **1.4** —— 运维：重试/连接/缓存指标、OpenTelemetry 适配、`doctor`/`clean`/`stats`。

详细计划、扩展边界与发布门槛见 [docs/roadmap.md](docs/roadmap.md)。

1.x 暂不做：daemon/RPC、做种、aria2 控制文件兼容、内置 7z/rar/xz 解压
（第三方可实现 `ArchiveExtractor`）、FTP/SFTP/WebDAV/S3 签名、动态插件 ABI
与 WASM。

## 测试

```console
$ cargo test --workspace
```

测试覆盖分片下载、续传、200 回退、重定向、自定义 header、校验、
tar.gz/zip 解压、HF/MS 仓库下载、include 过滤、鉴权失败、缓存填充与缓存命中，
以及本地 BitTorrent peer（创建种子、seeder session、leech 下载）。
不依赖外网。

## API 稳定性

1.0 稳定性审计（semver 规则、MSRV 策略、可构造类型与已知风险）见
[docs/api-audit.md](docs/api-audit.md)。所有 crate 都启用了
`#![forbid(unsafe_code)]`。跨平台 CI 覆盖 Linux、Windows、macOS，以及
rustfmt、clippy、rustdoc、feature 组合和 MSRV 1.92。

## 文档

- [库使用指南](docs/library.md) —— 嵌入、事件、队列、阻塞 API 与可观测性
- [路线图](docs/roadmap.md) —— 1.x 能力、扩展边界与发布门槛
- [扩展开发指南](docs/extending.md) —— 自定义 Parser/Backend、路由与测试
- [FAQ](docs/faq.md) —— 续传、校验、解压、Hub 缓存与队列
- [API 审计](docs/api-audit.md) —— 1.x semver 规则与已知风险
- [发布流程](docs/release.md) —— trusted publishing 与 CLI 二进制
- [Benchmark](docs/benchmarks.md) —— 本地 HTTP 吞吐与续传基线
- [架构决策](docs/adr) 与 [术语表](GLOSSARY.md)

## 许可证

Apache-2.0，见 [LICENSE](LICENSE)。

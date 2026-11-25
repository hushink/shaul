# shaul 1.0 API 审计

- 审计日期：2026-11-18
- 基线：`0.3.0`（commit `9e7226f`）
- 目标：`1.0.0` 的公开 API 稳定性契约
- 状态：`1.0.0` 于 2026-11-18 发布，`1.1.0`、`1.2.0` 于 2026-11-19 发布，`1.2.1` 于 2026-11-21 发布到 crates.io；后续按本文的 semver 规则维护
- 范围：`shaul-core`、`shaul-http`、`shaul-hub`、`shaul-bt`、`shaul-ftp`、
  `shaul-hls`、`shaul-dash`、`shaul`、`shaul-cli`

本文只记录**对外承诺**和**已知风险**，不重复 README 的功能说明。CLI 参数与
`config.toml` 的兼容性也在承诺范围内，但以行为兼容为主，不承诺内部字段顺序。

## 1. 公开面清单

| crate | 稳定公开面 | 备注 |
| --- | --- | --- |
| `shaul-core` | 领域模型、引擎、扩展 trait、配置、事件、错误、checksum、路径与 range 工具 | `util` 已改为私有 |
| `shaul-http` | `HttpBackend`、`DirectLinkParser`、默认 router | `filename` 已改为私有 |
| `shaul-hub` | `HubBackend`、`HubClient`、`HubEntry`、两个 parser、默认 router | `cache` 已改为私有 |
| `shaul-bt` | `BtBackend`、`TorrentParser`、默认 router | 不泄漏 `librqbit` 类型 |
| `shaul-ftp` | `FtpBackend`、`FtpParser`、默认 router | 不泄漏 `suppaftp` 类型 |
| `shaul-hls` | `HlsBackend`、`HlsClient`、`HlsParser`、playlist 类型 | 不泄漏 `reqwest` 类型；可选 feature |
| `shaul-dash` | `DashBackend`、`DashClient`、`DashParser`、MPD/Representation 类型 | 不泄漏 `reqwest`/`quick-xml` 类型；可选 feature |
| `shaul` | 重新导出上述稳定项 + `default_engine`/`default_blocking_engine`/`default_router`/`BlockingEngine`/`CancellationToken` | `bt`、`ftp`、`hls`、`dash` 为可选 feature |
| `shaul-cli` | `shaul` 二进制、命令、配置键、JSON 事件输出 | 进程边界，不是 Rust API |

第三方依赖类型只出现在明确接受的位置：`tokio::sync::broadcast::Receiver`
（事件订阅）、`tokio_util::sync::CancellationToken`（取消）、`uuid::Uuid`
（`TaskId`）和 `async_trait` 生成的 trait。这些都是 1.x 的固定选择，升级它们
属于 minor 版本中的依赖更新，不改变这些类型的公开用法。

## 2. 稳定性分级

### Tier 1：稳定 API

`Link`、`LinkParser`、`ParseOptions`、`Resource`、`ResourceKind`、
`ResourceFile`、`Router`、`ParserRegistry`、`DownloadBackend`、
`DownloadRequest`、`DownloadOutcome`、`FileOutcome`、`BackendCapabilities`、
`DownloadEngine`、`BlockingEngine`、`Task`、`TaskOptions`、`TaskState`、
`FileItem`、`FileState`、`TaskResume`、`FileResume`、`Validator`、
`Config` 及其子配置、`RequestOptions`、`Auth`、`Header`、
`FtpBackend`/`FtpParser`（启用 `ftp` 时）、
`EventKind`/`DownloadEvent`/`EventSink`、`Checksum`/`ChecksumAlgorithm`、
`Error`/`Result`、`TaskStore`/`SidecarStore`、`Finalizer` 管线、
`Telemetry`/`NoopTelemetry`/`metrics`、`ContentCache`/`CacheStats`/
`CachePruneReport`、`ensure_free_space`。

这些项遵循第 4 节的 semver 规则。

### Tier 2：扩展点

`LinkParser`、`DownloadBackend`、`ResumeCheckpoint`、`TaskStore`、
`Finalizer`、`ArchiveExtractor` 面向第三方实现。它们与 Tier 1 同等受
semver 保护，但额外约定：

- 未来新增 trait 方法必须带默认实现，否则只能进 major 版本。
- 扩展作者应只依赖 trait 方法和 `shaul-core` 的公开类型，不依赖具体后端的
  内部模块。
- `Resource.metadata`、`FileItem` 对应 sidecar 中的 `metadata` map 用于承载
  协议私有信息；新增协议字段优先放进这些 map，避免改动结构体布局。

### Tier 3：内部实现，不承诺

- 所有 `pub(crate)`/私有模块与函数（包括已私有的 `shaul-core::util`、
  `shaul-hub::cache`、`shaul-http::filename`）。
- sidecar JSON 的字段顺序、临时文件命名（`.shaul.part`、`.json.tmp`、
  `.store.lock`）。
- `librqbit` 的会话、DHT、tracker 内部行为。
- Hub provider API 的返回字段细节；只承诺 shaul 自身的解析结果。

sidecar 的**语义**仍受保护：`TASK_SCHEMA_VERSION = 1`、`TaskResume.version = 1`
在 1.x 内保持可读；新增字段必须带 `#[serde(default)]`，未知字段按 serde
默认策略忽略。删除或重命名字段需要提供迁移逻辑，不能静默丢任务。

## 3. 已应用的兼容性加固

1. **`#[non_exhaustive]`**
   - 错误与状态：`Error`、`TaskState`、`FileState`、`EventKind`、
     `ResourceKind`、`VerifyMode`。
   - 可扩展值枚举：`BitTorrentSource`、`HubProvider`、`HubRepoType`、
     `ChecksumAlgorithm`、`ArchiveKind`。
   - 输出/持久化结构：`Task`、`FileItem`、`TaskResume`、
     `BackendCapabilities`、`FileOutcome`。
   - 配置：`Config`、`DownloadConfig`、`NetworkConfig`、`HubConfig`、
     `BtConfig`、`ResolvedOptions`。
2. **构造函数补位**
   - `FileOutcome::new(path, relative_path, size, checksum, checksum_verified)`。
   - `BackendCapabilities::new(segmented, resume, multi_file)`。
   - `Config::default()` + 字段赋值是配置结构的受支持构造方式。
3. **跨 crate 匹配回退**
   - `shaul-hub` 对未知 `HubProvider`/`HubRepoType` 返回明确错误或使用中性的
     cache 前缀，而不是让新增变体导致编译失败。
   - `shaul-bt` 对未知 `BitTorrentSource` 返回 `Error::InvalidLink`。
4. **`#![forbid(unsafe_code)]`**
   - 七个 crate 的根模块全部启用；不安全代码只能来自依赖。
5. **公开面收缩**
   - `shaul-core::util`、`shaul-hub::cache`、`shaul-http::filename` 改为私有，
     避免把内部 helper 变成 1.0 承诺。
6. **feature 边界**
   - `shaul` 的默认 features 为空；`bt` 才引入 `shaul-bt`，`ftp` 才引入
     `shaul-ftp`，`hls` 才引入 `shaul-hls`，`dash` 才引入 `shaul-dash`，
     `xet` 才引入 `shaul-hub/xet` 与 `hf-xet`。增加 feature 是 minor 变更，
     删除或改变默认值是 major 变更。
7. **跨平台 CI**
   - `.github/workflows/ci.yml` 覆盖 Linux/Windows/macOS 测试、rustfmt、
     clippy、rustdoc、feature 组合、release build 与 MSRV 1.92。
8. **文档完整性**
   - 所有 crate 启用 `#![warn(missing_docs)]`；CI 的 `rustdoc -D warnings`
     会阻止新增未文档化的公开 API。
   - 扩展指南、FAQ 与可编译的 `custom_parser` 示例见
     [docs/extending.md](extending.md) 和 [docs/faq.md](faq.md)。
9. **1.x 扩展能力**
   - `ResourceKind::Custom { protocol }` 允许第三方 parser/backend 接入没有
     内置协议类型的新协议；协议数据放在 `Resource::metadata`。
   - `Telemetry` / `NoopTelemetry` / `metrics` 提供低基数计数器、gauge 和
     histogram；通过 `DownloadEngine::with_telemetry` 或 `set_telemetry` 注入。
     指标名是 1.x 稳定契约。
   - `DownloadEngine::with_finalizers` / `set_finalizers` 允许替换
     verify/extract 管线。
   - `ArchiveExtractor` / `ExtractorRegistry` / `ExtractorFinalizer` /
     `FinalizerPipeline::with_extractor_registry` 允许注册自定义解压格式；
     7z/rar/xz 不内置，由第三方实现。`ExtractFinalizer` 的内置行为保持不变。
     内置 zip/tar/gzip/zstd 解压限制为 100,000 个条目和 100 GiB 展开大小；
     自定义 extractor 需自行实现同等的 bomb 防护。
   - `tracing` span（`shaul.engine.*`、`shaul.http.*`、`shaul.hub.*`、
     `shaul.bittorrent.*`）和事件流是对外可观测接口；用户自行安装 tracing
     subscriber，并通过 `subscribe()` 消费任务事件。
   - `default_blocking_engine` 让无 Tokio runtime 的同步应用也能使用默认
     backend/store 组装；`CancellationToken` 从 `shaul` 直接重导出，避免库
     用户额外依赖 `tokio-util`。
   - `NetworkConfig::proxy` / `no_proxy`、`DownloadConfig::rate_limit`、
     `DownloadConfig::per_host_rate_limit`、`TaskOptions::rate_limit`、
     `DownloadConfig::file_jobs` 与 `TaskOptions::file_jobs` 提供代理、
     全局/单主机/单任务带宽和多文件并发控制；
     `DownloadConfig::global` / `per_host_active` 限制跨任务的活动 HTTP
     文件传输请求数。`TaskOptions.mirrors` / `ResolvedOptions.mirrors`
     提供兼容镜像回退。默认均为关闭/未设置，保持 1.0 行为。
     `BlockingEngine` 同步补齐
     `status`/`pause`/`resume`/`cancel`/`run_pending`/`requeue_stale`。
   - `RequestOptions.method` / `RequestOptions.body` 为直接 HTTP(S) 下载提供
     非 GET 方法和请求体；这类任务按单请求执行（不分段、不续传、不使用镜像，
     `POST`/`PATCH` 不自动重试）。Hub、FTP、BitTorrent 拒绝这两个字段。
   - CLI `pause`/`resume`/`cancel` 接受多个 id 或 `--all`；`--all` 对
     pause/cancel 默认选择 pending/running/paused，对 resume 默认选择
     paused/failed，`--status` 可覆盖。`list --status` 可重复按状态过滤。
     单 id 的旧用法保持兼容。
   - 本能力遵循“库与 CLI 尽量对齐”：库通过公开配置/任务字段获得同等能力，
     CLI 额外负责从用户配置、环境变量和 flags 填充这些字段。
   - `shaul-ftp` 是可选 crate：`FtpParser`/`FtpBackend` 通过 `ftp` feature
     暴露，不泄漏 `suppaftp` 类型。FTP 是单流后端，续传依赖服务器的
     `SIZE`/`MDTM`/`REST`；FTPS 暂未承诺。
   - `shaul` 的 `xet` feature 启用 `shaul-hub/xet`，由 Hub 后端在树 API
     返回 `xetHash` 时使用 `hf-xet`（Apache-2.0）下载；失败回退 HTTP。
     `hf-xet` 类型不泄漏到 `shaul-core`，默认 feature 仍为空。
   - `shaul` 的 `hls` feature 启用 `shaul-hls`：`HlsParser` 解析 HTTP(S)
     `.m3u8`，`HlsBackend` 下载分片并拼接为一个文件。支持点播 TS、单 init
     map 的 fMP4、AES-128 加密（密钥轮换、显式/默认 IV）和
     `EXT-X-BYTERANGE`（含隐式偏移）和有界直播录制；SAMPLE-AES 暂不支持。
     直播通过新增的 `TaskOptions::live_duration_secs`/`live_segments`（minor
     公开字段）限定停止条件，
     按 `#EXT-X-TARGETDURATION` 轮询、按 media sequence 去重；直播不支持续传，
     失败/取消的临时目录由 `clean --orphans` 清理。DASH 由独立的 `dash`
     feature 处理。AES-128 密钥只驻留内存，不写入 sidecar。`reqwest` 类型
     不泄漏到 `shaul-core`。CLI `clean --orphans` 会为已完成/已取消任务
     删除 HLS 分片临时目录，跳过符号链接和不安全路径。
   - `shaul` 的 `dash` feature 启用 `shaul-dash`：`DashParser` 解析 HTTP(S)
     `.mpd`，`DashBackend` 下载 SegmentTemplate/SegmentList/SegmentBase
     分片并拼接为一个文件。支持静态和动态（`type="dynamic"`）单 Period、
     最高码率视频 Representation；动态清单支持 SegmentTemplate +
     SegmentTimeline，以及带 `availabilityStartTime` 的 constant-duration
     SegmentTemplate。后端按 `minimumUpdatePeriod` 轮询、按 presentation
     time 去重，并在达到 `TaskOptions` live 限制、MPD 变为 static 或 Period
     结束时停止；constant-duration 录制从 live edge 开始，用
     `publishTime`（缺失时本机时钟）和 `timeShiftBufferDepth` 计算可用窗口。
     动态录制不支持续传，失败临时目录由 `clean --orphans` 清理。新增
     `SegmentTiming`、`SegmentRef.timing`、`DynamicTemplate`、
     `SelectedRepresentation.live`/`minimum_update_period_secs`/`dynamic`、
     `DashManifest::is_dynamic`/`select_representation` 等公开项，属于新
     crate 的 additive API。DRM、多 Period、低延迟
     `availabilityTimeOffset` 和交替音轨暂不支持。`quick-xml` 和 `reqwest`
     类型不泄漏到 `shaul-core`。
   - `HttpBackend::download_range(request, file, range, destination)` 下载
     精确字节区间，复用 header/认证/代理/超时/重试/限速和连接预算；HLS 的
     `EXT-X-BYTERANGE` 和 DASH 的字节范围使用该方法。它是新增公开方法，
     属于 minor 变更。
   - `Task.priority` / `TaskOptions.priority` 控制队列顺序：`run_pending`
     按高优先级、创建时间、任务 id 依次排序；默认优先级为 0。
   - `EventKind::Retry` 和 `metrics::RETRIES` 暴露重试尝试；指数 backoff
     增加最多 25% jitter（服务器 `Retry-After` 仍优先）。
   - `ContentCache` / `CacheStats` / `CachePruneReport` 提供按 checksum +
     size 寻址的内容缓存；`download.content_cache`、
     `download.content_cache_dir`、`TaskOptions.content_cache`、
     `SHAUL_CONTENT_CACHE` 与 `SHAUL_CONTENT_CACHE_DIR` 控制启用与位置。
     默认关闭；命中后重新校验，校验失败自动回退网络。
     `ContentCache::clear`/`stats`/`prune`/`prune_preview` 供运维命令使用。
   - `download.disk_space_check` 在已知全部文件大小且目标文件系统空间不足时
     提前失败；`SHAUL_DISK_SPACE_CHECK` 可覆盖。默认开启。
     `ensure_free_space(destination, required)` 是供自定义 backend 复用的
     公开检查函数；无法获取空间信息时按未知处理，不阻塞下载。
   - `metrics::ACTIVE_CONNECTIONS`、`QUEUE_DEPTH`、`CACHE_HITS`、
     `CACHE_MISSES`、`CACHE_BYTES`、`CACHE_INSERTS` 扩展观测面。
   - `DownloadEngine` 在 `BackendCapabilities::resume = false` 或用户关闭
     resume 时清空 `DownloadRequest::resume` 并强制 `options.resume = false`；
     FTP 在服务器拒绝 `REST` 时自动断线重连并从零开始。
10. **semver 检查与刻意例外**
   - `shaul-core/Cargo.toml` 将 `constructible_struct_adds_field` 配置为
     `warn` + `required-update = "minor"`。这是第 4 节 `TaskOptions` /
     `RequestOptions` 例外的机器可读表达：新增字段仍会在检查报告中提示，
     但不会把 1.x 版本强制提升为 major。
   - `EventKind::Retry` 追加在枚举末尾，避免已有变体的判别值变化；
     今后新增变体也必须追加。
   - `EventSink` 内部用 `AssertUnwindSafe` 保存 `Arc<dyn Telemetry>`，
     保持 1.0 已有的 `UnwindSafe` / `RefUnwindSafe` 自动 trait 实现。
     遥测回调不会在 unwinding 期间被观察。

## 4. semver 规则

### 允许的 patch

- bug fix、文档、性能、日志、内部重构。
- 不新增公开项，不改变可观察行为（除非修复的是明确 bug）。
- 不提高 MSRV。

### 允许的 minor

- 新增公开类型、函数、模块、feature。
- 在 `#[non_exhaustive]` 结构体上新增字段。
- 在 `#[non_exhaustive]` 枚举上新增变体。
- 在 trait 上新增带默认实现的方法。
- 标记旧 API 为 `#[deprecated]`，至少保留一个 minor 再删除。
- 提高 MSRV；必须在 CHANGELOG 和 release note 中单独说明。
- 升级依赖的主版本（如 `reqwest`、`librqbit`），前提是公开类型不泄漏。

### 需要 major

- 删除/重命名公开项，改变函数签名、trait 必需方法、枚举变体的字段。
- 给非 `#[non_exhaustive]` 的结构体加字段、给非 `#[non_exhaustive]`
  的枚举加变体。
- 改变已有配置键的含义、sidecar 语义或默认行为（例如默认开启校验）。
- 移除 feature 或改变默认 feature 集合。

### 两个刻意的例外

- **`TaskOptions` / `RequestOptions` 保持可穷举构造。** 它们是用户和 CLI
  高频构造的入参。README 示例使用 `..Default::default()`；只要遵守这个
  构造方式，新增字段就是 minor 兼容的。把字段全部列出的穷举字面量不受支持，
  新增字段时这类代码需要补一行。
- **`DownloadOutcome`、`Auth` 保持可穷举。** 前者必须允许第三方后端构造
  `Completed`/`Paused`；后者的 `Header` 变体已经能表达任意认证方案，
  新增变体的需求很低。若未来必须扩展，按 major 处理。

### 发布与 MSRV

- MSRV 为 Rust 1.92（workspace `rust-version`）。
- 1.x 内允许在 minor 版本提高 MSRV，但必须单独公告；patch 不提高。
- crates.io 发布仍需维护者显式确认；在功能未完全冻结前不发布。

## 5. 已知风险与后续约束

1. **`TaskOptions` 穷举构造**：如第 4 节所述，这是 ergonomics 与严格 semver
   之间的取舍。文档和示例统一使用 `..Default::default()`。
2. **扩展结构体**：`Resource`、`ResourceFile`、`FileResume`、`Validator`、
   `DownloadRequest` 由不同 crate 的 parser/backend 直接构造，因此没有加
   `#[non_exhaustive]`。给它们加字段属于 major 变更；协议私有数据应进入
   `metadata` map。若 1.x 内确实需要扩展，优先新增构造函数/`with_*` 方法，
   而不是改字段。
3. **trait 演进**：`LinkParser`、`DownloadBackend`、`TaskStore`、
   `ResumeCheckpoint`、`Finalizer` 当前的方法都是必需的。1.x 内只允许新增
   带默认实现的方法；需要改签名时走 major。
4. **公开的第三方类型**：`tokio`、`uuid`、`async_trait` 出现在签名中。
   1.x 不会更换这些选择；如果必须升级到不兼容的大版本，按 major 处理。
5. **sidecar 与 CLI 兼容**：CLI 的 `--json` 事件是 NDJSON，稳定字段包括
   `event`、`task_id`、`state`、`file`、`bytes` 等；新增字段是 minor。
   `config.toml` 通过 `PartialConfig` 合并；为避免拼写错误被静默忽略，未知
   键现在会被拒绝并在 CHANGELOG 中标注。新增配置键是 minor，改变已有键的
   含义是 major；旧版本读取新键会报错，发布时需要提醒用户。
   sidecar JSON 仍然忽略未知字段，新增字段必须带 `#[serde(default)]`。
6. **平台差异**：Hub cache 优先 hardlink，失败时回退 copy；Windows 的
   `rename` 语义与 Unix 不同。跨平台 CI 必须覆盖 Linux/Windows/macOS。
7. **BT 行为**：leech-only，完成后暂停，不做种。改变这一行为属于 major。

## 6. 1.0 发布前检查清单

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-features`
- [ ] `cargo check --workspace --no-default-features`
- [ ] `cargo check -p shaul --features bt`
- [ ] `cargo doc --workspace --no-deps`（`RUSTDOCFLAGS="-D warnings"`）
- [ ] MSRV 1.92 job 通过
- [ ] Linux/Windows/macOS CI 矩阵通过
- [ ] README 中英文示例与 `cargo test --doc` 一致
- [ ] 发布前再次确认：**未得到用户明确指令，不发布 crates.io、不创建远端仓库**

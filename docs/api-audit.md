# shaul 1.0 API 审计

- 审计日期：2026-11-18
- 基线：`0.3.0`（commit `9e7226f`）
- 目标：`1.0.0` 的公开 API 稳定性契约
- 范围：`shaul-core`、`shaul-http`、`shaul-hub`、`shaul-bt`、`shaul`、`shaul-cli`

本文只记录**对外承诺**和**已知风险**，不重复 README 的功能说明。CLI 参数与
`config.toml` 的兼容性也在承诺范围内，但以行为兼容为主，不承诺内部字段顺序。

## 1. 公开面清单

| crate | 稳定公开面 | 备注 |
| --- | --- | --- |
| `shaul-core` | 领域模型、引擎、扩展 trait、配置、事件、错误、checksum、路径与 range 工具 | `util` 已改为私有 |
| `shaul-http` | `HttpBackend`、`DirectLinkParser`、默认 router | `filename` 已改为私有 |
| `shaul-hub` | `HubBackend`、`HubClient`、`HubEntry`、两个 parser、默认 router | `cache` 已改为私有 |
| `shaul-bt` | `BtBackend`、`TorrentParser`、默认 router | 不泄漏 `librqbit` 类型 |
| `shaul` | 重新导出上述稳定项 + `default_engine`/`default_router`/`BlockingEngine` | `bt` 为可选 feature |
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
`EventKind`/`DownloadEvent`/`EventSink`、`Checksum`/`ChecksumAlgorithm`、
`Error`/`Result`、`TaskStore`/`SidecarStore`、`Finalizer` 管线。

这些项遵循第 4 节的 semver 规则。

### Tier 2：扩展点

`LinkParser`、`DownloadBackend`、`ResumeCheckpoint`、`TaskStore`、
`Finalizer` 面向第三方实现。它们与 Tier 1 同等受 semver 保护，但额外约定：

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
   - 六个 crate 的根模块全部启用；不安全代码只能来自依赖。
5. **公开面收缩**
   - `shaul-core::util`、`shaul-hub::cache`、`shaul-http::filename` 改为私有，
     避免把内部 helper 变成 1.0 承诺。
6. **feature 边界**
   - `shaul` 的默认 features 为空；`bt` 才引入 `shaul-bt`。
     增加 feature 是 minor 变更，删除或改变默认值是 major 变更。
7. **跨平台 CI**
   - `.github/workflows/ci.yml` 覆盖 Linux/Windows/macOS 测试、rustfmt、
     clippy、rustdoc、feature 组合、release build 与 MSRV 1.92。

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
   `config.toml` 通过 `PartialConfig` 合并，未知键被忽略以保证前向兼容；
   1.x 不会改为拒绝未知键。新增配置键是 minor，改变已有键的含义是 major。
   sidecar JSON 同样忽略未知字段，新增字段必须带 `#[serde(default)]`。
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

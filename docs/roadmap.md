# shaul 1.x 路线图

- 更新日期：2026-11-19
- 基线：1.1.0（1.1 与 1.2 的吞吐能力已合并发布）
- 当前开发线：1.2.0 —— 运维闭环与通用内容缓存
- 目标：在不破坏 1.0 兼容性的前提下，把 shaul 从“能下载”推进到“可长期运行、可嵌入、可扩展”的生产级下载引擎。

## 原则

1. **只做加法**：新增公开项、配置键和 feature；不改变现有默认行为。
2. **默认关闭**：代理、限速、Xet 等重能力默认不启用，避免改变已有用户的行为。
3. **重依赖隔离**：Xet、OpenTelemetry 等放入可选 crate/feature，默认构建保持轻量。
4. **扩展优先**：协议、解压、缓存等边界优先提供 trait/registry，而不是把第三方格式塞进核心。
5. **无外网测试**：所有新能力用本地 fixture 测试；性能改动以 benchmark 作为合入门槛。
6. **发布需确认**：每个 minor 走 release-plz，但合并 release PR、创建 tag 和发布仍需维护者明确同意。

## 版本总览

| 版本 | 主题 | 主要能力 | 状态 |
| --- | --- | --- | --- |
| **1.1** | 生产化基础 | 代理、限速、可替换解压器、BlockingEngine 生命周期、批量输入文件 | 已发布 2026-11-19 |
| **1.2** | 吞吐与多源 | FTP 后端、多文件并发、全局/单主机连接预算、镜像回退、任务优先级 | 已随 1.1.0 交付 |
| **1.2.0** | 运维闭环与缓存 | 内容寻址缓存、`doctor`/`stats`/`clean`、缓存/队列/连接指标 | 已实现，待发布 |
| **1.3** | Hub 差异化 | 可选 Xet 后端、缓存维护、CAS 增量维护 | 高，需先 spike |
| **1.4** | 运维与观测 | OpenTelemetry / Prometheus 适配、残留 `.part` 扫描、磁盘预检 | 低中 |

---

## 1.1 生产化基础（已随 1.1.0 发布）

### 代理

- `NetworkConfig` 增加 `proxy`、`no_proxy`。
- CLI 增加 `--proxy`、`--no-proxy`。
- 支持 `http://`、`https://`、`socks5://`；代理认证放在 URL 中。
- 读取 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`，显式配置优先。
- 首期覆盖 HTTP 与 Hub；BitTorrent DHT/UDP 代理语义不同，不在本期承诺。
- 本地代理 e2e 测试：绝对 URI、CONNECT、认证、重定向、Hub 下载经代理。

### 限速

- `DownloadConfig.rate_limit`：全局共享字节/秒。
- `TaskOptions.rate_limit`：额外的单任务上限；与全局上限同时生效，取较低值。
- CLI `--limit`，复用现有字节单位解析。
- token bucket 覆盖所有 HTTP Range 流；暂停/取消必须立即释放等待。
- 默认不限速；先覆盖 HTTP/Hub，BitTorrent 限速后续通过 librqbit 能力补齐。
- 测试：吞吐不超过上限 + 容差、单任务覆盖生效、取消不挂死。

### 可替换解压器

当前解压由 `ExtractFinalizer` 直接调用 `detect_archive` / `extract_archive`，格式集合写死在内置实现里。1.1 将其提升为核心抽象组件：

- `ArchiveExtractor`：单个格式的检测与解压实现。
- `ExtractorRegistry`：按顺序选择第一个 `supports()` 为真的 extractor；内置格式作为默认 registry 的最后一项。
- `ExtractorRequest` / `ExtractReport`：传递输入路径、解压根目录、覆盖策略和结果。
- `ExtractorFinalizer`：使用自定义 registry 的 finalizer；现有 `ExtractFinalizer` 保持默认行为不变。
- `FinalizerPipeline::with_extractors(...)`：一行替换引擎的解压管线。

拟定接口（实现时以 rustdoc 为准）：

```rust
pub trait ArchiveExtractor: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, path: &Path) -> Result<bool>;
    fn extract(&self, request: ExtractorRequest<'_>) -> Result<ExtractReport>;
}

pub struct ExtractorRegistry { /* built-ins + custom */ }
pub struct ExtractorFinalizer { /* registry */ }
```

约束：

- `supports()` 必须便宜、无副作用；第一个认领的 extractor 是权威的，失败时不得静默尝试下一个。
- extractor 只能写入 `extract_root` 下；自定义实现必须复用 `sanitize_relative_path`，拒绝 `..`、绝对路径、符号链接和特殊文件。
- 默认要求“临时目录写完再 rename”的原子语义；框架在完成后校验最终目标仍在 `extract_root` 内。
- 解压仍在 `spawn_blocking` 中执行；事件和 `delete_archive` 逻辑留在 `ExtractorFinalizer`。
- 内置格式保持不变：zip、tar、tar.gz、tar.zst、gz、zst。

扩展边界：

- 7z / rar / xz **不内置**。用户可以通过 `ArchiveExtractor` + `ExtractorRegistry` 接入 `sevenz-rust`、`unrar` 或外部命令，并自行承担依赖、许可证和解压安全责任。
- 提供一个 `custom_extractor` 示例，演示排序、错误处理和路径安全检查。
- `docs/extending.md` 增加“Custom archive extractors”章节；`docs/api-audit.md` 将 `ArchiveExtractor` 列为 Tier 2 扩展点。

### 库与 CLI 补齐

- `BlockingEngine` 增加 `pause`、`resume`、`cancel`、`status`、`run_pending`、`requeue_stale`。
- CLI 增加 `--input-file FILE`：每行一个链接，支持空行和 `#` 注释。
- 文档、示例和 rustdoc 同步更新。

### 1.1 实施顺序

1. [x] `ArchiveExtractor` / `ExtractorRegistry` / `ExtractorFinalizer` 及内置 registry；先保持现有解压行为完全不变。
2. [x] `custom_extractor` 示例、扩展文档和路径安全测试。
3. [x] 代理配置、CLI、环境变量和本地代理 e2e。
4. [x] 全局/单任务限速及取消测试。
5. [x] `BlockingEngine` 生命周期方法及库测试。
6. [x] `--input-file`、README/FAQ/rustdoc 更新。
7. [x] 全量 fmt、clippy、test、doc 和基准运行（8 连接结果需在安静环境复核）。

### 1.1 验收

- 代理、限速、解压器都有本地 e2e 测试。
- 现有自定义 finalizer 和 `ExtractFinalizer` 用法不回归。
- `cargo test --workspace --all-features`、clippy、rustdoc、基准测试全部通过。
- 默认配置下行为与 1.0.3 一致。

---

## 1.2 吞吐与多源（已随 1.1.0 发布）

### FTP 后端

- 新增可选 crate `shaul-ftp` 与 facade `ftp` feature。
- `FtpParser` 解析 `ftp://`，使用 `ResourceKind::Custom { protocol: "ftp" }`，不把 FTP 类型泄漏进 core。
- `FtpBackend` 单流下载，`BackendCapabilities::new(false, true, false)`；使用 `SIZE`/`MDTM`/`REST` 保守续传，服务器不支持时明确报错。
- 认证使用 `Auth::Basic`；URL userinfo 会转成 Basic auth，并从 Resource URL 中剥离。
- 本地 mock FTP server e2e：完整下载、Basic 认证、REST 续传。
- 第一版只支持 plain FTP；FTPS、代理和目录递归下载后续评估。

### 多文件并发

状态：已随 1.1.0 发布。

- `DownloadConfig.file_jobs` / `TaskOptions.file_jobs`，默认 1。
- Hub 快照和多文件 HTTP 任务内并发下载。
- 每个文件独立 checkpoint；sidecar 保存串行化，避免竞态。
- 暂停/取消恢复时，已完成文件不重新下载。
- 测试：最大并发、暂停恢复、单文件失败不影响已完成文件。

### 全局连接预算

状态：已随 1.1.0 发布。

- 启用 `download.global` 作为跨任务的活动 HTTP 请求上限。
- 新增 `download.per_host_active`，用共享 semaphore 限制单主机活动请求。
- 多任务并发时不超过全局预算，并避免小任务饥饿。
- 与 `file_jobs` 联合测试，防止“任务数 × 连接数”放大。

### 镜像回退

状态：已随 1.1.0 发布 fallback。

- `TaskOptions.mirrors: Vec<Url>`，CLI `--mirror`（可重复）。
- 主 URL 失败时尝试镜像；镜像必须通过 size/validator 兼容性检查。
- 不会静默下载不同内容；镜像兼容性不足时发出 Warning 并拒绝。
- 跨域继续剥离认证头。
- 先做 fallback，稳定后再评估多源竞速。

### 队列与重试

状态：`priority`、jitter、重试事件与指标、续传加固评估均已实现。

- 任务 `priority`，`run_pending` 按优先级取任务。
- 重试增加 jitter 和 attempt 指标；保留现有 `Retry-After` 与指数 backoff。
- 续传加固：`.part` 长度钳制、后端 capability 检查和 FTP REST 回退已实现；
  残余风险与测试矩阵见 [docs/resume.md](resume.md)。

---

## 1.2.0 运维闭环与内容缓存（已实现，待发布）

### 内容缓存

- [x] `ContentCache`：按 `checksum + size` 寻址，默认关闭。
- [x] `download.content_cache` / `download.content_cache_dir` 与
  `TaskOptions.content_cache`；CLI `--content-cache` / `--no-content-cache`。
- [x] 命中后重新校验，损坏条目自动删除并回退网络。
- [x] 仅在 verify finalizer 成功后写入缓存；`--no-verify` 不缓存。
- [x] 硬链接优先，失败时复制并原子替换。
- [x] `shaul stats` / `shaul clean --cache` 统计与清理。
- [x] `ContentCache::prune` / `prune_preview`，支持按时间和总大小淘汰。
- [x] 目标磁盘空间预检（`download.disk_space_check`，默认开启）。
- [x] 自适应 HTTP 分片大小，小文件中提高并发切分利用率。
- [x] `shaul clean --orphans` 清理已完成/取消任务遗留的 `.part`。
- [x] 内置解压的条目数/展开大小上限（100,000 条目、100 GiB）。

### 运维命令

- [x] `shaul doctor`：配置、状态目录、下载目录、缓存目录、代理与 provider
  token 检查。
- [x] `shaul stats`：任务状态、字节数、队列深度、缓存条目/字节。
- [x] `shaul clean`：按完成/取消/失败状态清理 sidecar，`--cache` 清理/按
  时间或大小淘汰缓存，`--orphans` 清理残留 `.part`，`--dry-run` 预览。
- [x] CLI 与库能力对齐：诊断和缓存能力通过公开 `Config`、`TaskOptions` 与
  `ContentCache` 暴露。

### 指标

- [x] `shaul.http.active_connections`
- [x] `shaul.queue.depth`
- [x] `shaul.cache.hits` / `misses` / `bytes` / `inserts`
- [x] OpenTelemetry / Prometheus 适配说明与接入路径（见
  [extending.md](extending.md)；不引入核心依赖）

---

## 1.3 Hub 差异化

### 可选 Xet 后端

- [x] 完成 spike：`hf-xet` 为 Apache-2.0，可满足许可证要求，但 Rust API
  仍在稳定过程中，依赖体积大，且需要 CAS fixture 才能做无外网 e2e。
  结论：1.2.0 暂不引入，保留 HTTP resolve 回退；详见
  [xet-spike.md](xet-spike.md)。
- 新建可选 crate `shaul-xet`，facade 增加 `xet` feature，默认关闭。
- 解析 `X-Xet-Hash`、`X-Xet-Cas-Url`、`X-Xet-Token-Expiration`、`X-Xet-Refresh-Route`。
- 支持 CAS 下载、短期 token 刷新；失败回退到普通 HTTP resolve。
- 后续实现前先建立本地 CAS fixture，确保测试不依赖外网。
- 如果上游 API 或 Windows 构建仍不稳定，则推迟到 2.0，不阻塞 1.x 其他能力。

### 通用 CAS 缓存（已随 1.2.0 实现）

- [x] 仅对“已知 checksum + 已知 size”的下载启用。
- [x] 以 `digest+size` 为 key，原子写入，硬链接/复制到目标。
- [x] 默认关闭，校验通过后才入缓存。
- [x] 提供 `clean`/`prune`，避免缓存无限增长。

---

## 1.4 运维与观测（后续）

- [x] 重试、活动连接、缓存命中/未命中、队列深度指标（已随 1.2.0 实现）。
- [x] 提供 OpenTelemetry / Prometheus 适配说明；SDK 由应用引入，核心依赖不变。
- [x] `shaul doctor`：配置、状态目录、代理、Hub token、目录权限。
- [x] `shaul clean`：已完成/失败任务 sidecar 与内容缓存。
- [x] `shaul stats`：任务数量、字节数、失败原因分布。
- [x] 残留 `.part` 扫描与磁盘空间预检（已随 1.2.0 实现）。

---

## 扩展边界

1. **协议扩展**：现有 `LinkParser` + `DownloadBackend` + `ResourceKind::Custom` 保持不变，用户可自行实现任意协议。1.x 不再为 SFTP、WebDAV、S3 签名等非目标新增专门支持；FTP 作为可选 `ftp` feature 提供。
2. **解压扩展**：1.x 唯一新增的专用扩展点是 `ArchiveExtractor` + `ExtractorRegistry`；7z/rar/xz 由用户实现。
3. **任务存储扩展**：`TaskStore` 保持可替换；`SidecarStore` 只是默认实现。
4. **观测扩展**：`Telemetry`、`tracing`、事件流保持独立，不绑定具体后端。
5. **不引入动态插件 ABI**：所有扩展仍是编译期 trait 组合。

## 明确不做（1.x）

- daemon / RPC / Web UI
- BitTorrent seeding（保持 leech-only）
- aria2 control-file 兼容
- 内置 7z / rar / xz 解压（仅提供扩展点）
- FTPS / SFTP / WebDAV / S3 SigV4 签名
- 动态插件 ABI
- WASM

## 发布门槛

每个 minor 发布前：

- 新增公开 API 有 rustdoc，`missing_docs` 通过。
- 本地 e2e 测试不依赖外网。
- 现有 1.0 行为回归测试通过。
- 性能相关改动附带 benchmark 对比。
- 文档、示例、`docs/api-audit.md` 同步更新。
- 合并 release PR 与 crates.io 发布需维护者确认。

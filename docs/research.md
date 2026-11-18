# 调研：下载器生态与 shaul 的取舍

日期：2026-11-18。事实来自公开文档、crate 源码与官方仓库；能力会变化，接入前应
重新核对。

## 1. Rust 生态：传输层成熟，下载层薄弱

- `reqwest`/`hyper` 只提供传输能力；Range 分段、续传、校验、镜像与调度都要
  自行实现。
- 现有下载 crate 规模小、多为单人维护且仅支持 HTTP：`downloader-rs`（分块+
  限速+xxHash）、`ripget`（多路 Range）、`down`（BLAKE3+镜像+位图续传）、
  `dlmgr`、`bucket_dl`、`http-downloader`（已停更）。共性短板：无 BT、无
  HF/ModelScope 鉴权、无共享缓存、API 不稳。
- 官方 `hf-hub` 1.0 支持 token、缓存与 `hf-xet`，但下载路径是单流、
  无断点续传、无单文件多连接。直接包装它会放弃 shaul 的续传卖点。
- `aria2-rs` 等只是 aria2 的 RPC 客户端，不能内嵌引擎。

## 2. HTTP Range 的正确语义

- 单段成功：`206` + `Content-Range: bytes a-b/total`。
- 不可满足：`416` + `Content-Range: bytes */len`。
- 服务器可忽略 Range 返回 `200`；客户端必须按状态码决定追加还是重下。
- 续传应带 `If-Range` + 强 ETag/Last-Modified；多段响应为
  `multipart/byteranges`（shaul 只发单 Range 请求，不解析多段响应）。

参考：RFC 9110 §14。

## 3. 主流下载器能力与限制

- **aria2**：HTTP/FTP/BT/Metalink、多源多连接；单服务器硬上限 16 连接；
  JSON-RPC/XML-RPC/WebSocket；RPC 暴露有风险。
- **Motrix**：aria2 GUI，2023 年后停更。
- **FDM**：多段+BT，闭源；**IDM**：付费、仅 Windows；**XDM**：维护弱；
  **JDownloader 2**：插件生态但与模型下载无关；**Persepolis** 只是 aria2 前端；
  **pyLoad** 无头插件式；**curl -C -** 仅单流续传。
- 共性短板：依赖服务器支持 Range；ETag/Last-Modified 变化会让续传失效；
  不具备 HF/ModelScope 的 token、LFS、Xet 语义。

## 4. Hugging Face

- 下载：`GET https://huggingface.co/{repo}/resolve/{rev}/{path}` 先 302 到 CDN
  （`cdn-lfs-*.hf.co`、`cas-*.xethub.hf.co`）。
- 关键头位于 302 响应：`X-Linked-Etag`、`X-Linked-Size`、`X-Repo-Commit`、
  `X-Xet-Hash`、`X-Xet-Cas-Url`、`X-Xet-Token-Expiration`、
  `X-Xet-Refresh-Route`。因此必须关闭自动重定向并逐跳处理。
- 跨域到 CDN 时 `Authorization` 会被剥离；CDN URL 通常自带签名。
- 缓存：`models--org--repo/{blobs,refs,snapshots}`（数据集为
  `datasets--...`），snapshots 为符号链接（Windows 用副本）。
- Xet 与 LFS：LFS 按整文件 SHA-256 去重；Xet 使用内容定义分块（约 64 KiB）
  做块级去重。`hf_xet` 自 1.0 起默认，`hf_transfer` 已移除。
- `huggingface_hub` 1.18 不再从 `.incomplete` 续传，原因是在 Lustre/GPFS/NFS
  上 flock 失效导致缓存损坏（NVIDIA 报告）。这为 shaul 的 sidecar 方案留下
  明确空档：不依赖 flock，且记录强 validator。
- `hf-xet` 是可依赖的 Rust crate（Apache-2.0）；Python `hf_xet` 是同一 crate
  的 PyO3 扩展，不是外部二进制。但接入需要 CAS URL、短期 token、refresh route，
  API 仍在重构，依赖树较大。0.2 先用纯 HTTP 下载 Xet 存储的文件，0.3+ 再把
  `hf-xet` 做成可选 feature。

## 5. ModelScope

- 模型文件：`/api/v1/models/{id}/repo?Revision=&FilePath=`；
  模型列表：`/api/v1/models/{id}/repo/files?Revision=&Recursive=`。
- 数据集：`/api/v1/datasets/{ns}/{name}/repo?...`；
  树：`/api/v1/datasets/{id}/repo/tree?Revision=`。
- 鉴权：`MODELSCOPE_API_TOKEN` 优先，其次
  `~/.modelscope/credentials/cookies` 中的 `m_session_id`；当前官方客户端
  同时发送 Bearer 与 Cookie。
- 官方 1.15 按 160 MiB 分片、最多 4 线程，碎片文件 `<file>_<start>_<end>`
  即续传控制文件；最新逻辑已迁至 `modelscope_hub`。
- shaul 用统一的 8 MiB 分片与自己的 sidecar，不复刻官方碎片文件。

## 6. BitTorrent 生态

- `librqbit`/`rqbit`：活跃（crates.io Apache-2.0），DHT、PEX、磁力、BEP 52、
  限速、`StorageFactory`/`TorrentStorage`、`only_files`、pause/unpause；
  进度靠 stats 轮询而非事件回调；默认不启 HTTP server/Web UI。
- `cratetorrent`：2022 年停更，仅 Linux、仅 BT v1、无 DHT/磁力。
- `bip_metainfo` 只解析种子且多年未更新；libtorrent/transmission 绑定依赖 C++。

## 7. 机会点与 shaul 的选择

| 机会 | shaul 的选择 |
| --- | --- |
| 工业级正确续传（含 HF 1.18 缺口） | v1 核心卖点；sidecar + 强 validator + `If-Range` + 200 回退 |
| 统一任务模型与可插拔后端 | v1 核心卖点；`Task → FileItem → Segment/Piece`，编译期 trait 扩展 |
| HF/ModelScope 一等公民 | 0.2；自研 REST/resolve 客户端，兼容 HF 缓存 |
| 多源竞速/镜像 | 不进 v1；预留 Router 优先级与 fallback |
| 内容寻址共享缓存 | 0.2；HF 兼容缓存 + 全局 CAS |
| 公平调度与限速 | v1 只做连接数；带宽限速后续 |
| 可观测性 | v1；统一事件流 + NDJSON |
| daemon/RPC | 不进 v1；持久队列与 run worker 预留 |

## 8. 主要来源

- RFC 9110 Range: https://www.rfc-editor.org/rfc/rfc9110.html#name-range
- aria2: https://aria2.github.io/
- hf-hub: https://docs.rs/hf-hub/latest/hf_hub/
- huggingface_hub file_download:
  https://github.com/huggingface/huggingface_hub/blob/main/src/huggingface_hub/file_download.py
- HF 1.18 release:
  https://github.com/huggingface/huggingface_hub/releases/tag/v1.18.0
- HF 1.18 resume PR: https://github.com/huggingface/huggingface_hub/pull/4306
- HF cache layout: https://huggingface.co/docs/hub/main/local-cache
- Xet overview: https://huggingface.co/docs/hub/en/xet/overview
- hf-xet crate: https://crates.io/crates/hf-xet
- ModelScope file_download:
  https://github.com/modelscope/modelscope/blob/master/modelscope/hub/file_download.py
- ModelScope 1.15 release:
  https://github.com/modelscope/modelscope/releases/tag/v1.15.0
- librqbit: https://crates.io/crates/librqbit
- rqbit: https://github.com/ikatson/rqbit
- crates.io API: https://crates.io/api/v1/crates/

# 下载领域模型

本项目统一描述"用户想下载什么、由谁执行、下载状态如何组织"的词汇。公开 API、文档与代码标识符以此为准。

## Language

**Link**:
用户提交的原始下载输入，可以是 URL、磁力链、种子文件或 Hub 仓库引用。
_Avoid_: Source, target, input

**Resource**:
链接解析器对 Link 的解释结果，描述可下载内容及其端点、鉴权、期望大小/摘要与版本。
_Avoid_: ParsedLink, target, download info

**LinkParser**:
把 Link 解析为 Resource 的组件；允许发起网络请求补全元数据。
_Avoid_: Resolver, extractor

**DownloadBackend**:
执行某一类协议下载的组件，消费 Resource 并产出 FileItem。
_Avoid_: Downloader, provider, engine

**Router**:
按 Resource 类型与 DownloadBackend 自声明能力选择后端的组件，支持优先级与失败回退。
_Avoid_: Dispatcher, selector, registry

**DownloadEngine**:
持有 Router、任务状态与调度的运行时，是库用户创建和驱动下载的入口。
_Avoid_: Manager, client, downloader

**Task**:
一次用户下载意图，是取消、暂停与恢复的单位；一个 Task 可包含多个 FileItem。
_Avoid_: Job, download, item

**FileItem**:
Task 内的单个输出文件，是落盘与内容校验的单位。
_Avoid_: File, entry, subtask

**Segment**:
FileItem 的一段 HTTP 字节区间，是并发传输与续传记录的单位。
_Avoid_: Chunk, part, block

**Piece**:
BitTorrent 的校验单元，一个 FileItem 可能跨多个 Piece。
_Avoid_: Chunk, block

**Resume**:
从已持久化的部分完成状态继续下载；默认开启，可在引擎或任务级关闭。
_Avoid_: Continue, restart

**Validator**:
服务器提供的资源版本判据（如 ETag、Last-Modified、总大小），用于判断 Resume 是否仍然有效。
_Avoid_: Check, fingerprint

**Direct link**:
直接指向文件字节的 HTTP(S) URL，由默认后端处理。
_Avoid_: Normal link, raw URL

**Hub**:
托管模型与数据集的平台（Hugging Face、ModelScope）；其下载需要鉴权、文件清单与版本概念。
_Avoid_: Registry, store

**Revision**:
Hub 仓库的版本标识，可以是分支、tag 或 commit。
_Avoid_: Version, branch

**Snapshot**:
Hub 仓库在某个 Revision 下的文件路径视图，是 Hub Task 的下载单位。
_Avoid_: Checkout, tree

**Blob**:
缓存中按内容哈希命名的文件；Snapshot 与目标目录通过链接或复制引用它。
_Avoid_: Object, chunk

**Content cache**:
跨协议复用的、按 Checksum + size 寻址的已验证内容缓存；与 Hub 的 provider 缓存相互独立，默认关闭。
_Avoid_: Cache, CAS, blob store

**Cache hit**:
Content cache 中存在校验通过、可直接链接或复制到目标的内容。
_Avoid_: Reuse, dedupe

**Magnet**:
由 info hash 与可选 trackers/名称组成的 BitTorrent 链接；元数据需要从 peer 获取。
_Avoid_: Magnet link, URI

**Torrent file**:
包含 BitTorrent metainfo 的种子文件；可以直接解析文件清单。
_Avoid_: Seed, metainfo file

**Leech**:
只下载、下载期间按协议响应上传、完成后不常驻做种的模式。
_Avoid_: Download-only, client

**Checksum**:
期望的内容校验值（如 SHA-256），可选择性地与下载结果比对。
_Avoid_: Hash, digest, fingerprint

**Verification**:
将 FileItem 内容与 Checksum 比对的过程；默认在存在 Checksum 时开启。
_Avoid_: Validation, hash check

**Extraction**:
下载完成后把压缩包 FileItem 展开到目录的可选后处理。
_Avoid_: Unpack, decompress

**Finalizer**:
Task 标记完成前按固定顺序执行的收尾步骤（合并、校验、解压、原子落盘）。
_Avoid_: Post-processor, hook

**Auth**:
用户为 Task 提供的鉴权信息，如 Bearer token、Basic 凭据或自定义鉴权头。
_Avoid_: Credentials, token

**Header override**:
用户为 Task 指定的额外 HTTP 请求头，优先级高于后端默认值；跨域重定向默认不转发敏感头。
_Avoid_: Custom headers

**HTTP method**:
直接 HTTP(S) 下载使用的请求方法；默认 GET。非 GET 或携带 Request body 的
Task 按单请求执行，不分段、不续传、不使用镜像。
_Avoid_: Verb, command

**Request body**:
非 GET 直接下载随请求发送的文本体；支持 `${env:NAME}` 占位符，持久化在
sidecar 中，并在 JSON 视图中脱敏。
_Avoid_: Payload, data

**HLS**:
HTTP Live Streaming；由 master/media playlist 和分片组成的点播媒体。HLS 任务
仍然只产生一个最终文件。
_Avoid_: m3u8 (as the domain term), stream

**HLS variant**:
master playlist 中的一个码率版本；默认选择最高 `BANDWIDTH` 的变体。
_Avoid_: Quality, rendition (unqualified)

**HLS segment**:
media playlist 中的一个分片，由 HlsBackend 内部管理，不是 `FileItem`，也不是
core 的 `Segment`（HTTP 字节区间）。
_Avoid_: Segment, chunk (unqualified)

**HLS key**:
AES-128 密钥，由 `#EXT-X-KEY` 指定并作用于其后的分片；运行时从 URI 获取，
只驻留内存，不写入 sidecar。
_Avoid_: Encryption key, DRM key

**HLS live capture**:
对没有 `#EXT-X-ENDLIST` 的 media playlist 做有界录制；必须提供
`live_duration_secs` 或 `live_segments`，按 target duration 轮询并按 media
sequence 去重。不可续传。
_Avoid_: Live stream, DVR

**DASH**:
基于 MPD（XML）的流媒体协议；DASH 任务与 HLS 一样只产生一个最终文件。
首期只支持静态、单 Period 的点播清单。
_Avoid_: mpd (as the domain term), stream

**DASH representation**:
MPD 中一组同编码参数的分片集合；默认选择最高 `bandwidth` 的视频
Representation。
_Avoid_: Variant (unqualified)

**DASH segment**:
MPD 描述的 init 或媒体分片，由 DashBackend 内部管理，不是 `FileItem`。
_Avoid_: Segment, chunk (unqualified)

**DASH live capture**:
对 `type="dynamic"` 的单 Period MPD 做有界录制；支持 SegmentTemplate +
SegmentTimeline，以及带 `availabilityStartTime` 的 constant-duration
SegmentTemplate。按 `minimumUpdatePeriod` 轮询、按 presentation time 去重，
达到 live 限制、MPD 变为 static 或 Period 结束时停止。不可续传。
_Avoid_: DASH stream, DVR

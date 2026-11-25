# HLS 直播只做有界录制，不支持无限任务与直播续传

## Context

HLS 直播 media playlist 没有 `#EXT-X-ENDLIST`，分片窗口会持续向前滑动，
新分片按 `#EXT-X-MEDIA-SEQUENCE` 递增。shaul 的任务模型假设一次下载会结束并
产出一个最终文件；如果直接支持"一直录制到用户取消"，取消时后端只拿到
`Error::Cancelled`，没有机会把已下载分片收尾成可播放文件，任务状态也会变成
cancelled，语义含糊。

同时，直播没有稳定的完整 playlist 指纹，分片会过期；沿用 VOD 的分片级续传
会在窗口滑动后产生缺口。首期需要明确停止条件和续传边界。

## Decision

- 解析器接受没有 `#EXT-X-ENDLIST` 的 media playlist，但要求：
  - `#EXT-X-TARGETDURATION` 存在且大于零；
  - 不得标记 `#EXT-X-PLAYLIST-TYPE:VOD`（VOD 必须有 ENDLIST）；
  - 仍复用既有 AES-128、单 `#EXT-X-MAP`、BYTERANGE 和加密 init 校验。
- `#EXT-X-ENDLIST` 出现后一律视为完整列表（包括 `PLAYLIST-TYPE:EVENT`），
  走点播下载路径；因此结束的 EVENT 直播也能作为 VOD 下载。
- `TaskOptions` 新增两个公开字段（serde 默认，向后兼容）：
  - `live_duration_secs: Option<u64>`：按 `#EXTINF` 累计时长停止；
  - `live_segments: Option<usize>`：按已录制分片数停止。
  CLI 对应 `--live-duration`（支持 `30m`/`1h` 等）和 `--live-segments`。
  两者同时存在时，任意一个达到即停止；都没有时后端拒绝运行并给出明确错误。
- 后端在运行时重新抓取 playlist，从当前窗口开始录制：
  - 初次下载当前窗口的所有分片，之后每隔 `TARGETDURATION`（最少 1 秒）重新
    抓取，只下载 `MEDIA-SEQUENCE` 大于已记录序列的新分片；
  - 用分片文件名的绝对 sequence 去重；如果新窗口的第一个 sequence 跳过未录制
    分片，直接报错，避免静默缺口；
  - 出现 `#EXT-X-ENDLIST` 时下载完当前批次并正常结束；
  - 分片按 sequence 顺序在最后拼接，`TARGETDURATION` 只决定轮询间隔。
- 直播分片按顺序下载（不再按 playlist 索引并发），以便在达到精确的时长/分片
  限制时立即停止；连接预算、限速、代理、认证和重试仍由共享 `HttpBackend`
  生效。
- init 段只下载一次；直播过程中 init 的 URL、BYTERANGE 或密钥发生变化时报错。
  分片 AES-128 密钥按 URL 缓存，密钥轮换和按 sequence 推导 IV 继续生效。
- 直播不做续传：每次运行清空 `<output>.shaul.hls.parts/`，从运行时窗口重新
  开始。`FileResume.metadata["hls.live"] = "true"` 标记直播录制；
  `clean --orphans` 因此也会清理**失败**直播任务的临时目录和拼接 `.part`
  （普通 VOD 任务仍保留失败数据用于续传）。
- 输出仍在停止后一次性拼接并原子 rename；录制过程中没有最终文件。取消或
  失败时保留临时分片供人工检查，`clean --orphans` 可删除。
- `--mirror` 和最终输出的内容寻址缓存仍不适用于 HLS；`--verify` 只校验
  收尾后的完整输出。

## Out of scope (phase 1)

- DASH 动态清单（`type="dynamic"`）与 DASH 直播（由
  [ADR 0018](0018-dash-live.md) 单独处理）；
- 取消时自动收尾并生成部分输出；
- 直播续传、跨运行拼接、EPG/时间平移窗口；
- 交替音轨、SAMPLE-AES、ffmpeg 转封装；
- 无限录制（必须提供时长或分片上限）。

## Alternatives

- **录制到用户取消为止**：取消路径无法产出最终文件，且 `run` worker 可能长
  期占用任务；首期不采用。
- **只在 playlist 出现 ENDLIST 后下载**：会漏掉直播窗口，退化为 VOD。
- **先缓存整个窗口再下载**：窗口会滑动，分片可能在下完前过期；逐批下载更
  可靠。
- **直播也做分片级续传**：窗口滑动后 sequence 不连续，缺口检测和续传语义
  复杂；首期明确不做。

## Consequences

- 直播录制有明确终点，任务最终状态是 completed 或 failed。
- 录制从 `run` 时的 playlist 窗口开始，可能包含开始前的一小段缓冲；不会
  录制 add 之前的窗口。
- 轮询间隔由 `TARGETDURATION` 决定；实际录制时长可能比 `--live-duration`
  略长，最多一个分片。
- 失败的直播任务不会自动重试成续传；重新运行会从新的窗口开始。
- `TaskOptions`/`ResolvedOptions` 增加两个字段，属于 minor API 变更。

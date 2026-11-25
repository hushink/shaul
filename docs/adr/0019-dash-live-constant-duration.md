# DASH 常量时长动态模板按可用时间窗口录制

## Context

`type="dynamic"` 的 DASH 清单不一定携带 `SegmentTimeline`。直播 profile 经常
只给出 `SegmentTemplate@duration`、`startNumber` 和
`availabilityStartTime`/`publishTime`，分片编号不会在 MPD 中逐条列出。客户端
必须根据时间属性推算当前可用的分片窗口。

首期动态支持（[ADR 0018](0018-dash-live.md)）只覆盖 SegmentTimeline；本次为
constant-duration 模板补充同样的有界录制能力，仍然保持单 Period、单
Representation、无 DRM。

## Decision

- 支持 `type="dynamic"` + `SegmentTemplate`（`media`、`duration`、
  `timescale`、`startNumber`、`presentationTimeOffset`）且没有
  `SegmentTimeline` 的清单。
- 时间模型：
  - 必须有 `availabilityStartTime`；`publishTime` 存在时优先作为参考时间，
    避免依赖本机时钟；缺失时使用系统时间。
  - `elapsed = reference - availabilityStartTime - Period@start`。
  - 分片时长 `d = duration / timescale`；已完成的最新分片索引为
    `floor(elapsed / d) - 1`。
  - 初始从 live edge 开始：`playhead = elapsed - suggestedPresentationDelay`，
    从 playhead 对应的分片开始，不默认下载整个 DVR 窗口。
- 录制循环：
  - 每次按 `minimumUpdatePeriod`（1–60 秒，缺省 5 秒）重新抓取 MPD；
  - 从上次的下一分片编号下载到最新可用分片，中间不跳过；
  - `timeShiftBufferDepth` 决定窗口下界；缺失时按 30 秒估算过期，
    已过期则报错，避免静默缺口；
  - 固定首次选中的 Representation id，MIME/codecs/init 变化时报错。
- 结束条件：
  - 达到 `live_duration_secs` / `live_segments` 限制；
  - MPD 变为 `type="static"`：用静态清单的分片按 timing 过滤，只补录已录末尾
    之后的部分，然后收尾；
  - `Period@duration` 到期：`DynamicTemplate` 返回 ended，下载完最后一批后
    停止。
- 输出、临时目录、`clean --orphans`、失败/取消语义与
  [ADR 0018](0018-dash-live.md) 一致；constant-duration 动态录制同样不支持
  续传。
- 公开 API：`SelectedRepresentation.dynamic: Option<DynamicTemplate>`；
  `DynamicTemplate` 的字段保持 `pub(crate)`，后端通过
  `segments_from(from)` 获取窗口。`shaul-dash` 仍是新 crate，属于 additive
  API。

## Out of scope (phase 1)

- 多 Period、DRM/CENC、字幕和交替音轨；
- `availabilityTimeOffset`/`availabilityTimeComplete` 和低延迟分块；
- 显式“从 DVR 窗口起点录制”的选项；
- 动态中切换 Representation 或音视频编码；
- 取消时自动收尾。

## Alternatives

- **用 `$Number$`/URL 去重**：窗口滑动后编号不稳定；用 availability 时间
  计算连续编号更可靠。
- **始终依赖系统时钟**：`publishTime` 可用时更贴近 MPD 发布者的可用性判断；
  缺失时才退回本机时钟。
- **从 0 号分片开始**：直播可能已运行很久，会下载无界历史；从 live edge
  开始更符合录制语义。
- **只支持 SegmentTimeline**：会漏掉大量 constant-duration 直播 MPD；本次
  补上该常见形态。

## Consequences

- `publishTime` 缺失时，本机时钟偏差可能导致等待或提前结束。
- 初始录制不包含 live edge 之前的 DVR 缓冲；用户可以等待新分片。
- 过期分片会明确报错，而不是静默产出缺口。
- 静态切换时只补录已录末尾之后的分片；之前的静态历史不会回填。

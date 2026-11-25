# DASH 动态清单支持有界录制，首期只覆盖 SegmentTimeline

## Context

`type="dynamic"` 的 MPD 不会列出固定分片总数，`SegmentTimeline` 会随直播
窗口增长/滑动，MPD 需要按 `minimumUpdatePeriod` 重新抓取。与 HLS 直播不同，
DASH 没有 ENDLIST；直播结束可能表现为 MPD 变成 `type="static"`，也可能只是
Period/Representation 消失。

DASH 还支持多种动态寻址方式：constant-duration SegmentTemplate 依赖
`availabilityStartTime` 和墙上时钟推算可用分片；SegmentList/SegmentBase 在
动态场景下语义复杂。首期需要选择一种可验证、可去重的子集，避免实现完整的
DASH 时钟模型。

## Decision

- 解析器接受 `type="dynamic"`，继续要求单 Period。动态清单新增：
  - `minimumUpdatePeriod`（ISO 8601）作为轮询间隔；
  - `is_dynamic()` 与 `SelectedRepresentation.live` 供后端判断；
  - `SelectedRepresentation.minimum_update_period_secs`。
- 动态清单首期只支持：
  - `SegmentTemplate` + `SegmentTimeline`；
  - `$Time$` 和 `$Number$` 模板替换（按 timeline 顺序展开）；
  - 单 Period、单选中的 Representation，最高码率视频优先规则不变。
  constant-duration 动态模板、动态 SegmentList/SegmentBase、多 Period 和
  DRM 继续报错。
- `SegmentRef` 增加 `timing: Option<SegmentTiming>`（start/duration/
  timescale）。动态录制的去重和顺序以 **presentation time** 为准，而不是
  URL 或 `$Number$`，这样窗口滑动、编号变化时不会重复或错位。
- 后端复用 `--live-duration` / `--live-segments` 语义：
  - 从运行时 MPD 列出的窗口开始，逐分片下载；
  - 每 `minimumUpdatePeriod`（限制在 1–60 秒，缺省 5 秒）重新抓取 MPD；
  - 用稳定 representation id 重新选择，避免新出现的高码率变体改变录制；
  - 时间线必须连续：新分片的 start 必须等于上一条的 end，否则报缺口/重叠
    错误；
  - 达到时长/分片上限，或 MPD 变成 `type="static"` 时停止并收尾。
- init 段只下载一次；init URL/范围或 MIME 类型在录制期间变化时报错。
  动态录制不做续传：每次运行清空 `<output>.shaul.dash.parts/`，从当前窗口
  重新开始。`FileResume.metadata["dash.live"] = "true"`，`clean --orphans`
  会同时清理失败动态任务的临时目录和拼接 `.part`。
- 输出仍在停止后一次性拼接并原子 rename；`--mirror` 和最终输出的内容寻址
  缓存不适用，`--verify` 只校验收尾后的完整输出。
- 新增公开项：`SegmentTiming`、`SegmentRef.timing`、
  `SelectedRepresentation.live`/`minimum_update_period_secs`、
  `DashManifest::is_dynamic`/`select_representation`。`shaul-dash` 是新
  crate，属于 additive API。

## Out of scope (phase 1)

- constant-duration 动态 MPD（后续由
  [ADR 0019](0019-dash-live-constant-duration.md) 以可用时间窗口加入）、
  `availabilityStartTime`/`publishTime` 时钟推算；
- 动态 SegmentList/SegmentBase、多 Period、Period 切换（静态多 Period 的兼容
  子集见 [ADR 0020](0020-dash-multi-period.md)）；
- DRM/CENC、`ContentProtection`、多音轨/字幕；
- 取消时自动收尾、直播续传、跨运行拼接；
- DASH 的 `$Bandwidth$` 动态变化、Representation 切换。

## Alternatives

- **用 URL 或 `$Number$` 去重**：窗口滑动时编号可能变化，会重复下载或漏分片；
  用 presentation time 更稳定。
- **支持 constant-duration 动态模板**：需要完整的 availabilityStartTime 与
  时钟模型，首期风险高；明确报错更安全。
- **录制到用户取消为止**：取消路径无法产出最终文件，任务状态语义含糊；沿用
  HLS 直播的有界限制。
- **每次重新选择最高码率**：直播中途切换 Representation 会破坏初始化段和
  编码连续性；固定首次选中的 id。

## Consequences

- 动态 DASH 录制有明确终点，任务最终状态是 completed 或 failed。
- 需要 MPD 的 SegmentTimeline 使用稳定的 presentation time；时间戳重置或
  滑动窗口丢失未录制分片时会明确失败，而不是静默产出缺口。
- 录制从 `run` 时的窗口开始，不录制 add 之前的窗口；时长按 timeline
  duration 累计，可能比 `--live-duration` 多一个分片。
- 不支持 constant-duration 动态 MPD；这类清单会得到明确的 SegmentTimeline
  要求错误。

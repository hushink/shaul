# DASH 多 Period 只拼接兼容的静态 Period

## Context

静态 DASH 可以用多个 `Period` 描述顺序播放的媒体段（例如节目 + 广告 + 节目）。
用户希望一个任务仍然产出一个文件，但多 Period 直接字节拼接有真实风险：

- 每个 Period 可能有不同的 `Representation` 编码参数；
- fMP4 每个 Period 可能带不同的初始化段；把多个 `moov` 拼进一个文件通常
  不可播放；
- 真正的通用方案需要转封装/混流，而 shaul 不内置 ffmpeg。

因此需要一个明确的安全子集，而不是假装所有多 Period 清单都能直接下载。

## Decision

- 静态（`type="static"`）清单接受多个 `Period`；动态多 Period 仍直接报错。
- 选择规则：
  - 每个 Period 内按既有规则选择最高码率视频（或回退音频）；
  - 后续 Period 优先复用第一个 Period 选中的 Representation `id`；
  - 如果该 id 不存在，则在该 Period 内重新选择最高码率候选；
  - `--include`/`--exclude` 仍然逐 Period 生效。
- 兼容性检查：
  - 所有 Period 的默认扩展名必须一致；
  - MIME 类型和 codecs 在双方都存在时必须一致；
  - 所有 Period 要么都没有初始化段，要么都使用**完全相同**的
    URL/字节范围初始化段；
  - 混用“有/无初始化段”，或初始化段不同，直接报错并说明需要转封装。
- 输出：按 Period 顺序展开分片；公共初始化段只写一次；之后顺序拼接所有
  分片，复用现有 `.part` 原子 rename、续传、校验和解压 finalizer。
  `SelectedRepresentation.parts` 暴露每个 Period 的 init/分片，
  `segments` 同时保留扁平列表供现有后端使用。
- Period 时长推导用于 constant-duration SegmentTemplate：
  `Period@duration`；否则用下一个 `Period@start` 减当前 start；最后一个
  Period 用 `mediaPresentationDuration` 减 start。无 `start` 的首个 Period
  默认从 0 开始。
- 续传指纹仍使用 MPD 文本、Representation id（多 Period 时为 id 组合）和
  扁平分片数量；MPD 变化会重新开始。
- 动态多 Period、不同初始化段、不同 codecs、不同 MIME 和通用转封装不在
  本期范围内。

## Alternatives

- **允许不同 init 段直接拼接**：fMP4 通常不可播放，不能作为默认行为。
- **只允许用户选择单个 Period**：更保守，但不能满足“一个任务产出一个完整
  文件”的常见需求；可作为后续 `--period` 选项补充。
- **内置 ffmpeg 转封装**：依赖和许可证成本高，且 shaul 的一贯策略是不内置
  外部媒体工具。
- **动态多 Period 同步支持**：时间线/时钟和 Representation 切换复杂度高，
  留作后续。

## Consequences

- 无初始化段（TS/简单拼接）和共享 init 的 fMP4 多 Period 可以直接下载。
- 初始化段不同、codecs 不同或 MIME 不同的清单会明确失败，需要用户先转封装
  或选择单个 Period。
- 如果后续 Period 缺少首个 id，质量可能切换；兼容性检查会阻止编码参数变化。
- `SelectedRepresentation.parts` 和 `RepresentationPart` 是新增公开项；
  `shaul-dash` 仍是新 crate，属于 additive API。

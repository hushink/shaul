# DASH 是独立可选 crate，首期支持静态单 Period 点播

## Context

DASH 用 MPD（XML）描述 Period、AdaptationSet、Representation 和分片寻址。
它与 HLS 共享"一个任务产出一个文件、分片由后端内部管理"的约束（见
[ADR 0013](0013-hls-backend.md)），但寻址模型不同：MPD 是 XML，存在
BaseURL 继承、SegmentTemplate/SegmentList/SegmentBase 三种寻址方式和
`$Number$`/`$Time$` 模板替换；大多数 MPD 还包含多 Period、多音轨和 DRM。

如果把 DASH 塞进 `shaul-hls`，XML 依赖和 DASH 专用解析会进入 HLS 用户；
独立 crate 可以保持 feature 和依赖边界，并与 `shaul-ftp`/`shaul-hub` 的
"一协议一 crate"结构一致。

## Decision

- 新增可选 crate `shaul-dash`，facade feature 为 `dash`，默认关闭；CLI 启用。
- `DashParser` 只接受 HTTP(S) 且路径以 `.mpd` 结尾的链接，抓取 MPD 后解析为
  `ResourceKind::Custom { protocol: "dash" }`，包含一个最终的
  `ResourceFile`。`file.url` 是重定向后的 MPD URL，也是续传键。
- 首期只支持静态、单 Period、单一 Representation：
  - 拒绝 `type="dynamic"`、多个 Period、`ContentProtection`（DRM）和
    `xlink:href` 展开；
  - 优先选择视频 AdaptationSet，其次音频，最后到其他音视频类型；
  - 同一候选集合内选择 `bandwidth` 最高的 Representation；
  - `--include`/`--exclude` 的 glob 匹配 AdaptationSet/Representation 的
    `id`、`mimeType`、`codecs`、`lang`、宽高和 `bandwidth`；
  - 字幕、缩略图、非音视频 AdaptationSet 被忽略。
- 分片寻址（Representation 级覆盖 AdaptationSet 级）：
  - `SegmentTemplate`：`$RepresentationID$`、`$Number$`、`$Time$`、
    `$Bandwidth$` 和 `$$`；支持 `%0Nd` 补零；SegmentTimeline（`S@t/d/r`，
    拒绝负 `r`）或 constant `duration`，缺失总时长时可用 `endNumber`；
    `startNumber` 默认 1；
  - `SegmentList`：`Initialization@sourceURL/@range` 与
    `SegmentURL@media/@mediaRange`；
  - `SegmentBase` 或仅 BaseURL：整个对象作为一个分片下载（init 已内嵌），
    不解析 `sidx`；
  - BaseURL 按 MPD → Period → AdaptationSet → Representation 逐级解析；
  - 字节范围复用 `HttpBackend::download_range`。
- 分片通过共享 `HttpBackend` 并发下载，复用代理、认证头、超时、重试、
  限速和连接预算。playlist、init 和分片跨源时默认剥离敏感头，除非开启
  `allow_cross_origin_auth`。
- 输出：init 加分片按顺序拼接为 `<output>.shaul.part`，fsync 后原子 rename。
  默认扩展名按 mimeType 推断：`webm` 用 `.webm`，其余音视频用 `.mp4`；
  `--output` 优先。不内置 ffmpeg，不转封装、不混流。
- 续传：MPD 文本的 SHA-256、representation id 和分片数量组成指纹，写入
  `FileResume.metadata["dash.state"]`；已完成分片文件按序号复用；临时目录
  为 `<output>.shaul.dash.parts`，记录在 `metadata["dash.temp_dir"]`。
  指纹或分片数量变化时删除临时目录重新开始。`clean --orphans` 会为已完成
  或已取消的任务删除该目录和拼接 `.part`。
- `--mirror` 和最终输出的内容寻址缓存不适用于 DASH；`--verify` 需要用户
  提供最终文件的 checksum。
- 新增依赖 `quick-xml`（MIT，MSRV 1.79），只在 `shaul-dash` 中启用。

## Out of scope (phase 1)

- live/dynamic MPD（后续由 [ADR 0018](0018-dash-live.md) 以 SegmentTimeline
  有界录制方式加入）、多 Period（后续由
  [ADR 0020](0020-dash-multi-period.md) 以兼容子集加入）、DRM/CENC、
  `ContentProtection`、`xlink:href`；
- 交替音轨/多音轨混流、字幕、缩略图、trick-mode；
- 解析 `sidx`、分片内字节级续传、ffmpeg 转封装；
- `--mirror`、内容寻址缓存、DASH 输出的解压/校验自动化。

## Alternatives

- **把分片暴露为 `FileItem`**：污染任务文件列表、统计、校验和清理语义；
  分片不是用户要的文件。
- **放进 `shaul-hls`**：会让 HLS 用户承担 XML 依赖和 DASH 解析代码，也模糊
  两个协议的 feature 边界。
- **首期解析 `sidx`**：需要实现 MP4 box 解析和索引校验；整对象下载覆盖
  on-demand profile 的常见情况，后续再增量。
- **内置 ffmpeg 混流**：依赖和许可证成本高；首期只做字节拼接。

## Consequences

- DASH 任务只产生一个输出文件，不暴露分片。
- 视频与音频分离的 MPD 首期只下载选中的视频 Representation，可能没有音轨；
  与 HLS 交替音轨的限制一致。
- 单 Period 限制覆盖大多数点播 MPD；超出范围的 MPD 会得到明确错误。
- `shaul-hls` 与 `shaul-dash` 暂时各自维护播放列表抓取和分片拼接代码；
  后续可在不改变公开 API 的前提下抽出公共流式后端。

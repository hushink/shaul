# shaul-dash

[![crates.io](https://img.shields.io/crates/v/shaul-dash.svg)](https://crates.io/crates/shaul-dash)
[![docs.rs](https://docs.rs/shaul-dash/badge.svg)](https://docs.rs/shaul-dash)

[English](README.md) | **简体中文**

[shaul](https://crates.io/crates/shaul) 的 DASH 后端。

本 crate 为 HTTP(S) `.mpd` 链接提供可选的 parser 与 backend。它选择最高码率
的视频 Representation，下载 init 与媒体分片，并拼接成一个输出文件；支持静态
点播清单和有界动态（直播）录制。分片是内部细节：任务仍然只产生一个文件。

## 用法

大多数应用应依赖带 `dash` feature 的 `shaul` 门面 crate：

```toml
[dependencies]
shaul = { version = "1.2", features = ["dash"] }
```

```console
$ shaul download https://example.com/video/manifest.mpd --connections 8
```

可用 `--include` / `--exclude` glob 按 `id`、`mimeType`、`codecs`、`lang`、
分辨率和 `bandwidth` 选择 Representation：

```console
$ shaul download https://example.com/video/manifest.mpd --include '*1080*'
```

用时长或分片上限录制动态直播清单：

```console
$ shaul download https://example.com/live/manifest.mpd --live-duration 10m
$ shaul download https://example.com/live/manifest.mpd --live-segments 300
```

`DashParser` 与 `DashBackend` 也可以注册到自定义 router。后端与协议解耦：
`shaul-core` 不依赖 DASH。

## 支持范围

- 静态（`type="static"`）多 Period 清单，以及只有一个 Period 的动态
  （`type="dynamic"`）清单
- 默认选择最高码率的视频 Representation；纯音频清单回退到最佳音频
- 静态多 Period 清单在 MIME/codecs 一致、且所有 Period 都没有 init 或使用
  同一 init 时按顺序拼接；公共 init 只写一次
- `SegmentTemplate`：`$Number$`、`$Time$`、`$RepresentationID$`、
  `$Bandwidth$`、`%0Nd` 补零、constant `duration`、`endNumber` 或
  `SegmentTimeline`（非负重复次数）
- 动态清单支持 `SegmentTemplate` + `SegmentTimeline`，也支持 constant
  duration 的 `SegmentTemplate` + `availabilityStartTime`。后端按
  `minimumUpdatePeriod` 轮询、按 presentation time 去重，并在达到时长/分片
  上限、清单变为静态或 Period 结束时停止。constant-duration 录制从 live
  edge（按 `suggestedPresentationDelay` 调整）开始
- `SegmentList`，包括可选 `Initialization` 与 `mediaRange` 字节范围
- `SegmentBase`/BaseURL 单对象 Representation（下载整个对象，不解析 `sidx`）
- BaseURL 沿 MPD → Period → AdaptationSet → Representation 继承
- 通过共享 HTTP 后端并发下载分片，代理、认证、超时、重试、限速和连接预算
  都生效
- 点播分片级续传：暂停或失败后复用已完成分片文件（动态录制不可续传）

## TODO / future work

以下项目尚未实现，不影响当前已支持的能力。DASH 专属的后续工作集中记录在
此 crate README，而不是根 roadmap：

- 动态多 Period 清单，以及直播过程中的 Period 切换
- init/MIME/codecs 不一致的静态多 Period 清单（这些需要转封装）
- 在无法转封装时，用 `--period` 只下载单个 Period
- `availabilityTimeOffset`、低延迟分块和显式从 DVR 窗口起点录制
- DRM（`ContentProtection`、CENC）与 `xlink:href`
- 字幕、缩略图、交替音轨混流和多音轨选择
- ffmpeg 转封装
- `sidx` 解析和分片内字节级续传
- 最终输出的内容寻址缓存
- `--mirror` 支持（当前 DASH 任务会忽略）

输出扩展名：WebM Representation 为 `.webm`，其余为 `.mp4`。`--output` 优先。

## 说明

- 后端在下载时重新抓取清单，并用其 SHA-256、选中的 Representation id 和
  分片数量作为续传指纹。
- 已完成分片存放在 `<output>.shaul.dash.parts/`，通过
  `<output>.shaul.part` 拼接；成功后删除临时目录。已完成或已取消任务遗留的
  目录及拼接 `.part` 可由 `shaul clean --orphans` 清理。
- 跨源清单和分片 URL 默认剥离敏感请求头，除非开启
  `allow_cross_origin_auth`。
- 当清单使用独立音频 AdaptationSet 时，视频 Representation 会产出无音轨的
  视频文件。
- 静态多 Period 清单通过 `SelectedRepresentation.parts` 暴露每个 Period 的
  init/分片；后端把公共 init 写一次，再顺序拼接扁平分片。
- 动态录制每次都从新的临时目录和当前窗口开始。constant-duration 录制优先用
  `publishTime` 作为参考时钟（缺失时用本机时钟），从 live edge 开始，并用
  `timeShiftBufferDepth`（或 30 秒的回看）判断分片是否过期。取消或失败会
  保留已录分片供检查；`clean --orphans` 也会清理失败动态任务的临时文件。输出
  只在录制停止后一次性收尾。

## 文档

- [项目 README](https://github.com/hushink/shaul/blob/main/README.zh-CN.md)
- [DASH ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0016-dash-backend.md)
- [直播 ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0018-dash-live.md)
- [常量时长直播 ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0019-dash-live-constant-duration.md)
- [多 Period ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0020-dash-multi-period.md)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API reference](https://docs.rs/shaul-dash)

# shaul-dash

[![crates.io](https://img.shields.io/crates/v/shaul-dash.svg)](https://crates.io/crates/shaul-dash)
[![docs.rs](https://docs.rs/shaul-dash/badge.svg)](https://docs.rs/shaul-dash)

[English](README.md) | **简体中文**

[shaul](https://crates.io/crates/shaul) 的 DASH 后端。

本 crate 为 HTTP(S) `.mpd` 链接提供可选的 parser 与 backend。它从静态点播
清单中选择最高码率的视频 Representation，并发下载 init 与媒体分片，并拼接
成一个输出文件。分片是内部细节：任务仍然只产生一个文件。

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

`DashParser` 与 `DashBackend` 也可以注册到自定义 router。后端与协议解耦：
`shaul-core` 不依赖 DASH。

## 支持范围

- 静态（`type="static"`）且只有一个 Period 的清单
- 默认选择最高码率的视频 Representation；纯音频清单回退到最佳音频
- `SegmentTemplate`：`$Number$`、`$Time$`、`$RepresentationID$`、
  `$Bandwidth$`、`%0Nd` 补零、constant `duration`、`endNumber` 或
  `SegmentTimeline`（非负重复次数）
- `SegmentList`，包括可选 `Initialization` 与 `mediaRange` 字节范围
- `SegmentBase`/BaseURL 单对象 Representation（下载整个对象，不解析 `sidx`）
- BaseURL 沿 MPD → Period → AdaptationSet → Representation 继承
- 通过共享 HTTP 后端并发下载分片，代理、认证、超时、重试、限速和连接预算
  都生效
- 分片级续传：暂停或失败后复用已完成分片文件

暂不支持：

- 直播/动态清单和多 Period
- DRM（`ContentProtection`、CENC）与 `xlink:href`
- 字幕、缩略图、交替音轨混流和多音轨选择
- ffmpeg 转封装
- `sidx` 解析和分片内字节级续传
- 最终输出的内容寻址缓存
- `--mirror`（DASH 任务会忽略）

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

## 文档

- [项目 README](https://github.com/hushink/shaul/blob/main/README.zh-CN.md)
- [DASH ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0016-dash-backend.md)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API reference](https://docs.rs/shaul-dash)
